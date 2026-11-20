//! Startup disk scanning for macOS.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use sys_manager_common::disk::{scan, CancelToken, ScanOptions, ScanSnapshot};

use crate::items::is_locked;

/// Paths the startup disk scan never enters.
///
/// `/System/Volumes/Data` holds the data volume that the firmlinks at `/` already expose
/// (`/Users`, `/Library`, and so on), so entering it would count everything twice. `/Volumes`
/// holds other mounted disks. The startup disk appears there only as a symlink back to `/`.
const SKIPPED_PATHS: [&str; 2] = ["/System/Volumes/Data", "/Volumes"];

/// Options for scanning the startup disk from `/`, charging each file the blocks it occupies.
pub fn startup_disk_options() -> ScanOptions {
    ScanOptions {
        skip: SKIPPED_PATHS.iter().map(PathBuf::from).collect(),
        measure: allocated_size,
        is_locked,
        ..ScanOptions::new("/")
    }
}

/// Bytes the file occupies on disk, which is what removing it would free.
pub fn allocated_size(metadata: &Metadata) -> u64 {
    metadata.blocks() * 512
}

/// A scan running on a background thread. Dropping it cancels the scan.
pub struct DiskScan {
    latest: Arc<Mutex<Option<ScanSnapshot>>>,
    cancel: CancelToken,
}

impl DiskScan {
    pub fn start(options: ScanOptions) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let cancel = CancelToken::default();
        let shared = Arc::clone(&latest);
        let token = cancel.clone();
        thread::Builder::new()
            .name("disk-scan".to_string())
            .spawn(move || {
                scan(&options, &token, |snapshot| {
                    if let Ok(mut slot) = shared.lock() {
                        *slot = Some(snapshot.clone());
                    }
                });
            })
            .expect("failed to spawn the disk scan thread");
        Self { latest, cancel }
    }

    /// Stops the scan. The final snapshot reports the state as cancelled.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Returns the newest snapshot that has not been taken yet. Older unread snapshots are dropped.
    pub fn take_latest(&self) -> Option<ScanSnapshot> {
        self.latest.lock().ok()?.take()
    }
}

impl Drop for DiskScan {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{current_uid, set_locked, test_support::UnlockOnDrop};
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    use std::time::{Duration, Instant};
    use sys_manager_common::disk::ScanState;
    use tempfile::TempDir;

    /// Restores a folder's permissions on drop so the temporary directory can be removed.
    struct Restore(PathBuf);

    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn folders_without_access_are_counted_and_the_rest_is_ranked() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("readable.bin"), vec![0u8; 20_000]).unwrap();
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden.bin"), vec![0u8; 90_000]).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = Restore(locked);

        let snapshot = scan(&ScanOptions::new(root), &CancelToken::default(), |_| {});

        assert_eq!(snapshot.state, ScanState::Completed);
        assert_eq!(snapshot.skipped_no_access, 1);
        assert_eq!(snapshot.other_errors, 0);
        let paths: Vec<&Path> = snapshot
            .entries
            .iter()
            .map(|entry| entry.path.as_path())
            .collect();
        assert_eq!(paths, [root.join("readable.bin").as_path()]);
    }

    #[test]
    fn symlinks_are_not_followed() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/big.bin"), vec![0u8; 50_000]).unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();

        let snapshot = scan(&ScanOptions::new(root), &CancelToken::default(), |_| {});

        let paths: Vec<PathBuf> = snapshot
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(paths, [root.join("real"), root.join("real/big.bin")]);
    }

    #[test]
    fn allocated_size_charges_the_blocks_a_file_occupies() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("tiny.txt");
        fs::write(&file, b"x").unwrap();

        let metadata = fs::metadata(&file).unwrap();

        assert_eq!(metadata.len(), 1);
        assert!(allocated_size(&metadata) > metadata.len());
    }

    #[test]
    fn scan_reports_the_lock_flag_of_files_and_folders() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let locked_file = root.join("locked.bin");
        fs::write(&locked_file, vec![0u8; 5_000]).unwrap();
        let locked_folder = root.join("locked-folder");
        fs::create_dir(&locked_folder).unwrap();
        fs::write(locked_folder.join("inside.bin"), vec![0u8; 7_000]).unwrap();
        let open_file = root.join("open.bin");
        fs::write(&open_file, vec![0u8; 6_000]).unwrap();
        let uid = current_uid();
        set_locked(&locked_file, true, uid).unwrap();
        set_locked(&locked_folder, true, uid).unwrap();
        let _unlock_file = UnlockOnDrop(locked_file.clone());
        let _unlock_folder = UnlockOnDrop(locked_folder.clone());

        let options = ScanOptions {
            is_locked,
            ..ScanOptions::new(root)
        };
        let snapshot = scan(&options, &CancelToken::default(), |_| {});

        let locked_of = |path: &Path| {
            snapshot
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap()
                .locked
        };
        assert!(locked_of(&locked_file));
        assert!(locked_of(&locked_folder));
        assert!(!locked_of(&open_file));
    }

    #[test]
    fn background_scan_publishes_a_completed_snapshot() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.bin"), vec![0u8; 4_000]).unwrap();
        let disk_scan = DiskScan::start(ScanOptions::new(dir.path()));

        let deadline = Instant::now() + Duration::from_secs(10);
        let snapshot = loop {
            if let Some(snapshot) = disk_scan.take_latest() {
                if snapshot.state == ScanState::Completed {
                    break snapshot;
                }
            }
            assert!(Instant::now() < deadline, "scan did not finish in time");
            thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(snapshot.entries.len(), 1);
    }
}

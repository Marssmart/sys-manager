//! Changes Sys Manager makes to disk items: moving them to the Trash, setting their lock flag, and opening them.

use std::ffi::CString;
use std::fs::{self, Metadata};
use std::io;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSError, NSFileManager, NSString, NSURL};

use sys_manager_common::actions::{ActionError, ItemAction};

/// Folders under which Sys Manager never removes or locks anything.
const PROTECTED_PATHS: [&str; 4] = ["/System", "/usr", "/bin", "/sbin"];

/// The BSD flag behind `chflags uchg`. While it is set, the item can't be changed, renamed or deleted.
const LOCK_FLAG: u32 = libc::UF_IMMUTABLE;

/// Whether the item carries the lock flag.
pub fn is_locked(metadata: &Metadata) -> bool {
    metadata.st_flags() & LOCK_FLAG != 0
}

/// Whether `path` is the volume root or lies under a system folder that Sys Manager leaves alone.
pub fn is_protected(path: &Path) -> bool {
    path.parent().is_none()
        || PROTECTED_PATHS
            .iter()
            .any(|protected| path.starts_with(protected))
}

/// The refusal that applies before any file system access, so the disk list can explain it before asking for confirmation.
pub fn refusal_before_removal(path: &Path, locked: bool) -> Option<ActionError> {
    if is_protected(path) {
        Some(ActionError::Protected)
    } else if locked {
        Some(ActionError::Locked)
    } else {
        None
    }
}

/// The effective user ID of this process.
pub fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and only reads the process credentials.
    unsafe { libc::geteuid() }
}

/// Moves an item to the Trash, so that Finder's Put Back can restore it.
pub trait Trash {
    fn move_to_trash(&self, path: &Path) -> io::Result<()>;
}

/// The system Trash, through `NSFileManager`.
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn move_to_trash(&self, path: &Path) -> io::Result<()> {
        let url = file_url(path)?;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, None)
            .map_err(|error| ns_error(&error))
    }
}

/// A file URL for `path`. Names that aren't UTF-8 are refused rather than converted, so a different file is never named.
fn file_url(path: &Path) -> io::Result<Retained<NSURL>> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the name isn't valid UTF-8"))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
}

/// Converts a Trash failure, keeping its kind when the error code says what went wrong.
fn ns_error(error: &NSError) -> io::Error {
    let kind = match (error.domain().to_string().as_str(), error.code()) {
        ("NSCocoaErrorDomain", 257 | 513) | ("NSPOSIXErrorDomain", 1 | 13) => {
            io::ErrorKind::PermissionDenied
        }
        ("NSCocoaErrorDomain", 4) | ("NSPOSIXErrorDomain", 2) => io::ErrorKind::NotFound,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error.localizedDescription().to_string())
}

/// Moves the item at `path` to the Trash after checking that it can be moved.
///
/// A symlink is moved as a link. Its target is never touched, and the folder it points to is not searched.
pub fn remove_item(path: &Path, trash: &dyn Trash) -> Result<(), ActionError> {
    if is_protected(path) {
        return Err(ActionError::Protected);
    }
    let metadata = fs::symlink_metadata(path)?;
    if let Some(blocker) = move_blocker(path, &metadata)? {
        return Err(blocker);
    }
    if metadata.is_dir() {
        if let Some(relative) = first_locked_inside(path)? {
            return Err(ActionError::LockedInside(relative));
        }
    }
    trash.move_to_trash(path).map_err(|error| {
        let blocker = match error.kind() {
            io::ErrorKind::PermissionDenied => fs::symlink_metadata(path)
                .ok()
                .and_then(|metadata| move_blocker(path, &metadata).ok().flatten()),
            _ => None,
        };
        blocker.unwrap_or_else(|| ActionError::from(error))
    })
}

/// The lock that keeps `path` from being moved: its own lock, or the lock on the folder that holds it.
fn move_blocker(path: &Path, metadata: &Metadata) -> io::Result<Option<ActionError>> {
    if is_locked(metadata) {
        return Ok(Some(ActionError::Locked));
    }
    if let Some(parent) = path.parent() {
        if is_locked(&fs::symlink_metadata(parent)?) {
            return Ok(Some(ActionError::LockedParent));
        }
    }
    Ok(None)
}

/// The path, relative to `root`, of the first locked item inside the folder. Symlinks are not followed.
fn first_locked_inside(root: &Path) -> Result<Option<String>, ActionError> {
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_locked(&metadata) {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                return Ok(Some(relative.display().to_string()));
            }
            if metadata.is_dir() {
                folders.push(path);
            }
        }
    }
    Ok(None)
}

/// Sets or clears the lock flag of the item at `path`. Only the owner can change it, and system folders are never locked.
///
/// `lchflags` changes a symlink's own flags, so a link's target is never locked through it.
pub fn set_locked(path: &Path, locked: bool, current_uid: u32) -> Result<(), ActionError> {
    if locked && is_protected(path) {
        return Err(ActionError::Protected);
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != current_uid {
        return Err(ActionError::NotOwner);
    }
    let flags = metadata.st_flags();
    let wanted = if locked {
        flags | LOCK_FLAG
    } else {
        flags & !LOCK_FLAG
    };
    if wanted == flags {
        return Ok(());
    }
    change_flags(path, wanted)?;
    Ok(())
}

fn change_flags(path: &Path, flags: u32) -> io::Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name contains a NUL byte"))?;
    // SAFETY: `c_path` is a valid NUL-terminated string that lives for the whole call, and lchflags only reads it.
    let status = unsafe { libc::lchflags(c_path.as_ptr(), flags) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Opens the item in its default app. A folder opens in Finder. Must run on the main thread.
pub fn open_item(path: &Path) -> Result<(), ActionError> {
    fs::symlink_metadata(path)?;
    let url = file_url(path)?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(ActionError::Failed("No app could open it.".to_string()))
    }
}

/// A change to one item, run on the worker thread.
pub enum ItemCommand {
    Remove(PathBuf),
    SetLocked { path: PathBuf, locked: bool },
}

/// How a command ended, sent back to the main thread.
pub struct ItemEvent {
    pub action: ItemAction,
    pub path: PathBuf,
    pub result: Result<(), ActionError>,
}

/// Runs removals and lock changes off the main thread, because checking a folder for locked items can take a while.
pub struct ItemWorker {
    commands: Sender<ItemCommand>,
    events: Receiver<ItemEvent>,
}

impl ItemWorker {
    pub fn start() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::Builder::new()
            .name("item-worker".to_string())
            .spawn(move || run_worker(command_rx, event_tx))
            .expect("failed to spawn the item worker thread");
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    pub fn submit(&self, command: ItemCommand) {
        let _ = self.commands.send(command);
    }

    /// Returns the events produced since the last call, without blocking.
    pub fn drain_events(&self) -> Vec<ItemEvent> {
        self.events.try_iter().collect()
    }
}

fn run_worker(commands: Receiver<ItemCommand>, events: Sender<ItemEvent>) {
    let uid = current_uid();
    for command in commands {
        let event = match command {
            ItemCommand::Remove(path) => ItemEvent {
                action: ItemAction::Remove,
                result: remove_item(&path, &SystemTrash),
                path,
            },
            ItemCommand::SetLocked { path, locked } => ItemEvent {
                action: if locked {
                    ItemAction::Lock
                } else {
                    ItemAction::Unlock
                },
                result: set_locked(&path, locked, uid),
                path,
            },
        };
        if events.send(event).is_err() {
            return;
        }
    }
}

/// Test helpers. Nothing here runs in the app.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Clears the lock on a test item when dropped, so a locked fixture never blocks cleanup.
    pub struct UnlockOnDrop(pub PathBuf);

    impl Drop for UnlockOnDrop {
        fn drop(&mut self) {
            let _ = set_locked(&self.0, false, current_uid());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::UnlockOnDrop;
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    /// Moves items into a folder the way the system Trash does, and records every call.
    struct FakeTrash {
        folder: PathBuf,
        calls: RefCell<Vec<PathBuf>>,
        failure: Option<io::ErrorKind>,
    }

    impl FakeTrash {
        fn new(dir: &TempDir) -> Self {
            let folder = dir.path().join("fake-trash");
            fs::create_dir(&folder).unwrap();
            Self {
                folder,
                calls: RefCell::new(Vec::new()),
                failure: None,
            }
        }

        fn failing(dir: &TempDir, kind: io::ErrorKind) -> Self {
            Self {
                failure: Some(kind),
                ..Self::new(dir)
            }
        }

        fn calls(&self) -> Vec<PathBuf> {
            self.calls.borrow().clone()
        }
    }

    impl Trash for FakeTrash {
        fn move_to_trash(&self, path: &Path) -> io::Result<()> {
            self.calls.borrow_mut().push(path.to_path_buf());
            if let Some(kind) = self.failure {
                return Err(io::Error::from(kind));
            }
            fs::rename(path, self.folder.join(path.file_name().unwrap()))
        }
    }

    fn write_file(path: &Path) {
        fs::write(path, b"contents").unwrap();
    }

    #[test]
    fn removes_an_item_through_the_trash() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let file = dir.path().join("notes.txt");
        write_file(&file);

        assert_eq!(remove_item(&file, &trash), Ok(()));

        assert_eq!(trash.calls(), [file.as_path()]);
        assert!(!file.exists());
        assert!(trash.folder.join("notes.txt").exists());
    }

    #[test]
    fn a_locked_item_is_refused_and_stays_put() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let file = dir.path().join("locked.txt");
        write_file(&file);
        set_locked(&file, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(file.clone());

        assert_eq!(remove_item(&file, &trash), Err(ActionError::Locked));

        assert!(trash.calls().is_empty());
        assert!(file.exists());
    }

    #[test]
    fn unlocking_restores_removal() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let file = dir.path().join("locked.txt");
        write_file(&file);
        set_locked(&file, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(file.clone());
        assert_eq!(remove_item(&file, &trash), Err(ActionError::Locked));

        set_locked(&file, false, current_uid()).unwrap();

        assert_eq!(remove_item(&file, &trash), Ok(()));
        assert!(!file.exists());
    }

    #[test]
    fn a_folder_holding_a_locked_item_is_refused_and_the_item_is_named() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let folder = dir.path().join("folder");
        let inner = folder.join("inner");
        fs::create_dir_all(&inner).unwrap();
        let locked = inner.join("notes.txt");
        write_file(&locked);
        set_locked(&locked, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(locked.clone());

        assert_eq!(
            remove_item(&folder, &trash),
            Err(ActionError::LockedInside("inner/notes.txt".to_string()))
        );
        assert!(trash.calls().is_empty());
    }

    #[test]
    fn a_locked_folder_is_refused() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let folder = dir.path().join("locked-folder");
        fs::create_dir(&folder).unwrap();
        set_locked(&folder, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(folder.clone());

        assert_eq!(remove_item(&folder, &trash), Err(ActionError::Locked));
        assert!(trash.calls().is_empty());
    }

    #[test]
    fn an_item_cannot_leave_a_locked_folder() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let file = folder.join("file.txt");
        write_file(&file);
        set_locked(&folder, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(folder.clone());

        assert_eq!(remove_item(&file, &trash), Err(ActionError::LockedParent));
        assert!(trash.calls().is_empty());
    }

    #[test]
    fn removing_a_symlink_leaves_its_target_intact() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        let locked = target.join("locked.txt");
        write_file(&locked);
        set_locked(&locked, true, current_uid()).unwrap();
        let _unlock = UnlockOnDrop(locked.clone());
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();

        assert_eq!(remove_item(&link, &trash), Ok(()));

        assert_eq!(trash.calls(), [link.as_path()]);
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(locked.exists());
    }

    #[test]
    fn protected_paths_are_refused_before_anything_is_touched() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);

        for path in ["/", "/System/Library", "/usr/local", "/bin/sh", "/sbin"] {
            let path = Path::new(path);
            assert_eq!(remove_item(path, &trash), Err(ActionError::Protected));
            assert_eq!(
                set_locked(path, true, current_uid()),
                Err(ActionError::Protected)
            );
        }
        assert!(trash.calls().is_empty());
    }

    #[test]
    fn protection_matches_whole_folder_names_only() {
        assert!(is_protected(Path::new("/usr")));
        assert!(!is_protected(Path::new("/usrlocal")));
        assert!(!is_protected(Path::new("/Users/someone/usr")));
    }

    #[test]
    fn permission_denied_is_reported_as_such() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::failing(&dir, io::ErrorKind::PermissionDenied);
        let file = dir.path().join("kept.txt");
        write_file(&file);

        let result = remove_item(&file, &trash);

        assert_eq!(result, Err(ActionError::PermissionDenied));
        assert!(result
            .unwrap_err()
            .to_string()
            .starts_with("Permission denied"));
        assert!(file.exists());
    }

    #[test]
    fn only_the_owner_can_lock_an_item() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("mine.txt");
        write_file(&file);
        let someone_else = current_uid().wrapping_add(1);

        assert_eq!(
            set_locked(&file, true, someone_else),
            Err(ActionError::NotOwner)
        );
        assert!(!is_locked(&fs::symlink_metadata(&file).unwrap()));
    }
}

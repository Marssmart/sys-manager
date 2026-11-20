//! Largest-first disk usage scanning.
//!
//! The walk is depth-first and never follows symlinks. Files and folders are
//! ranked in a list bounded by `ScanOptions::top_n`. A folder's size includes
//! everything under it, so its entry grows while the walk is still inside it.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, DirEntry, Metadata, ReadDir};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Shared flag that asks a running scan to stop.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Bytes charged to a file. Each platform supplies the measure that matches its disk accounting.
pub type SizeMeasure = fn(&Metadata) -> u64;

/// Reports whether a file or folder carries the platform's lock flag. Each platform supplies the check.
pub type LockProbe = fn(&Metadata) -> bool;

/// Logical length of the file in bytes.
pub fn apparent_size(metadata: &Metadata) -> u64 {
    metadata.len()
}

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub root: PathBuf,
    /// Paths that are never entered, for example a second view of data that is reachable elsewhere.
    pub skip: Vec<PathBuf>,
    /// Maximum number of entries in each snapshot.
    pub top_n: usize,
    pub measure: SizeMeasure,
    /// Reports the lock flag of each file and folder, so that entries can show it.
    pub is_locked: LockProbe,
    /// Minimum time between progress snapshots.
    pub report_interval: Duration,
}

impl ScanOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            skip: Vec::new(),
            top_n: 200,
            measure: apparent_size,
            is_locked: |_| false,
            report_interval: Duration::from_millis(250),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanState {
    Scanning,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub size: u64,
    pub kind: EntryKind,
    /// Items the scan counted under this entry, including the entry itself.
    pub items: u64,
    /// Whether the entry itself carries the lock flag.
    pub locked: bool,
}

/// Final path component, such as `Library`.
pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl ScanEntry {
    /// Final path component, such as `Library`.
    pub fn name(&self) -> String {
        display_name(&self.path)
    }

    /// Parent folder, shown under the name.
    pub fn location(&self) -> String {
        self.path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub struct ScanSnapshot {
    /// Counts up from 1 with every snapshot of the same scan.
    pub sequence: u64,
    pub state: ScanState,
    pub items_scanned: u64,
    pub bytes_scanned: u64,
    /// Items that could not be read because access was denied.
    pub skipped_no_access: u64,
    /// Items that could not be read for any other reason.
    pub other_errors: u64,
    pub current_path: Option<PathBuf>,
    /// Largest first. Ties are ordered by path. Never longer than `top_n`.
    pub entries: Vec<ScanEntry>,
    pub elapsed: Duration,
}

/// Walks `options.root`, reporting snapshots through `on_snapshot` while it runs.
///
/// The final snapshot is passed to `on_snapshot` and also returned.
pub fn scan<F>(options: &ScanOptions, cancel: &CancelToken, mut on_snapshot: F) -> ScanSnapshot
where
    F: FnMut(&ScanSnapshot),
{
    let mut walk = Walk::new(options);
    walk.start();
    let state = loop {
        if cancel.is_cancelled() {
            break ScanState::Cancelled;
        }
        if walk.report_due() {
            on_snapshot(&walk.report(ScanState::Scanning));
        }
        if !walk.step() {
            break ScanState::Completed;
        }
    };
    let snapshot = walk.report(state);
    on_snapshot(&snapshot);
    snapshot
}

/// Ranking key. Derived ordering is ascending by size, so the first entry is the one evicted first.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Ranked {
    size: u64,
    // Reversed so that among equal sizes the greater path is evicted first, keeping smaller paths.
    path: Reverse<PathBuf>,
    kind: EntryKind,
    items: u64,
    locked: bool,
}

struct Ranking {
    capacity: usize,
    ranked: BTreeSet<Ranked>,
}

impl Ranking {
    fn accepts(&self, size: u64) -> bool {
        if size == 0 || self.capacity == 0 {
            return false;
        }
        if self.ranked.len() < self.capacity {
            return true;
        }
        self.ranked
            .first()
            .is_some_and(|smallest| size > smallest.size)
    }

    fn insert(&mut self, entry: Ranked) {
        if self.ranked.len() == self.capacity {
            self.ranked.pop_first();
        }
        self.ranked.insert(entry);
    }
}

struct Frame {
    path: PathBuf,
    entries: ReadDir,
    /// Bytes found so far under this directory.
    total: u64,
    /// Items found so far under this directory, not counting the directory itself.
    items: u64,
    /// Whether the directory itself carries the lock flag.
    locked: bool,
    /// The scan root is walked but never ranked.
    rankable: bool,
    /// The entry this frame currently holds in the ranking, if any.
    ranked: Option<Ranked>,
}

/// Brings a frame's ranking entry up to date with its running total.
fn rank_frame(ranking: &mut Ranking, frame: &mut Frame) {
    let items = frame.items + 1;
    if !frame.rankable
        || frame
            .ranked
            .as_ref()
            .is_some_and(|ranked| ranked.size == frame.total && ranked.items == items)
    {
        return;
    }
    if let Some(previous) = frame.ranked.take() {
        ranking.ranked.remove(&previous);
    }
    if ranking.accepts(frame.total) {
        let entry = Ranked {
            size: frame.total,
            path: Reverse(frame.path.clone()),
            kind: EntryKind::Directory,
            items,
            locked: frame.locked,
        };
        ranking.insert(entry.clone());
        frame.ranked = Some(entry);
    }
}

struct Walk<'a> {
    options: &'a ScanOptions,
    skip: HashSet<PathBuf>,
    ranking: Ranking,
    stack: Vec<Frame>,
    items_scanned: u64,
    bytes_scanned: u64,
    skipped_no_access: u64,
    other_errors: u64,
    current_path: Option<PathBuf>,
    sequence: u64,
    started: Instant,
    last_report: Option<Instant>,
}

impl<'a> Walk<'a> {
    fn new(options: &'a ScanOptions) -> Self {
        Self {
            options,
            skip: options.skip.iter().cloned().collect(),
            ranking: Ranking {
                capacity: options.top_n,
                ranked: BTreeSet::new(),
            },
            stack: Vec::new(),
            items_scanned: 0,
            bytes_scanned: 0,
            skipped_no_access: 0,
            other_errors: 0,
            current_path: None,
            sequence: 0,
            started: Instant::now(),
            last_report: None,
        }
    }

    fn start(&mut self) {
        let root = self.options.root.clone();
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.is_dir() => self.enter(root, false, false),
            Ok(metadata) if metadata.is_file() => self.add_file(root, &metadata),
            Ok(_) => {}
            Err(err) => self.record_error(&err),
        }
    }

    /// Advances the walk by one entry. Returns false once every directory is finished.
    fn step(&mut self) -> bool {
        let next = match self.stack.last_mut() {
            Some(frame) => frame.entries.next(),
            None => return false,
        };
        match next {
            Some(Ok(entry)) => self.visit(entry),
            Some(Err(err)) => self.record_error(&err),
            None => self.finish_frame(),
        }
        true
    }

    fn visit(&mut self, entry: DirEntry) {
        self.items_scanned += 1;
        self.count_item();
        let path = entry.path();
        self.current_path = Some(path.clone());
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => return self.record_error(&err),
        };
        if file_type.is_symlink() {
            return;
        }
        if file_type.is_dir() {
            if !self.skip.contains(&path) {
                let locked = match entry.metadata() {
                    Ok(metadata) => (self.options.is_locked)(&metadata),
                    Err(err) => {
                        self.record_error(&err);
                        false
                    }
                };
                self.enter(path, true, locked);
            }
        } else if file_type.is_file() {
            match entry.metadata() {
                Ok(metadata) => self.add_file(path, &metadata),
                Err(err) => self.record_error(&err),
            }
        }
    }

    fn enter(&mut self, path: PathBuf, rankable: bool, locked: bool) {
        match fs::read_dir(&path) {
            Ok(entries) => self.stack.push(Frame {
                path,
                entries,
                total: 0,
                items: 0,
                locked,
                rankable,
                ranked: None,
            }),
            Err(err) => self.record_error(&err),
        }
    }

    fn finish_frame(&mut self) {
        if let Some(mut frame) = self.stack.pop() {
            rank_frame(&mut self.ranking, &mut frame);
        }
    }

    fn add_file(&mut self, path: PathBuf, metadata: &Metadata) {
        let size = (self.options.measure)(metadata);
        let locked = (self.options.is_locked)(metadata);
        self.bytes_scanned += size;
        for frame in &mut self.stack {
            frame.total += size;
        }
        if self.ranking.accepts(size) {
            self.ranking.insert(Ranked {
                size,
                path: Reverse(path),
                kind: EntryKind::File,
                items: 1,
                locked,
            });
        }
    }

    /// Counts one entry the walk has reached, in every folder still open on the stack.
    fn count_item(&mut self) {
        for frame in &mut self.stack {
            frame.items += 1;
        }
    }

    fn record_error(&mut self, err: &io::Error) {
        if err.kind() == io::ErrorKind::PermissionDenied {
            self.skipped_no_access += 1;
        } else {
            self.other_errors += 1;
        }
    }

    fn report_due(&self) -> bool {
        self.last_report
            .is_none_or(|at| at.elapsed() >= self.options.report_interval)
    }

    fn report(&mut self, state: ScanState) -> ScanSnapshot {
        for frame in &mut self.stack {
            rank_frame(&mut self.ranking, frame);
        }
        self.last_report = Some(Instant::now());
        self.sequence += 1;
        ScanSnapshot {
            sequence: self.sequence,
            state,
            items_scanned: self.items_scanned,
            bytes_scanned: self.bytes_scanned,
            skipped_no_access: self.skipped_no_access,
            other_errors: self.other_errors,
            current_path: self.current_path.clone(),
            entries: self
                .ranking
                .ranked
                .iter()
                .rev()
                .map(|ranked| ScanEntry {
                    path: ranked.path.0.clone(),
                    size: ranked.size,
                    kind: ranked.kind,
                    items: ranked.items,
                    locked: ranked.locked,
                })
                .collect(),
            elapsed: self.started.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_file(path: &Path, size: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; size as usize]).unwrap();
    }

    fn run(options: &ScanOptions) -> ScanSnapshot {
        scan(options, &CancelToken::default(), |_| {})
    }

    fn relative_entries(snapshot: &ScanSnapshot, root: &Path) -> Vec<(String, u64)> {
        snapshot
            .entries
            .iter()
            .map(|entry| {
                let path = entry.path.strip_prefix(root).unwrap();
                (path.to_string_lossy().into_owned(), entry.size)
            })
            .collect()
    }

    #[test]
    fn ranks_files_and_folders_largest_first() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write_file(&root.join("small.bin"), 10_000);
        write_file(&root.join("big.bin"), 30_000);
        write_file(&root.join("folder/inner.bin"), 20_000);
        write_file(&root.join("empty.bin"), 0);

        let snapshot = run(&ScanOptions::new(root));

        assert_eq!(snapshot.state, ScanState::Completed);
        assert_eq!(
            relative_entries(&snapshot, root),
            [
                ("big.bin", 30_000),
                ("folder", 20_000),
                ("folder/inner.bin", 20_000),
                ("small.bin", 10_000),
            ]
            .map(|(path, size)| (path.to_string(), size))
        );
    }

    #[test]
    fn keeps_only_the_largest_top_n_entries_at_every_snapshot() {
        let dir = TempDir::new().unwrap();
        for i in 1..=10u64 {
            write_file(&dir.path().join(format!("file-{i:02}.bin")), i * 1_000);
        }
        let options = ScanOptions {
            top_n: 3,
            report_interval: Duration::ZERO,
            ..ScanOptions::new(dir.path())
        };

        let mut reports = 0;
        let snapshot = scan(&options, &CancelToken::default(), |snapshot| {
            assert!(snapshot.entries.len() <= 3);
            reports += 1;
        });

        assert!(reports > 3);
        let sizes: Vec<u64> = snapshot.entries.iter().map(|entry| entry.size).collect();
        assert_eq!(sizes, [10_000, 9_000, 8_000]);
        assert_eq!(snapshot.items_scanned, 10);
    }

    #[test]
    fn folder_sizes_grow_while_the_scan_runs() {
        let dir = TempDir::new().unwrap();
        for i in 0..50 {
            write_file(
                &dir.path().join("big").join(format!("chunk-{i:02}.bin")),
                1_000,
            );
        }
        let options = ScanOptions {
            report_interval: Duration::ZERO,
            ..ScanOptions::new(dir.path())
        };

        let mut folder_sizes = Vec::new();
        let snapshot = scan(&options, &CancelToken::default(), |snapshot| {
            if let Some(entry) = snapshot
                .entries
                .iter()
                .find(|entry| entry.path.ends_with("big"))
            {
                folder_sizes.push(entry.size);
            }
        });

        assert_eq!(snapshot.entries[0].size, 50_000);
        assert!(folder_sizes.iter().any(|&size| size > 0 && size < 50_000));
        assert!(folder_sizes.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn cancelling_mid_scan_returns_partial_results() {
        let dir = TempDir::new().unwrap();
        for i in 0..200 {
            write_file(&dir.path().join(format!("file-{i:03}.bin")), 100 + i);
        }
        let cancel = CancelToken::default();
        let options = ScanOptions {
            report_interval: Duration::ZERO,
            ..ScanOptions::new(dir.path())
        };

        let snapshot = scan(&options, &cancel, |snapshot| {
            if snapshot.items_scanned >= 10 {
                cancel.cancel();
            }
        });

        assert_eq!(snapshot.state, ScanState::Cancelled);
        assert!((10..200).contains(&snapshot.items_scanned));
    }

    #[test]
    fn skipped_paths_are_never_entered() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write_file(&root.join("duplicate/huge.bin"), 50_000);
        write_file(&root.join("keep.bin"), 1_000);
        let options = ScanOptions {
            skip: vec![root.join("duplicate")],
            ..ScanOptions::new(root)
        };

        let snapshot = run(&options);

        assert_eq!(
            relative_entries(&snapshot, root),
            [("keep.bin".to_string(), 1_000)]
        );
        assert_eq!(snapshot.items_scanned, 2);
    }

    #[test]
    fn missing_root_is_counted_as_an_error_and_scan_completes() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing");

        let snapshot = run(&ScanOptions::new(&missing));

        assert_eq!(snapshot.state, ScanState::Completed);
        assert!(snapshot.entries.is_empty());
        assert_eq!(snapshot.other_errors, 1);
    }

    #[test]
    fn entries_count_their_items_and_carry_the_lock_flag() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write_file(&root.join("big/a.bin"), 3_000);
        write_file(&root.join("big/nested/b.bin"), 2_000);
        write_file(&root.join("big/nested/c.bin"), 1);
        let options = ScanOptions {
            is_locked: |metadata| metadata.len() == 1,
            ..ScanOptions::new(root)
        };

        let snapshot = run(&options);

        let entry = |name: &str| {
            snapshot
                .entries
                .iter()
                .find(|entry| entry.path.ends_with(name))
                .unwrap()
        };
        assert_eq!(entry("big").items, 5);
        assert_eq!(entry("nested").items, 3);
        assert_eq!(entry("a.bin").items, 1);
        assert!(entry("c.bin").locked);
        assert!(!entry("a.bin").locked);
    }
}

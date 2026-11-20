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

/// Decides what a lock change may touch. Production uses the system folders and the user's own items. Tests substitute rules of their own.
struct LockRules<'a> {
    is_protected: &'a dyn Fn(&Path) -> bool,
    is_owned: &'a dyn Fn(&Path, &Metadata) -> bool,
}

/// Locks or unlocks the item at `path`, and everything under it when it is a folder. Only the owner can change items, and system locations are never locked.
///
/// `lchflags` changes a symlink's own flags, so a link's target is never locked through it.
pub fn set_locked(path: &Path, locked: bool, current_uid: u32) -> Result<(), ActionError> {
    let is_owned = |_: &Path, metadata: &Metadata| metadata.uid() == current_uid;
    lock_tree(
        path,
        locked,
        &LockRules {
            is_protected: &is_protected,
            is_owned: &is_owned,
        },
    )
}

/// Changes `root` and everything under it. The whole tree is checked before anything changes. If a change fails part way, the items already changed are restored.
fn lock_tree(root: &Path, locked: bool, rules: &LockRules) -> Result<(), ActionError> {
    if locked && (rules.is_protected)(root) {
        return Err(ActionError::Protected);
    }
    let items = collect_tree(root, locked, rules)?;
    // Locking goes bottom-up, so each folder closes after its contents. Unlocking goes top-down.
    let ordered: Vec<&TreeItem> = if locked {
        items.iter().rev().collect()
    } else {
        items.iter().collect()
    };
    let mut changed: Vec<&TreeItem> = Vec::new();
    for item in ordered {
        let wanted = if locked {
            item.flags | LOCK_FLAG
        } else {
            item.flags & !LOCK_FLAG
        };
        if wanted == item.flags {
            continue;
        }
        if let Err(error) = change_flags(&item.path, wanted) {
            for done in changed.iter().rev() {
                let _ = change_flags(&done.path, done.flags);
            }
            return Err(ActionError::from(error));
        }
        changed.push(item);
    }
    Ok(())
}

/// An item in a tree that is about to change, with the flags it had when it was checked.
struct TreeItem {
    path: PathBuf,
    flags: u32,
}

/// Lists `root` and everything under it, each folder before its contents. Symlinks are listed but never entered. Refuses on the first item that may not change.
fn collect_tree(
    root: &Path,
    locking: bool,
    rules: &LockRules,
) -> Result<Vec<TreeItem>, ActionError> {
    let root_metadata = fs::symlink_metadata(root)?;
    check_item(root, root, &root_metadata, locking, rules)?;
    let mut items = vec![TreeItem {
        path: root.to_path_buf(),
        flags: root_metadata.st_flags(),
    }];
    let mut folders = Vec::new();
    if root_metadata.is_dir() {
        folders.push(root.to_path_buf());
    }
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            check_item(&path, root, &metadata, locking, rules)?;
            if metadata.is_dir() {
                folders.push(path.clone());
            }
            items.push(TreeItem {
                path,
                flags: metadata.st_flags(),
            });
        }
    }
    Ok(items)
}

/// Refuses an item that the change may not touch. Inside a tree, the refusal names the item.
fn check_item(
    path: &Path,
    root: &Path,
    metadata: &Metadata,
    locking: bool,
    rules: &LockRules,
) -> Result<(), ActionError> {
    let inside = path != root;
    let relative = || {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    if locking && (rules.is_protected)(path) {
        return Err(if inside {
            ActionError::ProtectedInside(relative())
        } else {
            ActionError::Protected
        });
    }
    if !(rules.is_owned)(path, metadata) {
        return Err(if inside {
            ActionError::NotOwnerInside(relative())
        } else {
            ActionError::NotOwner
        });
    }
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
        // Clearing the file's own flag leaves the locked folder as the only blocker.
        set_locked(&file, false, current_uid()).unwrap();

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

    fn locked(path: &Path) -> bool {
        is_locked(&fs::symlink_metadata(path).unwrap())
    }

    #[test]
    fn locking_a_nested_tree_locks_every_file_and_subfolder() {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        let deep = tree.join("sub/deep");
        fs::create_dir_all(&deep).unwrap();
        write_file(&tree.join("top.txt"));
        write_file(&tree.join("sub/middle.txt"));
        write_file(&deep.join("bottom.txt"));
        let _unlock = UnlockOnDrop(tree.clone());

        set_locked(&tree, true, current_uid()).unwrap();

        let items = [
            tree.clone(),
            tree.join("top.txt"),
            tree.join("sub"),
            tree.join("sub/middle.txt"),
            deep.clone(),
            deep.join("bottom.txt"),
        ];
        for item in items {
            assert!(locked(&item), "{} should be locked", item.display());
        }
    }

    #[test]
    fn creating_a_file_in_a_locked_subfolder_fails() {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        let sub = tree.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let _unlock = UnlockOnDrop(tree.clone());
        set_locked(&tree, true, current_uid()).unwrap();

        let error = fs::write(sub.join("new.txt"), b"new").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!sub.join("new.txt").exists());
    }

    #[test]
    fn unlocking_restores_removal_throughout_the_tree() {
        let dir = TempDir::new().unwrap();
        let trash = FakeTrash::new(&dir);
        let folder = dir.path().join("folder");
        fs::create_dir_all(folder.join("sub")).unwrap();
        write_file(&folder.join("sub/file.txt"));
        let _unlock = UnlockOnDrop(folder.clone());
        set_locked(&folder, true, current_uid()).unwrap();
        assert_eq!(remove_item(&folder, &trash), Err(ActionError::Locked));

        set_locked(&folder, false, current_uid()).unwrap();

        assert!(!locked(&folder.join("sub")));
        assert!(!locked(&folder.join("sub/file.txt")));
        assert_eq!(remove_item(&folder, &trash), Ok(()));
        assert!(trash.folder.join("folder/sub/file.txt").exists());
    }

    #[test]
    fn symlinks_in_a_locked_tree_are_locked_as_links_and_their_targets_are_untouched() {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        let outside = dir.path().join("outside");
        fs::create_dir(&tree).unwrap();
        fs::create_dir(&outside).unwrap();
        let outside_file = outside.join("keep.txt");
        write_file(&outside_file);
        let folder_link = tree.join("folder-link");
        let file_link = tree.join("file-link");
        symlink(&outside, &folder_link).unwrap();
        symlink(&outside_file, &file_link).unwrap();
        let _unlock = UnlockOnDrop(tree.clone());

        set_locked(&tree, true, current_uid()).unwrap();

        assert!(locked(&folder_link));
        assert!(locked(&file_link));
        assert!(!locked(&outside));
        assert!(!locked(&outside_file));
    }

    #[test]
    fn a_tree_holding_a_protected_item_refuses_the_lock_and_changes_nothing() {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        let protected = tree.join("sub/protected.txt");
        fs::create_dir_all(tree.join("sub")).unwrap();
        write_file(&tree.join("open.txt"));
        write_file(&protected);
        let _unlock = UnlockOnDrop(tree.clone());
        let rules = LockRules {
            is_protected: &|path: &Path| path == protected,
            is_owned: &|_: &Path, _: &Metadata| true,
        };

        let result = lock_tree(&tree, true, &rules);

        assert_eq!(
            result,
            Err(ActionError::ProtectedInside(
                "sub/protected.txt".to_string()
            ))
        );
        let items = [
            tree.clone(),
            tree.join("open.txt"),
            tree.join("sub"),
            protected.clone(),
        ];
        for item in items {
            assert!(!locked(&item), "{} should be unchanged", item.display());
        }
    }

    #[test]
    fn a_tree_holding_an_item_you_do_not_own_refuses_the_lock_and_changes_nothing() {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        let foreign = tree.join("foreign.txt");
        fs::create_dir(&tree).unwrap();
        write_file(&tree.join("mine.txt"));
        write_file(&foreign);
        let _unlock = UnlockOnDrop(tree.clone());
        let rules = LockRules {
            is_protected: &|_: &Path| false,
            is_owned: &|path: &Path, _: &Metadata| path != foreign,
        };

        let result = lock_tree(&tree, true, &rules);

        assert_eq!(
            result,
            Err(ActionError::NotOwnerInside("foreign.txt".to_string()))
        );
        assert!(!locked(&tree));
        assert!(!locked(&tree.join("mine.txt")));
        assert!(!locked(&foreign));
    }
}

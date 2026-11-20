# sys-manager

A Rust utility with a macOS menu bar app. OS-specific code is kept out of shared logic so other operating systems can be added later.

## Layout

- `common/` (`sys-manager-common`): platform-neutral logic. Must contain no OS-specific code.
- `platform/<os>/` (`sys-manager-platform-<os>`): OS-specific logic, one crate per operating system. macOS lives in `platform/macos/`.
- `app/` (`sys-manager`): thin binary and future menu bar entry point. Selects the platform crate for the current target with `cfg(target_os)`.

## Dependency rule

Platform crates depend on `common`. `common` never depends on a platform crate. `app` depends on `common` and the platform crate for the current target OS.

To add an OS, create `platform/<os>/` and add a `cfg(target_os)` dependency and alias in `app/`.

## Build

Requires Rust 1.95 or later from rustup.

```sh
cargo build --workspace
cargo test --workspace
cargo run -p sys-manager
```

## Disk cleaner

The disk cleaner lists the largest files and folders on the startup disk. Select an item to see its actions:

- **Open** opens a file in its default app, and a folder in Finder.
- **Lock** sets the macOS lock flag (`chflags uchg`) on an item you own. For a folder, it sets the flag on everything inside too, at every depth. A locked item can't be changed, renamed or deleted from Finder, Terminal or this app until it is unlocked. Locked items show a padlock. **Unlock** clears the flag on the item and everything under it.
- **Remove** moves the item to the Trash, so Finder's Put Back can restore it. It asks first, naming the item and its size. Remove is available once the scan has finished.

Sys Manager refuses an action and says why when:

- The item is under `/System`, `/usr`, `/bin` or `/sbin`, or it is the volume root `/`.
- The item is locked: `Locked. Unlock it first.`
- The item is a folder that holds a locked item, or it sits in a locked folder. Unlock the locked item or folder first.
- The folder you are locking holds a protected system item. The whole lock is refused, and nothing changes.
- You don't own the item, or an item inside the folder you are locking or unlocking. Nothing changes.
- macOS denies the action, reported as `Permission denied`.

A symlink is removed as a link, and its target is never touched. After a removal, the item leaves the list, and the sizes and item counts of the folders that held it go down by the same amount.

Moving to the Trash goes through the `Trash` trait in `platform/macos/src/items.rs`. The tests use a fake, so running them never moves a file to the real Trash.

### What a lock blocks

Checked on temporary folders with `chflags -R uchg` and `chflags -h uchg` on macOS, which set the same flags as Sys Manager:

| Operation | Result |
| --- | --- |
| Add an entry at any depth inside a locked folder tree | Blocked |
| Remove, rename or move an entry out of the tree | Blocked |
| Remove or rename a file or subfolder inside the tree, at any depth | Blocked |
| Remove or rename the locked folder, including `rm -rf` | Blocked, and its entries stay |
| Rename a folder that is not locked but holds a locked item | Allowed, and the locked item moves with it |
| `rm -rf` a folder that is not locked but holds a locked item | Unlocked items are deleted, and the locked items and their folders stay |
| Remove a symlink that has the lock flag set | Blocked, and its target keeps its own flags |

Locking a folder locks everything under it, at every depth: subfolders, files and symlinks themselves. Unlocking clears the flag throughout. Sys Manager never follows a symlink, so a link's target keeps its own flags. The whole tree is checked before anything changes, and a lock is refused, with nothing changed, if the tree holds a protected item or an item you don't own. Sys Manager still refuses to remove a folder that holds a locked item, because moving the folder would move the locked item too.

## Package the macOS app

```sh
platform/macos/packaging/build-app.sh
```

This writes `target/dist/Sys Manager.app`, a zipped copy of it and `target/dist/Sys-Manager-<version>.dmg`. Pushing a `v*` tag runs the release workflow, which publishes these files as a GitHub Release. Release steps, first-launch instructions for unsigned builds, and signing and notarization are in [`platform/macos/packaging/README.md`](platform/macos/packaging/README.md).

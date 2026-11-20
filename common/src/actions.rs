//! What an action on a disk item can fail with, and the sentences the disk list shows for each action.

use std::fmt;
use std::io;

/// An action on one item in the disk list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    Open,
    Remove,
    Lock,
    Unlock,
}

impl ItemAction {
    /// Shown while the action runs, such as `Moving “Caches” to the Trash…`.
    pub fn progress_text(self, name: &str) -> String {
        match self {
            Self::Open => format!("Opening “{name}”…"),
            Self::Remove => format!("Moving “{name}” to the Trash…"),
            Self::Lock => format!("Locking “{name}”…"),
            Self::Unlock => format!("Unlocking “{name}”…"),
        }
    }

    /// Shown when the action succeeds.
    pub fn success_text(self, name: &str) -> String {
        match self {
            Self::Open => format!("Opened “{name}”"),
            Self::Remove => format!("Moved “{name}” to the Trash"),
            Self::Lock => format!("Locked “{name}”"),
            Self::Unlock => format!("Unlocked “{name}”"),
        }
    }

    /// Shown when the action fails, followed by the reason.
    pub fn failure_text(self, name: &str, error: &ActionError) -> String {
        match self {
            Self::Open => format!("Couldn't open “{name}”. {error}"),
            Self::Remove => format!("Couldn't move “{name}” to the Trash. {error}"),
            Self::Lock => format!("Couldn't lock “{name}”. {error}"),
            Self::Unlock => format!("Couldn't unlock “{name}”. {error}"),
        }
    }
}

/// Why an action on an item was refused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// A system location that Sys Manager never removes or locks.
    Protected,
    /// The item itself has the lock flag set.
    Locked,
    /// The folder holds a locked item, given by its path relative to the folder.
    LockedInside(String),
    /// The folder that holds the item is locked, so the item can't be moved out of it.
    LockedParent,
    /// Only the owner of an item may lock or unlock it.
    NotOwner,
    /// The item is no longer where the scan found it.
    NotFound,
    /// The system denied access.
    PermissionDenied,
    /// Any other failure, with the system's message.
    Failed(String),
}

impl From<io::Error> for ActionError {
    /// Maps an I/O failure to the matching refusal. Other failures keep the system's message.
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Failed(error.to_string()),
        }
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protected => write!(
                f,
                "Protected system location. Sys Manager doesn't remove or lock it."
            ),
            Self::Locked => write!(f, "Locked. Unlock it first."),
            Self::LockedInside(relative) => write!(
                f,
                "It contains a locked item, “{relative}”. Unlock it first."
            ),
            Self::LockedParent => write!(f, "Its folder is locked. Unlock the folder first."),
            Self::NotOwner => write!(f, "You can only lock or unlock items you own."),
            Self::NotFound => write!(f, "It's no longer there. Rescan to refresh the list."),
            Self::PermissionDenied => write!(
                f,
                "Permission denied. macOS won't let Sys Manager change this item."
            ),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

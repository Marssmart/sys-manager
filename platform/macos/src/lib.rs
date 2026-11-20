//! macOS platform crate: startup disk scanning, process listing and killing, and the menu bar UI.

pub mod disk;
pub mod items;
pub mod process;
mod ui;

pub use ui::run;

//! Panel under the disk list for the selected item: its name, size and path, the actions that apply to it, and the removal confirmation.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSButton, NSColor, NSLineBreakMode,
    NSTextAlignment, NSTextField,
};
use objc2_foundation::{NSRect, NSString};

use sys_manager_common::disk::{EntryKind, ScanEntry};
use sys_manager_common::format::format_bytes;

use crate::items::is_protected;

use super::views::GroupView;
use super::widgets::{button, digits_font, font, frame, label, set_text, MEDIUM, REGULAR};

pub const ITEM_PANEL_HEIGHT: f64 = 92.0;
const EDGE: f64 = 12.0;
const BUTTON_WIDTH: f64 = 72.0;
const BUTTON_GAP: f64 = 8.0;

pub struct ItemPanel {
    group: Retained<GroupView>,
    placeholder: Retained<NSTextField>,
    name: Retained<NSTextField>,
    size: Retained<NSTextField>,
    path: Retained<NSTextField>,
    status: Retained<NSTextField>,
    open: Retained<NSButton>,
    lock: Retained<NSButton>,
    remove: Retained<NSButton>,
}

impl ItemPanel {
    /// Builds the panel. Its buttons send `openItem:`, `toggleLock:` and `removeItem:` to `target`.
    pub fn new(mtm: MainThreadMarker, bounds: NSRect, target: &AnyObject) -> Self {
        let placeholder = label(mtm, &font(12.0, REGULAR), &NSColor::secondaryLabelColor());
        set_text(
            &placeholder,
            "Select an item to open, lock or move it to the Trash.",
        );
        placeholder.setAlignment(NSTextAlignment::Center);
        placeholder.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        placeholder.setMaximumNumberOfLines(2);
        let size = label(mtm, &digits_font(12.5, MEDIUM), &NSColor::labelColor());
        size.setAlignment(NSTextAlignment::Right);
        let path = label(mtm, &font(11.0, REGULAR), &NSColor::secondaryLabelColor());
        path.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        let panel = Self {
            group: GroupView::new(mtm, bounds),
            placeholder,
            name: label(mtm, &font(13.0, MEDIUM), &NSColor::labelColor()),
            size,
            path,
            status: label(mtm, &font(11.0, REGULAR), &NSColor::secondaryLabelColor()),
            open: button(mtm, "Open", Some(target), Some(sel!(openItem:))),
            lock: button(mtm, "Lock", Some(target), Some(sel!(toggleLock:))),
            remove: button(mtm, "Remove", Some(target), Some(sel!(removeItem:))),
        };
        panel.group.addSubview(&panel.placeholder);
        for text in [&panel.name, &panel.size, &panel.path, &panel.status] {
            panel.group.addSubview(text);
        }
        for control in [&panel.open, &panel.lock, &panel.remove] {
            panel.group.addSubview(control);
        }
        panel.layout(bounds.size.width);
        panel
    }

    pub fn group(&self) -> &GroupView {
        &self.group
    }

    fn layout(&self, width: f64) {
        self.placeholder
            .setFrame(frame(EDGE, 26.0, width - 2.0 * EDGE, 40.0));
        self.name
            .setFrame(frame(EDGE, 8.0, width - 2.0 * EDGE - 96.0, 17.0));
        self.size
            .setFrame(frame(width - EDGE - 96.0, 8.0, 96.0, 17.0));
        self.path
            .setFrame(frame(EDGE, 28.0, width - 2.0 * EDGE, 15.0));
        self.status
            .setFrame(frame(EDGE, 46.0, width - 2.0 * EDGE, 14.0));
        for (index, control) in [&self.open, &self.lock, &self.remove]
            .into_iter()
            .enumerate()
        {
            let x = EDGE + index as f64 * (BUTTON_WIDTH + BUTTON_GAP);
            control.setFrame(frame(x, 64.0, BUTTON_WIDTH, 22.0));
        }
    }

    /// Shows the actions for `selected`, or the hint when nothing is selected. While `busy`, the actions are
    /// disabled. Remove also waits until the scan has finished, because a running scan still adds to sizes.
    pub fn show(&self, selected: Option<&ScanEntry>, can_change: bool, busy: bool) {
        let Some(entry) = selected else {
            self.placeholder.setHidden(false);
            for hidden in [&self.name, &self.size, &self.path, &self.status] {
                hidden.setHidden(true);
            }
            for hidden in [&self.open, &self.lock, &self.remove] {
                hidden.setHidden(true);
            }
            return;
        };
        self.placeholder.setHidden(true);
        set_text(&self.name, &entry.name());
        set_text(&self.size, &format_bytes(entry.size));
        set_text(&self.path, &entry.path.display().to_string());
        set_text(&self.status, status_text(entry));
        let lock_title = if entry.locked { "Unlock" } else { "Lock" };
        self.lock.setTitle(&NSString::from_str(lock_title));
        self.open.setEnabled(!busy);
        self.lock.setEnabled(!busy);
        self.remove.setEnabled(!busy && can_change);
        let hint = (!can_change).then(|| NSString::from_str("Available when the scan finishes."));
        self.remove.setToolTip(hint.as_deref());
        for shown in [&self.name, &self.size, &self.path, &self.status] {
            shown.setHidden(false);
        }
        for shown in [&self.open, &self.lock, &self.remove] {
            shown.setHidden(false);
        }
    }
}

/// The lock state, or that the item is a system location Sys Manager won't change.
fn status_text(entry: &ScanEntry) -> &'static str {
    if entry.locked {
        "Locked"
    } else if is_protected(&entry.path) {
        "System location"
    } else {
        "Not locked"
    }
}

/// Asks before an item is moved to the Trash. Returns true when the user confirms.
pub fn confirm_removal(mtm: MainThreadMarker, entry: &ScanEntry) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    let question = match entry.kind {
        EntryKind::Directory => {
            format!("Move “{}” and everything in it to the Trash?", entry.name())
        }
        EntryKind::File => format!("Move “{}” to the Trash?", entry.name()),
    };
    alert.setMessageText(&NSString::from_str(&question));
    alert.setInformativeText(&NSString::from_str(&format!(
        "{} will be freed. You can put it back from the Trash in Finder.",
        format_bytes(entry.size)
    )));
    alert.addButtonWithTitle(&NSString::from_str("Move to Trash"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.runModal() == NSAlertFirstButtonReturn
}

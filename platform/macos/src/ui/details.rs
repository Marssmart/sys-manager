//! Process details panel shown under the process list.

use std::time::UNIX_EPOCH;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSColor, NSLineBreakMode, NSTextAlignment, NSTextField};
use objc2_foundation::{NSDate, NSDateFormatter, NSRect};

use sys_manager_common::process::ProcessInfo;

use super::views::GroupView;
use super::widgets::{font, frame, label, set_text, REGULAR};

pub const DETAILS_HEIGHT: f64 = 124.0;
const CAPTION_HEIGHT: f64 = 14.0;
const VALUE_HEIGHT: f64 = 16.0;

struct Field {
    caption: Retained<NSTextField>,
    value: Retained<NSTextField>,
}

impl Field {
    fn new(mtm: MainThreadMarker, caption: &str) -> Self {
        let caption_label = label(mtm, &font(10.0, REGULAR), &NSColor::secondaryLabelColor());
        set_text(&caption_label, caption);
        Self {
            caption: caption_label,
            value: label(mtm, &font(11.5, REGULAR), &NSColor::labelColor()),
        }
    }

    fn place(&self, x: f64, y: f64, width: f64) {
        self.caption.setFrame(frame(x, y, width, CAPTION_HEIGHT));
        self.value
            .setFrame(frame(x, y + CAPTION_HEIGHT, width, VALUE_HEIGHT));
    }

    fn set(&self, value: &str) {
        set_text(&self.value, value);
    }

    fn set_hidden(&self, hidden: bool) {
        self.caption.setHidden(hidden);
        self.value.setHidden(hidden);
    }
}

pub struct DetailsView {
    group: Retained<GroupView>,
    placeholder: Retained<NSTextField>,
    executable: Field,
    user: Field,
    parent: Field,
    started: Field,
}

impl DetailsView {
    pub fn new(mtm: MainThreadMarker, bounds: NSRect) -> Self {
        let group = GroupView::new(mtm, bounds);
        let placeholder = label(mtm, &font(12.0, REGULAR), &NSColor::secondaryLabelColor());
        set_text(
            &placeholder,
            "Select a process to see its executable, owner, parent and start time.",
        );
        placeholder.setAlignment(NSTextAlignment::Center);
        placeholder.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        placeholder.setMaximumNumberOfLines(2);
        group.addSubview(&placeholder);
        let details = Self {
            group,
            placeholder,
            executable: Field::new(mtm, "EXECUTABLE"),
            user: Field::new(mtm, "USER"),
            parent: Field::new(mtm, "PARENT"),
            started: Field::new(mtm, "STARTED"),
        };
        for field in [
            &details.executable,
            &details.user,
            &details.parent,
            &details.started,
        ] {
            details.group.addSubview(&field.caption);
            details.group.addSubview(&field.value);
        }
        details.layout(bounds.size.width);
        details
    }

    pub fn group(&self) -> &GroupView {
        &self.group
    }

    fn layout(&self, width: f64) {
        let column = (width - 36.0) / 2.0;
        self.placeholder
            .setFrame(frame(12.0, 42.0, width - 24.0, 40.0));
        self.executable.place(12.0, 8.0, width - 24.0);
        self.user.place(12.0, 48.0, column);
        self.parent.place(24.0 + column, 48.0, column);
        self.started.place(12.0, 88.0, width - 24.0);
    }

    pub fn show(
        &self,
        process: Option<&ProcessInfo>,
        parent_name: Option<&str>,
        date_format: &NSDateFormatter,
    ) {
        let Some(process) = process else {
            self.placeholder.setHidden(false);
            for field in [&self.executable, &self.user, &self.parent, &self.started] {
                field.set_hidden(true);
            }
            return;
        };
        self.placeholder.setHidden(true);
        let executable = process.exe_path.as_ref().map_or_else(
            || "Not available".to_string(),
            |path| path.display().to_string(),
        );
        let parent = match (process.parent_pid, parent_name) {
            (Some(pid), Some(name)) => format!("{name} (PID {pid})"),
            (Some(pid), None) => format!("PID {pid}"),
            (None, _) => "None".to_string(),
        };
        let started = process
            .started_at
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since_epoch| {
                let date = NSDate::dateWithTimeIntervalSince1970(since_epoch.as_secs_f64());
                date_format.stringFromDate(&date).to_string()
            })
            .unwrap_or_else(|| "Unknown".to_string());
        self.executable.set(&executable);
        self.user.set(process.user.as_deref().unwrap_or("Unknown"));
        self.parent.set(&parent);
        self.started.set(&started);
        for field in [&self.executable, &self.user, &self.parent, &self.started] {
            field.set_hidden(false);
        }
    }
}

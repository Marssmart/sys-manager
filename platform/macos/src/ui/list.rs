//! Inset grouped list. Rows are created once and reused, so frequent refreshes stay cheap.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSBorderType, NSButton, NSColor, NSScrollView, NSTextAlignment, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::views::{FlippedView, GroupView, RowView};
use super::widgets::{button, digits_font, font, frame, label, set_text, wire, MEDIUM, REGULAR};

const ITEM_ROW_HEIGHT: f64 = 46.0;
const PROCESS_ROW_HEIGHT: f64 = 56.0;
const EDGE: f64 = 14.0;

pub enum RowContent {
    Item {
        title: String,
        location: String,
        size: String,
    },
    Process {
        pid: isize,
        title: String,
        pid_text: String,
        cpu_text: String,
        memory_text: String,
    },
}

pub struct RowSpec {
    pub content: RowContent,
    pub selected: bool,
}

struct Row {
    view: Retained<RowView>,
    title: Retained<NSTextField>,
    location: Retained<NSTextField>,
    size: Retained<NSTextField>,
    pid: Retained<NSTextField>,
    cpu: Retained<NSTextField>,
    memory: Retained<NSTextField>,
    kill: Retained<NSButton>,
}

impl Row {
    fn new(mtm: MainThreadMarker, on_click: Rc<dyn Fn(isize)>) -> Self {
        let view = RowView::new(mtm, frame(0.0, 0.0, 0.0, 0.0));
        view.set_on_click(on_click);
        let secondary = NSColor::secondaryLabelColor();
        let caption = font(11.0, REGULAR);
        let size = label(mtm, &digits_font(12.5, MEDIUM), &NSColor::labelColor());
        size.setAlignment(NSTextAlignment::Right);
        let row = Self {
            title: label(mtm, &font(13.0, MEDIUM), &NSColor::labelColor()),
            location: label(mtm, &caption, &secondary),
            size,
            pid: label(mtm, &caption, &secondary),
            cpu: label(mtm, &caption, &secondary),
            memory: label(mtm, &caption, &secondary),
            kill: button(mtm, "Kill", None, None),
            view,
        };
        for subview in [
            &row.title,
            &row.location,
            &row.size,
            &row.pid,
            &row.cpu,
            &row.memory,
        ] {
            row.view.addSubview(subview);
        }
        row.view.addSubview(&row.kill);
        row
    }

    /// Positions and fills the row at `y`, and returns its height.
    fn configure(
        &self,
        spec: &RowSpec,
        y: f64,
        width: f64,
        kill_target: &AnyObject,
        separator: bool,
    ) -> f64 {
        let height = match &spec.content {
            RowContent::Item {
                title,
                location,
                size,
            } => {
                self.view.configure(spec.selected, separator, false, 0);
                set_text(&self.title, title);
                set_text(&self.location, location);
                set_text(&self.size, size);
                self.title
                    .setFrame(frame(EDGE, 6.0, width - EDGE - 120.0, 17.0));
                self.location
                    .setFrame(frame(EDGE, 24.0, width - 2.0 * EDGE, 15.0));
                self.size
                    .setFrame(frame(width - EDGE - 96.0, 6.0, 96.0, 17.0));
                self.location.setHidden(false);
                self.size.setHidden(false);
                for hidden in [&self.pid, &self.cpu, &self.memory] {
                    hidden.setHidden(true);
                }
                self.kill.setHidden(true);
                ITEM_ROW_HEIGHT
            }
            RowContent::Process {
                pid,
                title,
                pid_text,
                cpu_text,
                memory_text,
            } => {
                self.view.configure(spec.selected, separator, true, *pid);
                set_text(&self.title, title);
                set_text(&self.pid, pid_text);
                set_text(&self.cpu, cpu_text);
                set_text(&self.memory, memory_text);
                self.title
                    .setFrame(frame(EDGE, 7.0, width - EDGE - 84.0, 17.0));
                self.pid.setFrame(frame(EDGE, 30.0, 72.0, 14.0));
                self.cpu.setFrame(frame(EDGE + 76.0, 30.0, 72.0, 14.0));
                self.memory.setFrame(frame(EDGE + 152.0, 30.0, 96.0, 14.0));
                self.kill
                    .setFrame(frame(width - 12.0 - 56.0, 17.0, 56.0, 22.0));
                self.kill.setTag(*pid);
                wire(&self.kill, kill_target, sel!(killProcess:));
                self.location.setHidden(true);
                self.size.setHidden(true);
                for shown in [&self.pid, &self.cpu, &self.memory] {
                    shown.setHidden(false);
                }
                self.kill.setHidden(false);
                PROCESS_ROW_HEIGHT
            }
        };
        self.view.setFrame(frame(0.0, y, width, height));
        height
    }
}

pub struct ListView {
    group: Retained<GroupView>,
    scroll: Retained<NSScrollView>,
    document: Retained<FlippedView>,
    empty: Retained<NSTextField>,
    rows: RefCell<Vec<Row>>,
    on_click: Rc<dyn Fn(isize)>,
    mtm: MainThreadMarker,
}

impl ListView {
    pub fn new(mtm: MainThreadMarker, on_click: Rc<dyn Fn(isize)>) -> Self {
        let group = GroupView::new(mtm, frame(0.0, 0.0, 0.0, 0.0));
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        let document = FlippedView::new(mtm, frame(0.0, 0.0, 0.0, 0.0));
        scroll.setDocumentView(Some(&document));
        group.addSubview(&scroll);
        let empty = label(mtm, &font(13.0, REGULAR), &NSColor::secondaryLabelColor());
        empty.setAlignment(NSTextAlignment::Center);
        group.addSubview(&empty);
        Self {
            group,
            scroll,
            document,
            empty,
            rows: RefCell::new(Vec::new()),
            on_click,
            mtm,
        }
    }

    pub fn group(&self) -> &GroupView {
        &self.group
    }

    pub fn set_frame(&self, bounds: NSRect) {
        self.group.setFrame(bounds);
        self.scroll.setFrame(frame(
            1.0,
            1.0,
            bounds.size.width - 2.0,
            bounds.size.height - 2.0,
        ));
        self.empty.setFrame(frame(
            EDGE,
            bounds.size.height / 2.0 - 10.0,
            bounds.size.width - 2.0 * EDGE,
            20.0,
        ));
    }

    pub fn scroll_to_top(&self) {
        self.document.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    pub fn set_rows(&self, specs: &[RowSpec], kill_target: &AnyObject, empty_message: &str) {
        let width = self.scroll.frame().size.width;
        let mut rows = self.rows.borrow_mut();
        while rows.len() < specs.len() {
            let row = Row::new(self.mtm, Rc::clone(&self.on_click));
            self.document.addSubview(&row.view);
            rows.push(row);
        }
        let mut y = 0.0;
        for (index, row) in rows.iter().enumerate() {
            match specs.get(index) {
                Some(spec) => {
                    let height = row.configure(spec, y, width, kill_target, index > 0);
                    y += height;
                    row.view.setHidden(false);
                }
                None => row.view.setHidden(true),
            }
        }
        self.document
            .setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, y)));
        set_text(&self.empty, empty_message);
        self.empty.setHidden(!specs.is_empty());
    }
}

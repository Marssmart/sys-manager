//! Popover layout and rendering: segmented control, header, list, process details and footer.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSButton, NSColor, NSControlSize, NSImage, NSLineBreakMode,
    NSPopover, NSPopoverBehavior, NSProgressIndicator, NSProgressIndicatorStyle,
    NSSegmentDistribution, NSSegmentSwitchTracking, NSSegmentedControl, NSStatusBar, NSStatusItem,
    NSTextField, NSVariableStatusItemLength, NSViewController, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSDateFormatter, NSDateFormatterStyle, NSRectEdge, NSSize, NSString};

use sys_manager_common::actions::ItemAction;
use sys_manager_common::disk::ScanState;
use sys_manager_common::format::{format_bytes, format_percent};
use sys_manager_common::model::{AppModel, Tab};

use super::details::{DetailsView, DETAILS_HEIGHT};
use super::item_panel::{ItemPanel, ITEM_PANEL_HEIGHT};
use super::list::{ListView, RowContent, RowSpec};
use super::views::FlippedView;
use super::widgets::{button, font, frame, label, set_text, wire, MEDIUM, REGULAR};

const WIDTH: f64 = 392.0;
const HEIGHT: f64 = 560.0;
const PAD: f64 = 14.0;
const TITLE_Y: f64 = 46.0;
const LIST_TOP: f64 = 96.0;
const FOOTER_HEIGHT: f64 = 40.0;
const GAP: f64 = 8.0;
const DETAILS_TOP: f64 = HEIGHT - FOOTER_HEIGHT - GAP - DETAILS_HEIGHT;
const ITEM_PANEL_TOP: f64 = HEIGHT - FOOTER_HEIGHT - GAP - ITEM_PANEL_HEIGHT;
const INDENT: f64 = 22.0;

pub struct Ui {
    mtm: MainThreadMarker,
    popover: Retained<NSPopover>,
    status_item: Retained<NSStatusItem>,
    tabs: Retained<NSSegmentedControl>,
    spinner: Retained<NSProgressIndicator>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    action: Retained<NSButton>,
    list: ListView,
    details: DetailsView,
    item_panel: ItemPanel,
    date_format: Retained<NSDateFormatter>,
}

impl Ui {
    pub fn build(
        mtm: MainThreadMarker,
        target: &AnyObject,
        on_row_click: Rc<dyn Fn(isize)>,
    ) -> Self {
        let root = FlippedView::new(mtm, frame(0.0, 0.0, WIDTH, HEIGHT));

        let effect = NSVisualEffectView::new(mtm);
        effect.setFrame(frame(0.0, 0.0, WIDTH, HEIGHT));
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&effect);

        let tabs = NSSegmentedControl::new(mtm);
        tabs.setSegmentCount(2);
        tabs.setLabel_forSegment(&NSString::from_str("Disk Cleaner"), 0);
        tabs.setLabel_forSegment(&NSString::from_str("Processes"), 1);
        tabs.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        tabs.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        tabs.setSelectedSegment(0);
        wire(&tabs, target, sel!(selectTab:));
        tabs.setFrame(frame(PAD, 12.0, WIDTH - 2.0 * PAD, 26.0));
        root.addSubview(&tabs);

        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(false);
        spinner.setFrame(frame(PAD + 3.0, TITLE_Y + 2.0, 14.0, 14.0));
        root.addSubview(&spinner);

        let title = label(mtm, &font(13.0, MEDIUM), &NSColor::labelColor());
        root.addSubview(&title);

        let detail = label(mtm, &font(11.0, REGULAR), &NSColor::secondaryLabelColor());
        detail.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        root.addSubview(&detail);

        let action = button(mtm, "Rescan", Some(target), Some(sel!(rescan:)));
        action.setFrame(frame(WIDTH - PAD - 80.0, TITLE_Y - 4.0, 80.0, 24.0));
        root.addSubview(&action);

        let list = ListView::new(mtm, on_row_click);
        root.addSubview(list.group());

        let details = DetailsView::new(
            mtm,
            frame(PAD, DETAILS_TOP, WIDTH - 2.0 * PAD, DETAILS_HEIGHT),
        );
        root.addSubview(details.group());

        let item_panel = ItemPanel::new(
            mtm,
            frame(PAD, ITEM_PANEL_TOP, WIDTH - 2.0 * PAD, ITEM_PANEL_HEIGHT),
            target,
        );
        root.addSubview(item_panel.group());

        let quit = button(mtm, "Quit", Some(target), Some(sel!(quit:)));
        quit.setFrame(frame(WIDTH - PAD - 64.0, HEIGHT - 33.0, 64.0, 24.0));
        root.addSubview(&quit);

        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(status_button) = status_item.button(mtm) {
            let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("internaldrive"),
                Some(&NSString::from_str("Sys Manager")),
            );
            match symbol {
                Some(image) => {
                    image.setTemplate(true);
                    status_button.setImage(Some(&image));
                }
                None => status_button.setTitle(&NSString::from_str("Sys")),
            }
            wire(&status_button, target, sel!(togglePopover:));
        }

        let content = NSViewController::new(mtm);
        content.setView(&root);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(WIDTH, HEIGHT));
        popover.setContentViewController(Some(&content));

        let date_format = NSDateFormatter::new();
        date_format.setDateStyle(NSDateFormatterStyle::MediumStyle);
        date_format.setTimeStyle(NSDateFormatterStyle::ShortStyle);

        Self {
            mtm,
            popover,
            status_item,
            tabs,
            spinner,
            title,
            detail,
            action,
            list,
            details,
            item_panel,
            date_format,
        }
    }

    pub fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    pub fn is_shown(&self) -> bool {
        self.popover.isShown()
    }

    pub fn show(&self) {
        if let Some(status_button) = self.status_item.button(self.mtm) {
            self.popover.showRelativeToRect_ofView_preferredEdge(
                status_button.bounds(),
                &status_button,
                NSRectEdge::MinY,
            );
        }
    }

    pub fn close(&self) {
        unsafe { self.popover.performClose(None) };
    }

    pub fn selected_segment(&self) -> isize {
        self.tabs.selectedSegment()
    }

    pub fn reset_scroll(&self) {
        self.list.scroll_to_top();
    }

    pub fn render(&self, model: &AppModel, target: &AnyObject) {
        let index = match model.tab() {
            Tab::DiskCleaner => 0,
            Tab::Processes => 1,
        };
        self.tabs.setSelectedSegment(index);
        match model.tab() {
            Tab::DiskCleaner => self.render_disk(model, target),
            Tab::Processes => self.render_processes(model, target),
        }
    }

    fn render_disk(&self, model: &AppModel, target: &AnyObject) {
        let scanning = model
            .scan()
            .is_some_and(|scan| scan.state == ScanState::Scanning);
        let busy = model.pending_item().is_some();
        let removing = model
            .pending_item()
            .is_some_and(|pending| pending.action == ItemAction::Remove);
        self.details.group().setHidden(true);
        self.item_panel.group().setHidden(false);
        self.list.set_frame(frame(
            PAD,
            LIST_TOP,
            WIDTH - 2.0 * PAD,
            ITEM_PANEL_TOP - GAP - LIST_TOP,
        ));
        let (status, status_is_error) = model.disk_status_line();
        self.set_header(
            &model.disk_title(),
            &status,
            status_is_error,
            scanning || busy,
        );
        self.action.setHidden(false);
        self.action.setEnabled(!removing);
        if scanning {
            self.action.setTitle(&NSString::from_str("Stop"));
            wire(&self.action, target, sel!(stopScan:));
        } else {
            self.action.setTitle(&NSString::from_str("Rescan"));
            wire(&self.action, target, sel!(rescan:));
        }
        let selected = model.selected_item();
        self.item_panel
            .show(selected, model.can_change_items(), busy);
        let selected_path = selected.map(|entry| entry.path.as_path());
        let specs: Vec<RowSpec> = model
            .disk_entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| RowSpec {
                content: RowContent::Item {
                    title: entry.name(),
                    location: entry.location(),
                    size: format_bytes(entry.size),
                    locked: entry.locked,
                },
                selected: selected_path == Some(entry.path.as_path()),
                tag: index as isize,
            })
            .collect();
        let empty = if scanning {
            "Looking for large items…"
        } else {
            "No large items found."
        };
        self.list.set_rows(&specs, target, empty);
    }

    fn render_processes(&self, model: &AppModel, target: &AnyObject) {
        self.action.setHidden(true);
        self.item_panel.group().setHidden(true);
        self.details.group().setHidden(false);
        self.list.set_frame(frame(
            PAD,
            LIST_TOP,
            WIDTH - 2.0 * PAD,
            DETAILS_TOP - GAP - LIST_TOP,
        ));
        let (notice_text, notice_is_error) = match model.notice() {
            Some(notice) => (notice.text.as_str(), notice.is_error),
            None => ("", false),
        };
        self.set_header(&model.process_title(), notice_text, notice_is_error, false);

        let selected = model.selected_process();
        let parent_name = selected
            .and_then(|process| process.parent_pid)
            .and_then(|parent| {
                model
                    .processes()
                    .iter()
                    .find(|candidate| candidate.pid == parent)
            })
            .map(|candidate| candidate.name.as_str());
        self.details.show(selected, parent_name, &self.date_format);

        let specs: Vec<RowSpec> = model
            .processes()
            .iter()
            .map(|process| RowSpec {
                content: RowContent::Process {
                    title: process.name.clone(),
                    pid_text: format!("PID {}", process.pid),
                    cpu_text: format!("CPU {}", format_percent(process.cpu_percent)),
                    memory_text: format!("Mem {}", format_bytes(process.memory_bytes)),
                },
                selected: selected.is_some_and(|chosen| chosen.pid == process.pid),
                tag: process.pid as isize,
            })
            .collect();
        self.list.set_rows(&specs, target, "No processes found.");
    }

    fn set_header(&self, title: &str, detail: &str, detail_is_error: bool, indented: bool) {
        let left = if indented { PAD + INDENT } else { PAD };
        self.title
            .setFrame(frame(left, TITLE_Y, WIDTH - left - PAD - 96.0, 18.0));
        set_text(&self.title, title);
        self.spinner.setHidden(!indented);
        if indented {
            unsafe { self.spinner.startAnimation(None) };
        } else {
            unsafe { self.spinner.stopAnimation(None) };
        }
        self.detail
            .setFrame(frame(PAD, TITLE_Y + 21.0, WIDTH - 2.0 * PAD, 15.0));
        set_text(&self.detail, detail);
        let color = if detail_is_error {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.detail.setTextColor(Some(&color));
    }
}

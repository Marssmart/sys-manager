//! Menu bar controller. Owns the workers and the popover, and runs the AppKit event loop.

mod details;
mod item_panel;
mod list;
mod popover;
mod views;
mod widgets;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSButton};
use objc2_foundation::{NSObject, NSTimer};

use sys_manager_common::actions::ItemAction;
use sys_manager_common::model::{AppModel, Notice, Tab};

use crate::disk::{startup_disk_options, DiskScan};
use crate::items::{open_item, refusal_before_removal, ItemCommand, ItemWorker};
use crate::process::{ProcessEvent, ProcessWorker};

use item_panel::confirm_removal;
use popover::Ui;

const TICK_SECONDS: f64 = 0.25;

pub struct ControllerIvars {
    model: RefCell<AppModel>,
    disk: RefCell<Option<DiskScan>>,
    processes: ProcessWorker,
    items: ItemWorker,
    ui: OnceCell<Ui>,
    dirty: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ControllerIvars]
    struct AppController;

    impl AppController {
        #[unsafe(method(togglePopover:))]
        fn toggle_popover(&self, _sender: Option<&AnyObject>) {
            self.toggle();
        }

        #[unsafe(method(selectTab:))]
        fn select_tab(&self, _sender: Option<&AnyObject>) {
            let ivars = self.ivars();
            let Some(ui) = ivars.ui.get() else {
                return;
            };
            let tab = if ui.selected_segment() == 1 {
                Tab::Processes
            } else {
                Tab::DiskCleaner
            };
            ivars.model.borrow_mut().select_tab(tab);
            ui.reset_scroll();
            ivars.dirty.set(true);
            self.render_if_visible();
        }

        #[unsafe(method(rescan:))]
        fn rescan(&self, _sender: Option<&AnyObject>) {
            self.start_scan();
            self.render_if_visible();
        }

        #[unsafe(method(stopScan:))]
        fn stop_scan(&self, _sender: Option<&AnyObject>) {
            if let Some(scan) = self.ivars().disk.borrow().as_ref() {
                scan.cancel();
            }
            self.ivars().dirty.set(true);
            self.render_if_visible();
        }

        #[unsafe(method(killProcess:))]
        fn kill_process(&self, sender: Option<&AnyObject>) {
            let Some(button) = sender.and_then(|sender| sender.downcast_ref::<NSButton>()) else {
                return;
            };
            if let Ok(pid) = u32::try_from(button.tag()) {
                self.kill(pid);
            }
        }

        #[unsafe(method(openItem:))]
        fn open_item_clicked(&self, _sender: Option<&AnyObject>) {
            self.open_selected();
        }

        #[unsafe(method(toggleLock:))]
        fn toggle_lock_clicked(&self, _sender: Option<&AnyObject>) {
            self.toggle_selected_lock();
        }

        #[unsafe(method(removeItem:))]
        fn remove_item_clicked(&self, _sender: Option<&AnyObject>) {
            self.remove_selected();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::new().expect("actions run on the main thread");
            NSApplication::sharedApplication(mtm).terminate(None);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.poll();
        }
    }
);

impl AppController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ControllerIvars {
            model: RefCell::new(AppModel::default()),
            disk: RefCell::new(None),
            processes: ProcessWorker::start(),
            items: ItemWorker::start(),
            ui: OnceCell::new(),
            dirty: Cell::new(true),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let owner = this.clone();
        let on_row_click: Rc<dyn Fn(isize)> = Rc::new(move |tag| owner.select_row(tag));
        let target: &AnyObject = &this;
        let ui = Ui::build(mtm, target, on_row_click);
        let _ = this.ivars().ui.set(ui);
        this
    }

    // activate() needs macOS 14; activateIgnoringOtherApps keeps older supported systems working.
    #[allow(deprecated)]
    fn toggle(&self) {
        let ivars = self.ivars();
        let Some(ui) = ivars.ui.get() else {
            return;
        };
        if ui.is_shown() {
            ui.close();
            return;
        }
        NSApplication::sharedApplication(ui.mtm()).activateIgnoringOtherApps(true);
        if ivars.disk.borrow().is_none() {
            self.start_scan();
        }
        ui.show();
        ivars.dirty.set(true);
        self.render_if_visible();
    }

    fn start_scan(&self) {
        let ivars = self.ivars();
        *ivars.disk.borrow_mut() = Some(DiskScan::start(startup_disk_options()));
        ivars.model.borrow_mut().reset_scan();
        ivars.dirty.set(true);
    }

    fn kill(&self, pid: u32) {
        let ivars = self.ivars();
        let name = ivars
            .model
            .borrow()
            .processes()
            .iter()
            .find(|process| process.pid == pid)
            .map_or_else(|| format!("PID {pid}"), |process| process.name.clone());
        ivars.processes.kill(pid, name);
    }

    /// Selects the disk item or process whose row was clicked. A disk row's tag is its index in the list.
    fn select_row(&self, tag: isize) {
        let ivars = self.ivars();
        {
            let mut model = ivars.model.borrow_mut();
            match model.tab() {
                Tab::DiskCleaner => {
                    if let Ok(index) = usize::try_from(tag) {
                        model.toggle_item(index);
                    }
                }
                Tab::Processes => {
                    if let Ok(pid) = u32::try_from(tag) {
                        model.toggle_selection(pid);
                    }
                }
            }
        }
        ivars.dirty.set(true);
        self.render_if_visible();
    }

    fn open_selected(&self) {
        let ivars = self.ivars();
        let path = {
            let model = ivars.model.borrow();
            if model.pending_item().is_some() {
                return;
            }
            match model.selected_item() {
                Some(entry) => entry.path.clone(),
                None => return,
            }
        };
        let result = open_item(&path);
        ivars
            .model
            .borrow_mut()
            .report_item_result(ItemAction::Open, &path, &result);
        ivars.dirty.set(true);
        self.render_if_visible();
    }

    fn toggle_selected_lock(&self) {
        let ivars = self.ivars();
        let (path, action) = {
            let model = ivars.model.borrow();
            if model.pending_item().is_some() {
                return;
            }
            match model.selected_item() {
                Some(entry) if entry.locked => (entry.path.clone(), ItemAction::Unlock),
                Some(entry) => (entry.path.clone(), ItemAction::Lock),
                None => return,
            }
        };
        ivars.model.borrow_mut().start_item_action(action, &path);
        ivars.items.submit(ItemCommand::SetLocked {
            path,
            locked: action == ItemAction::Lock,
        });
        ivars.dirty.set(true);
        self.render_if_visible();
    }

    /// Refuses what the disk list can already see is blocked, then asks before moving the item to the Trash.
    fn remove_selected(&self) {
        let ivars = self.ivars();
        let entry = {
            let model = ivars.model.borrow();
            if model.pending_item().is_some() || !model.can_change_items() {
                return;
            }
            match model.selected_item() {
                Some(entry) => entry.clone(),
                None => return,
            }
        };
        if let Some(refusal) = refusal_before_removal(&entry.path, entry.locked) {
            ivars.model.borrow_mut().report_item_result(
                ItemAction::Remove,
                &entry.path,
                &Err(refusal),
            );
        } else {
            let mtm = MainThreadMarker::new().expect("actions run on the main thread");
            if !confirm_removal(mtm, &entry) {
                return;
            }
            ivars
                .model
                .borrow_mut()
                .start_item_action(ItemAction::Remove, &entry.path);
            ivars.items.submit(ItemCommand::Remove(entry.path.clone()));
        }
        ivars.dirty.set(true);
        self.render_if_visible();
    }

    /// Applies worker results to the model, then redraws if the popover is open.
    fn poll(&self) {
        let ivars = self.ivars();
        for event in ivars.processes.drain_events() {
            let mut model = ivars.model.borrow_mut();
            match event {
                ProcessEvent::Listed(processes) => model.set_processes(processes),
                ProcessEvent::KillFinished { pid, name, result } => {
                    model.set_notice(Some(Notice::for_kill(pid, &name, &result)));
                }
            }
            ivars.dirty.set(true);
        }
        for event in ivars.items.drain_events() {
            ivars
                .model
                .borrow_mut()
                .report_item_result(event.action, &event.path, &event.result);
            ivars.dirty.set(true);
        }
        if let Some(scan) = ivars.disk.borrow().as_ref() {
            if let Some(snapshot) = scan.take_latest() {
                ivars.model.borrow_mut().set_scan(snapshot);
                ivars.dirty.set(true);
            }
        }
        self.render_if_visible();
    }

    fn render_if_visible(&self) {
        let ivars = self.ivars();
        let Some(ui) = ivars.ui.get() else {
            return;
        };
        if ui.is_shown() && ivars.dirty.replace(false) {
            let model = ivars.model.borrow();
            ui.render(&model, self);
        }
    }
}

/// Runs the menu bar app on the main thread until it is quit.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("Sys Manager must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let controller = AppController::new(mtm);
    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            TICK_SECONDS,
            &controller,
            sel!(tick:),
            None,
            true,
        )
    };
    app.run();
}

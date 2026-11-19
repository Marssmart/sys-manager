//! Custom AppKit views: a top-left origin container, the inset group background and list rows.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBezierPath, NSColor, NSEvent, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};

const GROUP_RADIUS: f64 = 10.0;
const ROW_HIGHLIGHT_RADIUS: f64 = 8.0;
const SEPARATOR_INSET: f64 = 14.0;

/// Called with the row's tag when the row is clicked.
pub type ClickHandler = Rc<dyn Fn(isize)>;

// Views use a top-left origin so layout can run top to bottom.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    pub struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    pub struct GroupView;

    impl GroupView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let outline = NSRect::new(
                NSPoint::new(0.5, 0.5),
                NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0),
            );
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                outline,
                GROUP_RADIUS,
                GROUP_RADIUS,
            );
            NSColor::controlBackgroundColor()
                .colorWithAlphaComponent(0.72)
                .setFill();
            path.fill();
            NSColor::separatorColor().setStroke();
            path.stroke();
        }
    }
);

impl GroupView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub struct RowIvars {
    selected: Cell<bool>,
    separator: Cell<bool>,
    clickable: Cell<bool>,
    tag: Cell<isize>,
    on_click: RefCell<Option<ClickHandler>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowIvars]
    pub struct RowView;

    impl RowView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let ivars = self.ivars();
            let bounds = self.bounds();
            if ivars.selected.get() {
                let highlight = NSRect::new(
                    NSPoint::new(4.0, 1.0),
                    NSSize::new(bounds.size.width - 8.0, bounds.size.height - 2.0),
                );
                NSColor::systemBlueColor()
                    .colorWithAlphaComponent(0.12)
                    .setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    highlight,
                    ROW_HIGHLIGHT_RADIUS,
                    ROW_HIGHLIGHT_RADIUS,
                )
                .fill();
            }
            if ivars.separator.get() {
                let line = NSRect::new(
                    NSPoint::new(SEPARATOR_INSET, 0.0),
                    NSSize::new(bounds.size.width - 2.0 * SEPARATOR_INSET, 0.5),
                );
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(line);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let ivars = self.ivars();
            if !ivars.clickable.get() {
                return;
            }
            let callback = ivars.on_click.borrow().clone();
            if let Some(callback) = callback {
                callback(ivars.tag.get());
            }
        }
    }
);

impl RowView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(RowIvars {
            selected: Cell::new(false),
            separator: Cell::new(false),
            clickable: Cell::new(false),
            tag: Cell::new(0),
            on_click: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set_on_click(&self, callback: Rc<dyn Fn(isize)>) {
        *self.ivars().on_click.borrow_mut() = Some(callback);
    }

    pub fn configure(&self, selected: bool, separator: bool, clickable: bool, tag: isize) {
        let ivars = self.ivars();
        ivars.selected.set(selected);
        ivars.separator.set(separator);
        ivars.clickable.set(clickable);
        ivars.tag.set(tag);
        self.setNeedsDisplay(true);
    }
}

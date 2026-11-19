//! Font, color, label and button helpers shared by the popover views.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSButton, NSColor, NSControl, NSControlSize, NSFont, NSLineBreakMode, NSTextField,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Font weights matching `NSFontWeightRegular` and `NSFontWeightMedium`.
pub const REGULAR: f64 = 0.0;
pub const MEDIUM: f64 = 0.23;

pub fn frame(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

pub fn font(size: f64, weight: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight)
}

pub fn digits_font(size: f64, weight: f64) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
}

pub fn label(mtm: MainThreadMarker, font: &NSFont, color: &NSColor) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    field.setFont(Some(font));
    field.setTextColor(Some(color));
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

pub fn set_text(field: &NSTextField, text: &str) {
    field.setStringValue(&NSString::from_str(text));
}

pub fn button(
    mtm: MainThreadMarker,
    title: &str,
    target: Option<&AnyObject>,
    action: Option<Sel>,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(title), target, action, mtm)
    };
    button.setControlSize(NSControlSize::Small);
    button
}

/// Connects a control to its target and action. The selectors are implemented by the app controller.
pub fn wire(control: &NSControl, target: &AnyObject, action: Sel) {
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(action));
    }
}

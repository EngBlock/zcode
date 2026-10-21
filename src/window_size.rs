//! A project window's least size, from what's in it: AppKit keeps the user
//! from making a window smaller than its `contentMinSize`, which the
//! workspace sets as its panes need (`Workspace::render_editor_area`), and
//! a window already smaller than that grows to it, as Xcode's does. Also
//! the control channel's `window` command.

use gpui_kit::{App, Window};
use objc2::rc::Retained;
use objc2_app_kit::{NSWindow, NSWindowStyleMask};
use objc2_foundation::NSSize;

/// The AppKit window behind a gpui window.
pub fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
    // SAFETY: gpui's AppKit handle points at its window's live content view.
    let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}

/// The window's content size and its least content size, in points.
pub fn content_sizes(window: &NSWindow) -> ((f64, f64), (f64, f64)) {
    let content = window.contentRectForFrameRect(window.frame()).size;
    let min = window.contentMinSize();
    ((content.width, content.height), (min.width, min.height))
}

/// Keeps the user from making the window's content smaller than `min`; a
/// window smaller than that now grows to it (within its screen, and not in
/// full screen), after the current update.
pub fn set_content_min_size(window: Retained<NSWindow>, min: (f64, f64), cx: &App) {
    window.setContentMinSize(NSSize::new(min.0, min.1));
    let ((width, height), _) = content_sizes(&window);
    if (width >= min.0 && height >= min.1) || window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    resize_content(window, (width.max(min.0), height.max(min.1)), cx);
}

/// Makes the window's content `size`, its top left staying where it is and
/// the window within its screen's visible area. After the current update:
/// AppKit tells gpui of the resize while it happens.
pub fn resize_content(window: Retained<NSWindow>, size: (f64, f64), cx: &App) {
    cx.foreground_executor()
        .spawn(async move {
            let frame = window.frame();
            let mut content = window.contentRectForFrameRect(frame);
            content.size = NSSize::new(size.0, size.1);
            let mut new = window.frameRectForContentRect(content);
            new.origin.x = frame.origin.x;
            new.origin.y = frame.origin.y + frame.size.height - new.size.height;
            if let Some(screen) = window.screen() {
                let visible = screen.visibleFrame();
                new.size.width = new.size.width.min(visible.size.width);
                new.size.height = new.size.height.min(visible.size.height);
                let (right, top) = (visible.origin.x + visible.size.width, visible.origin.y + visible.size.height);
                new.origin.x = new.origin.x.min(right - new.size.width).max(visible.origin.x);
                new.origin.y = new.origin.y.min(top - new.size.height).max(visible.origin.y);
            }
            window.setFrame_display(new, true);
        })
        .detach();
}

/// The control channel's `window size` (the content's size and least
/// size, as JSON) and `window resize <width> <height>` (as a drag of the
/// window's corner to that size would: no smaller than its least size).
pub fn command(arg: &str, window: &Window, cx: &App) -> Result<String, String> {
    let ns = ns_window(window).ok_or("no AppKit window")?;
    let (size, min) = content_sizes(&ns);
    match arg.split_whitespace().collect::<Vec<_>>()[..] {
        ["size"] => Ok(serde_json::json!({ "size": [size.0, size.1], "min_size": [min.0, min.1] }).to_string()),
        ["resize", width, height] => {
            let (Ok(width), Ok(height)) = (width.parse::<f64>(), height.parse::<f64>()) else {
                return Err("usage: window resize <width> <height>".into());
            };
            resize_content(ns, (width.max(min.0), height.max(min.1)), cx);
            Ok(String::new())
        }
        _ => Err("window size | window resize <width> <height>".into()),
    }
}

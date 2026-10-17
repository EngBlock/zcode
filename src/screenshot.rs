//! Pictures of zcode's own windows (no Screen Recording permission is
//! needed for a process's own windows) and of a simulator's screen. The
//! control channel (`debug.rs`), the startup benchmark (`perf.rs`) and the
//! MCP server's `screenshot` tool use them.

use std::path::Path;

use gpui_kit::Window;

/// The window server's number for a gpui window.
pub fn window_number(window: &Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
    // SAFETY: gpui's AppKit handle points at its window's live content view.
    let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast().as_ref() };
    Some(view.window()?.windowNumber() as u32)
}

/// Writes window `number` as a PNG, at the display's resolution.
pub fn write_window_png(number: u32, path: &Path) -> Result<(), String> {
    unsafe { ffi::write_window_png(number, path, 1.) }
}

/// Window `number` as PNG bytes, scaled down by `scale` (the window's
/// scale factor: a pixel per point, a quarter of a Retina capture's size,
/// which is plenty for a model to read).
pub fn window_png(number: u32, scale: f32) -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!("zcode-window-{}-{number}.png", std::process::id()));
    unsafe { ffi::write_window_png(number, &path, scale.max(1.) as f64) }?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string());
    std::fs::remove_file(&path).ok();
    bytes
}

/// A booted simulator's screen as PNG bytes (`simctl io screenshot`).
/// Blocks for as long as simctl takes: call it off the main thread.
pub fn simulator_png(udid: &str) -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!("zcode-simulator-{}-{udid}.png", std::process::id()));
    let output = std::process::Command::new("/usr/bin/xcrun")
        .args(["simctl", "io", udid, "screenshot", "--type=png"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn’t run simctl: {e}"))?;
    if !output.status.success() {
        std::fs::remove_file(&path).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("simctl failed").trim().to_string());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string());
    std::fs::remove_file(&path).ok();
    bytes
}

/// Whether the window server already shows drawn content in window `number`:
/// the area below the title bar has more than one color. `None` if the window
/// can't be captured.
pub fn window_shows_content(number: u32) -> Option<bool> {
    unsafe { ffi::window_shows_content(number) }
}

mod ffi {
    use std::ffi::{CString, c_void};
    use std::path::Path;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreateImage(rect: CGRect, list: u32, window: u32, options: u32) -> *const c_void;
        fn CGImageRelease(image: *const c_void);
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGImageGetBytesPerRow(image: *const c_void) -> usize;
        fn CGImageGetBitsPerPixel(image: *const c_void) -> usize;
        fn CGImageGetDataProvider(image: *const c_void) -> *const c_void;
        fn CGDataProviderCopyData(provider: *const c_void) -> *const c_void;
        fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
        fn CGColorSpaceRelease(space: *const c_void);
        fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bits: usize, row: usize, space: *const c_void, info: u32) -> *const c_void;
        fn CGBitmapContextCreateImage(context: *const c_void) -> *const c_void;
        fn CGContextSetInterpolationQuality(context: *const c_void, quality: i32);
        fn CGContextDrawImage(context: *const c_void, rect: CGRect, image: *const c_void);
        fn CGContextRelease(context: *const c_void);
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithURL(url: *const c_void, ty: *const c_void, count: usize, options: *const c_void) -> *const c_void;
        fn CGImageDestinationAddImage(dest: *const c_void, image: *const c_void, props: *const c_void);
        fn CGImageDestinationFinalize(dest: *const c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, dir: bool) -> *const c_void;
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const i8, encoding: u32) -> *const c_void;
        fn CFRelease(p: *const c_void);
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
        fn CFDataGetLength(data: *const c_void) -> isize;
    }

    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1;
    const NOMINAL_RESOLUTION: u32 = 1 << 9;

    pub unsafe fn window_shows_content(window: u32) -> Option<bool> {
        unsafe {
            let null_rect = CGRect { x: f64::INFINITY, y: f64::INFINITY, w: 0., h: 0. };
            let image = CGWindowListCreateImage(
                null_rect,
                INCLUDING_WINDOW,
                window,
                BOUNDS_IGNORE_FRAMING | NOMINAL_RESOLUTION,
            );
            if image.is_null() {
                return None;
            }
            let (width, height) = (CGImageGetWidth(image), CGImageGetHeight(image));
            let row_bytes = CGImageGetBytesPerRow(image);
            let ok = CGImageGetBitsPerPixel(image) == 32;
            let data = CGDataProviderCopyData(CGImageGetDataProvider(image));
            CGImageRelease(image);
            if data.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize);
            // Skip the top 15% (title bar, traffic lights) and compare every
            // other pixel below it with the first one.
            let mut differing = 0usize;
            let mut total = 0usize;
            let first = (height * 15 / 100) * row_bytes;
            let reference = bytes.get(first..first + 4).map(|p| [p[0], p[1], p[2], p[3]]);
            if let (true, Some(reference)) = (ok, reference) {
                for y in (height * 15 / 100..height * 95 / 100).step_by(2) {
                    for x in (width * 5 / 100..width * 95 / 100).step_by(2) {
                        let i = y * row_bytes + x * 4;
                        let Some(p) = bytes.get(i..i + 4) else { continue };
                        total += 1;
                        if p.iter().zip(reference).any(|(a, b)| a.abs_diff(b) > 24) {
                            differing += 1;
                        }
                    }
                }
            }
            CFRelease(data);
            ok.then_some(total > 0 && differing * 1000 > total)
        }
    }

    /// `image` drawn `scale` times smaller (it's released either way).
    unsafe fn scaled(image: *const c_void, scale: f64) -> *const c_void {
        if scale <= 1. {
            return image;
        }
        unsafe {
            let width = (CGImageGetWidth(image) as f64 / scale).round().max(1.) as usize;
            let height = (CGImageGetHeight(image) as f64 / scale).round().max(1.) as usize;
            let space = CGColorSpaceCreateDeviceRGB();
            // 8-bit RGBA, premultiplied.
            let context = CGBitmapContextCreate(std::ptr::null_mut(), width, height, 8, 0, space, 1);
            CGColorSpaceRelease(space);
            if context.is_null() {
                return image;
            }
            CGContextSetInterpolationQuality(context, 3);
            CGContextDrawImage(context, CGRect { x: 0., y: 0., w: width as f64, h: height as f64 }, image);
            let small = CGBitmapContextCreateImage(context);
            CGContextRelease(context);
            if small.is_null() {
                return image;
            }
            CGImageRelease(image);
            small
        }
    }

    pub unsafe fn write_window_png(window: u32, path: &Path, scale: f64) -> Result<(), String> {
        unsafe {
            let null_rect = CGRect { x: f64::INFINITY, y: f64::INFINITY, w: 0., h: 0. };
            let image = CGWindowListCreateImage(null_rect, INCLUDING_WINDOW, window, BOUNDS_IGNORE_FRAMING);
            if image.is_null() {
                return Err("CGWindowListCreateImage returned null".into());
            }
            let image = scaled(image, scale);
            let bytes = path.to_string_lossy().into_owned();
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, false);
            let ty = CString::new("public.png").unwrap();
            let ty = CFStringCreateWithCString(std::ptr::null(), ty.as_ptr(), 0x0800_0100);
            let dest = CGImageDestinationCreateWithURL(url, ty, 1, std::ptr::null());
            let ok = !dest.is_null() && {
                CGImageDestinationAddImage(dest, image, std::ptr::null());
                CGImageDestinationFinalize(dest)
            };
            if !dest.is_null() {
                CFRelease(dest);
            }
            CFRelease(ty);
            CFRelease(url);
            CGImageRelease(image);
            if ok { Ok(()) } else { Err("failed to write PNG".into()) }
        }
    }
}

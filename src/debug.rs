//! A scriptable control channel for automated UI testing, enabled only when
//! `ZCODE_DEBUG_DIR` is set. zcode polls `$ZCODE_DEBUG_DIR/commands`, runs
//! each line, deletes the file, and appends results to `$ZCODE_DEBUG_DIR/log`.
//!
//! Commands:
//!   screenshot <path.png>      capture zcode's key window (no permissions needed)
//!   action <namespace::Name>   dispatch an action to the focused element
//!   keys <keystroke> ...       simulate keystrokes, e.g. `keys cmd-shift-o`
//!   type <text>                type text into the focused input
//!   open <path>[:line]         open a file in the frontmost workspace
//!   wait <ms>                  pause
//!   dump                       log workspace state as JSON
//!   quit

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;

use crate::workspace::{Target, Workspace};

pub fn start(cx: &mut App, workspaces: impl Fn(&mut App) -> Vec<Entity<Workspace>> + 'static) {
    let Some(dir) = std::env::var_os("ZCODE_DEBUG_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).ok();
    let commands = dir.join("commands");
    let log_path = dir.join("log");
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let Ok(script) = std::fs::read_to_string(&commands) else { continue };
            std::fs::remove_file(&commands).ok();
            for line in script.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
                let arg = arg.trim().to_string();
                let result: Result<String, String> = match cmd {
                    "wait" => {
                        let ms = arg.parse().unwrap_or(100);
                        cx.background_executor().timer(Duration::from_millis(ms)).await;
                        Ok(String::new())
                    }
                    "quit" => {
                        cx.update(|cx| cx.quit());
                        Ok(String::new())
                    }
                    _ => cx.update(|cx| run_command(cmd, &arg, &workspaces, cx)),
                };
                let mut log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
                if let Some(log) = log.as_mut() {
                    match result {
                        Ok(out) => writeln!(log, "ok {line}{}", if out.is_empty() { String::new() } else { format!("\n{out}") }).ok(),
                        Err(err) => writeln!(log, "err {line}: {err}").ok(),
                    };
                }
                // Let the UI settle between commands.
                cx.background_executor().timer(Duration::from_millis(60)).await;
            }
        }
    })
    .detach();
}

fn front_window(cx: &App) -> Option<AnyWindowHandle> {
    cx.active_window().or_else(|| cx.windows().first().copied())
}

fn run_command(
    cmd: &str,
    arg: &str,
    workspaces: &dyn Fn(&mut App) -> Vec<Entity<Workspace>>,
    cx: &mut App,
) -> Result<String, String> {
    let window = front_window(cx).ok_or("no window")?;
    match cmd {
        "screenshot" => capture_key_window(Path::new(arg)).map(|_| String::new()),
        "action" => {
            let action = cx.build_action(arg, None).map_err(|e| e.to_string())?;
            window
                .update(cx, |_, window, cx| window.dispatch_action(action, cx))
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "keys" => {
            for key in arg.split_whitespace() {
                let keystroke = Keystroke::parse(key).map_err(|e| e.to_string())?;
                window
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .map_err(|e| e.to_string())?;
            }
            Ok(String::new())
        }
        "type" => {
            for ch in arg.chars() {
                let key = match ch {
                    ' ' => "space".to_string(),
                    '\n' => "enter".to_string(),
                    c => c.to_string(),
                };
                let mut keystroke = Keystroke::parse(&key).map_err(|e| e.to_string())?;
                keystroke.key_char = Some(ch.to_string());
                window
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .map_err(|e| e.to_string())?;
            }
            Ok(String::new())
        }
        "open" => {
            let (path, line) = match arg.rsplit_once(':') {
                Some((p, l)) if l.parse::<u32>().is_ok() => (p, l.parse::<u32>().ok()),
                _ => (arg, None),
            };
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let path = PathBuf::from(path);
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| {
                        let target = line.map(|l| Target::Line { line: l.saturating_sub(1), column: 0 });
                        ws.open_file(path, target, true, window, cx)
                    })
                })
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "dump" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            Ok(workspace.read(cx).debug_state(cx).to_string())
        }
        other => Err(format!("unknown command {other}")),
    }
}

/// Captures zcode's key (or main) window. A process may capture its own
/// windows without the Screen Recording permission.
fn capture_key_window(path: &Path) -> Result<(), String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let mtm = MainThreadMarker::new().ok_or("not on main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let window = app
        .keyWindow()
        .or_else(|| app.mainWindow())
        .or_else(|| app.windows().firstObject())
        .ok_or("no NSWindow")?;
    let number = window.windowNumber() as u32;
    unsafe { ffi::write_window_png(number, path) }
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
    }

    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1;

    pub unsafe fn write_window_png(window: u32, path: &Path) -> Result<(), String> {
        unsafe {
            let null_rect = CGRect { x: f64::INFINITY, y: f64::INFINITY, w: 0., h: 0. };
            let image = CGWindowListCreateImage(null_rect, INCLUDING_WINDOW, window, BOUNDS_IGNORE_FRAMING);
            if image.is_null() {
                return Err("CGWindowListCreateImage returned null".into());
            }
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

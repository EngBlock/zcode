//! A scriptable control channel for automated UI testing, enabled only when
//! `ZCODE_DEBUG_DIR` is set. zcode polls `$ZCODE_DEBUG_DIR/commands`, runs
//! each line, deletes the file, and appends results to `$ZCODE_DEBUG_DIR/log`.
//!
//! Commands:
//!   screenshot <path.png>      capture zcode's key window (no permissions needed)
//!   action <namespace::Name>   dispatch an action to the focused element
//!   keys <keystroke> ...       simulate keystrokes, e.g. `keys cmd-shift-o`
//!   type <text>                type text into the focused input
//!   mouse <x> <y> / click <x> <y>  move / click at window coordinates (points)
//!   open <path>[:line]         open a file in the frontmost workspace
//!   wait <ms>                  pause
//!   rename <path> -> <name>    rename a file or folder (a name containing `/` moves it)
//!   project-setting <KEY> <value>  edit a field of the open project settings
//!   dump                       log workspace state as JSON
//!   keymap                     log every command's key bindings as JSON
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
        "mouse" | "click" => {
            let mut parts = arg.split_whitespace().filter_map(|p| p.parse::<f32>().ok());
            let (Some(x), Some(y)) = (parts.next(), parts.next()) else {
                return Err("usage: mouse <x> <y> (window points)".into());
            };
            let position = point(px(x), px(y));
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                    if cmd == "click" {
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                            }),
                            cx,
                        );
                    }
                })
                .map_err(|e| e.to_string())?;
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
        "rename" => {
            let (from, to) = arg.split_once(" -> ").ok_or("usage: rename <path> -> <new name>")?;
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let (from, to) = (PathBuf::from(from), to.to_string());
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| ws.rename_path(from, to, window, cx))
                })
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "project-setting" => {
            let (key, value) = arg.split_once(' ').unwrap_or((arg, ""));
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let settings = workspace.read(cx).project_settings.clone().ok_or("project settings not open")?;
            window
                .update(cx, |_, window, cx| settings.update(cx, |s, cx| s.set_field(key, value, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "keymap" => {
            // Every menu/Quick Actions command with all keystrokes bound to it.
            // Includes context-specific (editor-only) bindings.
            let mut map = serde_json::Map::new();
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for command in crate::actions::commands() {
                if command.name == "-" {
                    continue;
                }
                let action = (command.action)();
                let keys: Vec<serde_json::Value> = keymap
                    .bindings()
                    .filter(|b| b.action().partial_eq(action.as_ref()))
                    .map(|b| {
                        serde_json::Value::String(
                            b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" "),
                        )
                    })
                    .collect();
                map.insert(action.name().to_string(), serde_json::Value::Array(keys));
            }
            let _ = window;
            Ok(serde_json::Value::Object(map).to_string())
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

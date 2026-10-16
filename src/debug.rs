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
//!   double-click <x> <y> / right-click <x> <y>
//!   drag <x1> <y1> <x2> <y2>   press at one point, move to the other and release (drag and drop)
//!   open <path>[:line]         open a file in the frontmost workspace
//!   wait <ms>                  pause
//!   rename <path> -> <name>    rename a file or folder (a name containing `/` moves it)
//!   project-setting <KEY> <value>  edit a field of the open project settings
//!   project-pane <target>|- <General|Signing|Info|BuildSettings>  select a target (`-`: the project) and pane
//!   signing <command>          edit the Signing & Capabilities tab (see `ProjectSettings::signing_command`)
//!   plist <command>            edit the property list editor on screen (see `PlistEditor::command`)
//!   open-as <path> source|plist  Open As ▸ Source Code / Property List
//!   new-file <path>            create a file as File ▸ New File does after its panel
//!   trash <path>               Move to Trash from the project navigator
//!   add-files <path>[;<path>…] -> <folder>  the Add Files sheet, after its open panel
//!   inspect <path>             show the File inspector for a file or folder
//!   membership <target> on|off toggle a Target Membership checkbox in the inspector
//!   run-test <Target/Class/test> run a test, class or target as the Test navigator's play button does
//!   destination <name|id>     choose a run destination, as its menu item does
//!   edit-scheme <field> [<value>]  edit the open scheme editor (see `EditSchemeDialog::set_field`)
//!   breakpoint <field> [<value>]   edit the open breakpoint editor (see `BreakpointEditor::set_field`)
//!   console <command>          run a command at the LLDB console's prompt
//!   bookmark <command>         a Bookmarks navigator command (see `Workspace::bookmark_command`)
//!   package <field> [<value>]  Add Package Dependencies: `open [query]`, `local <path>` (after Add
//!                              Local…'s panel), or a field of the open sheet (see `AddPackageDialog::set_field`)
//!   package-menu update|remove <identity>  a package's Update Package / Remove Package
//!   reveal <path>              reveal a file in the project navigator, package sources included
//!   variable <name> copy|describe  the Variables view's Copy Value / Print Description
//!   hover                      log the editor's hover (Quick Help or a data tip) at the cursor
//!   type-latency <text>        type `text`, logging keystroke-to-frame times as JSON
//!   editor <probe>             the editor's extension points (see `Workspace::editor_probe`)
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
                    "type-latency" => type_latency(&arg, cx).await,
                    "hover" => match cx.update(|cx| start_hover(&workspaces, cx)) {
                        Ok(task) => task.await.map_err(|e| format!("{e:#}")).map(|hover| hover_text(hover.as_ref())),
                        Err(err) => Err(err),
                    },
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

/// Types `text` a character at a time, timing each keystroke from its
/// dispatch to the end of the next frame: what typing feels like, minus the
/// display. Logs the median, 95th percentile and maximum in milliseconds.
async fn type_latency(text: &str, cx: &mut AsyncApp) -> Result<String, String> {
    let mut samples = vec![];
    for ch in text.chars() {
        let key = if ch == ' ' { "space".to_string() } else { ch.to_string() };
        let mut keystroke = Keystroke::parse(&key).map_err(|e| e.to_string())?;
        keystroke.key_char = Some(ch.to_string());
        let (tx, rx) = futures::channel::oneshot::channel();
        let start = std::time::Instant::now();
        cx.update(|cx| {
            let window = front_window(cx).ok_or("no window")?;
            window
                .update(cx, |_, window, cx| {
                    window.on_next_frame(move |_, _| {
                        tx.send(std::time::Instant::now()).ok();
                    });
                    window.dispatch_keystroke(keystroke, cx);
                })
                .map_err(|e| e.to_string())
        })?;
        if let Ok(end) = rx.await {
            samples.push((end - start).as_secs_f64() * 1000.);
        }
    }
    if samples.is_empty() {
        return Err("no frames".into());
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Ok(serde_json::json!({ "keys": samples.len(), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0) }).to_string())
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
        "mouse" | "click" | "double-click" | "right-click" => {
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
                    let (button, clicks) = match cmd {
                        "click" => (MouseButton::Left, 1),
                        "double-click" => (MouseButton::Left, 2),
                        "right-click" => (MouseButton::Right, 1),
                        _ => (MouseButton::Left, 0),
                    };
                    for click_count in 1..=clicks {
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button,
                                position,
                                modifiers: Modifiers::default(),
                                click_count,
                                first_mouse: false,
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button,
                                position,
                                modifiers: Modifiers::default(),
                                click_count,
                            }),
                            cx,
                        );
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "drag" => {
            let coords: Vec<f32> = arg.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            let [x1, y1, x2, y2] = coords[..] else {
                return Err("usage: drag <x1> <y1> <x2> <y2> (window points)".into());
            };
            let (from, to) = (point(px(x1), px(y1)), point(px(x2), px(y2)));
            window
                .update(cx, |_, window, cx| {
                    let moved = |position, pressed_button| {
                        PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button, modifiers: Modifiers::default() })
                    };
                    window.dispatch_event(moved(from, None), cx);
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: from,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                        cx,
                    );
                    // In steps, as a hand would: the drag starts once the
                    // pointer leaves the press point.
                    for step in 1..=8 {
                        let t = step as f32 / 8.;
                        window.dispatch_event(moved(from + (to - from) * t, Some(MouseButton::Left)), cx);
                    }
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: to,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
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
        "project-pane" | "signing" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let settings = workspace.read(cx).project_settings.clone().ok_or("project settings not open")?;
            window
                .update(cx, |_, window, cx| {
                    settings.update(cx, |s, cx| {
                        if cmd == "signing" {
                            return s.signing_command(arg, window, cx);
                        }
                        let (target, pane) = arg.rsplit_once(' ').ok_or("usage: project-pane <target>|- <pane>")?;
                        s.show(if target == "-" { "" } else { target }, pane, window, cx)
                    })
                })
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "plist" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.plist_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "open-as" => {
            let (path, mode) = arg.rsplit_once(' ').ok_or("usage: open-as <path> source|plist")?;
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let path = PathBuf::from(path);
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.open_file_as(path, mode == "source", window, cx)))
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "new-file" | "trash" | "inspect" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let path = PathBuf::from(arg);
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| match cmd {
                        "new-file" => ws.create_file(path, window, cx),
                        "trash" => ws.move_to_trash(path, window, cx),
                        _ => ws.show_file_inspector(Some(path), cx),
                    })
                })
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "add-files" => {
            let (paths, folder) = arg.split_once(" -> ").ok_or("usage: add-files <path>[;<path>…] -> <folder>")?;
            let paths: Vec<PathBuf> = paths.split(';').map(PathBuf::from).collect();
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let folder = PathBuf::from(folder);
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.show_add_files(paths, folder, window, cx)))
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "edit-scheme" => {
            let (field, value) = arg.split_once(' ').unwrap_or((arg, ""));
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.edit_scheme_field(field, value, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "editor" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.editor_probe(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "bookmark" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.bookmark_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "breakpoint" | "console" | "variable" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| match cmd {
                        "breakpoint" => {
                            let (field, value) = arg.split_once(' ').unwrap_or((arg, ""));
                            ws.breakpoint_editor_field(field, value, window, cx)
                        }
                        "console" => ws.console_command(arg, window, cx),
                        _ => {
                            let (name, command) = arg.rsplit_once(' ').ok_or("usage: variable <name> copy|describe")?;
                            ws.variable_command(name, command, cx)
                        }
                    })
                })
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "package" => {
            let (field, value) = arg.split_once(' ').unwrap_or((arg, ""));
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.add_package_field(field, value, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "package-menu" => {
            let (command, identity) = arg.split_once(' ').ok_or("usage: package-menu update|remove <identity>")?;
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| match command {
                        "update" => {
                            ws.update_package(identity, window, cx);
                            Ok(())
                        }
                        "remove" => ws.remove_package(identity, window, cx),
                        other => Err(format!("unknown package command {other}")),
                    })
                })
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "reveal" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let path = PathBuf::from(arg);
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.reveal_in_navigator(&path, window, cx)))
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "run-test" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.run_test_named(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "destination" => {
            // A run destination by name or id, as choosing it in the menu does.
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let build = workspace.read(cx).build.clone();
            let destination = build
                .read(cx)
                .destinations
                .iter()
                .find(|d| d.id == arg || d.name == arg)
                .cloned()
                .ok_or(format!("no destination {arg}"))?;
            build.update(cx, |build, cx| build.set_destination(destination, cx));
            Ok(String::new())
        }
        "membership" => {
            let (target, state) = arg.rsplit_once(' ').ok_or("usage: membership <target> on|off")?;
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| {
                        let (path, project, info) = ws.inspected_info().ok_or("the inspector shows no project file")?;
                        let crate::xcodeproj::files::Tracking::Referenced { targets, .. } = info.tracking else {
                            return Err("the inspected file has no editable membership".to_string());
                        };
                        let target = targets.iter().find(|t| t.name == target).ok_or(format!("no target {target}"))?;
                        ws.set_target_membership(project, path, target.id.clone(), state == "on", window, cx);
                        Ok(String::new())
                    })
                })
                .map_err(|e| e.to_string())?
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
            Ok(workspace.update(cx, |ws, cx| ws.debug_state(cx)).to_string())
        }
        other => Err(format!("unknown command {other}")),
    }
}

/// Asks the active editor's hover provider about the cursor's position.
fn start_hover(
    workspaces: &dyn Fn(&mut App) -> Vec<Entity<Workspace>>,
    cx: &mut App,
) -> Result<Task<anyhow::Result<Option<lsp_types::Hover>>>, String> {
    let window = front_window(cx).ok_or("no window")?;
    let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
    window
        .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.hover_at_cursor(window, cx)))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no editor with a hover provider".to_string())
}

fn hover_text(hover: Option<&lsp_types::Hover>) -> String {
    let Some(hover) = hover else { return "{}".into() };
    let text = match &hover.contents {
        lsp_types::HoverContents::Markup(markup) => markup.value.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    serde_json::json!({ "hover": text }).to_string()
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

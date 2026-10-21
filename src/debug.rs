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
//!   double-click <x> <y> / right-click <x> <y> / cmd-click <x> <y>
//!   wheel <x> <y> <lines>      turn the scroll wheel over a point (up is positive)
//!   drag <x1> <y1> <x2> <y2>   press at one point, move to the other and release (drag and drop)
//!   file-drop <x> <y> <path>[;<path>…]  files dragged in from Finder and dropped at a point
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
//!   pane <command>             editor panes: open-next <path>[:line] (⌥-click), focus <n>, tab / close-tab <n> <i>
//!                              (see `Workspace::pane_command`)
//!   package <field> [<value>]  Add Package Dependencies: `open [query]`, `local <path>` (after Add
//!                              Local…'s panel), or a field of the open sheet (see `AddPackageDialog::set_field`)
//!   package-menu update|remove <identity>  a package's Update Package / Remove Package
//!   reveal <path>              reveal a file in the project navigator, package sources included
//!   variable <name> copy|describe  the Variables view's Copy Value / Print Description
//!   hover                      log the editor's hover (Quick Help or a data tip) at the cursor
//!   type-latency <text>        type `text`, logging keystroke-to-frame times as JSON: to the next frame's
//!                              start, `drawn_*` to that frame on screen, and `work_*`, the keystroke's
//!                              handling plus the frame's draw (what zcode did, whatever the display's cadence)
//!   bench <command>            the scale benchmarks (scripts/bench.py): status (files, timings, memory, the
//!                              language server and the index build, the search, in a few lines), frame
//!                              (a redraw's time to the next frame, to it drawn, and its work), action <Name>
//!                              / keys <keystrokes> (dispatched and timed the same way), index (the index
//!                              build for the chosen scheme and destination, then the status)
//!   editor <probe>             the editor's extension points (see `Workspace::editor_probe`)
//!   hierarchy <command>        the call or type hierarchy in the Find navigator: kind callers|callees|
//!                              supertypes|subtypes, expand / collapse / open / reroot <row>, close
//!                              (see `Workspace::hierarchy_command`)
//!   signature-help             ask for signature help at the active editor's cursor
//!   inlay-hints on|off         Editor ▸ Inlay Hints (`inlay_hints.enabled` in settings.json)
//!   vim on|off                 Editor ▸ Vim Mode (`vim_mode`, for every window); `dump` has `vim`
//!   settings <command>         settings, keymap and theme files: paths, set <a.b.c> <json>, reload,
//!                              theme <name>, import-theme <path>, import-keys <path>, xcode-themes,
//!                              xcode-key-sets, open settings|keymap|project|default-settings|default-keymap
//!                              (see `Workspace::settings_command`); `dump` has `settings`
//!   git <command>              source control: hunks, the hunk popover, the commit, Push… and Pull…
//!                              sheets, branches, comparisons (see `Workspace::git_command`)
//!   find <command>             Find in Workspace: query <text> (searched), regex|case|word on|off
//!                              (see `Workspace::find_command`); `dump` has `find`
//!   multibuffer <command>      multibuffers: find (the Find navigator's results in a tab), issues,
//!                              replace <text> (Replace All), open (⌥↩), expand (⇧↩), all, save
//!                              (see `Workspace::multibuffer_command`)
//!   terminal <command>         terminals: panel / tab [<dir>] [-- <script>], at <path> (Open Terminal at
//!                              Location), type / focus / close / select-all / copy / clear <n>, scroll <n>
//!                              <lines>, link <n> <row> <column> (⌘-click), clipboard
//!                              (see `Workspace::terminal_command`)
//!   agent <command>            the agent panel (⌘0): show, hide, type <text>, send [<text>], mention <path>[:<a>-<b>],
//!                              mentions (the picker), allow|reject [always], cancel, new, agent <id>, follow on|off,
//!                              mode <id>, auth <method>, review / reject-file / keep [<path>], expand <entry>
//!                              (see `Workspace::agent_command`); `dump` has `agent`
//!   predict <command>          predictive code completion: status, request (⌃⌥Space), accept, dismiss,
//!                              edits (see `Workspace::prediction_command`); `dump` has `predictions`
//!   organizer <command>        Window ▸ Organizer: open, refresh, select <archive>, group <app>, distribute
//!                              (the sheet), delete, finder (see `Workspace::organizer_command`); `dump` has
//!                              `archives`
//!   distribute <command>       the Distribute App sheet: method <id>, upload / symbols / versions / strip /
//!                              notarize on|off, profile <name>, folder <path>, confirm, cancel
//!                              (see `DistributeSheet::command`)
//!   assets <command>           the asset catalog shown: select <asset>, slot <slot>, expand <folder>,
//!                              new image|color|folder, remove, rename <name>, drop <slot> <file> (a file
//!                              dropped into a well), import <file>[;<file>…] (dropped on the outline),
//!                              clear <slot>, color [<slot>] <component> <value>, space <slot> <space>,
//!                              method <slot> float|8-bit|hex, undo, redo, reload
//!                              (see `AssetCatalogView::command`); `dump` has `asset_catalog`
//!   app <command>              the app as packaged: bundle (where it runs from, its version), cli
//!                              install|status (Install CLI; `ZCODE_CLI_DIR`), login-item status|on|off|
//!                              toggle|menu (Open at Login; `ZCODE_LOGIN_ITEM_FILE`), updates check|status|
//!                              restart (Check for Updates…; `ZCODE_UPDATE_FEED`), open-url <file-url>
//!                              (what Finder hands over)
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
                    "bench" if !matches!(arg.as_str(), "status" | "index") => bench_frame(&arg, cx).await,
                    "file-drop" => file_drop(&arg, cx).await,
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
    let mut drawn = vec![];
    let mut work = vec![];
    for ch in text.chars() {
        let key = if ch == ' ' { "space".to_string() } else { ch.to_string() };
        let mut keystroke = Keystroke::parse(&key).map_err(|e| e.to_string())?;
        keystroke.key_char = Some(ch.to_string());
        let frame = timed_frame(cx, move |window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .await?;
        samples.push(frame.to_frame_ms);
        drawn.push(frame.drawn_ms);
        work.push(frame.work_ms());
    }
    if samples.is_empty() {
        return Err("no frames".into());
    }
    samples.sort_by(f64::total_cmp);
    drawn.sort_by(f64::total_cmp);
    work.sort_by(f64::total_cmp);
    let at = |s: &[f64], q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
    Ok(serde_json::json!({
        "keys": samples.len(),
        "median_ms": at(&samples, 0.5), "p95_ms": at(&samples, 0.95), "max_ms": at(&samples, 1.0),
        "drawn_median_ms": at(&drawn, 0.5), "drawn_p95_ms": at(&drawn, 0.95), "drawn_max_ms": at(&drawn, 1.0),
        "work_median_ms": at(&work, 0.5), "work_p95_ms": at(&work, 0.95), "work_max_ms": at(&work, 1.0),
    })
    .to_string())
}

/// What one input cost: its handling, the time to the start of the next
/// frame, and to that frame drawn and on screen.
struct FrameTiming {
    handled_ms: f64,
    to_frame_ms: f64,
    drawn_ms: f64,
}

impl FrameTiming {
    /// The input's handling plus the frame's draw and present: what zcode
    /// did for it, whatever the display's cadence (a loaded machine's ticks
    /// come at 30 Hz, which `to_frame_ms` and `drawn_ms` include).
    fn work_ms(&self) -> f64 {
        self.handled_ms + (self.drawn_ms - self.to_frame_ms)
    }
}

/// Runs `input` in the front window and times the frame it causes. Frame
/// callbacks run at the start of a display tick, before the draw, so a
/// task queued from there runs once the draw and present are done.
async fn timed_frame(
    cx: &mut AsyncApp,
    input: impl FnOnce(&mut Window, &mut App) + 'static,
) -> Result<FrameTiming, String> {
    let (frame_tx, frame_rx) = futures::channel::oneshot::channel();
    let (drawn_tx, drawn_rx) = futures::channel::oneshot::channel();
    let start = std::time::Instant::now();
    let handled = cx.update(|cx| {
        let window = front_window(cx).ok_or("no window")?;
        window
            .update(cx, |_, window, cx| {
                window.on_next_frame(move |_, cx| {
                    frame_tx.send(std::time::Instant::now()).ok();
                    cx.spawn(async move |_| {
                        drawn_tx.send(std::time::Instant::now()).ok();
                    })
                    .detach();
                });
                input(window, cx);
                std::time::Instant::now()
            })
            .map_err(|e| e.to_string())
    })?;
    let ms = |end: std::time::Instant| (end - start).as_secs_f64() * 1000.;
    let handled_ms = ms(handled);
    let to_frame_ms = frame_rx.await.map(ms).map_err(|_| "no frame")?;
    let drawn_ms = drawn_rx.await.map(ms).map_err(|_| "frame not drawn")?;
    Ok(FrameTiming { handled_ms, to_frame_ms, drawn_ms })
}

/// `bench frame|action <Name>|keys <keystrokes>`: the input and the frame
/// it causes, timed; `bench status` and `bench index` are run in `run_command`.
async fn bench_frame(arg: &str, cx: &mut AsyncApp) -> Result<String, String> {
    let (what, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    let rest = rest.trim().to_string();
    let frame = match what {
        "frame" => timed_frame(cx, |window, _| window.refresh()).await?,
        "action" => {
            let action = cx.update(|cx| cx.build_action(&rest, None)).map_err(|e| e.to_string())?;
            timed_frame(cx, move |window, cx| window.dispatch_action(action, cx)).await?
        }
        "keys" => {
            let keystrokes = rest
                .split_whitespace()
                .map(Keystroke::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            timed_frame(cx, move |window, cx| {
                for keystroke in keystrokes {
                    window.dispatch_keystroke(keystroke, cx);
                }
            })
            .await?
        }
        _ => return Err("usage: bench status|index|frame|action <Name>|keys <keystrokes>".into()),
    };
    Ok(serde_json::json!({ "to_frame_ms": frame.to_frame_ms, "drawn_ms": frame.drawn_ms, "work_ms": frame.work_ms() }).to_string())
}

/// Files dragged in from Finder and dropped at a point, as the platform
/// reports them: entering and moving, then (a frame later, as a real drag
/// spans many) moving and dropping, each its own update.
async fn file_drop(arg: &str, cx: &mut AsyncApp) -> Result<String, String> {
    let mut parts = arg.splitn(3, ' ');
    let (Some(x), Some(y), Some(paths)) = (parts.next().and_then(|v| v.parse().ok()), parts.next().and_then(|v| v.parse().ok()), parts.next())
    else {
        return Err("usage: file-drop <x> <y> <path>[;<path>…]".into());
    };
    let position = point(px(x), px(y));
    let paths = ExternalPaths(paths.split(';').map(|p| PathBuf::from(p.trim())).collect());
    let window = cx.update(|cx| front_window(cx)).ok_or("no window")?;
    let events = [
        vec![FileDropEvent::Entered { position, paths }, FileDropEvent::Pending { position }],
        vec![FileDropEvent::Pending { position }],
        vec![FileDropEvent::Pending { position }, FileDropEvent::Submit { position }],
    ];
    for (ix, batch) in events.into_iter().enumerate() {
        if ix > 0 {
            cx.background_executor().timer(Duration::from_millis(20)).await;
        }
        window
            .update(cx, |_, window, cx| {
                for event in batch {
                    window.dispatch_event(PlatformInput::FileDrop(event), cx);
                }
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(String::new())
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
        "mouse" | "click" | "double-click" | "right-click" | "cmd-click" => {
            let mut parts = arg.split_whitespace().filter_map(|p| p.parse::<f32>().ok());
            let (Some(x), Some(y)) = (parts.next(), parts.next()) else {
                return Err("usage: mouse <x> <y> (window points)".into());
            };
            let position = point(px(x), px(y));
            let modifiers = if cmd == "cmd-click" { Modifiers::command() } else { Modifiers::default() };
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers,
                        }),
                        cx,
                    );
                    let (button, clicks) = match cmd {
                        "click" | "cmd-click" => (MouseButton::Left, 1),
                        "double-click" => (MouseButton::Left, 2),
                        "right-click" => (MouseButton::Right, 1),
                        _ => (MouseButton::Left, 0),
                    };
                    for click_count in 1..=clicks {
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button,
                                position,
                                modifiers,
                                click_count,
                                first_mouse: false,
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button,
                                position,
                                modifiers,
                                click_count,
                            }),
                            cx,
                        );
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "wheel" => {
            let numbers: Vec<f32> = arg.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            let [x, y, lines] = numbers[..] else {
                return Err("usage: wheel <x> <y> <lines> (window points; up is positive)".into());
            };
            let position = point(px(x), px(y));
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Lines(point(0., lines)),
                            modifiers: Modifiers::default(),
                            touch_phase: TouchPhase::Moved,
                        }),
                        cx,
                    );
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
        "pane" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.pane_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "hierarchy" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.hierarchy_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "inlay-hints" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let root = workspace.read(cx).project.root.clone();
            crate::settings::set(Some(&root), &["inlay_hints", "enabled"], serde_json::json!(arg == "on"), cx)?;
            Ok(String::new())
        }
        // Editor ▸ Vim Mode, set rather than toggled.
        "vim" => {
            let on = match arg {
                "on" => true,
                "off" => false,
                _ => return Err("usage: vim on|off".into()),
            };
            crate::settings::set(None, &["vim_mode"], serde_json::json!(on), cx)?;
            Ok(String::new())
        }
        "settings" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.settings_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
        }
        "signature-help" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            let editor = workspace.read(cx).active_editor().ok_or("no editor")?;
            editor.update(cx, |state, cx| state.show_signature_help(cx));
            Ok(String::new())
        }
        "git" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.git_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "terminal" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.terminal_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
        }
        "assets" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.assets_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
        }
        "organizer" | "distribute" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| match cmd {
                        "organizer" => ws.organizer_command(arg, window, cx),
                        _ => ws.distribute_command(arg, window, cx),
                    })
                })
                .map_err(|e| e.to_string())?
        }
        "predict" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.prediction_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
        }
        "agent" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.agent_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
        }
        "find" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.find_command(arg, window, cx)))
                .map_err(|e| e.to_string())?
                .map(|_| String::new())
        }
        "multibuffer" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.multibuffer_command(arg, window, cx)))
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
        "app" => {
            let (what, rest) = arg.split_once(' ').unwrap_or((arg, ""));
            match what {
                "bundle" => Ok(crate::bundle::info().to_string()),
                "cli" => crate::cli::command(rest.trim(), cx),
                "login-item" => crate::login_item::command(rest.trim(), cx),
                "updates" => crate::updates::command(rest.trim(), cx),
                "open-url" => {
                    let paths = rest.split(';').filter_map(|url| crate::bundle::path_of_file_url(url.trim())).collect();
                    crate::app::open_paths(paths, cx);
                    let open: Vec<String> = workspaces(cx).iter().map(|w| w.read(cx).project.root.display().to_string()).collect();
                    Ok(serde_json::json!({ "open": open }).to_string())
                }
                _ => Err("usage: app bundle|cli …|login-item …|updates …|open-url <url>".into()),
            }
        }
        "dump" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            window
                .update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.debug_state(window, cx)).to_string())
                .map_err(|e| e.to_string())
        }
        // `bench status`: the few fields the benchmarks poll, cheap on a
        // project whose `dump` runs to hundreds of kilobytes.
        "bsp" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            workspace.update(cx, |ws, cx| ws.build_server_command(arg, cx))
        }
        "bench" => {
            let workspace = workspaces(cx).into_iter().next().ok_or("no workspace")?;
            if arg == "index" {
                workspace.update(cx, |ws, cx| ws.start_index_build(cx));
            }
            Ok(workspace.update(cx, |ws, cx| ws.bench_status(cx)).to_string())
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
    crate::screenshot::write_window_png(window.windowNumber() as u32, path)
}

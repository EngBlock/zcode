//! Application startup, windows and global commands.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::actions::{self, About, CheckForUpdates, InstallCli, OpenProject, Quit, ToggleOpenAtLogin};
use crate::perf::{self, Perf};
use crate::project;
use crate::welcome::Welcome;
use crate::workspace::Workspace;

#[derive(Default)]
struct Args {
    path: Option<PathBuf>,
    startup_bench: bool,
    /// Start the resident instance without a window (e.g. as a login item),
    /// so every later `zcode <path>` is a warm open.
    background: bool,
    perf_log: Option<PathBuf>,
}

impl Args {
    /// The arguments a launch through LaunchServices passes on, the path
    /// made absolute since the app won't share this shell's folder.
    fn for_launch(&self) -> Vec<String> {
        let mut args = vec![];
        if let Some(path) = &self.path {
            let absolute = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            args.push(absolute.display().to_string());
        }
        if self.background {
            args.push("--background".into());
        }
        if let Some(log) = &self.perf_log {
            args.push("--perf-log".into());
            args.push(log.display().to_string());
        }
        args
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--startup-bench" => args.startup_bench = true,
            "--background" => args.background = true,
            "--perf-log" => args.perf_log = argv.next().map(PathBuf::from),
            "--version" | "-v" => {
                println!("zcode {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "zcode {}\n\nUSAGE:\n    zcode [PATH] [--startup-bench] [--background] [--perf-log FILE]\n    zcode mcp\n\nPATH may be a folder, .xcodeproj, .xcworkspace, Package.swift or a file.\nWhen zcode is already running, PATH opens in that instance (~70 ms).\n\n--startup-bench    print startup timings and memory as JSON, then quit\n--background       start the resident instance without a window (login item)\n--perf-log FILE    append a JSON memory sample every 2 s to FILE\n\nzcode mcp          the MCP server for coding agents, on stdio (zcode must be running)",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            other if !other.starts_with('-') => args.path = Some(PathBuf::from(other)),
            _ => {}
        }
    }
    args
}

/// Tracks open workspaces so quitting can save their sessions.
#[derive(Default)]
struct Workspaces(Vec<WeakEntity<Workspace>>);
impl Global for Workspaces {}

/// The open project windows' workspaces.
pub fn workspaces(cx: &mut App) -> Vec<Entity<Workspace>> {
    cx.default_global::<Workspaces>().0.iter().filter_map(|w| w.upgrade()).collect()
}

fn window_options(title: &str, window_size: Size<Pixels>, cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, window_size, cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_string().into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(13.))),
        }),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(720.), px(420.))),
        // Created hidden and shown by `present` without AppKit's open
        // animation.
        show: false,
        focus: false,
        ..Default::default()
    }
}

/// Shows a window opened with `window_options`. Setting up AppKit's
/// window-open animation blocks the main thread for ~100 ms inside
/// `makeKeyAndOrderFront` before the first frame, so zcode's windows appear
/// without it.
fn present(window: AnyWindowHandle, cx: &mut App) {
    disable_window_animations();
    window
        .update(cx, |_, window, _| window.activate_window())
        .ok();
}

/// Starts `perf::watch_first_pixels` on the newest window.
fn watch_first_pixels(main_started: Instant) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    if let Some(window) = NSApplication::sharedApplication(mtm).windows().lastObject() {
        perf::watch_first_pixels(window.windowNumber() as u32, main_started);
    }
}

fn disable_window_animations() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindowAnimationBehavior};
    let Some(mtm) = MainThreadMarker::new() else { return };
    for window in NSApplication::sharedApplication(mtm).windows() {
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    }
}

/// The theme the settings pick for the window's appearance.
fn sync_theme(window: &mut Window, cx: &mut App) {
    crate::settings::themes::sync(window, cx);
}

/// Opens a project window for `path` (or focuses it if already open).
pub fn open_project(path: PathBuf, cx: &mut App) {
    let t0 = Instant::now();
    let trace = std::env::var_os("ZCODE_TRACE_STARTUP").is_some();
    let mark = move |label: &str| {
        if trace {
            eprintln!("[open_project] {label}: {:?}", t0.elapsed());
        }
    };
    let path = path.canonicalize().unwrap_or(path);
    let info = project::detect(&path);
    mark("detect");

    // Focus an existing window for the same project.
    let open: Vec<_> = cx
        .default_global::<Workspaces>()
        .0
        .iter()
        .filter_map(|w| w.upgrade())
        .collect();
    if open.iter().any(|w| w.read(cx).project.root == info.root) {
        cx.activate(true);
        return;
    }

    let title = info.name.clone();
    let opened = path.clone();
    let result = gpui_kit::open_window(
        window_options(&title, size(px(1440.), px(900.)), cx),
        cx,
        |window, cx| {
            mark("platform window created");
            sync_theme(window, cx);
            mark("theme");
            let workspace = cx.new(|cx| Workspace::new(info, opened, window, cx));
            mark("workspace");
            window.set_window_title(&workspace.read(cx).title());
            window
                .observe_window_appearance(sync_theme)
                .detach();
            workspace
        },
    );
    mark("open_window returned");
    match result {
        Ok((window, workspace)) => {
            present(window, cx);
            cx.default_global::<Workspaces>().0.push(workspace.downgrade());
            let weak = workspace.downgrade();
            window
                .update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, move |_, cx| {
                        if let Some(workspace) = weak.upgrade() {
                            workspace.update(cx, |ws, cx| {
                                ws.save_session(cx);
                                ws.stop_terminals(cx);
                                ws.end_runs(cx);
                                ws.lsp.update(cx, |lsp, cx| lsp.shutdown(cx)).detach();
                            });
                        }
                        true
                    });
                })
                .ok();
        }
        Err(err) => log::error!("failed to open window: {err:#}"),
    }
}

/// Shows a notification in the window in front (a project's or the
/// welcome window): what app-wide commands say when they're done.
pub fn notify(notification: gpui_kit::component::notification::Notification, cx: &mut App) {
    use gpui_kit::component::WindowExt as _;
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        return;
    };
    window.update(cx, |_, window, cx| window.push_notification(notification, cx)).ok();
}

/// Opens what Finder (or `app open-url` on the control channel) handed
/// over: each path its project, and the app comes forward.
pub fn open_paths(paths: Vec<PathBuf>, cx: &mut App) {
    for path in paths {
        match path.exists() {
            true => open_project(path, cx),
            false => log::warn!("{} does not exist", path.display()),
        }
    }
    if cx.windows().is_empty() {
        open_welcome(cx);
    }
    cx.activate(true);
}

fn prewarm_hidden_window(cx: &mut App) {
    // Shown (so the compositor and renderer are set up) but off-screen and
    // without focus; it closes after its first frames.
    let mut options = window_options("zcode", size(px(1440.), px(900.)), cx);
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::new(
        point(px(-30000.), px(-30000.)),
        size(px(400.), px(300.)),
    )));
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        sync_theme(window, cx);
        cx.new(Welcome::new)
    });
    if let Ok((window, _)) = opened {
        present(window, cx);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            cx.update(|cx| {
                window.update(cx, |_, window, _| window.remove_window()).ok();
            });
        })
        .detach();
    }
}

pub fn open_welcome(cx: &mut App) {
    let result = gpui_kit::open_window(
        window_options("Welcome to zcode", size(px(820.), px(480.)), cx),
        cx,
        |window, cx| {
            sync_theme(window, cx);
            window
                .observe_window_appearance(sync_theme)
                .detach();
            cx.new(Welcome::new)
        },
    );
    match result {
        Ok((window, _)) => present(window, cx),
        Err(err) => log::error!("failed to open welcome window: {err:#}"),
    }
}

fn prompt_open(cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: true,
        multiple: false,
        prompt: Some("Open".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await
            && let Some(path) = paths.into_iter().next()
        {
            cx.update(|cx| open_project(path, cx));
        }
    })
    .detach();
}

fn quit(cx: &mut App) {
    let workspaces: Vec<_> = cx
        .default_global::<Workspaces>()
        .0
        .iter()
        .filter_map(|w| w.upgrade())
        .collect();
    let mut shutdowns = vec![];
    for workspace in &workspaces {
        workspace.update(cx, |ws, cx| {
            ws.save_session(cx);
            ws.stop_terminals(cx);
            // Apps it ran don't outlive zcode (they'd be launchd's).
            ws.end_runs(cx);
            shutdowns.push(ws.lsp.update(cx, |lsp, cx| lsp.shutdown(cx)));
        });
    }
    cx.spawn(async move |cx| {
        let all = futures::future::join_all(shutdowns);
        let timeout = cx.background_executor().timer(Duration::from_millis(600));
        futures::future::select(Box::pin(all), Box::pin(timeout)).await;
        crate::instance::cleanup();
        cx.update(|cx| {
            for workspace in &workspaces {
                workspace.read(cx).kill_runs(cx);
            }
            cx.quit()
        });
    })
    .detach();
}

pub fn run() {
    // `zcode mcp`: the MCP server on stdio, relayed to the running zcode.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        std::process::exit(crate::mcp::bridge::run());
    }
    // Lets a sampling profiler attach before startup work begins
    // (`ZCODE_PAUSE_AT_START=1500 zcode & sample <pid> 3`).
    if let Some(ms) = std::env::var("ZCODE_PAUSE_AT_START").ok().and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let main_started = Instant::now();
    let args = parse_args();
    init_logging();

    // Hand off to an already-running zcode when there is one (scripted and
    // benchmark runs always start their own process).
    let single_instance = !args.startup_bench
        && std::env::var_os("ZCODE_DEBUG_DIR").is_none()
        && std::env::var_os("ZCODE_NEW_INSTANCE").is_none();
    if single_instance && crate::instance::forward(args.path.as_deref()) {
        return;
    }
    // `zcode …` typed in a shell (the link Install CLI makes) with no zcode
    // running starts the app the way Finder would and gives the prompt back,
    // when there's an app to start; a bare binary runs here.
    if single_instance
        && crate::bundle::launched_through_cli()
        && let Some(bundle) = crate::bundle::app_bundle()
    {
        match crate::bundle::launch(bundle, &args.for_launch()) {
            Ok(()) => return,
            Err(err) => eprintln!("zcode: couldn’t open {}: {err}", bundle.display()),
        }
    }

    // Settings, key bindings and the theme they name are read on a thread
    // of their own while AppKit starts.
    crate::settings::preload();
    if std::env::var_os("ZCODE_TRACE_STARTUP").is_some() {
        eprintln!("[startup] main: {:?}", main_started.elapsed());
    }
    let app = gpui_kit::application();
    if std::env::var_os("ZCODE_TRACE_STARTUP").is_some() {
        eprintln!("[startup] application(): {:?}", main_started.elapsed());
    }
    // Clicking the Dock icon with no windows open shows the welcome window.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_welcome(cx);
        }
    });
    // Finder (a double-clicked .xcodeproj, Open With…, a folder dropped on
    // the Dock icon) hands paths over as URLs, before the app has finished
    // launching or to a running one.
    let (urls_tx, urls_rx) = smol::channel::unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        urls_tx.send_blocking(urls).ok();
    });
    app
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            let trace = std::env::var_os("ZCODE_TRACE_STARTUP").is_some();
            let mark = |label: &str| {
                if trace {
                    eprintln!("[startup] {label}: {:?}", main_started.elapsed());
                }
            };
            mark("app running");
            gpui_kit::init(cx);
            zcode_editor::init(cx);
            mark("gpui_kit::init");
            crate::languages::register();
            crate::languages::configure(cx);
            cx.set_global(Perf::new(main_started));
            crate::settings::init(cx);
            mark("settings");
            actions::bind_keys(cx);
            crate::settings::keymap::bind_user(cx);
            actions::set_menus(cx);
            mark("keymap + menus");
            crate::workspace::register_global_settings_actions(cx);

            cx.on_action(|_: &Quit, cx| quit(cx));
            cx.on_action(|_: &OpenProject, cx| prompt_open(cx));
            cx.on_action(|_: &About, cx| {
                cx.open_url("https://github.com/zed-industries/zed/tree/main/crates/gpui");
            });
            cx.on_action(|_: &CheckForUpdates, cx| crate::updates::check(true, cx));
            cx.on_action(|_: &InstallCli, cx| crate::cli::install_action(cx));
            cx.on_action(|_: &ToggleOpenAtLogin, cx| crate::login_item::toggle(cx));
            cx.on_window_closed(move |cx, _| {
                // Like other Mac document apps, a single-instance zcode keeps
                // running with no windows, so the next open is instant.
                if cx.windows().is_empty() && !single_instance {
                    quit(cx);
                }
            })
            .detach();
            // A scripted zcode with a configuration folder of its own
            // listens there, for `zcode mcp` (never on the user's socket).
            if single_instance || (crate::paths::has_config_override() && !args.startup_bench) {
                crate::instance::listen(cx);
            }

            if let Some(path) = args.perf_log.clone() {
                cx.global_mut::<Perf>().log_path = Some(path);
            }
            let mut from_finder = vec![];
            while let Ok(urls) = urls_rx.try_recv() {
                from_finder.extend(urls.iter().filter_map(|url| crate::bundle::path_of_file_url(url)));
            }
            match args.path.clone() {
                _ if args.background && single_instance => {}
                Some(path) if path.exists() => open_project(path, cx),
                Some(path) => {
                    eprintln!("zcode: {} does not exist", path.display());
                    open_welcome(cx);
                }
                None if !from_finder.is_empty() => open_paths(from_finder, cx),
                None => open_welcome(cx),
            }
            cx.spawn(async move |cx| {
                while let Ok(urls) = urls_rx.recv().await {
                    let paths = urls.iter().filter_map(|url| crate::bundle::path_of_file_url(url)).collect();
                    cx.update(|cx| open_paths(paths, cx));
                }
            })
            .detach();
            mark("window opened");
            cx.global_mut::<Perf>().mark_window_opened();

            // Measure the first presented frame.
            if let Some(window) = cx.windows().first().copied() {
                let bench = args.startup_bench;
                if bench {
                    watch_first_pixels(cx.global::<Perf>().main_started());
                }
                window
                    .update(cx, move |_, window, _| {
                        window.on_next_frame(move |window, cx| {
                            // The frame after the first is presented once the
                            // first one has been committed.
                            window.on_next_frame(move |_, cx| {
                                cx.global_mut::<Perf>().mark_first_frame();
                                cx.global_mut::<Perf>().record(perf::sample_self());
                                if bench {
                                    cx.spawn(async move |cx| {
                                        cx.background_executor().timer(Duration::from_millis(500)).await;
                                        for _ in 0..50 {
                                            if perf::MAIN_TO_FIRST_PIXELS.get().is_some() {
                                                break;
                                            }
                                            cx.background_executor().timer(Duration::from_millis(50)).await;
                                        }
                                        cx.update(|cx| {
                                            cx.global_mut::<Perf>().record(perf::sample_self());
                                            println!("{}", cx.global::<Perf>().report_json());
                                            cx.quit();
                                        });
                                    })
                                    .detach();
                                }
                            });
                            let _ = (window, cx);
                        });
                    })
                    .ok();
            }
            perf::start_sampler(cx);
            crate::settings::watch(cx);
            // After the first frame: the login item's checkmark (launchd is
            // asked off the main thread) and the daily update check (a
            // minute from now).
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                cx.update(|cx| {
                    crate::login_item::refresh(cx);
                    crate::updates::start_automatic_checks(cx);
                });
            })
            .detach();
            if args.background && single_instance {
                // Resident and listening; windows open on request. Lay out
                // and draw one hidden window first so the first real open
                // doesn't pay one-time renderer, font and theme setup.
                prewarm_hidden_window(cx);
                return;
            }
            crate::debug::start(cx, |cx| {
                cx.default_global::<Workspaces>()
                    .0
                    .iter()
                    .filter_map(|w| w.upgrade())
                    .collect()
            });
            cx.activate(true);
        });
}

fn init_logging() {
    struct Logger;
    impl log::Log for Logger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Info
                && (metadata.target().starts_with("zcode") || metadata.level() <= log::Level::Warn)
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Logger = Logger;
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
}

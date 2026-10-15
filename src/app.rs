//! Application startup, windows and global commands.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use crate::actions::{self, About, OpenProject, Quit};
use crate::perf::{self, Perf};
use crate::project;
use crate::welcome::Welcome;
use crate::workspace::Workspace;

#[derive(Default)]
struct Args {
    path: Option<PathBuf>,
    startup_bench: bool,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--startup-bench" => args.startup_bench = true,
            "--version" | "-v" => {
                println!("zcode {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "zcode {}\n\nUSAGE:\n    zcode [PATH] [--startup-bench]\n\nPATH may be a folder, .xcodeproj, .xcworkspace, Package.swift or a file.\n--startup-bench prints startup timings and memory as JSON, then quits.",
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
        ..Default::default()
    }
}

fn sync_theme(window: &mut Window, cx: &mut App) {
    let mode = match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    };
    Theme::change(mode, Some(window), cx);
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
            cx.default_global::<Workspaces>().0.push(workspace.downgrade());
            let weak = workspace.downgrade();
            window
                .update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, move |_, cx| {
                        if let Some(workspace) = weak.upgrade() {
                            workspace.update(cx, |ws, cx| {
                                ws.save_session(cx);
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

fn open_welcome(cx: &mut App) {
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
    if let Err(err) = result {
        log::error!("failed to open welcome window: {err:#}");
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
    for workspace in workspaces {
        workspace.update(cx, |ws, cx| {
            ws.save_session(cx);
            shutdowns.push(ws.lsp.update(cx, |lsp, cx| lsp.shutdown(cx)));
        });
    }
    cx.spawn(async move |cx| {
        let all = futures::future::join_all(shutdowns);
        let timeout = cx.background_executor().timer(Duration::from_millis(600));
        futures::future::select(Box::pin(all), Box::pin(timeout)).await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

pub fn run() {
    let main_started = Instant::now();
    let args = parse_args();
    init_logging();

    if std::env::var_os("ZCODE_TRACE_STARTUP").is_some() {
        eprintln!("[startup] main: {:?}", main_started.elapsed());
    }
    let app = gpui_kit::application();
    if std::env::var_os("ZCODE_TRACE_STARTUP").is_some() {
        eprintln!("[startup] application(): {:?}", main_started.elapsed());
    }
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
            mark("gpui_kit::init");
            crate::languages::register();
            cx.set_global(Perf::new(main_started));
            actions::bind_keys(cx);
            actions::set_menus(cx);
            mark("keymap + menus");

            cx.on_action(|_: &Quit, cx| quit(cx));
            cx.on_action(|_: &OpenProject, cx| prompt_open(cx));
            cx.on_action(|_: &About, cx| {
                cx.open_url("https://github.com/zed-industries/zed/tree/main/crates/gpui");
            });
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    quit(cx);
                }
            })
            .detach();

            match args.path.clone() {
                Some(path) if path.exists() => open_project(path, cx),
                Some(path) => {
                    eprintln!("zcode: {} does not exist", path.display());
                    open_welcome(cx);
                }
                None => open_welcome(cx),
            }
            mark("window opened");
            cx.global_mut::<Perf>().mark_window_opened();

            // Measure the first presented frame.
            if let Some(window) = cx.windows().first().copied() {
                let bench = args.startup_bench;
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

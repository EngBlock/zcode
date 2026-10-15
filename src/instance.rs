//! Single-instance launching. When zcode is already running, `zcode <path>`
//! hands the path to it over a Unix socket and exits immediately; the warm
//! instance opens the window without paying process, platform and framework
//! startup again (Zed's CLI works the same way).

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::App;

pub fn socket_path() -> PathBuf {
    crate::paths::cache_dir().join("zcode.sock")
}

/// Asks a running instance to open `path` (or just come forward). Returns
/// `true` when the running instance acknowledged the request.
pub fn forward(path: Option<&Path>) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let request = match path {
        Some(path) => {
            let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            format!("open {}\n", absolute.display())
        }
        None => "activate\n".to_string(),
    };
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

enum Request {
    Open(PathBuf),
    Activate,
}

/// Listens for requests from later launches.
pub fn listen(cx: &mut App) {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    // A socket file left by a crashed instance refuses connections; replace it.
    std::fs::remove_file(&path).ok();
    let Ok(listener) = UnixListener::bind(&path) else {
        log::warn!("could not listen on {}", path.display());
        return;
    };
    let (tx, rx) = smol::channel::unbounded::<(Request, UnixStream)>();
    std::thread::Builder::new()
        .name("zcode-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let line = line.trim();
                let request = match line.strip_prefix("open ") {
                    Some(path) => Request::Open(PathBuf::from(path)),
                    None => Request::Activate,
                };
                if tx.send_blocking((request, stream)).is_err() {
                    break;
                }
            }
        })
        .ok();
    cx.spawn(async move |cx| {
        while let Ok((request, mut stream)) = rx.recv().await {
            let started = Instant::now();
            cx.update(|cx| {
                match request {
                    Request::Open(path) if path.exists() => crate::app::open_project(path, cx),
                    Request::Open(_) | Request::Activate => {
                        if cx.windows().is_empty() {
                            crate::app::open_welcome(cx);
                        }
                    }
                }
                cx.activate(true);
            });
            stream.write_all(b"ok\n").ok();
            if std::env::var_os("ZCODE_TRACE_STARTUP").is_some() {
                eprintln!("[instance] window opened in {:?}", started.elapsed());
                cx.update(|cx| {
                    if let Some(window) = cx.windows().last().copied() {
                        window
                            .update(cx, move |_, window, _| {
                                window.on_next_frame(move |window, _| {
                                    window.on_next_frame(move |_, _| {
                                        eprintln!("[instance] first frame in {:?}", started.elapsed());
                                    });
                                });
                            })
                            .ok();
                    }
                });
            }
        }
    })
    .detach();
}

/// Removes the socket on exit so the next launch doesn't try to forward.
pub fn cleanup() {
    std::fs::remove_file(socket_path()).ok();
}

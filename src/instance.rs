//! Single-instance launching. When zcode is already running, `zcode <path>`
//! hands the path to it over a Unix socket and exits immediately; the warm
//! instance opens the window without paying process, platform and framework
//! startup again (Zed's CLI works the same way). `zcode mcp` connects to the
//! same socket for the MCP server (`crate::mcp`).
//!
//! Only the user zcode runs as may use the socket: it's created readable
//! and writable by its owner alone, and every connection's peer is checked
//! (`getpeereid`) before anything it sends is read.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::App;

/// The socket: in the cache folder, or in `ZCODE_CONFIG_DIR` when that
/// stands in for the user's folders (tests), so a scripted zcode never
/// takes over the user's.
pub fn socket_path() -> PathBuf {
    match crate::paths::has_config_override() {
        true => crate::paths::config_dir().join("zcode.sock"),
        false => crate::paths::cache_dir().join("zcode.sock"),
    }
}

static LISTENING: OnceLock<PathBuf> = OnceLock::new();

/// The socket this zcode listens on, once it does.
pub fn listening() -> Option<&'static Path> {
    LISTENING.get().map(PathBuf::as_path)
}

/// Whether the other end of `stream` runs as the same user as zcode.
pub fn same_user(stream: &UnixStream) -> bool {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a connected socket's descriptor and two out-parameters.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    ok && uid == unsafe { libc::geteuid() }
}

/// Asks a running instance to open `path` (or just come forward). Returns
/// `true` when the running instance acknowledged the request.
pub fn forward(path: Option<&Path>) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    if !same_user(&stream) {
        return false;
    }
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
    /// `zcode mcp`: what it says about itself, and the connection, which
    /// the MCP server keeps (with what was read past the first line).
    Mcp(String, BufReader<UnixStream>),
}

/// Listens for requests from later launches.
pub fn listen(cx: &mut App) {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
        if !crate::paths::has_config_override() {
            // zcode's own cache folder: nobody else's to look into.
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok();
        }
    }
    // A socket file left by a crashed instance refuses connections; replace it.
    std::fs::remove_file(&path).ok();
    let Ok(listener) = UnixListener::bind(&path) else {
        log::warn!("could not listen on {}", path.display());
        return;
    };
    if std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).is_err() {
        log::warn!("could not make {} private; not listening", path.display());
        std::fs::remove_file(&path).ok();
        return;
    }
    LISTENING.set(path).ok();
    let (tx, rx) = smol::channel::unbounded::<(Request, UnixStream)>();
    std::thread::Builder::new()
        .name("zcode-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if !same_user(&stream) {
                    log::warn!("refused a connection from another user");
                    continue;
                }
                // A client that connects and says nothing doesn't hold up the next.
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let Ok(reader) = stream.try_clone() else { continue };
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let line = line.trim();
                let request = if let Some(path) = line.strip_prefix("open ") {
                    Request::Open(PathBuf::from(path))
                } else if line == "mcp" || line.starts_with("mcp ") {
                    stream.set_read_timeout(None).ok();
                    Request::Mcp(line[3..].trim().to_string(), reader)
                } else {
                    Request::Activate
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
            let request = match request {
                Request::Mcp(hello, reader) => {
                    cx.update(|cx| crate::mcp::server::serve(&hello, reader, stream, cx));
                    continue;
                }
                request => request,
            };
            cx.update(|cx| {
                match request {
                    Request::Open(path) if path.exists() => crate::app::open_project(path, cx),
                    _ => {
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
    if let Some(path) = listening() {
        std::fs::remove_file(path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_of_the_same_user_is_accepted() {
        let (a, b) = UnixStream::pair().unwrap();
        assert!(same_user(&a) && same_user(&b));
    }
}

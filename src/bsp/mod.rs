//! zcode's Build Server Protocol server: how sourcekit-lsp learns to
//! compile an Xcode project's files, the job xcode-build-server does.
//!
//! sourcekit-lsp starts a build server it finds a BSP connection file for.
//! It looks in the project's `.bsp/`, then in `$XDG_DATA_HOME/bsp/`, then
//! for a `buildServer.json` at the root. zcode never writes into the
//! project: it starts sourcekit-lsp with `XDG_DATA_HOME` pointing at a
//! folder of its own whose `bsp/zcode.json` names `zcode bsp`. A project
//! with a build server of its own (`.bsp/` or `buildServer.json`, say
//! xcode-build-server's) keeps it.
//!
//! `zcode bsp` connects to the running zcode's socket (`crate::instance`)
//! and relays the protocol both ways; zcode answers on threads of its own
//! from the project's live build graph ([`graph`], kept current by
//! [`model`]), and tells sourcekit-lsp which targets changed when a file
//! is added, a target's membership or a build setting is edited, or the
//! index build finishes. Without the socket, or with
//! `"sourcekit_build_settings": "compilation_database"`, sourcekit-lsp reads
//! the index build's compilation database instead, as before.

pub mod graph;
pub mod inputs;
pub mod model;
pub mod server;

use std::io::{BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// The socket `zcode bsp` connects to, set for the sourcekit-lsp zcode
/// starts (each zcode its own).
pub const SOCKET_ENV: &str = "ZCODE_BSP_SOCKET";

/// How sourcekit-lsp is started for a project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    /// Its arguments: where the compilation database is.
    pub args: Vec<String>,
    /// Through zcode's build server, answering on this socket.
    pub build_server: Option<PathBuf>,
}

impl Launch {
    /// The arguments and environment to start sourcekit-lsp with for the
    /// project at `root`. Blocking: writes zcode's connection file.
    pub fn resolve(&self, root: &Path) -> (Vec<String>, Vec<(String, String)>) {
        let Some(socket) = &self.build_server else { return (self.args.clone(), vec![]) };
        if let Some(own) = own_build_server(root) {
            log::info!("build server: {} has its own ({}); sourcekit-lsp uses that", root.display(), own.display());
            return (self.args.clone(), vec![]);
        }
        match write_connection_file() {
            Ok(dir) => (
                self.args.clone(),
                vec![("XDG_DATA_HOME".into(), dir.display().to_string()), (SOCKET_ENV.into(), socket.display().to_string())],
            ),
            Err(err) => {
                log::warn!("build server: couldn’t write its connection file ({err:#}); sourcekit-lsp falls back to the compilation database");
                (self.args.clone(), vec![])
            }
        }
    }
}

/// A build server the project brings: what sourcekit-lsp would find
/// before zcode's.
pub fn own_build_server(root: &Path) -> Option<PathBuf> {
    let bsp = root.join(".bsp");
    let in_bsp = std::fs::read_dir(&bsp).ok().and_then(|entries| {
        entries.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "json"))
    });
    in_bsp.or_else(|| Some(root.join("buildServer.json")).filter(|p| p.is_file()))
}

/// The connection file's folder for this zcode: `XDG_DATA_HOME` for its
/// sourcekit-lsp. One per zcode program, since the file names it.
fn connection_dir(program: &Path) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut hasher);
    crate::paths::cache_dir().join("bsp").join(format!("{:016x}", hasher.finish()))
}

/// The BSP connection file naming `zcode bsp`.
pub fn connection_file(program: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": "zcode",
        "version": env!("CARGO_PKG_VERSION"),
        "bspVersion": "2.2.0",
        "languages": ["swift", "c", "cpp", "objective-c", "objective-cpp"],
        "argv": [program, "bsp"],
    })
}

/// Writes (if it changed) `bsp/zcode.json` for this zcode; returns its
/// `XDG_DATA_HOME`.
fn write_connection_file() -> Result<PathBuf> {
    let program = std::env::current_exe()?;
    let dir = connection_dir(&program);
    let file = dir.join("bsp/zcode.json");
    let text = serde_json::to_string_pretty(&connection_file(&program))?;
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        std::fs::create_dir_all(file.parent().unwrap())?;
        // Another zcode may write the same file at the same time.
        let partial = file.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&partial, &text)?;
        std::fs::rename(&partial, &file)?;
    }
    Ok(dir)
}

/// `zcode bsp`, as sourcekit-lsp starts it: relays stdin and stdout to the
/// running zcode's socket until either side closes. `zcode bsp --graph
/// <file>` answers from a graph saved as JSON instead (the tests).
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if let [flag, file] = args.as_slice()
        && flag == "--graph"
    {
        return serve_file(Path::new(file));
    }
    let path = std::env::var_os(SOCKET_ENV).map(PathBuf::from).unwrap_or_else(crate::instance::socket_path);
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(_) => {
            eprintln!("zcode bsp: zcode isn’t running (nothing is listening at {})", path.display());
            return 1;
        }
    };
    if !crate::instance::same_user(&stream) {
        eprintln!("zcode bsp: {} belongs to another user", path.display());
        return 1;
    }
    if writeln!(stream, "bsp").is_err() {
        return 1;
    }
    let Ok(mut to_zcode) = stream.try_clone() else { return 1 };
    std::thread::spawn(move || {
        std::io::copy(&mut std::io::stdin().lock(), &mut to_zcode).ok();
        to_zcode.shutdown(std::net::Shutdown::Write).ok();
    });
    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0; 64 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(&buffer[..n]).and_then(|_| stdout.flush()).is_err() {
                    break;
                }
            }
        }
    }
    0
}

fn serve_file(file: &Path) -> i32 {
    let graph = match std::fs::read(file).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<graph::BuildGraph>(&b)?)) {
        Ok(graph) => graph,
        Err(err) => {
            eprintln!("zcode bsp: {}: {err:#}", file.display());
            return 1;
        }
    };
    let shared = server::Shared::fixed(PathBuf::from("/"), graph);
    server::serve(BufReader::new(std::io::stdin()), std::io::stdout(), move |_| Some(shared.clone()));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_own_build_server_wins() {
        let root = std::env::temp_dir().join(format!("zcode-bsp-own-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(own_build_server(&root), None);
        std::fs::write(root.join("buildServer.json"), "{}").unwrap();
        assert_eq!(own_build_server(&root), Some(root.join("buildServer.json")));
        std::fs::create_dir_all(root.join(".bsp")).unwrap();
        std::fs::write(root.join(".bsp/xcode.json"), "{}").unwrap();
        assert_eq!(own_build_server(&root), Some(root.join(".bsp/xcode.json")));
        // Theirs: sourcekit-lsp is started without zcode's.
        let launch = Launch { args: vec![], build_server: Some("/tmp/zcode.sock".into()) };
        assert_eq!(launch.resolve(&root), (vec![], vec![]));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_connection_file_names_zcode_bsp() {
        let file = connection_file(Path::new("/Applications/zcode.app/Contents/MacOS/zcode"));
        assert_eq!(file["argv"], serde_json::json!(["/Applications/zcode.app/Contents/MacOS/zcode", "bsp"]));
        assert!(file["languages"].as_array().unwrap().iter().any(|l| l == "swift"));
    }
}

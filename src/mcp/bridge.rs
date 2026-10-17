//! `zcode mcp`: the MCP server as clients start it, a process speaking the
//! protocol on stdin and stdout (newline-delimited JSON-RPC, MCP's stdio
//! transport). It connects to the running zcode's socket, says where it
//! was started and which agent panel (if any) started it, then relays
//! every line both ways until either side closes. It never starts zcode:
//! with no zcode running it says so on stderr and exits.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::{AGENT_ENV, SOCKET_ENV};

/// Runs the bridge; the process's exit status.
pub fn run() -> i32 {
    let path = std::env::var_os(SOCKET_ENV).map(PathBuf::from).unwrap_or_else(crate::instance::socket_path);
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(_) => {
            eprintln!(
                "zcode mcp: zcode isn’t running (nothing is listening at {}). Open your project in zcode (or start `zcode --background`) and try again.",
                path.display()
            );
            return 1;
        }
    };
    if !crate::instance::same_user(&stream) {
        eprintln!("zcode mcp: {} belongs to another user", path.display());
        return 1;
    }
    let hello = serde_json::json!({
        "cwd": std::env::current_dir().ok(),
        "agent": std::env::var(AGENT_ENV).ok().filter(|t| !t.is_empty()),
        "pid": std::process::id(),
    });
    if writeln!(stream, "mcp {hello}").is_err() {
        eprintln!("zcode mcp: zcode closed the connection");
        return 1;
    }
    let Ok(mut to_zcode) = stream.try_clone() else { return 1 };
    std::thread::spawn(move || {
        std::io::copy(&mut std::io::stdin().lock(), &mut to_zcode).ok();
        // The client is done: zcode cancels what's still running and closes.
        to_zcode.shutdown(std::net::Shutdown::Write).ok();
    });
    let mut stdout = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if stdout.write_all(line.as_bytes()).and_then(|_| stdout.write_all(b"\n")).and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
    0
}

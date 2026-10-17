//! zcode as a Model Context Protocol server: coding agents build, run and
//! test the project open in zcode, read its issues, build log and editor,
//! evaluate in the debugger and take screenshots, through a deliberate
//! set of tools (`tools.rs`).
//!
//! `zcode mcp` is the server as MCP clients start it, on stdio
//! (`bridge.rs`): it connects to the running zcode over its Unix socket
//! (`crate::instance`, the same one `zcode <path>` hands paths over) and
//! relays messages both ways. The protocol is answered inside zcode
//! (`server.rs`, `protocol.rs`), where the project's state is. The agent
//! panel hands the same command to the agents it starts, with a token
//! that tells zcode which panel's agent is calling, so tools that change
//! things ask there first.

pub mod bridge;
pub mod protocol;
pub mod server;
pub mod tools;

use std::collections::HashMap;

use gpui_kit::{App, Global, WeakEntity};

use crate::workspace::Workspace;

/// Set by the agent panel for the `zcode mcp` it hands its agent.
pub const AGENT_ENV: &str = "ZCODE_MCP_AGENT";
/// The socket `zcode mcp` connects to, when it isn't the usual one.
pub const SOCKET_ENV: &str = "ZCODE_MCP_SOCKET";

/// Agent panels by the token their agents' `zcode mcp` presents.
#[derive(Default)]
struct AgentTokens(HashMap<String, WeakEntity<Workspace>>);
impl Global for AgentTokens {}

/// An unguessable token for an agent panel.
pub fn new_token() -> String {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        // Not expected on macOS; still distinct per panel.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        bytes = (nanos ^ (std::process::id() as u128) << 64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn register_agent(token: String, workspace: WeakEntity<Workspace>, cx: &mut App) {
    let tokens = &mut cx.default_global::<AgentTokens>().0;
    tokens.retain(|_, w| w.upgrade().is_some());
    tokens.insert(token, workspace);
}

/// The workspace whose agent panel `token` is.
pub fn agent_workspace(token: &str, cx: &mut App) -> Option<WeakEntity<Workspace>> {
    cx.default_global::<AgentTokens>().0.get(token).cloned()
}

/// `zcode mcp` as an ACP session's MCP server, for the panel known by
/// `token`. `None` when zcode isn't listening on a socket to serve it.
pub fn agent_server(token: &str) -> Option<crate::acp::schema::McpServer> {
    use crate::acp::schema::{EnvVariable, McpServer, McpServerStdio};
    let socket = crate::instance::listening()?;
    let program = std::env::current_exe().ok()?;
    let server = McpServerStdio::new("zcode", program)
        .args(vec!["mcp".to_string()])
        .env(vec![EnvVariable::new(AGENT_ENV, token), EnvVariable::new(SOCKET_ENV, socket.to_string_lossy().to_string())]);
    Some(McpServer::Stdio(server))
}

#[cfg(test)]
mod tests {
    use super::new_token;

    #[test]
    fn tokens_are_long_and_distinct() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}

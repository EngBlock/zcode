//! The agents the panel can run: Claude Code, Codex and Gemini CLI through
//! their ACP adapters (fetched by `npx`, as Zed's registry does), and any
//! others `agent_servers` in settings.json adds, in Zed's format:
//!
//! ```json
//! "agent_servers": {
//!   "claude": { "env": { "ANTHROPIC_MODEL": "…" } },          // a built-in, changed
//!   "My Agent": { "command": "/usr/local/bin/my-agent", "args": ["--acp"] }
//! }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use super::connection::Launch;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentServer {
    /// The key in `agent_servers`: `claude`, `codex`, `gemini`, or a custom name.
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// What `agent_servers` says about one agent; whatever it leaves out of a
/// built-in stays the built-in's.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AgentServerContent {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
}

pub fn builtins() -> Vec<AgentServer> {
    let npx = |id: &str, name: &str, package: &str, extra: &[&str]| AgentServer {
        id: id.into(),
        name: name.into(),
        command: "npx".into(),
        args: ["-y", package].iter().chain(extra).map(|s| s.to_string()).collect(),
        env: BTreeMap::new(),
    };
    vec![
        npx("claude", "Claude Code", "@agentclientprotocol/claude-agent-acp", &[]),
        npx("codex", "Codex", "@agentclientprotocol/codex-acp", &[]),
        npx("gemini", "Gemini CLI", "@google/gemini-cli", &["--acp"]),
    ]
}

/// The built-ins as `agent_servers` changes them, then its own agents in
/// the order it lists them. A custom agent without a command is skipped.
pub fn resolve(content: &[(String, AgentServerContent)]) -> Vec<AgentServer> {
    let mut agents = builtins();
    for (id, entry) in content {
        let agent = match agents.iter_mut().find(|a| &a.id == id) {
            Some(agent) => agent,
            None => {
                let Some(command) = &entry.command else { continue };
                agents.push(AgentServer { id: id.clone(), name: id.clone(), command: command.clone(), args: vec![], env: BTreeMap::new() });
                agents.last_mut().unwrap()
            }
        };
        if let Some(command) = &entry.command {
            agent.command = command.clone();
            // A new command doesn't take the built-in's arguments.
            agent.args = entry.args.clone().unwrap_or_default();
        } else if let Some(args) = &entry.args {
            agent.args = args.clone();
        }
        if let Some(env) = &entry.env {
            agent.env.extend(env.clone());
        }
    }
    agents
}

impl AgentServer {
    /// The program to run, found on the login shell's `PATH` (an app
    /// started from the Finder has a short one), and its environment.
    /// Runs the shell the first time: call it off the main thread.
    pub fn launch(&self) -> Result<Launch, String> {
        let path = login_path();
        let program = find_program(&self.command, path).ok_or_else(|| {
            format!("“{}” wasn’t found on your PATH. Install it, or set \"agent_servers\" in settings.json to where it is.", self.command)
        })?;
        let mut env: Vec<(String, String)> = vec![("PATH".into(), path.to_string())];
        env.extend(self.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(Launch { program, args: self.args.clone(), env })
    }
}

/// `command` itself when it's a path, else the first match on `path`.
pub fn find_program(command: &str, path: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let program = PathBuf::from(shellexpand_home(command));
        return program.is_file().then_some(program);
    }
    std::env::split_paths(path).map(|dir| dir.join(command)).find(|candidate| is_executable(candidate))
}

fn shellexpand_home(command: &str) -> String {
    match (command.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => command.to_string(),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The `PATH` an interactive login shell has, read once per run (Zed does
/// the same); this process's own with the usual tool folders if the shell
/// doesn't answer in a few seconds.
pub fn login_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let own = std::env::var("PATH").unwrap_or_default();
        let fallback = || {
            let mut dirs: Vec<String> = own.split(':').filter(|d| !d.is_empty()).map(str::to_string).collect();
            for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
                if !dirs.iter().any(|d| d == extra) {
                    dirs.push(extra.into());
                }
            }
            dirs.join(":")
        };
        // Tests and CI pin the environment: no shell.
        if std::env::var_os("ZCODE_AGENT_NO_LOGIN_SHELL").is_some() {
            return fallback();
        }
        login_shell_path(Duration::from_secs(5)).unwrap_or_else(fallback)
    })
}

fn login_shell_path(timeout: Duration) -> Option<String> {
    const MARKER: &str = "__ZCODE_PATH__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child = std::process::Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '{MARKER}%s{MARKER}' \"$PATH\"")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let start = out.find(MARKER)? + MARKER.len();
    let end = start + out[start..].find(MARKER)?;
    Some(out[start..end].to_string()).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(json: serde_json::Value) -> Vec<(String, AgentServerContent)> {
        json.as_object().unwrap().iter().map(|(k, v)| (k.clone(), serde_json::from_value(v.clone()).unwrap())).collect()
    }

    #[test]
    fn builtins_changed_and_custom_agents_added_in_order() {
        let agents = resolve(&content(serde_json::json!({
            "Zeta": { "command": "/opt/zeta", "args": ["--acp"], "env": { "A": "1" } },
            "claude": { "env": { "ANTHROPIC_MODEL": "opus" } },
            "gemini": { "command": "gemini", "args": ["--acp"] },
            "codex": { "args": ["-y", "codex-acp@1"] },
            "Nothing": { "args": ["no command"] },
        })));
        let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "gemini", "Zeta"]);
        assert_eq!(agents[0].command, "npx");
        assert_eq!(agents[0].args, ["-y", "@agentclientprotocol/claude-agent-acp"]);
        assert_eq!(agents[0].env.get("ANTHROPIC_MODEL").map(String::as_str), Some("opus"));
        assert_eq!(agents[1].args, ["-y", "codex-acp@1"]);
        assert_eq!((agents[2].command.as_str(), agents[2].args.as_slice()), ("gemini", ["--acp".to_string()].as_slice()));
        assert_eq!((agents[3].name.as_str(), agents[3].command.as_str()), ("Zeta", "/opt/zeta"));
    }

    #[test]
    fn programs_are_found_on_the_path() {
        assert_eq!(find_program("sh", "/nonexistent:/bin"), Some(PathBuf::from("/bin/sh")));
        assert_eq!(find_program("/bin/sh", ""), Some(PathBuf::from("/bin/sh")));
        assert_eq!(find_program("no-such-agent-program", "/bin:/usr/bin"), None);
        assert_eq!(find_program("/no/such/agent", "/bin"), None);
    }
}

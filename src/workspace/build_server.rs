//! The control channel's view of sourcekit-lsp's build settings: how it was
//! started, and what zcode's build server (`crate::bsp`) answers.

use std::path::Path;

use gpui_kit::*;
use serde_json::{Value, json};

use super::Workspace;

impl Workspace {
    /// What `dump` and `bsp status` show.
    pub(super) fn build_server_status(&self, cx: &App) -> Value {
        let mut status = self.build_server.as_ref().map(|server| server.read(cx).status()).unwrap_or(json!({}));
        status["setting"] = json!(self.settings.current.sourcekit_build_settings.name());
        status["through_build_server"] = json!(self.lsp.read(cx).sourcekit().build_server.is_some());
        status
    }

    /// `bsp status`, `bsp options <file>` (the arguments sourcekit-lsp gets
    /// for it, in its first target) and `bsp refresh`.
    pub fn build_server_command(&mut self, arg: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let (command, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        match command {
            "" | "status" => Ok(self.build_server_status(cx).to_string()),
            "options" => {
                let server = self.build_server.as_ref().ok_or("not an Xcode project")?;
                let graph = server.read(cx).graph();
                let file = Path::new(rest.trim());
                let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
                let targets: Vec<&str> = graph.targets_of(&file).into_iter().map(|t| t.name.as_str()).collect();
                let options = graph.targets_of(&file).first().and_then(|t| graph.options(&t.id, &file, None));
                Ok(json!({
                    "targets": targets,
                    "arguments": options.as_ref().map(|o| &o.arguments),
                    "origin": options.as_ref().map(|o| o.origin),
                })
                .to_string())
            }
            "refresh" => {
                let server = self.build_server.clone().ok_or("not an Xcode project")?;
                server.update(cx, |server, cx| server.project_changed(cx));
                Ok(String::new())
            }
            other => Err(format!("usage: bsp status|options <file>|refresh (not {other})")),
        }
    }
}

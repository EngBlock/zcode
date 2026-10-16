//! The Source Control navigator's status and branch (read-only, from git).

use std::path::PathBuf;

use gpui_kit::*;

use super::Workspace;

impl Workspace {
    pub fn refresh_scm(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut status = std::process::Command::new("/usr/bin/git");
                    status
                        .args(["status", "--porcelain=v1", "-uall"])
                        .current_dir(&root);
                    let mut branch = std::process::Command::new("/usr/bin/git");
                    branch
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&root);
                    let toplevel = std::process::Command::new("/usr/bin/git")
                        .args(["rev-parse", "--show-toplevel"])
                        .current_dir(&root)
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
                    let status = crate::build::process::run_quiet(status).await.ok();
                    let branch = crate::build::process::run_quiet(branch).await.ok();
                    (status, branch, toplevel)
                })
                .await;
            this.update(cx, |this, cx| {
                let (status, branch, toplevel) = result;
                if let (Some((true, out)), Some(top)) = (status, toplevel) {
                    this.scm_status = out
                        .lines()
                        .filter(|l| l.len() > 3)
                        .map(|l| {
                            // "XY path" or "R  old -> new"; show the most
                            // significant status letter and the current path.
                            let code = l[..2].trim();
                            let status = if code == "??" { "??" } else { &code[..1] };
                            let path = l[3..].rsplit(" -> ").next().unwrap_or(&l[3..]);
                            (status.to_string(), top.join(path.trim_matches('"')))
                        })
                        .collect();
                }
                this.scm_branch = branch
                    .filter(|(ok, _)| *ok)
                    .map(|(_, b)| b.trim().to_string());
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }
}

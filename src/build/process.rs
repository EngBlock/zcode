//! Spawning tool processes in their own process group and streaming output.

use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context as _, Result};
use futures::AsyncBufReadExt as _;
use smol::channel;
use smol::io::BufReader;

/// Handle used to stop a running process tree.
#[derive(Clone, Default)]
pub struct ProcessHandle {
    pgid: Arc<AtomicI32>,
}

impl ProcessHandle {
    pub fn is_running(&self) -> bool {
        self.pgid.load(Ordering::SeqCst) > 0
    }

    /// SIGTERM the whole process group, escalating to SIGKILL.
    pub fn terminate(&self) {
        let pgid = self.pgid.load(Ordering::SeqCst);
        if pgid <= 0 {
            return;
        }
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
        }
        let handle = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let pgid = handle.pgid.load(Ordering::SeqCst);
            if pgid > 0 {
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        });
    }
}

/// Runs `cmd`, sending each stdout/stderr line to `lines`. Resolves to the
/// exit code (`None` when killed by a signal).
pub async fn run_streaming(
    mut cmd: Command,
    lines: channel::Sender<String>,
    handle: ProcessHandle,
) -> Result<Option<i32>> {
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NSUnbufferedIO", "YES");
    let program = format!("{:?}", cmd.get_program());
    // `From<std::process::Command>` does not carry stdio settings over.
    let mut child = smol::process::Command::from(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    handle.pgid.store(child.id() as i32, Ordering::SeqCst);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let pump = |stream: Option<Box<dyn smol::io::AsyncRead + Unpin + Send>>,
                lines: channel::Sender<String>| async move {
        let Some(stream) = stream else { return };
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    if lines.send(line).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    futures::join!(
        pump(
            stdout.map(|s| Box::new(s) as Box<dyn smol::io::AsyncRead + Unpin + Send>),
            lines.clone()
        ),
        pump(
            stderr.map(|s| Box::new(s) as Box<dyn smol::io::AsyncRead + Unpin + Send>),
            lines.clone()
        ),
    );
    let status = child.status().await?;
    handle.pgid.store(0, Ordering::SeqCst);
    Ok(status.code())
}

/// Runs a short command to completion, returning combined output.
pub async fn run_quiet(mut cmd: Command) -> Result<(bool, String)> {
    cmd.stdin(Stdio::null());
    let output = smol::process::Command::from(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

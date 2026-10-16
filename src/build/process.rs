//! Spawning tool processes in their own process group and streaming output.

use std::os::fd::{FromRawFd as _, OwnedFd};
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

/// Sends each line of `stream` to `lines` until it ends.
async fn pump(stream: Option<Box<dyn smol::io::AsyncRead + Unpin + Send>>, lines: channel::Sender<String>) {
    let Some(stream) = stream else { return };
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            // A pseudo-terminal reports EIO once the program closes it.
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
}

/// Like `run_streaming`, with stdout and stderr on a pseudo-terminal, as
/// Xcode's console gives the programs it runs: their output is then line
/// buffered, so it streams as it is printed rather than at exit.
pub async fn run_streaming_tty(
    mut cmd: Command,
    lines: channel::Sender<String>,
    handle: ProcessHandle,
) -> Result<Option<i32>> {
    let (mut master, mut slave) = (0, 0);
    let opened = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    };
    if opened != 0 {
        return run_streaming(cmd, lines, handle).await;
    }
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave))
        .env("NSUnbufferedIO", "YES");
    let program = format!("{:?}", cmd.get_program());
    let mut child = cmd.spawn().with_context(|| format!("failed to start {program}"))?;
    // Only the child may hold the terminal open, so reading ends with it.
    drop(cmd);
    handle.pgid.store(child.id() as i32, Ordering::SeqCst);
    let output = smol::Unblock::new(std::fs::File::from(master));
    pump(Some(Box::new(output)), lines).await;
    let status = smol::unblock(move || child.wait()).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn terminal_output_streams_before_exit() {
        // perl, like Swift's print, buffers stdout fully unless it is a terminal.
        let mut cmd = Command::new("/usr/bin/perl");
        cmd.args(["-e", "print qq(first\\n); sleep 2; print STDERR qq(second\\n); exit 3"]);
        let (tx, rx) = channel::unbounded();
        let started = Instant::now();
        let run = std::thread::spawn(move || smol::block_on(run_streaming_tty(cmd, tx, ProcessHandle::default())));
        let first = smol::block_on(rx.recv()).unwrap();
        assert_eq!(first, "first");
        assert!(started.elapsed() < Duration::from_millis(1500), "arrived at exit");
        assert_eq!(smol::block_on(rx.recv()).unwrap(), "second");
        assert_eq!(run.join().unwrap().unwrap(), Some(3));
    }
}

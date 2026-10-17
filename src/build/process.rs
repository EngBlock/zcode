//! Spawning tool processes in their own process group and streaming output;
//! ending a launched app wherever it ended up.

use std::os::fd::{FromRawFd as _, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::AsyncBufReadExt as _;
use futures::future::Either;
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

    /// The process group's leader: the program run, or 0.
    pub fn pid(&self) -> i32 {
        self.pgid.load(Ordering::SeqCst).max(0)
    }

    /// SIGKILL the whole process group now (quitting: nothing waits to
    /// escalate).
    pub fn kill(&self) {
        let pgid = self.pgid.load(Ordering::SeqCst);
        if pgid > 0 {
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }

    /// SIGINT the process group, as Ctrl-C in a terminal does (xctrace
    /// then saves its recording and exits; `xcrun` before it gets it too).
    pub fn interrupt(&self) {
        let pgid = self.pgid.load(Ordering::SeqCst);
        if pgid > 0 {
            unsafe {
                libc::killpg(pgid, libc::SIGINT);
            }
        }
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

/// The program starts with no signals blocked. zcode starts programs from
/// its background threads (GCD's), which block SIGTERM, SIGINT, SIGHUP and
/// most others, and a child inherits that: Stop's SIGTERM would wait for
/// the SIGKILL after it, and xctrace would never see Ctrl-C's SIGINT.
fn unblock_signals(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            Ok(())
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
    unblock_signals(&mut cmd);
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
    unblock_signals(&mut cmd);
    let program = format!("{:?}", cmd.get_program());
    let mut child = cmd.spawn().with_context(|| format!("failed to start {program}"))?;
    drop(cmd);
    handle.pgid.store(child.id() as i32, Ordering::SeqCst);
    // Waited for on a thread of its own, so it's reaped even when the run
    // is dropped (Stop cancels it) rather than staying a zombie.
    let (exited, exit) = futures::channel::oneshot::channel();
    std::thread::spawn(move || exited.send(child.wait()).ok());
    let output = smol::Unblock::new(std::fs::File::from(master));
    let pump = pump(Some(Box::new(output)), lines);
    futures::pin_mut!(pump);
    // Output ends when the last program holding the terminal closes it,
    // which can be one the child started and left running (xctrace's
    // app); the run ends with the child, once what it printed last is read.
    let status = match futures::future::select(pump, exit).await {
        Either::Left(((), exit)) => exit.await,
        Either::Right((status, pump)) => {
            futures::future::select(pump, smol::Timer::after(Duration::from_millis(300))).await;
            status
        }
    };
    let status = status.context("the program's wait ended")??;
    handle.pgid.store(0, Ordering::SeqCst);
    Ok(status.code())
}

/// A process zcode started that may not be its child or in a group it
/// owns (lldb-dap's debuggee, whose parent is debugserver, is orphaned
/// when the adapter goes), known by its pid and executable so a reused pid
/// is never signalled.
#[derive(Clone, Debug, PartialEq)]
pub struct AppProcess {
    pub pid: i32,
    pub executable: PathBuf,
}

impl AppProcess {
    /// The process `pid` as it is now (`None` when it's gone).
    pub fn find(pid: i32) -> Option<Self> {
        Some(Self { pid, executable: executable_of(pid)? })
    }

    pub fn is_running(&self) -> bool {
        executable_of(self.pid).is_some_and(|path| path == self.executable)
    }

    /// SIGKILL it now, if it's still this program.
    pub fn kill(&self) {
        if self.is_running() {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
        }
    }

    /// SIGTERM it, then SIGKILL if it outlives `grace`.
    pub fn terminate(&self, grace: Duration) {
        if !self.is_running() {
            return;
        }
        unsafe {
            libc::kill(self.pid, libc::SIGTERM);
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + grace;
            while std::time::Instant::now() < deadline {
                if !this.is_running() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            this.kill();
        });
    }
}

/// This user's processes running `executable`.
pub fn processes_running(executable: &Path) -> Vec<AppProcess> {
    let mut pids = vec![0i32; 8192];
    let bytes = unsafe {
        libc::proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, (pids.len() * std::mem::size_of::<i32>()) as i32)
    };
    if bytes <= 0 {
        return vec![];
    }
    pids.truncate(bytes as usize / std::mem::size_of::<i32>());
    pids.into_iter()
        .filter_map(AppProcess::find)
        .filter(|p| p.executable == executable)
        .collect()
}

/// A live process's executable (a zombie has none).
pub fn executable_of(pid: i32) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    (len > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..len as usize]).into_owned()))
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

    #[test]
    fn a_run_ends_with_its_program_when_another_holds_the_terminal() {
        // Like xctrace leaving the app it launched: the terminal stays open.
        let pid_file = std::env::temp_dir().join(format!("zcode-tty-holder-{}", std::process::id()));
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(format!("echo started; /bin/sleep 30 & echo $! > '{}'; exit 4", pid_file.display()));
        let (tx, rx) = channel::unbounded();
        let started = Instant::now();
        let code = smol::block_on(run_streaming_tty(cmd, tx, ProcessHandle::default())).unwrap();
        let holder: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        unsafe { libc::kill(holder, libc::SIGKILL) };
        std::fs::remove_file(&pid_file).ok();
        assert_eq!(code, Some(4));
        assert!(started.elapsed() < Duration::from_secs(5), "waited for the terminal to close");
        assert_eq!(smol::block_on(rx.recv()).unwrap(), "started");
    }

    #[test]
    fn programs_start_with_no_signals_blocked() {
        // As on gpui's background threads: SIGTERM blocked where the
        // program is started.
        std::thread::spawn(|| {
            unsafe {
                let mut term: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut term);
                libc::sigaddset(&mut term, libc::SIGTERM);
                libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut());
            }
            for tty in [false, true] {
                let mut cmd = Command::new("/bin/sh");
                cmd.args(["-c", "kill -TERM $$; sleep 1; echo survived"]);
                let (tx, rx) = channel::unbounded();
                let code = smol::block_on(async {
                    if tty { run_streaming_tty(cmd, tx, ProcessHandle::default()).await } else { run_streaming(cmd, tx, ProcessHandle::default()).await }
                });
                assert_eq!(code.unwrap(), None, "terminated by SIGTERM (tty: {tty})");
                assert!(rx.try_recv().is_err(), "nothing printed");
            }
        })
        .join()
        .unwrap();
    }

    fn gone(pid: i32) -> bool {
        // A reaped child is gone altogether; signal 0 finds a zombie too.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_dropped_run_is_reaped_after_terminate() {
        // Stop drops the run's task while its program runs; the program
        // gets the signal and then mustn't linger as a zombie.
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        let (tx, _rx) = channel::unbounded();
        let handle = ProcessHandle::default();
        let run = smol::spawn(run_streaming_tty(cmd, tx, handle.clone()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.pid() == 0 && Instant::now() < deadline {
            smol::block_on(smol::Timer::after(Duration::from_millis(20)));
        }
        let pid = handle.pid();
        assert!(pid > 0);
        drop(run);
        handle.terminate();
        assert!(gone(pid), "{pid} is still there");
    }

    #[test]
    fn an_app_process_outside_any_group_is_killed_by_pid() {
        // Like a debuggee whose debugserver went away: not our child's
        // group, found again by pid and path.
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30").process_group(0);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id() as i32;
        let app = AppProcess::find(pid).unwrap();
        assert_eq!(app.executable, PathBuf::from("/bin/sleep"));
        assert!(app.is_running());
        // Another program that has its pid isn't touched.
        let stranger = AppProcess { pid, executable: "/bin/cat".into() };
        assert!(!stranger.is_running());
        stranger.kill();
        assert!(app.is_running());
        app.terminate(Duration::from_secs(2));
        child.wait().unwrap();
        assert!(!app.is_running());
        assert!(AppProcess::find(pid).is_none());
    }

    #[test]
    fn processes_are_found_by_executable() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let found = processes_running(Path::new("/bin/sleep"));
        assert!(found.iter().any(|p| p.pid == pid), "{found:?}");
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!processes_running(Path::new("/bin/sleep")).iter().any(|p| p.pid == pid));
    }
}

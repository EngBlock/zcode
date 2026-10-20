//! The integrated terminal: a shell on a pseudo-terminal, its output parsed
//! into a grid of cells with scrollback by `alacritty_terminal` (Apache-2.0),
//! which also owns the PTY and the thread that reads it. This module starts
//! and stops that machinery and passes what the shell says to the UI;
//! `view.rs` is the terminal on screen (keys, mouse, selection, links) and
//! `element.rs` paints its grid.
//!
//! Output never waits on the UI: the reader thread parses into the grid
//! under a lock it holds for at most 64 KB at a time, and only tells the UI
//! that there is something new when it hasn't already (`output_pending`),
//! so a command spewing output costs one repaint per frame.

pub mod colors;
mod element;
pub mod keys;
mod lines;
pub mod links;
mod view;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use parking_lot::Mutex;

pub use lines::LineSplitter;
pub use view::{TerminalEvent, TerminalView};

/// Lines of scrollback each terminal keeps.
const SCROLLBACK: usize = 10_000;

/// What to run, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub dir: PathBuf,
    /// A program and its arguments; `None` is the user's shell:
    /// `ZCODE_TERMINAL_SHELL` (which tests set to a shell without the
    /// user's configuration), then `shell`, then the login shell.
    pub command: Option<(String, Vec<String>)>,
    /// The `terminal.shell` setting.
    pub shell: Option<(String, Vec<String>)>,
    /// `Some`: `PATH` is these folders, then the login shell's (`zcode`
    /// started from the Dock has only the system's); `None`: zcode's own,
    /// which a login shell replaces anyway.
    pub path: Option<Vec<PathBuf>>,
}

impl Spawn {
    pub fn shell(dir: PathBuf) -> Self {
        Self { dir, command: None, shell: None, path: None }
    }

    /// `/bin/sh -c <script>`.
    pub fn script(dir: PathBuf, script: &str) -> Self {
        Self { dir, command: Some(("/bin/sh".into(), vec!["-c".into(), script.into()])), shell: None, path: None }
    }

    /// Whether this is the user's shell: typing `exit` in it closes its tab.
    pub fn is_shell(&self) -> bool {
        self.command.is_none()
    }

    fn shell_override() -> Option<(String, Vec<String>)> {
        let words = shlex::split(&std::env::var("ZCODE_TERMINAL_SHELL").ok()?)?;
        let (program, args) = words.split_first()?;
        Some((program.clone(), args.to_vec()))
    }
}

/// What the reader thread tells the UI.
pub(crate) enum Notice {
    /// New output is in the grid.
    Output,
    /// The title a program set (OSC 0/2), or `None` when it reset it.
    Title(Option<String>),
    /// A program copied text (OSC 52).
    Copy(String),
    /// A program asked for one of the palette's colors (OSC 4/10/11).
    ColorRequest(usize, Arc<dyn Fn(alacritty_terminal::vte::ansi::Rgb) -> String + Sync + Send + 'static>),
    /// The process exited, with its exit code when it had one.
    Exited(Option<i32>),
}

/// Passes the terminal's events on: answers to queries go straight back
/// to the PTY from the reader thread; the rest to the UI.
#[derive(Clone)]
pub(crate) struct Listener {
    notices: smol::channel::Sender<Notice>,
    output_pending: Arc<AtomicBool>,
    pty: Arc<OnceLock<EventLoopSender>>,
    size: Arc<Mutex<WindowSize>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let notice = match event {
            Event::Wakeup => {
                if self.output_pending.swap(true, Ordering::AcqRel) {
                    return;
                }
                Notice::Output
            }
            Event::PtyWrite(text) => {
                if let Some(pty) = self.pty.get() {
                    pty.send(Msg::Input(Cow::Owned(text.into_bytes()))).ok();
                }
                return;
            }
            Event::TextAreaSizeRequest(format) => {
                let answer = format(*self.size.lock());
                if let Some(pty) = self.pty.get() {
                    pty.send(Msg::Input(Cow::Owned(answer.into_bytes()))).ok();
                }
                return;
            }
            Event::Title(title) => Notice::Title(Some(title)),
            Event::ResetTitle => Notice::Title(None),
            Event::ClipboardStore(_, text) => Notice::Copy(text),
            Event::ColorRequest(index, format) => Notice::ColorRequest(index, format),
            Event::ChildExit(status) => Notice::Exited(status.code()),
            Event::Exit => Notice::Exited(None),
            Event::ClipboardLoad(..) | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Bell => return,
        };
        self.notices.try_send(notice).ok();
    }
}

/// Runs `f` (a fork) with no signals blocked on this thread, then blocks
/// again what was.
fn with_no_signals_blocked<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let mut none: libc::sigset_t = std::mem::zeroed();
        let mut was: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, &mut was);
        let result = f();
        libc::pthread_sigmask(libc::SIG_SETMASK, &was, std::ptr::null_mut());
        result
    }
}

/// Where a terminal's output goes as lines besides its grid. Lines are
/// dropped while it's full rather than holding up the PTY.
pub type OutputTap = smol::channel::Sender<String>;

/// Starts the thread that reads `pty` into `term`; its sender, and the
/// thread that ends with it.
fn spawn_event_loop<T>(term: Arc<FairMutex<Term<Listener>>>, listener: Listener, pty: T) -> std::io::Result<(EventLoopSender, JoinHandle<()>)>
where
    T: tty::EventedPty + alacritty_terminal::event::OnResize + Send + 'static,
{
    let event_loop = EventLoop::new(term, listener, pty, true, false)?;
    let sender = event_loop.channel();
    let handle = event_loop.spawn();
    // The thread hands back the PTY when it ends; dropping it there (not
    // wherever the handle goes) keeps its `wait` off the UI.
    let reader = std::thread::Builder::new().name("terminal".into()).spawn(move || drop(handle.join()))?;
    Ok((sender, reader))
}

/// A PTY whose output is also read as lines (`OutputTap`).
struct TapPty {
    pty: tty::Pty,
    reader: TapReader,
}

/// Reads the PTY (through its own descriptor of it) and passes what came
/// on, line by line, to the tap.
struct TapReader {
    file: std::fs::File,
    splitter: LineSplitter,
    lines: OutputTap,
}

impl std::io::Read for TapReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.file.read(buf)?;
        for line in self.splitter.feed(&buf[..read]) {
            self.lines.try_send(line).ok();
        }
        Ok(read)
    }
}

impl tty::EventedReadWrite for TapPty {
    type Reader = TapReader;
    type Writer = std::fs::File;

    unsafe fn register(&mut self, poll: &Arc<polling::Poller>, interest: polling::Event, mode: polling::PollMode) -> std::io::Result<()> {
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<polling::Poller>, interest: polling::Event, mode: polling::PollMode) -> std::io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> std::io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut TapReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut std::fs::File {
        self.pty.writer()
    }
}

impl tty::EventedPty for TapPty {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        self.pty.next_child_event()
    }
}

impl alacritty_terminal::event::OnResize for TapPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty.on_resize(size)
    }
}

/// The grid's size in cells, and a cell's in points (for programs that ask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl GridSize {
    fn window_size(self) -> WindowSize {
        WindowSize { num_lines: self.rows, num_cols: self.columns, cell_width: self.cell_width, cell_height: self.cell_height }
    }

    fn term_size(self) -> TermSize {
        TermSize::new(self.columns as usize, self.rows as usize)
    }
}

/// A running terminal: the process on its PTY and the grid it draws into.
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    /// The process started on the PTY (the shell, or `login` running it).
    pub pid: i32,
    /// The PTY's controller side, for the foreground process; closed by the
    /// reader thread when it ends, so only used while it runs.
    controller: i32,
    reader: Mutex<Option<JoinHandle<()>>>,
    size: Arc<Mutex<WindowSize>>,
    pub(crate) output_pending: Arc<AtomicBool>,
    stopped: AtomicBool,
    pub spawn: Spawn,
}

impl Terminal {
    /// Starts `spawn` on a new PTY of `size`, its output also handed to
    /// `tap` line by line when there's one. This forks (and may ask the
    /// login shell for its `PATH`), so it's called off the main thread.
    pub(crate) fn start(spawn: Spawn, size: GridSize, tap: Option<OutputTap>) -> std::io::Result<(Terminal, smol::channel::Receiver<Notice>)> {
        let (notices, receiver) = smol::channel::unbounded();
        let output_pending = Arc::new(AtomicBool::new(false));
        let pty_sender = Arc::new(OnceLock::new());
        let window_size = Arc::new(Mutex::new(size.window_size()));
        let listener = Listener { notices, output_pending: output_pending.clone(), pty: pty_sender.clone(), size: window_size.clone() };

        let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size.term_size(), listener.clone())));

        let shell = spawn
            .command
            .clone()
            .or_else(Spawn::shell_override)
            .or_else(|| spawn.shell.clone())
            .map(|(program, args)| tty::Shell::new(program, args));
        let env = [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("TERM_PROGRAM", "zcode"),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ];
        let mut env: std::collections::HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(dirs) = &spawn.path {
            let login = crate::acp::agents::login_path();
            let dirs = dirs.iter().map(|d| d.to_string_lossy().into_owned()).chain(std::iter::once(login.to_string()));
            env.insert("PATH".into(), dirs.collect::<Vec<_>>().join(":"));
        }
        let options = tty::Options { shell, working_directory: Some(spawn.dir.clone()), drain_on_exit: true, env };
        // The child gets the forking thread's signal mask, and GCD's threads
        // block SIGHUP and most others: a program that doesn't reset it (a
        // dev server, unlike a shell) would never get the hang-up.
        let pty = with_no_signals_blocked(|| tty::new(&options, size.window_size(), 0))?;
        let pid = pty.child().id() as i32;
        let controller = std::os::fd::AsRawFd::as_raw_fd(pty.file());
        let (sender, reader) = match tap {
            Some(lines) => {
                let reader = TapReader { file: pty.file().try_clone()?, splitter: LineSplitter::default(), lines };
                spawn_event_loop(term.clone(), listener, TapPty { pty, reader })?
            }
            None => spawn_event_loop(term.clone(), listener, pty)?,
        };
        pty_sender.set(sender.clone()).ok();
        Ok((
            Terminal {
                term,
                pty: sender,
                pid,
                controller,
                reader: Mutex::new(Some(reader)),
                size: window_size,
                output_pending,
                stopped: AtomicBool::new(false),
                spawn,
            },
            receiver,
        ))
    }

    pub(crate) fn lock(&self) -> parking_lot::MutexGuard<'_, Term<Listener>> {
        self.term.lock()
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if !self.stopped.load(Ordering::Acquire) {
            self.pty.send(Msg::Input(bytes.into())).ok();
        }
    }

    pub fn size(&self) -> GridSize {
        let size = *self.size.lock();
        GridSize { columns: size.num_cols, rows: size.num_lines, cell_width: size.cell_width, cell_height: size.cell_height }
    }

    /// Fits the grid and the PTY to `size`; the program gets `SIGWINCH`.
    pub fn resize(&self, size: GridSize) {
        if self.size() == size {
            return;
        }
        *self.size.lock() = size.window_size();
        self.term.lock().resize(size.term_size());
        if !self.stopped.load(Ordering::Acquire) {
            self.pty.send(Msg::Resize(size.window_size())).ok();
        }
    }

    pub fn is_running(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
    }

    /// The process in the foreground: what the shell is running, or the
    /// shell itself.
    pub fn foreground_pid(&self) -> Option<i32> {
        if !self.is_running() {
            return None;
        }
        let group = unsafe { libc::tcgetpgrp(self.controller) };
        Some(if group > 0 { group } else { self.pid })
    }

    /// The foreground process's directory, which follows `cd`.
    pub fn current_dir(&self) -> Option<PathBuf> {
        self.foreground_pid().and_then(process_dir).or_else(|| process_dir(self.pid))
    }

    /// The foreground process's name (`zsh`, `vim`, `swift-build`).
    pub fn process_name(&self) -> Option<String> {
        self.foreground_pid().and_then(process_name)
    }

    /// Ends the terminal: the reader thread stops and closes the PTY, and
    /// the processes on it get `SIGHUP` (`SIGKILL` if they're still there a
    /// second later). Returns at once.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.pty.send(Msg::Shutdown).ok();
        let pid = self.pid;
        let reader = self.reader.lock().take();
        std::thread::Builder::new()
            .name("terminal stop".into())
            .spawn(move || {
                hang_up(pid);
                if let Some(reader) = reader {
                    reader.join().ok();
                }
            })
            .ok();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `SIGHUP` to the process and its group (it leads a session of its own),
/// then `SIGKILL` to whatever of the group outlives a second (a dev
/// server's child that ignores the hang-up, even once its parent is gone).
/// Reaps the process.
fn hang_up(pid: i32) {
    unsafe {
        libc::killpg(pid, libc::SIGHUP);
        libc::kill(pid, libc::SIGHUP);
    }
    let mut reaped = false;
    for _ in 0..20 {
        let mut status = 0;
        // Gone already (reaped by the reader thread) or reaped now.
        reaped = reaped || unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } != 0;
        let group_left = unsafe { libc::killpg(pid, 0) } == 0;
        if reaped && !group_left {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
        if !reaped {
            libc::kill(pid, libc::SIGKILL);
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
        }
    }
}

/// A process's current directory.
pub fn process_dir(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    if read != size {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path.to_bytes()));
    (!path.as_os_str().is_empty()).then(|| path.to_path_buf())
}

/// A process's name.
pub fn process_name(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 256];
    let len = unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buffer[..len as usize]).into_owned())
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::grid::Dimensions as _;

    use super::*;

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn screen_text(terminal: &Terminal) -> String {
        let term = terminal.lock();
        let mut text = String::new();
        let mut line = None;
        for cell in term.grid().display_iter() {
            if line != Some(cell.point.line) {
                if line.is_some() {
                    text.push('\n');
                }
                line = Some(cell.point.line);
            }
            text.push(cell.c);
        }
        text
    }

    const SIZE: GridSize = GridSize { columns: 120, rows: 10, cell_width: 8, cell_height: 16 };

    #[test]
    fn a_command_runs_in_its_folder_on_a_pty_of_the_grid_s_size() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let spawn = Spawn::script(dir.clone(), "pwd; stty size; printf '\\033[31mred\\033[0m'; exec sleep 5");
        let (terminal, notices) = Terminal::start(spawn, SIZE, None).unwrap();
        assert!(wait_until(|| screen_text(&terminal).contains("red")), "{}", screen_text(&terminal));
        let text = screen_text(&terminal);
        assert!(text.contains(dir.to_str().unwrap()) && text.contains("10 120"), "{text}");
        assert!(matches!(notices.try_recv(), Ok(Notice::Output)), "output was announced");
        assert_eq!(terminal.current_dir().as_deref(), Some(dir.as_path()));
        assert_eq!(terminal.process_name().as_deref(), Some("sleep"));
        // Resizing reaches the program.
        terminal.resize(GridSize { columns: 100, rows: 12, ..SIZE });
        assert_eq!(terminal.lock().columns(), 100);
    }

    #[test]
    fn output_is_announced_once_until_the_ui_has_seen_it() {
        let spawn = Spawn::script(std::env::temp_dir(), "yes | head -n 20000; sleep 5");
        let (terminal, notices) = Terminal::start(spawn, SIZE, None).unwrap();
        assert!(wait_until(|| screen_text(&terminal).contains('y')));
        std::thread::sleep(Duration::from_millis(300));
        let count = std::iter::from_fn(|| notices.try_recv().ok()).filter(|n| matches!(n, Notice::Output)).count();
        assert_eq!(count, 1, "one notice until the flag is cleared");
        terminal.output_pending.store(false, Ordering::Release);
        terminal.write(b"x".as_slice());
        terminal.resize(GridSize { columns: 30, ..SIZE });
    }

    #[test]
    fn stopping_hangs_up_the_processes_on_it() {
        let marker = std::env::temp_dir().join(format!("zcode-terminal-test-{}", std::process::id()));
        let script = format!("sleep 300 & echo $! > {}; wait", marker.display());
        let (terminal, _notices) = Terminal::start(Spawn::script(std::env::temp_dir(), &script), SIZE, None).unwrap();
        assert!(wait_until(|| std::fs::read_to_string(&marker).is_ok_and(|s| s.ends_with('\n'))));
        let child: i32 = std::fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
        std::fs::remove_file(&marker).ok();
        assert_eq!(unsafe { libc::kill(child, 0) }, 0, "running");
        terminal.stop();
        assert!(!terminal.is_running());
        assert!(wait_until(|| unsafe { libc::kill(child, 0) } != 0), "the background job is gone too");
    }

    #[test]
    fn output_reaches_a_tap_as_lines_with_the_path_asked_for() {
        let bin = std::env::temp_dir().join(format!("zcode-terminal-bin-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        let mut spawn = Spawn::script(std::env::temp_dir(), "printf 'a\\033[31mb\\033[0m\\r\\n'; echo \"$PATH\"; trap '' HUP; sleep 300 & echo $!; wait");
        spawn.path = Some(vec![bin.clone()]);
        let (tap, lines) = smol::channel::bounded(16);
        let (terminal, _notices) = Terminal::start(spawn, SIZE, Some(tap)).unwrap();
        let mut seen = vec![];
        assert!(wait_until(|| {
            seen.extend(std::iter::from_fn(|| lines.try_recv().ok()));
            seen.len() >= 3
        }));
        assert_eq!(seen[0], "ab");
        assert!(seen[1].starts_with(&format!("{}:", bin.display())), "{}", seen[1]);
        // A child that ignores the hang-up ends with the rest of its group.
        let child: i32 = seen[2].trim().parse().unwrap();
        assert!(wait_until(|| screen_text(&terminal).contains(&seen[2])), "the grid still gets it");
        terminal.stop();
        assert!(wait_until(|| unsafe { libc::kill(child, 0) } != 0), "the child that ignored SIGHUP is gone");
        std::fs::remove_dir_all(&bin).ok();
    }

    #[test]
    fn programs_start_with_no_signals_blocked() {
        // zcode starts terminals from GCD's threads, which block most signals.
        let probe = std::thread::spawn(|| {
            unsafe {
                let mut blocked: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut blocked);
                libc::sigaddset(&mut blocked, libc::SIGHUP);
                libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut());
            }
            let spawn = Spawn::script(std::env::temp_dir(), "python3 -c 'import signal; print(sorted(int(s) for s in signal.pthread_sigmask(signal.SIG_BLOCK, [])))'");
            let (tap, lines) = smol::channel::bounded(16);
            let (terminal, _notices) = Terminal::start(spawn, SIZE, Some(tap)).unwrap();
            (terminal, lines)
        });
        let (_terminal, lines) = probe.join().unwrap();
        let mut seen = vec![];
        assert!(wait_until(|| {
            seen.extend(std::iter::from_fn(|| lines.try_recv().ok()));
            seen.iter().any(|l| l.starts_with('['))
        }), "{seen:?}");
        assert!(seen.iter().any(|l| l == "[]"), "nothing blocked: {seen:?}");
    }

    #[test]
    fn exit_codes_reach_the_ui() {
        let (_terminal, notices) = Terminal::start(Spawn::script(std::env::temp_dir(), "exit 3"), SIZE, None).unwrap();
        let mut code = None;
        assert!(wait_until(|| {
            while let Ok(notice) = notices.try_recv() {
                // The exit status, then a plain "it ended".
                if let Notice::Exited(c) = notice {
                    code.get_or_insert(c);
                }
            }
            code.is_some()
        }));
        assert_eq!(code, Some(Some(3)));
    }
}

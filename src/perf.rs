//! Startup timing and memory monitoring.
//!
//! Memory is reported as the *physical footprint* (what Activity Monitor shows
//! in its "Memory" column) rather than plain RSS, because footprint includes
//! compressed and GPU-shared pages that RSS hides.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::{App, Global};

/// Number of samples retained for the memory history graph.
const HISTORY_LEN: usize = 120;

#[derive(Clone, Copy, Debug, Default)]
pub struct MemorySample {
    /// Physical footprint in bytes (Activity Monitor's "Memory").
    pub footprint: u64,
    /// Resident set size in bytes.
    pub resident: u64,
    /// Highest footprint the process has ever reached, in bytes.
    pub lifetime_peak: u64,
}

/// Samples memory for an arbitrary process (zcode itself, sourcekit-lsp, ...).
pub fn sample_pid(pid: i32) -> Option<MemorySample> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut _ as *mut libc::rusage_info_t,
        )
    };
    (ret == 0).then_some(MemorySample {
        footprint: info.ri_phys_footprint,
        resident: info.ri_resident_size,
        lifetime_peak: info.ri_lifetime_max_phys_footprint,
    })
}

/// Executable path of a process.
fn process_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
}

/// PIDs of this user's processes whose executable path ends with `suffix`.
fn pids_with_path_suffix(suffix: &str) -> Vec<i32> {
    let mut pids = vec![0i32; 8192];
    let bytes = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr() as *mut libc::c_void,
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if bytes <= 0 {
        return vec![];
    }
    pids.truncate(bytes as usize);
    pids.into_iter()
        .filter(|&pid| pid > 0 && process_path(pid).is_some_and(|p| p.ends_with(suffix)))
        .collect()
}

/// sourcekit-lsp does its heavy lifting in `SourceKitService` XPC helpers
/// (launched by launchd, so not our children); include them.
pub fn sample_language_server(pid: i32) -> Option<MemorySample> {
    let mut total = sample_pid(pid)?;
    for helper in pids_with_path_suffix("/SourceKitService") {
        if let Some(s) = sample_pid(helper) {
            total.footprint += s.footprint;
            total.resident += s.resident;
            total.lifetime_peak += s.lifetime_peak;
        }
    }
    Some(total)
}

pub fn sample_self() -> MemorySample {
    sample_pid(std::process::id() as i32).unwrap_or_default()
}

/// Wall-clock time the kernel recorded for this process's `exec`, so startup
/// numbers include dyld and static initializers, not just `main`.
fn process_start_time() -> Option<SystemTime> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let ret = unsafe {
        libc::proc_pidinfo(
            std::process::id() as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if ret != size {
        return None;
    }
    Some(
        UNIX_EPOCH
            + Duration::from_secs(info.pbi_start_tvsec)
            + Duration::from_micros(info.pbi_start_tvusec),
    )
}

#[derive(Clone, Debug, Default)]
pub struct StartupTimings {
    /// exec() → main().
    pub exec_to_main: Option<Duration>,
    /// main() → window opened.
    pub main_to_window: Option<Duration>,
    /// main() → first frame presented.
    pub main_to_first_frame: Option<Duration>,
}

impl StartupTimings {
    pub fn total(&self) -> Option<Duration> {
        Some(self.exec_to_main.unwrap_or_default() + self.main_to_first_frame?)
    }

    /// exec() → the window server showing the first window's content.
    pub fn total_to_first_pixels(&self) -> Option<Duration> {
        Some(self.exec_to_main.unwrap_or_default() + *MAIN_TO_FIRST_PIXELS.get()?)
    }
}

/// main() → the first window's content visible in the window server's copy
/// of the window, set by `watch_first_pixels` (`--startup-bench` only).
pub static MAIN_TO_FIRST_PIXELS: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();

/// Polls the window server's image of window `number` from a background
/// thread until it shows drawn content: what the user actually sees, rather
/// than when GPUI handed a frame over.
pub fn watch_first_pixels(number: u32, main_started: Instant) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            // Taken after the capture returns, so a slow capture (up to ~20
            // ms) counts against zcode, never for it.
            if crate::screenshot::window_shows_content(number) == Some(true) {
                let _ = MAIN_TO_FIRST_PIXELS.set(main_started.elapsed());
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}

/// App-wide performance state.
pub struct Perf {
    main_started: Instant,
    pub startup: StartupTimings,
    pub history: VecDeque<MemorySample>,
    pub peak_footprint: u64,
    /// Footprints of child processes we spawned (e.g. the language server).
    pub language_server: Option<(i32, MemorySample)>,
    /// When set (`--perf-log <file>` / `ZCODE_PERF_LOG`), every sample is
    /// appended to this file as a JSON line for long-running monitoring.
    pub log_path: Option<PathBuf>,
}

/// Idle footprint above which the status bar flags zcode's memory.
pub const WARN_BASELINE_FOOTPRINT: u64 = 1536 * 1024 * 1024;
/// Footprint above which the status bar flags the language server's memory.
pub const WARN_LANGUAGE_SERVER_FOOTPRINT: u64 = 4096 * 1024 * 1024;

impl Global for Perf {}

impl Perf {
    pub fn new(main_started: Instant) -> Self {
        let exec_to_main = process_start_time().and_then(|exec| {
            let main_wall = SystemTime::now() - main_started.elapsed();
            main_wall.duration_since(exec).ok()
        });
        Self {
            main_started,
            startup: StartupTimings {
                exec_to_main,
                ..Default::default()
            },
            history: VecDeque::with_capacity(HISTORY_LEN),
            peak_footprint: 0,
            language_server: None,
            log_path: std::env::var_os("ZCODE_PERF_LOG").map(PathBuf::from),
        }
    }

    pub fn main_started(&self) -> Instant {
        self.main_started
    }

    pub fn mark_window_opened(&mut self) {
        self.startup.main_to_window = Some(self.main_started.elapsed());
    }

    pub fn mark_first_frame(&mut self) {
        if self.startup.main_to_first_frame.is_none() {
            self.startup.main_to_first_frame = Some(self.main_started.elapsed());
        }
    }

    pub fn record(&mut self, sample: MemorySample) {
        if self.history.len() == HISTORY_LEN {
            self.history.pop_front();
        }
        self.peak_footprint = self.peak_footprint.max(sample.footprint);
        self.history.push_back(sample);
    }

    /// Lowest footprint over the last ~30 s. While a window is frontmost,
    /// macOS briefly attributes ~200 MB of compositor/GPU memory to each
    /// redraw (a bare GPUI window shows the same), so the idle baseline is the
    /// better measure of what zcode itself holds.
    pub fn baseline_footprint(&self) -> u64 {
        self.history
            .iter()
            .rev()
            .take(15)
            .map(|s| s.footprint)
            .min()
            .unwrap_or(0)
    }

    pub fn current(&self) -> MemorySample {
        self.history.back().copied().unwrap_or_default()
    }

    /// Whether memory is high enough to flag in the status bar.
    pub fn memory_warning(&self) -> bool {
        self.history.len() >= 5 && self.baseline_footprint() > WARN_BASELINE_FOOTPRINT
            || self
                .language_server
                .is_some_and(|(_, s)| s.footprint > WARN_LANGUAGE_SERVER_FOOTPRINT)
    }

    fn log_line(&self) -> String {
        let mem = self.current();
        serde_json::json!({
            "ts_ms": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            "footprint_mb": mb(mem.footprint),
            "baseline_footprint_mb": mb(self.baseline_footprint()),
            "resident_mb": mb(mem.resident),
            "peak_footprint_mb": mb(self.peak_footprint.max(mem.lifetime_peak)),
            "language_server_footprint_mb": self.language_server.map(|(_, s)| mb(s.footprint)),
        })
        .to_string()
    }

    /// The status bar's memory text.
    pub fn status_label(&self) -> String {
        let mem = self.current();
        match self.language_server.map(|(_, s)| s.footprint).filter(|f| *f > 0) {
            Some(ls) => format!("{} · LSP {}", format_bytes(mem.footprint), format_bytes(ls)),
            None => format_bytes(mem.footprint),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// One-line JSON report, used by `zcode --startup-bench` and the perf log.
    pub fn report_json(&self) -> String {
        let ms = |d: Option<Duration>| d.map(|d| d.as_secs_f64() * 1000.0);
        let mem = self.current();
        serde_json::json!({
            "exec_to_main_ms": ms(self.startup.exec_to_main),
            "main_to_window_ms": ms(self.startup.main_to_window),
            "main_to_first_frame_ms": ms(self.startup.main_to_first_frame),
            "total_to_first_frame_ms": ms(self.startup.total()),
            "total_to_first_pixels_ms": ms(self.startup.total_to_first_pixels()),
            "footprint_mb": mb(mem.footprint),
            "baseline_footprint_mb": mb(self.baseline_footprint()),
            "resident_mb": mb(mem.resident),
            "peak_footprint_mb": mb(self.peak_footprint.max(mem.lifetime_peak)),
            "language_server_footprint_mb": self.language_server.map(|(_, s)| mb(s.footprint)),
        })
        .to_string()
    }
}

pub fn mb(bytes: u64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0
}

pub fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// Starts the periodic sampler. Sampling is a single syscall, so it is cheap
/// enough to run every couple of seconds for the lifetime of the app.
pub fn start_sampler(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let me = sample_self();
            let ls_pid = cx.update(|cx| cx.global::<Perf>().language_server.map(|(pid, _)| pid));
            let ls = ls_pid.and_then(|pid| sample_language_server(pid).map(|s| (pid, s)));
            let log = cx.update(|cx| {
                let perf = cx.global_mut::<Perf>();
                perf.record(me);
                if ls_pid.is_some() {
                    perf.language_server = ls;
                }
                if perf.memory_warning() {
                    log::warn!("high memory: {}", perf.status_label());
                }
                perf.log_path.clone().map(|path| (path, perf.log_line()))
            });
            if let Some((path, line)) = log
                && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
            {
                writeln!(file, "{line}").ok();
            }
            cx.background_executor()
                .timer(Duration::from_secs(2))
                .await;
        }
    })
    .detach();
}

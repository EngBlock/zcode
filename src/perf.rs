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
    sample_language_servers(&[pid])
}

/// Every language server's memory together: theirs, their children's
/// (vtsls runs TypeScript's tsserver in processes of its own) and the
/// `SourceKitService` helpers zcode's sourcekit-lsp uses, but not the ones
/// of other apps (Xcode's, another editor's sourcekit-lsp's).
pub fn sample_language_servers(pids: &[i32]) -> Option<MemorySample> {
    let mut total: Option<MemorySample> = None;
    let mut add = |s: MemorySample| {
        let t = total.get_or_insert_with(MemorySample::default);
        t.footprint += s.footprint;
        t.resident += s.resident;
        t.lifetime_peak += s.lifetime_peak;
    };
    let mut seen = std::collections::HashSet::new();
    let mut queue: Vec<(i32, u8)> = pids.iter().map(|&pid| (pid, 0)).collect();
    while let Some((pid, depth)) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(s) = sample_pid(pid) {
            add(s);
        }
        if depth < 3 {
            queue.extend(child_pids(pid).into_iter().map(|child| (child, depth + 1)));
        }
    }
    if seen.iter().all(|&pid| sample_pid(pid).is_none()) {
        return None;
    }
    let clients: Vec<i32> = seen.iter().copied().filter(|&pid| process_path(pid).is_some_and(|p| p.ends_with("/sourcekit-lsp"))).collect();
    for helper in sourcekit_services_of(&clients) {
        if let Some(s) = sample_pid(helper) {
            add(s);
        }
    }
    total
}

/// Which of the machine's `SourceKitService` processes are `clients`'
/// (sourcekit-lsp processes): the ones launchd runs in their XPC domains.
///
/// sourcekitd, loaded into sourcekit-lsp, starts its framework's XPC
/// service, which launchd runs in the client's own domain (`pid/<client>`;
/// Xcode's runs in its SKAgent's). Neither the parent pid (launchd) nor the
/// responsible pid tells them apart: an app started from a terminal is
/// responsible to the terminal, and so are the helpers of every client it
/// starts, Xcode's included when Xcode was opened from the same terminal.
/// `launchctl print pid/<client>` lists the domain's services and their
/// pids. Asked again only when the set of `SourceKitService` processes or
/// of clients changes (one ~3 ms `launchctl` per client), so the 2 s
/// sampler doesn't spawn a process each time.
fn sourcekit_services_of(clients: &[i32]) -> Vec<i32> {
    struct Known {
        clients: Vec<i32>,
        running: Vec<i32>,
        ours: Vec<i32>,
    }
    static KNOWN: std::sync::Mutex<Option<Known>> = std::sync::Mutex::new(None);

    let mut clients = clients.to_vec();
    clients.sort_unstable();
    let mut running = pids_with_path_suffix("/SourceKitService");
    running.sort_unstable();
    let mut known = KNOWN.lock().unwrap();
    if let Some(k) = known.as_ref().filter(|k| k.clients == clients && k.running == running) {
        return k.ours.clone();
    }
    let ours: Vec<i32> = clients
        .iter()
        .flat_map(|&client| {
            let listing = std::process::Command::new("/bin/launchctl")
                .arg("print")
                .arg(format!("pid/{client}"))
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            xpc_domain_services(&listing)
        })
        .filter(|(pid, name)| name.starts_with("com.apple.SourceKitService") && running.contains(pid))
        .map(|(pid, _)| pid)
        .collect();
    *known = Some(Known { clients, running, ours: ours.clone() });
    ours
}

/// The running services in `launchctl print pid/<pid>`'s `services` block
/// (lines of pid, last exit status and label): their pids and labels.
fn xpc_domain_services(listing: &str) -> Vec<(i32, String)> {
    let mut services = vec![];
    let mut inside = false;
    for line in listing.lines() {
        if !inside {
            inside = line.trim() == "services = {";
            continue;
        }
        if line.trim() == "}" {
            break;
        }
        let mut fields = line.split_whitespace();
        if let (Some(pid), Some(_), Some(label)) = (fields.next(), fields.next(), fields.next())
            && let Ok(pid) = pid.parse::<i32>()
            && pid > 0
        {
            services.push((pid, label.to_string()));
        }
    }
    services
}

fn child_pids(pid: i32) -> Vec<i32> {
    let mut pids = vec![0i32; 256];
    let count = unsafe { libc::proc_listchildpids(pid, pids.as_mut_ptr() as *mut libc::c_void, (pids.len() * std::mem::size_of::<i32>()) as i32) };
    if count <= 0 {
        return vec![];
    }
    pids.truncate((count as usize).min(pids.len()));
    pids.retain(|&p| p > 0);
    pids
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

/// How long one of the operations that grow with a project took last
/// time (`Perf::note`): the file scan, the tree, a search, Open Quickly's
/// matching. Reported by the control channel's `dump` for the scale
/// benchmarks (`scripts/bench.py`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub ms: f64,
    /// What it covered: files scanned, matches found, candidates scored.
    pub count: usize,
    /// When it finished, in ms since `main()`.
    pub at_ms: f64,
}

/// App-wide performance state.
pub struct Perf {
    main_started: Instant,
    pub startup: StartupTimings,
    pub timings: std::collections::BTreeMap<&'static str, Timing>,
    pub history: VecDeque<MemorySample>,
    pub peak_footprint: u64,
    /// The language servers' footprint together (the first one's pid).
    pub language_server: Option<(i32, MemorySample)>,
    /// Every running language server's pid.
    pub language_server_pids: Vec<i32>,
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
            timings: Default::default(),
            history: VecDeque::with_capacity(HISTORY_LEN),
            peak_footprint: 0,
            language_server: None,
            language_server_pids: vec![],
            log_path: std::env::var_os("ZCODE_PERF_LOG").map(PathBuf::from),
        }
    }

    /// The language servers running now (none: no figure).
    pub fn set_language_servers(&mut self, pids: Vec<i32>) {
        if pids.is_empty() {
            self.language_server = None;
        }
        self.language_server_pids = pids;
    }

    pub fn main_started(&self) -> Instant {
        self.main_started
    }

    pub fn mark_window_opened(&mut self) {
        self.startup.main_to_window = Some(self.main_started.elapsed());
    }

    /// Records how long `key` took, replacing the last measurement.
    pub fn note(&mut self, key: &'static str, elapsed: Duration, count: usize) {
        let timing = Timing {
            ms: elapsed.as_secs_f64() * 1000.0,
            count,
            at_ms: self.main_started.elapsed().as_secs_f64() * 1000.0,
        };
        self.timings.insert(key, timing);
    }

    /// `note` through the app, from code that has a `cx`.
    pub fn record_timing(cx: &mut App, key: &'static str, elapsed: Duration, count: usize) {
        cx.global_mut::<Perf>().note(key, elapsed, count);
    }

    /// The timings as JSON: `{ "scan": { "ms": …, "count": …, "at_ms": … }, … }`.
    pub fn timings_json(&self) -> serde_json::Value {
        self.timings
            .iter()
            .map(|(key, t)| (key.to_string(), serde_json::json!({ "ms": t.ms, "count": t.count, "at_ms": t.at_ms })))
            .collect::<serde_json::Map<_, _>>()
            .into()
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

    /// The status bar's memory text: the idle baseline, not the last
    /// sample. A redraw adds compositor memory for a moment, so the last
    /// sample's figure flips after one, and showing it redrew the window
    /// for that, an idle window's only reason to draw.
    pub fn status_label(&self) -> String {
        let footprint = match self.baseline_footprint() {
            0 => self.current().footprint,
            baseline => baseline,
        };
        match self.language_server.map(|(_, s)| s.footprint).filter(|f| *f > 0) {
            Some(ls) => format!("{} · LSP {}", format_bytes(footprint), format_bytes(ls)),
            None => format_bytes(footprint),
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
            let pids = cx.update(|cx| cx.global::<Perf>().language_server_pids.clone());
            // Off the main thread: it lists the machine's processes, and
            // now and then asks launchd (`sourcekit_services_of`).
            let ls = if pids.is_empty() {
                None
            } else {
                let sampled = pids.clone();
                cx.background_executor().spawn(async move { sample_language_servers(&sampled) }).await.map(|s| (pids[0], s))
            };
            let log = cx.update(|cx| {
                let perf = cx.global_mut::<Perf>();
                perf.record(me);
                if !pids.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clients_xpc_services_are_read_from_its_launchd_domain() {
        // `launchctl print pid/<sourcekit-lsp>` (macOS 27), trimmed: the
        // services block, then others whose lines look alike.
        let listing = "pid/4242 = {\n\ttype = pid\n\tservices = {\n\t\t    5564      - \tcom.apple.SourceKitService.6.4.0.33.1_default\n\t\t       0      - \tcom.apple.MTLCompilerService\n\t}\n\n\tservice stubs = {\n\t\tcom.apple.siri.media-indexer\n\t}\n\tendpoints = {\n\t\t    7777      - \tcom.apple.SourceKitService.other\n\t}\n}\n";
        assert_eq!(xpc_domain_services(listing), vec![(5564, "com.apple.SourceKitService.6.4.0.33.1_default".to_string())]);
        assert!(xpc_domain_services("").is_empty());
    }

    #[test]
    fn timings_keep_the_last_measurement_per_key() {
        let mut perf = Perf::new(Instant::now());
        perf.note("scan", Duration::from_millis(120), 10_000);
        perf.note("scan", Duration::from_micros(2500), 10_001);
        perf.note("find", Duration::from_millis(3), 250);
        let json = perf.timings_json();
        assert_eq!(json["scan"]["ms"], 2.5);
        assert_eq!(json["scan"]["count"], 10_001);
        assert_eq!(json["find"]["count"], 250);
        assert!(json["find"]["at_ms"].as_f64().unwrap() >= 0.0);
        assert!(json.get("tree").is_none());
    }
}

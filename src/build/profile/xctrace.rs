//! `xctrace`, Instruments' command line: its templates, a recording's
//! command, and opening the trace in Instruments. Blocking functions; call
//! them from background tasks.
//!
//! `ZCODE_XCTRACE` names a program to run instead of `xcrun xctrace`, and
//! `ZCODE_OPEN_TRACE_LOG` a file a finished trace's path is appended to
//! instead of opening Instruments (the e2e tests use both).

use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};

use crate::scheme::Launch;

pub fn xctrace() -> Command {
    match std::env::var_os("ZCODE_XCTRACE") {
        Some(program) => Command::new(program),
        None => {
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.arg("xctrace");
            cmd
        }
    }
}

/// A recording template, as `xctrace list templates` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub name: String,
    /// "Standard", "User"…
    pub section: String,
}

/// The templates Instruments offers first, in its chooser's order.
const COMMON: &[&str] = &["Time Profiler", "Allocations", "Leaks", "Animation Hitches", "SwiftUI", "App Launch"];

/// `xctrace list templates`: `== Standard Templates ==` and the names under
/// it, then the user's own. The common ones come first.
pub fn parse_templates(text: &str) -> Vec<Template> {
    let mut section = String::new();
    let mut templates = vec![];
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(title) = line.strip_prefix("== ").and_then(|l| l.strip_suffix(" ==")) {
            section = title.trim_end_matches(" Templates").trim_end_matches(" templates").to_string();
        } else {
            templates.push(Template { name: line.to_string(), section: section.clone() });
        }
    }
    let rank = |t: &Template| COMMON.iter().position(|c| *c == t.name).unwrap_or(COMMON.len());
    // Stable: the rest keep xctrace's order.
    templates.sort_by_key(rank);
    templates
}

pub fn list_templates() -> Result<Vec<Template>> {
    let output = xctrace()
        .args(["list", "templates"])
        .stdin(Stdio::null())
        .output()
        .context("running xctrace")?;
    if !output.status.success() {
        bail!("xctrace list templates: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let templates = parse_templates(&String::from_utf8_lossy(&output.stdout));
    if templates.is_empty() {
        bail!("xctrace lists no templates");
    }
    Ok(templates)
}

/// The template a scheme's Profile action names (`savedToolIdentifier`):
/// a template's name, or a `.tracetemplate`'s path (its name when the file
/// isn't there). `None` is Ask on Launch.
pub fn scheme_template(identifier: &str) -> Option<String> {
    let identifier = identifier.trim();
    if identifier.is_empty() {
        return None;
    }
    let path = Path::new(identifier);
    if path.extension() == Some(OsStr::new("tracetemplate")) && !path.exists() {
        return path.file_stem().map(|s| s.to_string_lossy().to_string());
    }
    Some(identifier.to_string())
}

/// Where a recording is written: `<Scheme> <Template> <time>.trace` in `dir`.
pub fn trace_path(dir: &Path, scheme: &str, template: &str) -> PathBuf {
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let path = Path::new(template);
    let name = match path.extension() == Some(OsStr::new("tracetemplate")) {
        true => path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        false => template.to_string(),
    };
    let clean = |s: &str| s.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect::<String>();
    dir.join(format!("{} {} {time}.trace", clean(scheme), clean(&name)))
}

/// What to record.
pub struct Recording<'a> {
    pub template: &'a str,
    /// A simulator or device (`None`: this Mac).
    pub device: Option<&'a str>,
    pub output: &'a Path,
    /// Seconds; `None` records until Stop.
    pub time_limit: Option<u32>,
    pub launch: &'a Launch,
    /// The app bundle or executable to launch (a device's app: its bundle
    /// identifier).
    pub target: &'a OsStr,
    /// The program's stdout in xctrace's own (the console), on the Mac.
    pub forward_output: bool,
}

/// `xctrace record --launch`: launches `target` with the scheme's arguments
/// and environment and records it until the time limit or SIGINT (Stop),
/// when it saves the trace.
pub fn record_command(recording: &Recording) -> Command {
    let mut cmd = xctrace();
    cmd.args(["record", "--template", recording.template, "--no-prompt"]).arg("--output").arg(recording.output);
    if let Some(device) = recording.device {
        cmd.args(["--device", device]);
    }
    if let Some(limit) = recording.time_limit.filter(|l| *l > 0) {
        cmd.args(["--time-limit", &format!("{limit}s")]);
    }
    if recording.forward_output {
        cmd.args(["--target-stdout", "-"]);
    }
    for (key, value) in &recording.launch.environment {
        cmd.arg("--env").arg(format!("{key}={value}"));
    }
    cmd.args(["--launch", "--"]).arg(recording.target).args(&recording.launch.arguments);
    if let Some(dir) = &recording.launch.working_directory {
        cmd.current_dir(dir);
    }
    // zcode's threads block SIGINT, and a child inherits that: unblocked,
    // Stop's SIGINT reaches xctrace, which then saves the trace.
    unsafe {
        cmd.pre_exec(|| {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            Ok(())
        });
    }
    cmd
}

/// The active Xcode's Instruments.
pub fn instruments_app() -> Option<PathBuf> {
    let app = crate::toolchain::developer_dir()?.parent()?.join("Applications/Instruments.app");
    app.exists().then_some(app)
}

/// Opens a trace in Instruments (or notes its path in
/// `$ZCODE_OPEN_TRACE_LOG`).
pub fn open_trace(trace: &Path) -> Result<()> {
    if let Some(log) = std::env::var_os("ZCODE_OPEN_TRACE_LOG") {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        writeln!(file, "{}", trace.display())?;
        return Ok(());
    }
    let mut open = Command::new("/usr/bin/open");
    match instruments_app() {
        Some(app) => open.arg("-a").arg(app),
        None => open.args(["-b", "com.apple.dt.Instruments"]),
    };
    let output = open.arg(trace).stdin(Stdio::null()).output().context("running open")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// Traces zcode keeps; older ones are deleted when a recording starts.
const KEEP: usize = 20;

/// Deletes all but the newest traces in `dir`.
pub fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut traces: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension() == Some(OsStr::new("trace")))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH), p))
        .collect();
    traces.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in traces.into_iter().skip(KEEP) {
        if path.is_dir() {
            std::fs::remove_dir_all(&path).ok();
        } else {
            std::fs::remove_file(&path).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn templates_in_xctraces_listing() {
        let text = "== Standard Templates ==\nActivity Monitor\nAllocations\nAnimation Hitches\nLeaks\nSwiftUI\nSystem Trace\nTime Profiler\n\n== User Templates ==\nMy Hitches\n";
        let templates = parse_templates(text);
        let names: Vec<_> = templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Time Profiler", "Allocations", "Leaks", "Animation Hitches", "SwiftUI", "Activity Monitor", "System Trace", "My Hitches"]);
        assert_eq!(templates[0].section, "Standard");
        assert_eq!(templates.last().unwrap().section, "User");
    }

    #[test]
    fn a_schemes_saved_instrument() {
        assert_eq!(scheme_template(""), None);
        assert_eq!(scheme_template("Time Profiler").as_deref(), Some("Time Profiler"));
        assert_eq!(
            scheme_template("/Missing/Xcode.app/Contents/Applications/Instruments.app/Contents/Resources/templates/Leaks.tracetemplate").as_deref(),
            Some("Leaks")
        );
    }

    #[test]
    fn a_recordings_command() {
        let launch = Launch {
            arguments: vec!["-verbose".into(), "two words".into()],
            environment: vec![("GREETING".into(), "hi there".into())],
            working_directory: None,
        };
        let output = Path::new("/tmp/HelloClassic Time Profiler 1.trace");
        let mac = record_command(&Recording {
            template: "Time Profiler",
            device: None,
            output,
            time_limit: Some(5),
            launch: &launch,
            target: OsStr::new("/Build/HelloClassic.app"),
            forward_output: true,
        });
        assert_eq!(
            args(&mac).iter().skip_while(|a| *a != "record").collect::<Vec<_>>(),
            [
                "record", "--template", "Time Profiler", "--no-prompt", "--output", "/tmp/HelloClassic Time Profiler 1.trace",
                "--time-limit", "5s", "--target-stdout", "-", "--env", "GREETING=hi there", "--launch", "--",
                "/Build/HelloClassic.app", "-verbose", "two words",
            ]
        );
        let simulator = record_command(&Recording {
            template: "Leaks",
            device: Some("949CEA5F"),
            output,
            time_limit: None,
            launch: &Launch::default(),
            target: OsStr::new("/Build/HelloZ.app"),
            forward_output: false,
        });
        let simulator = args(&simulator);
        assert!(simulator.windows(2).any(|w| w == ["--device", "949CEA5F"]), "{simulator:?}");
        assert!(!simulator.iter().any(|a| a == "--time-limit" || a == "--target-stdout"), "{simulator:?}");
    }

    #[test]
    fn traces_are_named_for_the_scheme_and_template() {
        let path = trace_path(Path::new("/t"), "Hello/Z", "/x/Time Profiler.tracetemplate");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("Hello-Z Time Profiler ") && name.ends_with(".trace"), "{name}");
    }
}

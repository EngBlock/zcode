//! Physical devices through `devicectl` (CoreDevice): discovery, install,
//! lock state, launching with a console, and finding and terminating the
//! app's process. devicectl's JSON output (`--json-output`) is its stable
//! interface; its human-readable output is not. All functions block; call
//! them from background tasks.
//!
//! `ZCODE_DEVICECTL` names a program to run instead of `xcrun devicectl`
//! (the e2e tests use a fake one).

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::xcode::{Destination, Device, Platform, Transport};
use crate::scheme::Launch;

pub fn devicectl() -> Command {
    match std::env::var_os("ZCODE_DEVICECTL") {
        Some(program) => Command::new(program),
        None => {
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.arg("devicectl");
            cmd
        }
    }
}

/// A fresh path for one command's `--json-output`.
pub fn json_output_path() -> std::path::PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "zcode-devicectl-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Runs `devicectl <args>` and returns its JSON result (`result`), or the
/// error devicectl reported.
fn run(args: &[&str]) -> Result<Value> {
    let path = json_output_path();
    let output = devicectl()
        .args(args)
        .arg("--quiet")
        .arg("--json-output")
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .context("running devicectl")?;
    let json = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
    std::fs::remove_file(&path).ok();
    let json = match json {
        Some(json) => json,
        None if output.status.success() => bail!("devicectl wrote no result"),
        None => bail!("{}", last_error_line(&String::from_utf8_lossy(&output.stderr))),
    };
    if let Some(message) = error_message(&json) {
        bail!("{message}");
    }
    if !output.status.success() {
        bail!("{}", last_error_line(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(json.get("result").cloned().unwrap_or(Value::Null))
}

fn last_error_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| stderr.lines().map(str::trim).rfind(|l| !l.is_empty()))
        .map(|l| l.trim_start_matches("ERROR:").trim().to_string())
        .unwrap_or_else(|| "devicectl failed".into())
}

/// The error in a devicectl JSON result: its description, then the
/// innermost failure reason ("…because the device was not, or could not
/// be, unlocked").
pub fn error_message(json: &Value) -> Option<String> {
    let error = json.get("error")?;
    let string = |info: &Value, key: &str| info.get(key)?.get("string")?.as_str().map(str::to_string);
    let description = string(&error["userInfo"], "NSLocalizedDescription");
    let mut reason = None;
    let mut current = error;
    loop {
        let info = &current["userInfo"];
        if let Some(r) = string(info, "NSLocalizedFailureReason").or_else(|| string(info, "NSLocalizedRecoverySuggestion")) {
            reason = Some(r);
        }
        match info.get("NSUnderlyingError").and_then(|e| e.get("error")) {
            Some(next) => current = next,
            None => break,
        }
    }
    match (description, reason) {
        (Some(d), Some(r)) if d != r => Some(format!("{d} {r}")),
        (Some(d), _) => Some(d),
        (None, Some(r)) => Some(r),
        (None, None) => Some(format!(
            "devicectl error {}",
            error.get("code").and_then(Value::as_i64).unwrap_or_default()
        )),
    }
}

/// Physical devices CoreDevice knows: per platform (iOS first), usable ones
/// first, then by name.
pub fn list() -> Result<Vec<Destination>> {
    Ok(parse_devices(&run(&["list", "devices"])?))
}

/// The physical devices in `devicectl list devices`' result. Reads the
/// `properties` dictionary, falling back to the fields it replaced
/// (`hardwareProperties`, `deviceProperties`, `connectionProperties`) that
/// older devicectl versions write.
pub fn parse_devices(result: &Value) -> Vec<Destination> {
    let mut out: Vec<Destination> = result
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_device)
        .collect();
    out.sort_by(|a, b| {
        Platform::DEVICES
            .iter()
            .position(|p| *p == a.platform)
            .cmp(&Platform::DEVICES.iter().position(|p| *p == b.platform))
            .then(b.booted.cmp(&a.booted))
            .then(a.name.cmp(&b.name))
    });
    out
}

fn parse_device(device: &Value) -> Option<Destination> {
    let properties = &device["properties"];
    // A field of the `properties` dictionary, else of the deprecated one.
    let field = |section: &str, key: &str, legacy_section: &str, legacy_key: &str| -> Option<&Value> {
        properties[section].get(key).or_else(|| device[legacy_section].get(legacy_key))
    };
    let text = |section: &str, key: &str, legacy_section: &str| -> Option<String> {
        field(section, key, legacy_section, key).and_then(Value::as_str).map(str::to_string)
    };
    if text("hardware", "reality", "hardwareProperties").as_deref() != Some("physical") {
        return None;
    }
    let platform = match text("hardware", "platform", "hardwareProperties")?.as_str() {
        "iOS" | "iPadOS" => Platform::IOS,
        "watchOS" => Platform::WatchOS,
        "tvOS" => Platform::TvOS,
        "xrOS" | "visionOS" => Platform::VisionOS,
        _ => return None,
    };
    let identifier = device.get("identifier").and_then(Value::as_str)?.to_string();
    let udid = text("hardware", "udid", "hardwareProperties").unwrap_or_else(|| identifier.clone());
    let product_type = text("hardware", "productType", "hardwareProperties").unwrap_or_default();
    let model = text("hardware", "marketingName", "hardwareProperties").unwrap_or_else(|| product_type.clone());
    let os_build = properties["software"]["osBuildVersions"]["buildVersion"]
        .get("name")
        .or_else(|| device["deviceProperties"].get("osBuildUpdate"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = text("state", "name", "deviceProperties").unwrap_or_else(|| model.clone());
    let version = properties["software"]["osVersionNumber"]
        .get("stringValue")
        .or_else(|| device["deviceProperties"].get("osVersionNumber"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let connection = |key: &str| text("connection", key, "connectionProperties");
    let transport = match connection("transportType").as_deref() {
        Some("wired") => Some(Transport::Wired),
        Some("sameMachine") | None => None,
        Some(_) => Some(Transport::Network),
    };
    let connected = properties["connection"].get("state").and_then(Value::as_str) == Some("connected")
        || device["connectionProperties"].get("tunnelState").and_then(Value::as_str) == Some("connected");
    // `{"enabled": {"mode": 1}}` in `properties`, a plain string before.
    let developer_mode = field("state", "developerModeStatus", "deviceProperties", "developerModeStatus").and_then(|status| {
        let word = match status {
            Value::String(s) => s.as_str(),
            Value::Object(o) => o.keys().next()?.as_str(),
            _ => return None,
        };
        Some(word == "enabled")
    });
    let booted = text("state", "bootState", "deviceProperties").is_none_or(|s| s == "booted");
    let info = Device {
        identifier,
        model,
        product_type,
        os_build,
        transport,
        connected,
        paired: connection("pairingState").as_deref() == Some("paired"),
        developer_mode,
        booted,
    };
    Some(Destination {
        id: udid,
        name,
        platform,
        runtime: format!("{} {version}", platform.os_name()).trim_end().to_string(),
        booted: info.unavailable_reason().is_none(),
        device: Some(info),
    })
}

/// Why Run can't use a device, said the way Xcode says it.
pub fn unavailable_message(device: &Destination) -> Option<String> {
    let name = &device.name;
    Some(match device.device.as_ref()?.unavailable_reason()? {
        "Not paired" => format!("{name} is not paired with this Mac. Connect it with a cable, unlock it and tap Trust."),
        "Developer Mode disabled" => format!(
            "Developer Mode is disabled on {name}. Turn it on in Settings ▸ Privacy & Security ▸ Developer Mode, then restart the device."
        ),
        "Not booted" => format!("{name} is starting up. Wait for it to finish, then try again."),
        _ => format!("{name} is not connected. Connect it with a cable, or to the same network as this Mac."),
    })
}

/// An installed app: its bundle id and where it is on the device.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub bundle_id: String,
    /// e.g. `/private/var/containers/Bundle/Application/<UUID>/App.app`.
    pub path: String,
}

pub fn install(udid: &str, app: &Path) -> Result<Installed> {
    let app = app.to_string_lossy();
    parse_install(&run(&["device", "install", "app", "--device", udid, &app])?)
        .ok_or_else(|| anyhow::anyhow!("devicectl didn't report the installed app"))
}

pub fn parse_install(result: &Value) -> Option<Installed> {
    let app = result.get("installedApplications")?.as_array()?.first()?;
    Some(Installed {
        bundle_id: app.get("bundleID")?.as_str()?.to_string(),
        path: file_url_path(app.get("installationURL")?.as_str()?),
    })
}

/// `file:///private/var/…/App.app/` → `/private/var/…/App.app`.
fn file_url_path(url: &str) -> String {
    let path = url.strip_prefix("file://").unwrap_or(url);
    let path = path.trim_end_matches('/');
    percent_decode(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether the device is locked with a passcode (apps can't launch then).
pub fn is_locked(udid: &str) -> Result<bool> {
    Ok(parse_lock_state(&run(&["device", "info", "lockState", "--device", udid])?))
}

pub fn parse_lock_state(result: &Value) -> bool {
    result.get("passcodeRequired").and_then(Value::as_bool).unwrap_or(false)
}

/// `devicectl device process launch --console`: runs the app with the
/// scheme's arguments and environment and streams its stdout/stderr until
/// it exits. devicectl forwards signals it gets (Stop's SIGTERM) to the app.
/// `start_stopped` keeps it suspended until a debugger attaches.
pub fn launch_command(udid: &str, bundle_id: &str, launch: &Launch, start_stopped: bool, json_output: &Path) -> Command {
    let mut cmd = devicectl();
    cmd.args(["device", "process", "launch", "--device", udid, "--console", "--terminate-existing"]);
    if start_stopped {
        cmd.arg("--start-stopped");
    }
    cmd.arg("--json-output").arg(json_output).arg(bundle_id).args(&launch.arguments);
    // devicectl passes DEVICECTL_CHILD_-prefixed variables to the app.
    cmd.envs(launch.environment.iter().map(|(k, v)| (format!("DEVICECTL_CHILD_{k}"), v.clone())));
    cmd
}

/// The process running the app installed at `app` (its executable is in
/// the bundle).
pub fn find_process(udid: &str, app: &str) -> Result<Option<i32>> {
    Ok(parse_process(&run(&["device", "info", "processes", "--device", udid])?, app))
}

pub fn parse_process(result: &Value, app: &str) -> Option<i32> {
    let prefix = format!("{}/", app.trim_end_matches('/'));
    result
        .get("runningProcesses")?
        .as_array()?
        .iter()
        .find(|p| {
            p.get("executable")
                .and_then(Value::as_str)
                .is_some_and(|url| file_url_path(url).starts_with(&prefix))
        })
        .and_then(|p| p.get("processIdentifier")?.as_i64())
        .map(|pid| pid as i32)
}

/// Looks for a just-launched app's process: in the bundle, and not the
/// instance that was running before (which the launch terminates).
#[derive(Clone, Debug)]
pub struct ProcessWatch {
    pub udid: String,
    pub installed: String,
    pub stale_pid: Option<i32>,
}

impl ProcessWatch {
    pub fn find(&self) -> Result<Option<i32>> {
        Ok(find_process(&self.udid, &self.installed)?.filter(|pid| Some(*pid) != self.stale_pid))
    }
}

/// How a `--console` launch ended, from its JSON result.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    Exited(i32),
    Signaled(i32),
    Terminated,
    Failed(String),
}

pub fn read_launch_result(path: &Path) -> Ended {
    let json = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
    std::fs::remove_file(path).ok();
    match json {
        Some(json) => parse_launch_result(&json),
        None => Ended::Terminated,
    }
}

pub fn parse_launch_result(json: &Value) -> Ended {
    if let Some(message) = error_message(json) {
        return Ended::Failed(message);
    }
    let termination = &json["result"]["terminationResult"];
    if let Some(code) = termination.get("exitCode").and_then(Value::as_i64) {
        Ended::Exited(code as i32)
    } else if let Some(signal) = termination.get("terminatingSignal").and_then(Value::as_i64) {
        Ended::Signaled(signal as i32)
    } else {
        Ended::Terminated
    }
}

pub fn terminate(udid: &str, pid: i32) -> Result<()> {
    run(&["device", "process", "terminate", "--device", udid, "--pid", &pid.to_string()]).map(|_| ())
}

/// devicectl's own status lines in a `--console` launch, which Xcode's
/// console doesn't show.
pub fn is_console_chatter(line: &str) -> bool {
    line.starts_with("Launched application with ") || line.starts_with("Waiting for the application to terminate")
}

/// Whether Xcode has copied the device's system libraries for its OS build
/// to this Mac (`~/Library/Developer/Xcode/iOS DeviceSupport/<model> <version>
/// (<build>)`). Without them lldb reads every system library from the
/// device's memory, which over a network connection takes minutes.
pub fn has_cached_symbols(destination: &Destination) -> bool {
    let Some(device) = &destination.device else { return false };
    let Some(home) = std::env::var_os("HOME") else { return false };
    let folder = Path::new(&home)
        .join("Library/Developer/Xcode")
        .join(format!("{} DeviceSupport", destination.platform.os_name()));
    let suffix = format!("({})", device.os_build);
    !device.os_build.is_empty()
        && std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
}

/// The LLDB commands lldb-dap runs to attach to a device process. lldb's
/// `device` commands attach through CoreDevice, as Xcode does.
/// `device process attach` makes its own target, so the app's local build
/// is found through `target.exec-search-paths`, and the bundle's own
/// libraries (the debug dylib, embedded frameworks) are added to the target
/// once attached (`post_attach_commands`) so breakpoints in them resolve
/// before they load. Without the device's cached system symbols, lldb loads
/// only sections and function bounds of system libraries (accurate stacks,
/// no system symbol names) rather than downloading every symbol table.
pub fn attach_commands(identifier: &str, pid: i32, app: &Path, cached_symbols: bool) -> Vec<String> {
    let mut commands = vec![];
    if !cached_symbols {
        commands.push("settings set target.memory-module-load-level partial".to_string());
    }
    commands.extend([
        format!("settings set target.exec-search-paths {}", quoted(app)),
        format!("device select {identifier}"),
        format!("device process attach --pid {pid}"),
    ]);
    commands
}

/// What the debugger needs to attach to an app on a device.
#[derive(Clone, Debug, PartialEq)]
pub struct Attach {
    /// The device's CoreDevice identifier.
    pub identifier: String,
    pub pid: i32,
    /// The app's local build.
    pub app: std::path::PathBuf,
    /// See `has_cached_symbols`.
    pub cached_symbols: bool,
}

impl Attach {
    /// lldb-dap's `attach` arguments.
    pub fn dap_arguments(&self) -> Value {
        serde_json::json!({
            "attachCommands": attach_commands(&self.identifier, self.pid, &self.app, self.cached_symbols),
            "postRunCommands": post_attach_commands(&self.app),
            // Attaching over the network takes a while.
            "timeout": 120,
        })
    }
}

/// What lldb-dap runs once attached: add the bundle's libraries.
pub fn post_attach_commands(app: &Path) -> Vec<String> {
    bundle_libraries(app)
        .iter()
        .map(|library| format!("target modules add {}", quoted(library)))
        .collect()
}

/// The Mach-O libraries a built app bundle carries: its debug dylib
/// (`ENABLE_DEBUG_DYLIB`, Xcode's default for Debug builds) and embedded
/// frameworks and dylibs.
pub fn bundle_libraries(app: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(app) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "dylib") && !path.file_name().is_some_and(|n| n == "__preview.dylib") {
            out.push(path);
        }
    }
    if let Ok(frameworks) = std::fs::read_dir(app.join("Frameworks")) {
        for entry in frameworks.flatten() {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("dylib") => out.push(path),
                Some("framework") => {
                    let binary = path.join(path.file_stem().unwrap_or_default());
                    if binary.exists() {
                        out.push(binary);
                    }
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devicectl").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn lists_physical_devices_with_their_state() {
        let devices = parse_devices(&fixture("list-devices.json")["result"]);
        let summary: Vec<String> = devices
            .iter()
            .map(|d| format!("{} | {} | {} | {}", d.name, d.runtime, d.device.as_ref().unwrap().status(), d.id))
            .collect();
        assert_eq!(
            summary,
            [
                "Test iPhone | iOS 27.2 | Connected via network | 00008150-0000000000000001",
                "Test iPhone (USB) | iOS 27.2 | Connected via USB | 00008150-0000000000000004",
                "Old iPhone | iOS 18.6 | Unavailable | 00008030-0000000000000005",
                "Test iPad | iOS 26.4 | Developer Mode disabled | 00008112-0000000000000002",
                "Test Watch | watchOS 27.0 | Not paired | 00008310-0000000000000003",
                "Living Room | tvOS 27.0 | Connected via network | 00008110-0000000000000006",
                "Test Vision Pro | visionOS 27.0 | Connected via network | 00008112-0000000000000007",
            ]
        );
        // Simulators devicectl also lists aren't devices.
        assert!(devices.iter().all(|d| d.platform.is_device()));
        let iphone = devices.iter().find(|d| d.name == "Test iPhone").unwrap();
        let info = iphone.device.as_ref().unwrap();
        assert_eq!(info.identifier, "FAD3EBA8-0000-0000-0000-000000000001");
        assert_eq!(info.model, "iPhone 17 Pro");
        assert_eq!((info.product_type.as_str(), info.os_build.as_str()), ("iPhone18,1", "24B5084k"));
        assert!(iphone.booted && info.connected);
        assert!(!devices.iter().find(|d| d.name == "Old iPhone").unwrap().booted);
    }

    #[test]
    fn reads_the_fields_older_devicectl_writes() {
        let devices = parse_devices(&fixture("list-devices-legacy.json")["result"]);
        assert_eq!(devices.len(), 2);
        let wired = &devices[0];
        assert_eq!((wired.name.as_str(), wired.runtime.as_str()), ("Legacy iPhone", "iOS 17.5"));
        let info = wired.device.as_ref().unwrap();
        assert_eq!(info.transport, Some(Transport::Wired));
        assert!(info.connected && info.paired && info.developer_mode == Some(true));
        assert_eq!(devices[1].device.as_ref().unwrap().status(), "Developer Mode disabled");
    }

    #[test]
    fn install_lock_and_processes() {
        let installed = parse_install(&fixture("install.json")["result"]).unwrap();
        assert_eq!(installed.bundle_id, "dev.zcode.HelloZ");
        assert_eq!(installed.path, "/private/var/containers/Bundle/Application/431CB3DB-0000-0000-0000-000000000000/HelloZ.app");
        assert!(parse_lock_state(&fixture("lock-state.json")["result"]));
        let processes = &fixture("processes.json")["result"];
        assert_eq!(parse_process(processes, &installed.path), Some(90947));
        assert_eq!(parse_process(processes, "/private/var/containers/Bundle/Application/X/Other.app"), None);
        // Not a prefix match on a sibling bundle name.
        assert_eq!(parse_process(processes, "/private/var/containers/Bundle/Application/431CB3DB-0000-0000-0000-000000000000/Hello"), None);
    }

    #[test]
    fn errors_say_why() {
        assert_eq!(
            error_message(&fixture("launch-locked.json")).unwrap(),
            "The application failed to launch. Unable to launch dev.zcode.HelloZ because the device was not, or could not be, unlocked."
        );
        assert_eq!(error_message(&fixture("install.json")), None);
        assert_eq!(last_error_line("ERROR: Device not found (com.apple.dt.CoreDeviceError error 1000)\n  more"), "Device not found (com.apple.dt.CoreDeviceError error 1000)");
    }

    #[test]
    fn launch_results() {
        assert_eq!(parse_launch_result(&fixture("launch-console.json")), Ended::Signaled(15));
        assert!(matches!(parse_launch_result(&fixture("launch-locked.json")), Ended::Failed(m) if m.contains("unlocked")));
        let exited = serde_json::json!({ "info": { "outcome": "success" }, "result": { "terminationResult": { "exitCode": 3 } } });
        assert_eq!(parse_launch_result(&exited), Ended::Exited(3));
    }

    #[test]
    fn unavailable_devices_say_why() {
        let devices = parse_devices(&fixture("list-devices.json")["result"]);
        let message = |name: &str| unavailable_message(devices.iter().find(|d| d.name == name).unwrap());
        assert!(message("Test iPad").unwrap().starts_with("Developer Mode is disabled on Test iPad."));
        assert!(message("Test Watch").unwrap().contains("not paired"));
        assert!(message("Old iPhone").unwrap().contains("not connected"));
        assert_eq!(message("Test iPhone"), None);
    }

    #[test]
    fn launch_passes_arguments_and_environment() {
        let launch = Launch {
            arguments: vec!["-flag".into(), "two words".into()],
            environment: vec![("HELLOZ_MODE".into(), "device".into())],
            working_directory: None,
        };
        let cmd = launch_command("UDID", "dev.zcode.HelloZ", &launch, true, Path::new("/tmp/out.json"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        let tail = ["--start-stopped", "--json-output", "/tmp/out.json", "dev.zcode.HelloZ", "-flag", "two words"];
        assert!(args.ends_with(&tail.map(String::from)), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--device", "UDID"]));
        let env: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string()))).collect();
        assert!(env.contains(&("DEVICECTL_CHILD_HELLOZ_MODE".into(), Some("device".into()))));
    }

    #[test]
    fn attach_finds_the_local_build() {
        let dir = std::env::temp_dir().join(format!("zcode-devices-test-{}", std::process::id()));
        let app = dir.join("My App.app");
        std::fs::create_dir_all(app.join("Frameworks/Kit.framework")).unwrap();
        for file in ["My App", "My App.debug.dylib", "__preview.dylib", "Frameworks/Kit.framework/Kit", "Frameworks/libz2.dylib"] {
            std::fs::write(app.join(file), "").unwrap();
        }
        let commands = attach_commands("CORE-ID", 42, &app, true);
        assert_eq!(commands[0], format!("settings set target.exec-search-paths \"{}\"", app.display()));
        assert_eq!(commands[1..], ["device select CORE-ID".to_string(), "device process attach --pid 42".to_string()]);
        assert_eq!(attach_commands("CORE-ID", 42, &app, false)[0], "settings set target.memory-module-load-level partial");
        let libraries: Vec<String> = bundle_libraries(&app)
            .iter()
            .map(|p| p.strip_prefix(&app).unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(libraries, ["Frameworks/Kit.framework/Kit", "Frameworks/libz2.dylib", "My App.debug.dylib"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(file_url_path("file:///private/var/A%20B/App.app/"), "/private/var/A B/App.app");
        assert!(is_console_chatter("Launched application with dev.zcode.HelloZ bundle identifier."));
        assert!(!is_console_chatter("HelloZ launched"));
    }
}

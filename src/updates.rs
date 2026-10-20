//! Updates: zcode ▸ Check for Updates… and, with `auto_update` on in
//! settings.json (it is by default), a check in the background once a
//! day. The feed is a JSON file `scripts/package.py appcast` writes next
//! to each release (`version`, `url` of the zip, `sha256`, `notes`,
//! `minimum_system`); its URL is baked in from `ZCODE_UPDATE_FEED` at
//! build time, and `ZCODE_UPDATE_FEED` at run time names another (the e2e
//! suite's local one). Nothing happens at startup: the first check runs a
//! minute after launch, on a thread of its own, and not again within a
//! day of the last (its time is kept in the support folder). A scripted
//! zcode (`ZCODE_CONFIG_DIR`) checks on its own only when
//! `ZCODE_UPDATE_FEED` names a feed, so tests never reach a real server.
//!
//! A newer version is downloaded to the cache folder, its SHA-256 checked
//! against the feed's, unpacked with `ditto` next to the running bundle,
//! checked with `codesign` (a valid signature from the same team as the
//! running app's; ad hoc only when the running app's is too, as a local
//! build's) and swapped into the bundle's place, the way Sparkle and Zed
//! install updates. A notification then offers Restart Now
//! (`crate::bundle::relaunch_after_exit`). A bare binary (no bundle)
//! only reports the version; Download opens the zip's URL. No framework:
//! Sparkle would mean an Objective-C framework, XPC services and EdDSA
//! keys in the bundle for what `ureq`, `ditto` and `codesign` do here.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use serde::Deserialize;

/// Where releases are announced, unless `ZCODE_UPDATE_FEED` says otherwise.
pub const DEFAULT_FEED: &str = match option_env!("ZCODE_UPDATE_FEED") {
    Some(url) => url,
    None => "https://github.com/EngBlock/zcode/releases/latest/download/appcast.json",
};

/// The environment variable that names the feed at run time.
pub const FEED_ENV: &str = "ZCODE_UPDATE_FEED";

/// Between automatic checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// After launch, before the first automatic check.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// The feed can't be bigger than this, nor a download than `MAX_DOWNLOAD`.
const MAX_FEED: u64 = 1024 * 1024;
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;

pub fn feed_url() -> String {
    std::env::var(FEED_ENV).ok().filter(|url| !url.is_empty()).unwrap_or_else(|| DEFAULT_FEED.to_string())
}

/// One release, as the feed describes it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub minimum_system: String,
}

/// Where the updater is.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    /// Newer, and this zcode can't install it (no bundle, or no write
    /// access to its folder): Download opens the URL.
    Available(Release, String),
    Downloading(Release),
    Installing(Release),
    /// Installed in the bundle's place; Restart Now runs it.
    Ready(Release),
    Failed(String),
}

impl State {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::UpToDate => "up to date",
            Self::Available(..) => "available",
            Self::Downloading(_) => "downloading",
            Self::Installing(_) => "installing",
            Self::Ready(_) => "ready",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Default)]
pub struct Updates {
    pub state: Option<State>,
    /// Whether the check in progress was asked for from the menu: its
    /// result is always shown, an automatic one's only when there's
    /// something to install.
    manual: bool,
}
impl Global for Updates {}

fn state(cx: &App) -> State {
    cx.try_global::<Updates>().and_then(|u| u.state.clone()).unwrap_or(State::Idle)
}

/// Timestamps of the checks, kept across launches.
fn stamp_path() -> PathBuf {
    crate::paths::support_dir().join("updates.json")
}

fn last_check() -> Option<SystemTime> {
    let text = std::fs::read_to_string(stamp_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let secs = value.get("last_check")?.as_u64()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

fn stamp_check() {
    let path = stamp_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    std::fs::write(path, serde_json::json!({ "last_check": secs }).to_string()).ok();
}

/// Starts the daily checks (after a minute, then every day), when
/// `auto_update` is on; the setting is read at each turn so a change in
/// settings.json takes effect without a relaunch.
pub fn start_automatic_checks(cx: &mut App) {
    if crate::paths::has_config_override() && std::env::var_os(FEED_ENV).is_none_or(|v| v.is_empty()) {
        return;
    }
    cx.spawn(async move |cx| {
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            cx.background_executor().timer(delay).await;
            delay = CHECK_INTERVAL;
            let due = last_check().and_then(|t| t.elapsed().ok()).is_none_or(|since| since >= CHECK_INTERVAL);
            cx.update(|cx| {
                if due && crate::settings::Settings::get(cx).auto_update && state(cx) == State::Idle {
                    check(false, cx);
                }
            });
        }
    })
    .detach();
}

/// zcode ▸ Check for Updates… (`manual`), or the daily check.
pub fn check(manual: bool, cx: &mut App) {
    if matches!(state(cx), State::Checking | State::Downloading(_) | State::Installing(_)) {
        return;
    }
    if let State::Ready(release) = state(cx) {
        if manual {
            offer_restart(&release, cx);
        }
        return;
    }
    let updates = cx.default_global::<Updates>();
    updates.state = Some(State::Checking);
    updates.manual = manual;
    let (tx, rx) = futures::channel::mpsc::unbounded::<State>();
    let bundle = crate::bundle::app_bundle().map(Path::to_path_buf);
    let current = crate::bundle::version().to_string();
    let url = feed_url();
    let spawned = std::thread::Builder::new().name("zcode-updates".into()).spawn(move || {
        let report = |state: State| tx.unbounded_send(state).ok();
        let release = match fetch_feed(&url) {
            Ok(release) => release,
            Err(err) => {
                report(State::Failed(err));
                return;
            }
        };
        stamp_check();
        if !is_newer(&release.version, &current) {
            report(State::UpToDate);
            return;
        }
        if let Some(why) = too_new_for_this_mac(&release.minimum_system) {
            report(State::Available(release, why));
            return;
        }
        let Some(bundle) = bundle else {
            report(State::Available(release, "this zcode isn’t an app bundle".into()));
            return;
        };
        if let Err(why) = writable(&bundle) {
            report(State::Available(release, why));
            return;
        }
        report(State::Downloading(release.clone()));
        let archive = match download(&release) {
            Ok(path) => path,
            Err(err) => {
                report(State::Failed(err));
                return;
            }
        };
        report(State::Installing(release.clone()));
        match install(&archive, &release, &bundle) {
            Ok(()) => report(State::Ready(release)),
            Err(err) => report(State::Failed(err)),
        };
    });
    if spawned.is_err() {
        cx.default_global::<Updates>().state = Some(State::Failed("couldn’t start the check".into()));
        return;
    }
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        let mut rx = rx;
        while let Some(state) = rx.next().await {
            cx.update(|cx| {
                let manual = cx.default_global::<Updates>().manual;
                // A settled state stays until the next check.
                cx.default_global::<Updates>().state = Some(state.clone());
                match &state {
                    State::UpToDate if manual => {
                        crate::app::notify(Notification::info(format!("zcode {} is up to date.", crate::bundle::version())), cx)
                    }
                    State::Available(release, why) if manual => offer_download(release, why, cx),
                    State::Ready(release) => offer_restart(release, cx),
                    State::Failed(err) if manual => crate::app::notify(Notification::error(format!("Couldn’t check for updates: {err}")), cx),
                    State::Failed(err) => log::warn!("update check failed: {err}"),
                    _ => {}
                }
            });
        }
        // The thread ended without a settled state (it panicked): say so,
        // rather than stay Checking and turn every later check away.
        cx.update(|cx| {
            if matches!(state(cx), State::Checking | State::Downloading(_) | State::Installing(_)) {
                let manual = cx.default_global::<Updates>().manual;
                cx.default_global::<Updates>().state = Some(State::Failed("the update check stopped unexpectedly".into()));
                match manual {
                    true => crate::app::notify(Notification::error("Couldn’t check for updates: the check stopped unexpectedly"), cx),
                    false => log::warn!("update check stopped unexpectedly"),
                }
            }
        });
    })
    .detach();
}

fn offer_download(release: &Release, why: &str, cx: &mut App) {
    let url = release.url.clone();
    crate::app::notify(
        Notification::info(format!("zcode {} is available ({why}).", release.version)).action(move |_, _, _| {
            let url = url.clone();
            Button::new("download").label("Download").primary().on_click(move |_, _, cx| cx.open_url(&url))
        }),
        cx,
    );
}

fn offer_restart(release: &Release, cx: &mut App) {
    crate::app::notify(
        Notification::success(format!("zcode {} is installed. Restart to use it.", release.version))
            .action(|_, _, _| Button::new("restart").label("Restart Now").primary().on_click(|_, _, cx| restart(cx))),
        cx,
    );
}

/// Restart Now: the new bundle opens once this zcode has quit.
pub fn restart(cx: &mut App) {
    let Some(bundle) = crate::bundle::app_bundle() else { return };
    match crate::bundle::relaunch_after_exit(bundle) {
        Ok(()) => cx.dispatch_action(&crate::actions::Quit),
        Err(err) => crate::app::notify(Notification::error(err), cx),
    }
}

// --- the thread's work ------------------------------------------------------

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(15 * 60)))
            .user_agent(concat!("zcode/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let response = agent().get(url).call().map_err(|err| format!("{url}: {err}"))?;
    let status = response.status();
    match status.is_success() {
        true => Ok(response),
        false => Err(format!("{url}: HTTP {}", status.as_u16())),
    }
}

fn fetch_feed(url: &str) -> Result<Release, String> {
    let mut response = get(url)?;
    let text = response.body_mut().with_config().limit(MAX_FEED).read_to_string().map_err(|err| format!("{url}: {err}"))?;
    parse_feed(&text)
}

/// The feed: one release, or a list of them (the newest wins).
pub fn parse_feed(text: &str) -> Result<Release, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| format!("the feed isn’t JSON: {err}"))?;
    let releases: Vec<Release> = match value {
        serde_json::Value::Array(_) => serde_json::from_value(value).map_err(|err| format!("the feed’s releases: {err}"))?,
        serde_json::Value::Object(ref map) if map.contains_key("releases") => {
            serde_json::from_value(map["releases"].clone()).map_err(|err| format!("the feed’s releases: {err}"))?
        }
        _ => vec![serde_json::from_value(value).map_err(|err| format!("the feed’s release: {err}"))?],
    };
    releases
        .into_iter()
        .filter(|r| !r.version.is_empty() && !r.url.is_empty())
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .ok_or_else(|| "the feed has no release".to_string())
}

/// `major.minor.patch[-prerelease]`, as Cargo versions are: numbers
/// compared as numbers, a missing part as 0, a prerelease before its
/// release.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(v: &str) -> (Vec<u64>, Option<String>) {
        let (numbers, pre) = v.trim().trim_start_matches('v').split_once('-').map_or((v.trim().trim_start_matches('v'), None), |(n, p)| (n, Some(p.to_string())));
        let mut numbers: Vec<u64> = numbers.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        while numbers.len() < 3 {
            numbers.push(0);
        }
        (numbers, pre)
    }
    let (an, ap) = parts(a);
    let (bn, bp) = parts(b);
    an.cmp(&bn).then_with(|| match (ap, bp) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => a.cmp(&b),
    })
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == std::cmp::Ordering::Greater
}

/// Why the release can't run here, when the feed names a minimum macOS.
fn too_new_for_this_mac(minimum: &str) -> Option<String> {
    if minimum.trim().is_empty() {
        return None;
    }
    let output = std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().ok()?;
    let running = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (compare_versions(&running, minimum) == std::cmp::Ordering::Less).then(|| format!("it needs macOS {minimum}; this Mac runs {running}"))
}

/// Whether the bundle's folder takes a new bundle (`/Applications` of a
/// user who isn't an administrator doesn't).
fn writable(bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("the bundle has no folder")?;
    let probe = parent.join(format!(".zcode-update-probe-{}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            std::fs::remove_dir(&probe).ok();
            Ok(())
        }
        Err(_) => Err(format!("{} isn’t writable; download and replace zcode.app yourself", parent.display())),
    }
}

/// The cache folder downloads go to.
fn downloads_dir() -> PathBuf {
    match crate::paths::has_config_override() {
        true => crate::paths::config_dir().join("updates"),
        false => crate::paths::cache_dir().join("updates"),
    }
}

/// Downloads the release's zip, checking its SHA-256 against the feed's.
fn download(release: &Release) -> Result<PathBuf, String> {
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let path = dir.join(format!("zcode-{}.zip", release.version));
    let mut response = get(&release.url)?;
    let mut reader = response.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = std::fs::File::create(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|err| format!("downloading {}: {err}", release.url))?;
    file.write_all(&bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    drop(file);
    if release.sha256.trim().is_empty() {
        return Err("the feed doesn’t give the archive’s SHA-256".into());
    }
    let digest = sha256_hex(&bytes);
    if !digest.eq_ignore_ascii_case(release.sha256.trim()) {
        std::fs::remove_file(&path).ok();
        return Err(format!("the archive’s SHA-256 is {digest}, not the feed’s {}", release.sha256.trim()));
    }
    Ok(path)
}

unsafe extern "C" {
    // CommonCrypto, part of libSystem.
    fn CC_SHA256(data: *const u8, len: u32, md: *mut u8) -> *mut u8;
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = [0u8; 32];
    // SAFETY: the slice is readable for its length and the digest holds 32 bytes.
    unsafe { CC_SHA256(bytes.as_ptr(), bytes.len() as u32, digest.as_mut_ptr()) };
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks the archive next to the bundle, checks what came out and swaps
/// it into the bundle's place. The old bundle goes to the cache folder
/// (the running process keeps its mapped binary either way).
fn install(archive: &Path, release: &Release, bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("the bundle has no folder")?;
    let staging = parent.join(format!(".zcode-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging).map_err(|err| format!("{}: {err}", staging.display()))?;
    let result = install_in(archive, release, bundle, &staging);
    std::fs::remove_dir_all(&staging).ok();
    result
}

fn install_in(archive: &Path, release: &Release, bundle: &Path, staging: &Path) -> Result<(), String> {
    run(&["/usr/bin/ditto", "-x", "-k", &archive.display().to_string(), &staging.display().to_string()])?;
    let new = std::fs::read_dir(staging)
        .map_err(|err| err.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("the archive has no .app")?;
    verify(&new, release, bundle)?;
    let old = downloads_dir().join(format!("zcode-{}-previous.app", crate::bundle::version()));
    let _ = std::fs::remove_dir_all(&old);
    // Same folder first (one volume, so a rename), else the cache folder,
    // else out of the way beside the bundle.
    let aside = staging.join("previous.app");
    std::fs::rename(bundle, &aside).map_err(|err| format!("couldn’t move {} aside: {err}", bundle.display()))?;
    if let Err(err) = std::fs::rename(&new, bundle) {
        std::fs::rename(&aside, bundle).ok();
        return Err(format!("couldn’t put {} in place: {err}", new.display()));
    }
    if std::fs::rename(&aside, &old).is_err() {
        std::fs::remove_dir_all(&aside).ok();
    }
    Ok(())
}

/// What the new bundle must be before it replaces the running one.
fn verify(new: &Path, release: &Release, running: &Path) -> Result<(), String> {
    let info = std::fs::read_to_string(new.join("Contents/Info.plist")).map_err(|err| format!("the new bundle’s Info.plist: {err}"))?;
    let info = crate::plist::parse_xml(&info).map_err(|err| format!("the new bundle’s Info.plist: {err}"))?;
    let string = |key: &str| info.get(key).and_then(|n| n.as_str()).unwrap_or_default().to_string();
    if string("CFBundleIdentifier") != crate::bundle::IDENTIFIER {
        return Err(format!("the archive’s app is {}, not zcode", string("CFBundleIdentifier")));
    }
    if string("CFBundleShortVersionString") != release.version {
        return Err(format!("the archive’s app is version {}, not {}", string("CFBundleShortVersionString"), release.version));
    }
    run(&["/usr/bin/codesign", "--verify", "--deep", "--strict", &new.display().to_string()])
        .map_err(|err| format!("the new bundle’s signature: {err}"))?;
    let (new_team, running_team) = (team_of(new)?, team_of(running)?);
    if new_team != running_team {
        return Err(format!(
            "the new bundle is signed by {}, the running one by {}",
            new_team.as_deref().unwrap_or("nobody (ad hoc)"),
            running_team.as_deref().unwrap_or("nobody (ad hoc)")
        ));
    }
    if new_team.is_none() {
        log::warn!("installing an ad hoc signed update over an ad hoc signed zcode (a local build)");
    }
    Ok(())
}

/// The team a bundle is signed by (`None` for ad hoc).
fn team_of(bundle: &Path) -> Result<Option<String>, String> {
    let output = std::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2", &bundle.display().to_string()])
        .output()
        .map_err(|err| format!("couldn’t run codesign: {err}"))?;
    if !output.status.success() {
        return Err(format!("{} isn’t signed", bundle.display()));
    }
    // codesign describes on stderr.
    let text = String::from_utf8_lossy(&output.stderr);
    Ok(team_in(&text))
}

fn team_in(codesign_output: &str) -> Option<String> {
    let team = codesign_output.lines().find_map(|l| l.strip_prefix("TeamIdentifier="))?.trim();
    (team != "not set" && !team.is_empty()).then(|| team.to_string())
}

fn run(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .map_err(|err| format!("couldn’t run {}: {err}", args[0]))?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// The control channel's `app updates <command>`: `check` (as the menu
/// item), `status` (JSON: the state, the release it concerns), `restart`
/// (Restart Now), `feed` (the URL in use).
pub fn command(arg: &str, cx: &mut App) -> Result<String, String> {
    match arg {
        "check" => {
            check(true, cx);
            Ok(String::new())
        }
        "status" => {
            let state = state(cx);
            let (release, detail) = match &state {
                State::Available(release, why) => (Some(release), Some(why.clone())),
                State::Downloading(release) | State::Installing(release) | State::Ready(release) => (Some(release), None),
                State::Failed(err) => (None, Some(err.clone())),
                _ => (None, None),
            };
            Ok(serde_json::json!({
                "state": state.label(),
                "version": release.map(|r| r.version.clone()),
                "url": release.map(|r| r.url.clone()),
                "detail": detail,
                "current": crate::bundle::version(),
                "feed": feed_url(),
            })
            .to_string())
        }
        "restart" => {
            restart(cx);
            Ok(String::new())
        }
        _ => Err("usage: app updates check|status|restart".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{get, is_newer, parse_feed, sha256_hex, team_in};

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("1.0.0", "1.0"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        // A prerelease comes before its release and after the one before.
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
    }

    #[test]
    fn the_feed_may_be_one_release_or_many() {
        let one = parse_feed(r#"{"version": "0.2.0", "url": "https://x/zcode-0.2.0.zip", "sha256": "ab"}"#).unwrap();
        assert_eq!(one.version, "0.2.0");
        assert_eq!(one.sha256, "ab");
        let many = parse_feed(
            r#"{"releases": [{"version": "0.1.0", "url": "a"}, {"version": "0.3.0", "url": "c"}, {"version": "0.2.0", "url": "b"}]}"#,
        )
        .unwrap();
        assert_eq!((many.version.as_str(), many.url.as_str()), ("0.3.0", "c"));
        let list = parse_feed(r#"[{"version": "1.0.0", "url": "a"}, {"version": "1.0.1", "url": "b"}]"#).unwrap();
        assert_eq!(list.version, "1.0.1");
        assert!(parse_feed("not json").is_err());
        assert!(parse_feed(r#"{"version": "", "url": ""}"#).is_err());
    }

    #[test]
    fn sha256_matches_the_standard() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn https_fails_rather_than_panics() {
        // ureq panics on an https URL when its native-tls feature is off;
        // a closed port is an error either way.
        assert!(get("https://127.0.0.1:1/appcast.json").is_err());
    }

    #[test]
    fn the_team_is_read_from_codesign() {
        assert_eq!(team_in("Identifier=dev.zcode.zcode\nTeamIdentifier=ABCDE12345\n"), Some("ABCDE12345".into()));
        assert_eq!(team_in("Identifier=dev.zcode.zcode\nTeamIdentifier=not set\n"), None);
        assert_eq!(team_in(""), None);
    }
}

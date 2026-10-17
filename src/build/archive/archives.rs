//! The archives Product ▸ Archive makes and the Organizer lists: Xcode's
//! `.xcarchive` bundles in `~/Library/Developer/Xcode/Archives/<date>/`
//! (`crate::paths::archives_dir`), read from their `Info.plist` and their
//! app's. Xcode and zcode share the folder, so each lists the other's.
//! Blocking functions; call them from background tasks.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::plist::{self, Node, Value};

/// An `.xcarchive`, as its `Info.plist` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Archive {
    pub path: PathBuf,
    /// The Organizer's name for it: the scheme's (or its Archive Name).
    pub name: String,
    pub scheme: Option<String>,
    /// Seconds since 2001-01-01 (the plist's `CreationDate`).
    pub created: Option<f64>,
    /// The Organizer's Description field.
    pub comment: String,
    /// What an app archive holds; `None` for Other Items (a framework, a
    /// tool), which can't be distributed as an app.
    pub app: Option<App>,
}

/// An app archive's `ApplicationProperties`, and its platform from the
/// app's own Info.plist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct App {
    /// The app bundle in the archive.
    pub path: PathBuf,
    pub bundle_id: String,
    pub version: String,
    pub build: String,
    /// The team ID it was signed for (empty: Sign to Run Locally, no team).
    pub team: String,
    pub signing_identity: String,
    pub architectures: Vec<String>,
    /// "macOS", "iOS", "tvOS", "watchOS" or "visionOS".
    pub platform: String,
}

impl App {
    pub fn is_mac(&self) -> bool {
        self.platform == "macOS"
    }

    /// "1.0 (1)", as the Organizer shows it.
    pub fn version_label(&self) -> String {
        match (self.version.is_empty(), self.build.is_empty()) {
            (false, false) => format!("{} ({})", self.version, self.build),
            (false, true) => self.version.clone(),
            (true, false) => format!("({})", self.build),
            (true, true) => String::new(),
        }
    }
}

impl Archive {
    /// The Organizer's sidebar groups archives by app: its name and
    /// platform ("Other Items" for the rest).
    pub fn group(&self) -> String {
        match &self.app {
            Some(app) => format!("{} · {}", self.name, app.platform),
            None => "Other Items".into(),
        }
    }

    /// "macOS App", "iOS App", or Xcode's "Generic Xcode Archive".
    pub fn kind(&self) -> String {
        match &self.app {
            Some(app) => format!("{} App", app.platform),
            None => "Generic Xcode Archive".into(),
        }
    }

    /// "2026-10-17 15:16", in local time.
    pub fn created_label(&self) -> String {
        self.created.map(display_date).unwrap_or_default()
    }
}

fn read_plist(path: &Path) -> Result<Node> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (_, text) = plist::load(bytes)?;
    plist::parse_xml(&text)
}

fn string(node: &Node, key: &str) -> String {
    node.get(key).and_then(Node::as_str).unwrap_or_default().to_string()
}

/// The OS an app was built for, from its Info.plist (`DTPlatformName`, or
/// `CFBundleSupportedPlatforms`).
fn platform_of(info: &Node) -> Option<&'static str> {
    let names = std::iter::once(string(info, "DTPlatformName"))
        .chain(info.get("CFBundleSupportedPlatforms").map(Node::strings).unwrap_or_default());
    names.into_iter().find_map(|name| match name.to_lowercase().as_str() {
        "macosx" => Some("macOS"),
        "iphoneos" | "iphonesimulator" => Some("iOS"),
        "appletvos" | "appletvsimulator" => Some("tvOS"),
        "watchos" | "watchsimulator" => Some("watchOS"),
        "xros" | "xrsimulator" => Some("visionOS"),
        _ => None,
    })
}

/// Reads an `.xcarchive`.
pub fn read(path: &Path) -> Result<Archive> {
    let info = read_plist(&path.join("Info.plist"))?;
    let app = info.get("ApplicationProperties").map(|props| {
        let relative = string(props, "ApplicationPath");
        let bundle = path.join("Products").join(&relative);
        // A Mac app keeps its Info.plist in Contents.
        let mac_info = bundle.join("Contents/Info.plist");
        let app_info = if mac_info.is_file() { read_plist(&mac_info).ok() } else { read_plist(&bundle.join("Info.plist")).ok() };
        let platform = app_info
            .as_ref()
            .and_then(platform_of)
            .unwrap_or(if mac_info.is_file() { "macOS" } else { "iOS" });
        App {
            path: bundle,
            bundle_id: string(props, "CFBundleIdentifier"),
            version: string(props, "CFBundleShortVersionString"),
            build: string(props, "CFBundleVersion"),
            team: string(props, "Team"),
            signing_identity: string(props, "SigningIdentity"),
            architectures: props.get("Architectures").map(Node::strings).unwrap_or_default(),
            platform: platform.to_string(),
        }
    });
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = Some(string(&info, "Name")).filter(|n| !n.is_empty()).unwrap_or(stem);
    let created = match info.get("CreationDate").map(|n| &n.value) {
        Some(Value::Date(date)) => Some(*date),
        _ => None,
    };
    Ok(Archive {
        path: path.to_path_buf(),
        name,
        scheme: Some(string(&info, "SchemeName")).filter(|s| !s.is_empty()),
        created,
        comment: string(&info, "Comment"),
        app,
    })
}

fn is_archive(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("xcarchive")) && path.is_dir()
}

/// Every archive in `dir` (in its date folders, as Xcode files them, or
/// directly in it), the newest first. Unreadable ones are left out.
pub fn list(dir: &Path) -> Vec<Archive> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut paths = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        if is_archive(&path) {
            paths.push(path);
        } else if path.is_dir()
            && let Ok(inner) = std::fs::read_dir(&path)
        {
            paths.extend(inner.flatten().map(|e| e.path()).filter(|p| is_archive(p)));
        }
    }
    let mut archives: Vec<Archive> = paths
        .iter()
        .filter_map(|path| match read(path) {
            Ok(archive) => Some(archive),
            Err(err) => {
                ::log::debug!("archive {}: {err:#}", path.display());
                None
            }
        })
        .collect();
    archives.sort_by(|a, b| b.created.partial_cmp(&a.created).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.path.cmp(&b.path)));
    archives
}

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Seconds since 1970 in the Mac's time zone.
pub fn local_time(unix: i64) -> LocalTime {
    let time = unix as libc::time_t;
    // SAFETY: localtime_r only writes the struct it's given.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&time, &mut tm);
        tm
    };
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    }
}

/// Seconds between 1970-01-01 and 2001-01-01.
const UNIX_TO_2001: f64 = 978_307_200.;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// "2026-10-17 15:16" for a plist date, in local time.
pub fn display_date(since_2001: f64) -> String {
    let t = local_time((since_2001 + UNIX_TO_2001) as i64);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
}

/// Where a new archive goes, named as Xcode names them:
/// `<dir>/2026-10-17/HelloZ 17-10-2026, 15.16.xcarchive` (`… 2` and on
/// when that's taken).
pub fn new_archive_path(dir: &Path, name: &str, unix: i64) -> PathBuf {
    let t = local_time(unix);
    let folder = dir.join(format!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
    let name = name.replace(['/', ':'], "-");
    let base = format!("{name} {}-{}-{}, {:02}.{:02}", t.day, t.month, t.year, t.hour, t.minute);
    let mut path = folder.join(format!("{base}.xcarchive"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{base} {n}.xcarchive"));
        n += 1;
    }
    path
}

/// The folder Distribute App suggests exporting to, named as Xcode names
/// it: `<dir>/HelloZ 2026-10-17 15-16-42`.
pub fn export_folder(dir: &Path, name: &str, unix: i64) -> PathBuf {
    let t = local_time(unix);
    let name = name.replace(['/', ':'], "-");
    let base = format!("{name} {:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
    let mut path = dir.join(&base);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} {n}"));
        n += 1;
    }
    path
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An archive as `xcodebuild archive` leaves it (a Mac app's when
    /// `mac`), with an XML Info.plist and a binary one in the app.
    pub(crate) fn fake_archive(dir: &Path, file: &str, name: &str, mac: bool, team: &str, created: &str) -> PathBuf {
        let path = dir.join(file);
        let app = path.join("Products/Applications").join(format!("{name}.app"));
        let info_dir = if mac { app.join("Contents") } else { app.clone() };
        std::fs::create_dir_all(&info_dir).unwrap();
        let platform = if mac { "macosx" } else { "iphoneos" };
        let app_info = Value::Dict(vec![
            entry("CFBundleIdentifier", Value::String(format!("dev.zcode.{name}"))),
            entry("DTPlatformName", Value::String(platform.into())),
        ]);
        std::fs::write(info_dir.join("Info.plist"), plist::encode(&plist::to_xml(&app_info), plist::Format::Binary).unwrap()).unwrap();
        let info = Value::Dict(vec![
            entry(
                "ApplicationProperties",
                Value::Dict(vec![
                    entry("ApplicationPath", Value::String(format!("Applications/{name}.app"))),
                    entry("Architectures", Value::Array(vec![Node::new(Value::String("arm64".into()))])),
                    entry("CFBundleIdentifier", Value::String(format!("dev.zcode.{name}"))),
                    entry("CFBundleShortVersionString", Value::String("1.2".into())),
                    entry("CFBundleVersion", Value::String("7".into())),
                    entry("SigningIdentity", Value::String(if team.is_empty() { String::new() } else { "Apple Development: Someone (X)".into() })),
                    entry("Team", Value::String(team.into())),
                ]),
            ),
            entry("ArchiveVersion", Value::Integer(2)),
            entry("CreationDate", Value::Date(plist::parse_date(created).unwrap())),
            entry("Name", Value::String(name.into())),
            entry("SchemeName", Value::String(name.into())),
        ]);
        std::fs::write(path.join("Info.plist"), plist::to_xml(&info)).unwrap();
        path
    }

    /// A fresh folder for a test, removed afterwards.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("zcode-archive-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn entry(key: &str, value: Value) -> plist::Entry {
        plist::Entry { key: key.into(), key_span: 0..0, value: Node::new(value) }
    }

    #[test]
    fn reads_and_lists_archives_newest_first() {
        let dir = TempDir::new("list");
        fake_archive(&dir.path().join("2026-10-16"), "HelloZ 16-10-2026, 09.00.xcarchive", "HelloZ", false, "ABCDE12345", "2026-10-16T09:00:00Z");
        let mac = fake_archive(&dir.path().join("2026-10-17"), "HelloClassic 17-10-2026, 10.00.xcarchive", "HelloClassic", true, "", "2026-10-17T10:00:00Z");
        // Not an archive, and an archive without an app.
        std::fs::create_dir_all(dir.path().join("2026-10-17/notes")).unwrap();
        let other = dir.path().join("Kit.xcarchive");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Info.plist"), plist::to_xml(&Value::Dict(vec![entry("Name", Value::String("Kit".into()))]))).unwrap();

        let archives = list(dir.path());
        let names: Vec<_> = archives.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["HelloClassic", "HelloZ", "Kit"], "newest first, undated last");
        let classic = &archives[0];
        assert_eq!(classic.path, mac);
        let app = classic.app.as_ref().unwrap();
        assert!(app.is_mac() && app.team.is_empty() && app.architectures == ["arm64"], "{app:?}");
        assert_eq!((app.bundle_id.as_str(), app.version_label().as_str()), ("dev.zcode.HelloClassic", "1.2 (7)"));
        assert!(app.path.ends_with("Products/Applications/HelloClassic.app"));
        assert_eq!((classic.group().as_str(), classic.kind().as_str()), ("HelloClassic · macOS", "macOS App"));
        let ios = archives[1].app.as_ref().unwrap();
        assert_eq!((ios.platform.as_str(), ios.team.as_str()), ("iOS", "ABCDE12345"));
        assert_eq!((archives[2].app.is_none(), archives[2].kind().as_str()), (true, "Generic Xcode Archive"));
    }

    #[test]
    #[ignore = "reads ~/Library/Developer/Xcode/Archives (read-only)"]
    fn reads_xcodes_own_archives() {
        let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap().join("Library/Developer/Xcode/Archives");
        for archive in list(&dir) {
            assert!(!archive.name.is_empty() && archive.created.is_some(), "{archive:?}");
        }
    }

    #[test]
    fn new_archives_are_named_as_xcode_names_them() {
        let dir = TempDir::new("names");
        let unix = 1_792_249_200; // 2026-10-17 in every time zone's afternoon or morning
        let t = local_time(unix);
        let first = new_archive_path(dir.path(), "Hello/Z", unix);
        let expected = format!("Hello-Z {}-{}-{}, {:02}.{:02}.xcarchive", t.day, t.month, t.year, t.hour, t.minute);
        assert_eq!(first.file_name().unwrap().to_string_lossy(), expected);
        assert_eq!(first.parent().unwrap().file_name().unwrap().to_string_lossy(), format!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
        std::fs::create_dir_all(&first).unwrap();
        let second = new_archive_path(dir.path(), "Hello/Z", unix);
        assert!(second.to_string_lossy().ends_with(&format!("{:02}.{:02} 2.xcarchive", t.hour, t.minute)), "{second:?}");
        let export = export_folder(dir.path(), "HelloZ", unix);
        assert!(export.file_name().unwrap().to_string_lossy().starts_with(&format!("HelloZ {:04}-{:02}-{:02} ", t.year, t.month, t.day)));
    }
}

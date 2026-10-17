//! Distribute App's choices, as an `ExportOptions.plist` for `xcodebuild
//! -exportArchive`, which exports (or uploads, with `destination: upload`)
//! an archive for App Store Connect, TestFlight, Release Testing,
//! Enterprise, Debugging, Developer ID or as a copy of the app. Xcode's
//! Organizer runs the same distribution machinery.
//!
//! `ZCODE_EXPORT_ARCHIVE` names a program to run instead of `xcodebuild`
//! for exports (the e2e tests fake uploads with it).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::plist::{self, Entry, Node, Value};

/// How an archive is distributed: Distribute App's methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    AppStoreConnect,
    TestFlightInternal,
    ReleaseTesting,
    Enterprise,
    Debugging,
    DeveloperId,
    CopyApp,
}

impl Method {
    /// An iOS, tvOS, watchOS or visionOS app's, in Xcode's order.
    pub const DEVICE: [Method; 5] = [Self::AppStoreConnect, Self::TestFlightInternal, Self::ReleaseTesting, Self::Enterprise, Self::Debugging];
    /// A Mac app's.
    pub const MAC: [Method; 5] = [Self::AppStoreConnect, Self::TestFlightInternal, Self::DeveloperId, Self::CopyApp, Self::Debugging];

    pub fn for_app(mac: bool) -> &'static [Method] {
        if mac { &Self::MAC } else { &Self::DEVICE }
    }

    /// For the control channel and `dump`.
    pub fn id(self) -> &'static str {
        match self {
            Self::AppStoreConnect => "app-store-connect",
            Self::TestFlightInternal => "testflight-internal",
            Self::ReleaseTesting => "release-testing",
            Self::Enterprise => "enterprise",
            Self::Debugging => "debugging",
            Self::DeveloperId => "developer-id",
            Self::CopyApp => "copy-app",
        }
    }

    pub fn from_id(id: &str) -> Option<Method> {
        [Self::DEVICE.as_slice(), Self::MAC.as_slice()].concat().into_iter().find(|m| m.id() == id)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::AppStoreConnect => "App Store Connect",
            Self::TestFlightInternal => "TestFlight Internal Only",
            Self::ReleaseTesting => "Release Testing",
            Self::Enterprise => "Enterprise",
            Self::Debugging => "Debugging",
            Self::DeveloperId => "Developer ID",
            Self::CopyApp => "Copy App",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::AppStoreConnect => "Distribute on TestFlight and the App Store.",
            Self::TestFlightInternal => "Distribute to your team on TestFlight, never to external testers or the App Store.",
            Self::ReleaseTesting => "Install on your team’s registered devices (formerly Ad Hoc).",
            Self::Enterprise => "Distribute within your organization (Apple Developer Enterprise Program).",
            Self::Debugging => "Install on your team’s registered devices for debugging (Development).",
            Self::DeveloperId => "Distribute directly to customers, signed with Developer ID and notarized by Apple.",
            Self::CopyApp => "Export a copy of the archived app, as it was signed.",
        }
    }

    /// `ExportOptions.plist`'s `method`.
    pub fn export_method(self) -> &'static str {
        match self {
            Self::AppStoreConnect | Self::TestFlightInternal => "app-store-connect",
            Self::ReleaseTesting => "release-testing",
            Self::Enterprise => "enterprise",
            Self::Debugging => "debugging",
            Self::DeveloperId => "developer-id",
            Self::CopyApp => "mac-application",
        }
    }

    /// Whether `xcodebuild` can upload it to App Store Connect itself.
    pub fn can_upload(self) -> bool {
        matches!(self, Self::AppStoreConnect | Self::TestFlightInternal)
    }

    /// Developer ID apps are notarized after the export (`notarytool`).
    pub fn can_notarize(self) -> bool {
        self == Self::DeveloperId
    }

    /// Signed again for distribution: it needs a team.
    pub fn needs_team(self) -> bool {
        self != Self::CopyApp
    }

    /// App Store exports keep symbols and versions on Apple's side.
    pub fn is_app_store(self) -> bool {
        self.can_upload()
    }

    /// Thinning and stripping apply to installable copies.
    pub fn is_installable(self) -> bool {
        matches!(self, Self::ReleaseTesting | Self::Enterprise | Self::Debugging)
    }
}

/// What Distribute App was told.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportChoices {
    pub method: Method,
    /// `destination: upload` (App Store Connect and TestFlight), else export.
    pub upload: bool,
    /// The archive's team (`teamID`); empty leaves it to the archive.
    pub team: String,
    /// App Store: include symbols (`uploadSymbols`).
    pub upload_symbols: bool,
    /// App Store: Manage Version and Build Number.
    pub manage_version: bool,
    /// Installable copies: strip Swift symbols (`stripSwiftSymbols`).
    pub strip_swift_symbols: bool,
}

impl ExportChoices {
    /// Xcode's defaults: App Store methods upload.
    pub fn new(method: Method, team: &str) -> Self {
        Self {
            method,
            upload: method.can_upload(),
            team: team.to_string(),
            upload_symbols: true,
            manage_version: true,
            strip_swift_symbols: true,
        }
    }

    /// The `ExportOptions.plist`, keys in order as Xcode writes them.
    pub fn plist(&self) -> String {
        let mut entries = vec![("destination", Value::String(if self.upload && self.method.can_upload() { "upload" } else { "export" }.into()))];
        if self.method.is_app_store() {
            entries.push(("manageAppVersionAndBuildNumber", Value::Bool(self.manage_version)));
        }
        entries.push(("method", Value::String(self.method.export_method().into())));
        if self.method.is_installable() {
            entries.push(("stripSwiftSymbols", Value::Bool(self.strip_swift_symbols)));
        }
        if !self.team.is_empty() && self.method.needs_team() {
            entries.push(("teamID", Value::String(self.team.clone())));
        }
        if self.method == Method::TestFlightInternal {
            entries.push(("testFlightInternalTestingOnly", Value::Bool(true)));
        }
        if self.method.is_installable() {
            entries.push(("thinning", Value::String("<none>".into())));
        }
        if self.method.is_app_store() {
            entries.push(("uploadSymbols", Value::Bool(self.upload_symbols)));
        }
        let dict = entries.into_iter().map(|(key, value)| Entry { key: key.into(), key_span: 0..0, value: Node::new(value) }).collect();
        plist::to_xml(&Value::Dict(dict))
    }
}

pub fn xcodebuild() -> Command {
    Command::new(std::env::var_os("ZCODE_EXPORT_ARCHIVE").unwrap_or_else(|| "/usr/bin/xcodebuild".into()))
}

/// `xcodebuild -exportArchive`, with automatic signing allowed to create
/// and update profiles, as Xcode's Organizer does.
pub fn export_command(archive: &Path, export_path: &Path, options: &Path) -> Command {
    let mut cmd = xcodebuild();
    cmd.arg("-exportArchive")
        .arg("-archivePath")
        .arg(archive)
        .arg("-exportPath")
        .arg(export_path)
        .arg("-exportOptionsPlist")
        .arg(options)
        .arg("-allowProvisioningUpdates");
    cmd
}

/// What an export left in its folder: apps, `.ipa`s and installer packages.
pub fn exported_products(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut products: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(OsStr::to_str), Some("app" | "ipa" | "pkg")))
        .collect();
    products.sort();
    products
}

/// Xcode's export errors about signing point at its Signing & Capabilities
/// editor; zcode's at the project settings' tab. The rewritten message,
/// when it's one of those.
pub fn signing_hint(message: &str) -> Option<String> {
    const SIGNING: &[&str] = &["Team", "team", "signing certificate", "Signing certificate", "provisioning profile", "Provisioning profile", "No profiles", "Developer ID", "No Accounts", "No signing"];
    if !SIGNING.iter().any(|s| message.contains(s)) || message.contains("Signing & Capabilities tab") {
        return None;
    }
    let message = message.trim().trim_start_matches("exportArchive").trim_start_matches(':').trim();
    Some(format!(
        "{message}. Choose the team and signing in the target’s Signing & Capabilities tab in the project settings, sign in to your Apple account in Xcode ▸ Settings ▸ Accounts, then archive again."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(choices: &ExportChoices) -> Vec<(String, String)> {
        let root = plist::parse_xml(&choices.plist()).unwrap();
        (0..root.children_len())
            .map(|ix| {
                let (key, node) = root.child(ix).unwrap();
                (key.unwrap().to_string(), node.value.display())
            })
            .collect()
    }

    #[test]
    fn methods_per_platform() {
        let ids = |methods: &[Method]| methods.iter().map(|m| m.id()).collect::<Vec<_>>();
        assert_eq!(ids(Method::for_app(false)), ["app-store-connect", "testflight-internal", "release-testing", "enterprise", "debugging"]);
        assert_eq!(ids(Method::for_app(true)), ["app-store-connect", "testflight-internal", "developer-id", "copy-app", "debugging"]);
        assert_eq!(Method::from_id("copy-app"), Some(Method::CopyApp));
        assert_eq!(Method::from_id("ad-hoc"), None);
    }

    #[test]
    fn export_options_for_each_method() {
        let store = ExportChoices::new(Method::AppStoreConnect, "ABCDE12345");
        assert!(store.upload, "App Store Connect uploads unless told to export");
        assert_eq!(
            keys(&store),
            [
                ("destination", "upload"),
                ("manageAppVersionAndBuildNumber", "YES"),
                ("method", "app-store-connect"),
                ("teamID", "ABCDE12345"),
                ("uploadSymbols", "YES"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        let testflight = keys(&ExportChoices { upload: false, ..ExportChoices::new(Method::TestFlightInternal, "T") });
        assert!(testflight.contains(&("destination".into(), "export".into())));
        assert!(testflight.contains(&("testFlightInternalTestingOnly".into(), "YES".into())));
        let adhoc = keys(&ExportChoices { strip_swift_symbols: false, ..ExportChoices::new(Method::ReleaseTesting, "T") });
        assert_eq!(adhoc.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["destination", "method", "stripSwiftSymbols", "teamID", "thinning"]);
        assert!(adhoc.contains(&("method".into(), "release-testing".into())) && adhoc.contains(&("stripSwiftSymbols".into(), "NO".into())));
        assert!(adhoc.contains(&("destination".into(), "export".into())));
        // Uploading only where xcodebuild can.
        let developer_id = keys(&ExportChoices { upload: true, ..ExportChoices::new(Method::DeveloperId, "T") });
        assert!(developer_id.contains(&("destination".into(), "export".into())) && developer_id.contains(&("method".into(), "developer-id".into())));
        // A copy isn't signed again: no team.
        let copy = keys(&ExportChoices::new(Method::CopyApp, "T"));
        assert_eq!(copy, [("destination", "export"), ("method", "mac-application")].map(|(k, v)| (k.to_string(), v.to_string())));
        // An archive without a team leaves it out.
        assert!(!keys(&ExportChoices::new(Method::Debugging, "")).iter().any(|(k, _)| k == "teamID"));
    }

    #[test]
    fn an_exports_command() {
        let cmd = export_command(Path::new("/A/Hello.xcarchive"), Path::new("/Out/Hello 1"), Path::new("/c/ExportOptions.plist"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(
            args,
            ["-exportArchive", "-archivePath", "/A/Hello.xcarchive", "-exportPath", "/Out/Hello 1", "-exportOptionsPlist", "/c/ExportOptions.plist", "-allowProvisioningUpdates"]
        );
    }

    #[test]
    fn signing_errors_point_at_the_project_settings() {
        let hint = signing_hint("exportArchive No Team Found in Archive").unwrap();
        assert!(hint.starts_with("No Team Found in Archive. Choose the team"), "{hint}");
        assert!(hint.contains("Signing & Capabilities tab in the project settings"), "{hint}");
        assert!(signing_hint("exportArchive No signing certificate \"Developer ID Application\" found").is_some());
        assert_eq!(signing_hint("exportArchive: The operation couldn’t be completed."), None);
    }

    #[test]
    fn finds_what_an_export_left() {
        let dir = std::env::temp_dir().join(format!("zcode-exported-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Hello.app")).unwrap();
        std::fs::write(dir.join("ExportOptions.plist"), "").unwrap();
        std::fs::write(dir.join("Hello.ipa"), "").unwrap();
        let found = exported_products(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(found, [dir.join("Hello.app"), dir.join("Hello.ipa")]);
    }
}

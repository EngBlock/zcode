//! The capabilities Signing & Capabilities adds, and where each lives: keys
//! in the target's entitlements file, an Info.plist key (Background Modes),
//! or build settings (Hardened Runtime and App Sandbox, as Xcode 15+ manages
//! them). Adding or removing one is a list of `Edit`s for the caller to make;
//! `xcodebuild -allowProvisioningUpdates` registers them with Apple.

use crate::plist::{Node, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    IOS,
    MacOS,
    WatchOS,
    TvOS,
    VisionOS,
}

const ALL: &[Platform] = &[Platform::IOS, Platform::MacOS, Platform::WatchOS, Platform::TvOS, Platform::VisionOS];
const NOT_MAC: &[Platform] = &[Platform::IOS, Platform::WatchOS, Platform::TvOS, Platform::VisionOS];
const MAC: &[Platform] = &[Platform::MacOS];

/// The platforms a target builds for, from `SUPPORTED_PLATFORMS` (or
/// `SDKROOT` when that's unset).
pub fn platforms(sdkroot: Option<&str>, supported: Option<&str>) -> Vec<Platform> {
    let names = supported.filter(|s| !s.trim().is_empty()).or(sdkroot).unwrap_or("");
    let mut out = vec![];
    for name in names.split_whitespace() {
        let platform = match name {
            n if n.starts_with("iphone") => Platform::IOS,
            n if n.starts_with("macosx") => Platform::MacOS,
            n if n.starts_with("watch") => Platform::WatchOS,
            n if n.starts_with("appletv") => Platform::TvOS,
            n if n.starts_with("xr") => Platform::VisionOS,
            _ => continue,
        };
        if !out.contains(&platform) {
            out.push(platform);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    AppGroups,
    AppSandbox,
    AssociatedDomains,
    BackgroundModes,
    HardenedRuntime,
    HealthKit,
    ICloud,
    KeychainSharing,
    PushNotifications,
    SignInWithApple,
}

pub struct Capability {
    pub id: Id,
    pub name: &'static str,
    pub summary: &'static str,
    pub platforms: &'static [Platform],
}

/// In the order Xcode's capability library lists them.
pub static CAPABILITIES: &[Capability] = &[
    Capability { id: Id::AppGroups, name: "App Groups", summary: "Share data between apps and extensions from the same team.", platforms: ALL },
    Capability { id: Id::AppSandbox, name: "App Sandbox", summary: "Restrict the app’s access to system resources and user data.", platforms: MAC },
    Capability { id: Id::AssociatedDomains, name: "Associated Domains", summary: "Universal links, shared web credentials and App Clips for your domains.", platforms: ALL },
    Capability { id: Id::BackgroundModes, name: "Background Modes", summary: "Keep running in the background for audio, location, VoIP and more.", platforms: NOT_MAC },
    Capability { id: Id::HardenedRuntime, name: "Hardened Runtime", summary: "Runtime protections required for notarization.", platforms: MAC },
    Capability { id: Id::HealthKit, name: "HealthKit", summary: "Read and write health data with the user’s permission.", platforms: &[Platform::IOS, Platform::WatchOS, Platform::VisionOS] },
    Capability { id: Id::ICloud, name: "iCloud", summary: "Key-value storage, iCloud Documents and CloudKit.", platforms: ALL },
    Capability { id: Id::KeychainSharing, name: "Keychain Sharing", summary: "Share keychain items between your apps.", platforms: ALL },
    Capability { id: Id::PushNotifications, name: "Push Notifications", summary: "Receive remote notifications through APNs.", platforms: ALL },
    Capability { id: Id::SignInWithApple, name: "Sign in with Apple", summary: "Let people sign in with their Apple Account.", platforms: ALL },
];

pub fn capability(id: Id) -> &'static Capability {
    CAPABILITIES.iter().find(|c| c.id == id).expect("every Id is listed")
}

impl Capability {
    pub fn available_on(&self, platforms: &[Platform]) -> bool {
        platforms.is_empty() || platforms.iter().any(|p| self.platforms.contains(p))
    }
}

/// Where a value lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    Entitlement(&'static str),
    Info(&'static str),
    Setting(&'static str),
}

/// A change to make to the target.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Sets (or removes) a key of the entitlements file, creating the file.
    Entitlement(String, Option<Value>),
    /// Sets (or removes) a key of the Info.plist, creating the file.
    Info(String, Option<Value>),
    /// Sets (or removes) a build setting in every configuration.
    Setting(String, Option<String>),
}

/// What a target has now.
pub struct State<'a> {
    pub entitlements: Option<&'a Node>,
    pub info: Option<&'a Node>,
    /// A build setting as the target builds with it.
    pub setting: &'a dyn Fn(&str) -> Option<String>,
    pub platforms: &'a [Platform],
}

impl State<'_> {
    pub fn get(&self, store: Store) -> Option<Value> {
        match store {
            Store::Entitlement(key) => self.entitlements?.get(key).map(|n| n.value.clone()),
            Store::Info(key) => self.info?.get(key).map(|n| n.value.clone()),
            Store::Setting(key) => (self.setting)(key).map(Value::String),
        }
    }

    pub fn is_on(&self, store: Store) -> bool {
        match self.get(store) {
            Some(Value::Bool(b)) => b,
            Some(Value::String(s)) if matches!(store, Store::Setting(_)) => s.eq_ignore_ascii_case("YES"),
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Array(_) | Value::Dict(_)) => true,
            Some(_) => true,
            None => false,
        }
    }

    pub fn strings(&self, store: Store) -> Vec<String> {
        match self.get(store) {
            Some(Value::Array(items)) => items.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
            _ => vec![],
        }
    }

    fn mac_only(&self) -> bool {
        !self.platforms.is_empty() && self.platforms.iter().all(|p| *p == Platform::MacOS)
    }

    /// Whether the sandbox is the entitlement kind (before Xcode 15) rather
    /// than build settings.
    pub fn legacy_sandbox(&self) -> bool {
        !self.is_on(Store::Setting("ENABLE_APP_SANDBOX")) && self.is_on(Store::Entitlement(SANDBOX_ENTITLEMENT))
    }
}

const SANDBOX_ENTITLEMENT: &str = "com.apple.security.app-sandbox";

fn aps_key(state: &State) -> &'static str {
    if state.mac_only() { "com.apple.developer.aps-environment" } else { "aps-environment" }
}

/// The stores a capability's presence is read from.
fn markers(id: Id, state: &State) -> Vec<Store> {
    match id {
        Id::AppGroups => vec![Store::Entitlement("com.apple.security.application-groups")],
        Id::ICloud => vec![
            Store::Entitlement("com.apple.developer.icloud-container-identifiers"),
            Store::Entitlement("com.apple.developer.icloud-services"),
            Store::Entitlement("com.apple.developer.ubiquity-kvstore-identifier"),
        ],
        Id::PushNotifications => vec![Store::Entitlement(aps_key(state))],
        Id::KeychainSharing => vec![Store::Entitlement("keychain-access-groups")],
        Id::BackgroundModes => vec![Store::Info("UIBackgroundModes")],
        Id::AssociatedDomains => vec![Store::Entitlement("com.apple.developer.associated-domains")],
        Id::SignInWithApple => vec![Store::Entitlement("com.apple.developer.applesignin")],
        Id::HealthKit => vec![Store::Entitlement("com.apple.developer.healthkit")],
        Id::HardenedRuntime => vec![Store::Setting("ENABLE_HARDENED_RUNTIME")],
        Id::AppSandbox => vec![Store::Setting("ENABLE_APP_SANDBOX"), Store::Entitlement(SANDBOX_ENTITLEMENT)],
    }
}

pub fn is_enabled(id: Id, state: &State) -> bool {
    markers(id, state).into_iter().any(|store| state.is_on(store))
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|s| Node::new(Value::String(s.clone()))).collect())
}

/// The edits that add a capability, with the values Xcode starts it with.
pub fn add(id: Id, bundle_id: &str, state: &State) -> Vec<Edit> {
    let empty = || Some(Value::Array(vec![]));
    let entitlement = |key: &str, value: Option<Value>| Edit::Entitlement(key.to_string(), value);
    match id {
        Id::AppGroups => vec![entitlement("com.apple.security.application-groups", empty())],
        Id::ICloud => vec![
            entitlement("com.apple.developer.icloud-container-identifiers", empty()),
            entitlement("com.apple.developer.icloud-services", empty()),
        ],
        Id::PushNotifications => vec![entitlement(aps_key(state), Some(Value::String("development".into())))],
        Id::KeychainSharing => vec![entitlement("keychain-access-groups", Some(strings(&[format!("$(AppIdentifierPrefix){bundle_id}")])))],
        Id::BackgroundModes => vec![Edit::Info("UIBackgroundModes".into(), empty())],
        Id::AssociatedDomains => vec![entitlement("com.apple.developer.associated-domains", empty())],
        Id::SignInWithApple => vec![entitlement("com.apple.developer.applesignin", Some(strings(&["Default".into()])))],
        Id::HealthKit => vec![
            entitlement("com.apple.developer.healthkit", Some(Value::Bool(true))),
            entitlement("com.apple.developer.healthkit.access", empty()),
        ],
        Id::HardenedRuntime => vec![Edit::Setting("ENABLE_HARDENED_RUNTIME".into(), Some("YES".into()))],
        // As Xcode's macOS app template sets it up.
        Id::AppSandbox => vec![
            Edit::Setting("ENABLE_APP_SANDBOX".into(), Some("YES".into())),
            Edit::Setting("ENABLE_USER_SELECTED_FILES".into(), Some("readonly".into())),
        ],
    }
}

/// The edits that remove a capability and the options that belong to it.
pub fn remove(id: Id, state: &State) -> Vec<Edit> {
    let mut stores = markers(id, state);
    for section in controls(id, state) {
        for control in section.controls {
            stores.extend(control.stores());
        }
    }
    // Resource access settings are shared by App Sandbox and Hardened
    // Runtime; keep them while the other one stays.
    let other = match id {
        Id::AppSandbox => Some(Id::HardenedRuntime),
        Id::HardenedRuntime => Some(Id::AppSandbox),
        _ => None,
    };
    if let Some(other) = other.filter(|o| is_enabled(*o, state)) {
        let shared: Vec<Store> = controls(other, state).into_iter().flat_map(|s| s.controls).flat_map(|c| c.stores()).collect();
        stores.retain(|s| !matches!(s, Store::Setting(_)) || !shared.contains(s) || markers(id, state).contains(s));
    }
    let mut edits = vec![];
    for store in stores {
        if state.get(store).is_none() {
            continue;
        }
        let edit = match store {
            Store::Entitlement(key) => Edit::Entitlement(key.into(), None),
            Store::Info(key) => Edit::Info(key.into(), None),
            Store::Setting(key) => Edit::Setting(key.into(), None),
        };
        if !edits.contains(&edit) {
            edits.push(edit);
        }
    }
    edits
}

/// One option on a capability's card.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// A checkbox: on sets `store` to `on`, off removes it.
    Check { label: &'static str, store: Store, on: Value },
    /// A checkbox for one string in an array (`UIBackgroundModes`).
    Member { label: &'static str, store: Store, value: &'static str },
    /// An editable list of strings (App Groups, domains).
    List { store: Store, placeholder: String },
    /// None / Read Only / Read Write: a build setting (`readonly`,
    /// `readwrite`), or a pair of entitlements (`….read-only`, `….read-write`).
    Access { label: &'static str, store: Store },
}

impl Control {
    fn stores(&self) -> Vec<Store> {
        match self {
            Control::Check { store, .. } | Control::Member { store, .. } | Control::List { store, .. } => vec![*store],
            Control::Access { store: Store::Entitlement(prefix), .. } => {
                let (ro, rw) = access_keys(prefix);
                vec![Store::Entitlement(ro), Store::Entitlement(rw)]
            }
            Control::Access { store, .. } => vec![*store],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    None,
    ReadOnly,
    ReadWrite,
}

impl Access {
    pub const ALL: [Access; 3] = [Access::None, Access::ReadOnly, Access::ReadWrite];

    pub fn label(self) -> &'static str {
        match self {
            Access::None => "None",
            Access::ReadOnly => "Read Only",
            Access::ReadWrite => "Read/Write",
        }
    }
}

/// The pair of entitlements behind a legacy sandbox's file access popup.
fn access_keys(prefix: &'static str) -> (&'static str, &'static str) {
    match prefix {
        "com.apple.security.files.user-selected" => ("com.apple.security.files.user-selected.read-only", "com.apple.security.files.user-selected.read-write"),
        "com.apple.security.files.downloads" => ("com.apple.security.files.downloads.read-only", "com.apple.security.files.downloads.read-write"),
        "com.apple.security.assets.pictures" => ("com.apple.security.assets.pictures.read-only", "com.apple.security.assets.pictures.read-write"),
        "com.apple.security.assets.music" => ("com.apple.security.assets.music.read-only", "com.apple.security.assets.music.read-write"),
        _ => ("com.apple.security.assets.movies.read-only", "com.apple.security.assets.movies.read-write"),
    }
}

impl State<'_> {
    pub fn access(&self, store: Store) -> Access {
        match store {
            Store::Entitlement(prefix) => {
                let (ro, rw) = access_keys(prefix);
                if self.is_on(Store::Entitlement(rw)) {
                    Access::ReadWrite
                } else if self.is_on(Store::Entitlement(ro)) {
                    Access::ReadOnly
                } else {
                    Access::None
                }
            }
            _ => match self.get(store) {
                Some(Value::String(s)) if s == "readwrite" => Access::ReadWrite,
                Some(Value::String(s)) if s == "readonly" => Access::ReadOnly,
                _ => Access::None,
            },
        }
    }
}

/// The edits for a file access popup.
pub fn set_access(store: Store, access: Access) -> Vec<Edit> {
    match store {
        Store::Entitlement(prefix) => {
            let (ro, rw) = access_keys(prefix);
            let set = |key: &str, on: bool| Edit::Entitlement(key.into(), on.then_some(Value::Bool(true)));
            vec![set(ro, access == Access::ReadOnly), set(rw, access == Access::ReadWrite)]
        }
        Store::Setting(key) => vec![Edit::Setting(
            key.into(),
            match access {
                Access::None => None,
                Access::ReadOnly => Some("readonly".into()),
                Access::ReadWrite => Some("readwrite".into()),
            },
        )],
        Store::Info(key) => vec![Edit::Info(key.into(), None)],
    }
}

pub struct Section {
    pub title: &'static str,
    pub controls: Vec<Control>,
}

const BACKGROUND_MODES: &[(&str, &str)] = &[
    ("Audio, AirPlay, and Picture in Picture", "audio"),
    ("Location updates", "location"),
    ("Voice over IP", "voip"),
    ("External accessory communication", "external-accessory"),
    ("Uses Bluetooth LE accessories", "bluetooth-central"),
    ("Acts as a Bluetooth LE accessory", "bluetooth-peripheral"),
    ("Background fetch", "fetch"),
    ("Remote notifications", "remote-notification"),
    ("Background processing", "processing"),
    ("Nearby Interaction", "nearby-interaction"),
    ("Push to Talk", "push-to-talk"),
];

/// A sandbox or hardened runtime resource: a build setting, or the
/// entitlement older projects use.
fn resource(label: &'static str, setting: &'static str, entitlement: &'static str, legacy: bool) -> Control {
    let store = if legacy { Store::Entitlement(entitlement) } else { Store::Setting(setting) };
    let on = if legacy { Value::Bool(true) } else { Value::String("YES".into()) };
    Control::Check { label, store, on }
}

fn check(label: &'static str, key: &'static str) -> Control {
    Control::Check { label, store: Store::Entitlement(key), on: Value::Bool(true) }
}

/// The options on a capability's card, in Xcode's sections.
pub fn controls(id: Id, state: &State) -> Vec<Section> {
    let section = |title, controls| Section { title, controls };
    match id {
        Id::AppGroups => vec![section("", vec![Control::List { store: Store::Entitlement("com.apple.security.application-groups"), placeholder: "group.".into() }])],
        Id::ICloud => vec![
            section(
                "Services",
                vec![
                    Control::Check {
                        label: "Key-value storage",
                        store: Store::Entitlement("com.apple.developer.ubiquity-kvstore-identifier"),
                        on: Value::String("$(TeamIdentifierPrefix)$(CFBundleIdentifier)".into()),
                    },
                    Control::Member { label: "iCloud Documents", store: Store::Entitlement("com.apple.developer.icloud-services"), value: "CloudDocuments" },
                    Control::Member { label: "CloudKit", store: Store::Entitlement("com.apple.developer.icloud-services"), value: "CloudKit" },
                ],
            ),
            section("Containers", vec![Control::List { store: Store::Entitlement("com.apple.developer.icloud-container-identifiers"), placeholder: "iCloud.".into() }]),
        ],
        Id::KeychainSharing => vec![section("Keychain Groups", vec![Control::List { store: Store::Entitlement("keychain-access-groups"), placeholder: "$(AppIdentifierPrefix)".into() }])],
        Id::BackgroundModes => vec![section(
            "Modes",
            BACKGROUND_MODES.iter().map(|(label, value)| Control::Member { label, store: Store::Info("UIBackgroundModes"), value }).collect(),
        )],
        Id::AssociatedDomains => vec![section("Domains", vec![Control::List { store: Store::Entitlement("com.apple.developer.associated-domains"), placeholder: "applinks:".into() }])],
        Id::HealthKit => vec![section(
            "",
            vec![
                Control::Member { label: "Clinical Health Records", store: Store::Entitlement("com.apple.developer.healthkit.access"), value: "health-records" },
                check("Background Delivery", "com.apple.developer.healthkit.background-delivery"),
            ],
        )],
        Id::PushNotifications | Id::SignInWithApple => vec![],
        Id::HardenedRuntime => vec![
            section(
                "Runtime Exceptions",
                vec![
                    check("Allow Execution of JIT-compiled Code", "com.apple.security.cs.allow-jit"),
                    check("Allow Unsigned Executable Memory", "com.apple.security.cs.allow-unsigned-executable-memory"),
                    check("Allow DYLD Environment Variables", "com.apple.security.cs.allow-dyld-environment-variables"),
                    check("Disable Library Validation", "com.apple.security.cs.disable-library-validation"),
                    check("Disable Executable Memory Protection", "com.apple.security.cs.disable-executable-page-protection"),
                    check("Debugging Tool", "com.apple.security.cs.debugger"),
                ],
            ),
            section(
                "Resource Access",
                vec![
                    resource("Audio Input", "ENABLE_RESOURCE_ACCESS_AUDIO_INPUT", "com.apple.security.device.audio-input", false),
                    resource("Camera", "ENABLE_RESOURCE_ACCESS_CAMERA", "com.apple.security.device.camera", false),
                    resource("Location", "ENABLE_RESOURCE_ACCESS_LOCATION", "com.apple.security.personal-information.location", false),
                    resource("Address Book", "ENABLE_RESOURCE_ACCESS_CONTACTS", "com.apple.security.personal-information.addressbook", false),
                    resource("Calendar", "ENABLE_RESOURCE_ACCESS_CALENDARS", "com.apple.security.personal-information.calendars", false),
                    resource("Photos Library", "ENABLE_RESOURCE_ACCESS_PHOTO_LIBRARY", "com.apple.security.personal-information.photos-library", false),
                    check("Apple Events", "com.apple.security.automation.apple-events"),
                ],
            ),
        ],
        Id::AppSandbox => {
            let legacy = state.legacy_sandbox();
            let access = |label, setting, prefix| Control::Access { label, store: if legacy { Store::Entitlement(prefix) } else { Store::Setting(setting) } };
            vec![
                section(
                    "Network",
                    vec![
                        resource("Incoming Connections (Server)", "ENABLE_INCOMING_NETWORK_CONNECTIONS", "com.apple.security.network.server", legacy),
                        resource("Outgoing Connections (Client)", "ENABLE_OUTGOING_NETWORK_CONNECTIONS", "com.apple.security.network.client", legacy),
                    ],
                ),
                section(
                    "Hardware",
                    vec![
                        resource("Camera", "ENABLE_RESOURCE_ACCESS_CAMERA", "com.apple.security.device.camera", legacy),
                        resource("Audio Input", "ENABLE_RESOURCE_ACCESS_AUDIO_INPUT", "com.apple.security.device.audio-input", legacy),
                        resource("USB", "ENABLE_RESOURCE_ACCESS_USB", "com.apple.security.device.usb", legacy),
                        resource("Printing", "ENABLE_RESOURCE_ACCESS_PRINTING", "com.apple.security.print", legacy),
                        resource("Bluetooth", "ENABLE_RESOURCE_ACCESS_BLUETOOTH", "com.apple.security.device.bluetooth", legacy),
                    ],
                ),
                section(
                    "App Data",
                    vec![
                        resource("Contacts", "ENABLE_RESOURCE_ACCESS_CONTACTS", "com.apple.security.personal-information.addressbook", legacy),
                        resource("Location", "ENABLE_RESOURCE_ACCESS_LOCATION", "com.apple.security.personal-information.location", legacy),
                        resource("Calendar", "ENABLE_RESOURCE_ACCESS_CALENDARS", "com.apple.security.personal-information.calendars", legacy),
                    ],
                ),
                section(
                    "File Access",
                    vec![
                        access("User Selected File", "ENABLE_USER_SELECTED_FILES", "com.apple.security.files.user-selected"),
                        access("Downloads Folder", "ENABLE_FILE_ACCESS_DOWNLOADS_FOLDER", "com.apple.security.files.downloads"),
                        access("Pictures Folder", "ENABLE_FILE_ACCESS_PICTURE_FOLDER", "com.apple.security.assets.pictures"),
                        access("Music Folder", "ENABLE_FILE_ACCESS_MUSIC_FOLDER", "com.apple.security.assets.music"),
                        access("Movies Folder", "ENABLE_FILE_ACCESS_MOVIES_FOLDER", "com.apple.security.assets.movies"),
                    ],
                ),
            ]
        }
    }
}

/// The edit for toggling a checkbox or list membership.
pub fn toggle(control: &Control, state: &State) -> Vec<Edit> {
    let edit = |store: Store, value: Option<Value>| match store {
        Store::Entitlement(key) => Edit::Entitlement(key.into(), value),
        Store::Info(key) => Edit::Info(key.into(), value),
        Store::Setting(key) => Edit::Setting(
            key.into(),
            value.map(|v| match v {
                Value::String(s) => s,
                other => other.display(),
            }),
        ),
    };
    match control {
        Control::Check { store, on, .. } => vec![edit(*store, (!state.is_on(*store)).then(|| on.clone()))],
        Control::Member { store, value, .. } => {
            let mut values = state.strings(*store);
            match values.iter().position(|v| v == value) {
                Some(ix) => {
                    values.remove(ix);
                }
                None => values.push(value.to_string()),
            }
            vec![edit(*store, Some(strings(&values)))]
        }
        Control::List { .. } | Control::Access { .. } => vec![],
    }
}

/// The edit for adding (or with `remove`, removing) an item of a list.
pub fn edit_list(store: Store, item: &str, remove: bool, state: &State) -> Vec<Edit> {
    let mut values = state.strings(store);
    if remove {
        values.retain(|v| v != item);
    } else if !item.trim().is_empty() && !values.iter().any(|v| v == item.trim()) {
        values.push(item.trim().to_string());
    } else {
        return vec![];
    }
    let value = Some(strings(&values));
    vec![match store {
        Store::Entitlement(key) => Edit::Entitlement(key.into(), value),
        Store::Info(key) => Edit::Info(key.into(), value),
        Store::Setting(key) => Edit::Setting(key.into(), Some(values.join(" "))),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist::parse_xml;

    fn no_settings(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn reads_platforms() {
        assert_eq!(platforms(Some("iphoneos"), None), vec![Platform::IOS]);
        assert_eq!(platforms(Some("macosx"), Some("macosx iphoneos iphonesimulator")), vec![Platform::MacOS, Platform::IOS]);
        assert!(capability(Id::AppSandbox).available_on(&[Platform::MacOS]));
        assert!(!capability(Id::AppSandbox).available_on(&[Platform::IOS]));
        assert!(!capability(Id::BackgroundModes).available_on(&[Platform::MacOS]));
    }

    #[test]
    fn adds_and_finds_entitlement_capabilities() {
        let root = parse_xml("<plist><dict><key>com.apple.security.application-groups</key><array><string>group.a</string></array></dict></plist>").unwrap();
        let state = State { entitlements: Some(&root), info: None, setting: &no_settings, platforms: &[Platform::IOS] };
        assert!(is_enabled(Id::AppGroups, &state));
        assert!(!is_enabled(Id::ICloud, &state));
        assert_eq!(state.strings(Store::Entitlement("com.apple.security.application-groups")), vec!["group.a"]);
        assert_eq!(add(Id::PushNotifications, "dev.a", &state), vec![Edit::Entitlement("aps-environment".into(), Some(Value::String("development".into())))]);
        let keychain = add(Id::KeychainSharing, "dev.a", &state);
        assert_eq!(keychain, vec![Edit::Entitlement("keychain-access-groups".into(), Some(strings(&["$(AppIdentifierPrefix)dev.a".into()])))]);
        assert_eq!(remove(Id::AppGroups, &state), vec![Edit::Entitlement("com.apple.security.application-groups".into(), None)]);
        let list = edit_list(Store::Entitlement("com.apple.security.application-groups"), "group.b", false, &state);
        assert_eq!(list, vec![Edit::Entitlement("com.apple.security.application-groups".into(), Some(strings(&["group.a".into(), "group.b".into()])))]);
        assert!(edit_list(Store::Entitlement("com.apple.security.application-groups"), "group.a", false, &state).is_empty());
    }

    #[test]
    fn push_on_the_mac_uses_its_own_key() {
        let state = State { entitlements: None, info: None, setting: &no_settings, platforms: &[Platform::MacOS] };
        assert_eq!(add(Id::PushNotifications, "dev.a", &state)[0], Edit::Entitlement("com.apple.developer.aps-environment".into(), Some(Value::String("development".into()))));
    }

    #[test]
    fn background_modes_live_in_the_info_plist() {
        let info = parse_xml("<plist><dict><key>UIBackgroundModes</key><array><string>audio</string></array></dict></plist>").unwrap();
        let state = State { entitlements: None, info: Some(&info), setting: &no_settings, platforms: &[Platform::IOS] };
        assert!(is_enabled(Id::BackgroundModes, &state));
        let sections = controls(Id::BackgroundModes, &state);
        let fetch = sections[0].controls.iter().find(|c| matches!(c, Control::Member { value: "fetch", .. })).unwrap();
        assert_eq!(toggle(fetch, &state), vec![Edit::Info("UIBackgroundModes".into(), Some(strings(&["audio".into(), "fetch".into()])))]);
        let audio = &sections[0].controls[0];
        assert_eq!(toggle(audio, &state), vec![Edit::Info("UIBackgroundModes".into(), Some(Value::Array(vec![])))]);
        assert_eq!(add(Id::BackgroundModes, "", &state), vec![Edit::Info("UIBackgroundModes".into(), Some(Value::Array(vec![])))]);
    }

    #[test]
    fn sandbox_and_hardened_runtime_are_build_settings() {
        let settings = |key: &str| match key {
            "ENABLE_APP_SANDBOX" | "ENABLE_HARDENED_RUNTIME" | "ENABLE_RESOURCE_ACCESS_CAMERA" | "ENABLE_INCOMING_NETWORK_CONNECTIONS" => Some("YES".to_string()),
            "ENABLE_USER_SELECTED_FILES" => Some("readonly".to_string()),
            _ => None,
        };
        let state = State { entitlements: None, info: None, setting: &settings, platforms: &[Platform::MacOS] };
        assert!(is_enabled(Id::AppSandbox, &state));
        assert!(!state.legacy_sandbox());
        assert_eq!(state.access(Store::Setting("ENABLE_USER_SELECTED_FILES")), Access::ReadOnly);
        assert_eq!(set_access(Store::Setting("ENABLE_USER_SELECTED_FILES"), Access::ReadWrite), vec![Edit::Setting("ENABLE_USER_SELECTED_FILES".into(), Some("readwrite".into()))]);
        // Removing the sandbox keeps the camera, which the hardened runtime shares.
        let edits = remove(Id::AppSandbox, &state);
        assert!(edits.contains(&Edit::Setting("ENABLE_APP_SANDBOX".into(), None)));
        assert!(edits.contains(&Edit::Setting("ENABLE_INCOMING_NETWORK_CONNECTIONS".into(), None)));
        assert!(edits.contains(&Edit::Setting("ENABLE_USER_SELECTED_FILES".into(), None)));
        assert!(!edits.iter().any(|e| matches!(e, Edit::Setting(k, _) if k == "ENABLE_RESOURCE_ACCESS_CAMERA")), "{edits:?}");
        assert_eq!(add(Id::HardenedRuntime, "", &state), vec![Edit::Setting("ENABLE_HARDENED_RUNTIME".into(), Some("YES".into()))]);
    }

    #[test]
    fn older_sandboxes_are_entitlements() {
        let root = parse_xml("<plist><dict><key>com.apple.security.app-sandbox</key><true/><key>com.apple.security.files.user-selected.read-write</key><true/></dict></plist>").unwrap();
        let state = State { entitlements: Some(&root), info: None, setting: &no_settings, platforms: &[Platform::MacOS] };
        assert!(is_enabled(Id::AppSandbox, &state));
        assert!(state.legacy_sandbox());
        let sections = controls(Id::AppSandbox, &state);
        let client = sections[0].controls[1].clone();
        assert_eq!(toggle(&client, &state), vec![Edit::Entitlement("com.apple.security.network.client".into(), Some(Value::Bool(true)))]);
        assert_eq!(state.access(Store::Entitlement("com.apple.security.files.user-selected")), Access::ReadWrite);
        let edits = remove(Id::AppSandbox, &state);
        assert!(edits.contains(&Edit::Entitlement("com.apple.security.app-sandbox".into(), None)));
        assert!(edits.contains(&Edit::Entitlement("com.apple.security.files.user-selected.read-write".into(), None)));
    }
}

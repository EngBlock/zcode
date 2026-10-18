//! zcode ▸ Open at Login: the resident instance (`zcode --background`)
//! started when the user logs in, so even the first open of the day takes
//! the warm path. It's the bundle's LaunchAgent (`Contents/Library/
//! LaunchAgents/dev.zcode.zcode.background.plist`, from
//! `scripts/bundle/background.plist`) registered through SMAppService, so
//! System Settings ▸ General ▸ Login Items lists it under zcode and can
//! turn it off too. Asking launchd for the status takes a few
//! milliseconds, so it happens off the main thread, a moment after
//! launch; the menu's checkmark shows the last answer.
//!
//! `ZCODE_LOGIN_ITEM_FILE=<file>` keeps the state in that file instead of
//! launchd: the e2e suite never registers a real login item.

use std::path::PathBuf;

use gpui_kit::*;
use gpui_kit::component::notification::Notification;
use objc2_foundation::NSString;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// The agent's plist in the bundle.
pub const PLIST_NAME: &str = "dev.zcode.zcode.background.plist";

/// The environment variable that stands in for launchd.
pub const FILE_ENV: &str = "ZCODE_LOGIN_ITEM_FILE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Enabled,
    NotRegistered,
    /// Registered, and the user turned it off in System Settings.
    RequiresApproval,
    /// launchd has no record of the agent: never registered on this Mac
    /// (what a fresh bundle reports), or the bundle has no agent plist.
    NotFound,
    /// Not running from a bundle: launchd has nothing to register.
    NoBundle,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::NotRegistered => "not registered",
            Self::RequiresApproval => "requires approval",
            Self::NotFound => "not found",
            Self::NoBundle => "no bundle",
        }
    }
}

fn fake_file() -> Option<PathBuf> {
    std::env::var_os(FILE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn service() -> objc2::rc::Retained<SMAppService> {
    // SAFETY: a plist name; the service object has no other requirements.
    unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(PLIST_NAME)) }
}

fn from_sm(status: SMAppServiceStatus) -> Status {
    match status {
        SMAppServiceStatus::Enabled => Status::Enabled,
        SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
        SMAppServiceStatus::NotFound => Status::NotFound,
        _ => Status::NotRegistered,
    }
}

/// Asks launchd (or reads the file). Not for the main thread.
pub fn status() -> Status {
    if let Some(file) = fake_file() {
        return match std::fs::read_to_string(file).as_deref().map(str::trim) {
            Ok("on") => Status::Enabled,
            _ => Status::NotRegistered,
        };
    }
    if crate::bundle::app_bundle().is_none() {
        return Status::NoBundle;
    }
    // SAFETY: status has no requirements.
    from_sm(unsafe { service().status() })
}

/// Registers or unregisters the agent (or writes the file). Not for the
/// main thread.
pub fn set(on: bool) -> Result<Status, String> {
    if let Some(file) = fake_file() {
        std::fs::write(&file, if on { "on" } else { "off" }).map_err(|e| e.to_string())?;
        return Ok(status());
    }
    if crate::bundle::app_bundle().is_none() {
        return Err("zcode can open at login when it runs from zcode.app (scripts/package.py bundle)".into());
    }
    let service = service();
    // SAFETY: register and unregister have no requirements beyond the service.
    let result = unsafe {
        match on {
            true => service.registerAndReturnError(),
            false => service.unregisterAndReturnError(),
        }
    };
    match result {
        Ok(()) => Ok(from_sm(unsafe { service.status() })),
        Err(err) => Err(err.localizedDescription().to_string()),
    }
}

/// The last status read, for the menu's checkmark.
#[derive(Default)]
struct LoginItem(Option<Status>);
impl Global for LoginItem {}

/// Whether the menu shows Open at Login checked.
pub fn is_enabled(cx: &App) -> bool {
    cx.try_global::<LoginItem>().and_then(|l| l.0) == Some(Status::Enabled)
}

fn apply(status: Status, cx: &mut App) {
    cx.default_global::<LoginItem>().0 = Some(status);
    crate::actions::refresh_menus(cx);
}

/// Reads the status in the background and updates the checkmark.
pub fn refresh(cx: &mut App) {
    cx.spawn(async move |cx| {
        let status = cx.background_executor().spawn(async { status() }).await;
        cx.update(|cx| apply(status, cx));
    })
    .detach();
}

/// zcode ▸ Open at Login.
pub fn toggle(cx: &mut App) {
    let on = !is_enabled(cx);
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async move { set(on) }).await;
        cx.update(|cx| match result {
            Ok(status) => {
                apply(status, cx);
                if status == Status::RequiresApproval {
                    crate::app::notify(
                        Notification::warning("Open at Login is off in System Settings ▸ General ▸ Login Items; turn it on there."),
                        cx,
                    );
                }
            }
            Err(err) => crate::app::notify(Notification::error(format!("Couldn’t change Open at Login: {err}")), cx),
        });
    })
    .detach();
}

/// The control channel's `app login-item <command>`: `status` (asked now),
/// `on`, `off`, `toggle` (as the menu item), `menu` (what the checkmark
/// shows).
pub fn command(arg: &str, cx: &mut App) -> Result<String, String> {
    let json = |status: Status| serde_json::json!({ "status": status.label() }).to_string();
    match arg {
        "status" => Ok(json(status())),
        "on" | "off" => {
            let status = set(arg == "on")?;
            apply(status, cx);
            Ok(json(status))
        }
        "toggle" => {
            toggle(cx);
            Ok(String::new())
        }
        "menu" => Ok(serde_json::json!({ "checked": is_enabled(cx) }).to_string()),
        _ => Err("usage: app login-item status|on|off|toggle|menu".into()),
    }
}

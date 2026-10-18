//! zcode ▸ Install CLI: a `zcode` command for the shell, a link to the
//! bundle's binary in `/usr/local/bin` (Zed installs its CLI the same
//! way). When the folder isn't writable, an administrator prompt makes it
//! so (`osascript`'s `with administrator privileges`, as Zed asks).
//! `ZCODE_CLI_DIR` names another folder (tests). Once installed, `zcode
//! <path>` hands the path to the running zcode (`crate::instance`) or
//! starts the app (`crate::bundle`).

use std::path::{Path, PathBuf};

use gpui_kit::*;
use gpui_kit::component::notification::Notification;

/// The environment variable that names the folder the link goes in.
pub const DIR_ENV: &str = "ZCODE_CLI_DIR";

/// Where the link goes: `/usr/local/bin`, as Zed's, or `ZCODE_CLI_DIR`.
pub fn dir() -> PathBuf {
    std::env::var_os(DIR_ENV)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin"))
}

/// The link itself.
pub fn link_path() -> PathBuf {
    dir().join("zcode")
}

/// What's at the link's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A link to this zcode's binary.
    Installed,
    /// A link to another binary (an older copy, a bare build).
    Other(PathBuf),
    /// A file that isn't a link: not ours to replace.
    File,
    Missing,
}

pub fn status() -> Status {
    status_at(&link_path(), crate::bundle::cli_target().as_deref())
}

fn status_at(link: &Path, target: Option<&Path>) -> Status {
    match std::fs::symlink_metadata(link) {
        Err(_) => Status::Missing,
        Ok(meta) if !meta.file_type().is_symlink() => Status::File,
        Ok(_) => match std::fs::read_link(link) {
            Ok(points) if target.is_some_and(|t| t == points) => Status::Installed,
            Ok(points) => Status::Other(points),
            Err(_) => Status::Missing,
        },
    }
}

/// How the link was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub link: PathBuf,
    pub target: PathBuf,
    /// Through the administrator prompt.
    pub admin: bool,
}

/// Makes the link, replacing an older one; a plain file in the way is an
/// error.
pub fn install() -> Result<Installed, String> {
    let target = crate::bundle::cli_target().ok_or("zcode doesn’t know where its own binary is")?;
    let link = link_path();
    match install_at(&link, &target) {
        Ok(()) => Ok(Installed { link, target, admin: false }),
        // Not ours to write: ask for an administrator, unless a test chose
        // the folder (it would be writable).
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied && std::env::var_os(DIR_ENV).is_none() => {
            install_as_admin(&link, &target)?;
            Ok(Installed { link, target, admin: true })
        }
        Err(err) => Err(format!("couldn’t link {}: {err}", link.display())),
    }
}

fn install_at(link: &Path, target: &Path) -> std::io::Result<()> {
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(link)?,
        Ok(_) => {
            return Err(std::io::Error::other(format!("{} is a file, not a link", link.display())));
        }
        Err(_) => {}
    }
    std::os::unix::fs::symlink(target, link)
}

/// `mkdir` and `ln` with administrator privileges, through the system's
/// authorization dialog. `ZCODE_XCODE_COMMANDS` records the command.
fn install_as_admin(link: &Path, target: &Path) -> Result<(), String> {
    let quote = |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let shell = format!(
        "mkdir -p {} && ln -sfh {} {}",
        quote(link.parent().unwrap_or(Path::new("/usr/local/bin"))),
        quote(target),
        quote(link)
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let command = vec!["/usr/bin/osascript".to_string(), "-e".into(), script];
    if crate::xcode_app::record(&command) {
        return Ok(());
    }
    let mut child = std::process::Command::new(&command[0]);
    child.args(&command[1..]).stdin(std::process::Stdio::null());
    crate::build::process::unblock_signals(&mut child);
    let output = child.output().map_err(|err| format!("couldn’t run osascript: {err}"))?;
    match output.status.success() {
        true => Ok(()),
        false => {
            let why = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(match why.contains("-128") {
                true => "cancelled".into(),
                false => why,
            })
        }
    }
}

/// zcode ▸ Install CLI: the link, then a notification saying where.
pub fn install_action(cx: &mut App) {
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async { install() }).await;
        cx.update(|cx| match result {
            Ok(installed) => {
                let what = match crate::bundle::app_bundle() {
                    Some(_) => "".to_string(),
                    None => " (to this build of zcode, which isn’t an app bundle)".to_string(),
                };
                crate::app::notify(Notification::success(format!("Installed the zcode command at {}{what}.", installed.link.display())), cx);
            }
            Err(err) if err == "cancelled" => {}
            Err(err) => crate::app::notify(Notification::error(format!("Couldn’t install the zcode command: {err}")), cx),
        });
    })
    .detach();
}

/// The control channel's `app cli <command>`: `install`, `status`.
pub fn command(arg: &str, cx: &mut App) -> Result<String, String> {
    let describe = |status: Status| {
        serde_json::json!({
            "link": link_path().display().to_string(),
            "target": crate::bundle::cli_target().map(|p| p.display().to_string()),
            "status": match &status {
                Status::Installed => "installed".to_string(),
                Status::Other(path) => format!("other: {}", path.display()),
                Status::File => "file".into(),
                Status::Missing => "missing".into(),
            },
        })
        .to_string()
    };
    match arg {
        "install" => {
            install_action(cx);
            Ok(String::new())
        }
        "status" => Ok(describe(status())),
        _ => Err("usage: app cli install|status".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Status, install_at, status_at};

    #[test]
    fn the_link_is_made_replaced_and_told_apart_from_files() {
        let dir = tempdir();
        let link = dir.join("zcode");
        let target = dir.join("zcode.app/Contents/MacOS/zcode");
        assert_eq!(status_at(&link, Some(&target)), Status::Missing);
        install_at(&link, &target).unwrap();
        assert_eq!(status_at(&link, Some(&target)), Status::Installed);
        assert_eq!(status_at(&link, Some(Path::new("/elsewhere/zcode"))), Status::Other(target.clone()));
        // An older link is replaced.
        let newer = dir.join("new/zcode");
        install_at(&link, &newer).unwrap();
        assert_eq!(status_at(&link, Some(&newer)), Status::Installed);
        // A file is left alone.
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "#!/bin/sh\n").unwrap();
        assert_eq!(status_at(&link, Some(&newer)), Status::File);
        assert!(install_at(&link, &newer).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-cli-test-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

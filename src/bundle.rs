//! The `zcode.app` this binary runs from, when it runs from one
//! (`scripts/package.py bundle` makes it): its path, identifier and
//! version; whether this launch came through the command-line link
//! `zcode ▸ Install CLI` makes; starting the app through LaunchServices,
//! as Finder does; the paths Finder hands a running zcode.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The bundle identifier `scripts/package.py` writes to `Info.plist`.
pub const IDENTIFIER: &str = "dev.zcode.zcode";

/// zcode's version, from `Cargo.toml` (the bundle's
/// `CFBundleShortVersionString` is the same).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// This binary, symlinks resolved.
pub fn executable() -> Option<&'static Path> {
    static EXECUTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXECUTABLE
        .get_or_init(|| std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok()))
        .as_deref()
}

/// The `.app` this binary is the executable of (`…/zcode.app/Contents/
/// MacOS/zcode`), or `None` for a bare `target/release/zcode`.
pub fn app_bundle() -> Option<&'static Path> {
    static BUNDLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    BUNDLE.get_or_init(|| executable().and_then(bundle_of)).as_deref()
}

/// The bundle an executable path lies in, by its layout alone.
pub fn bundle_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is = |dir: &Path, name: &str| dir.file_name().is_some_and(|n| n == name);
    (is(macos, "MacOS") && is(contents, "Contents") && bundle.extension().is_some_and(|e| e == "app"))
        .then(|| bundle.to_path_buf())
}

/// Where the command-line `zcode` points after Install CLI: the bundle's
/// executable, or this binary when there's no bundle.
pub fn cli_target() -> Option<PathBuf> {
    executable().map(Path::to_path_buf)
}

/// Whether this process was started by name from a shell (`zcode …` through
/// the link Install CLI makes) rather than by LaunchServices or by its own
/// path. `ZCODE_CLI=1` says so outright.
pub fn launched_through_cli() -> bool {
    if std::env::var_os("ZCODE_CLI").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    // The login item's agent (`scripts/package.py` writes its plist) runs
    // the bundle's binary itself.
    if std::env::var_os("ZCODE_LAUNCHED_AT_LOGIN").is_some() {
        return false;
    }
    argv0_path().and_then(|p| std::fs::symlink_metadata(p).ok()).is_some_and(|m| m.file_type().is_symlink())
}

/// `argv[0]` as a path: as given when it has a directory, else where the
/// shell found it in `PATH`.
fn argv0_path() -> Option<PathBuf> {
    let argv0 = PathBuf::from(std::env::args_os().next()?);
    if argv0.components().count() > 1 {
        return std::path::absolute(argv0).ok();
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(&argv0)).find(|p| p.exists())
}

/// Starts the app through LaunchServices (`open -a`), so a launch from a
/// shell runs like one from Finder: the Dock icon, document events and
/// the login item's instance are the bundle's, and the shell gets its
/// prompt back. `args` are the app's arguments (a path, `--background`).
/// `ZCODE_XCODE_COMMANDS` records the command instead of running it.
pub fn launch(bundle: &Path, args: &[String]) -> Result<(), String> {
    let mut command = vec!["/usr/bin/open".to_string(), "-a".into(), bundle.display().to_string()];
    if !args.is_empty() {
        command.push("--args".into());
        command.extend(args.iter().cloned());
    }
    if crate::xcode_app::record(&command) {
        return Ok(());
    }
    let mut child = std::process::Command::new(&command[0]);
    child.args(&command[1..]).stdin(std::process::Stdio::null());
    crate::build::process::unblock_signals(&mut child);
    let output = child.output().map_err(|err| format!("couldn’t run open: {err}"))?;
    match output.status.success() {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// Opens `bundle` again once this process has exited (Restart Now after an
/// update): a shell waits for the pid, then `open`s the app, so the new
/// launch is the bundle's and not this process's child.
pub fn relaunch_after_exit(bundle: &Path) -> Result<(), String> {
    let script = format!(
        "while /bin/kill -0 {pid} 2>/dev/null; do /bin/sleep 0.1; done; /usr/bin/open -a \"{app}\"",
        pid = std::process::id(),
        app = bundle.display()
    );
    let command = vec!["/bin/sh".to_string(), "-c".into(), script];
    if crate::xcode_app::record(&command) {
        return Ok(());
    }
    let mut child = std::process::Command::new(&command[0]);
    child
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::build::process::unblock_signals(&mut child);
    child.spawn().map(|_| ()).map_err(|err| format!("couldn’t start the relaunch: {err}"))
}

/// The path of a `file:` URL LaunchServices hands over (percent-encoded,
/// a folder's with a trailing slash); `None` for any other scheme.
pub fn path_of_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/…` names this machine; anything else is remote.
    let path = match rest.strip_prefix('/') {
        Some(_) => rest,
        None => rest.strip_prefix("localhost/").map(|p| &rest[rest.len() - p.len() - 1..])?,
    };
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'%' if raw.len() >= i + 3 => {
                let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            byte => {
                bytes.push(byte);
                i += 1;
            }
        }
    }
    let mut path = String::from_utf8(bytes).ok()?;
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    Some(PathBuf::from(path))
}

/// What the control channel's `app bundle` reports.
pub fn info() -> serde_json::Value {
    serde_json::json!({
        "bundle": app_bundle().map(|p| p.display().to_string()),
        "executable": executable().map(|p| p.display().to_string()),
        "identifier": IDENTIFIER,
        "version": version(),
        "cli": launched_through_cli(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_known_by_its_layout() {
        assert_eq!(
            bundle_of(Path::new("/Applications/zcode.app/Contents/MacOS/zcode")),
            Some(PathBuf::from("/Applications/zcode.app"))
        );
        assert_eq!(bundle_of(Path::new("/Users/me/zcode/target/release/zcode")), None);
        assert_eq!(bundle_of(Path::new("/tmp/zcode.app/Contents/zcode")), None);
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(
            path_of_file_url("file:///Users/me/My%20App/My%20App.xcodeproj/"),
            Some(PathBuf::from("/Users/me/My App/My App.xcodeproj"))
        );
        assert_eq!(path_of_file_url("file://localhost/tmp/Package.swift"), Some(PathBuf::from("/tmp/Package.swift")));
        assert_eq!(path_of_file_url("file:///"), Some(PathBuf::from("/")));
        assert_eq!(path_of_file_url("https://example.com/a.xcodeproj"), None);
        assert_eq!(path_of_file_url("file://server/share/a"), None);
    }
}

//! Locating Apple developer tools via `xcrun`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context as _, Result, bail};
use parking_lot::Mutex;

fn cache() -> &'static Mutex<HashMap<String, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Resolves a developer tool (e.g. `sourcekit-lsp`, `lldb-dap`) through the
/// active Xcode (`xcode-select -p`), caching the answer. Blocking: call from a
/// background task.
pub fn find_tool(name: &str) -> Result<PathBuf> {
    if let Some(path) = cache().lock().get(name) {
        return Ok(path.clone());
    }
    let output = Command::new("/usr/bin/xcrun")
        .args(["--find", name])
        .output()
        .with_context(|| format!("running xcrun --find {name}"))?;
    if !output.status.success() {
        bail!(
            "xcrun could not find {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    cache().lock().insert(name.to_string(), path.clone());
    Ok(path)
}

/// `xcrun --sdk <sdk> --show-sdk-path`.
pub fn sdk_path(sdk: &str) -> Result<PathBuf> {
    let output = Command::new("/usr/bin/xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()?;
    if !output.status.success() {
        bail!("unknown SDK {sdk}");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// `xcrun --sdk <sdk> --show-sdk-version`.
pub fn sdk_version(sdk: &str) -> Result<String> {
    let output = Command::new("/usr/bin/xcrun")
        .args(["--sdk", sdk, "--show-sdk-version"])
        .output()?;
    if !output.status.success() {
        bail!("unknown SDK {sdk}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The app that shows simulator windows for the active Xcode: `Simulator.app`
/// up to Xcode 26, `DeviceHub.app` from Xcode 27. Opened by path because
/// `open -a` resolves names through Launch Services, which may not know about
/// an Xcode outside /Applications (or may pick a different one).
pub fn simulator_app() -> Option<PathBuf> {
    let output = Command::new("/usr/bin/xcode-select").arg("-p").output().ok()?;
    let developer = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let contents = developer.parent()?;
    [
        contents.join("Applications/DeviceHub.app"),
        developer.join("Applications/Simulator.app"),
    ]
    .into_iter()
    .find(|app| app.exists())
}

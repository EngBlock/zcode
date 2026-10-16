//! Where a project's packages are: its `Package.resolved`, and the folder
//! resolved packages are checked out in. Xcode keeps checkouts in the
//! project's DerivedData folder (`SourcePackages/checkouts`), found through
//! the `WorkspacePath` each DerivedData folder records in its `info.plist`;
//! Swift packages keep them in `.build/checkouts`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::project::ProjectKind;

/// The `Package.resolved` a project's packages are pinned in.
pub fn resolved_file(kind: &ProjectKind, root: &Path) -> Option<PathBuf> {
    match kind {
        ProjectKind::XcodeProject(project) => {
            Some(project.join("project.xcworkspace/xcshareddata/swiftpm/Package.resolved"))
        }
        ProjectKind::XcodeWorkspace(workspace) => Some(workspace.join("xcshareddata/swiftpm/Package.resolved")),
        ProjectKind::SwiftPackage => Some(root.join("Package.resolved")),
        ProjectKind::Folder => None,
    }
}

/// Whether `path` is inside a package checkout (sources zcode shows
/// read-only): `SourcePackages/checkouts` or `.build/checkouts`.
pub fn is_package_checkout(path: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    parts.windows(2).any(|w| (w[0] == "SourcePackages" || w[0] == ".build") && w[1] == "checkouts")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Xcode's DerivedData folder: the user's custom location if set (read
/// from Xcode's defaults), else `~/Library/Developer/Xcode/DerivedData`.
pub fn derived_data_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(read_derived_data_root).clone()
}

fn read_derived_data_root() -> PathBuf {
    let custom = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    match custom {
        Some(path) if path.starts_with('/') => PathBuf::from(path),
        _ => home().join("Library/Developer/Xcode/DerivedData"),
    }
}

/// MD5, for DerivedData folder names.
fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4,
        11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.).sin().abs() * 4294967296.) as u32).collect();
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((input.len() as u64).wrapping_mul(8)).to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in message.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]).rotate_left(S[i]);
            (a, d, c, b) = (d, c, b, b.wrapping_add(rotated));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, word) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// Xcode's DerivedData folder name for a workspace path: `<Name>-` and 28
/// letters from the path's MD5 (each half of the digest in base 26).
pub fn derived_data_name(workspace: &Path) -> String {
    let digest = md5(workspace.to_string_lossy().as_bytes());
    let mut letters = [b'a'; 28];
    let mut start = u64::from_be_bytes(digest[..8].try_into().unwrap_or_default());
    for letter in letters[..14].iter_mut().rev() {
        *letter = b'a' + (start % 26) as u8;
        start /= 26;
    }
    let mut end = u64::from_be_bytes(digest[8..].try_into().unwrap_or_default());
    for letter in letters[14..].iter_mut().rev() {
        *letter = b'a' + (end % 26) as u8;
        end /= 26;
    }
    let name = workspace.file_stem().map(|s| s.to_string_lossy().replace(' ', "_")).unwrap_or_default();
    format!("{name}-{}", String::from_utf8_lossy(&letters))
}

/// The DerivedData folder Xcode and `xcodebuild` use for `workspace` (an
/// `.xcodeproj` or `.xcworkspace`) in `root`: the folder named for its
/// path (as Foundation standardizes it, without `/private` before `/tmp`
/// and `/var`), else the one whose `info.plist` records the path.
pub fn derived_data_for(root: &Path, workspace: &Path) -> Option<PathBuf> {
    let standardized = ["/private/tmp/", "/private/var/", "/private/etc/"]
        .iter()
        .find(|prefix| workspace.starts_with(prefix.trim_end_matches('/')))
        .and_then(|_| workspace.strip_prefix("/private").ok())
        .map(|rest| Path::new("/").join(rest));
    for path in standardized.iter().map(PathBuf::as_path).chain([workspace]) {
        let dir = root.join(derived_data_name(path));
        if dir.exists() {
            return Some(dir);
        }
    }
    let name = workspace.file_stem()?.to_string_lossy().replace(' ', "_");
    let prefix = format!("{name}-");
    let needle = workspace.to_string_lossy().to_string();
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
        .filter(|p| {
            // XML or binary, an ASCII path appears as is.
            std::fs::read(p.join("info.plist"))
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        })
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// The folder holding a project's `checkouts` and `workspace-state.json`:
/// `SourcePackages` in its DerivedData folder (`None` before anything
/// resolved there), or a package's `.build`.
pub fn packages_dir(kind: &ProjectKind, root: &Path) -> Option<PathBuf> {
    match kind {
        ProjectKind::XcodeProject(workspace) | ProjectKind::XcodeWorkspace(workspace) => {
            derived_data_for(&derived_data_root(), workspace).map(|d| d.join("SourcePackages"))
        }
        ProjectKind::SwiftPackage => Some(root.join(".build")),
        ProjectKind::Folder => None,
    }
}

/// Checked-out (or local) packages by identity, from SwiftPM's
/// `workspace-state.json` in `dir`, falling back to the `checkouts`
/// folder's contents.
pub fn checkouts(dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    if let Some(state) = std::fs::read_to_string(dir.join("workspace-state.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        let dependencies = state.pointer("/object/dependencies").and_then(Value::as_array).cloned().unwrap_or_default();
        for dependency in dependencies {
            let Some(reference) = dependency.get("packageRef") else { continue };
            let Some(identity) = reference.get("identity").and_then(Value::as_str) else { continue };
            let kind = dependency.pointer("/state/name").and_then(Value::as_str).unwrap_or("");
            let path = match kind {
                "fileSystem" => reference.get("location").and_then(Value::as_str).map(PathBuf::from),
                // Packages in edit mode live in Packages/.
                "edited" => dependency
                    .pointer("/state/path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .or_else(|| dependency.get("subpath").and_then(Value::as_str).map(|s| dir.join("../Packages").join(s))),
                _ => dependency.get("subpath").and_then(Value::as_str).map(|s| dir.join("checkouts").join(s)),
            };
            if let Some(path) = path.filter(|p| p.exists()) {
                out.insert(identity.to_string(), path);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("checkouts")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() && !name.starts_with('.') {
                out.entry(name.to_lowercase()).or_insert_with(|| entry.path());
            }
        }
    }
    out
}

/// Makes SwiftPM forget the checkouts of `identities` (every checkout
/// when `None`) in `dir`: their entries in `workspace-state.json` and their
/// folders. Without a pin and a checkout, the next resolve picks the newest
/// version a package's rule allows, which is how `xcodebuild` updates
/// packages (it prefers an existing checkout to a newer version otherwise).
/// The repository caches stay, so only new commits are fetched.
pub fn forget_checkouts(dir: &Path, identities: Option<&[String]>) -> anyhow::Result<()> {
    let state_file = dir.join("workspace-state.json");
    let Ok(text) = std::fs::read_to_string(&state_file) else { return Ok(()) };
    let mut state: Value = serde_json::from_str(&text)?;
    let Some(dependencies) = state.pointer_mut("/object/dependencies").and_then(Value::as_array_mut) else { return Ok(()) };
    let mut forgotten = vec![];
    dependencies.retain(|dependency| {
        let identity = dependency.pointer("/packageRef/identity").and_then(Value::as_str).unwrap_or("");
        let checkout = dependency.pointer("/state/name").and_then(Value::as_str) == Some("sourceControlCheckout");
        let forget = checkout && identities.is_none_or(|ids| ids.iter().any(|i| i.eq_ignore_ascii_case(identity)));
        if forget && let Some(subpath) = dependency.get("subpath").and_then(Value::as_str) {
            forgotten.push(dir.join("checkouts").join(subpath));
        }
        !forget
    });
    std::fs::write(&state_file, super::resolved::to_swiftpm_json(&state))?;
    for checkout in forgotten {
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_checkouts_are_read_only() {
        assert!(is_package_checkout(Path::new(
            "/Users/me/Library/Developer/Xcode/DerivedData/App-abc/SourcePackages/checkouts/swift-collections/Sources/A.swift"
        )));
        assert!(is_package_checkout(Path::new("/p/ZKit/.build/checkouts/Tally/Sources/Tally/Tally.swift")));
        assert!(!is_package_checkout(Path::new("/p/ZKit/Sources/checkouts/A.swift")));
        assert!(!is_package_checkout(Path::new("/p/App/SourcePackages/A.swift")));
    }

    #[test]
    fn resolved_files() {
        let project = ProjectKind::XcodeProject(PathBuf::from("/p/App.xcodeproj"));
        assert_eq!(
            resolved_file(&project, Path::new("/p")).unwrap(),
            Path::new("/p/App.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved")
        );
        let workspace = ProjectKind::XcodeWorkspace(PathBuf::from("/p/App.xcworkspace"));
        assert_eq!(
            resolved_file(&workspace, Path::new("/p")).unwrap(),
            Path::new("/p/App.xcworkspace/xcshareddata/swiftpm/Package.resolved")
        );
        assert_eq!(resolved_file(&ProjectKind::SwiftPackage, Path::new("/p")).unwrap(), Path::new("/p/Package.resolved"));
    }

    #[test]
    fn hashes_like_xcode() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"The quick brown fox jumps over the lazy dog")), "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(hex(md5(&[b'a'; 200])), "887f30b43b2867f4a9accceee7d16e6c");
        assert_eq!(
            derived_data_name(Path::new("/Users/nathan/Programming/OpenConverter/CoinConverter.xcodeproj")),
            "CoinConverter-gymihpfrpceafketxxltntmnkyhw"
        );
    }

    #[test]
    fn finds_derived_data_and_checkouts() {
        let dir = std::env::temp_dir().join(format!("zcode-dd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = Path::new("/Users/me/Projects/My App/My App.xcodeproj");
        for (name, path) in [("My_App-aaaa", "/elsewhere/My App.xcodeproj"), ("My_App-bbbb", workspace.to_str().unwrap())] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            let plist = format!("<plist><dict><key>WorkspacePath</key><string>{path}</string></dict></plist>");
            std::fs::write(dir.join(name).join("info.plist"), plist).unwrap();
        }
        let found = derived_data_for(&dir, workspace).unwrap();
        assert!(found.ends_with("My_App-bbbb"));

        let packages = found.join("SourcePackages");
        std::fs::create_dir_all(packages.join("checkouts/swift-collections")).unwrap();
        std::fs::create_dir_all(packages.join("checkouts/Tally")).unwrap();
        let state = r#"{"object":{"dependencies":[{"basedOn":null,"packageRef":{"identity":"tally","kind":"localSourceControl","location":"/tmp/Tally.git","name":"Tally"},"state":{"checkoutState":{"version":"1.2.0"},"name":"sourceControlCheckout"},"subpath":"Tally"}]},"version":7}"#;
        std::fs::write(packages.join("workspace-state.json"), state).unwrap();
        let found = checkouts(&packages);
        assert_eq!(found["tally"], packages.join("checkouts/Tally"));
        assert_eq!(found["swift-collections"], packages.join("checkouts/swift-collections"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

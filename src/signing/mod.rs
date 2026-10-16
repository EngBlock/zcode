//! What Signing & Capabilities offers to choose from, read from this Mac
//! without Apple's developer portal: development teams (Xcode's signed-in
//! accounts, and the teams of the keychain's signing identities), the
//! signing identities themselves (`security find-identity`), and installed
//! provisioning profiles (decoded with `security cms -D`). Reading spawns
//! processes, so callers run `Catalog::load` off the main thread.

pub mod capabilities;

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::plist::{self, Node, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Team {
    pub id: String,
    pub name: String,
    /// "Individual", "Company", "Personal Team"…
    pub kind: Option<String>,
}

impl Team {
    /// "Nathan Beddoe (68GMM3MWGN)", or with " (Personal Team)".
    pub fn label(&self) -> String {
        match self.kind.as_deref() {
            Some("Personal Team") => format!("{} (Personal Team)", self.name),
            _ if self.name.is_empty() => self.id.clone(),
            _ => format!("{} ({})", self.name, self.id),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// "Apple Development: Nathan Beddoe (C83T9WVRNA)".
    pub name: String,
    pub sha1: String,
    pub team: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub uuid: String,
    pub teams: Vec<String>,
    pub team_name: Option<String>,
    /// The entitlements' `application-identifier`: `TEAMID.bundle.id` or `TEAMID.*`.
    pub app_id: Option<String>,
    pub platforms: Vec<String>,
    /// Seconds since 2001, as plists store dates.
    pub expires: Option<f64>,
    pub xcode_managed: bool,
    pub path: PathBuf,
}

impl Profile {
    /// Whether this profile can sign `bundle_id` for `team`.
    pub fn matches(&self, team: &str, bundle_id: &str) -> bool {
        if !team.is_empty() && !self.teams.iter().any(|t| t == team) {
            return false;
        }
        let Some(app_id) = &self.app_id else { return true };
        let pattern = app_id.split_once('.').map_or(app_id.as_str(), |(_, rest)| rest);
        match pattern.strip_suffix('*') {
            Some(prefix) => bundle_id.starts_with(prefix),
            None => pattern == bundle_id,
        }
    }

    pub fn expired(&self) -> bool {
        self.expires.is_some_and(|e| e < plist::now_since_2001())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub teams: Vec<Team>,
    pub identities: Vec<Identity>,
    pub profiles: Vec<Profile>,
}

fn run(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(program).args(args).output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// The teams of Xcode's signed-in accounts, from its defaults
/// (`IDEProvisioningTeamByIdentifier` in Xcode 15+, `IDEProvisioningTeams`
/// before).
pub fn teams_from_defaults(root: &Node) -> Vec<Team> {
    let mut teams = vec![];
    for key in ["IDEProvisioningTeamByIdentifier", "IDEProvisioningTeams"] {
        let Some(Value::Dict(accounts)) = root.get(key).map(|n| &n.value) else { continue };
        for account in accounts {
            let Value::Array(list) = &account.value.value else { continue };
            for team in list {
                let Some(id) = team.get("teamID").and_then(Node::as_str) else { continue };
                let free = team.get("isFreeProvisioningTeam").and_then(Node::as_bool).unwrap_or(false);
                let kind = if free { Some("Personal Team".to_string()) } else { team.get("teamType").and_then(Node::as_str).map(str::to_string) };
                push_team(&mut teams, Team { id: id.to_string(), name: team.get("teamName").and_then(Node::as_str).unwrap_or("").to_string(), kind });
            }
        }
    }
    teams
}

fn push_team(teams: &mut Vec<Team>, team: Team) {
    match teams.iter_mut().find(|t| t.id == team.id) {
        Some(known) if known.name.is_empty() => known.name = team.name,
        Some(_) => {}
        None => teams.push(team),
    }
}

/// `security find-identity -v -p codesigning` lines: `  1) SHA1 "Name"`.
pub fn parse_identities(text: &str) -> Vec<Identity> {
    text.lines()
        .filter_map(|line| {
            let (_, rest) = line.trim().split_once(") ")?;
            let (sha1, name) = rest.split_once(' ')?;
            let name = name.trim().strip_prefix('"')?.strip_suffix('"')?;
            (sha1.len() == 40).then(|| Identity { name: name.to_string(), sha1: sha1.to_string(), team: None })
        })
        .collect()
}

/// A field of an `openssl x509 -subject` line (`subject= /UID=…/CN=…/OU=TEAM/O=Name/C=US`).
pub fn subject_field(subject: &str, field: &str) -> Option<String> {
    let needle = format!("/{field}=");
    let start = subject.find(&needle)? + needle.len();
    let rest = &subject[start..];
    let end = rest.find("/").filter(|&i| rest[i..].find('=').is_some()).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// The certificate's team (its subject's organizational unit) and name.
fn identity_team(name: &str) -> Option<(String, String)> {
    let pem = run("/usr/bin/security", &["find-certificate", "-c", name, "-p"])?;
    let mut child = Command::new("/usr/bin/openssl")
        .args(["x509", "-noout", "-subject"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    use std::io::Write as _;
    child.stdin.take()?.write_all(&pem).ok()?;
    let output = child.wait_with_output().ok()?;
    let subject = String::from_utf8_lossy(&output.stdout).to_string();
    Some((subject_field(&subject, "OU")?, subject_field(&subject, "O").unwrap_or_default()))
}

/// A decoded provisioning profile (`security cms -D` output).
pub fn parse_profile(xml: &str, path: &Path) -> Option<Profile> {
    let root = plist::parse_xml(xml).ok()?;
    let entitlements = root.get("Entitlements");
    Some(Profile {
        name: root.get("Name")?.as_str()?.to_string(),
        uuid: root.get("UUID").and_then(Node::as_str).unwrap_or("").to_string(),
        teams: root.get("TeamIdentifier").map(Node::strings).unwrap_or_default(),
        team_name: root.get("TeamName").and_then(Node::as_str).map(str::to_string),
        app_id: entitlements
            .and_then(|e| e.get("application-identifier").or_else(|| e.get("com.apple.application-identifier")))
            .and_then(Node::as_str)
            .map(str::to_string),
        platforms: root.get("Platform").map(Node::strings).unwrap_or_default(),
        expires: match root.get("ExpirationDate").map(|n| &n.value) {
            Some(Value::Date(d)) => Some(*d),
            _ => None,
        },
        xcode_managed: root.get("IsXcodeManaged").and_then(Node::as_bool).unwrap_or(false),
        path: path.to_path_buf(),
    })
}

pub fn profile_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return vec![] };
    vec![
        home.join("Library/Developer/Xcode/UserData/Provisioning Profiles"),
        home.join("Library/MobileDevice/Provisioning Profiles"),
    ]
}

impl Catalog {
    /// Reads everything (blocking: runs `security`, `openssl` and `defaults`).
    pub fn load() -> Self {
        let mut catalog = Catalog::default();
        if let Some(bytes) = run("/usr/bin/defaults", &["export", "com.apple.dt.Xcode", "-"])
            && let Ok((_, xml)) = plist::load(bytes)
            && let Ok(root) = plist::parse_xml(&xml)
        {
            catalog.teams = teams_from_defaults(&root);
        }
        if let Some(out) = run("/usr/bin/security", &["find-identity", "-v", "-p", "codesigning"]) {
            catalog.identities = parse_identities(&String::from_utf8_lossy(&out));
        }
        for identity in &mut catalog.identities {
            if let Some((team, name)) = identity_team(&identity.name) {
                identity.team = Some(team.clone());
                push_team(&mut catalog.teams, Team { id: team, name, kind: None });
            }
        }
        let mut seen = std::collections::HashSet::new();
        for dir in profile_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.extension().is_some_and(|e| e == "mobileprovision" || e == "provisionprofile") {
                    continue;
                }
                let Some(path_str) = path.to_str() else { continue };
                let Some(xml) = run("/usr/bin/security", &["cms", "-D", "-i", path_str]) else { continue };
                if let Some(profile) = parse_profile(&String::from_utf8_lossy(&xml), &path)
                    && seen.insert(profile.uuid.clone())
                {
                    for team in &profile.teams {
                        push_team(&mut catalog.teams, Team { id: team.clone(), name: profile.team_name.clone().unwrap_or_default(), kind: None });
                    }
                    catalog.profiles.push(profile);
                }
            }
        }
        catalog.profiles.sort_by(|a, b| a.name.cmp(&b.name));
        catalog
    }

    pub fn team(&self, id: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_xcode_accounts() {
        let xml = "<plist><dict><key>IDEProvisioningTeamByIdentifier</key><dict><key>FA69</key><array><dict><key>isFreeProvisioningTeam</key><false/><key>teamID</key><string>68GMM3MWGN</string><key>teamName</key><string>Nathan Beddoe</string><key>teamType</key><string>Individual</string></dict><dict><key>isFreeProvisioningTeam</key><true/><key>teamID</key><string>ABCDE12345</string><key>teamName</key><string>Me</string></dict></array></dict></dict></plist>";
        let teams = teams_from_defaults(&plist::parse_xml(xml).unwrap());
        assert_eq!(teams.len(), 2);
        assert_eq!(teams[0].label(), "Nathan Beddoe (68GMM3MWGN)");
        assert_eq!(teams[1].label(), "Me (Personal Team)");
    }

    #[test]
    fn reads_identities_and_subjects() {
        let ids = parse_identities("  1) 5D2277F6C6BF30C76A879D9D764D82E208457BE5 \"Apple Development: Nathan Beddoe (C83T9WVRNA)\"\n     1 valid identities found\n");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].name, "Apple Development: Nathan Beddoe (C83T9WVRNA)");
        let subject = "subject= /UID=Y3L6JPL9FK/CN=Apple Development: Nathan Beddoe (C83T9WVRNA)/OU=68GMM3MWGN/O=Nathan Beddoe/C=US";
        assert_eq!(subject_field(subject, "OU").as_deref(), Some("68GMM3MWGN"));
        assert_eq!(subject_field(subject, "O").as_deref(), Some("Nathan Beddoe"));
        assert_eq!(subject_field(subject, "CN").as_deref(), Some("Apple Development: Nathan Beddoe (C83T9WVRNA)"));
    }

    #[test]
    fn reads_decoded_profiles() {
        // `security cms -D` output keeps odd indentation; trimmed here.
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>AppIDName</key>\n\t<string>XC Wildcard</string>\n\t<key>Platform</key>\n\t<array>\n\t\t<string>iOS</string>\n\t</array>\n\t<key>IsXcodeManaged</key>\n\t<true/>\n\t<key>DeveloperCertificates</key>\n\t<array>\n\t\t<data>MIIFyDCC</data>\n\t</array>\n\n\t<key>Entitlements</key>\n\t<dict>\n\t\t\t\t\n\t\t\t\t<key>application-identifier</key>\n\t\t<string>68GMM3MWGN.*</string>\n\t</dict>\n\t<key>ExpirationDate</key>\n\t<date>2027-09-12T20:12:27Z</date>\n\t<key>Name</key>\n\t<string>iOS Team Provisioning Profile: *</string>\n\t<key>TeamIdentifier</key>\n\t<array>\n\t\t<string>68GMM3MWGN</string>\n\t</array>\n\t<key>TeamName</key>\n\t<string>Nathan Beddoe</string>\n\t<key>UUID</key>\n\t<string>12eedf33</string>\n</dict>\n</plist>";
        let profile = parse_profile(xml, Path::new("/p.mobileprovision")).unwrap();
        assert_eq!(profile.name, "iOS Team Provisioning Profile: *");
        assert_eq!(profile.teams, vec!["68GMM3MWGN"]);
        assert!(profile.xcode_managed);
        assert!(profile.matches("68GMM3MWGN", "dev.zcode.HelloZ"));
        assert!(!profile.matches("OTHERTEAM1", "dev.zcode.HelloZ"));
        let exact = Profile { app_id: Some("68GMM3MWGN.dev.zcode.App".into()), ..profile.clone() };
        assert!(exact.matches("68GMM3MWGN", "dev.zcode.App"));
        assert!(!exact.matches("68GMM3MWGN", "dev.zcode.Other"));
        assert_eq!(plist::format_date(profile.expires.unwrap()), "2027-09-12T20:12:27Z");
    }
}

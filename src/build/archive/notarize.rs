//! Notarizing a Developer ID export: the app zipped (`ditto`), submitted to
//! Apple's notary service with `notarytool submit --wait` and credentials
//! from a keychain profile (made once with `xcrun notarytool
//! store-credentials`), then the ticket stapled to the app (`stapler`). A
//! rejected submission's log says why.
//!
//! `ZCODE_NOTARYTOOL` and `ZCODE_STAPLER` name programs to run instead of
//! `xcrun notarytool` and `xcrun stapler` (the e2e tests fake both: nothing
//! is ever sent to Apple from a test).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn xcrun(tool: &str, override_var: &str) -> Command {
    match std::env::var_os(override_var) {
        Some(program) => Command::new(program),
        None => {
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.arg(tool);
            cmd
        }
    }
}

pub fn notarytool() -> Command {
    xcrun("notarytool", "ZCODE_NOTARYTOOL")
}

pub fn stapler() -> Command {
    xcrun("stapler", "ZCODE_STAPLER")
}

/// The app as the zip notarytool takes (`ditto`, keeping its folder).
pub fn zip_command(app: &Path, zip: &Path) -> Command {
    let mut cmd = Command::new("/usr/bin/ditto");
    cmd.args(["-c", "-k", "--keepParent"]).arg(app).arg(zip);
    cmd
}

/// Submits and waits for Apple's verdict, answered as JSON.
pub fn submit_command(zip: &Path, profile: &str) -> Command {
    let mut cmd = notarytool();
    cmd.arg("submit").arg(zip).args(["--keychain-profile", profile, "--wait", "--output-format", "json"]);
    cmd
}

/// A submission's log: why it was rejected.
pub fn log_command(id: &str, profile: &str) -> Command {
    let mut cmd = notarytool();
    cmd.args(["log", id, "--keychain-profile", profile]);
    cmd
}

pub fn staple_command(app: &Path) -> Command {
    let mut cmd = stapler();
    cmd.arg("staple").arg(app);
    cmd
}

/// notarytool's answer to a submission.
#[derive(Clone, Debug, PartialEq)]
pub struct Submission {
    pub id: String,
    /// "Accepted", "Invalid", "Rejected", "In Progress"…
    pub status: String,
    pub message: String,
}

impl Submission {
    pub fn accepted(&self) -> bool {
        self.status == "Accepted"
    }
}

/// The JSON object notarytool prints (`--output-format json`, or a log),
/// after any progress lines.
fn last_object(output: &str) -> Option<Value> {
    let whole = output.find('{').and_then(|start| serde_json::from_str::<Value>(output[start..].trim_end()).ok());
    whole.or_else(|| {
        output
            .lines()
            .rev()
            .map(str::trim)
            .filter(|l| l.starts_with('{'))
            .find_map(|l| serde_json::from_str::<Value>(l).ok())
    })
}

pub fn parse_submission(output: &str) -> Option<Submission> {
    let value = last_object(output)?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let id = text("id");
    (!id.is_empty()).then(|| Submission { id, status: text("status"), message: text("message") })
}

/// A rejected submission's issues from its log: (severity, message, path).
pub fn parse_log(output: &str) -> Vec<(String, String, Option<String>)> {
    let Some(value) = last_object(output) else { return vec![] };
    value
        .get("issues")
        .and_then(Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .map(|issue| {
                    let text = |key: &str| issue.get(key).and_then(Value::as_str).map(str::to_string);
                    (text("severity").unwrap_or_else(|| "error".into()), text("message").unwrap_or_default(), text("path"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What to do about a keychain profile notarytool couldn't use.
pub fn credentials_hint(output: &str, profile: &str) -> Option<String> {
    let lower = output.to_lowercase();
    (lower.contains("keychain") && (lower.contains("no keychain password item") || lower.contains("profile"))).then(|| {
        format!(
            "No notarization credentials named “{profile}” in the keychain. Store them once with `xcrun notarytool store-credentials {profile}` in a terminal (an app-specific password or an App Store Connect API key), then distribute again."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn commands() {
        let submit = submit_command(Path::new("/x/Hello.zip"), "zcode-notary");
        assert_eq!(args(&submit)[1..], ["submit", "/x/Hello.zip", "--keychain-profile", "zcode-notary", "--wait", "--output-format", "json"].map(String::from));
        assert_eq!(args(&zip_command(Path::new("/x/Hello.app"), Path::new("/t/Hello.zip"))), ["-c", "-k", "--keepParent", "/x/Hello.app", "/t/Hello.zip"]);
        assert_eq!(args(&staple_command(Path::new("/x/Hello.app")))[1..], ["staple", "/x/Hello.app"].map(String::from));
        assert_eq!(args(&log_command("abc", "p"))[1..], ["log", "abc", "--keychain-profile", "p"].map(String::from));
    }

    #[test]
    fn a_submissions_answer() {
        let accepted = "Conducting pre-submission checks for Hello.zip and initiating connection to the Apple notary service...\n{\"id\":\"2efe2717-52ef-43a5-96dc-0797e4ca1041\",\"status\":\"Accepted\",\"message\":\"Processing complete\"}\n";
        let submission = parse_submission(accepted).unwrap();
        assert!(submission.accepted() && submission.id == "2efe2717-52ef-43a5-96dc-0797e4ca1041", "{submission:?}");
        let invalid = parse_submission("{\"id\":\"1\",\"status\":\"Invalid\",\"message\":\"Processing complete\"}").unwrap();
        assert!(!invalid.accepted());
        assert_eq!(parse_submission("Error: HTTP status code: 401. Unable to authenticate."), None);
    }

    #[test]
    fn a_rejected_submissions_log() {
        let log = r#"{
  "jobId": "1",
  "status": "Invalid",
  "issues": [
    { "severity": "error", "code": null, "path": "Hello.zip/Hello.app/Contents/MacOS/Hello", "message": "The binary is not signed with a valid Developer ID certificate.", "architecture": "arm64" },
    { "severity": "warning", "path": null, "message": "The executable does not have the hardened runtime enabled." }
  ]
}"#;
        let issues = parse_log(log);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].1, "The binary is not signed with a valid Developer ID certificate.");
        assert_eq!(issues[0].2.as_deref(), Some("Hello.zip/Hello.app/Contents/MacOS/Hello"));
        assert_eq!((issues[1].0.as_str(), issues[1].2.as_deref()), ("warning", None));
    }

    #[test]
    fn missing_credentials() {
        let hint = credentials_hint("Error: No Keychain password item found for profile: zcode-notary", "zcode-notary").unwrap();
        assert!(hint.contains("xcrun notarytool store-credentials zcode-notary"), "{hint}");
        assert_eq!(credentials_hint("Error: HTTP status code: 500", "p"), None);
    }
}

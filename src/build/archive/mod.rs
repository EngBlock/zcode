//! Product ▸ Archive and Distribute App. Archive builds the scheme with its
//! Archive action's configuration (Release unless the scheme says
//! otherwise) for a generic device of the run destination's platform
//! (`xcodebuild archive`), into Xcode's Archives folder, where the
//! Organizer lists it (`archives`). Distribute App exports it with an
//! `ExportOptions.plist` made from its choices (`export`), uploading to App
//! Store Connect when asked, and notarizes and staples Developer ID apps
//! (`notarize`). Both run like builds: their output in the build log,
//! errors in the Issue navigator, a report in the Reports navigator, Stop
//! ends them.

pub mod archives;
pub mod export;
pub mod notarize;

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use gpui_kit::{AppContext as _, AsyncApp, Context, WeakEntity};

use super::log::{Issue, IssueSource, Severity};
use super::process::ProcessHandle;
use super::xcode::{Destination, Platform};
use super::{BuildAction, BuildController, BuildEvent, Phase, Report, Sink, display_command, log_file, stream_into};
pub use archives::Archive;
pub use export::{ExportChoices, Method};
pub use notarize::Submission;

/// How the last Product ▸ Archive ended.
#[derive(Clone, Debug, Default)]
pub struct ArchiveOutcome {
    pub archive: Option<Archive>,
    pub error: Option<String>,
}

/// What Distribute App was asked to do.
#[derive(Clone, Debug)]
pub struct DistributeRequest {
    pub archive: Archive,
    pub choices: ExportChoices,
    /// Where the export goes (an upload's in zcode's cache).
    pub export_path: PathBuf,
    /// Developer ID: the keychain profile notarytool submits with.
    pub notarize: Option<String>,
}

/// How the last distribution ended.
#[derive(Clone, Debug)]
pub struct DistributionOutcome {
    pub archive: PathBuf,
    pub method: Method,
    pub uploaded: bool,
    pub export_path: PathBuf,
    /// What the export left (apps, `.ipa`s, packages).
    pub products: Vec<PathBuf>,
    pub notarization: Option<Submission>,
    pub stapled: bool,
    pub error: Option<String>,
}

/// Archives are built for a generic device: a simulator's platform's
/// devices, the Mac for My Mac; or the first platform the scheme supports
/// (`SUPPORTED_PLATFORMS`, when known) if the destination's isn't one.
pub fn destination_for(destination: &Destination, supported: &[String]) -> Destination {
    let generic = generic_for(destination);
    let supports = |platform: Platform| supported.iter().any(|s| s == platform.sdk());
    if supported.is_empty() || supports(generic.platform) {
        return generic;
    }
    match std::iter::once(Platform::MacOS).chain(Platform::DEVICES).find(|p| supports(*p)) {
        Some(Platform::MacOS) => Destination::any_mac(),
        Some(platform) => Destination::any_device(platform),
        None => generic,
    }
}

fn generic_for(destination: &Destination) -> Destination {
    let platform = match destination.platform {
        Platform::IOSSimulator => Platform::IOS,
        Platform::WatchOSSimulator => Platform::WatchOS,
        Platform::TvOSSimulator => Platform::TvOS,
        Platform::VisionOSSimulator => Platform::VisionOS,
        platform => platform,
    };
    match platform {
        Platform::MacOS => Destination::any_mac(),
        platform => Destination::any_device(platform),
    }
}

impl BuildController {
    /// Product ▸ Archive.
    pub fn archive(&mut self, cx: &mut Context<Self>) {
        self.start(BuildAction::Archive, cx);
    }

    /// Distribute App: exports (or uploads) an archive, then notarizes a
    /// Developer ID app when asked.
    pub fn distribute(&mut self, request: DistributeRequest, cx: &mut Context<Self>) {
        self.generation += 1;
        if self.phase.is_busy() {
            self.end_recording();
            self.stop(cx);
        }
        let started = Instant::now();
        self.log.clear();
        self.issues.clear();
        self.distributing = Some(request.clone());
        self.phase = Phase::Building { action: BuildAction::Distribute, started };
        cx.emit(BuildEvent::IssuesChanged);
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
        let handle = ProcessHandle::default();
        self.process = handle.clone();
        let cache_dir = self.cache_dir.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let outcome = run_distribution(&this, cx, &request, handle, &cache_dir).await;
            let (raw, outcome) = outcome;
            let log_path = log_file(&cache_dir, BuildAction::Distribute);
            cx.background_spawn({
                let log_path = log_path.clone();
                async move {
                    if let Some(dir) = log_path.parent() {
                        std::fs::create_dir_all(dir).ok();
                    }
                    std::fs::write(&log_path, raw).ok();
                }
            })
            .await;
            this.update(cx, |this, cx| {
                if this.phase == Phase::Stopped {
                    this.distributing = None;
                    return;
                }
                let success = outcome.error.is_none();
                if let Some(error) = &outcome.error
                    && this.error_count() == 0
                {
                    this.add_error(error.clone());
                }
                let what = if outcome.uploaded { "Uploaded" } else { "Exported" };
                if success {
                    let place = outcome.products.first().unwrap_or(&outcome.export_path);
                    this.push_log_line(match outcome.uploaded {
                        true => format!("{what} {} to App Store Connect ({})", request.archive.name, request.choices.method.title()),
                        false => format!("{what} {} to {}", request.archive.name, place.display()),
                    });
                }
                this.reports.insert(
                    0,
                    Report {
                        action: BuildAction::Distribute,
                        scheme: request.archive.name.clone(),
                        destination: request.choices.method.title().to_string(),
                        finished_at: SystemTime::now(),
                        duration: started.elapsed(),
                        success,
                        errors: this.error_count(),
                        warnings: this.warning_count(),
                        log_path,
                    },
                );
                this.last_distribution = Some(outcome);
                this.distributing = None;
                this.phase = Phase::Finished { action: BuildAction::Distribute, success, duration: started.elapsed() };
                cx.emit(BuildEvent::Distributed);
                cx.emit(BuildEvent::LogUpdated);
                cx.emit(BuildEvent::IssuesChanged);
                cx.emit(BuildEvent::PhaseChanged);
                cx.notify();
            })
            .ok();
        }));
    }

    /// An error in the Issue navigator (and the log), once.
    fn add_error(&mut self, message: String) {
        self.push_log_line(format!("error: {message}"));
        let issue = Issue { severity: Severity::Error, message, path: None, line: None, column: None, source: IssueSource::Build };
        if !self.issues.contains(&issue) {
            self.issues.push(issue);
        }
    }

    /// Export errors about signing point at the project settings.
    fn hint_signing_errors(&mut self) {
        for issue in self.issues.iter_mut().filter(|i| i.severity == Severity::Error) {
            if let Some(hinted) = export::signing_hint(&issue.message) {
                issue.message = hinted;
            }
        }
    }
}

/// After `xcodebuild archive`: the archive read and remembered (the
/// Organizer opens on it), or why there is none. Whether it was archived,
/// and the lines added to the log.
pub(super) async fn finish(this: &WeakEntity<BuildController>, cx: &mut AsyncApp, path: &Path, built: bool) -> (bool, String) {
    let read = match built {
        true => {
            let path = path.to_path_buf();
            cx.background_spawn(async move { archives::read(&path) }).await
        }
        false => Err(anyhow::anyhow!("not built")),
    };
    this.update(cx, |this, cx| {
        let (archived, line) = match read {
            Ok(archive) => {
                let version = archive.app.as_ref().map(|a| a.version_label()).unwrap_or_default();
                let line = format!("Archived {} {version} to {}", archive.name, path.display());
                this.push_log_line(line.clone());
                this.last_archive = Some(ArchiveOutcome { archive: Some(archive), error: None });
                cx.emit(BuildEvent::Archived(path.to_path_buf()));
                (true, line)
            }
            Err(err) => {
                let error = match built {
                    true => format!("xcodebuild left no archive at {}: {err:#}", path.display()),
                    false => this
                        .issues
                        .iter()
                        .find(|i| i.severity == Severity::Error)
                        .map(|i| i.message.clone())
                        .unwrap_or_else(|| "The archive failed (see the build log)".into()),
                };
                if built {
                    this.add_error(error.clone());
                }
                this.last_archive = Some(ArchiveOutcome { archive: None, error: Some(error.clone()) });
                (false, format!("error: {error}"))
            }
        };
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::IssuesChanged);
        cx.notify();
        (archived, line)
    })
    .unwrap_or((false, String::new()))
}

fn say(this: &WeakEntity<BuildController>, cx: &mut AsyncApp, line: String) {
    this.update(cx, |this, cx| {
        this.push_log_line(line);
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::IssuesChanged);
        cx.notify();
    })
    .ok();
}

/// Runs a step's command into the build log. Its exit code and output.
async fn step(this: &WeakEntity<BuildController>, cx: &mut AsyncApp, cmd: std::process::Command, handle: &ProcessHandle, raw: &mut String) -> (Option<i32>, String) {
    let line = format!("$ {}", display_command(&cmd));
    raw.push_str(&line);
    raw.push('\n');
    say(this, cx, line);
    let (code, output) = stream_into(this, cx, cmd, handle.clone(), Sink::Log).await;
    raw.push_str(&output);
    (code, output)
}

fn stopped(this: &WeakEntity<BuildController>, cx: &mut AsyncApp) -> bool {
    this.read_with(cx, |this, _| this.phase == Phase::Stopped).unwrap_or(true)
}

/// The export, then notarization: the raw log and how it ended.
async fn run_distribution(
    this: &WeakEntity<BuildController>,
    cx: &mut AsyncApp,
    request: &DistributeRequest,
    handle: ProcessHandle,
    cache_dir: &Path,
) -> (String, DistributionOutcome) {
    let choices = &request.choices;
    let uploading = choices.upload && choices.method.can_upload();
    let mut outcome = DistributionOutcome {
        archive: request.archive.path.clone(),
        method: choices.method,
        uploaded: false,
        export_path: request.export_path.clone(),
        products: vec![],
        notarization: None,
        stapled: false,
        error: None,
    };
    let mut raw = String::new();
    let options = cache_dir.join("ExportOptions.plist");
    let written = cx
        .background_spawn({
            let (options, plist, export_path) = (options.clone(), choices.plist(), request.export_path.clone());
            async move {
                std::fs::create_dir_all(&export_path)?;
                if let Some(dir) = options.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&options, plist)
            }
        })
        .await;
    if let Err(err) = written {
        outcome.error = Some(format!("Could not prepare the export: {err}"));
        return (raw, outcome);
    }
    say(this, cx, format!("{} {} with {}{}", if uploading { "Uploading" } else { "Exporting" }, request.archive.name, choices.method.title(), match uploading {
        true => " to App Store Connect".to_string(),
        false => format!(" to {}", request.export_path.display()),
    }));

    let (code, _) = step(this, cx, export::export_command(&request.archive.path, &request.export_path, &options), &handle, &mut raw).await;
    if stopped(this, cx) {
        return (raw, outcome);
    }
    this.update(cx, |this, cx| {
        this.hint_signing_errors();
        cx.emit(BuildEvent::IssuesChanged);
    })
    .ok();
    if code != Some(0) {
        outcome.error = Some(match uploading {
            true => "The upload failed (see the build log)".into(),
            false => "The export failed (see the build log)".into(),
        });
        return (raw, outcome);
    }
    outcome.uploaded = uploading;
    outcome.products = {
        let dir = request.export_path.clone();
        cx.background_spawn(async move { export::exported_products(&dir) }).await
    };

    let Some(profile) = request.notarize.clone().filter(|p| !p.trim().is_empty() && choices.method.can_notarize()) else {
        return (raw, outcome);
    };
    let Some(app) = outcome.products.iter().find(|p| p.extension().is_some_and(|e| e == "app")).cloned() else {
        outcome.error = Some("The export left no app to notarize".into());
        return (raw, outcome);
    };
    let name = app.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let zip = cache_dir.join("Notarization").join(format!("{}.zip", name.trim_end_matches(".app")));
    let made = cx
        .background_spawn({
            let zip = zip.clone();
            async move {
                std::fs::remove_file(&zip).ok();
                zip.parent().map(std::fs::create_dir_all).transpose()
            }
        })
        .await;
    if let Err(err) = made {
        outcome.error = Some(format!("Could not prepare notarization: {err}"));
        return (raw, outcome);
    }
    say(this, cx, format!("Notarizing {name} with the keychain profile “{profile}”…"));
    let (code, _) = step(this, cx, notarize::zip_command(&app, &zip), &handle, &mut raw).await;
    if stopped(this, cx) {
        return (raw, outcome);
    }
    if code != Some(0) {
        outcome.error = Some(format!("Could not zip {name} for the notary service"));
        return (raw, outcome);
    }
    let (_, output) = step(this, cx, notarize::submit_command(&zip, &profile), &handle, &mut raw).await;
    cx.background_spawn(async move { std::fs::remove_file(zip).ok() }).await;
    if stopped(this, cx) {
        return (raw, outcome);
    }
    let Some(submission) = notarize::parse_submission(&output) else {
        let last = output.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer").to_string();
        outcome.error = Some(notarize::credentials_hint(&output, &profile).unwrap_or_else(|| format!("notarytool: {last}")));
        return (raw, outcome);
    };
    outcome.notarization = Some(submission.clone());
    if !submission.accepted() {
        let (_, log) = step(this, cx, notarize::log_command(&submission.id, &profile), &handle, &mut raw).await;
        this.update(cx, |this, cx| {
            for (severity, message, path) in notarize::parse_log(&log) {
                let message = match path {
                    Some(path) => format!("{message} ({path})"),
                    None => message,
                };
                this.push_log_line(format!("{}: {message}", if severity == "warning" { "warning" } else { "error" }));
            }
            cx.emit(BuildEvent::IssuesChanged);
            cx.emit(BuildEvent::LogUpdated);
        })
        .ok();
        outcome.error = Some(format!("Apple’s notary service didn’t accept {name}: {} ({})", submission.status, submission.message));
        return (raw, outcome);
    }
    let (code, _) = step(this, cx, notarize::staple_command(&app), &handle, &mut raw).await;
    if code != Some(0) {
        outcome.error = Some(format!("{name} was notarized, but stapling its ticket failed (see the build log)"));
        return (raw, outcome);
    }
    outcome.stapled = true;
    say(this, cx, format!("Notarized and stapled {name}"));
    (raw, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_build_for_a_generic_device() {
        let simulator = Destination { platform: Platform::IOSSimulator, id: "SIM".into(), name: "iPhone 17".into(), ..Destination::my_mac() };
        let generic = destination_for(&simulator, &[]);
        assert_eq!((generic.platform, generic.xcodebuild_specifier()), (Platform::IOS, "generic/platform=iOS".to_string()));
        let mac = destination_for(&Destination::my_mac(), &[]);
        assert_eq!(mac.xcodebuild_specifier(), "generic/platform=macOS");
        let vision = destination_for(&Destination { platform: Platform::VisionOSSimulator, ..Destination::my_mac() }, &[]);
        assert_eq!(vision.platform, Platform::VisionOS);
        // A Mac app with a simulator still chosen (the list not narrowed yet).
        let mac_only = ["macosx".to_string()];
        assert_eq!(destination_for(&simulator, &mac_only).xcodebuild_specifier(), "generic/platform=macOS");
        let ios = ["iphoneos".to_string(), "iphonesimulator".to_string()];
        assert_eq!(destination_for(&simulator, &ios).platform, Platform::IOS);
        assert_eq!(destination_for(&Destination::my_mac(), &ios).platform, Platform::IOS);
    }
}

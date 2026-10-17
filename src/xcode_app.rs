//! Handing work to Xcode, the one `xcode-select` points at: Open in Xcode
//! (a project) and Show in Xcode Canvas (a file at a line, with the canvas
//! showing its SwiftUI previews).
//!
//! The project opens first (`xed <project>`), and the file only once the
//! project's window is up: a file that reaches Xcode before its project has
//! loaded opens in a window of its own, where the canvas can't build it
//! ("Active scheme does not build this file"). Then `xed --project
//! <project> --line <n> <file>` opens the file in the project's window with
//! line `n` selected (the canvas shows the file's previews as tabs; on a
//! file's first showing it picks the one the selection is in, after that
//! the tab last chosen). Xcode has no scripting command for the canvas, so
//! System Events chooses Editor ▸ Canvas (⌥⌘↩) once the file is in Xcode's
//! front window, unless it's checked already (Xcode remembers the canvas
//! per editor, and the item toggles), and Refresh Canvas when the canvas
//! waits paused. That needs zcode to be allowed to control the computer
//! (Privacy & Security ▸ Accessibility); without it the file still opens,
//! and the user is told to press ⌥⌘↩.
//!
//! `ZCODE_XCODE_COMMANDS=<file>` records each command as a JSON line (its
//! arguments) instead of running it, so the e2e suite never opens Xcode on
//! the screen.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::project::ProjectKind;

/// The environment variable that records commands instead of running them.
pub const COMMANDS_ENV: &str = "ZCODE_XCODE_COMMANDS";

/// How Show in Xcode Canvas went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasOutcome {
    /// The file is open in Xcode and the canvas was turned on.
    Shown,
    /// The file is open and the canvas was on already.
    AlreadyShown,
    /// The file is open, but the canvas couldn't be turned on (why).
    OpenedWithoutCanvas(String),
    /// Xcode couldn't open the file.
    Failed(String),
    /// `ZCODE_XCODE_COMMANDS` recorded the commands.
    Recorded,
}

impl CanvasOutcome {
    pub fn label(&self) -> String {
        match self {
            Self::Shown => "shown".into(),
            Self::AlreadyShown => "already shown".into(),
            Self::OpenedWithoutCanvas(why) => format!("opened without the canvas: {why}"),
            Self::Failed(why) => format!("failed: {why}"),
            Self::Recorded => "recorded".into(),
        }
    }
}

/// What `xed --project` opens for a project: the workspace or project, a
/// package's folder; nothing for a plain folder.
pub fn project_to_open(kind: &ProjectKind, root: &Path) -> Option<PathBuf> {
    match kind {
        ProjectKind::XcodeWorkspace(path) | ProjectKind::XcodeProject(path) => Some(path.clone()),
        ProjectKind::SwiftPackage => Some(root.to_path_buf()),
        ProjectKind::Folder => None,
    }
}

/// Opens a project (or workspace, or package folder) in Xcode.
pub fn open_project_command(project: &Path) -> Vec<String> {
    vec!["/usr/bin/xed".into(), project.display().to_string()]
}

/// Waits up to a minute for a window of the project (the script's
/// arguments: its bundle or folder name, and its title) in Xcode: titled
/// `HelloZ — ContentView.swift`, or showing `HelloZ.xcodeproj` itself.
/// Prints `open`, `denied: …` or `timed out`, and waits a moment more
/// when the window is new, for the project to load its files.
const PROJECT_WINDOW_SCRIPT: &str = r#"on run argv
    set bundleName to item 1 of argv
    set projectTitle to item 2 of argv
    set waited to false
    tell application "System Events"
        repeat 240 times
            repeat with xcodeProcess in (application processes whose bundle identifier is "com.apple.dt.Xcode")
                try
                    repeat with candidate in windows of xcodeProcess
                        set shownDocument to ""
                        try
                            set shownDocument to value of attribute "AXDocument" of candidate
                        end try
                        set windowName to name of candidate
                        if windowName is projectTitle or windowName starts with projectTitle & " — " or shownDocument ends with "/" & bundleName or shownDocument ends with "/" & bundleName & "/" then
                            if waited then delay 1
                            return "open"
                        end if
                    end repeat
                on error message number code
                    if code is in {-25211, -1743, -10004} or message contains "assistive access" then return "denied: " & message
                end try
            end repeat
            set waited to true
            delay 0.25
        end repeat
    end tell
    return "timed out"
end run"#;

/// The System Events step that waits for `project`'s window.
pub fn project_window_command(project: &Path) -> Vec<String> {
    let bundle = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let title = project.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    vec!["/usr/bin/osascript".into(), "-e".into(), PROJECT_WINDOW_SCRIPT.into(), bundle, title]
}

/// Opens `file` in Xcode at `line` (1-based), in `project`'s window.
pub fn xed_command(project: Option<&Path>, file: &Path, line: u32) -> Vec<String> {
    let mut args = vec!["/usr/bin/xed".to_string()];
    if let Some(project) = project {
        args.extend(["--project".into(), project.display().to_string()]);
    }
    args.extend(["--line".into(), line.max(1).to_string(), file.display().to_string()]);
    args
}

/// Waits up to a minute (Xcode may be launching and opening the project)
/// for the file (the script's arguments: its name and URL) to be in an
/// Xcode's front window, and turns on Editor ▸ Canvas unless it's on. A
/// project's first preview may wait, "Canvas paused", to be asked for:
/// then it chooses Editor ▸ Canvas ▸ Refresh Canvas (⌥⌘P). The menu has
/// two "Canvas" items, the toggle and the canvas's options; other editors
/// call the toggle "Preview". Prints `shown`, `already shown`, `denied: …`
/// or `timed out`. (-1719 is also "no window 1 yet": denials are told
/// apart by their message.)
const CANVAS_SCRIPT: &str = r#"on run argv
    set fileName to item 1 of argv
    set fileURL to item 2 of argv
    tell application "System Events"
        repeat 240 times
            repeat with xcodeProcess in (application processes whose bundle identifier is "com.apple.dt.Xcode")
                try
                    set frontWindow to window 1 of xcodeProcess
                    set shownDocument to ""
                    try
                        set shownDocument to value of attribute "AXDocument" of frontWindow
                    end try
                    if shownDocument is fileURL or name of frontWindow ends with fileName then
                        set editorMenu to menu 1 of menu bar item "Editor" of menu bar 1 of xcodeProcess
                        set toggleItem to missing value
                        repeat with candidate in (menu items of editorMenu whose name is "Canvas" or name is "Preview")
                            if toggleItem is missing value and not (exists menu 1 of candidate) then set toggleItem to contents of candidate
                        end repeat
                        if toggleItem is not missing value and enabled of toggleItem then
                            set outcome to "already shown"
                            if value of attribute "AXMenuItemMarkChar" of toggleItem is missing value then
                                click toggleItem
                                set outcome to "shown"
                            end if
                            repeat 6 times
                                delay 1
                                set paused to false
                                try
                                    -- A list, not `in (entire contents …)`, which is asked again for each item.
                                    set shownItems to entire contents of frontWindow
                                    repeat with shownItem in shownItems
                                        try
                                            if role of shownItem is "AXStaticText" and value of shownItem is "Canvas paused" then set paused to true
                                        end try
                                        if paused then exit repeat
                                    end repeat
                                end try
                                if paused then
                                    repeat with candidate in (menu items of editorMenu whose name is "Canvas")
                                        if exists menu 1 of candidate then click menu item "Refresh Canvas" of menu 1 of candidate
                                    end repeat
                                    return outcome
                                end if
                            end repeat
                            return outcome
                        end if
                    end if
                on error message number code
                    if code is in {-25211, -1743, -10004} or message contains "assistive access" then return "denied: " & message
                end try
            end repeat
            delay 0.25
        end repeat
    end tell
    return "timed out"
end run"#;

/// The System Events step of Show in Xcode Canvas.
pub fn canvas_command(file: &Path) -> Vec<String> {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let url = crate::lsp::path_to_uri(file).as_str().to_string();
    vec!["/usr/bin/osascript".into(), "-e".into(), CANVAS_SCRIPT.into(), name, url]
}

/// Appends `args` to the `ZCODE_XCODE_COMMANDS` file when it's set; `false`
/// when the command should run.
fn record(args: &[String]) -> bool {
    let Some(path) = std::env::var_os(COMMANDS_ENV) else { return false };
    let line = serde_json::to_string(args).unwrap_or_default();
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            let _ = writeln!(file, "{line}");
        }
        Err(err) => log::warn!("couldn’t record an Xcode command in {}: {err}", PathBuf::from(path).display()),
    }
    true
}

/// Runs `args`, returning its stdout, or stderr when it fails.
fn run(args: &[String]) -> Result<String, String> {
    let mut command = Command::new(&args[0]);
    command.args(&args[1..]).stdin(Stdio::null());
    crate::build::process::unblock_signals(&mut command);
    let output = command.output().map_err(|err| format!("couldn’t run {}: {err}", args[0]))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() { format!("{} exited with {}", args[0], output.status) } else { stderr })
    }
}

/// Show in Xcode Canvas: opens `file` at `line` (1-based) in Xcode with
/// `project`, then turns on the canvas. Blocking (up to about a minute
/// while Xcode launches): call it from a background thread.
pub fn show_in_canvas(project: Option<&Path>, file: &Path, line: u32) -> CanvasOutcome {
    let open = project.map(|p| (open_project_command(p), project_window_command(p)));
    let xed = xed_command(project, file, line);
    let canvas = canvas_command(file);
    if std::env::var_os(COMMANDS_ENV).is_some() {
        for args in open.iter().flat_map(|(open, wait)| [open, wait]).chain([&xed, &canvas]) {
            record(args);
        }
        return CanvasOutcome::Recorded;
    }
    // Why the canvas step is skipped, when the project's window never came.
    let mut skip_canvas = None;
    if let Some((open, wait)) = &open {
        if let Err(err) = run(open) {
            return CanvasOutcome::Failed(err);
        }
        if let CanvasOutcome::OpenedWithoutCanvas(why) = canvas_outcome(run(wait)) {
            skip_canvas = Some(why);
        }
    }
    if let Err(err) = run(&xed) {
        return CanvasOutcome::Failed(err);
    }
    match skip_canvas {
        Some(why) => CanvasOutcome::OpenedWithoutCanvas(why),
        None => canvas_outcome(run(&canvas)),
    }
}

/// What the System Events step's output means.
fn canvas_outcome(result: Result<String, String>) -> CanvasOutcome {
    match result.as_deref() {
        Ok("shown") => CanvasOutcome::Shown,
        Ok("already shown") => CanvasOutcome::AlreadyShown,
        Ok("open") => CanvasOutcome::AlreadyShown,
        Ok("timed out") => CanvasOutcome::OpenedWithoutCanvas("Xcode didn’t show it in time".into()),
        Ok(other) if other.starts_with("denied") => {
            CanvasOutcome::OpenedWithoutCanvas("zcode isn’t allowed to control Xcode (System Settings ▸ Privacy & Security ▸ Accessibility)".into())
        }
        Ok(other) => CanvasOutcome::OpenedWithoutCanvas(other.to_string()),
        // osascript reports a denial as an error too.
        Err(err) if ["-1719", "-25211", "-1743", "assistive access", "not allowed"].iter().any(|code| err.contains(code)) => {
            CanvasOutcome::OpenedWithoutCanvas("zcode isn’t allowed to control Xcode (System Settings ▸ Privacy & Security ▸ Accessibility)".into())
        }
        Err(err) => CanvasOutcome::OpenedWithoutCanvas(err.to_string()),
    }
}

/// The Xcode `xcode-select` points at, else the one Launch Services knows.
fn xcode_app() -> Vec<String> {
    let app = Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .and_then(|dev| dev.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf));
    match app {
        Some(app) => vec!["-a".into(), app.display().to_string()],
        None => vec!["-b".into(), "com.apple.dt.Xcode".into()],
    }
}

/// Open in Xcode: opens a project (or workspace, or package) in Xcode.
pub fn open_project(path: &Path) {
    let args: Vec<String> = std::iter::once("/usr/bin/open".to_string())
        .chain(xcode_app())
        .chain([path.display().to_string()])
        .collect();
    if record(&args) {
        return;
    }
    if let Err(err) = run(&args) {
        log::warn!("couldn’t open {} in Xcode: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasOutcome, canvas_command, canvas_outcome, open_project_command, project_to_open, project_window_command, xed_command};
    use crate::project::ProjectKind;
    use std::path::{Path, PathBuf};

    #[test]
    fn xed_opens_the_project_then_the_file_at_the_line() {
        let project = PathBuf::from("/p/App.xcodeproj");
        assert_eq!(
            xed_command(Some(&project), Path::new("/p/App/ContentView.swift"), 12),
            ["/usr/bin/xed", "--project", "/p/App.xcodeproj", "--line", "12", "/p/App/ContentView.swift"]
        );
        assert_eq!(xed_command(None, Path::new("/f/A.swift"), 0), ["/usr/bin/xed", "--line", "1", "/f/A.swift"]);
    }

    #[test]
    fn what_xed_opens_first() {
        let root = Path::new("/p");
        assert_eq!(project_to_open(&ProjectKind::XcodeProject("/p/A.xcodeproj".into()), root), Some("/p/A.xcodeproj".into()));
        assert_eq!(project_to_open(&ProjectKind::XcodeWorkspace("/p/A.xcworkspace".into()), root), Some("/p/A.xcworkspace".into()));
        assert_eq!(project_to_open(&ProjectKind::SwiftPackage, root), Some("/p".into()));
        assert_eq!(project_to_open(&ProjectKind::Folder, root), None);
    }

    #[test]
    fn the_project_opens_first_and_its_window_is_waited_for() {
        let project = Path::new("/p/HelloZ.xcodeproj");
        assert_eq!(open_project_command(project), ["/usr/bin/xed", "/p/HelloZ.xcodeproj"]);
        let wait = project_window_command(project);
        assert!(wait[2].starts_with("on run argv") && wait[2].contains("windowName starts with projectTitle"));
        assert_eq!(&wait[3..], ["HelloZ.xcodeproj", "HelloZ"]);
        // A package's folder.
        assert_eq!(&project_window_command(Path::new("/p/ZKit"))[3..], ["ZKit", "ZKit"]);
    }

    #[test]
    fn the_canvas_script_is_given_the_file() {
        let args = canvas_command(Path::new("/p/My Views/ContentView.swift"));
        assert_eq!(args[0], "/usr/bin/osascript");
        assert!(args[2].starts_with("on run argv") && args[2].contains("\"Canvas\""));
        assert_eq!(&args[3..], ["ContentView.swift", "file:///p/My%20Views/ContentView.swift"]);
    }

    #[test]
    fn canvas_outcomes() {
        assert_eq!(canvas_outcome(Ok("shown".into())), CanvasOutcome::Shown);
        assert_eq!(canvas_outcome(Ok("already shown".into())), CanvasOutcome::AlreadyShown);
        let denied = canvas_outcome(Err("execution error: System Events got an error: osascript is not allowed assistive access. (-25211)".into()));
        assert!(matches!(denied, CanvasOutcome::OpenedWithoutCanvas(why) if why.contains("Accessibility")));
        assert!(matches!(canvas_outcome(Ok("timed out".into())), CanvasOutcome::OpenedWithoutCanvas(_)));
    }
}

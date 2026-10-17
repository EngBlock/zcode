//! Xcode's custom key bindings (`.idekeybindings` in
//! `~/Library/Developer/Xcode/UserData/KeyBindings`, only read) as
//! `keymap.json` blocks: each menu command Xcode lets you rebind that zcode
//! has gets its shortcut, and loses zcode's default ones, as a rebinding
//! does in Xcode (a command whose shortcut was removed just loses them);
//! text editing bindings (Cocoa's `moveWordLeft:` and so on) go to the
//! editor. Importing again replaces the blocks the last import wrote.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::plist;

/// Xcode's command (its action selector, and whether it's the menu item's
/// ⌥-alternate) → zcode's action.
const MENU_COMMANDS: &[(&str, bool, &str)] = &[
    ("openDocument:", false, "zcode::OpenProject"),
    ("openQuickly:", false, "zcode::OpenQuickly"),
    ("newFileFromTemplate:", false, "zcode::NewFile"),
    ("newEditorAreaSplitOnRight:", false, "zcode::NewEditorPaneOnRight"),
    ("newEditorAreaSplitBelow:", false, "zcode::NewEditorPaneBelow"),
    ("addFiles:", false, "zcode::AddFiles"),
    ("addPackages:", false, "zcode::AddPackageDependencies"),
    ("ide_saveDocument:", false, "zcode::Save"),
    ("saveAllEditorDocuments:", false, "zcode::SaveAll"),
    ("dvt_closeActiveEditorTab:", false, "zcode::CloseTab"),
    ("_closeOtherEditorTabs:", false, "zcode::CloseOtherTabs"),
    ("_closeActiveEditor:", false, "zcode::CloseEditorPane"),
    ("closeOtherEditors:", false, "zcode::CloseOtherEditorPanes"),
    ("performCloseWorkspace:", false, "zcode::CloseProject"),
    ("showInFinder:", false, "zcode::RevealInFinder"),
    ("openTerminalAtLocation:", false, "zcode::OpenTerminalAtLocation"),
    ("resolvePackageVersions:", false, "zcode::ResolvePackageVersions"),
    ("updateToLatestPackageVersions:", false, "zcode::UpdateToLatestPackageVersions"),
    ("showPreferencesPanel:", false, "zcode::OpenSettings"),
    ("undo:", false, "input::Undo"),
    ("redo:", false, "input::Redo"),
    ("toggleComments:", false, "zcode::ToggleComment"),
    ("duplicate:", false, "zcode::DuplicateLine"),
    ("moveCurrentLineUp:", false, "zcode::MoveLineUp"),
    ("moveCurrentLineDown:", false, "zcode::MoveLineDown"),
    ("selectNextOccurrence:", false, "zcode::SelectNextOccurrence"),
    ("addSelectedInnerDocumentLocationsToTray:", false, "zcode::BookmarkLine"),
    ("addSelectedInnerDocumentLocationsToTrayWithDialog:", false, "zcode::BookmarkLineWithDialog"),
    ("addSelectedFileLocationsToTray:", false, "zcode::BookmarkFile"),
    ("addSelectedFileLocationsToTrayWithDialog:", false, "zcode::BookmarkFileWithDialog"),
    ("find:", false, "zcode::FindInFile"),
    ("findAndReplace:", false, "zcode::FindAndReplaceInFile"),
    ("findInWorkspace:", false, "zcode::FindInWorkspace"),
    ("findAndReplaceInWorkspace:", false, "zcode::FindAndReplaceInWorkspace"),
    ("findSelectedSymbolInWorkspace:", false, "zcode::FindSelectedSymbol"),
    ("showCallHierarchy:", false, "zcode::FindCallHierarchy"),
    ("findAncestorTypes:", false, "zcode::FindAncestorTypes"),
    ("findDescendentTypes:", false, "zcode::FindDescendentTypes"),
    ("findConformingTypes:", false, "zcode::FindConformingTypes"),
    ("editorContext_jumpToDefinition:", false, "zcode::JumpToDefinition"),
    ("editorContext_jumpToDefinitionWithAlternate:", false, "zcode::JumpToDefinitionInNextPane"),
    ("jumpToLine:", false, "zcode::JumpToLine"),
    ("topLevelEditorContext_goBackInHistoryByCommand:", false, "zcode::GoBack"),
    ("topLevelEditorContext_goForwardInHistoryByCommand:", false, "zcode::GoForward"),
    ("topLevelEditorContext_jumpToNextIssue:", false, "zcode::NextIssue"),
    ("topLevelEditorContext_jumpToPreviousIssue:", false, "zcode::PreviousIssue"),
    ("selectNextPlaceholder:", false, "zcode::NextPlaceholder"),
    ("selectPreviousPlaceholder:", false, "zcode::PreviousPlaceholder"),
    ("topLevelEditorContext_jumpToNextBookmark:", false, "zcode::NextBookmark"),
    ("topLevelEditorContext_jumpToPreviousBookmark:", false, "zcode::PreviousBookmark"),
    ("topLevelEditorContext_jumpToNextBookmarkInWorkspace:", false, "zcode::NextBookmarkInWorkspace"),
    ("topLevelEditorContext_jumpToPreviousBookmarkInWorkspace:", false, "zcode::PreviousBookmarkInWorkspace"),
    ("jumpToNextChange:", false, "zcode::NextChange"),
    ("jumpToPreviousChange:", false, "zcode::PreviousChange"),
    ("revealInProjectNavigator:", false, "zcode::RevealInNavigator"),
    ("_showNextEditorTab:", false, "zcode::NextTab"),
    ("_showPreviousEditorTab:", false, "zcode::PreviousTab"),
    ("jumpToNextCounterpart:", false, "zcode::JumpToCounterpart"),
    ("jumpToNextCounterpartWithAlternate:", false, "zcode::JumpToCounterpartInNextPane"),
    ("moveKeyboardFocusToNextEditor:", false, "zcode::FocusNextEditorPane"),
    ("moveKeyboardFocusToPreviousEditor:", false, "zcode::FocusPreviousEditorPane"),
    ("toggleStructureNavigator:", false, "zcode::ShowProjectNavigator"),
    ("toggleWorkingCopiesNavigator:", false, "zcode::ShowSourceControlNavigator"),
    ("toggleBookmarksNavigator:", false, "zcode::ShowBookmarksNavigator"),
    ("toggleFindNavigator:", false, "zcode::ShowFindNavigator"),
    ("toggleIssuesNavigator:", false, "zcode::ShowIssueNavigator"),
    ("toggleTestNavigator:", false, "zcode::ShowTestNavigator"),
    ("toggleDebuggerNavigator:", false, "zcode::ShowDebugNavigator"),
    ("toggleBreakpointsNavigator:", false, "zcode::ShowBreakpointNavigator"),
    ("toggleLogsNavigator:", false, "zcode::ShowReportNavigator"),
    ("toggleNavigator:", false, "zcode::ToggleNavigator"),
    ("toggleCodingAssistantNavigator:", false, "zcode::ShowCodingAssistant"),
    ("newCodingAssistant:", false, "zcode::NewConversation"),
    ("toggleDebuggerVisibility:", false, "zcode::ToggleDebugArea"),
    ("toggleInspectorArea:", false, "zcode::ToggleInspector"),
    ("activateConsole:", false, "zcode::ActivateConsole"),
    ("topLevelEditorContext_showDocumentItemsMenu:", false, "zcode::ShowDocumentItems"),
    ("showQuickActions:", false, "zcode::QuickActions"),
    ("increaseFontSize:", false, "zcode::IncreaseFontSize"),
    ("decreaseFontSize:", false, "zcode::DecreaseFontSize"),
    ("resetFontSize:", false, "zcode::ResetFontSize"),
    ("toggleWrapLines:", false, "zcode::ToggleSoftWrap"),
    ("toggleInvisibleCharactersShown:", false, "zcode::ToggleInvisibles"),
    ("toggleViMode:", false, "zcode::ToggleVimMode"),
    ("_toggleMaximizeEditor:", false, "zcode::FocusEditorPane"),
    ("_showEditorOnly:", false, "zcode::ShowEditorOnly"),
    ("_toggleShowAssistantMode:", false, "zcode::ShowAssistant"),
    ("formatFile:", false, "zcode::FormatFile"),
    ("indentSelection:", false, "zcode::ReIndent"),
    ("renameRefactor:", false, "zcode::Rename"),
    ("toggleTokenizedEditing:", false, "zcode::EditAllInScope"),
    ("fixAllIssues:", false, "zcode::FixIssue"),
    ("_toggleShowCodeReviewForEditor:", false, "zcode::CodeReview"),
    ("toggleAuthorsShown:", false, "zcode::ShowAuthors"),
    ("complete:", false, "input::ShowCompletions"),
    ("jumpToCallers:", false, "zcode::ShowCallers"),
    ("addAction:", false, "zcode::AddPlistItem"),
    ("toggleEditRawKeysAndValues:", false, "zcode::ToggleRawKeysAndValues"),
    ("buildActiveRunContext:", false, "zcode::Build"),
    ("runActiveRunContext:", false, "zcode::Run"),
    ("runActiveRunContext:", true, "zcode::RunWithOptions"),
    ("testActiveRunContext:", false, "zcode::Test"),
    ("testActiveRunContext:", true, "zcode::TestWithOptions"),
    ("testSelectedItems:", false, "zcode::TestSelectedItems"),
    ("testAgain:", false, "zcode::TestAgain"),
    ("clearTestResults:", false, "zcode::ClearTestResults"),
    ("cancelCurrentExecution:", false, "zcode::Stop"),
    ("cleanActiveRunContext:", false, "zcode::Clean"),
    ("chooseScheme:", false, "zcode::ChooseScheme"),
    ("selectNextRunContext:", false, "zcode::NextScheme"),
    ("selectPreviousRunContext:", false, "zcode::PreviousScheme"),
    ("editActiveRunContext:", false, "zcode::EditScheme"),
    ("chooseDestination:", false, "zcode::ChooseDestination"),
    ("pauseOrContinue:", false, "zcode::DebugContinue"),
    ("stepOver:", false, "zcode::StepOver"),
    ("stepInto:", false, "zcode::StepInto"),
    ("stepOut:", false, "zcode::StepOut"),
    ("toggleBreakpoints:", false, "zcode::ToggleBreakpointsActive"),
    ("toggleBreakpointAtCurrentLine:", false, "zcode::ToggleBreakpoint"),
    ("createSymbolicBreakpoint:", false, "zcode::CreateSymbolicBreakpoint"),
    ("createSwiftErrorBreakpoint:", false, "zcode::CreateSwiftErrorBreakpoint"),
    ("createExceptionBreakpoint:", false, "zcode::CreateExceptionBreakpoint"),
    ("deleteAllBreakpoints:", false, "zcode::DeleteAllBreakpoints"),
    ("clearConsole:", false, "zcode::ClearConsole"),
    ("commitChangesCommand:", false, "zcode::Commit"),
    ("pushCommand:", false, "zcode::Push"),
    ("fetchChangesCommand:", false, "zcode::FetchChanges"),
    ("refreshFileStatusCommand:", false, "zcode::RefreshFileStatus"),
];

/// Xcode's command identifiers for commands whose selector is shared
/// (`showInspectorWithChoiceFromSender:`).
const COMMAND_IDS: &[(&str, &str)] = &[("Xcode.IDEKit.CmdDefinition.Inspectors.FileInspector", "zcode::ShowFileInspector")];

/// Cocoa's text system selectors (Xcode's Text Key Bindings) → the
/// editor's actions.
const TEXT_COMMANDS: &[(&str, &str)] = &[
    ("moveUp:", "input::MoveUp"),
    ("moveDown:", "input::MoveDown"),
    ("moveLeft:", "input::MoveLeft"),
    ("moveRight:", "input::MoveRight"),
    ("moveBackward:", "input::MoveLeft"),
    ("moveForward:", "input::MoveRight"),
    ("moveWordLeft:", "input::MoveToPreviousWord"),
    ("moveWordBackward:", "input::MoveToPreviousWord"),
    ("moveWordRight:", "input::MoveToNextWord"),
    ("moveWordForward:", "input::MoveToNextWord"),
    ("moveToBeginningOfLine:", "input::MoveHome"),
    ("moveToLeftEndOfLine:", "input::MoveHome"),
    ("moveToBeginningOfParagraph:", "input::MoveHome"),
    ("moveToEndOfLine:", "input::MoveEnd"),
    ("moveToRightEndOfLine:", "input::MoveEnd"),
    ("moveToEndOfParagraph:", "input::MoveEnd"),
    ("moveToBeginningOfText:", "input::MoveHome"),
    ("moveToEndOfText:", "input::MoveEnd"),
    ("moveToBeginningOfTextAndModifySelection:", "input::SelectToStartOfLine"),
    ("moveToEndOfTextAndModifySelection:", "input::SelectToEndOfLine"),
    ("deleteToBeginningOfText:", "input::DeleteToBeginningOfLine"),
    ("deleteToEndOfText:", "input::DeleteToEndOfLine"),
    ("moveToBeginningOfDocument:", "input::MoveToStart"),
    ("moveToEndOfDocument:", "input::MoveToEnd"),
    ("pageUp:", "input::MovePageUp"),
    ("pageDown:", "input::MovePageDown"),
    ("scrollPageUp:", "input::MovePageUp"),
    ("scrollPageDown:", "input::MovePageDown"),
    ("moveUpAndModifySelection:", "input::SelectUp"),
    ("moveDownAndModifySelection:", "input::SelectDown"),
    ("moveLeftAndModifySelection:", "input::SelectLeft"),
    ("moveRightAndModifySelection:", "input::SelectRight"),
    ("moveBackwardAndModifySelection:", "input::SelectLeft"),
    ("moveForwardAndModifySelection:", "input::SelectRight"),
    ("moveWordLeftAndModifySelection:", "input::SelectToPreviousWordStart"),
    ("moveWordBackwardAndModifySelection:", "input::SelectToPreviousWordStart"),
    ("moveWordRightAndModifySelection:", "input::SelectToNextWordEnd"),
    ("moveWordForwardAndModifySelection:", "input::SelectToNextWordEnd"),
    ("moveToBeginningOfLineAndModifySelection:", "input::SelectToStartOfLine"),
    ("moveToLeftEndOfLineAndModifySelection:", "input::SelectToStartOfLine"),
    ("moveToEndOfLineAndModifySelection:", "input::SelectToEndOfLine"),
    ("moveToRightEndOfLineAndModifySelection:", "input::SelectToEndOfLine"),
    ("moveToBeginningOfDocumentAndModifySelection:", "input::SelectToStart"),
    ("moveToEndOfDocumentAndModifySelection:", "input::SelectToEnd"),
    ("deleteBackward:", "input::Backspace"),
    ("deleteForward:", "input::Delete"),
    ("deleteWordBackward:", "input::DeleteToPreviousWordStart"),
    ("deleteWordForward:", "input::DeleteToNextWordEnd"),
    ("deleteToBeginningOfLine:", "input::DeleteToBeginningOfLine"),
    ("deleteToEndOfLine:", "input::DeleteToEndOfLine"),
    ("deleteToEndOfParagraph:", "input::DeleteToEndOfLine"),
    ("selectAll:", "input::SelectAll"),
    ("copy:", "input::Copy"),
    ("cut:", "input::Cut"),
    ("paste:", "input::Paste"),
    ("undo:", "input::Undo"),
    ("redo:", "input::Redo"),
    ("insertTab:", "input::IndentInline"),
    ("insertBacktab:", "input::OutdentInline"),
    ("cancelOperation:", "input::Escape"),
    ("complete:", "input::ShowCompletions"),
    ("selectColumnUp:", "input::AddCursorAbove"),
    ("selectColumnDown:", "input::AddCursorBelow"),
];

/// Xcode's key bindings folder (`ZCODE_XCODE_USER_DATA` stands in for the
/// user data folder in tests).
pub fn bindings_dir() -> PathBuf {
    super::xcode_theme::user_data_dir().join("KeyBindings")
}

/// The key binding sets there, `Default` first.
pub fn find() -> Vec<PathBuf> {
    let mut sets: Vec<PathBuf> = std::fs::read_dir(bindings_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "idekeybindings"))
        .collect();
    sets.sort_by_key(|p| (p.file_stem().is_none_or(|s| s != "Default"), p.clone()));
    sets
}

/// An Xcode keyboard shortcut (`$@o`: `@` ⌘, `^` ⌃, `~` ⌥, `$` ⇧, then the
/// key, with AppKit's function-key characters) as gpui keystrokes.
pub fn keystroke(shortcut: &str) -> Option<String> {
    let mut modifiers = vec![];
    let mut chars = shortcut.chars().peekable();
    let mut key = None;
    while let Some(c) = chars.next() {
        // `\@`: the key is `@`, not ⌘.
        if c == '\\' && chars.peek().is_some() {
            key = chars.next();
            if chars.next().is_some() {
                return None;
            }
            break;
        }
        let modifier = match c {
            '^' => Some("ctrl"),
            '~' => Some("alt"),
            '$' => Some("shift"),
            '@' => Some("cmd"),
            _ => None,
        };
        // A modifier character as the last one is the key (`@^`: ⌘^).
        match modifier {
            Some(m) if chars.peek().is_some() => modifiers.push(m),
            _ => {
                key = Some(c);
                if chars.next().is_some() {
                    return None;
                }
            }
        }
    }
    let key = key?;
    let name: String = match key {
        '\u{F700}' => "up".into(),
        '\u{F701}' => "down".into(),
        '\u{F702}' => "left".into(),
        '\u{F703}' => "right".into(),
        c @ '\u{F704}'..='\u{F71A}' => format!("f{}", c as u32 - 0xF704 + 1),
        '\u{F728}' => "delete".into(),
        '\u{F729}' => "home".into(),
        '\u{F72B}' => "end".into(),
        '\u{F72C}' => "pageup".into(),
        '\u{F72D}' => "pagedown".into(),
        '\u{7F}' | '\u{8}' => "backspace".into(),
        '\r' | '\n' | '\u{3}' => "enter".into(),
        '\t' => "tab".into(),
        '\u{19}' => {
            modifiers.push("shift");
            "tab".into()
        }
        '\u{1B}' => "escape".into(),
        ' ' => "space".into(),
        c if c.is_ascii_uppercase() => {
            modifiers.push("shift");
            c.to_ascii_lowercase().to_string()
        }
        c if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) => return None,
        c => c.to_string(),
    };
    let mut ordered: Vec<&str> = ["ctrl", "alt", "shift", "cmd"].into_iter().filter(|m| modifiers.contains(m)).collect();
    ordered.push(&name);
    Some(ordered.join("-"))
}

/// Keystrokes with their modifiers in one order, to compare them.
fn canonical(keys: &str) -> String {
    keys.split_whitespace()
        .map(|stroke| {
            let (mods, key) = match stroke.strip_suffix("--") {
                Some(mods) => (mods, "-"),
                None => stroke.rsplit_once('-').unwrap_or(("", stroke)),
            };
            let mods: Vec<&str> = mods.split('-').filter(|m| !m.is_empty()).collect();
            let ordered: Vec<&str> = ["ctrl", "alt", "shift", "cmd"].into_iter().filter(|m| mods.contains(m)).collect();
            format!("{}{key}", ordered.iter().map(|m| format!("{m}-")).collect::<String>())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an import makes: the blocks to write, and what it left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    /// The global block (menu commands) and the editor's (text editing).
    pub blocks: Vec<Value>,
    pub bound: usize,
    pub skipped: Vec<String>,
}

/// Converts a key binding set. `defaults` are zcode's default bindings as
/// (keystrokes, action name), for the ones a rebinding takes away.
pub fn convert(path: &Path, defaults: &[(String, String)]) -> Result<Imported> {
    let entries = super::xcode_theme::read_dict(path)?;
    let section = |name: &str| entries.iter().find(|e| e.key == name).map(|e| &e.value.value);
    // Xcode writes an empty set until something is changed.
    if !entries.is_empty() && section("Menu Key Bindings").is_none() && section("Text Key Bindings").is_none() {
        return Err(anyhow!("{} isn’t an Xcode key bindings file", path.display()));
    }
    let marker = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut imported = Imported::default();

    // Menu commands: unbinds first, so a binding to the same keys wins.
    let mut unbinds = Map::new();
    let mut binds = Map::new();
    if let Some(plist::Value::Dict(menu)) = section("Menu Key Bindings")
        && let Some(plist::Value::Array(items)) = menu.iter().find(|e| e.key == "Key Bindings").map(|e| &e.value.value)
    {
        for item in items {
            let plist::Value::Dict(fields) = &item.value else { continue };
            let field = |key: &str| fields.iter().find(|e| e.key == key).and_then(|e| e.value.as_str()).unwrap_or("");
            let (selector, alternate, id, title) = (field("Action"), field("Alternate") == "YES", field("CommandID"), field("Title"));
            let shortcut = field("Keyboard Shortcut");
            let action = COMMAND_IDS
                .iter()
                .find(|(cmd, _)| *cmd == id)
                .map(|(_, action)| *action)
                .or_else(|| MENU_COMMANDS.iter().find(|(sel, alt, _)| *sel == selector && *alt == alternate).map(|(_, _, action)| *action));
            let Some(action) = action else {
                imported.skipped.push(format!("{} ({selector})", if title.is_empty() { selector } else { title }));
                continue;
            };
            let keys = if shortcut.is_empty() { None } else { keystroke(shortcut) };
            if keys.is_none() && !shortcut.is_empty() {
                imported.skipped.push(format!("{title}: shortcut {shortcut:?}"));
                continue;
            }
            let same_keys = |default: &str| keys.as_deref().is_some_and(|keys| canonical(keys) == canonical(default));
            for (default_keys, _) in defaults.iter().filter(|(k, a)| a == action && !same_keys(k)) {
                unbinds.insert(default_keys.clone(), json!(["zed::Unbind", action]));
            }
            if let Some(keys) = keys {
                binds.insert(keys, json!(action));
            }
            imported.bound += 1;
        }
    }
    let mut menu_bindings = unbinds;
    menu_bindings.extend(binds);

    // Text editing, in the editor.
    let mut text_bindings = Map::new();
    if let Some(plist::Value::Dict(text)) = section("Text Key Bindings")
        && let Some(plist::Value::Dict(keys)) = text.iter().find(|e| e.key == "Key Bindings").map(|e| &e.value.value)
    {
        text_entries(keys, "", &mut text_bindings, &mut imported);
    }

    if !menu_bindings.is_empty() {
        imported.blocks.push(json!({ "xcode_import": marker, "bindings": menu_bindings }));
    }
    if !text_bindings.is_empty() {
        imported.blocks.push(json!({ "xcode_import": marker, "context": "Editor", "bindings": text_bindings }));
    }
    Ok(imported)
}

fn text_entries(entries: &[plist::Entry], prefix: &str, out: &mut Map<String, Value>, imported: &mut Imported) {
    for entry in entries {
        let Some(keys) = keystroke(&entry.key) else {
            imported.skipped.push(format!("text binding {:?}", entry.key));
            continue;
        };
        let keys = if prefix.is_empty() { keys } else { format!("{prefix} {keys}") };
        match &entry.value.value {
            plist::Value::Dict(inner) => text_entries(inner, &keys, out, imported),
            plist::Value::String(selector) => match TEXT_COMMANDS.iter().find(|(sel, _)| sel == selector) {
                Some((_, action)) => {
                    out.insert(keys, json!(action));
                    imported.bound += 1;
                }
                None => imported.skipped.push(format!("{keys}: {selector}")),
            },
            _ => imported.skipped.push(format!("{keys}: a sequence of commands")),
        }
    }
}

/// keymap.json with `imported`'s blocks in place of the ones an import
/// from the same file wrote before (or after the user's blocks).
pub fn merge_into(text: &str, imported: &Imported, marker: &str) -> Result<String, super::jsonc::ParseError> {
    let existing: Vec<usize> = match super::jsonc::parse_value(text)? {
        Value::Array(blocks) => blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.get("xcode_import").and_then(Value::as_str) == Some(marker))
            .map(|(ix, _)| ix)
            .collect(),
        _ => vec![],
    };
    let mut text = text.to_string();
    let mut next = existing.iter().copied();
    let comment = format!("Imported from Xcode’s {marker}; importing it again replaces this.");
    for block in &imported.blocks {
        text = match next.next() {
            Some(ix) => super::jsonc::set_array_item(&text, ix, block, None)?,
            None => super::jsonc::set_array_item(&text, usize::MAX, block, Some(&comment))?,
        };
    }
    // Blocks the last import had and this one doesn't: emptied.
    for ix in next {
        text = super::jsonc::set_array_item(&text, ix, &json!({ "xcode_import": marker, "bindings": {} }), None)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xcode_shortcuts_as_keystrokes() {
        assert_eq!(keystroke("$@o").as_deref(), Some("shift-cmd-o"));
        assert_eq!(keystroke("^~@\u{F702}").as_deref(), Some("ctrl-alt-cmd-left"));
        assert_eq!(keystroke("@K").as_deref(), Some("shift-cmd-k"));
        assert_eq!(keystroke("\u{F709}").as_deref(), Some("f6"));
        assert_eq!(keystroke("@\r").as_deref(), Some("cmd-enter"));
        assert_eq!(keystroke("^@").as_deref(), Some("ctrl-@"));
        assert_eq!(keystroke("^\\@").as_deref(), Some("ctrl-@"));
        assert_eq!(keystroke("@'").as_deref(), Some("cmd-'"));
        assert_eq!(keystroke("@ab"), None);
        assert_eq!(canonical("cmd-alt-r"), canonical("alt-cmd-r"));
        assert_eq!(canonical("cmd--"), "cmd--");
        assert_eq!(keystroke(""), None);
    }

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>Menu Key Bindings</key>
	<dict>
		<key>Key Bindings</key>
		<array>
			<dict>
				<key>Action</key><string>buildActiveRunContext:</string>
				<key>Alternate</key><string>NO</string>
				<key>CommandID</key><string>Xcode.IDEKit.CmdDefinition.Build</string>
				<key>Keyboard Shortcut</key><string>$@b</string>
				<key>Title</key><string>Build</string>
			</dict>
			<dict>
				<key>Action</key><string>runActiveRunContext:</string>
				<key>Alternate</key><string>YES</string>
				<key>Keyboard Shortcut</key><string>~@r</string>
				<key>Title</key><string>Run…</string>
			</dict>
			<dict>
				<key>Action</key><string>openQuickly:</string>
				<key>Alternate</key><string>NO</string>
				<key>Keyboard Shortcut</key><string></string>
				<key>Title</key><string>Open Quickly…</string>
			</dict>
			<dict>
				<key>Action</key><string>captureViewHierarchy:</string>
				<key>Alternate</key><string>NO</string>
				<key>Keyboard Shortcut</key><string>^@v</string>
				<key>Title</key><string>Capture View Hierarchy</string>
			</dict>
		</array>
		<key>Version</key><integer>3</integer>
	</dict>
	<key>Text Key Bindings</key>
	<dict>
		<key>Key Bindings</key>
		<dict>
			<key>^$K</key><string>deleteToEndOfParagraph:</string>
			<key>^t</key><string>transpose:</string>
			<key>^x</key>
			<dict>
				<key>^e</key><string>moveToEndOfDocument:</string>
			</dict>
		</dict>
		<key>Version</key><integer>3</integer>
	</dict>
</dict>
</plist>
"#;

    #[test]
    fn a_key_binding_set_becomes_keymap_blocks() {
        let path = std::env::temp_dir().join(format!("zcode-keys-{}.idekeybindings", std::process::id()));
        std::fs::write(&path, FIXTURE).unwrap();
        let defaults: Vec<(String, String)> = [("cmd-b", "zcode::Build"), ("cmd-p", "zcode::OpenQuickly"), ("cmd-shift-o", "zcode::OpenQuickly"), ("cmd-alt-r", "zcode::RunWithOptions")]
            .iter()
            .map(|(k, a)| (k.to_string(), a.to_string()))
            .collect();
        let imported = convert(&path, &defaults).unwrap();
        assert_eq!(imported.bound, 5);
        assert_eq!(imported.skipped, ["Capture View Hierarchy (captureViewHierarchy:)", "ctrl-t: transpose:"]);
        let marker = path.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            imported.blocks[0],
            json!({ "xcode_import": marker, "bindings": {
                "cmd-b": ["zed::Unbind", "zcode::Build"],
                "cmd-p": ["zed::Unbind", "zcode::OpenQuickly"],
                "cmd-shift-o": ["zed::Unbind", "zcode::OpenQuickly"],
                "shift-cmd-b": "zcode::Build",
                "alt-cmd-r": "zcode::RunWithOptions",
            } })
        );
        assert_eq!(
            imported.blocks[1],
            json!({ "xcode_import": marker, "context": "Editor", "bindings": {
                "ctrl-shift-k": "input::DeleteToEndOfLine",
                "ctrl-x ctrl-e": "input::MoveToEnd",
            } })
        );
        // Into the user's keymap, then again in place of the first import.
        let mine = "[\n  // Mine.\n  { \"bindings\": { \"f5\": \"zcode::Run\" } }\n]\n";
        let once = merge_into(mine, &imported, &marker).unwrap();
        let blocks = super::super::jsonc::parse_value(&once).unwrap();
        assert_eq!(blocks.as_array().unwrap().len(), 3);
        assert!(once.contains("// Mine.") && once.contains("// Imported from Xcode’s"), "{once}");
        let twice = merge_into(&once, &imported, &marker).unwrap();
        assert_eq!(super::super::jsonc::parse_value(&twice).unwrap(), blocks);
        // What it writes is a keymap zcode reads.
        let (content, warnings) = <super::super::keymap::KeymapContent as super::super::Content>::read(&twice, false).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(content.blocks[1].xcode_import.as_deref(), Some(marker.as_str()));
        std::fs::remove_file(&path).ok();
    }

    /// The user's own sets convert (only read; may be empty).
    #[test]
    #[ignore]
    fn the_users_xcode_key_bindings_convert() {
        for path in find() {
            convert(&path, &[]).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        }
        let xcode = crate::toolchain::developer_dir().unwrap().parent().unwrap().join("Frameworks/IDEKit.framework/Versions/A/Resources/IDEDefaultKeyBindings.idekeybindings");
        let imported = convert(&xcode, &[]).unwrap();
        assert!(imported.bound > 5, "{imported:?}");
    }
}

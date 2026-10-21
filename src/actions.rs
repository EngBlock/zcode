//! Commands, their Xcode-compatible key bindings, and the menu bar.
//!
//! Shortcuts follow Xcode 27's defaults (read from its IDEKit command
//! definitions); a few Zed favorites are added where Xcode has none.

use zcode_editor::{Redo, ShowCompletions, ToggleCodeActions, Undo};
use gpui_kit::{Action, App, KeyBinding, Menu, MenuItem, NoAction, OsAction, SystemMenuType};

gpui_kit::actions!(
    zcode,
    [
        // App / File
        Quit,
        About,
        OpenSettings,
        OpenKeymap,
        OpenProjectSettingsFile,
        OpenDefaultSettings,
        OpenDefaultKeymap,
        SelectTheme,
        ImportXcodeTheme,
        ImportXcodeKeyBindings,
        // Packaging (`crate::updates`, `crate::cli`, `crate::login_item`).
        CheckForUpdates,
        InstallCli,
        ToggleOpenAtLogin,
        OpenProject,
        OpenQuickly,
        NewFile,
        AddFiles,
        AddPackageDependencies,
        ResetPackageCaches,
        ResolvePackageVersions,
        UpdateToLatestPackageVersions,
        Save,
        SaveAll,
        CloseTab,
        CloseOtherTabs,
        CloseProject,
        NewEditorPaneOnRight,
        NewEditorPaneBelow,
        CloseEditorPane,
        CloseOtherEditorPanes,
        RevealInFinder,
        OpenTerminalAtLocation,
        NewTerminalTab,
        ShowPerformance,
        ShowProjectSettings,
        // Edit
        CutOrCutLine,
        CopyOrCopyLine,
        PasteOrPasteLine,
        ToggleComment,
        DuplicateLine,
        MoveLineUp,
        MoveLineDown,
        SelectNextOccurrence,
        BookmarkLine,
        BookmarkLineWithDialog,
        BookmarkFile,
        BookmarkFileWithDialog,
        NextPlaceholder,
        PreviousPlaceholder,
        TabOrPlaceholder,
        // Predictive code completion
        ShowPredictiveCompletion,
        AcceptEditPrediction,
        DismissEditPrediction,
        // Find
        FindInWorkspace,
        FindSelectedSymbol,
        FindCallHierarchy,
        FindAncestorTypes,
        FindDescendentTypes,
        FindConformingTypes,
        FindInFile,
        FindAndReplaceInFile,
        FindAndReplaceInWorkspace,
        ShowFindResultsInEditor,
        ShowIssuesInEditor,
        OpenExcerpts,
        ExpandExcerpts,
        // Navigate
        JumpToDefinition,
        JumpToDefinitionInNextPane,
        JumpToLine,
        GoBack,
        GoForward,
        NextIssue,
        PreviousIssue,
        RevealInNavigator,
        NextTab,
        PreviousTab,
        JumpToCounterpart,
        JumpToCounterpartInNextPane,
        FocusNextEditorPane,
        FocusPreviousEditorPane,
        NextBookmark,
        PreviousBookmark,
        NextBookmarkInWorkspace,
        PreviousBookmarkInWorkspace,
        NextChange,
        PreviousChange,
        // View
        ShowProjectNavigator,
        ShowSourceControlNavigator,
        ShowBookmarksNavigator,
        ShowFindNavigator,
        ShowIssueNavigator,
        ShowTestNavigator,
        ShowReportNavigator,
        ToggleNavigator,
        ShowCodingAssistant,
        NewConversation,
        ToggleDebugArea,
        ShowFileInspector,
        ToggleInspector,
        ActivateConsole,
        ActivateTerminal,
        NewTerminal,
        ShowDocumentItems,
        QuickActions,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        ToggleSoftWrap,
        FocusEditorPane,
        // Editor
        ShowEditorOnly,
        ShowInXcodeCanvas,
        ShowAssistant,
        FormatFile,
        ReIndent,
        Rename,
        EditAllInScope,
        FixIssue,
        CodeReview,
        SideBySideComparison,
        InlineComparison,
        ShowAuthors,
        ToggleInlineBlame,
        ToggleInlayHints,
        ToggleVimMode,
        ToggleInvisibles,
        ShowCallers,
        ExpandAbbreviation,
        WrapWithAbbreviation,
        RestartLanguageServers,
        // Editor (property lists)
        AddPlistItem,
        DeletePlistItem,
        PlistExpand,
        PlistCollapse,
        SetPlistTypeArray,
        SetPlistTypeDictionary,
        SetPlistTypeBoolean,
        SetPlistTypeData,
        SetPlistTypeDate,
        SetPlistTypeNumber,
        SetPlistTypeString,
        ToggleRawKeysAndValues,
        OpenAsSourceCode,
        // A Markdown or SVG file's live preview (`workspace/rendered_view.rs`).
        OpenPreview,
        OpenPreviewToTheSide,
        OpenAsPropertyList,
        // Product
        Build,
        Run,
        Test,
        TestSelectedItems,
        TestAgain,
        RerunFailedTests,
        ClearTestResults,
        ChooseTestPlan,
        Stop,
        Clean,
        ChooseScheme,
        NextScheme,
        PreviousScheme,
        EditScheme,
        RunWithOptions,
        TestWithOptions,
        Profile,
        ProfileWithOptions,
        ProfileWithoutBuilding,
        BuildForProfiling,
        Archive,
        ShowOrganizer,
        ChooseDestination,
        RefreshDestinations,
        // Debug
        ClearConsole,
        ToggleBreakpoint,
        ToggleBreakpointsActive,
        DeleteAllBreakpoints,
        EditBreakpoint,
        CreateSymbolicBreakpoint,
        CreateSwiftErrorBreakpoint,
        CreateExceptionBreakpoint,
        DebugContinue,
        StepOver,
        StepInto,
        StepOut,
        ShowDebugNavigator,
        ShowBreakpointNavigator,
        // Integrate
        Commit,
        Push,
        Pull,
        FetchChanges,
        RefreshFileStatus,
        StageAllChanges,
        UnstageAllChanges,
        DiscardAllChanges,
        SwitchBranch,
        NewBranch,
        // Pickers
        PickerSelectNext,
        PickerSelectPrevious,
        PickerConfirm,
        PickerConfirmAlternate,
        PickerDismiss,
    ]
);

/// Context of focused code editors (see gpui-base's `Input mode=editor`).
const EDITOR: &str = "Input && mode == editor";
/// Any text input. Bindings here override the editor's own defaults for the
/// same keystroke because they are registered later at the same depth.
const INPUT: &str = "Input";
const PICKER_INPUT: &str = "Picker > Input";
/// The property list outline, and the key or value being typed in it.
const PLIST: &str = "PlistEditor";
const PLIST_INPUT: &str = "PlistEditor > Input";
const ASSET_CATALOG: &str = "AssetCatalog";
/// A multibuffer's editor (Find results, issues, references, a rename).
const MULTIBUFFER: &str = "Input && multibuffer";
/// A terminal (`src/terminal/view.rs`).
const TERMINAL: &str = "Terminal";
/// A code editor showing an edit prediction (Zed's context name).
const EDIT_PREDICTION: &str = "Input && mode == editor && edit_prediction";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(key_bindings());
}

fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        // App / File
        KeyBinding::new("cmd-q", Quit, None),
        // Xcode ▸ Settings…; zcode's opens settings.json, as Zed's does.
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("cmd-p", OpenQuickly, None),
        KeyBinding::new("cmd-shift-o", OpenQuickly, None),
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-alt-a", AddFiles, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-alt-s", SaveAll, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-alt-w", CloseOtherTabs, None),
        KeyBinding::new("ctrl-alt-cmd-w", CloseProject, None),
        // File ▸ New ▸ Editor Pane on Right / Below, Close Editor Pane and
        // Close Other Editor Panes (Xcode: ⌃⌘T, ⌃⌥⌘T, ⌃⇧⌘W, ⌃⌥⇧⌘W).
        KeyBinding::new("ctrl-cmd-t", NewEditorPaneOnRight, None),
        KeyBinding::new("ctrl-alt-cmd-t", NewEditorPaneBelow, None),
        KeyBinding::new("ctrl-shift-cmd-w", CloseEditorPane, None),
        KeyBinding::new("ctrl-alt-shift-cmd-w", CloseOtherEditorPanes, None),
        // VS Code's ⇧⌘V; its ⌘K V would hold up Clear Console's ⌘K, so the
        // side's is ⌃⇧⌘V (Xcode has neither).
        KeyBinding::new("shift-cmd-v", OpenPreview, None),
        KeyBinding::new("ctrl-shift-cmd-v", OpenPreviewToTheSide, None),
        // Edit
        KeyBinding::new("cmd-x", CutOrCutLine, Some(EDITOR)),
        KeyBinding::new("cmd-c", CopyOrCopyLine, Some(EDITOR)),
        KeyBinding::new("cmd-v", PasteOrPasteLine, Some(EDITOR)),
        KeyBinding::new("cmd-/", ToggleComment, Some(EDITOR)),
        KeyBinding::new("cmd-d", DuplicateLine, Some(EDITOR)),
        KeyBinding::new("alt-up", MoveLineUp, Some(EDITOR)),
        KeyBinding::new("alt-down", MoveLineDown, Some(EDITOR)),
        KeyBinding::new("cmd-alt-[", MoveLineUp, Some(EDITOR)),
        KeyBinding::new("cmd-alt-]", MoveLineDown, Some(EDITOR)),
        KeyBinding::new("cmd-alt-e", SelectNextOccurrence, Some(EDITOR)),
        KeyBinding::new("ctrl-/", NextPlaceholder, Some(EDITOR)),
        KeyBinding::new("ctrl-shift-/", PreviousPlaceholder, Some(EDITOR)),
        KeyBinding::new("tab", TabOrPlaceholder, Some(EDITOR)),
        KeyBinding::new("ctrl-space", ShowCompletions, Some(EDITOR)),
        // Editor ▸ Show Predictive Completion (Xcode: ⌃⌥Space); while a
        // prediction shows, Tab accepts it and ⌥] dismisses it (as Escape
        // does). Later than Tab's binding above, so it wins.
        KeyBinding::new("ctrl-alt-space", ShowPredictiveCompletion, Some(EDITOR)),
        KeyBinding::new("tab", AcceptEditPrediction, Some(EDIT_PREDICTION)),
        KeyBinding::new("alt-]", DismissEditPrediction, Some(EDIT_PREDICTION)),
        // Xcode's keys are ⌃⌘' and ⌃⇧⌘' (and ⌥ for the dialogs); with ⇧ the
        // key may arrive as `"`.
        KeyBinding::new("ctrl-cmd-'", BookmarkLine, None),
        KeyBinding::new("ctrl-alt-cmd-'", BookmarkLineWithDialog, None),
        KeyBinding::new("ctrl-shift-cmd-'", BookmarkFile, None),
        KeyBinding::new("ctrl-cmd-\"", BookmarkFile, None),
        KeyBinding::new("ctrl-alt-shift-cmd-'", BookmarkFileWithDialog, None),
        KeyBinding::new("ctrl-alt-cmd-\"", BookmarkFileWithDialog, None),
        // Find
        KeyBinding::new("cmd-shift-f", FindInWorkspace, None),
        KeyBinding::new("cmd-shift-f", FindInWorkspace, Some(INPUT)),
        KeyBinding::new("ctrl-shift-cmd-f", FindSelectedSymbol, None),
        KeyBinding::new("ctrl-shift-cmd-h", FindCallHierarchy, None),
        KeyBinding::new("cmd-alt-f", FindAndReplaceInFile, Some(EDITOR)),
        KeyBinding::new("alt-shift-cmd-f", FindAndReplaceInWorkspace, None),
        KeyBinding::new("alt-shift-cmd-f", FindAndReplaceInWorkspace, Some(INPUT)),
        // The editor handles ⌘F itself; this binding gives the Find menu its
        // shortcut and works when focus is elsewhere.
        KeyBinding::new("cmd-f", FindInFile, None),
        // Navigate
        KeyBinding::new("f12", JumpToDefinition, None),
        KeyBinding::new("ctrl-cmd-j", JumpToDefinition, None),
        // With ⌥ these open in the next pane, the assistant when there is one.
        KeyBinding::new("ctrl-alt-cmd-j", JumpToDefinitionInNextPane, None),
        KeyBinding::new("ctrl-alt-cmd-up", JumpToCounterpartInNextPane, None),
        // Move Focus to Next / Previous Editor Pane; with ⇧ the key may
        // arrive as `~`.
        KeyBinding::new("ctrl-`", FocusNextEditorPane, None),
        KeyBinding::new("ctrl-shift-`", FocusPreviousEditorPane, None),
        KeyBinding::new("ctrl-~", FocusPreviousEditorPane, None),
        KeyBinding::new("cmd-l", JumpToLine, None),
        KeyBinding::new("ctrl--", GoBack, None),
        KeyBinding::new("ctrl-cmd-left", GoBack, None),
        KeyBinding::new("ctrl-cmd-right", GoForward, None),
        KeyBinding::new("cmd-'", NextIssue, None),
        KeyBinding::new("cmd-shift-'", PreviousIssue, None),
        KeyBinding::new("cmd-\"", PreviousIssue, None),
        KeyBinding::new("cmd-shift-j", RevealInNavigator, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PreviousTab, None),
        KeyBinding::new("ctrl-cmd-up", JumpToCounterpart, None),
        KeyBinding::new("ctrl-cmd-down", JumpToCounterpart, None),
        // ⌃; and ⌃: (⌃⇧;), with ⌥ for In Workspace.
        KeyBinding::new("ctrl-;", NextBookmark, None),
        KeyBinding::new("ctrl-shift-;", PreviousBookmark, None),
        KeyBinding::new("ctrl-:", PreviousBookmark, None),
        KeyBinding::new("ctrl-alt-;", NextBookmarkInWorkspace, None),
        KeyBinding::new("ctrl-alt-shift-;", PreviousBookmarkInWorkspace, None),
        KeyBinding::new("ctrl-alt-:", PreviousBookmarkInWorkspace, None),
        // Next / Previous Change; with ⇧ the key may arrive as `|`.
        KeyBinding::new("ctrl-\\", NextChange, None),
        KeyBinding::new("ctrl-shift-\\", PreviousChange, None),
        KeyBinding::new("ctrl-|", PreviousChange, None),
        // View
        KeyBinding::new("cmd-1", ShowProjectNavigator, None),
        KeyBinding::new("cmd-2", ShowSourceControlNavigator, None),
        KeyBinding::new("cmd-3", ShowBookmarksNavigator, None),
        KeyBinding::new("cmd-4", ShowFindNavigator, None),
        KeyBinding::new("cmd-5", ShowIssueNavigator, None),
        KeyBinding::new("cmd-6", ShowTestNavigator, None),
        KeyBinding::new("cmd-9", ShowReportNavigator, None),
        // Xcode 27 gave ⌘0 to the Coding Assistant; Show Navigator has no key.
        KeyBinding::new("cmd-0", ShowCodingAssistant, None),
        KeyBinding::new("ctrl-cmd-h", NewConversation, None),
        KeyBinding::new("cmd-shift-y", ToggleDebugArea, None),
        KeyBinding::new("cmd-alt-1", ShowFileInspector, None),
        KeyBinding::new("cmd-shift-c", ActivateConsole, None),
        // Zed's terminal key (⌃`) is Xcode's Move Focus to Next Editor
        // Pane, so the terminal gets ⌃⌘`, which Xcode doesn't use.
        KeyBinding::new("ctrl-cmd-`", ActivateTerminal, None),
        KeyBinding::new("ctrl-6", ShowDocumentItems, None),
        KeyBinding::new("cmd-shift-p", QuickActions, None),
        KeyBinding::new("cmd-shift-a", QuickActions, None),
        KeyBinding::new("cmd-+", IncreaseFontSize, None),
        KeyBinding::new("cmd-=", IncreaseFontSize, None),
        KeyBinding::new("cmd--", DecreaseFontSize, None),
        KeyBinding::new("ctrl-cmd-0", ResetFontSize, None),
        KeyBinding::new("ctrl-shift-cmd-l", ToggleSoftWrap, None),
        KeyBinding::new("ctrl-shift-cmd-enter", FocusEditorPane, None),
        // Editor. ⌘↩ is Show Editor Only in Xcode, in the editor too.
        KeyBinding::new("cmd-enter", ShowEditorOnly, None),
        KeyBinding::new("cmd-enter", ShowEditorOnly, Some(EDITOR)),
        // Xcode's Canvas key: zcode shows previews in Xcode's canvas.
        KeyBinding::new("alt-cmd-enter", ShowInXcodeCanvas, None),
        KeyBinding::new("ctrl-alt-cmd-enter", ShowAssistant, None),
        KeyBinding::new("ctrl-shift-i", FormatFile, Some(EDITOR)),
        // Emmet (Xcode has none of its own on ⌃⌥E).
        KeyBinding::new("ctrl-alt-e", ExpandAbbreviation, Some(EDITOR)),
        KeyBinding::new("ctrl-i", ReIndent, Some(EDITOR)),
        KeyBinding::new("f2", Rename, Some(EDITOR)),
        KeyBinding::new("ctrl-cmd-e", EditAllInScope, Some(EDITOR)),
        KeyBinding::new("ctrl-alt-cmd-f", FixIssue, Some(EDITOR)),
        KeyBinding::new("alt-enter", ToggleCodeActions, Some(EDITOR)),
        // In a multibuffer (after the editor's, so they win): ⌥↩ opens the
        // file at the cursor and ⇧↩ shows more lines, as in Zed.
        KeyBinding::new("alt-enter", OpenExcerpts, Some(MULTIBUFFER)),
        KeyBinding::new("shift-enter", ExpandExcerpts, Some(MULTIBUFFER)),
        // Code Review isn't in Xcode 27's inventory (⌥⌘↩ is Canvas now);
        // ⌥⇧⌘↩ is its key in the Xcodes that have it.
        KeyBinding::new("alt-shift-cmd-enter", CodeReview, None),
        KeyBinding::new("ctrl-shift-cmd-a", ShowAuthors, None),
        // Product
        KeyBinding::new("cmd-b", Build, None),
        KeyBinding::new("cmd-r", Run, None),
        KeyBinding::new("cmd-u", Test, None),
        KeyBinding::new("ctrl-alt-cmd-u", TestSelectedItems, None),
        KeyBinding::new("ctrl-alt-cmd-g", TestAgain, None),
        KeyBinding::new("ctrl-alt-cmd-k", ClearTestResults, None),
        KeyBinding::new("cmd-.", Stop, None),
        KeyBinding::new("cmd-.", Stop, Some(INPUT)),
        KeyBinding::new("cmd-shift-k", Clean, None),
        KeyBinding::new("ctrl-0", ChooseScheme, None),
        KeyBinding::new("ctrl-cmd-]", NextScheme, None),
        KeyBinding::new("ctrl-cmd-[", PreviousScheme, None),
        KeyBinding::new("cmd-<", EditScheme, None),
        KeyBinding::new("cmd-shift-,", EditScheme, None),
        KeyBinding::new("cmd-alt-r", RunWithOptions, None),
        KeyBinding::new("cmd-alt-u", TestWithOptions, None),
        KeyBinding::new("cmd-i", Profile, None),
        KeyBinding::new("cmd-alt-i", ProfileWithOptions, None),
        KeyBinding::new("ctrl-cmd-i", ProfileWithoutBuilding, None),
        KeyBinding::new("cmd-shift-i", BuildForProfiling, None),
        KeyBinding::new("cmd-alt-shift-o", ShowOrganizer, None),
        KeyBinding::new("ctrl-shift-0", ChooseDestination, None),
        // Debug
        KeyBinding::new("cmd-k", ClearConsole, None),
        KeyBinding::new("cmd-\\", ToggleBreakpoint, None),
        KeyBinding::new("cmd-alt-\\", CreateSymbolicBreakpoint, None),
        KeyBinding::new("cmd-y", ToggleBreakpointsActive, None),
        KeyBinding::new("ctrl-cmd-y", DebugContinue, None),
        KeyBinding::new("f6", StepOver, None),
        KeyBinding::new("f7", StepInto, None),
        KeyBinding::new("f8", StepOut, None),
        KeyBinding::new("cmd-7", ShowDebugNavigator, None),
        KeyBinding::new("cmd-8", ShowBreakpointNavigator, None),
        // Integrate
        KeyBinding::new("cmd-alt-c", Commit, None),
        KeyBinding::new("cmd-alt-x", Pull, None),
        // Pickers
        KeyBinding::new("down", PickerSelectNext, Some(PICKER_INPUT)),
        KeyBinding::new("ctrl-n", PickerSelectNext, Some(PICKER_INPUT)),
        KeyBinding::new("up", PickerSelectPrevious, Some(PICKER_INPUT)),
        KeyBinding::new("ctrl-p", PickerSelectPrevious, Some(PICKER_INPUT)),
        KeyBinding::new("enter", PickerConfirm, Some(PICKER_INPUT)),
        KeyBinding::new("alt-enter", PickerConfirmAlternate, Some(PICKER_INPUT)),
        KeyBinding::new("escape", PickerDismiss, Some(PICKER_INPUT)),
        KeyBinding::new("escape", PickerDismiss, Some("PerfPanel")),
        KeyBinding::new("enter", PickerConfirm, Some("AddFilesDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("AddFilesDialog")),
        KeyBinding::new("enter", PickerConfirm, Some("AddPackageDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("AddPackageDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("EditSchemeDialog")),
        KeyBinding::new("enter", PickerConfirm, Some("BreakpointEditor")),
        KeyBinding::new("escape", PickerDismiss, Some("BreakpointEditor")),
        KeyBinding::new("enter", PickerConfirm, Some("BookmarkDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("BookmarkDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("BookmarkTitle > Input")),
        KeyBinding::new("escape", PickerDismiss, Some("HunkPopover")),
        KeyBinding::new("escape", PickerDismiss, Some("CommitPopover")),
        KeyBinding::new("escape", PickerDismiss, Some("CommitSheet")),
        KeyBinding::new("enter", PickerConfirm, Some("RemoteSheet")),
        KeyBinding::new("escape", PickerDismiss, Some("RemoteSheet")),
        // The property list editor: rows, and the key or value being typed.
        KeyBinding::new("cmd-z", Undo, Some(PLIST)),
        KeyBinding::new("cmd-shift-z", Redo, Some(PLIST)),
        KeyBinding::new("down", PickerSelectNext, Some(PLIST)),
        KeyBinding::new("up", PickerSelectPrevious, Some(PLIST)),
        KeyBinding::new("right", PlistExpand, Some(PLIST)),
        KeyBinding::new("left", PlistCollapse, Some(PLIST)),
        KeyBinding::new("enter", PickerConfirm, Some(PLIST)),
        KeyBinding::new("backspace", DeletePlistItem, Some(PLIST)),
        KeyBinding::new("delete", DeletePlistItem, Some(PLIST)),
        KeyBinding::new("cmd-alt-[", MoveLineUp, Some(PLIST)),
        KeyBinding::new("cmd-alt-]", MoveLineDown, Some(PLIST)),
        KeyBinding::new("alt-up", MoveLineUp, Some(PLIST)),
        KeyBinding::new("alt-down", MoveLineDown, Some(PLIST)),
        KeyBinding::new("down", PickerSelectNext, Some(PLIST_INPUT)),
        KeyBinding::new("up", PickerSelectPrevious, Some(PLIST_INPUT)),
        KeyBinding::new("enter", PickerConfirm, Some(PLIST_INPUT)),
        KeyBinding::new("escape", PickerDismiss, Some(PLIST_INPUT)),
        // The asset catalog editor: its edits' undo, the outline's rows.
        KeyBinding::new("cmd-z", Undo, Some(ASSET_CATALOG)),
        KeyBinding::new("cmd-shift-z", Redo, Some(ASSET_CATALOG)),
        KeyBinding::new("down", PickerSelectNext, Some(ASSET_CATALOG)),
        KeyBinding::new("up", PickerSelectPrevious, Some(ASSET_CATALOG)),
        KeyBinding::new("backspace", DeletePlistItem, Some(ASSET_CATALOG)),
        KeyBinding::new("delete", DeletePlistItem, Some(ASSET_CATALOG)),
        // In a terminal, ⌘C ⌘V ⌘A act on it; plain and ⌃ keys go to the
        // shell, so zcode's ⌃ and function-key commands step aside there.
        KeyBinding::new("cmd-c", CopyOrCopyLine, Some(TERMINAL)),
        KeyBinding::new("cmd-v", PasteOrPasteLine, Some(TERMINAL)),
        KeyBinding::new("cmd-a", zcode_editor::SelectAll, Some(TERMINAL)),
        // The LLDB console's history.
        KeyBinding::new("up", PickerSelectPrevious, Some("LldbConsole > Input")),
        KeyBinding::new("down", PickerSelectNext, Some("LldbConsole > Input")),
    ];
    bindings.extend(TERMINAL_KEYS.iter().map(|keys| KeyBinding::new(keys, NoAction, Some(TERMINAL))));
    bindings
}

/// zcode's keys without ⌘ that work everywhere, which a terminal leaves to
/// the shell: ⌃ with symbols and digits, F keys, and Tab and ⇧Tab (which
/// gpui-kit's root takes for moving focus; the shell completes with them).
const TERMINAL_KEYS: &[&str] = &[
    "tab", "shift-tab",
    "ctrl-;", "ctrl-shift-;", "ctrl-:", "ctrl-alt-;", "ctrl-alt-shift-;", "ctrl-alt-:", "ctrl-`", "ctrl-shift-`", "ctrl-~",
    "ctrl--", "ctrl-\\", "ctrl-shift-\\", "ctrl-|", "ctrl-6", "ctrl-0", "ctrl-shift-0", "f6", "f7", "f8", "f12",
];

/// A command shown in menus and in Quick Actions.
pub struct Command {
    pub menu: &'static str,
    pub name: &'static str,
    pub action: fn() -> Box<dyn Action>,
}

macro_rules! cmd {
    ($menu:literal, $name:literal, $action:expr) => {
        Command {
            menu: $menu,
            name: $name,
            action: || Box::new($action),
        }
    };
}

/// Every user-facing command, in menu order. `"-"` names are separators.
pub fn commands() -> Vec<Command> {
    vec![
        // The app menu: Xcode's Settings… (⌘,) opens the settings file, as in
        // Zed, with the keymap, themes and their Xcode importers.
        // Check for Updates… after About, Install CLI and Open at Login (the
        // login item: `zcode --background`) with the other app-wide items,
        // as Zed's app menu has them; Xcode updates through the App Store.
        cmd!("zcode", "Check for Updates…", CheckForUpdates),
        cmd!("zcode", "-", Quit),
        cmd!("zcode", "Settings…", OpenSettings),
        cmd!("zcode", "Key Bindings…", OpenKeymap),
        cmd!("zcode", "Select Theme…", SelectTheme),
        cmd!("zcode", "-", Quit),
        cmd!("zcode", "Import Xcode Theme…", ImportXcodeTheme),
        cmd!("zcode", "Import Xcode Key Bindings…", ImportXcodeKeyBindings),
        cmd!("zcode", "-", Quit),
        cmd!("zcode", "Install CLI", InstallCli),
        cmd!("zcode", "Open at Login", ToggleOpenAtLogin),
        cmd!("zcode", "-", Quit),
        cmd!("zcode", "Open Project Settings File", OpenProjectSettingsFile),
        cmd!("zcode", "Open Default Settings", OpenDefaultSettings),
        cmd!("zcode", "Open Default Key Bindings", OpenDefaultKeymap),
        cmd!("File", "Open…", OpenProject),
        cmd!("File", "Open Quickly…", OpenQuickly),
        cmd!("File", "-", Quit),
        // Xcode's File ▸ New ▸ Editor Pane on Right / Below.
        cmd!("File ▸ New", "Editor Pane on Right", NewEditorPaneOnRight),
        cmd!("File ▸ New", "Editor Pane Below", NewEditorPaneBelow),
        cmd!("File ▸ New", "Terminal Tab", NewTerminalTab),
        cmd!("File ▸ New", "Conversation", NewConversation),
        cmd!("File", "New File…", NewFile),
        cmd!("File", "Add Files…", AddFiles),
        cmd!("File", "Add Package Dependencies…", AddPackageDependencies),
        cmd!("File", "Save", Save),
        cmd!("File", "Save All", SaveAll),
        cmd!("File", "-", Quit),
        cmd!("File", "Close Tab", CloseTab),
        cmd!("File", "Close Other Tabs", CloseOtherTabs),
        cmd!("File", "Close Editor Pane", CloseEditorPane),
        cmd!("File", "Close Other Editor Panes", CloseOtherEditorPanes),
        cmd!("File", "Close Project", CloseProject),
        cmd!("File", "-", Quit),
        cmd!("File", "Show in Finder", RevealInFinder),
        cmd!("File", "Open Terminal at Location", OpenTerminalAtLocation),
        cmd!("File", "-", Quit),
        // Xcode's File ▸ Packages, in its order.
        cmd!("File ▸ Packages", "Reset Package Caches", ResetPackageCaches),
        cmd!("File ▸ Packages", "Resolve Package Versions", ResolvePackageVersions),
        cmd!("File ▸ Packages", "-", Quit),
        cmd!("File ▸ Packages", "Update to Latest Package Versions", UpdateToLatestPackageVersions),
        cmd!("Edit", "Undo", Undo),
        cmd!("Edit", "Redo", Redo),
        cmd!("Edit", "-", Quit),
        cmd!("Edit", "Comment Selection", ToggleComment),
        cmd!("Edit", "Duplicate", DuplicateLine),
        cmd!("Edit", "Move Line Up", MoveLineUp),
        cmd!("Edit", "Move Line Down", MoveLineDown),
        cmd!("Edit", "Select Next Occurrence", SelectNextOccurrence),
        cmd!("Edit", "-", Quit),
        // Xcode shows the dialog variants as ⌥-alternates.
        cmd!("Edit", "Bookmark Line", BookmarkLine),
        cmd!("Edit", "Bookmark Line With Dialog", BookmarkLineWithDialog),
        cmd!("Edit", "Bookmark File", BookmarkFile),
        cmd!("Edit", "Bookmark File With Dialog", BookmarkFileWithDialog),
        cmd!("Find", "Find…", FindInFile),
        cmd!("Find", "Find and Replace…", FindAndReplaceInFile),
        cmd!("Find", "-", Quit),
        cmd!("Find", "Find in Workspace…", FindInWorkspace),
        cmd!("Find", "Find and Replace in Workspace…", FindAndReplaceInWorkspace),
        cmd!("Find", "Find Selected Symbol in Workspace", FindSelectedSymbol),
        cmd!("Find", "Show Find Results in Editor", ShowFindResultsInEditor),
        cmd!("Find", "Find Call Hierarchy", FindCallHierarchy),
        cmd!("Find", "Find Ancestor Types", FindAncestorTypes),
        cmd!("Find", "Find Descendent Types", FindDescendentTypes),
        cmd!("Find", "Find Conforming Types", FindConformingTypes),
        cmd!("Navigate", "Jump to Definition", JumpToDefinition),
        cmd!("Navigate", "Jump to Definition in Assistant", JumpToDefinitionInNextPane),
        cmd!("Navigate", "Jump to Line…", JumpToLine),
        cmd!("Navigate", "Jump to Next Counterpart", JumpToCounterpart),
        cmd!("Navigate", "Jump to Next Counterpart in Assistant", JumpToCounterpartInNextPane),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Go Back", GoBack),
        cmd!("Navigate", "Go Forward", GoForward),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Jump to Next Issue", NextIssue),
        cmd!("Navigate", "Jump to Previous Issue", PreviousIssue),
        cmd!("Navigate", "Show Issues in Editor", ShowIssuesInEditor),
        cmd!("Navigate", "Jump to Next Placeholder", NextPlaceholder),
        cmd!("Navigate", "Jump to Previous Placeholder", PreviousPlaceholder),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Jump to Next Bookmark", NextBookmark),
        cmd!("Navigate", "Jump to Next Bookmark In Workspace", NextBookmarkInWorkspace),
        cmd!("Navigate", "Jump to Previous Bookmark", PreviousBookmark),
        cmd!("Navigate", "Jump to Previous Bookmark In Workspace", PreviousBookmarkInWorkspace),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Jump to Next Change", NextChange),
        cmd!("Navigate", "Jump to Previous Change", PreviousChange),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Reveal in Project Navigator", RevealInNavigator),
        cmd!("Navigate", "Show Next Tab", NextTab),
        cmd!("Navigate", "Show Previous Tab", PreviousTab),
        cmd!("Navigate", "Move Focus to Next Editor Pane", FocusNextEditorPane),
        cmd!("Navigate", "Move Focus to Previous Editor Pane", FocusPreviousEditorPane),
        cmd!("Editor", "Show Editor Only", ShowEditorOnly),
        cmd!("Editor", "Show in Xcode Canvas", ShowInXcodeCanvas),
        cmd!("Editor", "Assistant", ShowAssistant),
        cmd!("Editor", "Code Review", CodeReview),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Authors", ShowAuthors),
        cmd!("Editor", "Inline Blame", ToggleInlineBlame),
        cmd!("Editor", "Inlay Hints", ToggleInlayHints),
        cmd!("Editor", "Vim Mode", ToggleVimMode),
        cmd!("Editor", "Side by Side Comparison", SideBySideComparison),
        cmd!("Editor", "Inline Comparison", InlineComparison),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Show Completions", ShowCompletions),
        cmd!("Editor", "Show Predictive Completion", ShowPredictiveCompletion),
        cmd!("Editor", "Show Code Actions", ToggleCodeActions),
        cmd!("Editor", "Fix Issue", FixIssue),
        cmd!("Editor", "Show Callers…", ShowCallers),
        cmd!("Editor", "Open Excerpts", OpenExcerpts),
        cmd!("Editor", "Expand Excerpts", ExpandExcerpts),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Rename…", Rename),
        cmd!("Editor", "Edit All in Scope", EditAllInScope),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Re-Indent", ReIndent),
        cmd!("Editor", "Format File", FormatFile),
        cmd!("Editor ▸ Emmet", "Expand Abbreviation", ExpandAbbreviation),
        cmd!("Editor ▸ Emmet", "Wrap with Abbreviation…", WrapWithAbbreviation),
        cmd!("Editor", "Restart Language Servers", RestartLanguageServers),
        cmd!("Editor", "Wrap Lines", ToggleSoftWrap),
        cmd!("Editor", "Invisibles", ToggleInvisibles),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Increase Font Size", IncreaseFontSize),
        cmd!("Editor", "Decrease Font Size", DecreaseFontSize),
        cmd!("Editor", "Reset Font Size", ResetFontSize),
        cmd!("Editor", "-", Quit),
        // Xcode's property list Editor menu.
        cmd!("Editor", "Add Item", AddPlistItem),
        cmd!("Editor ▸ Value Type", "Array", SetPlistTypeArray),
        cmd!("Editor ▸ Value Type", "Dictionary", SetPlistTypeDictionary),
        cmd!("Editor ▸ Value Type", "-", Quit),
        cmd!("Editor ▸ Value Type", "Boolean", SetPlistTypeBoolean),
        cmd!("Editor ▸ Value Type", "Data", SetPlistTypeData),
        cmd!("Editor ▸ Value Type", "Date", SetPlistTypeDate),
        cmd!("Editor ▸ Value Type", "Number", SetPlistTypeNumber),
        cmd!("Editor ▸ Value Type", "String", SetPlistTypeString),
        cmd!("Editor", "Raw Keys and Values", ToggleRawKeysAndValues),
        // Xcode offers Open As on a file's context menu.
        cmd!("Editor ▸ Open As", "Source Code", OpenAsSourceCode),
        cmd!("Editor ▸ Open As", "Property List", OpenAsPropertyList),
        cmd!("Editor", "Open Preview", OpenPreview),
        cmd!("Editor", "Open Preview to the Side", OpenPreviewToTheSide),
        cmd!("View", "Show Project Navigator", ShowProjectNavigator),
        cmd!("View", "Show Source Control Navigator", ShowSourceControlNavigator),
        cmd!("View", "Show Bookmarks Navigator", ShowBookmarksNavigator),
        cmd!("View", "Show Find Navigator", ShowFindNavigator),
        cmd!("View", "Show Issue Navigator", ShowIssueNavigator),
        cmd!("View", "Show Test Navigator", ShowTestNavigator),
        cmd!("View", "Show Debug Navigator", ShowDebugNavigator),
        cmd!("View", "Show Breakpoint Navigator", ShowBreakpointNavigator),
        cmd!("View", "Show Report Navigator", ShowReportNavigator),
        cmd!("View", "-", Quit),
        cmd!("View", "Show Coding Assistant", ShowCodingAssistant),
        cmd!("View", "Show/Hide Navigator", ToggleNavigator),
        cmd!("View", "Show/Hide Debug Area", ToggleDebugArea),
        cmd!("View", "Show/Hide Inspector", ToggleInspector),
        cmd!("View", "Show File Inspector", ShowFileInspector),
        cmd!("View", "Activate Console", ActivateConsole),
        cmd!("View", "Activate Terminal", ActivateTerminal),
        cmd!("View", "New Terminal", NewTerminal),
        cmd!("View", "-", Quit),
        cmd!("View", "Focus Editor Pane", FocusEditorPane),
        cmd!("View", "Document Items…", ShowDocumentItems),
        cmd!("View", "Project Settings", ShowProjectSettings),
        cmd!("View", "Quick Actions…", QuickActions),
        cmd!("View", "Performance…", ShowPerformance),
        cmd!("Product", "Build", Build),
        cmd!("Product", "Run", Run),
        cmd!("Product", "Test", Test),
        // Xcode's ⌥-alternates of Run and Test: the scheme editor, then run.
        cmd!("Product", "Run…", RunWithOptions),
        cmd!("Product", "Test…", TestWithOptions),
        cmd!("Product", "Profile", Profile),
        // Xcode's opens the scheme's Profile action; zcode's asks for a template.
        cmd!("Product", "Profile…", ProfileWithOptions),
        cmd!("Product", "Archive", Archive),
        cmd!("Product", "Stop", Stop),
        cmd!("Product", "-", Quit),
        // Xcode's Product ▸ Perform Action.
        cmd!("Product", "Test Selected Items", TestSelectedItems),
        cmd!("Product", "Test Again", TestAgain),
        cmd!("Product", "Re-run Failed Tests", RerunFailedTests),
        cmd!("Product", "Profile Without Building", ProfileWithoutBuilding),
        cmd!("Product ▸ Build For", "Profiling", BuildForProfiling),
        cmd!("Product", "-", Quit),
        cmd!("Product", "Clean Build Folder", Clean),
        cmd!("Product", "Clear Test Results", ClearTestResults),
        cmd!("Product", "-", Quit),
        cmd!("Product ▸ Scheme", "Choose Scheme…", ChooseScheme),
        cmd!("Product ▸ Scheme", "Next Scheme", NextScheme),
        cmd!("Product ▸ Scheme", "Previous Scheme", PreviousScheme),
        cmd!("Product ▸ Scheme", "-", Quit),
        cmd!("Product ▸ Scheme", "Edit Scheme…", EditScheme),
        cmd!("Product", "Choose Destination…", ChooseDestination),
        cmd!("Product", "Choose Test Plan…", ChooseTestPlan),
        cmd!("Product", "Refresh Destinations", RefreshDestinations),
        cmd!("Debug", "Pause / Continue", DebugContinue),
        cmd!("Debug", "Step Over", StepOver),
        cmd!("Debug", "Step Into", StepInto),
        cmd!("Debug", "Step Out", StepOut),
        cmd!("Debug", "-", Quit),
        cmd!("Debug", "Activate/Deactivate Breakpoints", ToggleBreakpointsActive),
        cmd!("Debug ▸ Breakpoints", "Create Breakpoint at Current Line", ToggleBreakpoint),
        cmd!("Debug ▸ Breakpoints", "Create Symbolic Breakpoint…", CreateSymbolicBreakpoint),
        cmd!("Debug ▸ Breakpoints", "Create Swift Error Breakpoint", CreateSwiftErrorBreakpoint),
        cmd!("Debug ▸ Breakpoints", "Create Exception Breakpoint…", CreateExceptionBreakpoint),
        cmd!("Debug ▸ Breakpoints", "-", Quit),
        cmd!("Debug ▸ Breakpoints", "Edit Breakpoint…", EditBreakpoint),
        cmd!("Debug ▸ Breakpoints", "Delete All Breakpoints", DeleteAllBreakpoints),
        cmd!("Debug", "-", Quit),
        cmd!("Debug", "Clear Console", ClearConsole),
        // Xcode's Integrate menu, plus branches.
        cmd!("Integrate", "Commit…", Commit),
        cmd!("Integrate", "Push…", Push),
        cmd!("Integrate", "Pull…", Pull),
        cmd!("Integrate", "Fetch Changes", FetchChanges),
        cmd!("Integrate", "Refresh File Status", RefreshFileStatus),
        cmd!("Integrate", "-", Quit),
        cmd!("Integrate", "Stage All Changes", StageAllChanges),
        cmd!("Integrate", "Unstage All Changes", UnstageAllChanges),
        cmd!("Integrate", "Discard All Changes…", DiscardAllChanges),
        cmd!("Integrate", "-", Quit),
        cmd!("Integrate", "Switch Branch…", SwitchBranch),
        cmd!("Integrate", "New Branch…", NewBranch),
        cmd!("Window", "Organizer", ShowOrganizer),
    ]
}

fn menu_item(command: &Command, checked: &[&str]) -> MenuItem {
    if command.name == "-" {
        return MenuItem::separator();
    }
    let action = (command.action)();
    MenuItem::Action {
        name: command.name.into(),
        checked: checked.contains(&action.name()),
        action,
        os_action: match command.name {
            "Undo" => Some(OsAction::Undo),
            "Redo" => Some(OsAction::Redo),
            _ => None,
        },
        disabled: false,
    }
}

/// The Editor menu's toggles that are on, by action: the settings of the
/// window in front (or the user's, without one).
fn checked_actions(cx: &App) -> Vec<&'static str> {
    let settings = match cx.try_global::<MenuSettings>() {
        Some(MenuSettings(Some(settings), _)) => settings.clone(),
        _ => crate::settings::Settings::get(cx),
    };
    let toggles = [
        ("zcode::ShowAuthors", settings.show_authors),
        ("zcode::ToggleInlineBlame", settings.inline_blame),
        ("zcode::ToggleInlayHints", settings.inlay_hints),
        ("zcode::ToggleVimMode", settings.vim_mode),
        ("zcode::SideBySideComparison", settings.side_by_side_comparison),
        ("zcode::InlineComparison", !settings.side_by_side_comparison),
        ("zcode::ToggleSoftWrap", settings.soft_wrap),
        ("zcode::ToggleInvisibles", settings.show_whitespaces),
        ("zcode::ToggleOpenAtLogin", crate::login_item::is_enabled(cx)),
    ];
    toggles.into_iter().filter(|(_, on)| *on).map(|(name, _)| name).collect()
}

/// The front window's settings for the menus' checkmarks, and the
/// checkmarks the menu bar has.
#[derive(Default)]
struct MenuSettings(Option<crate::settings::Settings>, Option<Vec<&'static str>>);
impl gpui_kit::Global for MenuSettings {}

/// A project window came to the front, or its settings changed.
pub fn set_menu_settings(settings: crate::settings::Settings, cx: &mut App) {
    cx.default_global::<MenuSettings>().0 = Some(settings);
    refresh_menus(cx);
}

/// Builds the menu bar again if its checkmarks changed (or the key
/// bindings it shows did).
pub fn refresh_menus(cx: &mut App) {
    let checked = checked_actions(cx);
    if cx.try_global::<MenuSettings>().and_then(|m| m.1.as_ref()) != Some(&checked) {
        set_menus(cx);
    }
}

/// For the control channel's `dump`: what the menu bar shows checked.
pub fn menu_checkmarks(cx: &App) -> Vec<&'static str> {
    cx.try_global::<MenuSettings>().and_then(|m| m.1.clone()).unwrap_or_default()
}

pub fn set_menus(cx: &mut App) {
    let commands = commands();
    let checked = checked_actions(cx);
    let mut app_menu = vec![MenuItem::action("About zcode", About), MenuItem::separator()];
    app_menu.extend(commands.iter().filter(|c| c.menu == "zcode").map(|c| menu_item(c, &checked)));
    app_menu.extend([
        MenuItem::separator(),
        MenuItem::action("Performance…", ShowPerformance),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Quit zcode", Quit),
    ]);
    let mut menus = vec![Menu::new("zcode").items(app_menu)];
    for menu_name in ["File", "Edit", "Find", "Navigate", "Editor", "View", "Product", "Debug", "Integrate", "Window"] {
        let mut items: Vec<MenuItem> = Vec::new();
        if menu_name == "Edit" {
            // Native items keep the system text services (cut/copy/paste in
            // dialogs) working.
            items.push(MenuItem::os_action("Cut", CutOrCutLine, OsAction::Cut));
            items.push(MenuItem::os_action("Copy", CopyOrCopyLine, OsAction::Copy));
            items.push(MenuItem::os_action("Paste", PasteOrPasteLine, OsAction::Paste));
            items.push(MenuItem::os_action(
                "Select All",
                zcode_editor::SelectAll,
                OsAction::SelectAll,
            ));
            items.push(MenuItem::separator());
        }
        // Commands of a submenu (`"Product ▸ Scheme"`) go where its first one is.
        let submenu_of = |c: &Command| c.menu.strip_prefix(menu_name).and_then(|rest| rest.strip_prefix(" ▸ "));
        let mut submenus: Vec<&str> = Vec::new();
        for command in commands.iter().filter(|c| c.menu == menu_name || submenu_of(c).is_some()) {
            if let Some(submenu) = submenu_of(command) {
                if !submenus.contains(&submenu) {
                    submenus.push(submenu);
                    let entries = commands.iter().filter(|c| submenu_of(c) == Some(submenu)).map(|c| menu_item(c, &checked)).collect::<Vec<_>>();
                    items.push(MenuItem::submenu(Menu::new(submenu).items(entries)));
                }
                continue;
            }
            items.push(menu_item(command, &checked));
        }
        menus.push(Menu::new(menu_name).items(items));
    }
    cx.set_menus(menus);
    cx.default_global::<MenuSettings>().1 = Some(checked);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_gets_every_key_without_command() {
        let bindings = key_bindings();
        let left_to_the_shell: Vec<_> = TERMINAL_KEYS.iter().map(|keys| gpui_kit::Keystroke::parse(keys).unwrap()).collect();
        for binding in bindings.iter().filter(|b| b.predicate().is_none()) {
            let [keystroke] = binding.keystrokes() else { continue };
            let keystroke = gpui_kit::Keystroke::parse(&keystroke.unparse()).unwrap();
            if keystroke.modifiers.platform {
                continue;
            }
            assert!(left_to_the_shell.contains(&keystroke), "{} works everywhere but isn't left to terminals", keystroke.unparse());
        }
        // gpui-kit's root binds these to moving focus.
        for keys in ["tab", "shift-tab"] {
            assert!(left_to_the_shell.contains(&gpui_kit::Keystroke::parse(keys).unwrap()), "{keys} isn't left to terminals");
        }
    }
}

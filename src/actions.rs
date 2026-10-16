//! Commands, their Xcode-compatible key bindings, and the menu bar.
//!
//! Shortcuts follow Xcode 27's defaults (read from its IDEKit command
//! definitions); a few Zed favorites are added where Xcode has none.

use gpui_kit::component::input::{Redo, ShowCompletions, ToggleCodeActions, Undo};
use gpui_kit::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

gpui_kit::actions!(
    zcode,
    [
        // App / File
        Quit,
        About,
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
        CloseProject,
        RevealInFinder,
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
        NextPlaceholder,
        PreviousPlaceholder,
        TabOrPlaceholder,
        // Find
        FindInWorkspace,
        FindSelectedSymbol,
        FindInFile,
        FindAndReplaceInFile,
        // Navigate
        JumpToDefinition,
        JumpToLine,
        GoBack,
        GoForward,
        NextIssue,
        PreviousIssue,
        RevealInNavigator,
        NextTab,
        PreviousTab,
        JumpToCounterpart,
        // View
        ShowProjectNavigator,
        ShowSourceControlNavigator,
        ShowSymbolNavigator,
        ShowFindNavigator,
        ShowIssueNavigator,
        ShowTestNavigator,
        ShowReportNavigator,
        ToggleNavigator,
        ToggleDebugArea,
        ShowFileInspector,
        ToggleInspector,
        ActivateConsole,
        ShowDocumentItems,
        QuickActions,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        ToggleSoftWrap,
        // Editor
        FormatFile,
        ReIndent,
        Rename,
        EditAllInScope,
        FixIssue,
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
        // Pickers
        PickerSelectNext,
        PickerSelectPrevious,
        PickerConfirm,
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

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        // App / File
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("cmd-p", OpenQuickly, None),
        KeyBinding::new("cmd-shift-o", OpenQuickly, None),
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-alt-a", AddFiles, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-alt-s", SaveAll, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-alt-cmd-w", CloseProject, None),
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
        // Find
        KeyBinding::new("cmd-shift-f", FindInWorkspace, None),
        KeyBinding::new("cmd-shift-f", FindInWorkspace, Some(INPUT)),
        KeyBinding::new("ctrl-shift-cmd-f", FindSelectedSymbol, None),
        KeyBinding::new("cmd-alt-f", FindAndReplaceInFile, Some(EDITOR)),
        // The editor handles ⌘F itself; this binding gives the Find menu its
        // shortcut and works when focus is elsewhere.
        KeyBinding::new("cmd-f", FindInFile, None),
        // Navigate
        KeyBinding::new("f12", JumpToDefinition, None),
        KeyBinding::new("ctrl-cmd-j", JumpToDefinition, None),
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
        // View
        KeyBinding::new("cmd-1", ShowProjectNavigator, None),
        KeyBinding::new("cmd-2", ShowSourceControlNavigator, None),
        KeyBinding::new("cmd-3", ShowSymbolNavigator, None),
        KeyBinding::new("cmd-4", ShowFindNavigator, None),
        KeyBinding::new("cmd-5", ShowIssueNavigator, None),
        KeyBinding::new("cmd-6", ShowTestNavigator, None),
        KeyBinding::new("cmd-9", ShowReportNavigator, None),
        KeyBinding::new("cmd-0", ToggleNavigator, None),
        KeyBinding::new("cmd-shift-y", ToggleDebugArea, None),
        KeyBinding::new("cmd-alt-1", ShowFileInspector, None),
        KeyBinding::new("cmd-shift-c", ActivateConsole, None),
        KeyBinding::new("ctrl-6", ShowDocumentItems, None),
        KeyBinding::new("cmd-shift-p", QuickActions, None),
        KeyBinding::new("cmd-shift-a", QuickActions, None),
        KeyBinding::new("cmd-+", IncreaseFontSize, None),
        KeyBinding::new("cmd-=", IncreaseFontSize, None),
        KeyBinding::new("cmd--", DecreaseFontSize, None),
        KeyBinding::new("ctrl-cmd-0", ResetFontSize, None),
        KeyBinding::new("ctrl-shift-cmd-l", ToggleSoftWrap, None),
        // Editor
        KeyBinding::new("ctrl-shift-i", FormatFile, Some(EDITOR)),
        KeyBinding::new("ctrl-i", ReIndent, Some(EDITOR)),
        KeyBinding::new("f2", Rename, Some(EDITOR)),
        KeyBinding::new("ctrl-cmd-e", EditAllInScope, Some(EDITOR)),
        KeyBinding::new("ctrl-alt-cmd-f", FixIssue, Some(EDITOR)),
        KeyBinding::new("alt-enter", ToggleCodeActions, Some(EDITOR)),
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
        // Pickers
        KeyBinding::new("down", PickerSelectNext, Some(PICKER_INPUT)),
        KeyBinding::new("ctrl-n", PickerSelectNext, Some(PICKER_INPUT)),
        KeyBinding::new("up", PickerSelectPrevious, Some(PICKER_INPUT)),
        KeyBinding::new("ctrl-p", PickerSelectPrevious, Some(PICKER_INPUT)),
        KeyBinding::new("enter", PickerConfirm, Some(PICKER_INPUT)),
        KeyBinding::new("escape", PickerDismiss, Some(PICKER_INPUT)),
        KeyBinding::new("escape", PickerDismiss, Some("PerfPanel")),
        KeyBinding::new("enter", PickerConfirm, Some("AddFilesDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("AddFilesDialog")),
        KeyBinding::new("enter", PickerConfirm, Some("AddPackageDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("AddPackageDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("EditSchemeDialog")),
        KeyBinding::new("escape", PickerDismiss, Some("BreakpointEditor")),
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
        // The LLDB console's history.
        KeyBinding::new("up", PickerSelectPrevious, Some("LldbConsole > Input")),
        KeyBinding::new("down", PickerSelectNext, Some("LldbConsole > Input")),
    ]);
}

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
        cmd!("File", "Open…", OpenProject),
        cmd!("File", "Open Quickly…", OpenQuickly),
        cmd!("File", "-", Quit),
        cmd!("File", "New File…", NewFile),
        cmd!("File", "Add Files…", AddFiles),
        cmd!("File", "Add Package Dependencies…", AddPackageDependencies),
        cmd!("File", "Save", Save),
        cmd!("File", "Save All", SaveAll),
        cmd!("File", "-", Quit),
        cmd!("File", "Close Tab", CloseTab),
        cmd!("File", "Close Project", CloseProject),
        cmd!("File", "-", Quit),
        cmd!("File", "Show in Finder", RevealInFinder),
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
        cmd!("Find", "Find…", FindInFile),
        cmd!("Find", "Find and Replace…", FindAndReplaceInFile),
        cmd!("Find", "-", Quit),
        cmd!("Find", "Find in Workspace…", FindInWorkspace),
        cmd!("Find", "Find Selected Symbol in Workspace", FindSelectedSymbol),
        cmd!("Navigate", "Jump to Definition", JumpToDefinition),
        cmd!("Navigate", "Jump to Line…", JumpToLine),
        cmd!("Navigate", "Jump to Next Counterpart", JumpToCounterpart),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Go Back", GoBack),
        cmd!("Navigate", "Go Forward", GoForward),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Jump to Next Issue", NextIssue),
        cmd!("Navigate", "Jump to Previous Issue", PreviousIssue),
        cmd!("Navigate", "Jump to Next Placeholder", NextPlaceholder),
        cmd!("Navigate", "Jump to Previous Placeholder", PreviousPlaceholder),
        cmd!("Navigate", "-", Quit),
        cmd!("Navigate", "Reveal in Project Navigator", RevealInNavigator),
        cmd!("Navigate", "Show Next Tab", NextTab),
        cmd!("Navigate", "Show Previous Tab", PreviousTab),
        cmd!("Editor", "Show Completions", ShowCompletions),
        cmd!("Editor", "Show Code Actions", ToggleCodeActions),
        cmd!("Editor", "Fix Issue", FixIssue),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Rename…", Rename),
        cmd!("Editor", "Edit All in Scope", EditAllInScope),
        cmd!("Editor", "-", Quit),
        cmd!("Editor", "Re-Indent", ReIndent),
        cmd!("Editor", "Format File", FormatFile),
        cmd!("Editor", "Wrap Lines", ToggleSoftWrap),
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
        cmd!("View", "Show Project Navigator", ShowProjectNavigator),
        cmd!("View", "Show Source Control Navigator", ShowSourceControlNavigator),
        cmd!("View", "Show Symbol Navigator", ShowSymbolNavigator),
        cmd!("View", "Show Find Navigator", ShowFindNavigator),
        cmd!("View", "Show Issue Navigator", ShowIssueNavigator),
        cmd!("View", "Show Test Navigator", ShowTestNavigator),
        cmd!("View", "Show Debug Navigator", ShowDebugNavigator),
        cmd!("View", "Show Breakpoint Navigator", ShowBreakpointNavigator),
        cmd!("View", "Show Report Navigator", ShowReportNavigator),
        cmd!("View", "-", Quit),
        cmd!("View", "Show/Hide Navigator", ToggleNavigator),
        cmd!("View", "Show/Hide Debug Area", ToggleDebugArea),
        cmd!("View", "Show/Hide Inspector", ToggleInspector),
        cmd!("View", "Show File Inspector", ShowFileInspector),
        cmd!("View", "Activate Console", ActivateConsole),
        cmd!("View", "-", Quit),
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
        cmd!("Product", "Stop", Stop),
        cmd!("Product", "-", Quit),
        // Xcode's Product ▸ Perform Action.
        cmd!("Product", "Test Selected Items", TestSelectedItems),
        cmd!("Product", "Test Again", TestAgain),
        cmd!("Product", "Re-run Failed Tests", RerunFailedTests),
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
    ]
}

fn menu_item(command: &Command) -> MenuItem {
    if command.name == "-" {
        return MenuItem::separator();
    }
    MenuItem::Action {
        name: command.name.into(),
        action: (command.action)(),
        os_action: match command.name {
            "Undo" => Some(OsAction::Undo),
            "Redo" => Some(OsAction::Redo),
            _ => None,
        },
        checked: false,
        disabled: false,
    }
}

pub fn set_menus(cx: &mut App) {
    let mut menus = vec![Menu::new("zcode").items([
        MenuItem::action("About zcode", About),
        MenuItem::separator(),
        MenuItem::action("Performance…", ShowPerformance),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Quit zcode", Quit),
    ])];
    let commands = commands();
    for menu_name in ["File", "Edit", "Find", "Navigate", "Editor", "View", "Product", "Debug"] {
        let mut items: Vec<MenuItem> = Vec::new();
        if menu_name == "Edit" {
            // Native items keep the system text services (cut/copy/paste in
            // dialogs) working.
            items.push(MenuItem::os_action("Cut", CutOrCutLine, OsAction::Cut));
            items.push(MenuItem::os_action("Copy", CopyOrCopyLine, OsAction::Copy));
            items.push(MenuItem::os_action("Paste", PasteOrPasteLine, OsAction::Paste));
            items.push(MenuItem::os_action(
                "Select All",
                gpui_kit::component::input::SelectAll,
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
                    let entries = commands.iter().filter(|c| submenu_of(c) == Some(submenu)).map(menu_item).collect::<Vec<_>>();
                    items.push(MenuItem::submenu(Menu::new(submenu).items(entries)));
                }
                continue;
            }
            items.push(menu_item(command));
        }
        menus.push(Menu::new(menu_name).items(items));
    }
    menus.push(Menu::new("Window").items([]));
    cx.set_menus(menus);
}

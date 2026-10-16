pub(crate) mod text_boundary;
pub(crate) mod touch_selection;

/// What the input can offer to its context menu, at the moment it is opened.
///
/// Built by the input and read by the menu, the fields are private and reached
/// through the methods below, so that a new capability can be added without
/// breaking the menu builders.
///
/// ```
/// use zcode_editor::InputContextMenuCapabilities;
///
/// let capabilities = InputContextMenuCapabilities::new()
///     .code_editor(true)
///     .selection(true);
///
/// assert!(capabilities.is_editable());
/// assert!(capabilities.has_selection());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputContextMenuCapabilities {
    disabled: bool,
    readonly: bool,
    code_editor: bool,
    selection: bool,
    masked: bool,
    go_to_definition: bool,
    code_actions: bool,
}

impl InputContextMenuCapabilities {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn code_editor(mut self, code_editor: bool) -> Self {
        self.code_editor = code_editor;
        self
    }

    /// Set whether the input has a non-empty selection.
    pub fn selection(mut self, selection: bool) -> Self {
        self.selection = selection;
        self
    }

    /// Set whether the input renders its value masked.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn go_to_definition(mut self, go_to_definition: bool) -> Self {
        self.go_to_definition = go_to_definition;
        self
    }

    pub fn code_actions(mut self, code_actions: bool) -> Self {
        self.code_actions = code_actions;
        self
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Returns true if the user is allowed to change the text.
    ///
    /// The items that write to the input (Cut, Paste, Code Actions) belong to
    /// this, the reading ones (Copy, Go to Definition) do not.
    pub fn is_editable(&self) -> bool {
        !self.disabled && !self.readonly
    }

    pub fn is_code_editor(&self) -> bool {
        self.code_editor
    }

    pub fn has_selection(&self) -> bool {
        self.selection
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    /// Returns true if the user is allowed to copy the text out.
    ///
    /// A masked input keeps its value out of the clipboard, so Copy and Cut
    /// are both unavailable while the value is hidden.
    pub fn is_copyable(&self) -> bool {
        self.selection && !self.masked
    }

    pub fn has_definition(&self) -> bool {
        self.go_to_definition
    }

    pub fn has_code_actions(&self) -> bool {
        self.code_actions
    }
}

//! zcode's code editor.
//!
//! The engine (rope buffer, display map with soft wrap and folding,
//! highlighting, undo, multiple cursors, the LSP providers) started as
//! gpui-base 0.7.3's `input` module, and the styled element, popovers and
//! find panel as gpui-component 0.7.3's; both are Apache-2.0 (see `NOTICE`).
//! zcode owns this copy and builds its extension points on it: see
//! `docs/editor-engine.md`.
//!
//! The engine is generic over a mode marker because it began as the engine of
//! every text field; zcode only builds [`EditorState`], and the single-line
//! and textarea modes remain for the engine's own tests.
//!
//! Nothing here should be `pub` unless it is reachable from outside the crate.
#![warn(unreachable_pub)]

use gpui::App;

/// Character used by masked editor modes.
pub(crate) const MASK_CHAR: char = '•';

#[path = "editor/auto_close.rs"]
mod auto_close;
mod base;
#[path = "base/blink_cursor.rs"]
pub(crate) mod blink_cursor;
#[path = "base/change.rs"]
mod change;
#[path = "base/cursor.rs"]
mod cursor;
#[path = "editor/decorations.rs"]
mod decorations;
#[path = "editor/diagnostics.rs"]
mod diagnostics;
#[path = "editor/display_map/mod.rs"]
mod display_map;
mod editor;
#[path = "base/element.rs"]
mod element;
#[path = "editor/highlighting.rs"]
mod highlighting;
#[path = "editor/indent.rs"]
mod indent;
#[path = "base/inline_tokens.rs"]
mod inline_tokens;
mod input;
#[path = "base/token_presentation.rs"]
mod token_presentation;
pub use inline_tokens::{InlineToken, InlineTokenError, InlineTokenSpan, InputContent};
pub(crate) use token_presentation::InlineTokenPresentation;
pub use token_presentation::{
    InlineTokenClickEvent, InlineTokenClickListener, InlineTokenContext, InlineTokenHoverEvent,
    InlineTokenHoverListener, InlineTokenRenderer,
};
#[path = "base/kind.rs"]
mod kind;
#[path = "editor/language.rs"]
mod language;
#[path = "editor/language_config.rs"]
pub mod language_config;
#[path = "base/layout.rs"]
mod layout;
#[path = "editor/lsp/mod.rs"]
mod lsp;
#[path = "base/mask_pattern.rs"]
mod mask_pattern;
#[path = "base/mode.rs"]
mod mode;
#[path = "base/movement.rs"]
mod movement;
#[path = "base/native.rs"]
mod native;
#[path = "base/rope_ext.rs"]
mod rope_ext;
#[path = "editor/search.rs"]
mod search;
#[path = "base/selection.rs"]
mod selection;
#[path = "base/state.rs"]
mod state;
mod textarea;
#[path = "base/touch.rs"]
mod touch;
#[path = "base/undo_manager.rs"]
mod undo_manager;

mod anchors;
mod annotations;
mod gutter;
mod highlights;
mod edits;
mod history;
mod linked;
mod modal;
mod viewport;
mod vim;
pub mod multibuffer;
mod multibuffer_editor;
#[cfg(test)]
mod test_support;
mod ui;

/// Installs the editor's defaults. Call after `gpui_component::init`, whose
/// gpui-base layer binds the editing keys this editor shares.
pub fn init(cx: &mut App) {
    state::init(cx);
    ui::init(cx);
}

/// What an app's startup does before it opens an editor: gpui-component
/// (with gpui-base's key bindings and globals), then this crate.
#[cfg(test)]
pub(crate) fn test_init(cx: &mut App) {
    gpui_component::init(cx);
    init(cx);
}

pub use gpui_base::{NumberInputEvent, NumberStep};
pub use base::InputContextMenuCapabilities;
pub use anchors::{Anchor, AnchorBias, AnchorRow};
pub use annotations::{Block, BlockClicked, Inlay};
pub use gutter::{
    GutterEvent, GutterGeometry, GutterItem, GutterLane, GutterPaint, GutterPlacement, GutterRows,
    LineNumberLabels, PaintedGutterItem,
};
pub use cursor::Selection;
pub use edits::BufferEdit;
pub use highlights::HighlightPriority;
pub use history::{HistoryRequest, TransactionId};
pub use modal::{CursorShape, KeystrokeInterceptor};
pub use viewport::ViewportChanged;
pub use vim::{VimCommand, VimMode, VimStatus, VimStatusChanged};
pub use multibuffer_editor::{EXPAND_LINES, HeaderStyle, MultiBufferEditor, MultiBufferEvent};
pub use decorations::{
    RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, TextDecoration,
    TextDecorationCollection,
};
pub use diagnostics::{
    Diagnostic, DiagnosticEntry, DiagnosticRelatedInformation, DiagnosticSet, DiagnosticSeverity,
    DiagnosticSummary, DiagnosticTag, RelatedInformation,
};
pub use display_map::{BufferPoint, DisplayMap, DisplayPoint, FoldRange, WrappingIndent};
pub use editor::EditorState;
pub use ui::Editor;
pub use highlighting::{
    DiagnosticColors, FoldIconRenderer, HighlightStyleResolver, InputEditorStyle, InputHighlighter,
    InputHighlighterFactory, SharedHighlightStyleResolver, SyntaxContext, SyntaxContextProvider,
};
pub use indent::TabSize;
#[doc(hidden)]
pub use input::{Input, InputState};
pub use kind::{
    EditorExtras, EditorMode, InputExtras, InputMode, InputModeKind, MultiLineMode, TextareaMode,
};
pub(crate) use language::EditorLanguage;
pub use language::{LanguageProvider, set_language_config, set_language_provider};
pub(crate) use language_config::LanguageConfig;
pub use language_config::{AutoClosingPair, BracketPair, IndentationRules};
pub use lsp::{
    CodeActionItem, CodeActionMenuState, CodeActionProvider, CompletionMenuOptions,
    CompletionMenuState, CompletionProvider, DefinitionProvider, DocumentColorProvider,
    HoverPopoverState, HoverProvider, InputOverlayKind, Lsp, ShowDocumentHandler,
    SignatureHelpProvider, SignatureHelpState, split_signature_label,
};
pub use lsp_types::Position;
pub use mask_pattern::MaskPattern;
pub use native::{NativeMenu, NativeMenuItem};
pub use rope_ext::{InputEdit, Point, RopeExt, RopeLines};
pub use ropey::Rope;
pub use search::{SearchMatcher, SearchSession};
pub use state::*;
#[doc(hidden)]
pub use textarea::{Textarea, TextareaState};

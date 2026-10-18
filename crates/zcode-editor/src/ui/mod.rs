//! The editor's presentation: the styled element, its popovers and find
//! panel, and the bridge to gpui-component's tree-sitter highlighter.
//!
//! The popovers and the find panel keep the `pub` items they had in
//! gpui-component; only [`Editor`] leaves the crate.
#![allow(unreachable_pub)]

mod element;
mod highlighter;
mod language_config;
mod overlay;
mod popovers;
mod search;
mod syntax_context;

pub use element::Editor;
pub use highlighter::prepare_highlighting;

pub(crate) fn init(cx: &mut gpui::App) {
    language_config::init(cx);
}

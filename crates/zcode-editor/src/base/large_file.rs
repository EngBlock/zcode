//! Files too large for a syntax tree.
//!
//! tree-sitter's tree costs 25–60 times the text it parses (a 15 MB JSON
//! file: ~370 MB; a 3.3 MB minified script: ~190 MB; a 1.2 MB Swift file:
//! ~50 MB). Past these limits a code editor keeps its language (comments,
//! brackets, indentation, language servers) but builds no tree: no syntax
//! colors, folds or string-and-comment context. Decided once, when the
//! editor's highlighter would be made for the text it then has.

use ropey::{LineType, Rope};

/// Past this size a file gets no syntax tree.
pub const SYNTAX_TREE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// A file over this size that is mostly lines over [`MINIFIED_LINE_BYTES`]
/// gets no syntax tree either.
pub const MINIFIED_MIN_BYTES: usize = 256 * 1024;

/// Past this many bytes a line gets no syntax colors anyway, so a file made
/// of such lines (a minified script or style sheet, a JSON record per line)
/// would keep a tree nothing reads.
pub const MINIFIED_LINE_BYTES: usize = 10_000;

/// Why a file has no syntax tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoSyntaxTree {
    /// Over [`SYNTAX_TREE_MAX_BYTES`].
    Large,
    /// Over [`MINIFIED_MIN_BYTES`], most of it on lines over
    /// [`MINIFIED_LINE_BYTES`].
    Minified,
}

impl NoSyntaxTree {
    /// For `text`, or `None` when it gets a tree.
    pub fn for_text(text: &Rope) -> Option<Self> {
        let len = text.len();
        if len > SYNTAX_TREE_MAX_BYTES {
            return Some(Self::Large);
        }
        if len <= MINIFIED_MIN_BYTES {
            return None;
        }
        let lines = text.len_lines(LineType::LF);
        let mut on_long_lines = 0;
        let mut start = 0;
        for line in 1..=lines {
            let end = if line < lines { text.line_to_byte_idx(line, LineType::LF) } else { len };
            if end - start > MINIFIED_LINE_BYTES {
                on_long_lines += end - start;
            }
            start = end;
        }
        (on_long_lines * 2 > len).then_some(Self::Minified)
    }

    /// For the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Large => "Large file: no syntax highlighting",
            Self::Minified => "Minified: no syntax highlighting",
        }
    }

    /// For the status bar's tooltip and the control channel.
    pub fn explanation(self) -> String {
        let mb = SYNTAX_TREE_MAX_BYTES / (1024 * 1024);
        match self {
            Self::Large => format!("Files over {mb} MB open without a syntax tree: no syntax colors or folding"),
            Self::Minified => format!(
                "Files over {} KB made mostly of lines over {} bytes open without a syntax tree: no syntax colors or folding",
                MINIFIED_MIN_BYTES / 1024,
                MINIFIED_LINE_BYTES
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorState;
    use crate::test_support::EditorWindow;
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};

    /// A Rust editor on `text`, rendered and its background parse done.
    fn rust_editor(cx: &mut TestAppContext, text: String) -> (Entity<EditorState>, VisualTestContext) {
        let mut editor = None;
        let window = cx.open_window(size(px(800.), px(600.)), |window, cx| {
            cx.set_global(gpui_base::Theme::default());
            crate::test_init(cx);
            let state = cx.new(|cx| EditorState::new(window, cx).language("rust").default_value(text));
            editor = Some(state.clone());
            EditorWindow(state)
        });
        let cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        (editor.unwrap(), cx)
    }

    const BLOCK: &str = "fn f() {\n    let a = (1, 2);\n    a.0\n}\n";

    #[gpui::test]
    fn an_editor_on_a_large_file_builds_no_tree(app: &mut TestAppContext) {
        let (small, mut cx) = rust_editor(app, BLOCK.repeat(100));
        cx.update(|_, cx| {
            let small = small.read(cx);
            assert_eq!(small.no_syntax_tree(), None);
            assert_eq!(small.display_map.fold_candidates().len(), 100);
        });

        let (large, mut cx) = rust_editor(app, BLOCK.repeat(SYNTAX_TREE_MAX_BYTES / BLOCK.len() + 1));
        cx.update(|_, cx| {
            let large = large.read(cx);
            assert_eq!(large.no_syntax_tree(), Some(NoSyntaxTree::Large));
            // No folds, and no string or comment context: brackets, comments
            // and indentation go by the language's rules alone.
            assert!(large.display_map.fold_candidates().is_empty());
            assert_eq!(large.syntax_context_at(BLOCK.find("1, 2").unwrap()), crate::SyntaxContext::Code);
        });
        // Editing it parses nothing either; replacing the whole text with a
        // small one decides again.
        cx.update(|window, cx| {
            large.update(cx, |state, cx| state.insert("// typed\n", window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(large.read(cx).no_syntax_tree(), Some(NoSyntaxTree::Large));
            large.update(cx, |state, cx| state.set_value(BLOCK, window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(large.read(cx).no_syntax_tree(), None));
    }

    #[test]
    fn small_files_get_a_tree() {
        assert_eq!(NoSyntaxTree::for_text(&Rope::from_str("let a = 1\n")), None);
        // A small minified file still gets one.
        assert_eq!(NoSyntaxTree::for_text(&Rope::from_str(&"a=1;".repeat(20_000))), None);
    }

    #[test]
    fn large_files_get_none() {
        let line = "let value = compute(1, 2, 3)\n";
        let text = Rope::from_str(&line.repeat(SYNTAX_TREE_MAX_BYTES / line.len() + 1));
        assert_eq!(NoSyntaxTree::for_text(&text), Some(NoSyntaxTree::Large));
        let text = Rope::from_str(&line.repeat(SYNTAX_TREE_MAX_BYTES / line.len() - 1));
        assert_eq!(NoSyntaxTree::for_text(&text), None);
    }

    #[test]
    fn minified_files_get_none() {
        // One line: a minified script.
        let text = Rope::from_str(&"var a=function(b){return b};".repeat(12_000));
        assert_eq!(NoSyntaxTree::for_text(&text), Some(NoSyntaxTree::Minified));
        // A record per line, each past the limit, CRLF.
        let record = format!("{{\"k\":\"{}\"}}\r\n", "v".repeat(MINIFIED_LINE_BYTES));
        let text = Rope::from_str(&record.repeat(40));
        assert_eq!(NoSyntaxTree::for_text(&text), Some(NoSyntaxTree::Minified));
    }

    #[test]
    fn a_long_line_in_ordinary_code_keeps_the_tree() {
        // An inlined data URI: one long line, most of the file ordinary lines.
        let code = "    <p class=\"note\">Some text in a paragraph.</p>\n".repeat(8_000);
        let text = format!("{code}<img src=\"data:image/png;base64,{}\">\n{code}", "A".repeat(60_000));
        assert!(text.len() > MINIFIED_MIN_BYTES);
        assert_eq!(NoSyntaxTree::for_text(&Rope::from_str(&text)), None);
    }
}

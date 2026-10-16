//! Inlay hints (`textDocument/inlayHint`): the type sourcekit-lsp infers for
//! a binding without one (`let total: Double`), and the parameter names
//! clangd shows before C and Objective-C arguments (`add(left: 1, right: 2)`).
//! They become the editor's inlays; this module only converts.

use zcode_editor::{Rope, RopeExt as _};

use super::{lsp_to_offset, offset_to_lsp};

/// One hint, placed in the text it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub offset: usize,
    pub text: String,
    /// Belongs to the text after it (a parameter name), so the caret at its
    /// offset is drawn after it; a type belongs to the text before it.
    pub before_text: bool,
}

/// The range to ask about: rows `rows` (0-based, end exclusive), whole lines.
pub fn request_range(text: &Rope, rows: std::ops::Range<u32>) -> lsp_types::Range {
    let lines = text.lines_len() as u32;
    let start = rows.start.min(lines);
    let end = rows.end.max(start);
    let end = if end >= lines {
        offset_to_lsp(text, text.len())
    } else {
        lsp_types::Position::new(end, 0)
    };
    lsp_types::Range::new(lsp_types::Position::new(start, 0), end)
}

/// The response, as inlays in `text` (the text the request was made for).
pub fn to_hints(text: &Rope, hints: Vec<lsp_types::InlayHint>) -> Vec<Hint> {
    let mut out: Vec<Hint> = hints
        .into_iter()
        .filter_map(|hint| {
            let label = match hint.label {
                lsp_types::InlayHintLabel::String(label) => label,
                lsp_types::InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|part| part.value).collect(),
            };
            let label = label.trim().to_string();
            if label.is_empty() {
                return None;
            }
            let before_text = match hint.kind {
                Some(lsp_types::InlayHintKind::PARAMETER) => true,
                Some(lsp_types::InlayHintKind::TYPE) => false,
                // Unlabelled: a name ending in a colon introduces what follows.
                _ => label.ends_with(':'),
            };
            let mut text_out = String::with_capacity(label.len() + 2);
            if hint.padding_left == Some(true) {
                text_out.push(' ');
            }
            text_out.push_str(&label);
            // A parameter name is followed by its argument: keep them apart.
            if hint.padding_right == Some(true) || (before_text && hint.padding_right.is_none()) {
                text_out.push(' ');
            }
            Some(Hint { offset: lsp_to_offset(text, hint.position), text: text_out, before_text })
        })
        .collect();
    out.sort_by_key(|hint| (hint.offset, hint.before_text));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<lsp_types::InlayHint> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn sourcekit_type_hints_follow_their_binding() {
        // As sourcekit-lsp answers for `let areas = shapes.map { $0.area() }`.
        let text = Rope::from("func f() {\n    let areas = shapes.map { $0.area() }\n}\n");
        let hints = parse(
            r#"[{"label": ": [Double]", "position": {"line": 1, "character": 13}, "kind": 1,
                 "textEdits": [{"newText": ": [Double]", "range": {"start": {"line": 1, "character": 13}, "end": {"line": 1, "character": 13}}}],
                 "data": {"version": 1}}]"#,
        );
        let offset = "func f() {\n    let areas".len();
        assert_eq!(to_hints(&text, hints), [Hint { offset, text: ": [Double]".into(), before_text: false }]);
    }

    #[test]
    fn clangd_parameter_names_come_before_their_argument() {
        let text = Rope::from("int t = add(1, 2);\n");
        let hints = parse(
            r#"[{"label": [{"value": "right:"}], "position": {"line": 0, "character": 15}, "paddingRight": true, "paddingLeft": false, "kind": 2},
                {"label": [{"value": "left:"}], "position": {"line": 0, "character": 12}, "paddingRight": true, "paddingLeft": false, "kind": 2}]"#,
        );
        assert_eq!(
            to_hints(&text, hints),
            [
                Hint { offset: 12, text: "left: ".into(), before_text: true },
                Hint { offset: 15, text: "right: ".into(), before_text: true },
            ]
        );
    }

    #[test]
    fn positions_are_utf16_and_empty_labels_are_dropped() {
        let text = Rope::from("let 🙂 = x\n");
        let hints = parse(
            r#"[{"label": ": Int", "position": {"line": 0, "character": 6}, "kind": 1, "paddingLeft": true},
                {"label": "  ", "position": {"line": 0, "character": 0}}]"#,
        );
        assert_eq!(to_hints(&text, hints), [Hint { offset: "let 🙂".len(), text: " : Int".into(), before_text: false }]);
    }

    #[test]
    fn the_request_covers_whole_lines_and_stops_at_the_end() {
        let text = Rope::from("a\nb\nc");
        assert_eq!(request_range(&text, 1..2), lsp_types::Range::new(lsp_types::Position::new(1, 0), lsp_types::Position::new(2, 0)));
        assert_eq!(request_range(&text, 0..40), lsp_types::Range::new(lsp_types::Position::new(0, 0), lsp_types::Position::new(2, 1)));
    }
}

//! Semantic tokens (`textDocument/semanticTokens/full`): what each
//! identifier is, as the compiler sees it. Tree-sitter can't tell a type
//! from a function at a call (`Greeter(name:)`), a property from a local,
//! or a parameter from a variable; sourcekit-lsp can. This module decodes a
//! response into byte ranges and the highlight theme's names for them; the
//! workspace lays them over tree-sitter's colors as a highlight layer.
//!
//! Only identifiers are taken: keywords, literals, comments and operators
//! are tree-sitter's, which colors them as Xcode does already.

use std::ops::Range;

use zcode_editor::{Rope, RopeExt as _};

/// The server's legend, resolved once to theme names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Legend {
    /// Theme name for each token type index; `None` leaves tree-sitter's.
    names: Vec<Option<&'static str>>,
}

impl Legend {
    pub fn new(legend: &lsp_types::SemanticTokensLegend) -> Self {
        Self { names: legend.token_types.iter().map(|ty| theme_name(ty.as_str())).collect() }
    }

    /// The legend in an `initialize` response's `semanticTokensProvider`.
    pub fn from_capability(capability: &serde_json::Value) -> Option<Self> {
        let legend = serde_json::from_value(capability.get("legend")?.clone()).ok()?;
        Some(Self::new(&legend))
    }
}

/// The highlight theme's name for a token type.
pub fn theme_name(token_type: &str) -> Option<&'static str> {
    Some(match token_type {
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" => "type",
        "function" | "method" => "function",
        "property" => "property",
        "variable" => "variable",
        // Falls back to "variable" in themes without a parameter color.
        "parameter" => "variable.parameter",
        "enumMember" => "constant",
        "macro" => "preproc",
        "decorator" => "attribute",
        _ => return None,
    })
}

/// Decodes `data` (five numbers per token, relative to the token before:
/// line delta, start delta in UTF-16 units, length, type, modifiers) into
/// byte ranges of `text` with their theme names, in order.
pub fn decode(text: &Rope, data: &[u32], legend: &Legend) -> Vec<(Range<usize>, &'static str)> {
    let mut out = Vec::with_capacity(data.len() / 5);
    let mut line: usize = 0;
    let mut start: u32 = 0;
    // The current line, walked forward as its tokens come in order.
    let mut cursor = LineCursor::default();
    for token in data.chunks_exact(5) {
        let (delta_line, delta_start, length, token_type) = (token[0], token[1], token[2], token[3]);
        if delta_line > 0 {
            line += delta_line as usize;
            start = delta_start;
        } else {
            start += delta_start;
        }
        let Some(Some(name)) = legend.names.get(token_type as usize) else { continue };
        if line >= text.lines_len() {
            break;
        }
        if cursor.row != Some(line) {
            cursor = LineCursor::at(text, line);
        }
        let begin = cursor.offset_of(start);
        // sourcekit-lsp marks an operator as the function it calls (`*` as
        // `Double.*`); operators keep tree-sitter's color, as in Xcode.
        if !cursor.at_identifier() {
            continue;
        }
        let end = cursor.offset_of(start + length);
        if begin < end {
            out.push((begin..end, *name));
        }
    }
    out
}

/// Converts UTF-16 columns of one line to byte offsets, moving forward.
#[derive(Default)]
struct LineCursor {
    row: Option<usize>,
    chars: Vec<char>,
    line_start: usize,
    /// Where the last conversion ended: char index, UTF-16 column, byte.
    at: (usize, u32, usize),
}

impl LineCursor {
    fn at(text: &Rope, row: usize) -> Self {
        let chars = text.slice_line(row).chars().take_while(|&c| c != '\n' && c != '\r').collect();
        Self { row: Some(row), chars, line_start: text.line_start_offset(row), at: (0, 0, 0) }
    }

    /// Whether the character the last conversion stopped at starts an
    /// identifier (or a backquoted one, or `$0`).
    fn at_identifier(&self) -> bool {
        self.chars.get(self.at.0).is_some_and(|&c| c.is_alphanumeric() || matches!(c, '_' | '`' | '$'))
    }

    fn offset_of(&mut self, column: u32) -> usize {
        if column < self.at.1 {
            self.at = (0, 0, 0);
        }
        let (mut ix, mut utf16, mut bytes) = self.at;
        while ix < self.chars.len() && utf16 < column {
            let ch = self.chars[ix];
            utf16 += ch.len_utf16() as u32;
            bytes += ch.len_utf8();
            ix += 1;
        }
        self.at = (ix, utf16, bytes);
        self.line_start + bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sourcekit_legend() -> Legend {
        // The token types sourcekit-lsp advertises, in its order.
        let types = [
            "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property",
            "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number",
            "regexp", "operator", "decorator", "bracket", "label", "concept", "unknown", "identifier",
        ];
        Legend::from_capability(&serde_json::json!({ "legend": { "tokenTypes": types, "tokenModifiers": [] } })).unwrap()
    }

    #[test]
    fn identifiers_get_theme_names_and_the_rest_is_left_to_tree_sitter() {
        let source = "let greeter = Greeter(name: name)\nprint(greeter.name)\n";
        let text = Rope::from(source);
        // greeter (variable), Greeter (struct), name (variable), keyword `let`
        // skipped, then line 1: print (function), greeter (variable), name (property).
        #[rustfmt::skip]
        let data = [
            0, 0, 3, 15, 0,
            0, 4, 7, 8, 0,
            0, 10, 7, 5, 0,
            0, 14, 4, 8, 0,
            1, 0, 5, 12, 512,
            0, 6, 7, 8, 0,
            0, 8, 4, 9, 0,
        ];
        let at = |needle: &str, nth: usize| {
            let start = source.match_indices(needle).nth(nth).unwrap().0;
            start..start + needle.len()
        };
        assert_eq!(
            decode(&text, &data, &sourcekit_legend()),
            [
                (at("greeter", 0), "variable"),
                (at("Greeter", 0), "type"),
                (at("name", 1), "variable"),
                (at("print", 0), "function"),
                (at("greeter", 1), "variable"),
                (at("name", 2), "property"),
            ]
        );
    }

    #[test]
    fn columns_are_utf16_and_tokens_past_the_text_are_dropped() {
        let text = Rope::from("let 🙂x = y\n");
        // `x` starts after a surrogate pair: UTF-16 column 6, byte 8.
        let data = [0, 6, 1, 8, 0, 4, 0, 3, 8, 0, 0, 2, 0, 99, 0];
        assert_eq!(decode(&text, &data, &sourcekit_legend()), [(8..9, "variable")]);
    }

    #[test]
    fn operators_keep_tree_sitters_color() {
        // `side * side`: sourcekit-lsp sends `*` as a method (`Double.*`).
        let text = Rope::from("side * side\n");
        let data = [0, 0, 4, 9, 0, 0, 5, 1, 13, 0, 0, 2, 4, 9, 0];
        assert_eq!(decode(&text, &data, &sourcekit_legend()), [(0..4, "property"), (7..11, "property")]);
    }

    #[test]
    fn unknown_legends_resolve_to_nothing() {
        assert_eq!(theme_name("keyword"), None);
        assert_eq!(theme_name("struct"), Some("type"));
        assert!(Legend::from_capability(&serde_json::json!({ "full": true })).is_none());
    }
}

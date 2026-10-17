//! A language server as a prediction provider: LSP 3.18's
//! `textDocument/inlineCompletion`, asked only of a server that advertises
//! `inlineCompletionProvider` (sourcekit-lsp doesn't yet). Its first item
//! is the prediction: `insertText` (a snippet's placeholders become
//! Xcode's `<#…#>`) in place of its range, or at the cursor.

use std::ops::Range;
use std::path::Path;

use serde_json::{Value, json};
use zcode_editor::Rope;

use crate::lsp::{lsp_range_to_offsets, offset_to_lsp, path_to_uri, snippet_to_placeholders};

pub const METHOD: &str = "textDocument/inlineCompletion";

pub fn params(path: &Path, text: &Rope, cursor: usize, invoked: bool) -> Value {
    json!({
        "textDocument": { "uri": path_to_uri(path).as_str() },
        "position": offset_to_lsp(text, cursor),
        // 1: invoked, 2: automatic.
        "context": { "triggerKind": if invoked { 1 } else { 2 } },
    })
}

/// The first item of a response, as text in place of a range of `text`.
pub fn parse(response: &Value, text: &Rope, cursor: usize) -> Option<(Range<usize>, String)> {
    let items = match response {
        Value::Array(items) => items,
        Value::Object(list) => list.get("items")?.as_array()?,
        _ => return None,
    };
    let item = items.first()?;
    let insert = match item.get("insertText")? {
        Value::String(text) => text.clone(),
        Value::Object(snippet) => snippet_to_placeholders(snippet.get("value")?.as_str()?),
        _ => return None,
    };
    let range = match item.get("range") {
        Some(range) if !range.is_null() => lsp_range_to_offsets(text, serde_json::from_value(range.clone()).ok()?),
        _ => cursor..cursor,
    };
    (range.start <= range.end && range.end <= text.len()).then_some((range, insert))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_item_in_place_of_its_range() {
        let text = Rope::from("let gree\n");
        let list = json!({ "items": [
            { "insertText": "greeting = \"hi\"", "range": { "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 8 } } },
            { "insertText": "other" },
        ] });
        assert_eq!(parse(&list, &text, 8), Some((4..8, "greeting = \"hi\"".into())));
        let bare = json!([{ "insertText": { "kind": "snippet", "value": "ting(${1:name})" } }]);
        assert_eq!(parse(&bare, &text, 8), Some((8..8, "ting(<#name#>)".into())));
        assert_eq!(parse(&Value::Null, &text, 8), None);
        assert_eq!(parse(&json!([]), &text, 8), None);
        let params = params(Path::new("/p/A.swift"), &text, 8, true);
        assert_eq!(params["position"], json!({ "line": 0, "character": 8 }));
        assert_eq!(params["context"]["triggerKind"], 1);
    }
}

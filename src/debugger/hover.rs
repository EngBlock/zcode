//! Hovering over a variable while paused shows its value, like Xcode's data
//! tips: the identifier under the mouse, with the `a.b.` before it, is
//! evaluated in the selected frame (lldb-dap `evaluate`, context `hover`),
//! and a structured value lists its first members. Anything else, or
//! anything lldb can't evaluate, gets the language server's Quick Help.

use std::ops::Range;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::input::{EditorState, HoverProvider, Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Entity, Task, WeakEntity, Window};
use serde_json::{Value, json};

use super::Debugger;

/// Members a structured value's data tip lists.
const MAX_MEMBERS: usize = 12;

/// Swift and C keywords, which are never variables.
const KEYWORDS: &[&str] = &[
    "as", "break", "case", "catch", "class", "continue", "default", "defer", "do", "else", "enum", "extension", "false",
    "fileprivate", "for", "func", "guard", "if", "import", "in", "init", "inout", "internal", "is", "let", "nil", "private",
    "protocol", "public", "repeat", "return", "static", "struct", "switch", "throw", "throws", "true", "try", "var",
    "where", "while", "async", "await", "some", "any", "const", "int", "void", "char", "sizeof", "typedef",
];

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// The expression to evaluate for a hover at byte `column` of `line`: the
/// identifier there with any `a.b.` member path before it, and its byte
/// range. `None` on keywords, numbers, literals and calls.
pub fn hover_expression(line: &str, column: usize) -> Option<(Range<usize>, String)> {
    let column = column.min(line.len());
    if !line.is_char_boundary(column) {
        return None;
    }
    let ident_start = |end: usize| {
        line[..end].char_indices().rev().take_while(|(_, c)| is_identifier_char(*c)).last().map(|(i, _)| i).unwrap_or(end)
    };
    let end = column + line[column..].char_indices().find(|(_, c)| !is_identifier_char(*c)).map(|(i, _)| i).unwrap_or(line.len() - column);
    let mut start = ident_start(column);
    let word = &line[start..end];
    if word.is_empty() || word.starts_with(|c: char| c.is_ascii_digit()) || KEYWORDS.contains(&word) {
        return None;
    }
    // A call (`greeting(`) isn't a value to show.
    if line[end..].trim_start().starts_with('(') {
        return None;
    }
    if in_string_literal(&line[..start]) {
        return None;
    }
    // `a.b.` before it, as long as each part is an identifier.
    while start > 0 && line[..start].ends_with('.') {
        let before = ident_start(start - 1);
        if before == start - 1 || line[before..start - 1].starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        start = before;
    }
    Some((start..end, line[start..end].to_string()))
}

/// Whether text after `prefix` is inside a string literal (but not in an
/// interpolation, `\(…)`).
fn in_string_literal(prefix: &str) -> bool {
    let mut in_string = false;
    // Open parentheses in each interpolation we're in.
    let mut interpolations: Vec<usize> = vec![];
    let mut chars = prefix.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_string {
            match ch {
                '\\' if chars.peek() == Some(&'(') => {
                    chars.next();
                    in_string = false;
                    interpolations.push(0);
                }
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
        } else {
            match (ch, interpolations.last_mut()) {
                ('"', _) => in_string = true,
                ('(', Some(open)) => *open += 1,
                (')', Some(0)) => {
                    interpolations.pop();
                    in_string = true;
                }
                (')', Some(open)) => *open -= 1,
                _ => {}
            }
        }
    }
    in_string
}

/// The editor's hover provider while debugging, in front of the language
/// server's.
pub struct DebugHover {
    debugger: WeakEntity<Debugger>,
    fallback: Option<Rc<dyn HoverProvider>>,
}

/// Puts a debugger hover in front of `editor`'s hover provider.
pub fn install(editor: &Entity<EditorState>, debugger: WeakEntity<Debugger>, cx: &mut App) {
    editor.update(cx, |state, _| {
        let lsp = state.lsp_mut();
        let fallback = lsp.hover_provider.take();
        lsp.hover_provider = Some(Rc::new(DebugHover { debugger, fallback }));
    });
}

/// A character-column range on one row, as the editor expects hovers.
fn editor_range(row: usize, line: &str, range: Range<usize>) -> lsp_types::Range {
    let column = |byte: usize| line[..byte].chars().count() as u32;
    lsp_types::Range {
        start: lsp_types::Position { line: row as u32, character: column(range.start) },
        end: lsp_types::Position { line: row as u32, character: column(range.end) },
    }
}

impl HoverProvider for DebugHover {
    fn hover(&self, text: &Rope, offset: usize, window: &mut Window, cx: &mut App) -> Task<Result<Option<lsp_types::Hover>>> {
        let quick_help = self.fallback.as_ref().map(|fallback| fallback.hover(text, offset, window, cx));
        let fallback = || quick_help.unwrap_or_else(|| Task::ready(Ok(None)));
        let Some((client, frame_id)) = self.debugger.upgrade().and_then(|d| d.read(cx).paused_frame()) else {
            return fallback();
        };
        let point = text.offset_to_point(offset);
        let line = text.slice_line(point.row).to_string();
        let Some((range, expression)) = hover_expression(&line, point.column) else {
            return fallback();
        };
        let range = editor_range(point.row, &line, range);
        let evaluate = client.request("evaluate", json!({ "expression": expression, "context": "hover", "frameId": frame_id }));
        let quick_help = fallback();
        cx.background_spawn(async move {
            let Ok(value) = evaluate.await else { return quick_help.await };
            let result = value.get("result").and_then(Value::as_str).unwrap_or("");
            let mut tip = match value.get("type").and_then(Value::as_str) {
                Some(ty) => format!("{expression}: {ty} = {result}"),
                None => format!("{expression} = {result}"),
            };
            let reference = value.get("variablesReference").and_then(Value::as_i64).unwrap_or(0);
            if reference > 0
                && let Ok(members) = client.request("variables", json!({ "variablesReference": reference })).await
            {
                let members = members.get("variables").and_then(Value::as_array).cloned().unwrap_or_default();
                for member in members.iter().take(MAX_MEMBERS) {
                    let name = member.get("name").and_then(Value::as_str).unwrap_or("");
                    let value = member.get("value").and_then(Value::as_str).unwrap_or("");
                    tip.push_str(&format!("\n  {name} = {value}"));
                }
                if members.len() > MAX_MEMBERS {
                    tip.push_str(&format!("\n  … {} more", members.len() - MAX_MEMBERS));
                }
            }
            Ok(Some(lsp_types::Hover {
                contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: format!("```swift\n{tip}\n```"),
                }),
                range: Some(range),
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, needle: &str) -> Option<String> {
        hover_expression(line, line.find(needle).unwrap()).map(|(_, e)| e)
    }

    #[test]
    fn hovers_find_the_variable_and_its_member_path() {
        let line = "    print(greeter.greeting(excited: true), name, self.items.count)";
        assert_eq!(at(line, "name"), Some("name".into()));
        assert_eq!(at(line, "items"), Some("self.items".into()));
        assert_eq!(at(line, "count"), Some("self.items.count".into()));
        assert_eq!(at(line, "self"), Some("self".into()));
        assert_eq!(at(line, "greeter"), Some("greeter".into()));
        // A call, a keyword, a literal.
        assert_eq!(at(line, "greeting"), None);
        assert_eq!(at(line, "print"), None);
        assert_eq!(at(line, "true"), None);
        assert_eq!(at("for i in 1...3 {", "in"), None);
        assert_eq!(at("for i in 1...3 {", "3"), None);
        assert_eq!(at("for i in 1...3 {", "i "), Some("i".into()));
        assert_eq!(at(r#"print("tick \(i)")"#, "tick"), None);
        assert_eq!(at(r#"print("tick \(i)")"#, "i)"), Some("i".into()));
        assert_eq!(at(r#"print("a \(f(x)) b, c")"#, "c\""), None);
        // In the middle of a word, and past the end of a line.
        assert_eq!(hover_expression("let total = 1", 7), Some((4..9, "total".into())));
        assert_eq!(hover_expression("x", 5), Some((0..1, "x".into())));
        assert_eq!(hover_expression("  ", 1), None);
    }
}

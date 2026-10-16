//! Language-server integration (sourcekit-lsp for Swift, C, C++, Objective-C).

mod client;
pub mod hierarchy;
pub mod inlay_hints;
mod providers;
pub mod semantic_tokens;
mod store;

pub use client::{LanguageServer, ServerEvent};
pub use providers::{ApplyEditFn, DocumentProviders, definition_links};
pub use store::{Features, LspStatus, LspStore, LspStoreEvent, SignatureTriggers};

use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use zcode_editor::{Rope, RopeExt as _};

/// LSP `languageId` for a file, or `None` when sourcekit-lsp can't serve it.
pub fn language_id_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "swift" | "swiftinterface" => "swift",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "c" => "c",
        "h" => "objective-c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        _ => return None,
    })
}

const URI_SAFE: &[u8] = b"-_.~/!$&'()*+,;=:@";

pub fn path_to_uri(path: &Path) -> lsp_types::Uri {
    let mut encoded = String::from("file://");
    for &byte in path.to_string_lossy().as_bytes() {
        if byte.is_ascii_alphanumeric() || URI_SAFE.contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    lsp_types::Uri::from_str(&encoded).expect("file URI is always valid")
}

pub fn uri_to_path(uri: &lsp_types::Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let rest = s.strip_prefix("file://")?;
    // Strip an optional authority ("localhost").
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                bytes.push(byte);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Byte offset → LSP position (UTF-16 columns).
pub fn offset_to_lsp(text: &Rope, offset: usize) -> lsp_types::Position {
    let offset = offset.min(text.len());
    let point = text.offset_to_point(offset);
    let line_start = text.line_start_offset(point.row);
    let character: usize = text
        .slice(line_start..offset)
        .chars()
        .map(char::len_utf16)
        .sum();
    lsp_types::Position::new(point.row as u32, character as u32)
}

/// LSP position (UTF-16 columns) → byte offset, clamped to the line.
pub fn lsp_to_offset(text: &Rope, position: lsp_types::Position) -> usize {
    let row = position.line as usize;
    if row >= text.lines_len() {
        return text.len();
    }
    let line_start = text.line_start_offset(row);
    let mut remaining = position.character as usize;
    let mut offset = line_start;
    for ch in text.slice_line(row).chars() {
        if remaining == 0 || ch == '\n' || ch == '\r' {
            break;
        }
        remaining = remaining.saturating_sub(ch.len_utf16());
        offset += ch.len_utf8();
    }
    offset
}

pub fn lsp_range_to_offsets(text: &Rope, range: lsp_types::Range) -> std::ops::Range<usize> {
    lsp_to_offset(text, range.start)..lsp_to_offset(text, range.end)
}

/// Converts an LSP snippet into Xcode-style editor placeholders:
/// `foo(${1:Int})$0` → `foo(<#Int#>)`.
pub fn snippet_to_placeholders(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len());
    let chars: Vec<char> = snippet.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                // ${N:placeholder} or ${N}
                let mut j = i + 2;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ':' {
                    let mut depth = 1;
                    let mut k = j + 1;
                    let mut inner = String::new();
                    while k < chars.len() {
                        match chars[k] {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(chars[k]);
                        k += 1;
                    }
                    out.push_str("<#");
                    out.push_str(&snippet_to_placeholders(&inner));
                    out.push_str("#>");
                    i = k + 1;
                } else {
                    // ${N} — skip to the closing brace.
                    while j < chars.len() && chars[j] != '}' {
                        j += 1;
                    }
                    i = j + 1;
                }
            }
            '$' if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() => {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Finds the next `<#placeholder#>` at or after `from`, wrapping around.
pub fn next_placeholder(text: &str, from: usize) -> Option<std::ops::Range<usize>> {
    let find = |start: usize| -> Option<std::ops::Range<usize>> {
        let open = text.get(start..)?.find("<#")? + start;
        let close = text.get(open..)?.find("#>")? + open + 2;
        Some(open..close)
    };
    find(from).or_else(|| find(0))
}

pub fn previous_placeholder(text: &str, before: usize) -> Option<std::ops::Range<usize>> {
    let head = text.get(..before)?;
    let open = head.rfind("<#").or_else(|| text.rfind("<#"))?;
    let close = text.get(open..)?.find("#>")? + open + 2;
    Some(open..close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_become_placeholders() {
        assert_eq!(
            snippet_to_placeholders("increment(by: ${1:Int})"),
            "increment(by: <#Int#>)"
        );
        assert_eq!(snippet_to_placeholders("print($0)"), "print()");
        assert_eq!(snippet_to_placeholders("a(${1})"), "a()");
        assert_eq!(snippet_to_placeholders("x\\$y"), "x$y");
    }

    #[test]
    fn placeholder_navigation() {
        let text = "foo(<#Int#>, <#String#>)";
        assert_eq!(next_placeholder(text, 0), Some(4..11));
        assert_eq!(next_placeholder(text, 11), Some(13..23));
        assert_eq!(next_placeholder(text, 23), Some(4..11));
    }

    #[test]
    fn uri_round_trip() {
        let path = Path::new("/tmp/My Project/Sources/naïve.swift");
        let uri = path_to_uri(path);
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn utf16_positions() {
        let text = Rope::from("let 🙂 = 1\nlet x = 2\n");
        let pos = offset_to_lsp(&text, "let 🙂 =".len());
        assert_eq!(pos, lsp_types::Position::new(0, 8));
        assert_eq!(lsp_to_offset(&text, pos), "let 🙂 =".len());
        assert_eq!(
            lsp_to_offset(&text, lsp_types::Position::new(1, 4)),
            "let 🙂 = 1\nlet ".len()
        );
    }
}

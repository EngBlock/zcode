//! Language-server integration: sourcekit-lsp for Swift, C, C++ and
//! Objective-C, and the web's servers (vtsls, HTML, CSS, JSON, ESLint,
//! Tailwind, Vue, Svelte, Astro) for web apps (`servers`).

mod client;
pub mod hierarchy;
pub mod inlay_hints;
pub mod install;
pub mod prettier;
mod providers;
pub mod semantic_tokens;
pub mod servers;
mod store;
pub mod sync;

pub use client::{LanguageServer, ServerEvent};
pub use providers::{ApplyEditFn, DocumentProviders, LocalCompletions, definition_links};
pub use store::{Features, LspConfig, LspStatus, LspStore, LspStoreEvent, ServerSummary, SignatureTriggers};

use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use zcode_editor::{Rope, RopeExt as _};

/// LSP `languageId` for a file, or `None` when no language server serves it.
pub fn language_id_for(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    // JSON with comments, as TypeScript and VS Code read them.
    if name.starts_with("tsconfig") && name.ends_with(".json") || name.starts_with("jsconfig") && name.ends_with(".json") {
        return Some("jsonc");
    }
    if matches!(name, ".eslintrc" | ".eslintrc.json" | ".babelrc" | ".swcrc" | "biome.jsonc") {
        return Some("jsonc");
    }
    if matches!(name, ".prettierrc") {
        return Some("json");
    }
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "swift" | "swiftinterface" => "swift",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "c" => "c",
        "h" => "objective-c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "json" => "json",
        "jsonc" => "jsonc",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        _ => return None,
    })
}

/// `workspace/symbol` asked of every server in `servers`, their answers
/// together (only symbols with a location range, as `SymbolInformation`).
pub fn workspace_symbols(
    servers: &[std::sync::Arc<LanguageServer>],
    query: &str,
) -> impl std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send + 'static + use<> {
    let requests: Vec<_> = servers
        .iter()
        .filter(|server| server.supports("/workspaceSymbolProvider"))
        .map(|server| server.request_raw("workspace/symbol", serde_json::json!({ "query": query })))
        .collect();
    async move {
        let mut symbols = vec![];
        for answer in futures::future::join_all(requests).await {
            if let Ok(serde_json::Value::Array(found)) = answer {
                symbols.extend(found.into_iter().filter(|s| s.pointer("/location/range").is_some()));
            }
        }
        Ok(serde_json::Value::Array(symbols))
    }
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

/// Byte offset → LSP position (UTF-16 columns). The rope counts UTF-16
/// units, so this doesn't walk the line (a minified file's is megabytes).
pub fn offset_to_lsp(text: &Rope, offset: usize) -> lsp_types::Position {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let point = text.offset_to_point(offset);
    let line_start = text.line_start_offset(point.row);
    let character = text.offset_to_offset_utf16(offset) - text.offset_to_offset_utf16(line_start);
    lsp_types::Position::new(point.row as u32, character as u32)
}

/// LSP position (UTF-16 columns) → byte offset, clamped to the line.
pub fn lsp_to_offset(text: &Rope, position: lsp_types::Position) -> usize {
    let row = position.line as usize;
    if row >= text.lines_len() {
        return text.len();
    }
    let line_start = text.line_start_offset(row);
    let mut line_end = line_start + text.slice_line(row).len();
    if line_end > line_start && text.char_at(line_end - 1) == Some('\r') {
        line_end -= 1;
    }
    let start_utf16 = text.offset_to_offset_utf16(line_start);
    let end_utf16 = text.offset_to_offset_utf16(line_end);
    let target = (start_utf16 + position.character as usize).min(end_utf16);
    let offset = text.offset_utf16_to_offset(target);
    // A column inside a surrogate pair counts the whole character.
    if text.offset_to_offset_utf16(offset) < target {
        text.ceil_char_boundary(offset + 1)
    } else {
        offset
    }
}

pub fn lsp_range_to_offsets(text: &Rope, range: lsp_types::Range) -> std::ops::Range<usize> {
    lsp_to_offset(text, range.start)..lsp_to_offset(text, range.end)
}

/// Converts an LSP snippet into Xcode-style editor placeholders:
/// `foo(${1:Int})$0` → `foo(<#Int#>)`.
pub fn snippet_to_placeholders(snippet: &str) -> String {
    zcode_editor::snippet::to_placeholders(snippet)
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
    fn web_files_have_language_ids() {
        let id = |name: &str| language_id_for(Path::new(name));
        assert_eq!(id("web/src/App.tsx"), Some("typescriptreact"));
        assert_eq!(id("web/vite.config.mts"), Some("typescript"));
        assert_eq!(id("web/src/main.jsx"), Some("javascriptreact"));
        assert_eq!(id("web/tsconfig.app.json"), Some("jsonc"));
        assert_eq!(id("web/package.json"), Some("json"));
        assert_eq!(id("web/src/App.vue"), Some("vue"));
        assert_eq!(id("web/index.html"), Some("html"));
        assert_eq!(id("web/src/app.scss"), Some("scss"));
        assert_eq!(id("App/Model.swift"), Some("swift"));
        assert_eq!(id("README.md"), None);
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

    /// The rope's UTF-16 counts give what walking each line's characters did.
    #[test]
    fn utf16_positions_match_walking_the_line() {
        let walk_to_lsp = |text: &Rope, offset: usize| {
            let point = text.offset_to_point(offset);
            let start = text.line_start_offset(point.row);
            let column: usize = text.slice(start..offset).chars().map(char::len_utf16).sum();
            lsp_types::Position::new(point.row as u32, column as u32)
        };
        let walk_to_offset = |text: &Rope, position: lsp_types::Position| {
            let row = position.line as usize;
            if row >= text.lines_len() {
                return text.len();
            }
            let mut remaining = position.character as usize;
            let mut offset = text.line_start_offset(row);
            for ch in text.slice_line(row).chars() {
                if remaining == 0 || ch == '\n' || ch == '\r' {
                    break;
                }
                remaining = remaining.saturating_sub(ch.len_utf16());
                offset += ch.len_utf8();
            }
            offset
        };
        let source = "a🙂b\r\nnaïve 中文 𝄞x\n\n🙂🙂\r\nend";
        let text = Rope::from(source);
        for (offset, _) in source.char_indices().chain([(source.len(), ' ')]) {
            assert_eq!(offset_to_lsp(&text, offset), walk_to_lsp(&text, offset), "at {offset}");
        }
        for line in 0..6 {
            for character in 0..16 {
                let position = lsp_types::Position::new(line, character);
                assert_eq!(lsp_to_offset(&text, position), walk_to_offset(&text, position), "at {position:?}");
            }
        }
    }
}

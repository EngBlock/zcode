//! Call and type hierarchies (`callHierarchy/*`, `typeHierarchy/*`): Find
//! Call Hierarchy's callers and callees, and Find Ancestor / Descendent /
//! Conforming Types. sourcekit-lsp answers them from its index, so they
//! cover what the index has seen: the project once it's indexed, and
//! the SDK's declarations (in their `.swiftinterface` files).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use super::{LanguageServer, path_to_uri, uri_to_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HierarchyKind {
    /// Who calls it (Xcode's Find Call Hierarchy).
    Callers,
    /// What it calls.
    Callees,
    /// What it inherits from or conforms to (Find Ancestor Types).
    Supertypes,
    /// What inherits from it or conforms to it (Find Descendent Types,
    /// Find Conforming Types).
    Subtypes,
}

impl HierarchyKind {
    pub fn is_calls(self) -> bool {
        matches!(self, Self::Callers | Self::Callees)
    }

    /// The capability the server advertises for it.
    pub fn capability(self) -> &'static str {
        if self.is_calls() { "/callHierarchyProvider" } else { "/typeHierarchyProvider" }
    }

    fn prepare_method(self) -> &'static str {
        if self.is_calls() { "textDocument/prepareCallHierarchy" } else { "textDocument/prepareTypeHierarchy" }
    }

    fn method(self) -> &'static str {
        match self {
            Self::Callers => "callHierarchy/incomingCalls",
            Self::Callees => "callHierarchy/outgoingCalls",
            Self::Supertypes => "typeHierarchy/supertypes",
            Self::Subtypes => "typeHierarchy/subtypes",
        }
    }
}

/// A symbol in a hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub struct HierarchyItem {
    pub name: String,
    pub kind: lsp_types::SymbolKind,
    /// The module, for types (sourcekit-lsp's `detail`).
    pub detail: Option<String>,
    pub path: PathBuf,
    /// The symbol's name in `path`.
    pub selection: lsp_types::Range,
    /// For a caller, where it makes the calls (in `path`); for a callee,
    /// where its parent calls it (in the parent's file).
    pub call_sites: Vec<lsp_types::Range>,
    /// The item as the server sent it, which is what it wants back.
    raw: Value,
}

impl HierarchyItem {
    fn parse(raw: Value) -> Option<Self> {
        let path = uri_to_path(&serde_json::from_value(raw.get("uri")?.clone()).ok()?)?;
        Some(Self {
            name: raw.get("name")?.as_str()?.to_string(),
            kind: serde_json::from_value(raw.get("kind")?.clone()).ok()?,
            detail: raw.get("detail").and_then(Value::as_str).filter(|d| !d.is_empty()).map(str::to_string),
            path,
            selection: serde_json::from_value(raw.get("selectionRange").or(raw.get("range"))?.clone()).ok()?,
            call_sites: vec![],
            raw,
        })
    }

    /// The module a symbol from the SDK comes from (`Swift` for
    /// `…/Swift.swiftmodule/arm64e-apple-macos.swiftinterface`).
    pub fn interface_module(&self) -> Option<String> {
        if self.path.extension()? != "swiftinterface" {
            return None;
        }
        let parent = self.path.parent()?;
        let stem = if parent.extension().is_some_and(|e| e == "swiftmodule") { parent.file_stem() } else { self.path.file_stem() };
        stem.map(|stem| stem.to_string_lossy().into_owned())
    }
}

/// The symbols at a position (usually one) that a hierarchy starts from.
pub async fn prepare(server: Arc<LanguageServer>, kind: HierarchyKind, path: PathBuf, position: lsp_types::Position) -> Result<Vec<HierarchyItem>> {
    let params = json!({ "textDocument": { "uri": path_to_uri(&path).as_str() }, "position": position });
    let value = server.request_raw(kind.prepare_method(), params).await?;
    Ok(parse_items(value))
}

/// An item's callers, callees, supertypes or subtypes.
pub async fn children(server: Arc<LanguageServer>, kind: HierarchyKind, item: &HierarchyItem) -> Result<Vec<HierarchyItem>> {
    let value = server.request_raw(kind.method(), json!({ "item": item.raw })).await?;
    Ok(parse_children(kind, value))
}

fn parse_items(value: Value) -> Vec<HierarchyItem> {
    match value {
        Value::Array(items) => items.into_iter().filter_map(HierarchyItem::parse).collect(),
        _ => vec![],
    }
}

/// Parses a response of `kind`; calls come wrapped with their call sites.
pub fn parse_children(kind: HierarchyKind, value: Value) -> Vec<HierarchyItem> {
    if !kind.is_calls() {
        return parse_items(value);
    }
    let field = if kind == HierarchyKind::Callers { "from" } else { "to" };
    let Value::Array(calls) = value else { return vec![] };
    calls
        .into_iter()
        .filter_map(|mut call| {
            let mut item = HierarchyItem::parse(call.get_mut(field)?.take())?;
            item.call_sites = serde_json::from_value(call.get("fromRanges").cloned().unwrap_or_default()).unwrap_or_default();
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_carry_their_call_sites() {
        // sourcekit-lsp's incoming calls for `total(of:)`.
        let response = json!([{
            "from": { "data": { "usr": "s:4ZKit6reportyyF" }, "uri": "file:///p/Shapes.swift", "kind": 12, "name": "report()",
                      "range": { "start": { "line": 21, "character": 5 }, "end": { "line": 21, "character": 5 } },
                      "selectionRange": { "start": { "line": 21, "character": 5 }, "end": { "line": 21, "character": 5 } } },
            "fromRanges": [{ "start": { "line": 23, "character": 12 }, "end": { "line": 23, "character": 12 } }]
        }]);
        let items = parse_children(HierarchyKind::Callers, response);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!((item.name.as_str(), item.kind, item.path.as_path()), ("report()", lsp_types::SymbolKind::FUNCTION, std::path::Path::new("/p/Shapes.swift")));
        assert_eq!(item.selection.start, lsp_types::Position::new(21, 5));
        assert_eq!(item.call_sites, [lsp_types::Range::new(lsp_types::Position::new(23, 12), lsp_types::Position::new(23, 12))]);
        // The item goes back to the server as it came, `data` included.
        assert_eq!(item.raw["data"]["usr"], "s:4ZKit6reportyyF");
    }

    #[test]
    fn callees_from_the_sdk_name_their_module() {
        let response = json!([{
            "to": { "name": "Collection.map(_:)", "kind": 6,
                    "uri": "file:///Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk/usr/lib/swift/Swift.swiftmodule/arm64e-apple-macos.swiftinterface",
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                    "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } } },
            "fromRanges": [{ "start": { "line": 15, "character": 23 }, "end": { "line": 15, "character": 23 } }]
        }, { "to": { "name": "broken" } }]);
        let items = parse_children(HierarchyKind::Callees, response);
        assert_eq!(items.len(), 1, "an item without a location is dropped");
        assert_eq!(items[0].interface_module().as_deref(), Some("Swift"));
        assert_eq!(items[0].call_sites.len(), 1);
    }

    #[test]
    fn types_are_plain_items_and_null_is_empty() {
        let response = json!([{ "uri": "file:///p/Shapes.swift", "name": "Shape", "detail": "ZKit", "kind": 11,
            "range": { "start": { "line": 0, "character": 9 }, "end": { "line": 0, "character": 9 } },
            "selectionRange": { "start": { "line": 0, "character": 9 }, "end": { "line": 0, "character": 9 } } }]);
        let items = parse_children(HierarchyKind::Supertypes, response);
        assert_eq!((items[0].name.as_str(), items[0].detail.as_deref(), items[0].kind), ("Shape", Some("ZKit"), lsp_types::SymbolKind::INTERFACE));
        assert!(items[0].interface_module().is_none());
        assert!(parse_children(HierarchyKind::Subtypes, Value::Null).is_empty());
        assert!(parse_items(Value::Null).is_empty());
    }
}

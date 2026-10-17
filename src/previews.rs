//! Finds SwiftUI previews in Swift source, for the gutter's preview marks
//! and Editor ▸ Show in Xcode Canvas: `#Preview` macros in any of their
//! forms (named, with traits, `arguments:`, a widget's `timeline:`, a UIKit
//! view controller) and types that conform to `PreviewProvider`.
//!
//! Like the test diamonds (`testing::discover`), previews are found from the
//! text alone with tree-sitter, before anything is built. A file is parsed
//! on its own: a preview never depends on another file to be one.

use tree_sitter::Node;

use crate::testing::discover::parse_swift;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// `#Preview { … }`.
    Macro,
    /// `struct ContentView_Previews: PreviewProvider`.
    Provider,
}

/// A preview as declared in source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewDecl {
    pub kind: PreviewKind,
    /// `#Preview("Dark")`'s name, or the provider's type name.
    pub name: Option<String>,
    /// 0-based line of the mark (`#Preview`, or the provider's `struct`
    /// keyword after its attributes), and the declaration's range.
    pub line: u32,
    pub start_line: u32,
    pub end_line: u32,
}

impl PreviewDecl {
    /// What the canvas calls it: `Preview`, `Dark`, `ContentView_Previews`.
    pub fn title(&self) -> String {
        match (self.kind, &self.name) {
            (_, Some(name)) => name.clone(),
            (PreviewKind::Macro, None) => "Preview".into(),
            (PreviewKind::Provider, None) => "Previews".into(),
        }
    }

    pub fn contains_line(&self, line: u32) -> bool {
        (self.start_line..=self.end_line).contains(&line)
    }
}

/// Whether a file could declare previews, without parsing it.
pub fn may_declare_previews(text: &str) -> bool {
    text.contains("#Preview") || text.contains("PreviewProvider")
}

/// The previews a Swift file declares, in source order.
pub fn previews(text: &str) -> Vec<PreviewDecl> {
    if !may_declare_previews(text) {
        return vec![];
    }
    let Some(tree) = parse_swift(text) else { return vec![] };
    let mut out = vec![];
    collect(tree.root_node(), text, &mut out);
    out
}

/// The preview whose declaration `line` (0-based) is in.
pub fn preview_at(previews: &[PreviewDecl], line: u32) -> Option<&PreviewDecl> {
    previews.iter().find(|p| p.contains_line(line))
}

fn node_text<'a>(node: Node, text: &'a str) -> &'a str {
    &text[node.byte_range()]
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Walks the whole tree: previews sit at the top level, under `#if DEBUG`
/// (whose directives are siblings, not parents), and now and then in a type.
fn collect(node: Node, text: &str, out: &mut Vec<PreviewDecl>) {
    for child in children(node) {
        match child.kind() {
            "macro_invocation" => {
                if let Some(preview) = macro_preview(child, text) {
                    out.push(preview);
                    // A preview's body declares no previews of its own.
                    continue;
                }
            }
            "class_declaration" => {
                if let Some(preview) = provider(child, text) {
                    out.push(preview);
                }
            }
            _ => {}
        }
        collect(child, text, out);
    }
}

fn macro_preview(node: Node, text: &str) -> Option<PreviewDecl> {
    let name = children(node).into_iter().find(|c| c.kind() == "simple_identifier")?;
    if node_text(name, text) != "Preview" {
        return None;
    }
    // The name is the first unlabeled argument, when it's a string.
    let title = children(node)
        .into_iter()
        .find(|c| c.kind() == "call_suffix")
        .and_then(|suffix| children(suffix).into_iter().find(|c| c.kind() == "value_arguments"))
        .and_then(|arguments| children(arguments).into_iter().find(|c| c.kind() == "value_argument"))
        .filter(|argument| argument.child_by_field_name("name").is_none())
        .and_then(|argument| argument.child_by_field_name("value"))
        .filter(|value| value.kind() == "line_string_literal")
        .map(|value| {
            children(value)
                .into_iter()
                .filter(|c| c.kind() != "\"")
                .map(|c| node_text(c, text))
                .collect::<String>()
        });
    Some(PreviewDecl {
        kind: PreviewKind::Macro,
        name: title,
        line: node.start_position().row as u32,
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
    })
}

/// A type (or extension) that conforms to `PreviewProvider`.
fn provider(node: Node, text: &str) -> Option<PreviewDecl> {
    let conforms = children(node)
        .into_iter()
        .filter(|c| c.kind() == "inheritance_specifier")
        .filter_map(|s| s.child_by_field_name("inherits_from"))
        .any(|t| {
            let name = node_text(t, text).trim();
            name.rsplit('.').next() == Some("PreviewProvider")
        });
    if !conforms {
        return None;
    }
    let name = node.child_by_field_name("name").map(|n| node_text(n, text).to_string());
    let line = node.child_by_field_name("declaration_kind").unwrap_or(node).start_position().row as u32;
    Some(PreviewDecl {
        kind: PreviewKind::Provider,
        name,
        line,
        start_line: node.start_position().row as u32,
        end_line: node.end_position().row as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{PreviewKind, preview_at, previews};

    const SWIFT: &str = r#"import SwiftUI
import WidgetKit

struct ContentView: View {
    var body: some View { Text("Hi") }
}

#Preview {
    ContentView()
}

#Preview("Dark", traits: .sizeThatFitsLayout) {
    ContentView()
        .preferredColorScheme(.dark)
}

#if DEBUG
#Preview(as: .systemSmall) {
    CounterWidget()
} timeline: {
    CounterEntry(date: .now)
}

@available(iOS 17, *)
struct ContentView_Previews: SwiftUI.PreviewProvider, Equatable {
    static var previews: some View {
        ContentView()
    }
}
#endif

#Preview(traits: .landscapeLeft) { ContentView() }
let notAPreview = #URL("https://example.com")
"#;

    fn ids(text: &str) -> Vec<String> {
        previews(text).iter().map(|p| format!("{:?} {} @{}-{}:{}", p.kind, p.title(), p.start_line + 1, p.end_line + 1, p.line + 1)).collect()
    }

    #[test]
    fn finds_macros_and_providers() {
        assert_eq!(
            ids(SWIFT),
            [
                "Macro Preview @8-10:8",
                "Macro Dark @12-15:12",
                "Macro Preview @18-22:18",
                "Provider ContentView_Previews @24-29:25",
                "Macro Preview @32-32:32",
            ]
        );
    }

    #[test]
    fn the_preview_at_a_line() {
        let found = previews(SWIFT);
        assert_eq!(preview_at(&found, 13).map(|p| p.title()), Some("Dark".into()));
        assert_eq!(preview_at(&found, 25).map(|p| p.kind), Some(PreviewKind::Provider));
        assert_eq!(preview_at(&found, 4), None);
    }

    #[test]
    fn files_without_previews() {
        assert!(previews("struct A: View { var body: some View { EmptyView() } }").is_empty());
        // Mentioned in a comment or a string, not declared.
        assert!(previews("// #Preview { A() }\nlet s = \"PreviewProvider\"\n").is_empty());
        // A protocol refining PreviewProvider isn't a preview.
        assert!(previews("protocol MyPreviews: PreviewProvider {}").is_empty());
    }

    #[test]
    fn previews_in_an_extension_and_a_view_controller() {
        let text = "extension Card: PreviewProvider {\n    static var previews: some View { Card() }\n}\n#Preview(\"Controller\") {\n    UIViewController()\n}\n";
        assert_eq!(ids(text), ["Provider Card @1-3:1", "Macro Controller @4-6:4"]);
    }
}

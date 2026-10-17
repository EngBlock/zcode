//! Language-specific bracket pairing and indentation rules.

use gpui::SharedString;
use regex::Regex;
use std::sync::Arc;

use super::SyntaxContext;

/// A structural pair, used for indentation and splitting Enter between delimiters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BracketPair {
    pub open: SharedString,
    pub close: SharedString,
}

impl BracketPair {
    pub fn new(open: impl Into<SharedString>, close: impl Into<SharedString>) -> Self {
        Self {
            open: open.into(),
            close: close.into(),
        }
    }
}

/// An automatic closing pair and the syntax contexts in which insertion is disabled.
/// Empty delimiters are ignored. Strings support delimiters such as `/*` and `*/`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AutoClosingPair {
    pub open: SharedString,
    pub close: SharedString,
    pub not_in: Vec<SyntaxContext>,
}

impl AutoClosingPair {
    pub fn new(open: impl Into<SharedString>, close: impl Into<SharedString>) -> Self {
        Self {
            open: open.into(),
            close: close.into(),
            not_in: Vec::new(),
        }
    }

    pub fn not_in(mut self, contexts: impl IntoIterator<Item = SyntaxContext>) -> Self {
        self.not_in = contexts.into_iter().collect();
        self
    }
}

/// Language indentation patterns, using Rust's `regex` syntax.
/// Patterns are compiled by the caller, so invalid expressions are reported at setup.
/// Applied on Enter: increase matches text before the caret; decrease matches text
/// after it. This does not reformat existing lines or indentation on paste.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct IndentationRules {
    pub increase_indent_pattern: Option<Arc<Regex>>,
    pub decrease_indent_pattern: Option<Arc<Regex>>,
    /// Text before the caret that doesn't indent even though it matches
    /// `increase_indent_pattern` (a void element's tag after markup's
    /// opening-tag pattern; `regex` has no lookahead to say so in one).
    pub not_increase_indent_pattern: Option<Arc<Regex>>,
}

impl IndentationRules {
    pub fn new(increase: Regex, decrease: Regex) -> Self {
        Self {
            increase_indent_pattern: Some(Arc::new(increase)),
            decrease_indent_pattern: Some(Arc::new(decrease)),
            not_increase_indent_pattern: None,
        }
    }

    pub fn except(mut self, not_increase: Regex) -> Self {
        self.not_increase_indent_pattern = Some(Arc::new(not_increase));
        self
    }
}

/// Declarative language editing rules, independent of editor preferences and parsers.
///
/// Mirrors the supported subset of Monaco's language configuration. `None` for
/// `auto_closing_pairs` uses `brackets`; `Some(vec![])` disables all automatic pairs.
/// Use the builders to configure a default value; additional language capabilities
/// can be added without breaking callers. Tree-sitter queries are configured in
/// the parser implementation, not in these rules.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LanguageConfig {
    pub brackets: Vec<BracketPair>,
    pub auto_closing_pairs: Option<Vec<AutoClosingPair>>,
    /// Automatic insertion is allowed before these characters, whitespace, or EOF.
    pub auto_close_before: SharedString,
    pub indentation_rules: Option<IndentationRules>,
    /// Typing an opening tag's `>` (HTML, JSX) adds its closing tag after
    /// the caret.
    pub auto_close_tags: bool,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        let brackets = vec![
            BracketPair::new("(", ")"),
            BracketPair::new("[", "]"),
            BracketPair::new("{", "}"),
        ];
        let mut pairs: Vec<_> = brackets
            .iter()
            .map(|p| {
                AutoClosingPair::new(p.open.clone(), p.close.clone())
                    .not_in([SyntaxContext::String, SyntaxContext::Comment])
            })
            .collect();
        pairs.extend(
            [
                AutoClosingPair::new("\"", "\""),
                AutoClosingPair::new("'", "'"),
            ]
            .into_iter()
            .map(|p| p.not_in([SyntaxContext::String, SyntaxContext::Comment])),
        );
        Self {
            brackets,
            auto_closing_pairs: Some(pairs),
            auto_close_before: ";:.,=}])>".into(),
            indentation_rules: None,
            auto_close_tags: false,
        }
    }
}

impl LanguageConfig {
    pub fn brackets(mut self, pairs: impl IntoIterator<Item = BracketPair>) -> Self {
        self.brackets = pairs.into_iter().collect();
        self
    }
    pub fn auto_closing_pairs(mut self, pairs: impl IntoIterator<Item = AutoClosingPair>) -> Self {
        self.auto_closing_pairs = Some(pairs.into_iter().collect());
        self
    }
    pub fn auto_close_before(mut self, characters: impl Into<SharedString>) -> Self {
        self.auto_close_before = characters.into();
        self
    }
    pub fn indentation_rules(mut self, rules: IndentationRules) -> Self {
        self.indentation_rules = Some(rules);
        self
    }
    pub fn auto_close_tags(mut self, on: bool) -> Self {
        self.auto_close_tags = on;
        self
    }

    pub(crate) fn closing_pairs(&self) -> impl Iterator<Item = (&str, &str, &[SyntaxContext])> {
        let configured = self
            .auto_closing_pairs
            .as_ref()
            .into_iter()
            .flatten()
            .map(|p| (p.open.as_ref(), p.close.as_ref(), p.not_in.as_slice()));
        let fallback = self
            .brackets
            .iter()
            .filter(|_| self.auto_closing_pairs.is_none())
            .map(|p| (p.open.as_ref(), p.close.as_ref(), &[][..]));
        configured
            .chain(fallback)
            .filter(|(open, close, _)| !open.is_empty() && !close.is_empty())
    }

    pub(crate) fn opens_indent(&self, text: &str) -> bool {
        self.indentation_rules
            .as_ref()
            .and_then(|r| r.increase_indent_pattern.as_ref())
            .map_or_else(
                || {
                    self.brackets
                        .iter()
                        .any(|p| !p.open.is_empty() && text.trim_end().ends_with(p.open.as_ref()))
                },
                |r| {
                    r.is_match(text)
                        && !self
                            .indentation_rules
                            .as_ref()
                            .and_then(|r| r.not_increase_indent_pattern.as_ref())
                            .is_some_and(|not| not.is_match(text))
                },
            )
    }

    /// The closing tag to add when `>` is typed after `before` (the text
    /// before the caret on its line), if it ends an opening tag.
    pub(crate) fn closing_tag(&self, before: &str) -> Option<String> {
        if !self.auto_close_tags {
            return None;
        }
        closing_tag(before)
    }

    pub(crate) fn closes_indent(&self, text: &str) -> bool {
        self.indentation_rules
            .as_ref()
            .and_then(|r| r.decrease_indent_pattern.as_ref())
            .is_some_and(|r| r.is_match(text))
    }
}

const VOID_ELEMENTS: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr", "!doctype"];

/// `</name>` for the opening tag `before` ends with (the `>` about to be
/// typed closes it): `<div class="a"` → `</div>`, `<Foo.Bar x={1}` →
/// `</Foo.Bar>`, a JSX fragment's `<` → `</>`. Not for a void element, a
/// self-closing tag, a comparison or a generic type (`a < b`, `Array<T`),
/// or a `>` inside an attribute's quotes or braces (`onClick={() =`).
fn closing_tag(before: &str) -> Option<String> {
    let open = before.rfind('<')?;
    let tag = &before[open + 1..];
    // What can't come before a tag: an identifier (a generic type, a
    // comparison), unless it's a keyword (`return <div`), or a closing
    // bracket.
    let head = before[..open].trim_end();
    if head.ends_with([')', ']']) {
        return None;
    }
    let word: String = head.chars().rev().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$')).collect::<Vec<_>>().into_iter().rev().collect();
    if !word.is_empty() && !["return", "yield", "await", "default", "case", "else", "in", "of"].contains(&word.as_str()) {
        return None;
    }
    if tag.is_empty() {
        return Some("</>".into());
    }
    let name_len = tag
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '-' | '_')))
        .map_or(tag.len(), |(i, _)| i);
    let (name, rest) = tag.split_at(name_len);
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) || VOID_ELEMENTS.contains(&name.to_ascii_lowercase().as_str()) {
        return None;
    }
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    // Attributes: names, `=`, quoted values and braced expressions only.
    let mut quote = None;
    let mut braces = 0usize;
    let mut last = ' ';
    for c in rest.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if braces > 0 => match c {
                '{' => braces += 1,
                '}' => braces -= 1,
                _ => {}
            },
            None => match c {
                '"' | '\'' => quote = Some(c),
                '{' => braces += 1,
                c if c.is_whitespace() || c.is_alphanumeric() || matches!(c, '=' | ':' | '.' | '-' | '_' | '@' | '#' | '[' | ']' | '(' | ')' | '*' | '!' | '/') => {}
                _ => return None,
            },
        }
        if !c.is_whitespace() {
            last = c;
        }
    }
    (quote.is_none() && braces == 0 && last != '/').then(|| format!("</{name}>"))
}

#[cfg(test)]
mod tag_tests {
    use super::closing_tag;

    #[test]
    fn tags_close_as_typed() {
        assert_eq!(closing_tag("<div").as_deref(), Some("</div>"));
        assert_eq!(closing_tag("  <p class=\"a > b\" id='x'").as_deref(), Some("</p>"));
        assert_eq!(closing_tag("return <Foo.Bar onClick={() => go(1)} disabled").as_deref(), Some("</Foo.Bar>"));
        assert_eq!(closing_tag("<template #header").as_deref(), Some("</template>"));
        assert_eq!(closing_tag("<button @click=\"n++\" :class=\"{ on }\"").as_deref(), Some("</button>"));
        assert_eq!(closing_tag("(<").as_deref(), Some("</>"));
        for not in [
            "<img src=\"a.png\"",
            "<br",
            "<input /",
            "<div class=\"x",
            "<b onClick={() =",
            "if (a < b",
            "const x: Array<string",
            "a<b",
            "<!-- x",
            "<1",
            "f(a) <b",
            "<b && c",
        ] {
            assert_eq!(closing_tag(not), None, "{not}");
        }
    }
}

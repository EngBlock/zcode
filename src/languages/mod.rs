//! Grammar registrations the component library lacks or needs changed, and
//! the editing rules of the languages zcode edits.

use gpui_kit::App;
use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
use regex::Regex;
use zcode_editor::language_config::LanguageConfig as LanguageRules;
use zcode_editor::{AutoClosingPair, IndentationRules, SyntaxContext};

/// See the file header for why zcode ships its own query.
pub const SWIFT_HIGHLIGHTS: &str = include_str!("swift-highlights.scm");
/// Tags and attributes in JSX, which JavaScript's and TypeScript's queries
/// leave as identifiers.
pub const JSX_HIGHLIGHTS: &str = include_str!("jsx-highlights.scm");
/// A Vue component's blocks in their own languages.
pub const VUE_INJECTIONS: &str = include_str!("vue-injections.scm");

pub fn register() {
    let registry = LanguageRegistry::singleton();
    registry.register(
        "swift",
        &LanguageConfig::new("swift", tree_sitter_swift::LANGUAGE.into(), vec![], SWIFT_HIGHLIGHTS, "", ""),
    );
    let with_jsx = |config: LanguageConfig, name: &str| LanguageConfig {
        name: name.to_string().into(),
        highlights: format!("{}\n{JSX_HIGHLIGHTS}", config.highlights).into(),
        ..config
    };
    if let Some(javascript) = registry.language("javascript") {
        let javascript = with_jsx(javascript, "javascript");
        registry.register("javascript", &javascript);
        // `.jsx` files: JavaScript, with tags closed as they're typed.
        registry.register("jsx", &LanguageConfig { name: "jsx".into(), ..javascript });
    }
    // TSX's own query is only TypeScript's additions to JavaScript's: use
    // the full TypeScript query, and JSX's, with TSX's grammar.
    if let (Some(typescript), Some(tsx)) = (registry.language("typescript"), registry.language("tsx")) {
        let config = LanguageConfig { language: tsx.language, injections: typescript.injections.clone(), ..typescript };
        registry.register("tsx", &with_jsx(config, "tsx"));
    }
    if let Some(html) = registry.language("html") {
        registry.register(
            "vue",
            &LanguageConfig {
                name: "vue".into(),
                injection_languages: ["javascript", "typescript", "tsx", "css"].map(Into::into).to_vec(),
                injections: VUE_INJECTIONS.into(),
                ..html
            },
        );
    }
    if let Some(css) = registry.language("css") {
        registry.register("less", &LanguageConfig { name: "less".into(), ..css });
    }
}

/// The languages whose tags close as their `>` is typed.
pub const TAG_LANGUAGES: &[&str] = &["html", "vue", "svelte", "astro", "jsx", "tsx"];

/// Editing rules: bracket pairs and indentation after tags for markup,
/// tags closed as they're typed, and CSS's and JSON's own pairs.
pub fn configure(cx: &mut App) {
    let pairs = |open_close: &[(&str, &str)]| -> Vec<AutoClosingPair> {
        open_close
            .iter()
            .map(|(open, close)| AutoClosingPair::new(*open, *close).not_in([SyntaxContext::String, SyntaxContext::Comment]))
            .collect()
    };
    // An opening tag (not a void element's, a self-closed one or a generic
    // type's) or a bracket.
    let markup_indent = || {
        IndentationRules::new(
            Regex::new(r"(?:<[A-Za-z][\w:.-]*(?:\s[^<>]*)?>|<>|[{(\[])\s*$").unwrap(),
            Regex::new(r"^\s*(?:</|[}\])])").unwrap(),
        )
        .except(Regex::new(r"(?i)(?:<(?:area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)\b[^<>]*>|/>|\w<[^<>]*>)\s*$").unwrap())
    };
    let script = pairs(&[("(", ")"), ("[", "]"), ("{", "}"), ("\"", "\""), ("'", "'"), ("`", "`")]);
    for language in ["javascript", "typescript"] {
        zcode_editor::set_language_config(language, LanguageRules::default().auto_closing_pairs(script.clone()), cx);
    }
    for language in ["jsx", "tsx"] {
        let config = LanguageRules::default()
            .auto_closing_pairs(script.clone())
            .indentation_rules(markup_indent())
            .auto_close_tags(true);
        zcode_editor::set_language_config(language, config, cx);
    }
    for language in ["html", "vue", "svelte", "astro"] {
        let config = LanguageRules::default()
            .auto_closing_pairs(pairs(&[("(", ")"), ("[", "]"), ("{", "}"), ("\"", "\""), ("'", "'"), ("<!--", " -->")]))
            .indentation_rules(markup_indent())
            .auto_close_tags(true);
        zcode_editor::set_language_config(language, config, cx);
    }
    for language in ["css", "less"] {
        let config = LanguageRules::default()
            .auto_closing_pairs(pairs(&[("(", ")"), ("[", "]"), ("{", "}"), ("\"", "\""), ("'", "'"), ("/*", " */")]));
        zcode_editor::set_language_config(language, config, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_grammars_compile() {
        register();
        let registry = LanguageRegistry::singleton();
        for name in ["javascript", "jsx", "typescript", "tsx", "css", "less", "html", "vue", "svelte", "astro", "json"] {
            let config = registry.language(name).unwrap_or_else(|| panic!("{name} isn't registered"));
            let grammar = config.language.clone().unwrap_or_else(|| panic!("{name} has no grammar"));
            for (what, query) in [("highlights", &config.highlights), ("injections", &config.injections)] {
                if !query.is_empty() {
                    tree_sitter::Query::new(&grammar, query).unwrap_or_else(|e| panic!("{name}'s {what}: {e}"));
                }
            }
        }
    }
}

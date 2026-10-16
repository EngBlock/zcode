//! The editing rules for languages that have no configuration of their
//! own, and the bridge to gpui-component's grammar registry.

use crate::language_config::LanguageConfig;

use crate::{AutoClosingPair, BracketPair, IndentationRules, SyntaxContext};
use regex::Regex;
use std::sync::LazyLock;

struct DefaultLanguages {
    defaults: std::collections::HashMap<&'static str, std::rc::Rc<LanguageConfig>>,
    fallback: std::rc::Rc<LanguageConfig>,
}

impl crate::LanguageProvider for DefaultLanguages {
    fn language_name(&self, name: &str) -> gpui::SharedString {
        gpui_component::highlighter::LanguageRegistry::singleton().editing_language_name(name)
    }

    fn config(&self, name: &str) -> std::rc::Rc<LanguageConfig> {
        self.defaults.get(name).unwrap_or(&self.fallback).clone()
    }

    fn syntax_context_provider(
        &self,
        name: &str,
    ) -> Option<std::rc::Rc<dyn crate::SyntaxContextProvider>> {
        super::syntax_context::syntax_context_provider(name)
    }
}

pub(crate) fn init(cx: &mut gpui::App) {
    crate::set_language_provider(
        std::rc::Rc::new(DefaultLanguages {
            defaults: ["text", "json", "python"]
                .into_iter()
                .map(|name| (name, std::rc::Rc::new(default_language_config(name))))
                .collect(),
            fallback: std::rc::Rc::new(LanguageConfig::default()),
        }),
        cx,
    );
}

/// Language editing defaults. Editor preferences are stored separately on EditorState.
/// Unknown languages use structural bracket indentation; only Python treats a colon
/// as a block opener. Plain text has no structural or automatic pairs.
fn default_language_config(language: &str) -> LanguageConfig {
    static PYTHON_INDENT: LazyLock<IndentationRules> = LazyLock::new(|| {
        IndentationRules::new(
            Regex::new(r"[\{\(\[:]\s*$").unwrap(),
            Regex::new(r"^\s*[\}\)\]]").unwrap(),
        )
    });
    match language.to_lowercase().as_str() {
        "text" => LanguageConfig::default()
            .brackets([])
            .auto_closing_pairs([]),
        "json" => LanguageConfig::default()
            .brackets([BracketPair::new("{", "}"), BracketPair::new("[", "]")])
            .auto_closing_pairs(
                [
                    AutoClosingPair::new("{", "}"),
                    AutoClosingPair::new("[", "]"),
                    AutoClosingPair::new("\"", "\""),
                ]
                .into_iter()
                .map(|p| p.not_in([SyntaxContext::String, SyntaxContext::Comment])),
            ),
        "python" => LanguageConfig::default().indentation_rules(PYTHON_INDENT.clone()),
        _ => LanguageConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_editing_rules() {
        let rules = default_language_config("text");
        assert!(rules.brackets.is_empty());
        assert!(rules.auto_closing_pairs.unwrap().is_empty());
        assert!(rules.indentation_rules.is_none());
    }

    #[test]
    fn json_only_closes_json_delimiters() {
        let rules = default_language_config("json");
        let pairs = rules.auto_closing_pairs.unwrap();
        assert_eq!(pairs.len(), 3);
        assert!(
            pairs
                .iter()
                .all(|p| p.open.as_ref() != "'" && p.open.as_ref() != "(")
        );
    }

    #[test]
    fn colon_indentation_is_language_specific() {
        assert!(
            default_language_config("python")
                .indentation_rules
                .unwrap()
                .increase_indent_pattern
                .unwrap()
                .is_match("if enabled:")
        );
        for language in ["rust", "javascript", "json", "unknown"] {
            assert!(
                default_language_config(language)
                    .indentation_rules
                    .is_none()
            );
        }
    }
}

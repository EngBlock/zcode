//! Grammar registrations that the component library ships without
//! highlight queries.

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};

pub fn register() {
    LanguageRegistry::singleton().register(
        "swift",
        &LanguageConfig::new(
            "swift",
            tree_sitter_swift::LANGUAGE.into(),
            vec![],
            tree_sitter_swift::HIGHLIGHTS_QUERY,
            "",
            tree_sitter_swift::LOCALS_QUERY,
        ),
    );
}

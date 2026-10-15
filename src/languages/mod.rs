//! Grammar registrations that the component library ships without
//! highlight queries.

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};

/// See the file header for why zcode ships its own query.
pub const SWIFT_HIGHLIGHTS: &str = include_str!("swift-highlights.scm");

pub fn register() {
    LanguageRegistry::singleton().register(
        "swift",
        &LanguageConfig::new(
            "swift",
            tree_sitter_swift::LANGUAGE.into(),
            vec![],
            SWIFT_HIGHLIGHTS,
            "",
            "",
        ),
    );
}

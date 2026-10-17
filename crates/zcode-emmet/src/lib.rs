//! Emmet for zcode: abbreviations such as `ul>li.item$*3` or `m10-20`
//! expanded natively (no Node, no network) to LSP snippet strings.
//!
//! The markup side follows Emmet's own pipeline: the abbreviation is parsed
//! (`markup::parser`), repeats are unrolled with their numbering
//! (`markup::tree`), Emmet's HTML snippets are resolved
//! (`markup::snippets`), implicit tag names, lorem ipsum and JSX attribute
//! names are filled in (`markup::transform`), and the tree is printed the
//! way Emmet's default HTML profile formats it (`markup::render`). CSS has
//! its own small parser and table (`css`).
//!
//! Output is an LSP snippet: tabstops are numbered from `$1` in document
//! order, every empty attribute value and empty element body gets one, fields
//! of Emmet's snippets become `${N:placeholder}`, and `$`, `}` and `\` in
//! literal text are escaped with a backslash. `$0` is never emitted, so the
//! client puts the final cursor after the inserted text.
#![warn(unreachable_pub)]

mod css;
mod error;
mod extract;
mod lorem;
mod markup;
mod plausible;
mod snippet;

pub use error::Error;

/// The longest abbreviation (in bytes) that is parsed at all.
const MAX_ABBREVIATION: usize = 8 * 1024;

/// The language an abbreviation is expanded for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Syntax {
    /// HTML markup: HTML, and the markup of Vue, Svelte and Astro files.
    Html,
    /// JSX and TSX: `className`, `htmlFor` and `<img />`.
    Jsx,
    /// Stylesheets: CSS, SCSS and Less.
    Css,
}

/// How expanded markup is laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// One level of indentation, e.g. four spaces or `"\t"`.
    pub indent: String,
    /// The line separator, normally `"\n"`.
    pub newline: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            indent: "    ".to_owned(),
            newline: "\n".to_owned(),
        }
    }
}

/// The abbreviation ending at the end of `line_before_cursor` (the text of the
/// cursor's line up to the cursor): its byte start in that string and the
/// abbreviation.
///
/// `None` when there is none: the line is empty or ends in whitespace, the
/// cursor is inside an open tag (an attribute value, say), the text before
/// the cursor doesn't parse as an abbreviation, or, in JSX, it follows a
/// JavaScript context (`=`, `,`, a keyword or an identifier). In CSS the
/// abbreviation must be the first thing of a declaration (after `{`, `;`, `}`
/// or the start of the line), which keeps selectors and values out.
pub fn extract_abbreviation(line_before_cursor: &str, syntax: Syntax) -> Option<(usize, &str)> {
    extract::extract(line_before_cursor, syntax)
}

/// Expands `abbreviation` to an LSP snippet string (`$1`, `${2:text}`; literal
/// `$`, `}` and `\` escaped with a backslash).
///
/// Multi-line output isn't indented for the line it lands on: the client
/// indents every line after the first like the cursor's line.
pub fn expand(abbreviation: &str, syntax: Syntax, options: &Options) -> Result<String, Error> {
    if abbreviation.len() > MAX_ABBREVIATION {
        return Err(Error::TooLarge);
    }
    match syntax {
        Syntax::Html => markup::expand(abbreviation, false, options),
        Syntax::Jsx => markup::expand(abbreviation, true, options),
        Syntax::Css => css::expand(abbreviation, options),
    }
}

/// Whether offering `abbreviation` as a completion is sensible without the
/// user asking for it, keeping plain words from producing noise.
///
/// - HTML: a known element, snippet or lorem name (`div`, `bq`, `lorem10`),
///   or anything using Emmet's operators and selectors (`>`, `+`, `*`, `.`,
///   `#`, `[`, `{`, `(`, `^`, `!`, `:` snippets) whose leading name, if any,
///   is known, a custom element (has a `-`) or a component (capitalized).
///   Text alone (`{name}`, as in Svelte and Vue templates) isn't offered.
/// - JSX: the same, except that the abbreviation must start with a known
///   lowercase element or snippet, or with `.` or `#`: a capitalized
///   component name, `{...}`, `[...]` and JavaScript-looking text with
///   unknown names (`foo.bar`, `x*2`) are never offered.
/// - CSS: every `+`-separated part is a known property abbreviation (from the
///   table, not the `property:value` fallback).
pub fn is_plausible(abbreviation: &str, syntax: Syntax) -> bool {
    plausible::is_plausible(abbreviation, syntax)
}

/// Wrap with Abbreviation: `content` (possibly several lines) goes inside the
/// innermost last element of `abbreviation`, or where it has `$#`.
///
/// With a repeated element whose `*` has no number (`ul>li*`), each non-empty
/// line of `content` becomes one repetition (trimmed). The content's common
/// indentation is removed and it is re-indented with the element. The output
/// is a snippet like [`expand`]'s. Not supported for [`Syntax::Css`].
pub fn wrap(
    abbreviation: &str,
    content: &str,
    syntax: Syntax,
    options: &Options,
) -> Result<String, Error> {
    if abbreviation.len() > MAX_ABBREVIATION || content.len() > snippet::MAX_OUTPUT {
        return Err(Error::TooLarge);
    }
    match syntax {
        Syntax::Html => markup::wrap(abbreviation, content, false, options),
        Syntax::Jsx => markup::wrap(abbreviation, content, true, options),
        Syntax::Css => Err(Error::Unsupported("Wrap with Abbreviation is for markup")),
    }
}

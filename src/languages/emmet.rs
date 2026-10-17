//! Emmet in the editor (`zcode_emmet` does the expanding): an abbreviation
//! before the cursor is offered as a completion in HTML, Vue, Svelte and
//! Astro markup, JSX and TSX, and CSS, Less and SCSS (a component's
//! `<style>` too), and Editor ▸ Emmet ▸ Expand Abbreviation (⌃⌥E) expands it
//! outright; Wrap with Abbreviation puts the selection inside one.

use std::ops::Range;

use lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, InsertTextFormat, MarkupContent, MarkupKind, TextEdit};
use zcode_editor::{Rope, RopeExt as _};
use zcode_emmet::{Options, Syntax};

/// `language` (the editor's) when Emmet works in it.
pub fn syntax_language(language: &'static str) -> Option<&'static str> {
    matches!(language, "css" | "less" | "jsx" | "tsx" | "html" | "vue" | "svelte" | "astro").then_some(language)
}

/// The Emmet syntax at `offset` of `text` in a file of `language` (the
/// editor's): a markup file's `<style>` is CSS and its `<script>` none.
pub fn syntax_at(language: &str, text: &Rope, offset: usize) -> Option<Syntax> {
    match language {
        "css" | "less" => Some(Syntax::Css),
        "jsx" | "tsx" => Some(Syntax::Jsx),
        "html" | "vue" | "svelte" | "astro" => {
            // Only the text before the cursor matters; a long file reads a page of it.
            let start = offset.saturating_sub(64 * 1024);
            let start = (start..=offset).find(|&o| text.is_char_boundary(o)).unwrap_or(offset);
            match markup_block_at(&text.slice(start..offset).to_string(), offset - start) {
                Some(MarkupBlock::Style) => Some(Syntax::Css),
                Some(MarkupBlock::Script) => None,
                None => Some(Syntax::Html),
            }
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkupBlock {
    Script,
    Style,
}

/// The `<script>` or `<style>` block `offset` is in, if any (an Astro
/// file's `---` frontmatter is a script).
pub fn markup_block_at(text: &str, offset: usize) -> Option<MarkupBlock> {
    let before = &text[..offset.min(text.len())];
    let lower = before.to_ascii_lowercase();
    for (name, block) in [("script", MarkupBlock::Script), ("style", MarkupBlock::Style)] {
        let open = lower.rfind(&format!("<{name}"));
        let close = lower.rfind(&format!("</{name}"));
        if let Some(open) = open
            && close.is_none_or(|close| close < open)
            && lower[open..].contains('>')
        {
            return Some(block);
        }
    }
    (before.starts_with("---\n") && !before[3..].contains("\n---")).then_some(MarkupBlock::Script)
}

/// The abbreviation before `offset` and its expansion as a snippet, its
/// lines after the first indented like the cursor's line. `unasked`: only
/// one worth offering without being asked (`is_plausible`).
pub fn expansion_at(language: &str, text: &Rope, offset: usize, options: &Options, unasked: bool) -> Option<(Range<usize>, String, Syntax)> {
    let syntax = syntax_at(language, text, offset)?;
    let line_start = text.line_start_offset(text.offset_to_point(offset).row);
    let line = text.slice(line_start..offset).to_string();
    let (start, abbreviation) = zcode_emmet::extract_abbreviation(&line, syntax)?;
    if unasked && !zcode_emmet::is_plausible(abbreviation, syntax) {
        return None;
    }
    let snippet = zcode_emmet::expand(abbreviation, syntax, options).ok()?;
    let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    Some((line_start + start..offset, indent_after_first(&snippet, &indent, &options.newline), syntax))
}

/// `content` wrapped in `abbreviation` (Wrap with Abbreviation), indented
/// like `indent`.
pub fn wrap(abbreviation: &str, content: &str, syntax: Syntax, options: &Options, indent: &str) -> Option<String> {
    // The content's own common indentation goes; the abbreviation's takes its place.
    let common = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let content: Vec<&str> = content.lines().map(|l| l.get(common..).unwrap_or(l.trim_start())).collect();
    let snippet = zcode_emmet::wrap(abbreviation, &content.join("\n"), syntax, options).ok()?;
    Some(indent_after_first(&snippet, indent, &options.newline))
}

fn indent_after_first(snippet: &str, indent: &str, newline: &str) -> String {
    if indent.is_empty() {
        return snippet.to_string();
    }
    snippet.split(newline).enumerate().map(|(i, line)| if i == 0 || line.is_empty() { line.to_string() } else { format!("{indent}{line}") }).collect::<Vec<_>>().join(newline)
}

/// The editor's indentation as Emmet's.
pub fn options(tab_size: u32, hard_tabs: bool) -> Options {
    Options { indent: if hard_tabs { "\t".into() } else { " ".repeat(tab_size as usize) }, newline: "\n".into() }
}

/// What Emmet adds to a file's completions.
pub struct EmmetCompletions {
    pub language: &'static str,
    pub options: Options,
}

impl crate::lsp::LocalCompletions for EmmetCompletions {
    fn completions(&self, text: &Rope, offset: usize, invoked: bool) -> Vec<CompletionItem> {
        let Some((range, snippet, syntax)) = expansion_at(self.language, text, offset, &self.options, !invoked) else {
            return vec![];
        };
        let abbreviation = text.slice(range.clone()).to_string();
        // Above the servers' for an abbreviation only Emmet reads (`ul>li*3`,
        // `m10`); below them for a bare word (a tag the server offers too, or
        // in JSX, what may be JavaScript).
        let operators = abbreviation.chars().any(|c| ">+*^#.[{(!$:".contains(c)) || abbreviation.chars().any(|c| c.is_ascii_digit());
        let first = match syntax {
            Syntax::Jsx => abbreviation.contains(['>', '*', '+', '{', '[']),
            _ => operators,
        };
        let preview = zcode_editor::snippet::to_placeholders(&snippet).replace("<##>", "");
        vec![CompletionItem {
            label: abbreviation.clone(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some("Emmet Abbreviation".into()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```{}\n{preview}\n```", if syntax == Syntax::Css { "css" } else { "html" }),
            })),
            filter_text: Some(abbreviation),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: lsp_types::Range::new(text.offset_to_position(range.start), text.offset_to_position(range.end)),
                new_text: snippet,
            })),
            preselect: Some(first),
            ..Default::default()
        }]
    }

    /// Markup's operators; CSS abbreviations are words and numbers, which
    /// ask anyway.
    fn is_trigger(&self, ch: char) -> bool {
        !matches!(self.language, "css" | "less") && ">*+^#.]})!".contains(ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_style_blocks_are_css_and_scripts_have_none() {
        let vue = Rope::from("<template>\n  ul>li\n</template>\n<style>\n.a { m10\n</style>\n<script>\ndiv\n");
        let at = |needle: &str| vue.to_string().find(needle).unwrap() + needle.len();
        assert_eq!(syntax_at("vue", &vue, at("ul>li")), Some(Syntax::Html));
        assert_eq!(syntax_at("vue", &vue, at("m10")), Some(Syntax::Css));
        assert_eq!(syntax_at("vue", &vue, at("\ndiv")), None);
        assert_eq!(syntax_at("tsx", &vue, 0), Some(Syntax::Jsx));
        assert_eq!(syntax_at("typescript", &vue, 0), None);
        assert_eq!(markup_block_at("---\nconst a = 1\n", 10), Some(MarkupBlock::Script));
        assert_eq!(markup_block_at("---\nconst a = 1\n---\n<p>", 22), None);
    }

    #[test]
    fn expansions_follow_the_lines_indentation() {
        let text = Rope::from("<body>\n    ul>li*2");
        let (range, snippet, _) = expansion_at("html", &text, text.len(), &options(4, false), true).unwrap();
        assert_eq!(range, 11..text.len());
        assert_eq!(snippet, "<ul>\n        <li>$1</li>\n        <li>$2</li>\n    </ul>");
        // A plain word in prose isn't offered unasked; asked for, it is.
        let prose = Rope::from("<p>Hello there");
        assert!(expansion_at("html", &prose, prose.len(), &options(4, false), true).is_none());
        let css = Rope::from(".a {\n  m10-20");
        assert_eq!(expansion_at("css", &css, css.len(), &options(2, false), true).unwrap().1, "margin: 10px 20px;");
    }

    #[test]
    fn wrapping_lines() {
        let wrapped = wrap("ul>li*", "  One\n  Two", Syntax::Html, &options(2, false), "").unwrap();
        assert_eq!(zcode_editor::snippet::to_placeholders(&wrapped), "<ul>\n  <li>One</li>\n  <li>Two</li>\n</ul>");
    }

    #[test]
    fn completions_rank_by_what_only_emmet_reads() {
        use crate::lsp::LocalCompletions as _;
        let emmet = EmmetCompletions { language: "html", options: options(2, false) };
        let text = Rope::from("<body>\n  .card>h2");
        let items = emmet.completions(&text, text.len(), false);
        assert_eq!(items[0].label, ".card>h2");
        assert_eq!(items[0].preselect, Some(true));
        let text = Rope::from("<body>\n  section");
        assert_eq!(emmet.completions(&text, text.len(), false)[0].preselect, Some(false));
    }
}

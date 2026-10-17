//! What the markup side knows about HTML elements and attributes.

/// Inline elements, after Emmet's `inlineElements` (plus a few newer ones):
/// adjacent inline siblings stay on one line, and an implicit child of one
/// is a `span`.
const INLINE: &[&str] = &[
    "a", "abbr", "acronym", "applet", "b", "basefont", "bdi", "bdo", "big", "br", "button", "cite",
    "code", "data", "del", "dfn", "em", "font", "i", "iframe", "img", "input", "ins", "kbd",
    "label", "map", "mark", "meter", "object", "output", "picture", "progress", "q", "s", "samp",
    "select", "small", "span", "strike", "strong", "sub", "sup", "textarea", "time", "tt", "u",
    "var", "wbr",
];

/// Elements without content: `<img>` in HTML, `<img />` in JSX.
const VOID: &[&str] = &[
    "area", "base", "basefont", "br", "col", "command", "embed", "frame", "hr", "img", "input",
    "isindex", "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Attributes printed without a value when they have none.
const BOOLEAN: &[&str] = &[
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "contenteditable",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
    "seamless",
    "typemustmatch",
];

/// HTML and SVG elements a bare word may name.
const ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "bdi",
    "bdo",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "circle",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "defs",
    "del",
    "details",
    "dfn",
    "dialog",
    "div",
    "dl",
    "dt",
    "ellipse",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "g",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "line",
    "link",
    "main",
    "map",
    "mark",
    "math",
    "menu",
    "meta",
    "meter",
    "nav",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "param",
    "path",
    "picture",
    "polygon",
    "polyline",
    "pre",
    "progress",
    "q",
    "rect",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "script",
    "search",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "template",
    "text",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "u",
    "ul",
    "use",
    "var",
    "video",
    "wbr",
];

/// Elements whose children aren't indented.
pub(crate) const FORMAT_SKIP: &[&str] = &["html"];

/// Elements whose empty body goes on a line of its own.
pub(crate) const FORMAT_FORCE: &[&str] = &["body"];

/// How many adjacent inline elements make each go on its own line.
pub(crate) const INLINE_BREAK: usize = 3;

// The lists are matched exactly: a `Link` component isn't `link`.

pub(crate) fn is_inline(name: &str) -> bool {
    INLINE.contains(&name)
}

pub(crate) fn is_void(name: &str) -> bool {
    VOID.contains(&name)
}

pub(crate) fn is_boolean_attribute(name: &str) -> bool {
    BOOLEAN.contains(&name)
}

/// Whether `name` is a known element, exactly as written (lowercase).
pub(crate) fn is_element(name: &str) -> bool {
    ELEMENTS.contains(&name)
}

/// The name of an element written without one (`.card`, `#main`, `[x]`)
/// inside `parent`.
pub(crate) fn implicit_name(parent: Option<&str>) -> &'static str {
    let Some(parent) = parent else {
        return "div";
    };
    match parent {
        "p" => "span",
        "ul" | "ol" | "menu" => "li",
        "table" | "tbody" | "thead" | "tfoot" => "tr",
        "tr" => "td",
        "select" | "optgroup" | "datalist" => "option",
        "audio" | "video" | "picture" => "source",
        "object" => "param",
        "colgroup" => "col",
        "map" => "area",
        name if is_inline(name) => "span",
        _ => "div",
    }
}

//! Parser for Emmet's markup abbreviations: elements with their id, classes,
//! attributes and text, groups, and the `>`, `+`, `^` and `*` operators.

use crate::error::Error;
use crate::snippet::closing_brace;

/// The largest `*N`.
pub(crate) const MAX_REPEAT: usize = 1000;
/// How deeply elements and groups may nest.
const MAX_DEPTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Literal(String),
    /// `$`, `$$$` (zero padded to `size`), `$@-` (descending), `$@3` (from
    /// 3): the number of the repetition. Outside a repetition it is `raw`.
    Number {
        size: usize,
        reverse: bool,
        base: usize,
        raw: String,
    },
    /// A field: `${1}` or `${1:placeholder}`.
    Field {
        index: u32,
        placeholder: String,
    },
    /// `$#`: where Wrap with Abbreviation puts the content.
    Content,
}

pub(crate) type Value = Vec<Token>;

#[derive(Clone, Debug, Default)]
pub(crate) struct Attr {
    pub(crate) name: Value,
    pub(crate) value: Option<Value>,
    /// `[onClick={handler}]`: printed in braces rather than quotes.
    pub(crate) expr: bool,
    /// `[disabled.]`: printed without a value.
    pub(crate) boolean: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Element {
    pub(crate) name: Option<Value>,
    /// `#id` and `.class` are attributes too, in the order written.
    pub(crate) attrs: Vec<Attr>,
    pub(crate) text: Option<Value>,
    pub(crate) self_close: bool,
    pub(crate) children: Vec<Item>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Repeat {
    Count(usize),
    /// `*` without a number: once, or once per line when wrapping.
    Implicit,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Element(Element),
    Group(Vec<Item>),
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) kind: Kind,
    pub(crate) repeat: Option<Repeat>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mode {
    /// `Foo.Bar` is one (component) name rather than `Foo` with a class.
    pub(crate) jsx: bool,
    /// A snippet definition: `${lang}` and `${charset}` are variables.
    pub(crate) snippet: bool,
}

/// Parses an abbreviation into its top-level items.
pub(crate) fn parse(src: &str, mode: Mode) -> Result<Vec<Item>, Error> {
    let mut parser = Parser {
        src,
        pos: 0,
        mode,
        depth: 0,
    };
    let items = parser.context()?;
    match parser.peek() {
        None => Ok(items),
        Some(_) => Err(parser.error("unmatched `)`")),
    }
}

/// Characters that end a name, id or class.
fn ends_name(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '#' | '.'
                | '['
                | ']'
                | '{'
                | '}'
                | '('
                | ')'
                | '*'
                | '/'
                | '+'
                | '>'
                | '^'
                | '='
                | '"'
                | '\''
                | '`'
                | '<'
                | ','
                | ';'
                | '\\'
                | '|'
                | '&'
                | '?'
                | '~'
                | '%'
        )
}

fn push_char(value: &mut Value, c: char) {
    match value.last_mut() {
        Some(Token::Literal(text)) => text.push(c),
        _ => value.push(Token::Literal(c.to_string())),
    }
}

fn push_str(value: &mut Value, s: &str) {
    for c in s.chars() {
        push_char(value, c);
    }
}

/// The values of `${variable}` in snippet definitions.
fn variable(name: &str) -> &'static str {
    match name {
        "lang" => "en",
        "charset" => "UTF-8",
        _ => "",
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    mode: Mode,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        let found = self.peek() == Some(expected);
        if found {
            self.pos += expected.len_utf8();
        }
        found
    }

    fn error(&self, message: &'static str) -> Error {
        Error::Syntax {
            position: self.pos,
            message,
        }
    }

    /// The items of the whole abbreviation or of a group, up to its end or
    /// `)`. `^` can't climb out of it.
    fn context(&mut self) -> Result<Vec<Item>, Error> {
        let mut items = Vec::new();
        loop {
            let (mut sequence, _) = self.sequence()?;
            items.append(&mut sequence);
            if matches!(self.peek(), None | Some(')')) {
                return Ok(items);
            }
        }
    }

    /// Siblings at one level, with their children. Returns how many levels a
    /// `^` that ended the sequence still has to climb.
    fn sequence(&mut self) -> Result<(Vec<Item>, usize), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooLarge);
        }
        let mut items = Vec::new();
        let climb = loop {
            let mut item = self.item()?;
            match self.peek() {
                Some('+') => {
                    self.bump();
                    items.push(item);
                }
                Some('>') => {
                    self.bump();
                    let (children, climb) = self.sequence()?;
                    let Kind::Element(element) = &mut item.kind else {
                        return Err(self.error("a group can't have children: put `>` inside it"));
                    };
                    element.children = children;
                    items.push(item);
                    match climb {
                        0 => break 0,
                        // A `^` in the children: what follows is a sibling here.
                        1 if matches!(self.peek(), None | Some(')')) => break 0,
                        1 => {}
                        n => break n - 1,
                    }
                }
                Some('^') => {
                    let mut climb = 0;
                    while self.eat('^') {
                        climb += 1;
                    }
                    items.push(item);
                    break climb;
                }
                None | Some(')') => {
                    items.push(item);
                    break 0;
                }
                Some(_) => return Err(self.error("unexpected character")),
            }
        };
        self.depth -= 1;
        Ok((items, climb))
    }

    fn item(&mut self) -> Result<Item, Error> {
        if self.eat('(') {
            let items = self.context()?;
            if !self.eat(')') {
                return Err(self.error("unclosed `(`"));
            }
            let repeat = self.repeat()?;
            return Ok(Item {
                kind: Kind::Group(items),
                repeat,
            });
        }
        self.element()
    }

    fn element(&mut self) -> Result<Item, Error> {
        let start = self.pos;
        let mut element = Element::default();
        let mut repeat = None;
        let name = self.name();
        if !name.is_empty() {
            element.name = Some(name);
        }
        loop {
            match self.peek() {
                Some(c @ ('#' | '.')) => {
                    self.bump();
                    let value = self.ident(c == '.');
                    if value.is_empty() {
                        return Err(self.error(if c == '.' {
                            "expected a class name"
                        } else {
                            "expected an id"
                        }));
                    }
                    element.attrs.push(Attr {
                        name: vec![Token::Literal(
                            if c == '.' { "class" } else { "id" }.to_owned(),
                        )],
                        value: Some(value),
                        ..Attr::default()
                    });
                }
                Some('[') => {
                    self.bump();
                    self.attributes(&mut element.attrs)?;
                }
                Some('{') => {
                    self.bump();
                    let text = self.text()?;
                    match &mut element.text {
                        Some(existing) => existing.extend(text),
                        None => element.text = Some(text),
                    }
                }
                Some('*') => repeat = self.repeat()?,
                Some('/') => {
                    self.bump();
                    element.self_close = true;
                }
                _ => break,
            }
        }
        if self.pos == start {
            return Err(self.error("expected an element"));
        }
        Ok(Item {
            kind: Kind::Element(element),
            repeat,
        })
    }

    /// `*N` or `*`, if there is one.
    fn repeat(&mut self) -> Result<Option<Repeat>, Error> {
        if !self.eat('*') {
            return Ok(None);
        }
        let digits = self.rest().bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Ok(Some(Repeat::Implicit));
        }
        let count = self.rest()[..digits].parse::<usize>().unwrap_or(usize::MAX);
        if count > MAX_REPEAT {
            return Err(Error::TooLarge);
        }
        if count == 0 {
            return Err(self.error("a repeat needs a count of at least 1"));
        }
        self.pos += digits;
        Ok(Some(Repeat::Count(count)))
    }

    fn name(&mut self) -> Value {
        let mut value = Vec::new();
        let component = self.mode.jsx && self.peek().is_some_and(char::is_uppercase);
        while let Some(c) = self.peek() {
            if c == '$' {
                self.number(&mut value);
            } else if c == '.' && component && self.peek_second().is_some_and(char::is_uppercase) {
                // `Foo.Bar` in JSX is a member of `Foo`.
                self.bump();
                push_char(&mut value, c);
            } else if ends_name(c) {
                break;
            } else {
                self.bump();
                push_char(&mut value, c);
            }
        }
        value
    }

    /// An id or class name. A class may contain `/` (Tailwind's `w-1/2`).
    fn ident(&mut self, class: bool) -> Value {
        let mut value = Vec::new();
        while let Some(c) = self.peek() {
            if c == '$' {
                self.number(&mut value);
            } else if c == '/' && class && self.peek_second().is_some_and(char::is_alphanumeric) {
                self.bump();
                push_char(&mut value, c);
            } else if ends_name(c) {
                break;
            } else {
                self.bump();
                push_char(&mut value, c);
            }
        }
        value
    }

    /// The attributes in `[...]`, after the `[`.
    fn attributes(&mut self, attrs: &mut Vec<Attr>) -> Result<(), Error> {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            match self.peek() {
                None => return Err(self.error("unclosed `[`")),
                Some(']') => {
                    self.bump();
                    return Ok(());
                }
                _ => {}
            }
            let mut name = Vec::new();
            while let Some(c) = self.peek() {
                if c.is_whitespace() || matches!(c, '=' | ']' | '"' | '\'' | '[') {
                    break;
                }
                if c == '$' {
                    self.number(&mut name);
                } else {
                    self.bump();
                    push_char(&mut name, c);
                }
            }
            if name.is_empty() {
                return Err(self.error("expected an attribute name"));
            }
            let mut boolean = false;
            if let Some(Token::Literal(last)) = name.last_mut()
                && last.len() > 1
                && last.ends_with('.')
            {
                last.pop();
                boolean = true;
            }
            let (value, expr) = if self.eat('=') {
                let (value, expr) = self.attribute_value()?;
                (Some(value), expr)
            } else {
                (None, false)
            };
            attrs.push(Attr {
                name,
                value,
                expr,
                boolean,
            });
        }
    }

    fn attribute_value(&mut self) -> Result<(Value, bool), Error> {
        match self.peek() {
            Some(quote @ ('"' | '\'')) => {
                self.bump();
                let mut value = Vec::new();
                loop {
                    match self.peek() {
                        None => return Err(self.error("unclosed quote")),
                        Some(c) if c == quote => {
                            self.bump();
                            return Ok((value, false));
                        }
                        Some(c) => self.value_char(c, &mut value),
                    }
                }
            }
            Some('{') => {
                self.bump();
                Ok((self.text()?, true))
            }
            _ => {
                let mut value = Vec::new();
                while let Some(c) = self.peek() {
                    if c.is_whitespace() || c == ']' {
                        break;
                    }
                    self.value_char(c, &mut value);
                }
                Ok((value, false))
            }
        }
    }

    /// One character of an attribute value or text: `\` escapes, `$`
    /// numbering, fields and content.
    fn value_char(&mut self, c: char, value: &mut Value) {
        match c {
            '\\' => {
                self.bump();
                if let Some(next) = self.bump() {
                    push_char(value, next);
                }
            }
            '$' => self.dollar(value),
            _ => {
                self.bump();
                push_char(value, c);
            }
        }
    }

    /// Text after `{`, up to the matching `}` (nested braces balanced).
    fn text(&mut self) -> Result<Value, Error> {
        let mut value = Vec::new();
        let mut depth = 0usize;
        loop {
            match self.peek() {
                None => return Err(self.error("unclosed `{`")),
                Some('}') if depth == 0 => {
                    self.bump();
                    return Ok(value);
                }
                Some(c @ ('{' | '}')) => {
                    if c == '{' {
                        depth += 1;
                    } else {
                        depth -= 1;
                    }
                    self.bump();
                    push_char(&mut value, c);
                }
                Some(c) => self.value_char(c, &mut value),
            }
        }
    }

    /// At a `$`: content, a field, a variable or numbering.
    fn dollar(&mut self, value: &mut Value) {
        match self.peek_second() {
            Some('#') => {
                self.pos += 2;
                value.push(Token::Content);
            }
            Some('{') if self.field(value) => {}
            _ => self.number(value),
        }
    }

    /// `${1}`, `${1:placeholder}` and, in snippet definitions, `${variable}`.
    /// Leaves the position alone and returns false when what follows the `$`
    /// isn't one.
    fn field(&mut self, value: &mut Value) -> bool {
        let after = &self.rest()[2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let Ok(index) = after[..digits].parse::<u32>() else {
                return false;
            };
            let tail = &after[digits..];
            let (placeholder, used) = if tail.starts_with('}') {
                (String::new(), 1)
            } else if let Some(placeholder) = tail.strip_prefix(':') {
                let Some(end) = closing_brace(placeholder) else {
                    return false;
                };
                (placeholder[..end].to_owned(), 1 + end + 1)
            } else {
                return false;
            };
            self.pos += 2 + digits + used;
            value.push(Token::Field { index, placeholder });
            return true;
        }
        if self.mode.snippet
            && let Some(end) = after.find('}')
            && end > 0
            && after[..end].bytes().all(|b| b.is_ascii_alphabetic())
        {
            let name = &after[..end];
            self.pos += 2 + end + 1;
            push_str(value, variable(name));
            return true;
        }
        false
    }

    /// `$`-numbering at a `$`.
    fn number(&mut self, value: &mut Value) {
        let start = self.pos;
        let mut size = 0;
        while self.eat('$') {
            size += 1;
        }
        let mut reverse = false;
        let mut base = 1;
        if self.peek() == Some('@')
            && self
                .peek_second()
                .is_some_and(|c| c == '-' || c.is_ascii_digit())
        {
            self.bump();
            reverse = self.eat('-');
            let digits = self.rest().bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                base = self.rest()[..digits]
                    .parse::<usize>()
                    .map_or(usize::MAX, |n| n.min(1 << 32));
                self.pos += digits;
            }
        }
        value.push(Token::Number {
            size,
            reverse,
            base,
            raw: self.src[start..self.pos].to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[Item]) -> String {
        let mut out = String::new();
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push('+');
            }
            match &item.kind {
                Kind::Element(element) => {
                    for token in element.name.iter().flatten() {
                        if let Token::Literal(text) = token {
                            out.push_str(text);
                        }
                    }
                    if !element.children.is_empty() {
                        out.push('(');
                        out.push_str(&names(&element.children));
                        out.push(')');
                    }
                }
                Kind::Group(items) => {
                    out.push('[');
                    out.push_str(&names(items));
                    out.push(']');
                }
            }
        }
        out
    }

    fn tree(src: &str) -> String {
        names(&parse(src, Mode::default()).unwrap())
    }

    #[test]
    fn operators() {
        assert_eq!(tree("a>b+c"), "a(b+c)");
        assert_eq!(tree("a>b>c^d"), "a(b(c)+d)");
        assert_eq!(tree("a>b>c^^d"), "a(b(c))+d");
        assert_eq!(tree("a>b>c^^^^d"), "a(b(c))+d");
        assert_eq!(tree("a>(b>c)+d"), "a([b(c)]+d)");
        assert_eq!(tree("(a>b^c)+d"), "[a(b)+c]+d");
        assert_eq!(tree("a>b^"), "a(b)");
    }

    #[test]
    fn errors() {
        for src in [
            "", ">a", "a>", "a+", "a>>b", "(a", "a)", "a[b", "a{b", "a[x=\"y]", ".", "a#", "a*0",
            "a b", "(a)>b",
        ] {
            assert!(parse(src, Mode::default()).is_err(), "{src}");
        }
        assert_eq!(
            parse("a*1001", Mode::default()).unwrap_err(),
            Error::TooLarge
        );
    }
}

//! The old-style (OpenStep) property list text format: `{ key = value; }`,
//! `( item, item )`, quoted or bare strings and `/* */` or `//` comments.
//! Xcode still ships some files in it (`IDEDefaultKeyBindings.idekeybindings`);
//! only reading is needed, and every leaf is a string, as in the format.

use anyhow::{Result, anyhow, bail};

use super::{Entry, Node, Value};

pub fn parse(text: &str) -> Result<Value> {
    let mut parser = Parser { chars: text.chars().collect(), at: 0 };
    let value = parser.value()?;
    parser.blanks();
    if parser.at < parser.chars.len() {
        bail!("unexpected text after the value at character {}", parser.at);
    }
    Ok(value)
}

/// Whether `text` looks like this format rather than XML.
pub fn is_openstep(text: &str) -> bool {
    let start = text.trim_start();
    start.starts_with('{') || start.starts_with('(') || start.starts_with("//") || start.starts_with("/*")
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn blanks(&mut self) {
        loop {
            match (self.peek(), self.chars.get(self.at + 1)) {
                (Some(c), _) if c.is_whitespace() => self.at += 1,
                (Some('/'), Some('/')) => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.at += 1;
                    }
                }
                (Some('/'), Some('*')) => {
                    self.at += 2;
                    while self.at < self.chars.len() && !(self.chars[self.at] == '*' && self.chars.get(self.at + 1) == Some(&'/')) {
                        self.at += 1;
                    }
                    self.at += 2;
                }
                _ => return,
            }
        }
    }

    fn expect(&mut self, c: char) -> Result<()> {
        self.blanks();
        if self.peek() == Some(c) {
            self.at += 1;
            Ok(())
        } else {
            Err(anyhow!("expected “{c}” at character {}", self.at))
        }
    }

    fn value(&mut self) -> Result<Value> {
        self.blanks();
        match self.peek().ok_or_else(|| anyhow!("unexpected end"))? {
            '{' => {
                self.at += 1;
                let mut entries = vec![];
                loop {
                    self.blanks();
                    if self.peek() == Some('}') {
                        self.at += 1;
                        return Ok(Value::Dict(entries));
                    }
                    let key = self.string()?;
                    self.expect('=')?;
                    let value = self.value()?;
                    self.expect(';')?;
                    entries.push(Entry { key, key_span: 0..0, value: Node::new(value) });
                }
            }
            '(' => {
                self.at += 1;
                let mut items = vec![];
                loop {
                    self.blanks();
                    if self.peek() == Some(')') {
                        self.at += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(Node::new(self.value()?));
                    self.blanks();
                    if self.peek() == Some(',') {
                        self.at += 1;
                    }
                }
            }
            _ => Ok(Value::String(self.string()?)),
        }
    }

    fn string(&mut self) -> Result<String> {
        self.blanks();
        let mut out = String::new();
        if self.peek() == Some('"') {
            self.at += 1;
            loop {
                let c = self.peek().ok_or_else(|| anyhow!("unterminated string"))?;
                self.at += 1;
                match c {
                    '"' => return Ok(out),
                    '\\' => {
                        let escaped = self.peek().ok_or_else(|| anyhow!("unterminated string"))?;
                        self.at += 1;
                        match escaped {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            'a' => out.push('\u{7}'),
                            'b' => out.push('\u{8}'),
                            'f' => out.push('\u{c}'),
                            'v' => out.push('\u{b}'),
                            'U' | 'u' => {
                                let hex: String = self.chars[self.at..].iter().take(4).collect();
                                self.at += hex.len();
                                let code = u32::from_str_radix(&hex, 16).map_err(|_| anyhow!("bad \\U escape"))?;
                                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            '0'..='7' => {
                                let mut code = escaped.to_digit(8).unwrap_or(0);
                                for _ in 0..2 {
                                    match self.peek().and_then(|c| c.to_digit(8)) {
                                        Some(d) => {
                                            code = code * 8 + d;
                                            self.at += 1;
                                        }
                                        None => break,
                                    }
                                }
                                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            other => out.push(other),
                        }
                    }
                    c => out.push(c),
                }
            }
        }
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || "_$+/:.-".contains(c) {
                out.push(c);
                self.at += 1;
            } else {
                break;
            }
        }
        if out.is_empty() {
            bail!("expected a string at character {}", self.at);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_style_property_lists() {
        let text = r#"
            // Comments, of both kinds.
            {
                "Text Key Bindings" = {
                    Version = 3;
                    "Key Bindings" = {
                        "^\U0040" = "setMark:";   /* ⌃@ */
                        "^x" = { "^x" = "swapWithMark:"; };
                        "\033" = ( "a", b );
                    };
                };
            }
        "#;
        assert!(is_openstep(text));
        let Value::Dict(top) = parse(text).unwrap() else { panic!() };
        let Value::Dict(text_bindings) = &top[0].value.value else { panic!() };
        assert_eq!(text_bindings[0].key, "Version");
        assert_eq!(text_bindings[0].value.value, Value::String("3".into()));
        let Value::Dict(keys) = &text_bindings[1].value.value else { panic!() };
        assert_eq!(keys[0].key, "^@");
        assert_eq!(keys[2].key, "\u{1b}");
        assert!(matches!(&keys[1].value.value, Value::Dict(inner) if inner[0].key == "^x"));
        assert!(matches!(&keys[2].value.value, Value::Array(items) if items.len() == 2));
        assert!(parse("{ a = b }").is_err());
        assert!(!is_openstep("<?xml version=\"1.0\"?><plist/>"));
    }
}

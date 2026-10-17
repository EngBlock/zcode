//! The `:` command line: a range of lines (`%`, `.`, `$`, `12`, `'a`,
//! `'<,'>`, with `+N` / `-N`) and one of the commands Vim users reach for
//! in an editor like this one.

use std::ops::RangeInclusive;

/// What `:…` asks for. Rows are 0-based.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Command {
    /// `:12`, `:$`, `:'a`
    Goto(usize),
    /// `:w`, `:wq`, `:x`, `:wa`, `:wqa`
    Write { all: bool, quit: bool },
    /// `:q`, `:q!`, `:qa`, `:qa!`
    Quit { all: bool, force: bool },
    /// `:s/pattern/replacement/flags`; `None` repeats the last one (`:s`, `:&`).
    Substitute { rows: RangeInclusive<usize>, with: Option<Substitution> },
    Delete { rows: RangeInclusive<usize>, register: Option<char> },
    Yank { rows: RangeInclusive<usize>, register: Option<char> },
    Join { rows: RangeInclusive<usize> },
    Shift { rows: RangeInclusive<usize>, right: bool, times: usize },
    NoHighlight,
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Substitution {
    /// Empty: the last search's pattern.
    pub(super) pattern: String,
    pub(super) replacement: String,
    pub(super) flags: Flags,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Flags {
    /// `g`: every match in a line, not just the first.
    pub(super) global: bool,
    /// `i` / `I`
    pub(super) ignore_case: Option<bool>,
    /// `n`: count, don't change.
    pub(super) count_only: bool,
}

/// What the range is read against.
pub(super) struct Context<'a> {
    pub(super) current: usize,
    pub(super) last: usize,
    /// A mark's row.
    pub(super) mark: &'a dyn Fn(char) -> Option<usize>,
}

pub(super) fn parse(line: &str, cx: &Context) -> Result<Command, String> {
    let input = line.trim_start_matches([':', ' ']);
    let mut parser = Parser { input, at: 0 };
    let range = parser.range(cx)?;
    parser.skip_spaces();
    let rest = &input[parser.at..];
    // `>>`, `<` and `&&` are runs of one character; other names are letters.
    let name_len = match rest.chars().next() {
        Some(c @ ('>' | '<' | '&')) => rest.chars().take_while(|d| *d == c).count(),
        _ => rest.chars().take_while(char::is_ascii_alphabetic).count(),
    };
    let name = &rest[..name_len];
    let mut args = &rest[name_len..];
    let bang = args.starts_with('!');
    if bang {
        args = &args[1..];
    }
    let args = args.trim();
    let current = cx.current..=cx.current;
    let rows = range.clone().unwrap_or(current.clone());
    let register = || args.chars().next().filter(|c| super::registers::is_register(*c));
    let is = |short: &str, long: &str| name.len() >= short.len() && long.starts_with(name);
    let command = match name {
        "" => match range {
            Some(rows) => Command::Goto(*rows.end()),
            None => return Err(String::new()),
        },
        "x" | "xit" | "exi" | "exit" => Command::Write { all: false, quit: true },
        "xa" | "xall" | "wqa" | "wqall" => Command::Write { all: true, quit: true },
        "wq" => Command::Write { all: false, quit: true },
        "wa" | "wall" => Command::Write { all: true, quit: false },
        "qa" | "qall" | "quita" | "quitall" => Command::Quit { all: true, force: bang },
        _ if is("w", "write") => Command::Write { all: false, quit: false },
        _ if is("q", "quit") => Command::Quit { all: false, force: bang },
        _ if is("s", "substitute") => Command::Substitute { rows, with: substitution(args)? },
        "&" | "&&" => Command::Substitute { rows, with: None },
        _ if is("d", "delete") => Command::Delete { rows, register: register() },
        _ if is("y", "yank") => Command::Yank { rows, register: register() },
        _ if is("j", "join") => {
            let rows = match range {
                Some(rows) if rows.start() != rows.end() => rows,
                _ => *rows.start()..=(*rows.start() + 1).min(cx.last),
            };
            Command::Join { rows }
        }
        _ if name.starts_with('>') || name.starts_with('<') => {
            Command::Shift { rows, right: name.starts_with('>'), times: name.len() }
        }
        _ if is("noh", "nohlsearch") => Command::NoHighlight,
        _ if is("u", "undo") => Command::Undo,
        _ if is("red", "redo") => Command::Redo,
        _ => return Err(format!("E492: Not an editor command: {}", line.trim_start_matches(':'))),
    };
    Ok(command)
}

struct Parser<'a> {
    input: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.input[self.at..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.bump();
        }
    }

    fn number(&mut self) -> Option<usize> {
        let digits: String = self.input[self.at..].chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        self.at += digits.len();
        digits.parse().ok()
    }

    fn range(&mut self, cx: &Context) -> Result<Option<RangeInclusive<usize>>, String> {
        self.skip_spaces();
        if self.peek() == Some('%') {
            self.bump();
            return Ok(Some(0..=cx.last));
        }
        let Some(first) = self.address(cx)? else { return Ok(None) };
        self.skip_spaces();
        if matches!(self.peek(), Some(',' | ';')) {
            self.bump();
            let second = self.address(cx)?.unwrap_or(cx.current);
            let (a, b) = if first <= second { (first, second) } else { (second, first) };
            return Ok(Some(a..=b));
        }
        Ok(Some(first..=first))
    }

    fn address(&mut self, cx: &Context) -> Result<Option<usize>, String> {
        self.skip_spaces();
        let base = match self.peek() {
            Some('.') => {
                self.bump();
                Some(cx.current as isize)
            }
            Some('$') => {
                self.bump();
                Some(cx.last as isize)
            }
            Some('\'') => {
                self.bump();
                let name = self.bump().ok_or("E20: Mark not set")?;
                Some((cx.mark)(name).ok_or("E20: Mark not set")? as isize)
            }
            Some(c) if c.is_ascii_digit() => Some(self.number().unwrap_or(1) as isize - 1),
            Some('+' | '-') => Some(cx.current as isize),
            _ => None,
        };
        let Some(mut row) = base else { return Ok(None) };
        while let Some(sign @ ('+' | '-')) = self.peek() {
            self.bump();
            let n = self.number().unwrap_or(1) as isize;
            row += if sign == '+' { n } else { -n };
        }
        Ok(Some(row.clamp(0, cx.last as isize) as usize))
    }
}

/// `/pattern/replacement/flags` (any punctuation as the separator).
fn substitution(args: &str) -> Result<Option<Substitution>, String> {
    let mut chars = args.chars();
    let Some(separator) = chars.next() else { return Ok(None) };
    if separator.is_alphanumeric() || separator == '\\' || separator == '"' || separator == ' ' {
        return Err("E146: Regular expressions can't be delimited by letters".into());
    }
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in chars {
        if escaped {
            if c != separator {
                parts.last_mut().unwrap().push('\\');
            }
            parts.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == separator && parts.len() < 3 {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    if escaped {
        parts.last_mut().unwrap().push('\\');
    }
    let mut flags = Flags::default();
    for flag in parts.get(2).map(|f| f.trim()).unwrap_or("").chars() {
        match flag {
            'g' => flags.global = !flags.global,
            'i' => flags.ignore_case = Some(true),
            'I' => flags.ignore_case = Some(false),
            'n' => flags.count_only = true,
            'e' | '&' => {}
            'c' => return Err("Confirming each substitution (c) isn't supported".into()),
            other => return Err(format!("E488: Trailing characters: {other}")),
        }
    }
    Ok(Some(Substitution {
        pattern: parts[0].clone(),
        replacement: parts.get(1).cloned().unwrap_or_default(),
        flags,
    }))
}

/// A piece of a replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Group(usize),
    /// `\u` / `\l`: the next character's case; `\U` / `\L`: until `\E`.
    Upper { one: bool },
    Lower { one: bool },
    End,
}

/// Vim's replacement text for one match: `&` and `\0` the match, `\1`–`\9`
/// its groups, `\r` / `\n` a line break, `\t` a tab, `\u` `\l` `\U` `\L`
/// `\E` change case.
pub(super) fn expand(replacement: &str, captures: &regex::Captures) -> String {
    let mut pieces = vec![];
    let mut chars = replacement.chars();
    let mut text = String::new();
    let flush = |text: &mut String, pieces: &mut Vec<Piece>| {
        if !text.is_empty() {
            pieces.push(Piece::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '&' => {
                flush(&mut text, &mut pieces);
                pieces.push(Piece::Group(0));
            }
            '\\' => match chars.next() {
                Some(d @ '0'..='9') => {
                    flush(&mut text, &mut pieces);
                    pieces.push(Piece::Group(d as usize - '0' as usize));
                }
                Some('r' | 'n') => text.push('\n'),
                Some('t') => text.push('\t'),
                Some(case @ ('u' | 'l' | 'U' | 'L' | 'e' | 'E')) => {
                    flush(&mut text, &mut pieces);
                    pieces.push(match case {
                        'u' => Piece::Upper { one: true },
                        'l' => Piece::Lower { one: true },
                        'U' => Piece::Upper { one: false },
                        'L' => Piece::Lower { one: false },
                        _ => Piece::End,
                    });
                }
                Some(other) => text.push(other),
                None => text.push('\\'),
            },
            _ => text.push(c),
        }
    }
    flush(&mut text, &mut pieces);

    let mut out = String::new();
    let mut one: Option<bool> = None;
    let mut run: Option<bool> = None;
    for piece in pieces {
        let text = match piece {
            Piece::Text(text) => text,
            Piece::Group(n) => captures.get(n).map_or(String::new(), |m| m.as_str().to_string()),
            Piece::Upper { one: true } => {
                one = Some(true);
                continue;
            }
            Piece::Lower { one: true } => {
                one = Some(false);
                continue;
            }
            Piece::Upper { one: false } => {
                run = Some(true);
                continue;
            }
            Piece::Lower { one: false } => {
                run = Some(false);
                continue;
            }
            Piece::End => {
                run = None;
                continue;
            }
        };
        for c in text.chars() {
            let upper = one.take().or(run);
            match upper {
                Some(true) => out.extend(c.to_uppercase()),
                Some(false) => out.extend(c.to_lowercase()),
                None => out.push(c),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Context<'static> {
        Context { current: 4, last: 9, mark: &|c| (c == 'a').then_some(2) }
    }

    #[test]
    fn ranges_and_commands() {
        let cx = cx();
        assert_eq!(parse(":12", &cx), Ok(Command::Goto(9)), "past the end: the last line");
        assert_eq!(parse("3", &cx), Ok(Command::Goto(2)));
        assert_eq!(parse("$", &cx), Ok(Command::Goto(9)));
        assert_eq!(parse(".+2", &cx), Ok(Command::Goto(6)));
        assert_eq!(parse("'a", &cx), Ok(Command::Goto(2)));
        assert_eq!(parse("w", &cx), Ok(Command::Write { all: false, quit: false }));
        assert_eq!(parse("write", &cx), Ok(Command::Write { all: false, quit: false }));
        assert_eq!(parse("wq", &cx), Ok(Command::Write { all: false, quit: true }));
        assert_eq!(parse("x", &cx), Ok(Command::Write { all: false, quit: true }));
        assert_eq!(parse("q!", &cx), Ok(Command::Quit { all: false, force: true }));
        assert_eq!(parse("qa", &cx), Ok(Command::Quit { all: true, force: false }));
        assert_eq!(parse("wa", &cx), Ok(Command::Write { all: true, quit: false }));
        assert_eq!(parse("2,4d a", &cx), Ok(Command::Delete { rows: 1..=3, register: Some('a') }));
        assert_eq!(parse("'a,.y", &cx), Ok(Command::Yank { rows: 2..=4, register: None }));
        assert_eq!(parse("j", &cx), Ok(Command::Join { rows: 4..=5 }));
        assert_eq!(parse("%>>", &cx), Ok(Command::Shift { rows: 0..=9, right: true, times: 2 }));
        assert_eq!(parse("noh", &cx), Ok(Command::NoHighlight));
        assert!(parse("frobnicate", &cx).unwrap_err().starts_with("E492"));
        assert!(parse("'z", &cx).unwrap_err().starts_with("E20"));
        let s = parse("%s/a\\/b/c/gi", &cx).unwrap();
        assert_eq!(
            s,
            Command::Substitute {
                rows: 0..=9,
                with: Some(Substitution {
                    pattern: "a/b".into(),
                    replacement: "c".into(),
                    flags: Flags { global: true, ignore_case: Some(true), count_only: false },
                }),
            }
        );
        assert_eq!(parse("s", &cx), Ok(Command::Substitute { rows: 4..=4, with: None }));
        assert_eq!(parse("s#x#y", &cx), Ok(Command::Substitute { rows: 4..=4, with: Some(Substitution { pattern: "x".into(), replacement: "y".into(), flags: Flags::default() }) }));
        assert!(parse("s/a/b/c", &cx).is_err());
    }

    #[test]
    fn replacements_use_groups_and_case() {
        let regex = regex::Regex::new("(\\w+)=(\\w+)").unwrap();
        let captures = regex.captures("key=value").unwrap();
        assert_eq!(expand("\\2=\\1", &captures), "value=key");
        assert_eq!(expand("[&]", &captures), "[key=value]");
        assert_eq!(expand("\\u\\1 \\U\\2\\E!", &captures), "Key VALUE!");
        assert_eq!(expand("a\\rb\\&", &captures), "a\nb&");
    }
}

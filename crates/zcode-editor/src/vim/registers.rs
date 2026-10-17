//! Registers, shared by every editor as Vim's are: the unnamed register
//! (which is also the system clipboard, as in Zed), `"0` for the last yank,
//! `"1`–`"9` for deleted lines, `"-` for small deletes, `"a`–`"z` (`"A`–`"Z`
//! append), `"+` and `"*` for the clipboard, `"_` for nothing, and `".`,
//! `"/` and `":` for the last insert, search and command.
//!
//! Text yanked here and pasted in another app is plain text; text copied
//! elsewhere pastes characterwise, or linewise when it ends with a newline.
//! A yank with several cursors keeps a piece per cursor, so pasting with as
//! many cursors puts each piece back at its own.

use std::collections::HashMap;

use gpui::{App, ClipboardItem, Global};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Kind {
    Chars,
    Lines,
    Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Register {
    pub(super) pieces: Vec<String>,
    pub(super) kind: Kind,
}

impl Register {
    pub(super) fn new(text: impl Into<String>, kind: Kind) -> Self {
        Register { pieces: vec![text.into()], kind }
    }

    pub(super) fn text(&self) -> String {
        self.pieces.join("\n")
    }
}

/// Which of Vim's rules a write follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Write {
    Yank,
    Delete,
}

#[derive(Serialize, Deserialize)]
struct Metadata {
    vim: Kind,
}

#[derive(Default)]
struct Registers {
    stored: HashMap<char, Register>,
    /// What the unnamed register last put on the clipboard, to tell our
    /// own text from text copied elsewhere since.
    clipboard: Option<String>,
}

impl Global for Registers {}

fn registers(cx: &mut App) -> &mut Registers {
    cx.default_global::<Registers>()
}

/// Whether `name` is a register `"x` can name.
pub(super) fn is_register(name: char) -> bool {
    name.is_ascii_alphanumeric() || "\"-_+*./:".contains(name)
}

pub(super) fn write(cx: &mut App, name: Option<char>, register: Register, write: Write) {
    let name = name.unwrap_or('"');
    if name == '_' {
        return;
    }
    let register = match name {
        'A'..='Z' => {
            let lower = name.to_ascii_lowercase();
            let appended = match registers(cx).stored.get(&lower) {
                Some(existing) => {
                    let lines = existing.kind == Kind::Lines || register.kind == Kind::Lines;
                    let mut text = existing.text();
                    if lines && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&register.text());
                    Register::new(text, if lines { Kind::Lines } else { existing.kind })
                }
                None => register,
            };
            registers(cx).stored.insert(lower, appended.clone());
            appended
        }
        'a'..='z' => {
            registers(cx).stored.insert(name, register.clone());
            register
        }
        _ => {
            let stored = &mut registers(cx).stored;
            match write {
                Write::Yank => {
                    stored.insert('0', register.clone());
                }
                Write::Delete if register.kind == Kind::Lines || register.text().contains('\n') => {
                    for n in (1..9).rev() {
                        let digit = |n: u32| char::from_digit(n, 10).unwrap();
                        if let Some(older) = stored.remove(&digit(n)) {
                            stored.insert(digit(n + 1), older);
                        }
                    }
                    stored.insert('1', register.clone());
                }
                Write::Delete => {
                    stored.insert('-', register.clone());
                }
            }
            register
        }
    };
    // The unnamed register is the clipboard.
    let text = register.text();
    let item = ClipboardItem::new_string_with_json_metadata(text.clone(), Metadata { vim: register.kind });
    cx.write_to_clipboard(item);
    let registers = registers(cx);
    registers.clipboard = Some(text);
    registers.stored.insert('"', register);
}

/// Sets one of the read-only registers (`.`, `/`, `:`).
pub(super) fn set_special(cx: &mut App, name: char, text: String) {
    registers(cx).stored.insert(name, Register::new(text, Kind::Chars));
}

pub(super) fn read(cx: &mut App, name: Option<char>) -> Option<Register> {
    let name = name.unwrap_or('"').to_ascii_lowercase();
    match name {
        '"' | '+' | '*' => {
            let item = cx.read_from_clipboard();
            let registers = registers(cx);
            match item.as_ref().and_then(ClipboardItem::text) {
                Some(text) if registers.clipboard.as_ref() == Some(&text) => {
                    registers.stored.get(&'"').cloned().or_else(|| Some(Register::new(text, Kind::Chars)))
                }
                Some(text) => {
                    let kind = item
                        .as_ref()
                        .and_then(|item| item.metadata())
                        .and_then(|metadata| serde_json::from_str::<Metadata>(metadata).ok())
                        .map(|m| m.vim)
                        .unwrap_or(if text.ends_with('\n') { Kind::Lines } else { Kind::Chars });
                    Some(Register::new(text, kind))
                }
                None => registers.stored.get(&'"').cloned(),
            }
        }
        '_' => None,
        _ => registers(cx).stored.get(&name).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn deletes_shift_the_numbered_registers_and_the_clipboard_follows(cx: &mut TestAppContext) {
        cx.update(|cx| {
            write(cx, None, Register::new("one\n", Kind::Lines), Write::Delete);
            write(cx, None, Register::new("two\n", Kind::Lines), Write::Delete);
            write(cx, None, Register::new("x", Kind::Chars), Write::Delete);
            write(cx, None, Register::new("kept", Kind::Chars), Write::Yank);
            assert_eq!(read(cx, Some('1')).unwrap().text(), "two\n");
            assert_eq!(read(cx, Some('2')).unwrap().text(), "one\n");
            assert_eq!(read(cx, Some('-')).unwrap().text(), "x");
            assert_eq!(read(cx, Some('0')).unwrap().text(), "kept");
            assert_eq!(read(cx, None).unwrap().text(), "kept");
            assert_eq!(cx.read_from_clipboard().and_then(|i| i.text()).as_deref(), Some("kept"));
            // Copied elsewhere: the clipboard wins, linewise when it ends a line.
            cx.write_to_clipboard(ClipboardItem::new_string("other\n".into()));
            assert_eq!(read(cx, None).unwrap(), Register::new("other\n", Kind::Lines));
            write(cx, Some('a'), Register::new("a", Kind::Chars), Write::Yank);
            write(cx, Some('A'), Register::new("b", Kind::Chars), Write::Yank);
            assert_eq!(read(cx, Some('a')).unwrap().text(), "ab");
            assert_eq!(read(cx, Some('0')).unwrap().text(), "kept", "named yanks leave \"0 alone");
            write(cx, Some('_'), Register::new("gone", Kind::Chars), Write::Delete);
            assert_eq!(read(cx, None).unwrap().text(), "ab");
        });
    }
}

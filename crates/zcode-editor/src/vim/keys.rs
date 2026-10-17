//! Keystrokes as Vim reads them: a typed character, a control key or one of
//! the named keys. ⌘, ⌥ and function-key chords aren't Vim's and go on to
//! the keymap.

use gpui::Keystroke;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Char(char),
    /// ⌃ and a character, lowercased (`⌃R` is `Ctrl('r')`).
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    pub(super) fn from_keystroke(keystroke: &Keystroke) -> Option<Key> {
        let m = &keystroke.modifiers;
        if m.platform || m.function {
            return None;
        }
        let named = match keystroke.key.as_str() {
            "escape" => Some(Key::Escape),
            "enter" => Some(Key::Enter),
            "backspace" => Some(Key::Backspace),
            "delete" => Some(Key::Delete),
            "tab" => Some(Key::Tab),
            "left" => Some(Key::Left),
            "right" => Some(Key::Right),
            "up" => Some(Key::Up),
            "down" => Some(Key::Down),
            "home" => Some(Key::Home),
            "end" => Some(Key::End),
            "pageup" => Some(Key::PageUp),
            "pagedown" => Some(Key::PageDown),
            "space" => Some(Key::Char(' ')),
            _ => None,
        };
        if m.control {
            // ⌃[ is Escape, as in a terminal.
            return match (named, single(&keystroke.key)) {
                (_, Some('[')) => Some(Key::Escape),
                (None, Some(c)) if !m.alt => Some(Key::Ctrl(c.to_ascii_lowercase())),
                _ => None,
            };
        }
        if let Some(named) = named {
            // ⇧ and ⌥ with a named key (⇧→ extends a selection, ⌥← moves
            // by word) stay the editor's.
            return (!m.shift && !m.alt || named == Key::Char(' ')).then_some(named);
        }
        if m.alt {
            return None;
        }
        match keystroke.key_char.as_deref().and_then(single) {
            Some(c) if !c.is_control() => Some(Key::Char(c)),
            _ => single(&keystroke.key).map(|c| Key::Char(if m.shift { c.to_ascii_uppercase() } else { c })),
        }
    }

    /// How the key shows among the pending keys in the status bar.
    pub(super) fn label(self) -> String {
        match self {
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("^{}", c.to_ascii_uppercase()),
            Key::Escape => "<Esc>".into(),
            Key::Enter => "<CR>".into(),
            Key::Backspace => "<BS>".into(),
            Key::Delete => "<Del>".into(),
            Key::Tab => "<Tab>".into(),
            Key::Left => "<Left>".into(),
            Key::Right => "<Right>".into(),
            Key::Up => "<Up>".into(),
            Key::Down => "<Down>".into(),
            Key::Home => "<Home>".into(),
            Key::End => "<End>".into(),
            Key::PageUp => "<PageUp>".into(),
            Key::PageDown => "<PageDown>".into(),
        }
    }
}

fn single(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: &str) -> Option<Key> {
        Key::from_keystroke(&Keystroke::parse(source).unwrap())
    }

    #[test]
    fn reads_characters_control_keys_and_named_keys() {
        assert_eq!(key("j"), Some(Key::Char('j')));
        assert_eq!(key("J"), Some(Key::Char('J')));
        assert_eq!(key("shift-4->$"), Some(Key::Char('$')));
        assert_eq!(key("$"), Some(Key::Char('$')));
        assert_eq!(key("ctrl-r"), Some(Key::Ctrl('r')));
        assert_eq!(key("ctrl-["), Some(Key::Escape));
        assert_eq!(key("space"), Some(Key::Char(' ')));
        assert_eq!(key("escape"), Some(Key::Escape));
        assert_eq!(key("cmd-s"), None);
        assert_eq!(key("alt-left"), None);
        assert_eq!(key("shift-right"), None);
        assert_eq!(key("ctrl-alt-x"), None);
        assert_eq!(Key::Ctrl('v').label(), "^V");
    }
}

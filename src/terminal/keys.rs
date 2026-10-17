//! Keystrokes and the mouse as the bytes a terminal sends for them: text
//! as typed, ⌃ letters as control characters, the xterm sequences for
//! arrows, function and editing keys (with their modifiers, and in the
//! application cursor mode programs like `vim` switch on), and mouse
//! reports for programs that ask for them. ⌘ keystrokes are zcode's, so
//! they have none.

use std::borrow::Cow;

use alacritty_terminal::term::TermMode;
use gpui_kit::Keystroke;

/// The bytes for `keystroke`, or `None` when the terminal doesn't take it.
pub fn bytes_for(keystroke: &Keystroke, mode: TermMode) -> Option<Cow<'static, [u8]>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    // xterm's modifier parameter: 1 + Shift 1, Alt 2, Control 4.
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let csi = |code: &str, ch: char| -> Cow<'static, [u8]> {
        Cow::Owned(match (modifier, code) {
            (1, "") => format!("\x1b[{ch}"),
            (1, code) => format!("\x1b[{code}{ch}"),
            (modifier, "") => format!("\x1b[1;{modifier}{ch}"),
            (modifier, code) => format!("\x1b[{code};{modifier}{ch}"),
        }
        .into_bytes())
    };
    let cursor = |ch: char| -> Cow<'static, [u8]> {
        if modifier == 1 && app_cursor { Cow::Owned(format!("\x1bO{ch}").into_bytes()) } else { csi("", ch) }
    };
    let fixed = |bytes: &'static [u8]| Some(Cow::Borrowed(bytes));
    let with_alt = |bytes: &[u8]| -> Option<Cow<'static, [u8]>> {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        Some(Cow::Owned(out))
    };
    match keystroke.key.as_str() {
        "enter" => with_alt(b"\r"),
        "tab" if m.shift => fixed(b"\x1b[Z"),
        "tab" => with_alt(b"\t"),
        "escape" => with_alt(b"\x1b"),
        "backspace" if m.control => with_alt(b"\x08"),
        "backspace" => with_alt(b"\x7f"),
        "space" if m.control => with_alt(b"\x00"),
        // Terminal.app's ⌥← and ⌥→: back and forward a word in the shell.
        "left" if m.alt && !m.shift && !m.control => fixed(b"\x1bb"),
        "right" if m.alt && !m.shift && !m.control => fixed(b"\x1bf"),
        "up" => Some(cursor('A')),
        "down" => Some(cursor('B')),
        "right" => Some(cursor('C')),
        "left" => Some(cursor('D')),
        "home" => Some(cursor('H')),
        "end" => Some(cursor('F')),
        "insert" => Some(csi("2", '~')),
        "delete" => Some(csi("3", '~')),
        "pageup" => Some(csi("5", '~')),
        "pagedown" => Some(csi("6", '~')),
        "f1" | "f2" | "f3" | "f4" => {
            let ch = (b'P' + keystroke.key.as_bytes()[1] - b'1') as char;
            Some(if modifier == 1 { Cow::Owned(format!("\x1bO{ch}").into_bytes()) } else { csi("", ch) })
        }
        key if key.starts_with('f') && key.len() > 1 && key[1..].parse::<u8>().is_ok() => {
            let code = match key[1..].parse::<u8>().ok()? {
                5 => "15",
                6 => "17",
                7 => "18",
                8 => "19",
                9 => "20",
                10 => "21",
                11 => "23",
                12 => "24",
                _ => return None,
            };
            Some(csi(code, '~'))
        }
        key if m.control => control_char(key).and_then(|c| with_alt(&[c])),
        key => {
            // What the key types (⇧ and ⌥ included, as macOS composes it).
            let text = keystroke.key_char.clone().or_else(|| (key.chars().count() == 1).then(|| key.to_string()))?;
            if m.alt && keystroke.key_char.is_none() {
                return with_alt(text.as_bytes());
            }
            Some(Cow::Owned(text.into_bytes()))
        }
    }
}

/// The control character ⌃ makes of a key: ⌃A is 1, ⌃[ is ESC, ⌃? is DEL.
fn control_char(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (ch, None) = (chars.next()?, chars.next()) else { return None };
    Some(match ch.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' | '`' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Pasted text: newlines as Return, and wrapped in bracketed-paste markers
/// when the program asked for them (so a shell doesn't run each line).
pub fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A pasted end marker would end the paste early.
        let text = text.replace('\x1b', "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// A mouse button, or the wheel, as xterm numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 64,
    WheelDown = 65,
}

/// What happened to the mouse, for a program that asked to hear about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    /// Moved with the button down (or with none, `MOUSE_MOTION` only).
    Drag,
}

/// The report for a mouse event at a cell (0-based), in the encoding the
/// program chose (SGR, or xterm's original bytes), or `None` when it didn't
/// ask for this kind of event.
pub fn mouse_report(
    button: MouseButton,
    action: MouseAction,
    column: usize,
    line: usize,
    modifiers: &gpui_kit::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let wanted = match action {
        MouseAction::Press | MouseAction::Release => mode.intersects(TermMode::MOUSE_MODE),
        MouseAction::Drag => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
    };
    if !wanted {
        return None;
    }
    let mut code = button as u8 + 4 * modifiers.shift as u8 + 8 * modifiers.alt as u8 + 16 * modifiers.control as u8;
    if action == MouseAction::Drag {
        code += 32;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, line + 1).into_bytes());
    }
    if action == MouseAction::Release {
        // The original encoding doesn't say which button was let go.
        code = 3 + (code & !3);
    }
    // One byte each, so nothing past column or line 223.
    let at = |n: usize| (n < 223).then_some(32 + 1 + n as u8);
    Some(vec![0x1b, b'[', b'M', 32 + code, at(column)?, at(line)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(keys: &str, mode: TermMode) -> Option<Vec<u8>> {
        let mut keystroke = Keystroke::parse(keys).unwrap();
        if keystroke.key_char.is_none() && keystroke.key.chars().count() == 1 && !keystroke.modifiers.control {
            let ch = keystroke.key.clone();
            keystroke.key_char = Some(if keystroke.modifiers.shift { ch.to_uppercase() } else { ch });
        }
        bytes_for(&keystroke, mode).map(|b| b.into_owned())
    }

    fn plain(keys: &str) -> Option<Vec<u8>> {
        bytes(keys, TermMode::empty())
    }

    #[test]
    fn text_control_characters_and_editing_keys() {
        assert_eq!(plain("a"), Some(b"a".to_vec()));
        assert_eq!(plain("shift-a"), Some(b"A".to_vec()));
        assert_eq!(plain("ctrl-c"), Some(vec![3]));
        assert_eq!(plain("ctrl-d"), Some(vec![4]));
        assert_eq!(plain("ctrl-["), Some(vec![0x1b]));
        assert_eq!(plain("ctrl-\\"), Some(vec![0x1c]));
        assert_eq!(plain("ctrl-space"), Some(vec![0]));
        assert_eq!(plain("ctrl-alt-b"), Some(vec![0x1b, 2]));
        assert_eq!(plain("enter"), Some(b"\r".to_vec()));
        assert_eq!(plain("backspace"), Some(vec![0x7f]));
        assert_eq!(plain("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(plain("delete"), Some(b"\x1b[3~".to_vec()));
        assert_eq!(plain("pageup"), Some(b"\x1b[5~".to_vec()));
    }

    #[test]
    fn arrows_follow_the_cursor_mode_and_carry_modifiers() {
        assert_eq!(plain("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(bytes("up", TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));
        assert_eq!(plain("shift-right"), Some(b"\x1b[1;2C".to_vec()));
        assert_eq!(bytes("ctrl-left", TermMode::APP_CURSOR), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(plain("alt-left"), Some(b"\x1bb".to_vec()));
        assert_eq!(plain("f1"), Some(b"\x1bOP".to_vec()));
        assert_eq!(plain("f5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(plain("shift-f5"), Some(b"\x1b[15;2~".to_vec()));
    }

    #[test]
    fn command_keys_stay_zcode_s() {
        assert_eq!(plain("cmd-w"), None);
        assert_eq!(plain("cmd-k"), None);
        assert_eq!(plain("cmd-shift-t"), None);
    }

    #[test]
    fn mouse_reports_in_the_program_s_encoding() {
        let none = gpui_kit::Modifiers::default();
        let report = |button, action, mode| mouse_report(button, action, 4, 9, &none, mode);
        assert_eq!(report(MouseButton::Left, MouseAction::Press, TermMode::empty()), None);
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(report(MouseButton::Left, MouseAction::Press, sgr), Some(b"\x1b[<0;5;10M".to_vec()));
        assert_eq!(report(MouseButton::Left, MouseAction::Release, sgr), Some(b"\x1b[<0;5;10m".to_vec()));
        assert_eq!(report(MouseButton::WheelDown, MouseAction::Press, sgr), Some(b"\x1b[<65;5;10M".to_vec()));
        assert_eq!(report(MouseButton::Left, MouseAction::Drag, sgr), None, "clicks only");
        assert_eq!(report(MouseButton::Left, MouseAction::Drag, sgr | TermMode::MOUSE_DRAG), Some(b"\x1b[<32;5;10M".to_vec()));
        let x10 = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(report(MouseButton::Right, MouseAction::Press, x10), Some(vec![0x1b, b'[', b'M', 34, 37, 42]));
        assert_eq!(report(MouseButton::Right, MouseAction::Release, x10), Some(vec![0x1b, b'[', b'M', 35, 37, 42]));
    }

    #[test]
    fn pastes_bracketed_when_asked() {
        assert_eq!(paste("ls\nwc -l\n", TermMode::empty()), b"ls\rwc -l\r");
        assert_eq!(paste("a\x1b[201~b", TermMode::BRACKETED_PASTE), b"\x1b[200~a[201~b\x1b[201~");
    }
}

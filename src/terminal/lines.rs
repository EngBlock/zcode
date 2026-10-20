//! A terminal's output as lines of plain text, for whoever reads what a
//! program says (a web app's script: its URL and errors). Bytes come as
//! the PTY gives them; a line is what's left of it on screen: escape
//! sequences dropped, and what a carriage return (or a move to the first
//! column) wrote over replaced.

use std::sync::OnceLock;

use regex::Regex;

/// A line longer than this is passed on in pieces.
const MAX_LINE: usize = 64 * 1024;

#[derive(Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
}

impl LineSplitter {
    /// The lines `bytes` completes.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = vec![];
        for &byte in bytes {
            if byte == b'\n' {
                lines.push(finish(&std::mem::take(&mut self.partial)));
            } else {
                self.partial.push(byte);
                if self.partial.len() >= MAX_LINE {
                    lines.push(finish(&std::mem::take(&mut self.partial)));
                }
            }
        }
        lines
    }
}

fn finish(bytes: &[u8]) -> String {
    static TO_FIRST_COLUMN: OnceLock<Regex> = OnceLock::new();
    let text = String::from_utf8_lossy(bytes);
    // `ESC [ G` / `ESC [ 1 G` (column 1) rewrites the line as `\r` does.
    let text = TO_FIRST_COLUMN.get_or_init(|| Regex::new("\u{1b}\\[[01]?G").expect("valid regex")).replace_all(&text, "\r");
    let shown = text.rsplit('\r').find(|part| !crate::build::log::strip_ansi(part).trim().is_empty()).unwrap_or("");
    crate::build::log::strip_ansi(shown).trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_come_whole_and_plain() {
        let mut splitter = LineSplitter::default();
        assert!(splitter.feed(b"  \x1b[32m\xe2\x9e\x9c\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:").is_empty());
        assert_eq!(splitter.feed(b"\x1b[1m5173\x1b[22m/\x1b[39m\r\nnext"), ["  ➜  Local:   http://localhost:5173/"]);
        assert_eq!(splitter.feed(b" line\n"), ["next line"]);
        // Progress overwritten in place: what's left on screen.
        assert_eq!(splitter.feed(b"10%\r50%\r\x1b[2Kdone\n"), ["done"]);
        assert_eq!(splitter.feed(b"\x1b[2K\x1b[1Gbuilding\x1b[2K\x1b[1Gbuilt in 30ms\n"), ["built in 30ms"]);
        // A screen cleared before the banner.
        assert_eq!(splitter.feed(b"\x1b[2J\x1b[3J\x1b[H\n"), [""]);
    }
}

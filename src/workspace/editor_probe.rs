//! The test harness's handle on the editor's extension points
//! (`ZCODE_DEBUG_DIR`'s `editor` command): inlays, blocks, a highlight
//! layer and a two-key modal layer on the active editor, so e2e checks can
//! drive them with real keystrokes in the real window.

use std::rc::Rc;

use gpui_kit::*;
use zcode_editor::{Block, CursorShape, HighlightPriority, Inlay};

use super::Workspace;

const PROBE: &str = "e2e-probe";

impl Workspace {
    /// `inlay <offset> before|after <text>`, `block <row> <line>|<line>…`,
    /// `layer <start> <end>`, `modal on|off`, `clear`, `text <file>` (writes
    /// the text to the file, for checks that compare it).
    pub(crate) fn editor_probe(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let editor = self.active_editor().ok_or("no editor")?;
        let (command, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        editor.update(cx, |state, cx| {
            match command {
                "inlay" => {
                    let mut parts = rest.splitn(3, ' ');
                    let offset: usize = parts.next().and_then(|o| o.parse().ok()).ok_or("usage: inlay <offset> before|after <text>")?;
                    let side = parts.next().unwrap_or("after");
                    let text = parts.next().unwrap_or("").to_string();
                    let mut inlays = state.inlays(PROBE);
                    let inlay = Inlay::new(offset, text);
                    inlays.push(if side == "before" { inlay.before_text() } else { inlay });
                    state.set_inlays(PROBE, inlays, cx);
                }
                "block" => {
                    let (row, lines) = rest.split_once(' ').ok_or("usage: block <row> <line>|<line>")?;
                    let row: u32 = row.parse().map_err(|_| "bad row")?;
                    let mut blocks = state.blocks(PROBE);
                    blocks.push(Block::new(row, lines.split('|').map(str::to_string)));
                    state.set_blocks(PROBE, blocks, cx);
                }
                "layer" => {
                    let (start, end) = rest.split_once(' ').ok_or("usage: layer <start> <end>")?;
                    let range = start.parse().map_err(|_| "bad start")?..end.parse().map_err(|_| "bad end")?;
                    let style = HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() };
                    state.set_highlight_layer(PROBE, HighlightPriority::REFERENCES, vec![(range, style)], cx);
                }
                "modal" if rest == "on" => {
                    // A sliver of Vim's normal mode: `j` goes to the next line's
                    // start, `x` deletes the character under the caret.
                    state.set_cursor_shape(CursorShape::Block, cx);
                    state.set_extra_key_context(Some("vim_mode=normal".into()), cx);
                    state.set_keystroke_interceptor(
                        Some(Rc::new(|state, keystroke, window, cx| match keystroke.key.as_str() {
                            "j" if keystroke.modifiers.number_of_modifiers() == 0 => {
                                let text = state.text().to_string();
                                let cursor = state.cursor();
                                let next = text[cursor..].find('\n').map_or(cursor, |i| cursor + i + 1);
                                state.set_selected_range(next..next, cx);
                                true
                            }
                            "x" if keystroke.modifiers.number_of_modifiers() == 0 => {
                                let at = state.cursor();
                                let end = state.text().to_string()[at..].chars().next().map_or(at, |c| at + c.len_utf8());
                                state.transact(window, cx, |state, window, cx| state.edit([(at..end, "")], window, cx));
                                true
                            }
                            _ => false,
                        })),
                        cx,
                    );
                }
                "modal" => {
                    state.set_keystroke_interceptor(None, cx);
                    state.set_cursor_shape(CursorShape::Bar, cx);
                    state.set_extra_key_context(None, cx);
                }
                "text" => {
                    std::fs::write(rest, state.text().to_string()).map_err(|e| e.to_string())?;
                }
                "clear" => {
                    state.clear_inlays(PROBE, cx);
                    state.clear_blocks(PROBE, cx);
                    state.clear_highlight_layer(PROBE, cx);
                }
                other => return Err(format!("unknown editor probe {other}")),
            }
            Ok(())
        })?;
        let _ = window;
        cx.notify();
        Ok(())
    }

    /// The active editor's probe state, for the dump.
    pub(crate) fn editor_probe_state(&self, cx: &App) -> serde_json::Value {
        let Some(editor) = self.active_editor() else { return serde_json::Value::Null };
        let state = editor.read(cx);
        let caret = state.prepaint_caret_geometry().map(|g| g.cursor_bounds());
        serde_json::json!({
            "inlays": state.inlays(PROBE).iter().map(|i| serde_json::json!([i.offset, i.text.to_string()])).collect::<Vec<_>>(),
            "blocks": state.blocks(PROBE).iter().map(|b| serde_json::json!([b.row, b.lines.len()])).collect::<Vec<_>>(),
            "layer": state.highlight_layer_ranges(PROBE).iter().map(|r| serde_json::json!([r.start, r.end])).collect::<Vec<_>>(),
            "cursor_shape": format!("{:?}", state.cursor_shape()),
            "cursor": state.cursor(),
            "caret": caret.map(|b| serde_json::json!({ "x": f32::from(b.origin.x), "y": f32::from(b.origin.y), "w": f32::from(b.size.width) })),
            // Where the caret is drawn, after scrolling (`caret` is in the text).
            "caret_on_screen_y": state.prepaint_caret_geometry().map(|g| f32::from(g.cursor_bounds().origin.y + g.scroll_offset().y)),
            "rows": state.gutter_geometry().rows.iter().map(|&(row, top, _)| serde_json::json!([row + 1, f32::from(top)])).collect::<Vec<_>>(),
            // Folded lines, first to last, 1-based.
            "folds": state.folded_lines().iter().map(|f| serde_json::json!([f.start_line + 1, f.end_line + 1])).collect::<Vec<_>>(),
        })
    }
}

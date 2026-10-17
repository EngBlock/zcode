# Editor engine

Roadmap item 2.1: which code editor zcode builds on, and the extension
points the Phase 2 and 3 features need from it.

## What we had

zcode's editor was gpui-component's `Editor` over an `EditorState`. The
roadmap assumed that meant ~8k lines in `vendor/gpui-component/src/input`.
It doesn't: that directory is only the styled shell (the `Input` element
shared by every text field, the completion, code action, hover and
diagnostic popovers, the find panel, and the glue to the tree-sitter
highlighter). The engine itself is `gpui-base`'s `input` module, a separate
crate zcode didn't vendor: about 30k lines with tests, 17k without.

How that engine is built:

* **Buffer.** A `ropey` rope (`InputBaseState::text`), edited in place.
  Offsets are UTF-8 bytes everywhere; LSP positions are converted at the
  edges (`RopeExt`). There is no separate buffer model: the rope, the
  selections, the undo history and the view state are one GPUI entity.
* **Modes.** `InputBaseState<M>` is generic over a sealed mode marker:
  `InputMode` (text field), `TextareaMode`, `EditorMode`. `EditorState` is
  the editor mode; language features live in its `EditorExtras` and behind
  `InputModeKind` hooks the edit path calls.
* **Display map.** Two layers: `WrapMap` (soft wrap, per buffer line, in a
  `SumTree` with measured wrap boundaries) and `FoldMap` (folded wrap rows as
  sorted runs). `DisplayMap` maps buffer points to display rows and back.
  Rows have one height; the only exception is the inline-completion ghost
  block, which the element shifts by hand.
* **Layout and paint.** One custom element (`base/element.rs`, ~2.3k lines
  without tests) lays out the visible lines each frame: shapes each wrapped
  row with the highlight runs, computes selections, search matches, range
  decorations, indent guides, the gutter (line numbers, fold chevrons) and
  carets, then paints in a fixed order. Inline tokens (chips replacing a
  range of text) already lay out a row as fragments (`InputLine::inline`)
  and feed their widths to the wrapper.
* **Highlighting.** A trait (`InputHighlighter`) the tree-sitter highlighter
  in gpui-component implements, with incremental reparsing and a background
  parse for large files. Per frame the element composes, lowest first:
  tree-sitter styles, LSP semantic tokens, text decorations (collections in
  creation order, first wins), diagnostics. (2.4 replaced the engine's
  semantic-token provider with a highlight layer; see below.)
* **Decorations.** `TextDecorationCollection` (styles over ranges) and
  `RangeDecorationCollection` (fills and frames), indexed by an interval
  tree, adjusted on every edit; a range that becomes empty is dropped.
* **Input.** GPUI's `EntityInputHandler` for typing and IME, actions bound
  in the `Input` key context for everything else, mouse handlers on the
  element. Multiple cursors, column selection, auto-closing pairs,
  smart indent, completion and placeholders are all in the engine.
* **Undo.** An `UndoManager` of transactions of `Change`s (old and new text
  per range) with coalescing for typing and deletes and selection
  snapshots. It was private: undo was reachable only as the `Undo` action
  dispatched to a focused editor.

What zcode had to work around (Phase 1): three gutter overlays painted over
the editor (test diamonds, breakpoints, bookmarks) that read back where
lines landed and hit-tested clicks themselves; line anchors made of
invisible range decorations (so a deleted line's anchor was gone for good,
undo or not); a hidden zero-size editor so the property list editor could
forward ⌘Z; read-only checks outside the editor; and two gpui-component
patches.

## Options

**(a) Own the editor.** Move the engine and the editor UI into a crate in
this repository, keep the Apache-2.0 license and attribution, and change it
directly.

**(b) Keep the vendored editor and add an extension API.** The engine isn't
vendored, so this means vendoring all of gpui-base (64k lines, every other
widget's foundation too) and carrying patches inside an engine that is also
the engine of every text field, then rebasing them on each gpui-kit
release.

Zed's editor is not an option: it is GPL. Only its public architecture
write-ups (anchors over a CRDT buffer, a display map of inlays, folds, tabs,
wraps and blocks, multibuffers of excerpts) informed the shape of the APIs
below; no code was read or copied.

## Decision: (a), `crates/zcode-editor`

* Every Phase 2 feature needs changes in the layout and paint path, the
  display map and the undo history. Under (b) each would be another patch
  to a crate we don't own; under (a) they are ordinary commits with tests.
* The engine is well structured and well tested (349 tests came with it and
  run in the crate), so owning it is cheap: the copy compiled with path and
  visibility changes only.
* gpui-component's other widgets stay as they are: the pickers, forms and
  consoles keep using its text fields; the tree-sitter highlighter and its
  language registry stay shared with markdown rendering.
* Startup, typing and memory don't change: the code is the same code, the
  generic engine is only instantiated for the editor, and the duplicated
  single-line machinery is never instantiated (generic code that isn't
  used isn't compiled into the binary).

What moved:

| From | To |
|---|---|
| gpui-base 0.7.3 `src/input` (engine, display map, decorations, LSP providers, search, undo) | `crates/zcode-editor/src/{base,editor,input,textarea}` |
| gpui-base `text_boundary.rs`, `touch_selection.rs` (private helpers) | `src/base/` |
| gpui-component 0.7.3 `input/{editor,input}.rs` (editor path only) | `src/ui/element.rs` |
| gpui-component popovers, find panel, overlay host, default language rules, syntax-context provider, tree-sitter adapter | `src/ui/` |

Kept shared on purpose:

* The editing actions (`Undo`, `MoveLeft`, `Search`, …) and `InputEvent`
  are gpui-base's types, re-exported. gpui-base binds the editing keys for
  the `Input` key context, and the editor keeps that context, so the keymap
  and every zcode binding written against those actions work unchanged and
  no action name is registered twice.
* `HighlightStyleResolver` is gpui-base's trait, so gpui-component's
  highlight themes resolve for the editor.
* The tree-sitter highlighter stays in gpui-component; the patch there only
  makes the methods the editor's adapter calls public
  (`vendor/gpui-component/ZCODE-PATCH.md`). The popover padding patch moved
  into the crate with the popovers, so gpui-component's `input` module is
  upstream again.

Dropped from the copy: the macOS text-content-type hook (password fields),
the touch selection handles' painting, and the single-line element's
frame (prefix and suffix slots, clear button, mask toggle).

## Extension points

Each one is in its own module of `crates/zcode-editor/src`, has tests there,
and replaced a workaround in zcode.

### Undo, redo, transactions, read-only (`history.rs`)

```rust
editor.undo(window, cx);            // ⌘Z without dispatching an action
editor.redo(window, cx);
editor.can_undo(); editor.can_redo();
editor.transact(window, cx, |editor, window, cx| {   // one undo step;
    editor.edit([(range, "text"), (other, "")], window, cx);  // None when read-only
});
editor.set_read_only(true, cx);     // typing, paste, undo, transact refused
```

`edit` applies several ranges as one change; `set_value` and `edit` stay
available in a read-only editor, so a file can be reloaded.

Transactions have ids (2.5), the same through undo and redo, so an editor
can add to one it made earlier and undo exactly that one; and an editor can
hand ⌘Z and ⇧⌘Z to whoever keeps its history:

```rust
let t = editor.edit_joining(None, [(range, "text")], window, cx);  // Option<TransactionId>
editor.edit_joining(t, [(other, "more")], window, cx);   // joins t while it's still the last
editor.last_transaction(); editor.next_redo_transaction();
editor.undo_transaction(t, window, cx) -> bool;   // only if t is the last one
editor.redo_transaction(t, window, cx) -> bool;
editor.set_external_history(true);   // ⌘Z / ⇧⌘Z emit HistoryRequest::Undo / Redo instead;
                                     // edits are still recorded, so last_transaction tells them apart
``` Ported: the
property list editor's ⌘Z/⇧⌘Z call `undo`/`redo` on its text (the hidden
zero-size editor is gone), and zcode's line commands (comment, duplicate,
move line) run through `transact`, so a read-only package source refuses
them in the engine rather than in a workspace check.

### Anchors (`anchors.rs`)

```rust
let a = editor.anchor(offset, AnchorBias::Left);  // a position; the bias picks
                                                   // the side of text inserted at it
let line = editor.line_anchor(row);                // a whole line, newline included
editor.anchor_offset(a);   // Option<usize>
editor.anchor_row(line);   // Option<u32>: None while the line is deleted
editor.anchor_rows();      // every anchor's row, after an edit
editor.remove_anchor(a);
```

Anchors move with every edit. Text inserted at a line's start belongs to
that line (a newline typed there pushes it down), text at its end to the
next one; a line anchor is deleted when an edit removes the whole line with
its newline. Shifting past an edit is reversible by arithmetic, but landing
inside replaced text isn't, so each undo history entry (`Change`) records
the anchors its edit moved that way, and undoing the entry restores them
exactly, deleted lines included; redo replays the edit and moves them
again. `set_value` (a reload, which clears the history) keeps line anchors
on their row.

Ported: `src/workspace/line_anchors.rs` keeps breakpoints and bookmarks on
line anchors instead of invisible range decorations. It reports each line
as moved, deleted or restored; a breakpoint or bookmark deleted with its
line is kept with the anchor and comes back (same id, same place in the
bookmark list) with the undo that restores the line, which the invisible
decorations could never do.

### Gutter decorations (`gutter.rs`)

```rust
editor.set_gutter_lane(
    GutterLane::new("breakpoints", |rows: &GutterRows, cx| {
        // the items on rows.rows (visible 0-based rows), read straight from
        // the owner's model; rows.anchor_row(a) resolves anchors
        vec![GutterItem::new(row, |paint: &GutterPaint, window, cx| { /* paint in paint.bounds */ })
            .bounds(|cell, line_height| /* part of the cell */)
            .on_click(|e: &GutterEvent, window, cx| true)          // handled?
            .on_double_click(..).on_secondary_click(..)
            .on_hover(|hovered, window, cx| ..)
            .cursor(CursorStyle::PointingHand)]
    })
    .order(0)                                       // bottom to top, left to right
    .placement(GutterPlacement::OverNumbers)        // or BeforeNumbers(w) / AfterNumbers(w)
    .on_click(|e, window, cx| true),                // rows without an item
    cx,
);
editor.painted_gutter_items();   // lane, row, bounds as painted last frame
editor.gutter_geometry();        // number column and visible rows
```

The element asks each lane for its items in prepaint, for the visible rows
only, and paints them after the line numbers and before the fold chevrons.
Lanes with a column of their own widen the gutter and move the numbers and
chevrons over; lanes over the numbers stack by order. A mouse down goes to
the topmost item under the pointer whose handler takes it, then to the
topmost lane with a handler for rows without an item; the editor tracks the
hovered item, repaints it with `hovered` set and sets its cursor.
`GutterPaint::paint_line_number` repaints the number in another color (white
on a breakpoint). `set_line_number_labels(Some(Rc::new(|row, text, cx| ..)))`
shows other numbers than the rows' own, asked for the visible rows only (a
multibuffer shows each excerpt line's number in its file, and the number
column keeps room for five digits); `GutterPaint::number` is the row's.

Ported: breakpoint markers (order 0, over the numbers, reaching into the
fold column; click, double-click, right-click, click on an empty row adds
one), bookmark ribbons (order 1, right-click menu, left clicks fall through
to the breakpoint lane) and test diamonds (order 2, hover shows the play
glyph, click runs). The three canvases layered over the editor, their
`gutter_width` arithmetic with gpui-base's private constants, and their
own hit-testing are gone. Git change bars (2.3) are an
`AfterNumbers(px(6.))` lane (where Xcode draws them) whose items come from
hunks diffed again in the background after each edit, and Editor ▸ Authors
a `BeforeNumbers` lane that writes each commit's author and date with
`GutterPaint::paint_label` (text in the editor's font, aligned in an x range
and cut short with an ellipsis).

### Highlight layers (`highlights.rs`)

```rust
editor.set_highlight_layer("semantic-tokens", HighlightPriority::SEMANTIC_TOKENS,
    vec![(range, HighlightStyle { color: Some(..), ..Default::default() })], cx);
editor.clear_highlight_layer("semantic-tokens", cx);
```

Per frame, over the visible bytes: tree-sitter styles, then the layers
from the lowest priority to the highest, then text decorations, then
diagnostics. Where layers overlap, a higher layer's set fields win
and its unset fields let the lower ones show (a references background over
a semantic color). Each layer is sorted by start with a running maximum of
ends, so finding the visible ranges is a binary search even with overlaps;
ranges follow edits like decorations. An edit maps offsets in order, so a
layer stays sorted through it: moving the ranges skips those that end
before the edit and only recomputes the running maximum from there, which
matters for semantic tokens (a range per identifier, moved on every
keystroke). The composition is deterministic
(`compose_in_order`): GPUI's `combine_highlights` folds overlapping styles
in hash-set order, which can't express a priority, so decorations use the
new composition too.

Semantic tokens (2.4) are such a layer: zcode decodes sourcekit-lsp's
tokens into byte ranges and theme names off the main thread
(`src/lsp/semantic_tokens.rs`) and sets the layer in every view of the
file. The engine's own provider (`DocumentRangeSemanticTokensProvider`,
from gpui-base) was removed: nothing installed it, it fetched the whole
document 100 ms after every change, and its cache of LSP positions didn't
follow edits, so colors slid off their names until the next response.

### Inlays and blocks (`annotations.rs`)

```rust
editor.set_inlays("inlay-hints", vec![
    Inlay::new(offset, ": Int"),                     // belongs to the text before
    Inlay::new(arg_start, "name: ").before_text(),   // belongs to the text after
    Inlay::new(line_end, "  Jane, 2 days ago").color(muted),   // inline blame
], cx);
editor.set_blocks("edit-prediction", vec![Block::new(row, ["line 2", "line 3"])], cx);
editor.set_blocks("headers", vec![Block::new(row, ["a.swift"]).above().clickable().font_weight(FontWeight::SEMIBOLD)], cx);
cx.subscribe(&editor, |_, click: &BlockClicked, _| { /* click.key, .row, .line, .column */ });
editor.painted_blocks();   // group, row, line and bounds as painted last frame
editor.clear_inlays(key, cx); editor.clear_blocks(key, cx);
editor.inlays(key); editor.blocks(key);   // where they are after edits
```

Neither is in the buffer: offsets, selections, copy, search and the
language server never see them. Both sit on anchors, so they follow edits,
undo and redo; an inlay that belongs to the text before it moves with text
typed at its offset, one that belongs to the text after it stays in front.

An inlay is a zero-length fragment of its row (`InputLine::inline`, the
machinery inline tokens already used), shaped in the editor's font with its
own color and background. The caret at its offset is drawn before or after
it according to which text it belongs to; the caret steps over it in one
move because it has no offset; a click on it lands at its offset. The soft
wrapper gets every inlay's width (`set_inline_metrics`), so rows wrap where
their text plus their inlays reach the edge.

A block adds rows below its line: the line's layout gets the extra height,
so everything that stacks lines (text, backgrounds, selections, search
matches, decorations, line numbers, gutter lanes, fold chevrons, mouse
hit-testing) moves the rows below it down, and the few places that compute
a row's position from the display map (carets, the visible range, scroll
height, reveal) add the block rows above it. The caret moves past a block
in one step.

A block `above` its row (2.5) is laid out as a block below the row before
it, after that row's own blocks, so nothing that stacks rows changed; only
blocks above the first row have a place of their own: their rows count
before every row (`block_rows_before`), the visual rows they take map to
row 0, and the element paints them above the first line when it's on
screen. A clickable block's lines report a press as `BlockClicked` (with
the byte under the pointer, from the shaped line) instead of placing the
caret, and take the pointing hand. A multibuffer's excerpt headers are
both; Code Review puts lines deleted at a file's top above its first line.

The engine's LSP inline-completion ghost text predates this
and still paints over the text after the caret; edit predictions (3.3)
should use an inlay for the first line and a block for the rest instead.

### Signature help (`editor/lsp/signature_help.rs`)

```rust
editor.lsp_mut().signature_help_provider = Some(Rc::new(provider));  // trigger_characters(), signature_help()
editor.signature_help();          // Option<&SignatureHelpState>: the help shown
editor.show_signature_help(cx);   // ask at the caret, as `(` does
editor.dismiss_signature_help(cx);
split_signature_label(&signature, active_parameter);  // before, the argument, after
```

Typing a trigger character opens it; while it's open, every edit and caret
move asks again (the engine's `hide_context_menu` hook runs on both), a
retrigger character at once and anything else 120 ms later, each request
replacing the last; it closes when the provider has nothing for the caret
(past the call), on Escape (when no menu takes it) and on blur (a new
`on_blur` mode hook). The popover is the hover popover's element above the
caret, `sticky` so the mouse doesn't close it.

### The rows on screen (`viewport.rs`)

```rust
editor.visible_rows();   // Option<Range<u32>>: buffer rows shown last frame
cx.subscribe(&editor, |_, editor, event: &ViewportChanged, cx| { /* event.rows */ });
```

The element emits `ViewportChanged` after a frame that shows different
rows than the one before (a scroll, a resize, a fold, lines inserted
above), never for a frame showing the same rows, so typing on screen
doesn't. Inlay hints use it to ask for what's on screen.

### Keystroke interception, cursor shape, key context (`modal.rs`)

```rust
editor.set_keystroke_interceptor(Some(Rc::new(|editor, keystroke, window, cx| {
    // before key bindings and text input; true consumes the keystroke
    vim.handle(editor, keystroke, window, cx)
})), cx);
editor.set_cursor_shape(CursorShape::Block, cx);   // Bar, Block, Underline, Hollow
editor.set_extra_key_context(Some("vim_mode=normal".into()), cx);  // for "Editor && vim_mode == normal" bindings
```

A code editor's key context is `Input Editor mode=editor` plus the extra
one, so keymaps written as Zed's are (`"context": "Editor"`) reach it;
zcode's own bindings still say `Input && mode == editor`. Its settings
(2.8) are read back with `current_tab_size()`, `is_soft_wrapped()`,
`shows_line_numbers()` and `shows_whitespaces()`, and a theme's
`editor.foreground` colors its text when the theme has one (Xcode's
themes do).

The interceptor is a GPUI keystroke interceptor scoped to the editor's
focus, so it runs before the keymap and before text input; a consumed
keystroke neither dispatches an action nor types. A block caret covers the
character after the caret and repaints that character in the background
color; underline and hollow use the same width. With the selection and
editing APIs (`set_selected_range`, `add_selection` for visual block,
`edit`, `transact` for one-step undo of `dw`, `undo`), this is what Vim mode
(2.7) needs: normal, insert, visual (character, line, block) and replace
modes are states of the layer, not of the engine. Terminals
(`src/terminal/`) aren't editors and have no interceptor: their keys go to
the shell through their own key handler, in the `Terminal` key context,
where zcode's ⌃ and function-key commands are unbound (`TERMINAL_KEYS` in
`src/actions.rs`), so a Vim layer leaves them alone by construction.

### Vim mode (`vim/`, 2.7)

```rust
editor.set_vim_enabled(true, cx);          // normal mode; off removes the interceptor and observer
editor.vim_status();                       // Option<VimStatus>: mode, pending keys, command line, message
cx.subscribe(&editor, |_, _, _: &VimStatusChanged, cx| cx.notify());   // the status bar
cx.subscribe(&editor, |_, editor, command: &VimCommand, cx| { /* Write, Quit, NextTab, GoBack, Reindent */ });
editor.set_vim_message("E37: …", true, cx);  // a host refusing a command
editor.set_vim_search_style(style, cx);    // the matches of `/`
```

The layer is the interceptor above plus an observer of the editor itself.
Its state lives in the editor (`EditorExtras::vim`), so linked views each
have their own mode, cursors and marks. The modules: `keys.rs` (keystrokes
as Vim reads them; ⌘, ⌥ and the app's own ⌃ shortcuts pass), `text.rs`
(lines and characters as Vim counts them: the empty line after a final
newline isn't one), `motion.rs` (targets and whether they're exclusive,
inclusive or linewise), `object.rs`, `ops.rs` (operators over the ranges of
every cursor, paste, join, replace, increment), `normal.rs` (the command
parser, visual modes, `.`, scrolling and folds), `registers.rs` (a GPUI
global, the unnamed register written to the clipboard with its kind as
metadata), `search.rs` (Vim's magic patterns as Rust regexes),
`command.rs` (`:` ranges, commands and `:s` replacements).

Bindings in the app's keymap whose context names a Vim mode (`"context":
"Editor && vim_mode == normal"`, from `keymap.json`, 2.8) come before Vim's
own keys: while no Vim command is pending, a keystroke such a binding is
(or starts), and the keys that finish a multi-key binding underway, go to
the keymap instead of the layer. Other keys stay Vim's.

What it needed from the engine, beside `modal.rs`:

* `caret_heads`: in visual modes the element draws the caret on the
  character Vim's cursor is on (inside the selection), one for a block,
  instead of at each selection's end.
* `UndoManager::merge_after(mark)`: at Escape the transactions since the
  command that started an insert join into one, so `cw` and what was
  typed undo together, and the cursor goes back to where `cw` was. Not in
  an editor with an external history (a multibuffer keeps its own steps).
* `InputModeKind::on_mouse_down`: a click drops a half-typed command and
  restarts what `.` would repeat.
* `DisplayMap::fold_candidates` for `zc` and `zM`.

The layer never asks the engine for a "mode": a normal-mode cursor is a
collapsed selection kept on a character, visual modes are real selections
(so ⌘C copies them), visual block is a selection per row, and the
observer reads selections the layer didn't make (a click, ⌘D, a
placeholder selected by ⌃/) as a cursor or as visual mode. Every command
puts the cursors where undo should leave them and then edits in one
`transact`. `.` stores the command's keys and, for an insert, the text it
typed, worked out at Escape by comparing the text before and after around
the first cursor (each cursor types the same), so completions and closed
brackets repeat without keystroke replay.

### Edit events and read-only ranges (`edits.rs`)

```rust
cx.subscribe(&editor, |_, edit: &BufferEdit, cx| { /* edit.range (before), edit.new_text */ });
editor.set_read_only_ranges(vec![separator], cx);
```

`BufferEdit` reports every replacement in order (typing, commands, undo,
redo, and `set_value` as one whole-text edit). Read-only ranges refuse the
user's edits that replace any of them or insert strictly inside them;
inserting at their edges works, and like read-only mode they don't limit
undo or the programmatic APIs. Both are what a multibuffer needs, and
`BufferEdit` is also what incremental LSP sync needs (zcode still sends
whole documents).

### Linked views: one buffer in several editors (`linked.rs`)

```rust
let view = cx.new(|cx| EditorState::new(window, cx).language("swift"));
EditorState::link(&buffer_editor, &view, window, cx);  // view takes the text
view.read(cx).shares_buffer_with(buffer_editor.read(cx));  // true
buffer_editor.read(cx).linked_views();                     // the others still open
```

Two panes on one file (2.2) are two editors, each with its own selections,
scroll position, folds, soft wrap, search, popovers, gutter lanes and
decorations. Linked, they are one buffer: every edit one of them applies
(typing, commands, `edit`, `transact`, undo and redo, `set_value`) is
applied to the others inside the same update, so their texts are equal
whenever anyone reads them; they share one undo history, so ⌘Z in any view
undoes the last edit made in any of them (and puts the cursors back in the
view that undoes); and they share one anchor table, so an anchor made
through one view resolves in the others and moves once per edit. A view
applying another's edit keeps its own cursors (moved past the edit),
records nothing in the history and leaves the anchors alone: the view that
made the edit did both. The undo history and anchor table are shared
handles (`Rc<RefCell<…>>`) behind the same methods the engine used before,
so nothing else in the engine changed. Views hold each other weakly; the
last one open is the buffer.

zcode keeps one `Buffer` per file (dirty state, the language server's
document, breakpoint and bookmark anchors) whose `editor` is one of its
views, and gives each pane a view of its own; breakpoint markers drawn in
every pane come from the one set of anchors. Typing in a 6,000-line Swift
file shown in two panes takes as long as in one (keystroke to frame, median
3.9–4.6 ms on this branch with two panes, 3.9–5.3 ms on `master` with one).

### Multibuffers (`multibuffer.rs`, `multibuffer_editor.rs`)

A multibuffer is one editor showing editable excerpts of many files (2.5).
`multibuffer.rs` is the model: which ranges of which sources are shown,
the composed text (each excerpt's text, separated by a newline), and
positions and edits translated both ways; `MultiBufferEditor` hosts it in
an editor.

```rust
let mb = cx.new(|cx| MultiBufferEditor::new(host.clone(), HeaderStyle { color, background }, window, cx));
mb.update(cx, |mb, cx| {
    let source = mb.add_source(view_of_the_file, "Greeter.swift", "Sources/ZKit", window, cx);
    mb.push_excerpts(source, vec![4..7, 12..14], window, cx);   // rows; touching ones merge
    mb.expand(excerpt, 3, 3, window, cx);   // more lines; merges with the file's neighbor
    mb.expand_at_cursors(EXPAND_LINES, window, cx);   // ⇧↩
    mb.open_at_cursor(cx);                            // ⌥↩: emits Open { source, offset }
});
mb.read(cx).source_position(offset); mb.read(cx).host_position(source, offset);
mb.read(cx).source_row(row, cx);    // the file and row of a host row
cx.subscribe(&mb, |_, event: &MultiBufferEvent, _| { /* Open, ExcerptsChanged, Edited */ });
```

How it works:

1. Each source is an editor showing the file, a view linked to its buffer
   (`EditorState::link`), so it shares the file's text, history and
   anchors. Typing in the host is a `BufferEdit`; the model turns it into
   the file's edit (`edit_composed`, which also moves the file's later
   excerpts) and the source applies it with `edit_joining`, so it joins
   the file's history, quietly (a crate-internal flag keeps the source
   from reporting its own edit back). An edit of the file from anywhere
   else comes back through the source's `BufferEdit`s into the excerpts
   that show it (`edit_source`; text inserted at an excerpt's edge joins
   it), applied quietly to the host keeping its cursors. The multibuffer
   follows each file a change at a time, since events arrive after the
   update that made them (an undo of several joined edits is several
   events). A file replaced whole (a reload) keeps its excerpts' rows.
2. The separators are read-only ranges, and so are the excerpts of
   read-only files; an edit the files can't take (across a separator,
   inserted at a read-only excerpt's edge) is put back.
3. Undo: the host's history is external. A step is what the host records
   as one transaction (a typed word, a multi-cursor edit, Replace All) and
   holds the transactions it added to each file. ⌘Z undoes exactly those
   with `undo_transaction`, in each file, so each file's own history
   agrees (⌘Z in the file's tab undoes its part, and the multibuffer then
   keeps it as undone); the files' changes come back into the excerpts
   like any other, and the cursor goes to the first. A file edited since
   keeps its part; a step none of whose parts can be undone is dropped.
4. Headers: a clickable block above each excerpt's first row (the file's
   name and folder, `⋯` for the same file's next excerpt), rebuilt when
   excerpts change or an edit adds or removes a line; a click opens the
   file at the excerpt, a click on its `↕` expands the excerpt.
5. Line numbers: `set_line_number_labels` maps each host row to its
   file's row through the model; a click on a number opens the file there.
6. An empty host takes its first text whole, so the highlighter parses it
   from scratch (incremental edits from nothing never parse).

zcode's side (`src/workspace/multibuffer.rs`): the tab, find results,
references, issues and renames as excerpts, a page of files at a time as
the tab scrolls (`ViewportChanged`), match highlights as a highlight layer
per page, each file's diagnostics mapped into the host, issue messages as
blocks under their lines, and change bars and breakpoints as gutter lanes
that map rows to files. Files no pane had open get headless buffers (no
highlighting or language server document until they're edited or shown).
Typing in Swift excerpts measured 3.8–3.9 ms keystroke to frame (median),
against 5.1–5.2 ms in the same file's own tab; the first page of a
400-file search took 13–27 ms.

## How the upcoming features hook in

| Feature (roadmap) | Hooks |
|---|---|
| Git change bars (2.3, done) | a gutter lane `AfterNumbers(px(6.))` whose items come from hunks diffed again 30 ms after each edit, in the background (re-diffing turned out simpler than keeping hunks on anchors, and fast enough) |
| Inline blame (2.3, done) | an inlay group at the end of the cursor's line, replaced when the cursor moves, cleared while typing |
| Diff view (2.3, done) | side by side: two editors, blank blocks padding the shorter side of each change, fill decorations on changed lines, scroll offsets synced; inline: blocks for deleted lines under the row before them, above the first row for deletions at a file's top (2.5) |
| Inlay hints (2.4, done) | an inlay group per view and response (`before_text` for parameter names), on anchors so they survive typing until the next response; asked for the rows `visible_rows()` reports, again on `ViewportChanged` |
| Semantic tokens (2.4, done) | a highlight layer at `HighlightPriority::SEMANTIC_TOKENS` in every view of the file |
| Signature help (2.4, done) | `SignatureHelpProvider` next to hover and completion, and a popover above the caret (the hover popover's element, `sticky`) |
| Call/type hierarchy (2.4, done) | nothing in the engine: a tree in zcode's Find navigator |
| Multibuffers (2.5, done) | the excerpt model, `BufferEdit` events, read-only ranges, clickable blocks above rows for headers, other line numbers, transaction ids and an external history for one undo step across files (see above) |
| Vim mode (2.7, done) | the keystroke interceptor, cursor shapes, an extra key context, transactions for one-step undo of a command; carets drawn inside visual selections, an insert's transactions joined at Escape, a mouse-down hook (see above) |
| Settings, keymap, themes (2.8, done) | `set_tab_size`, `set_soft_wrap`, `set_line_number`, `set_indent_guides`, `set_show_whitespaces` on every open editor when a settings file changes; the `Editor` key context; Vim leaving keys to bindings made for its modes; `edit` for trimming whitespace on save (one undo step) |
| Edit predictions (3.3) | an inlay at the cursor for the first line and a block for the rest; Tab accepts through `edit` in a `transact` |
| Minimap (later) | an element beside the editor reading the rope and the highlight styles; the display map already gives row ↔ offset |
| Read-only views (package sources, generated interfaces) | `set_read_only`, or read-only ranges for parts of a file |
| Split panes (2.2, done) | linked views: an editor per pane, one text, history and anchor table |

## Performance

The extraction moved code without changing it, and the extension points cost
nothing when unused (no lanes, inlays, blocks or layers means the same paths
as before; gutter lanes query their owners for visible rows only).
Measured on this branch against `master`, alternating runs, release builds,
load average 4–14:

| | master | branch |
|---|---|---|
| Cold launch, welcome window, first pixels (median of 15) | 246.5 ms | 245.8 ms |
| Cold launch, ZKit with 3 Swift tabs restored, first pixels (median of 9) | 263.9 ms | 266.3 ms |
| Typing in the middle of a 6,000-line Swift file, 21 tabs open: keystroke → frame (median / p95) | 4.7 / 6.2 ms | 3.9–4.8 / 6.1–6.2 ms |
| Memory with those 21 tabs (resident / footprint) | 159 MB / 420 MB | 159 MB / 420 MB |

The binary grows by 3.6 MB (54 → 57.6 MB): gpui-base's engine stays linked
for gpui-component's text fields, next to this copy. Typing latency comes
from the control channel's `type-latency` command (dispatch to the end of
the next frame).

## Limits and follow-ups

* The engine is still generic over the text-field modes it came with; only
  the editor mode is instantiated. Removing the others (masks, number
  stepping, touch selection) would shrink the crate by a few thousand lines.
* The LSP inline-completion ghost text predates inlays and blocks and still
  paints over the text after the caret; 3.3 should replace it.
* Inlays aren't counted in the scroll width when soft wrap is off, and an
  editor using inline tokens (zcode's doesn't) doesn't wrap around inlays.
* Clicking a block that isn't clickable puts the caret at the start of the
  next line. A block above a row whose previous row is folded away hides
  with it.
* A multibuffer's host is highlighted in one language (zcode picks most
  files'), and has no language features of its own; an undo step skips a
  file edited since. Excerpts of one file are merged only with their
  neighbors in the list.
* Anchors come back exactly when undo replays the history entry that moved
  them; `set_value` (a reload) clears the history and keeps line anchors on
  their rows instead, so a line deleted before a reload stays deleted.

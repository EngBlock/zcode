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
  creation order, first wins), diagnostics.
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
available in a read-only editor, so a file can be reloaded. Ported: the
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
on a breakpoint).

Ported: breakpoint markers (order 0, over the numbers, reaching into the
fold column; click, double-click, right-click, click on an empty row adds
one), bookmark ribbons (order 1, right-click menu, left clicks fall through
to the breakpoint lane) and test diamonds (order 2, hover shows the play
glyph, click runs). The three canvases layered over the editor, their
`gutter_width` arithmetic with gpui-base's private constants, and their
own hit-testing are gone. Git change bars (2.3) will be a
`BeforeNumbers(px(3.))` lane over the hunks' anchors.

### Highlight layers (`highlights.rs`)

```rust
editor.set_highlight_layer("semantic-tokens", HighlightPriority::SEMANTIC_TOKENS,
    vec![(range, HighlightStyle { color: Some(..), ..Default::default() })], cx);
editor.clear_highlight_layer("semantic-tokens", cx);
```

Per frame, over the visible bytes: tree-sitter styles, the engine's own
LSP semantic tokens (`DocumentRangeSemanticTokensProvider`), then the
layers from the lowest priority to the highest, then text decorations,
then diagnostics. Where layers overlap, a higher layer's set fields win
and its unset fields let the lower ones show (a references background over
a semantic color). Each layer is sorted by start with a running maximum of
ends, so finding the visible ranges is a binary search even with overlaps;
ranges follow edits like decorations. The composition is deterministic
(`compose_in_order`): GPUI's `combine_highlights` folds overlapping styles
in hash-set order, which can't express a priority, so decorations use the
new composition too.

### Inlays and blocks (`annotations.rs`)

```rust
editor.set_inlays("inlay-hints", vec![
    Inlay::new(offset, ": Int"),                     // belongs to the text before
    Inlay::new(arg_start, "name: ").before_text(),   // belongs to the text after
    Inlay::new(line_end, "  Jane, 2 days ago").color(muted),   // inline blame
], cx);
editor.set_blocks("edit-prediction", vec![Block::new(row, ["line 2", "line 3"])], cx);
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
in one step. The engine's LSP inline-completion ghost text predates this
and still paints over the text after the caret; edit predictions (3.3)
should use an inlay for the first line and a block for the rest instead.

### Keystroke interception, cursor shape, key context (`modal.rs`)

```rust
editor.set_keystroke_interceptor(Some(Rc::new(|editor, keystroke, window, cx| {
    // before key bindings and text input; true consumes the keystroke
    vim.handle(editor, keystroke, window, cx)
})), cx);
editor.set_cursor_shape(CursorShape::Block, cx);   // Bar, Block, Underline, Hollow
editor.set_extra_key_context(Some("vim_mode=normal".into()), cx);  // for "Input && vim_mode == normal" bindings
```

The interceptor is a GPUI keystroke interceptor scoped to the editor's
focus, so it runs before the keymap and before text input; a consumed
keystroke neither dispatches an action nor types. A block caret covers the
character after the caret and repaints that character in the background
color; underline and hollow use the same width. With the selection and
editing APIs (`set_selected_range`, `add_selection` for visual block,
`edit`, `transact` for one-step undo of `dw`, `undo`), this is what Vim mode
(2.7) needs: normal, insert, visual (character, line, block) and replace
modes are states of the layer, not of the engine.

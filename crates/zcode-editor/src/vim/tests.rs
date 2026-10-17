//! Keystroke sequences in a real editor (headless): what Vim mode does to
//! the text, the cursors and the mode.
//!
//! States are written with `|` before the character each cursor is on and
//! `«…»` around selections.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{ClipboardItem, Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::test_support::open_editor;

struct VimTest {
    editor: Entity<EditorState>,
    cx: VisualTestContext,
    commands: Rc<RefCell<Vec<VimCommand>>>,
    _subscription: gpui::Subscription,
}

/// The text with `|` at each collapsed selection and `«»` around the others.
fn marked(state: &EditorState) -> String {
    let text = state.text().to_string();
    let mut marks: Vec<(usize, &str)> = vec![];
    for sel in state.selections.iter() {
        if sel.is_empty() {
            marks.push((sel.start, "|"));
        } else {
            marks.push((sel.start, "«"));
            marks.push((sel.end, "»"));
        }
    }
    marks.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| (a.1 == "»").cmp(&(b.1 == "»")).reverse()));
    let mut out = String::new();
    let mut last = 0;
    for (offset, mark) in marks {
        out.push_str(&text[last..offset]);
        out.push_str(mark);
        last = offset;
    }
    out.push_str(&text[last..]);
    out
}

impl VimTest {
    fn new(cx: &mut TestAppContext, marked_text: &str) -> Self {
        let cursors: Vec<usize> = marked_text.match_indices('|').enumerate().map(|(i, (o, _))| o - i).collect();
        let text = marked_text.replace('|', "");
        let (editor, mut cx) = open_editor(cx, &text);
        let commands = Rc::new(RefCell::new(vec![]));
        let subscription = cx.update(|window, cx| {
            let commands = commands.clone();
            let subscription = cx.subscribe(&editor, move |_, command: &VimCommand, _| commands.borrow_mut().push(command.clone()));
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                if let Some((&first, rest)) = cursors.split_first() {
                    state.set_selected_range(first..first, cx);
                    for &at in rest {
                        state.add_selection(at..at, cx);
                    }
                }
                state.set_vim_enabled(true, cx);
            });
            subscription
        });
        cx.run_until_parked();
        VimTest { editor, cx, commands, _subscription: subscription }
    }

    /// Space-separated keystrokes, as `simulate_keystrokes` reads them.
    fn keys(&mut self, keys: &str) -> &mut Self {
        self.cx.simulate_keystrokes(keys);
        self
    }

    /// Typed text.
    fn input(&mut self, text: &str) -> &mut Self {
        self.cx.simulate_input(text);
        self
    }

    #[track_caller]
    fn assert(&mut self, expected: &str) -> &mut Self {
        let actual = self.editor.read_with(&self.cx, |state, _| marked(state));
        assert_eq!(actual, expected);
        self
    }

    #[track_caller]
    fn mode(&mut self, mode: VimMode) -> &mut Self {
        assert_eq!(self.status().mode, mode);
        self
    }

    fn status(&self) -> VimStatus {
        self.editor.read_with(&self.cx, |state, _| state.vim_status().unwrap())
    }

    fn text(&self) -> String {
        self.editor.read_with(&self.cx, |state, _| state.text().to_string())
    }

    fn update(&mut self, f: impl FnOnce(&mut EditorState, &mut Window, &mut Context<EditorState>)) -> &mut Self {
        let editor = self.editor.clone();
        self.cx.update(|window, cx| editor.update(cx, |state, cx| f(state, window, cx)));
        self.cx.run_until_parked();
        self
    }
}

#[gpui::test]
fn starts_in_normal_mode_with_a_block_cursor_and_moves_without_typing(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\nthree\n");
    t.mode(VimMode::Normal);
    t.editor.read_with(&t.cx, |state, _| {
        assert_eq!(state.cursor_shape(), CursorShape::Block);
        assert_eq!(state.extra_key_context.as_deref(), Some("vim_mode=normal"));
    });
    t.keys("l l l l").assert("one |two\nthree\n");
    t.keys("j").assert("one two\nthre|e\n");
    t.keys("$").assert("one two\nthre|e\n");
    t.keys("k").assert("one tw|o\nthree\n");
    t.keys("0").assert("|one two\nthree\n");
    t.keys("q Q K").assert("|one two\nthree\n");
    t.keys("G").assert("one two\n|three\n");
    t.keys("g g").assert("|one two\nthree\n");
    t.keys("2 G").assert("one two\n|three\n");
    t.keys("i").mode(VimMode::Insert).input("x").assert("one two\nx|three\n");
    t.editor.read_with(&t.cx, |state, _| assert_eq!(state.cursor_shape(), CursorShape::Bar));
    t.keys("escape").mode(VimMode::Normal).assert("one two\n|xthree\n");
}

#[gpui::test]
fn word_motions_counts_and_finds(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|foo.bar(baz, qux) end");
    t.keys("w").assert("foo|.bar(baz, qux) end");
    t.keys("W").assert("foo.bar(baz, |qux) end");
    t.keys("b b").assert("foo.bar(|baz, qux) end");
    t.keys("e").assert("foo.bar(ba|z, qux) end");
    t.keys("0 3 w").assert("foo.bar|(baz, qux) end");
    t.keys("f )").assert("foo.bar(baz, qux|) end");
    t.keys("%").assert("foo.bar|(baz, qux) end");
    t.keys("t ,").assert("foo.bar(ba|z, qux) end");
    t.keys("0 f a").assert("foo.b|ar(baz, qux) end");
    t.keys(";").assert("foo.bar(b|az, qux) end");
    t.keys(",").assert("foo.b|ar(baz, qux) end");
    t.keys("$ g e").assert("foo.bar(baz, qux|) end");
}

#[gpui::test]
fn operators_with_motions(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two three four\nnext line\n");
    t.keys("d w").assert("|two three four\nnext line\n");
    t.keys("2 d w").assert("|four\nnext line\n");
    t.keys("d w").assert("|\nnext line\n");
    t.keys("u u u").assert("|one two three four\nnext line\n");
    t.keys("w d 2 e").assert("one | four\nnext line\n");
    t.keys("u $ d b").assert("one two three |r\nnext line\n");
    t.keys("u 0 d $").assert("|\nnext line\n");
    t.keys("u w D").assert("one| \nnext line\n");
    t.keys("u 0 c w").mode(VimMode::Insert).input("ONE").keys("escape").assert("ON|E two three four\nnext line\n");
    t.keys("w x").assert("ONE |wo three four\nnext line\n");
    t.keys("X").assert("ONE|wo three four\nnext line\n");
    t.keys("2 x").assert("ONE| three four\nnext line\n");
}

#[gpui::test]
fn whole_lines(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "one\n  |two\nthree\nfour\n");
    t.keys("d d").assert("one\n|three\nfour\n");
    t.keys("p").assert("one\nthree\n  |two\nfour\n");
    t.keys("k P").assert("one\n  |two\nthree\n  two\nfour\n");
    t.keys("2 d d").assert("one\n  |two\nfour\n");
    t.keys("y y j p").assert("one\n  two\nfour\n  |two\n");
    t.keys("g g y j G p").assert("one\n  two\nfour\n  two\n|one\n  two\n");
    t.keys("g g j c c").mode(VimMode::Insert).input("TWO").keys("escape").assert("one\n  TW|O\nfour\n  two\none\n  two\n");
    t.keys("G d d").assert("one\n  TWO\nfour\n  two\n|one\n");
    t.keys("d k").assert("one\n  TWO\n|four\n");
}

#[gpui::test]
fn the_last_line_without_a_newline(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "one\n|two");
    t.keys("y y p").assert("one\ntwo\n|two");
    t.keys("d d").assert("one\n|two");
    t.keys("d d").assert("|one");
    t.keys("d d").assert("|");
}

#[gpui::test]
fn text_objects(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "call(\"a |b\", [1, 2])");
    t.keys("d i \"").assert("call(\"|\", [1, 2])");
    t.keys("u d a (").assert("cal|l");
    t.keys("u f 2 c i [").mode(VimMode::Insert).input("3").keys("escape").assert("call(\"a b\", [|3])");
    t.keys("0 d i w").assert("|(\"a b\", [3])");

    let mut t = VimTest::new(cx, "if x {\n    |a\n    b\n}\n");
    t.keys("d i {").assert("if x {\n|}\n");
    t.keys("u c i {").mode(VimMode::Insert).input("c").keys("escape").assert("if x {\n    |c\n}\n");

    let mut t = VimTest::new(cx, "a\n|b\n\nc\n");
    t.keys("d a p").assert("|c\n");
    let mut t = VimTest::new(cx, "<a><b>x|y</b></a>");
    t.keys("d i t").assert("<a><b>|</b></a>");
    t.keys("u d a t").assert("<a>|</a>");
    let mut t = VimTest::new(cx, "one |two three");
    t.keys("d a w").assert("one |three");
    t.keys("y i w P").assert("one thre|ethree");
}

#[gpui::test]
fn visual_modes(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\nthree four\nfive\n");
    t.keys("v e").mode(VimMode::Visual).assert("«one» two\nthree four\nfive\n");
    t.editor.read_with(&t.cx, |state, _| assert_eq!(state.caret_heads, Some(vec![2])));
    t.keys("o").assert("«one» two\nthree four\nfive\n");
    t.editor.read_with(&t.cx, |state, _| assert_eq!(state.caret_heads, Some(vec![0])));
    t.keys("o w d").mode(VimMode::Normal).assert("|wo\nthree four\nfive\n");
    t.keys("u");
    t.keys("V j").mode(VimMode::VisualLine).assert("«one two\nthree four\n»five\n");
    t.keys("y").mode(VimMode::Normal).assert("|one two\nthree four\nfive\n");
    t.keys("G p").assert("one two\nthree four\nfive\n|one two\nthree four\n");
    t.keys("g g ctrl-v j l").mode(VimMode::VisualBlock).assert("«on»e two\n«th»ree four\nfive\none two\nthree four\n");
    t.keys("d").mode(VimMode::Normal).assert("|e two\nree four\nfive\none two\nthree four\n");
    // A block goes in at the same column on each row.
    t.keys("$ p").assert("e two|on\nree fthour\nfive\none two\nthree four\n");
    t.keys("u g v").mode(VimMode::VisualBlock);
    t.keys("escape g g w v i w").assert("e «two»\nree four\nfive\none two\nthree four\n");
    t.keys("U").assert("e |TWO\nree four\nfive\none two\nthree four\n");
    t.keys("j 0 v $ ~").assert("e TWO\n|REE FOUR\nfive\none two\nthree four\n");
    t.keys("V j >").assert("e TWO\n  |REE FOUR\n  five\none two\nthree four\n");
    t.keys("V j <").assert("e TWO\n|REE FOUR\nfive\none two\nthree four\n");
    t.keys("V j J").assert("e TWO\nREE FOUR| five\none two\nthree four\n");
}

#[gpui::test]
fn visual_block_insert_and_append_type_on_every_row(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|let a\nlet bb\nlet c\n");
    t.keys("ctrl-v j j I").mode(VimMode::Insert).input("// ").keys("escape").assert("|// let a\n// let bb\n// let c\n");
    t.keys("w w ctrl-v j j $ A").input(";").keys("escape").assert("// let a|;\n// let bb;\n// let c;\n");
    t.keys("g g ctrl-v j l c").input("#").keys("escape").assert("|# let a;\n# let bb;\n// let c;\n");
}

#[gpui::test]
fn counts_on_inserts_and_dot_repeat(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|x\n");
    t.keys("3 i").input("ab").keys("escape").assert("ababa|bx\n");
    t.keys("o").input("line").keys("escape").assert("ababab x\nlin|e\n".replace(" x", "x").as_str());
    t.keys("2 .").assert("abababx\nline\nline\nlin|e\n");
    t.keys("g g c w").input("one").keys("escape").assert("on|e\nline\nline\nline\n");
    t.keys("j 0 .").assert("one\non|e\nline\nline\n");
    t.keys("j A").input(";").keys("escape j .").assert("one\none\nline;\nline|;\n");
    t.keys("g g d w j .").assert("\n|\nline;\nline;\n");
    t.keys("u u").assert("|one\none\nline;\nline;\n");
}

#[gpui::test]
fn dot_repeats_typed_text_with_completions_and_pairs(cx: &mut TestAppContext) {
    // What an insert typed is taken from the text, so text the editor
    // added (a closing bracket, a completion) repeats as well.
    let mut t = VimTest::new(cx, "|a\nb\n");
    t.keys("A").update(|state, window, cx| {
        state.vim_type("(", window, cx);
        // A completion replacing text before the cursor.
        let at = state.cursor();
        state.edit([(at - 1..at, "foo(")], window, cx);
    });
    t.keys("escape").assert("afoo|()\nb\n");
    t.keys("j .").assert("afoo()\nbfoo|()\n");
}

#[gpui::test]
fn an_insert_undoes_in_one_step_with_the_change_that_started_it(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\n");
    t.keys("c w").input("uno").keys("enter").input("dos").keys("escape").assert("uno\ndo|s two\n");
    t.keys("u").mode(VimMode::Normal).assert("|one two\n");
    t.keys("ctrl-r").assert("|uno\ndos two\n");
    assert_eq!(t.text(), "uno\ndos two\n");
    t.keys("u").assert("|one two\n");
}

#[gpui::test]
fn registers_and_the_clipboard(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|alpha beta gamma\n");
    t.keys("\" a y w w \" b y w");
    t.keys("$ \" a p").assert("alpha beta gammaalpha| \n");
    t.keys("u \" b P").assert("alpha beta gammbeta| a\n");
    t.keys("u 0 \" A y w \" a P").assert("alpha alpha| alpha beta gamma\n");
    t.keys("u");
    // The unnamed register is the clipboard.
    t.keys("y i w");
    t.cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().and_then(|i| i.text()).as_deref(), Some("alpha")));
    t.cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("pasted".into())));
    t.keys("P").assert("paste|dalpha beta gamma\n");
    // The black hole keeps the clipboard; "0 still has the last yank.
    t.keys("\" _ d w \" 0 P").assert("pastealph|abeta gamma\n");
    // The only line deleted leaves an empty text, without a newline.
    t.keys("d d \" 1 p").assert("\n|pastealphabeta gamma");
}

#[gpui::test]
fn search_and_star(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|let count = 1\nlet x = count + counter\ncount\n");
    t.keys("/ c o u n t");
    assert_eq!(t.status().command_line.as_deref(), Some("/count"));
    t.keys("enter").assert("let |count = 1\nlet x = count + counter\ncount\n");
    t.keys("n").assert("let count = 1\nlet x = |count + counter\ncount\n");
    t.keys("N N").assert("let count = 1\nlet x = count + counter\n|count\n");
    assert_eq!(t.status().message.as_deref(), Some("search hit TOP, continuing at BOTTOM"));
    t.keys("g g w *").assert("let count = 1\nlet x = |count + counter\ncount\n");
    t.keys("n").assert("let count = 1\nlet x = count + counter\n|count\n");
    t.keys("#").assert("let count = 1\nlet x = |count + counter\ncount\n");
    t.keys("g g d / = enter").assert("|= 1\nlet x = count + counter\ncount\n");
    t.keys("/ z z z enter");
    assert_eq!(t.status().message.as_deref(), Some("E486: Pattern not found: zzz"));
    assert!(t.status().error);
    t.keys("? l e t enter").assert("= 1\n|let x = count + counter\ncount\n");
    t.keys(": n o h enter");
}

#[gpui::test]
fn marks_follow_edits(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "one\ntw|o\nthree\n");
    t.keys("m a g g O").input("zero").keys("escape");
    t.keys("` a").assert("zero\none\ntw|o\nthree\n");
    t.keys("g g ' a").assert("zero\none\n|two\nthree\n");
    t.keys("G d ' a").assert("zero\n|one\n");
    t.keys("' '").assert("zero\n|one\n");
}

#[gpui::test]
fn command_line(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|a1 a2\nb a\nc a a\nd\n");
    t.keys(": 3 enter").assert("a1 a2\nb a\n|c a a\nd\n");
    t.keys(": s / a / X enter").assert("a1 a2\nb a\n|c X a\nd\n");
    t.keys(": % s / a / Y / g enter").assert("Y1 Y2\nb Y\n|c X Y\nd\n");
    assert_eq!(t.status().message.as_deref(), Some("4 substitutions on 3 lines"));
    t.keys("u").assert("|a1 a2\nb a\nc X a\nd\n");
    t.keys("V j : s / \\ ( . \\ ) $ / < \\ 1 > enter").assert("a1 a<2>\n|b <a>\nc X a\nd\n");
    t.keys(": 2 , 3 d enter").assert("a1 a<2>\n|d\n");
    t.keys(": f r o b enter");
    assert_eq!(t.status().message.as_deref(), Some("E492: Not an editor command: frob"));
    t.keys(": w enter : q enter : w q enter : q a ! enter Z Z");
    assert_eq!(
        *t.commands.borrow(),
        vec![
            VimCommand::Write { all: false, quit: false },
            VimCommand::Quit { all: false, force: false },
            VimCommand::Write { all: false, quit: true },
            VimCommand::Quit { all: true, force: true },
            VimCommand::Write { all: false, quit: true },
        ]
    );
    // History: ↑ brings the last command back.
    t.keys(": up");
    assert_eq!(t.status().command_line.as_deref(), Some(":qa!"));
    t.keys("escape");
    assert_eq!(t.status().command_line, None);
}

#[gpui::test]
fn escape_closes_find_before_leaving_insert_mode(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|abc\n");
    t.keys("i").update(|state, _, cx| state.open_search(false, cx));
    t.update(|state, window, cx| state.focus(window, cx));
    assert!(t.editor.read_with(&t.cx, |state, _| state.search_session.is_active()));
    t.keys("escape").mode(VimMode::Insert);
    assert!(!t.editor.read_with(&t.cx, |state, _| state.search_session.is_active()));
    t.keys("escape").mode(VimMode::Normal);
}

#[gpui::test]
fn every_cursor_moves_and_operates(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\n|three four\n");
    t.keys("w").assert("one |two\nthree |four\n");
    t.keys("d w").assert("one| \nthree| \n");
    t.keys("u");
    t.keys("i").input("X").keys("escape").assert("one |Xtwo\nthree |Xfour\n");
    t.keys("l .").assert("one X|Xtwo\nthree X|Xfour\n");
    t.keys("escape").assert("one X|Xtwo\nthree XXfour\n");
}

#[gpui::test]
fn selections_made_elsewhere_are_visual_mode(cx: &mut TestAppContext) {
    // A placeholder selected by Tab or ⌃/ (or the mouse, or ⌘D).
    let mut t = VimTest::new(cx, "|f(<#name#>)\n");
    t.update(|state, _, cx| state.set_selected_range(2..10, cx));
    t.mode(VimMode::Visual);
    t.keys("c").mode(VimMode::Insert).input("x").keys("escape").assert("f(|x)\n");
    // A click in visual mode ends it.
    t.keys("v l").mode(VimMode::Visual);
    t.update(|state, _, cx| state.set_selected_range(0..0, cx)).mode(VimMode::Normal);
    // A click past the end of a line puts the cursor back on its last character.
    t.update(|state, _, cx| state.set_selected_range(5..5, cx)).assert("f(x|)\n");
}

#[gpui::test]
fn replace_mode_and_r(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|abcd\n");
    t.keys("R").mode(VimMode::Replace).keys("x y").assert("xy|cd\n");
    t.keys("backspace").assert("x|bcd\n");
    t.keys("z z z").assert("xzzz|\n");
    t.keys("escape").mode(VimMode::Normal).assert("xzz|z\n");
    t.keys("u").assert("|abcd\n");
    t.keys("3 r -").assert("--|-d\n");
    t.keys("9 r -").assert("--|-d\n");
    t.keys("0 r enter").assert("\n|--d\n");
}

#[gpui::test]
fn case_join_increment_and_indent(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|hello world\n    next\nx = 9;\n");
    t.update(|state, _, cx| state.set_tab_size(crate::TabSize { tab_size: 4, hard_tabs: false }, cx));
    t.keys("~ ~").assert("HE|llo world\n    next\nx = 9;\n");
    t.keys("g U i w").assert("|HELLO world\n    next\nx = 9;\n");
    t.keys("g u u").assert("|hello world\n    next\nx = 9;\n");
    t.keys("g ~ w").assert("|HELLO world\n    next\nx = 9;\n");
    t.keys("J").assert("HELLO world| next\nx = 9;\n");
    t.keys("u g J").assert("HELLO world|    next\nx = 9;\n");
    t.keys("j 0 ctrl-a").assert("HELLO world    next\nx = 1|0;\n");
    t.keys("5 ctrl-x").assert("HELLO world    next\nx = |5;\n");
    t.keys("> >").assert("HELLO world    next\n    |x = 5;\n");
    t.keys("< <").assert("HELLO world    next\n|x = 5;\n");
}

#[gpui::test]
fn insert_mode_control_keys(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|\n");
    t.keys("i").input("one two").keys("ctrl-w").assert("one |\n");
    t.keys("ctrl-u").assert("|\n");
    t.input("abc").keys("ctrl-o 0").mode(VimMode::Insert).assert("|abc\n");
    t.keys("ctrl-o").mode(VimMode::Normal).keys("$").mode(VimMode::Insert).assert("ab|c\n");
    t.keys("escape").mode(VimMode::Normal);
    let mut t = VimTest::new(cx, "|word\n");
    t.keys("y i w A space ctrl-r \"").assert("word word|\n");
    t.keys("ctrl-o 0").mode(VimMode::Insert).assert("|word word\n");
    t.keys("ctrl-t").assert("  |word word\n");
    t.keys("ctrl-d").assert("|word word\n");
}

#[gpui::test]
fn read_only_editors_move_but_dont_change(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one\ntwo\n");
    t.update(|state, _, cx| state.set_read_only(true, cx));
    t.keys("d d x j").assert("one\n|two\n");
    t.keys("y y k p").assert("|one\ntwo\n");
}

#[gpui::test]
fn turned_off_it_types_again(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|ab\n");
    t.keys("v l").update(|state, _, cx| state.set_vim_enabled(false, cx));
    t.editor.read_with(&t.cx, |state, _| {
        assert_eq!(state.cursor_shape(), CursorShape::Bar);
        assert_eq!(state.extra_key_context, None);
        assert_eq!(state.caret_heads, None);
        assert!(state.vim_status().is_none());
    });
    t.update(|state, _, cx| state.set_selected_range(0..0, cx));
    t.input("j");
    assert_eq!(t.text(), "jab\n");
}

#[gpui::test]
fn pending_keys_show_in_the_status(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\n");
    t.keys("2 \" a d");
    assert_eq!(t.status().pending.as_ref(), "2\"ad");
    t.editor.read_with(&t.cx, |state, _| assert_eq!(state.cursor_shape(), CursorShape::Underline));
    t.keys("escape");
    assert_eq!(t.status().pending.as_ref(), "");
    t.keys("cmd-a");
    assert_eq!(t.status().pending.as_ref(), "");
}

#[test]
fn what_an_insert_typed() {
    let before = Rope::from_str("abc def");
    // Typed "xy" at 3.
    let typed_text = |text: &str, after, caret| Typed { before: 0, after, text: text.into(), caret };
    assert_eq!(typed(&before, &Rope::from_str("abcxy def"), &[3], 5), Some(typed_text("xy", 0, 2)));
    // Backspaced over "c" and typed "Z".
    assert_eq!(typed(&before, &Rope::from_str("abZ def"), &[3], 3), Some(Typed { before: 1, after: 0, text: "Z".into(), caret: 1 }));
    // Typing what's already there is still typing at the cursor.
    assert_eq!(typed(&Rope::from_str("aa"), &Rope::from_str("aaa"), &[1], 2), Some(typed_text("a", 0, 1)));
    // Two cursors, each typing "!".
    assert_eq!(typed(&before, &Rope::from_str("a!bc d!ef"), &[1, 5], 2), Some(typed_text("!", 0, 1)));
    // A pair closed for the cursor: it's inside.
    assert_eq!(typed(&before, &Rope::from_str("abc() def"), &[3], 4), Some(typed_text("()", 0, 1)));
}


/// Signature help for any call the caret is in.
struct Calls;

impl crate::SignatureHelpProvider for Calls {
    fn trigger_characters(&self, _: &gpui::App) -> (Vec<String>, Vec<String>) {
        (vec!["(".into()], vec![])
    }

    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        _: lsp_types::SignatureHelpContext,
        _: &mut gpui::App,
    ) -> gpui::Task<anyhow::Result<Option<lsp_types::SignatureHelp>>> {
        let before = text.slice(..offset).to_string();
        let inside = before.rfind('(').is_some_and(|open| !before[open..].contains(')'));
        gpui::Task::ready(Ok(inside.then(|| lsp_types::SignatureHelp {
            signatures: vec![lsp_types::SignatureInformation {
                label: "f(a: Int)".into(),
                documentation: None,
                parameters: None,
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: Some(0),
        })))
    }
}

#[gpui::test]
fn escape_closes_signature_help_before_leaving_insert_mode(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|\n");
    t.update(|state, _, _| state.lsp_mut().signature_help_provider = Some(Rc::new(Calls)));
    let shown = |t: &VimTest| t.editor.read_with(&t.cx, |state, _| state.signature_help().is_some());
    t.keys("i").input("f(");
    t.cx.run_until_parked();
    assert!(shown(&t));
    t.keys("escape").mode(VimMode::Insert);
    assert!(!shown(&t));
    t.keys("escape").mode(VimMode::Normal);
    // In normal mode the cursor moving doesn't bring it back.
    t.keys("h");
    t.cx.run_until_parked();
    assert!(!shown(&t));
}

#[gpui::test]
fn the_apps_control_keys_still_reach_the_keymap(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one\n");
    let editor = t.editor.clone();
    // ⌃6 (Document Items) and ⌃I (Re-Indent) aren't Vim's; ⌃D is.
    let handled = |t: &mut VimTest, key: &str| {
        let keystroke = gpui::Keystroke::parse(key).unwrap();
        t.cx.update(|window, cx| editor.update(cx, |state, cx| state.vim_keystroke(&keystroke, window, cx)))
    };
    assert!(!handled(&mut t, "ctrl-6"));
    assert!(!handled(&mut t, "ctrl-i"));
    assert!(handled(&mut t, "ctrl-d"));
    assert!(!handled(&mut t, "cmd-s"));
}

gpui::actions!(vim_keymap_test, [OpenSomething]);

#[gpui::test]
fn a_binding_made_for_a_vim_mode_wins_over_vims_own_keys(cx: &mut TestAppContext) {
    let mut t = VimTest::new(cx, "|one two\nthree\n");
    let hits = Rc::new(std::cell::Cell::new(0));
    t.cx.update(|_, cx| {
        cx.bind_keys([gpui::KeyBinding::new("space f", OpenSomething, Some("Editor && vim_mode == normal"))]);
        let hits = hits.clone();
        cx.on_action(move |_: &OpenSomething, _| hits.set(hits.get() + 1));
    });
    t.cx.run_until_parked();
    t.keys("space f");
    assert_eq!(hits.get(), 1, "the keymap's binding ran");
    t.assert("|one two\nthree\n");
    // Keys no such binding starts with are still Vim's.
    t.keys("w").assert("one |two\nthree\n");
    // Not in insert mode, where the binding's context doesn't match.
    t.keys("i space").mode(VimMode::Insert);
    assert_eq!(t.text(), "one  two\nthree\n");
}

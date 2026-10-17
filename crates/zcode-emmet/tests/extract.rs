//! Finding the abbreviation before the cursor, and whether to offer it.

use zcode_emmet::{Syntax, extract_abbreviation, is_plausible};

fn html(line: &str) -> Option<(usize, &str)> {
    extract_abbreviation(line, Syntax::Html)
}

fn jsx(line: &str) -> Option<(usize, &str)> {
    extract_abbreviation(line, Syntax::Jsx)
}

fn css(line: &str) -> Option<(usize, &str)> {
    extract_abbreviation(line, Syntax::Css)
}

#[test]
fn markup_extraction() {
    assert_eq!(html("ul>li*3"), Some((0, "ul>li*3")));
    assert_eq!(html("    ul>li*3"), Some((4, "ul>li*3")));
    assert_eq!(html("\tdiv.a"), Some((1, "div.a")));
    assert_eq!(html("<p>hello</p>div.a"), Some((12, "div.a")));
    assert_eq!(html("text  .card"), Some((6, ".card")));
    assert_eq!(html("<div>ul>li"), Some((5, "ul>li")));
    assert_eq!(html("<a href=\"x\">span.icon"), Some((12, "span.icon")));
    assert_eq!(html("<br/>p"), Some((5, "p")));
    assert_eq!(html("<!-- note -->p"), Some((13, "p")));
    assert_eq!(html("a[title=\"a b\"]"), Some((0, "a[title=\"a b\"]")));
    assert_eq!(html("x a[title='a ] b']"), Some((2, "a[title='a ] b']")));
    assert_eq!(html("ul>li{a b}"), Some((0, "ul>li{a b}")));
    assert_eq!(html("p{a {nested} b}"), Some((0, "p{a {nested} b}")));
    assert_eq!(html("(ul>li)*2"), Some((0, "(ul>li)*2")));
    assert_eq!(html("x (ul>li"), Some((3, "ul>li")));
    assert_eq!(html("é ul>li"), Some((3, "ul>li")));
    assert_eq!(html("!"), Some((0, "!")));
    assert_eq!(html("h$*3"), Some((0, "h$*3")));
    assert_eq!(html("li.item$@-*3"), Some((0, "li.item$@-*3")));
    assert_eq!(html("ul>li^p"), Some((0, "ul>li^p")));
}

#[test]
fn markup_none() {
    assert_eq!(html(""), None);
    assert_eq!(html("ul>li "), None);
    assert_eq!(html("<div class=\"x"), None);
    assert_eq!(html("<div class=\"a b"), None);
    assert_eq!(html("<div class=x"), None);
    assert_eq!(html("<input disabled"), None);
    assert_eq!(html("<div>"), None);
    assert_eq!(html("hello world."), None);
    assert_eq!(html("ul>"), None);
    assert_eq!(html("ul>li["), None);
    assert_eq!(html("a]"), None);
    assert_eq!(html("foo)"), None);
}

#[test]
fn jsx_extraction() {
    assert_eq!(jsx("    ul>li"), Some((4, "ul>li")));
    assert_eq!(jsx("  return ul>li"), Some((9, "ul>li")));
    assert_eq!(jsx("  {items.map(i => li.x"), Some((18, "li.x")));
    assert_eq!(jsx("<>ul"), Some((2, "ul")));
    assert_eq!(jsx("  return (div.card"), Some((10, "div.card")));
    assert_eq!(jsx("{ok && p"), Some((7, "p")));
    assert_eq!(jsx("<div>p{Hello}"), Some((5, "p{Hello}")));
    assert_eq!(jsx("const a = b"), None);
    assert_eq!(jsx("const a=b"), None);
    assert_eq!(jsx("return"), None);
    assert_eq!(jsx("  const"), None);
    assert_eq!(jsx("foo(a, b"), None);
    assert_eq!(jsx("typeof x"), None);
    assert_eq!(jsx("x = y.z"), None);
    assert_eq!(jsx("<div onClick={go"), None);
}

#[test]
fn css_extraction() {
    assert_eq!(css("m10"), Some((0, "m10")));
    assert_eq!(css("  m10"), Some((2, "m10")));
    assert_eq!(css("a { m10"), Some((4, "m10")));
    assert_eq!(css("a { color: red; m10-20"), Some((16, "m10-20")));
    assert_eq!(css("  m10+p5!"), Some((2, "m10+p5!")));
    assert_eq!(css("  c#fc0"), Some((2, "c#fc0")));
    assert_eq!(css("  color:red"), Some((2, "color:red")));
    assert_eq!(css("color: re"), None);
    assert_eq!(css("div p"), None);
    assert_eq!(css(".card"), None);
    assert_eq!(css("  $m10"), None);
    assert_eq!(css("@media"), None);
    assert_eq!(css("&:hover"), None);
    assert_eq!(css("  m10 "), None);
    assert_eq!(css(""), None);
}

#[test]
fn plausible_html() {
    for yes in [
        "div",
        "p",
        "h1",
        "ul>li",
        "ul>li*3",
        ".card",
        "#app",
        "!",
        "html:5",
        "a:link",
        "bq",
        "lorem",
        "lorem10",
        "h$*3",
        "my-el.x",
        "MyComp.x",
        "[title]",
        "(div)*2",
        "input:email",
    ] {
        assert!(is_plausible(yes, Syntax::Html), "{yes}");
    }
    for no in [
        "", "{Hello}", "{count}", "hello", "e.g", "file.txt", "world.", "foo", "Foo", "ul>", "a b",
        "1div",
    ] {
        assert!(!is_plausible(no, Syntax::Html), "{no}");
    }
}

#[test]
fn plausible_jsx() {
    for yes in [
        "div",
        "ul>li",
        ".card",
        "#app",
        "(div>p)*2",
        "a.b",
        "btn:s",
        "lorem5",
        "section.hero>h1",
    ] {
        assert!(is_plausible(yes, Syntax::Jsx), "{yes}");
    }
    for no in [
        "Foo",
        "Foo>Bar",
        "Foo.Bar",
        "foo.bar",
        "x*2",
        "{x}",
        "[a]",
        "props.children",
        "useState",
        "return",
        "a b",
    ] {
        assert!(!is_plausible(no, Syntax::Jsx), "{no}");
    }
}

#[test]
fn plausible_css() {
    for yes in [
        "m10", "df", "d:f", "m10+p5", "pos", "c#fff", "color", "bd+", "jcsb",
    ] {
        assert!(is_plausible(yes, Syntax::Css), "{yes}");
    }
    for no in ["", "foo:bar", "foo", "a", "hello", "m10 20"] {
        assert!(!is_plausible(no, Syntax::Css), "{no}");
    }
}

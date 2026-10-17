//! HTML and JSX expansion, compared with Emmet's own output.

use zcode_emmet::{Error, Options, Syntax, expand, wrap};

fn html(abbreviation: &str) -> String {
    expand(abbreviation, Syntax::Html, &Options::default())
        .unwrap_or_else(|e| panic!("{abbreviation}: {e}"))
}

fn jsx(abbreviation: &str) -> String {
    expand(abbreviation, Syntax::Jsx, &Options::default())
        .unwrap_or_else(|e| panic!("{abbreviation}: {e}"))
}

fn wrap_html(abbreviation: &str, content: &str) -> String {
    wrap(abbreviation, content, Syntax::Html, &Options::default())
        .unwrap_or_else(|e| panic!("{abbreviation}: {e}"))
}

#[test]
fn elements_ids_classes() {
    assert_eq!(html("div"), "<div>$1</div>");
    assert_eq!(html("div#main"), "<div id=\"main\">$1</div>");
    assert_eq!(html("div.a"), "<div class=\"a\">$1</div>");
    assert_eq!(html("div.a.b"), "<div class=\"a b\">$1</div>");
    assert_eq!(html("span.a.b#c"), "<span class=\"a b\" id=\"c\">$1</span>");
    assert_eq!(html("div#a.b"), "<div id=\"a\" class=\"b\">$1</div>");
    assert_eq!(html("div#a#b"), "<div id=\"b\">$1</div>");
    assert_eq!(html("x-foo.bar"), "<x-foo class=\"bar\">$1</x-foo>");
    assert_eq!(
        html("div.sm:flex.w-1/2"),
        "<div class=\"sm:flex w-1/2\">$1</div>"
    );
    assert_eq!(html("MyComponent"), "<MyComponent>$1</MyComponent>");
}

#[test]
fn attributes() {
    assert_eq!(html("div[title]"), "<div title=\"$1\">$2</div>");
    assert_eq!(html("div[title=x]"), "<div title=\"x\">$1</div>");
    assert_eq!(html("div[title=]"), "<div title=\"$1\">$2</div>");
    assert_eq!(
        html("a[title=\"a b\" data-x='y' bool]"),
        "<a href=\"$1\" title=\"a b\" data-x=\"y\" bool=\"$2\">$3</a>"
    );
    assert_eq!(
        html("input[disabled]"),
        "<input type=\"${1:text}\" disabled>"
    );
    assert_eq!(html("div[foo.]"), "<div foo>$1</div>");
    assert_eq!(
        html("div[a=1 b=2][c=3]"),
        "<div a=\"1\" b=\"2\" c=\"3\">$1</div>"
    );
    assert_eq!(html("div.a[class=b]"), "<div class=\"a b\">$1</div>");
    assert_eq!(
        html("div[title='say \"hi\"']"),
        "<div title='say \"hi\"'>$1</div>"
    );
    assert_eq!(
        html("div[@click=go :value=x v-if=ok]"),
        "<div @click=\"go\" :value=\"x\" v-if=\"ok\">$1</div>"
    );
    assert_eq!(html("div[on:click={go}]"), "<div on:click={go\\}>$1</div>");
    assert_eq!(html("a[href=#top]"), "<a href=\"#top\">$1</a>");
}

#[test]
fn text() {
    assert_eq!(html("p{Hello}"), "<p>Hello</p>");
    assert_eq!(html("{Hello}"), "Hello");
    assert_eq!(html("a{click}+b"), "<a href=\"$1\">click</a><b>$2</b>");
    assert_eq!(html("p{a {b} c}"), "<p>a {b\\} c</p>");
    assert_eq!(html("p{a\\}b}"), "<p>a\\}b</p>");
    assert_eq!(html("p{$5}"), "<p>\\$5</p>");
    assert_eq!(html("p{C:\\\\dir}"), "<p>C:\\\\dir</p>");
    assert_eq!(
        html("p{Click }+a{here}"),
        "<p>Click </p>\n<a href=\"$1\">here</a>"
    );
    assert_eq!(
        html("p>{Click }+a{here}+{ now}"),
        "<p>Click <a href=\"$1\">here</a> now</p>"
    );
}

#[test]
fn operators() {
    assert_eq!(html("div>p"), "<div>\n    <p>$1</p>\n</div>");
    assert_eq!(html("div+p"), "<div>$1</div>\n<p>$2</p>");
    assert_eq!(
        html("div>p^h1"),
        "<div>\n    <p>$1</p>\n</div>\n<h1>$2</h1>"
    );
    assert_eq!(
        html("div>section>p^^h1"),
        "<div>\n    <section>\n        <p>$1</p>\n    </section>\n</div>\n<h1>$2</h1>"
    );
    assert_eq!(
        html("div>section>p^h2"),
        "<div>\n    <section>\n        <p>$1</p>\n    </section>\n    <h2>$2</h2>\n</div>"
    );
    assert_eq!(
        html("div>p^^^^h1"),
        "<div>\n    <p>$1</p>\n</div>\n<h1>$2</h1>"
    );
}

#[test]
fn groups() {
    assert_eq!(
        html("div>(header>h1)+main"),
        "<div>\n    <header>\n        <h1>$1</h1>\n    </header>\n    <main>$2</main>\n</div>"
    );
    assert_eq!(
        html("(div>p)*2"),
        "<div>\n    <p>$1</p>\n</div>\n<div>\n    <p>$2</p>\n</div>"
    );
    assert_eq!(
        html("(a>b)*2"),
        "<a href=\"$1\"><b>$2</b></a><a href=\"$3\"><b>$4</b></a>"
    );
    assert_eq!(
        html("((h1+h2)*2)+p"),
        "<h1>$1</h1>\n<h2>$2</h2>\n<h1>$3</h1>\n<h2>$4</h2>\n<p>$5</p>"
    );
    assert_eq!(
        html("div>(p>span)^h1"),
        "<div>\n    <p><span>$1</span></p>\n</div>\n<h1>$2</h1>"
    );
}

#[test]
fn multiplication_and_numbering() {
    assert_eq!(
        html("ul>li*2"),
        "<ul>\n    <li>$1</li>\n    <li>$2</li>\n</ul>"
    );
    assert_eq!(
        html("li.item$*3"),
        "<li class=\"item1\">$1</li>\n<li class=\"item2\">$2</li>\n<li class=\"item3\">$3</li>"
    );
    assert_eq!(
        html("li.item$$$*2"),
        "<li class=\"item001\">$1</li>\n<li class=\"item002\">$2</li>"
    );
    assert_eq!(html("li{$@-}*3"), "<li>3</li>\n<li>2</li>\n<li>1</li>");
    assert_eq!(html("li{$@3}*3"), "<li>3</li>\n<li>4</li>\n<li>5</li>");
    assert_eq!(html("li{$@-3}*3"), "<li>5</li>\n<li>4</li>\n<li>3</li>");
    assert_eq!(
        html("li#id$[data-n=$]*2"),
        "<li id=\"id1\" data-n=\"1\">$1</li>\n<li id=\"id2\" data-n=\"2\">$2</li>"
    );
    assert_eq!(html("h$*3"), "<h1>$1</h1>\n<h2>$2</h2>\n<h3>$3</h3>");
    assert_eq!(html("p*2>{$}"), "<p>1</p>\n<p>2</p>");
    assert_eq!(
        html("ul*2>li{$}*2"),
        "<ul>\n    <li>1</li>\n    <li>2</li>\n</ul>\n<ul>\n    <li>1</li>\n    <li>2</li>\n</ul>"
    );
    assert_eq!(html("(div{$})*2"), "<div>1</div>\n<div>2</div>");
    assert_eq!(html("li{\\$}*2"), "<li>\\$</li>\n<li>\\$</li>");
    assert_eq!(html("li*"), "<li>$1</li>");
}

#[test]
fn implicit_tags() {
    assert_eq!(html(".a"), "<div class=\"a\">$1</div>");
    assert_eq!(html("#a"), "<div id=\"a\">$1</div>");
    assert_eq!(html("[x]"), "<div x=\"$1\">$2</div>");
    assert_eq!(html("ul>.x"), "<ul>\n    <li class=\"x\">$1</li>\n</ul>");
    assert_eq!(html("ol>.x"), "<ol>\n    <li class=\"x\">$1</li>\n</ol>");
    assert_eq!(
        html("table>.x"),
        "<table>\n    <tr class=\"x\">$1</tr>\n</table>"
    );
    assert_eq!(
        html("tbody>.x"),
        "<tbody>\n    <tr class=\"x\">$1</tr>\n</tbody>"
    );
    assert_eq!(html("tr>.x"), "<tr>\n    <td class=\"x\">$1</td>\n</tr>");
    assert_eq!(
        html("table>.r>.c"),
        "<table>\n    <tr class=\"r\">\n        <td class=\"c\">$1</td>\n    </tr>\n</table>"
    );
    assert_eq!(
        html("select>.x"),
        "<select name=\"$1\" id=\"$1\">\n    <option class=\"x\">$2</option>\n</select>"
    );
    assert_eq!(html("p>.x"), "<p><span class=\"x\">$1</span></p>");
    assert_eq!(html("em>.x"), "<em><span class=\"x\">$1</span></em>");
    assert_eq!(
        html("video>[src]"),
        "<video src=\"$1\">\n    <source src=\"$2\">\n</video>"
    );
    assert_eq!(
        html("colgroup>.x"),
        "<colgroup>\n    <col class=\"x\">\n</colgroup>"
    );
    assert_eq!(
        html("map>.x"),
        "<map name=\"$1\">\n    <area class=\"x\">\n</map>"
    );
    assert_eq!(
        html("div>.x"),
        "<div>\n    <div class=\"x\">$1</div>\n</div>"
    );
}

#[test]
fn self_closing() {
    assert_eq!(html("img"), "<img src=\"$1\" alt=\"$2\">");
    assert_eq!(html("br"), "<br>");
    assert_eq!(html("hr.x"), "<hr class=\"x\">");
    assert_eq!(html("wbr+track"), "<wbr>\n<track>");
    assert_eq!(html("div/"), "<div />");
    assert_eq!(html("my-icon[name=x]/"), "<my-icon name=\"x\" />");
    assert_eq!(jsx("img"), "<img src=\"$1\" alt=\"$2\" />");
    assert_eq!(jsx("br"), "<br />");
    assert_eq!(jsx("Icon/"), "<Icon />");
}

#[test]
fn formatting() {
    assert_eq!(html("p>a"), "<p><a href=\"$1\">$2</a></p>");
    assert_eq!(html("p>span*2"), "<p><span>$1</span><span>$2</span></p>");
    assert_eq!(
        html("p>span*3"),
        "<p>\n    <span>$1</span>\n    <span>$2</span>\n    <span>$3</span>\n</p>"
    );
    assert_eq!(
        html("ul>li*3>a"),
        "<ul>\n    <li><a href=\"$1\">$2</a></li>\n    <li><a href=\"$3\">$4</a></li>\n    <li><a href=\"$5\">$6</a></li>\n</ul>"
    );
    assert_eq!(
        html("div>p+p"),
        "<div>\n    <p>$1</p>\n    <p>$2</p>\n</div>"
    );
    assert_eq!(
        html("nav>ul>li*3>a"),
        "<nav>\n    <ul>\n        <li><a href=\"$1\">$2</a></li>\n        <li><a href=\"$3\">$4</a></li>\n        <li><a href=\"$5\">$6</a></li>\n    </ul>\n</nav>"
    );
    assert_eq!(
        html("table>tr*2>td*2"),
        "<table>\n    <tr>\n        <td>$1</td>\n        <td>$2</td>\n    </tr>\n    <tr>\n        <td>$3</td>\n        <td>$4</td>\n    </tr>\n</table>"
    );
    assert_eq!(
        html("form>input:email+btn:s"),
        "<form action=\"$1\"><input type=\"email\" name=\"$2\" id=\"$2\"><button type=\"submit\">$3</button></form>"
    );
    assert_eq!(
        html(".card>.card-body>h5.card-title{Title}+p.card-text"),
        "<div class=\"card\">\n    <div class=\"card-body\">\n        <h5 class=\"card-title\">Title</h5>\n        <p class=\"card-text\">$1</p>\n    </div>\n</div>"
    );
    // An inline element next to a block one goes on its own line.
    assert_eq!(
        html("div>span+p"),
        "<div>\n    <span>$1</span>\n    <p>$2</p>\n</div>"
    );
    assert_eq!(
        html("div>p+span"),
        "<div>\n    <p>$1</p>\n    <span>$2</span>\n</div>"
    );
    // An inline element with a block child is formatted.
    assert_eq!(
        html("div>a>div"),
        "<div>\n    <a href=\"$1\">\n        <div>$2</div>\n    </a>\n</div>"
    );
    assert_eq!(html("a+b"), "<a href=\"$1\">$2</a><b>$3</b>");
    assert_eq!(
        html("span*3"),
        "<span>$1</span>\n<span>$2</span>\n<span>$3</span>"
    );
    assert_eq!(
        html("label+input+label+input"),
        "<label for=\"$1\">$2</label>\n<input type=\"${3:text}\">\n<label for=\"$4\">$5</label>\n<input type=\"${6:text}\">"
    );
}

#[test]
fn options() {
    let tabs = Options {
        indent: "\t".to_owned(),
        newline: "\r\n".to_owned(),
    };
    assert_eq!(
        expand("ul>li*2", Syntax::Html, &tabs).unwrap(),
        "<ul>\r\n\t<li>$1</li>\r\n\t<li>$2</li>\r\n</ul>"
    );
    let two = Options {
        indent: "  ".to_owned(),
        ..Options::default()
    };
    assert_eq!(
        expand("div>p", Syntax::Html, &two).unwrap(),
        "<div>\n  <p>$1</p>\n</div>"
    );
}

#[test]
fn html5_boilerplate() {
    let expected = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n    <meta charset=\"UTF-8\">\n    <meta name=\"viewport\" content=\"width=${1:device-width}, initial-scale=${2:1.0}\">\n    <title>${3:Document}</title>\n</head>\n<body>\n    $4\n</body>\n</html>";
    assert_eq!(html("!"), expected);
    assert_eq!(html("html:5"), expected);
    assert!(html("!>div#app").ends_with("<body>\n    <div id=\"app\">$4</div>\n</body>\n</html>"));
    assert_eq!(html("!!!"), "<!DOCTYPE html>");
}

#[test]
fn snippets() {
    assert_eq!(html("a"), "<a href=\"$1\">$2</a>");
    assert_eq!(html("a.btn"), "<a href=\"$1\" class=\"btn\">$2</a>");
    assert_eq!(html("a[href=/x]"), "<a href=\"/x\">$1</a>");
    assert_eq!(html("a:link"), "<a href=\"http://$1\">$2</a>");
    assert_eq!(html("a:mail"), "<a href=\"mailto:$1\">$2</a>");
    assert_eq!(html("a:tel"), "<a href=\"tel:+$1\">$2</a>");
    assert_eq!(
        html("a:blank"),
        "<a href=\"http://$1\" target=\"_blank\" rel=\"noopener noreferrer\">$2</a>"
    );
    assert_eq!(html("link"), "<link rel=\"stylesheet\" href=\"$1\">");
    assert_eq!(
        html("link:css"),
        "<link rel=\"stylesheet\" href=\"${1:style}.css\">"
    );
    assert_eq!(
        html("link:favicon"),
        "<link rel=\"shortcut icon\" href=\"${1:favicon.ico}\" type=\"image/x-icon\">"
    );
    assert_eq!(html("script"), "<script>$1</script>");
    assert_eq!(html("script:src"), "<script src=\"$1\">$2</script>");
    assert_eq!(
        html("script:module"),
        "<script type=\"module\" src=\"$1\">$2</script>"
    );
    assert_eq!(
        html("meta:vp"),
        "<meta name=\"viewport\" content=\"width=${1:device-width}, initial-scale=${2:1.0}\">"
    );
    assert_eq!(
        html("meta:utf"),
        "<meta http-equiv=\"Content-Type\" content=\"text/html;charset=UTF-8\">"
    );
    assert_eq!(
        html("meta:edge"),
        "<meta http-equiv=\"X-UA-Compatible\" content=\"${1:ie=edge}\">"
    );
    assert_eq!(html("img"), "<img src=\"$1\" alt=\"$2\">");
    assert_eq!(html("input"), "<input type=\"${1:text}\">");
    assert_eq!(
        html("inp"),
        "<input type=\"${1:text}\" name=\"$2\" id=\"$2\">"
    );
    assert_eq!(
        html("input:text"),
        "<input type=\"text\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:email"),
        "<input type=\"email\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:password"),
        "<input type=\"password\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:checkbox"),
        "<input type=\"checkbox\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:c"),
        "<input type=\"checkbox\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:radio"),
        "<input type=\"radio\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(html("input:hidden"), "<input type=\"hidden\" name=\"$1\">");
    assert_eq!(html("input:submit"), "<input type=\"submit\" value=\"$1\">");
    assert_eq!(html("input:button"), "<input type=\"button\" value=\"$1\">");
    assert_eq!(html("input:reset"), "<input type=\"reset\" value=\"$1\">");
    assert_eq!(
        html("input:file"),
        "<input type=\"file\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:number"),
        "<input type=\"number\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:date"),
        "<input type=\"date\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:color"),
        "<input type=\"color\" name=\"$1\" id=\"$1\">"
    );
    assert_eq!(
        html("input:search+input:url+input:tel+input:range")
            .matches("<input")
            .count(),
        4
    );
    assert_eq!(html("btn"), "<button>$1</button>");
    assert_eq!(html("btn:s"), "<button type=\"submit\">$1</button>");
    assert_eq!(html("btn:r"), "<button type=\"reset\">$1</button>");
    assert_eq!(html("btn:d"), "<button disabled>$1</button>");
    assert_eq!(html("form"), "<form action=\"$1\">$2</form>");
    assert_eq!(
        html("form:get"),
        "<form action=\"$1\" method=\"get\">$2</form>"
    );
    assert_eq!(
        html("form:post"),
        "<form action=\"$1\" method=\"post\">$2</form>"
    );
    assert_eq!(html("label"), "<label for=\"$1\">$2</label>");
    assert_eq!(html("select"), "<select name=\"$1\" id=\"$1\">$2</select>");
    assert_eq!(
        html("select>opt*2"),
        "<select name=\"$1\" id=\"$1\">\n    <option value=\"$2\">$3</option>\n    <option value=\"$4\">$5</option>\n</select>"
    );
    assert_eq!(
        html("textarea"),
        "<textarea name=\"$1\" id=\"$1\" cols=\"${2:30}\" rows=\"${3:10}\">$4</textarea>"
    );
    assert_eq!(html("tarea"), html("textarea"));
    assert_eq!(html("bq"), "<blockquote>$1</blockquote>");
    assert_eq!(html("hdr+ftr"), "<header>$1</header>\n<footer>$2</footer>");
    assert_eq!(
        html("sect>art"),
        "<section>\n    <article>$1</article>\n</section>"
    );
    assert_eq!(
        html("fig>figc"),
        "<figure>\n    <figcaption>$1</figcaption>\n</figure>"
    );
    assert_eq!(html("str"), "<strong>$1</strong>");
    assert_eq!(html("emb"), "<embed src=\"$1\" type=\"$2\">");
    assert_eq!(html("obj"), "<object data=\"$1\" type=\"$2\">$3</object>");
    assert_eq!(
        html("ifr"),
        "<iframe src=\"$1\" frameborder=\"0\">$2</iframe>"
    );
    assert_eq!(html("optg"), "<optgroup>$1</optgroup>");
    assert_eq!(html("leg"), "<legend>$1</legend>");
    assert_eq!(html("fst"), "<fieldset>$1</fieldset>");
    assert_eq!(html("colg"), "<colgroup>$1</colgroup>");
    assert_eq!(html("cap"), "<caption>$1</caption>");
    assert_eq!(html("menu:c"), "<menu type=\"context\">$1</menu>");
    assert_eq!(html("abbr"), "<abbr title=\"$1\">$2</abbr>");
    assert_eq!(html("time"), "<time datetime=\"$1\">$2</time>");
    assert_eq!(html("c"), "<!-- $1 -->");
    assert_eq!(
        html("ri:a"),
        "<picture>\n    <source media=\"(${1:min-width: })\" srcset=\"$2\">\n    <img src=\"$3\" alt=\"$4\">\n</picture>"
    );
}

#[test]
fn lorem() {
    assert_eq!(html("p>lorem4"), "<p>Lorem ipsum dolor sit.</p>");
    assert_eq!(html("lorem4"), "Lorem ipsum dolor sit.");
    assert_eq!(html("lipsum3"), "Lorem ipsum dolor.");
    let long = html("lorem");
    assert!(long.starts_with("Lorem ipsum dolor sit amet consectetur adipisicing elit. "));
    assert_eq!(long.split_whitespace().count(), 30);
    assert_eq!(html("lorem"), long, "deterministic");
    let p = html("p>lorem10");
    assert!(
        p.starts_with("<p>Lorem ipsum dolor sit amet consectetur adipisicing elit. ")
            && p.ends_with("</p>")
    );
    assert_eq!(p.split_whitespace().count(), 10);
    let list = html("ul>li*3>lorem4");
    let items: Vec<&str> = list.lines().filter(|line| line.contains("<li>")).collect();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0], "    <li>Lorem ipsum dolor sit.</li>");
    assert!(!items[1].contains("Lorem") && !items[2].contains("Lorem"));
    assert_ne!(items[1], items[2]);
    let implicit = html("ul>lorem3*2");
    assert!(implicit.starts_with("<ul>\n    <li>Lorem ipsum dolor.</li>\n    <li>"));
    assert!(expand("lorem100000", Syntax::Html, &Options::default()).is_err());
}

#[test]
fn tabstops_and_escaping() {
    assert_eq!(html("a{$}+b{\\}}"), "<a href=\"$1\">\\$</a><b>\\}</b>");
    // A tabstop followed by a digit is braced so `$1` and `0` don't read as `$10`.
    assert_eq!(html("div[x=${1}0]"), "<div x=\"${1}0\">$2</div>");
    assert_eq!(
        html("div{${1:name} and ${1}}"),
        "<div>${1:name} and $1</div>"
    );
    assert_eq!(html("div{${2:b} ${1:a}}"), "<div>${1:b} ${2:a}</div>");
    assert_eq!(html("p{a\\\\b}"), "<p>a\\\\b</p>");
}

#[test]
fn jsx_specifics() {
    assert_eq!(jsx(".a"), "<div className=\"a\">$1</div>");
    assert_eq!(jsx("div.a.b"), "<div className=\"a b\">$1</div>");
    assert_eq!(jsx("label"), "<label htmlFor=\"$1\">$2</label>");
    assert_eq!(jsx("label[for=x]"), "<label htmlFor=\"x\">$1</label>");
    assert_eq!(
        jsx("input:c"),
        "<input type=\"checkbox\" name=\"$1\" id=\"$1\" />"
    );
    assert_eq!(jsx("p{Hello {name}}"), "<p>Hello {name\\}</p>");
    assert_eq!(
        jsx("button[onClick={handle}]{Go}"),
        "<button onClick={handle\\}>Go</button>"
    );
    assert_eq!(jsx("div[onClick={}]"), "<div onClick={$1\\}>$2</div>");
    assert_eq!(jsx("c"), "{/* $1 */\\}");
    assert_eq!(
        jsx("Foo.Bar>Baz"),
        "<Foo.Bar>\n    <Baz>$1</Baz>\n</Foo.Bar>"
    );
    assert_eq!(jsx("Link"), "<Link>$1</Link>");
    assert_eq!(
        jsx("ul>li.item$*2"),
        "<ul>\n    <li className=\"item1\">$1</li>\n    <li className=\"item2\">$2</li>\n</ul>"
    );
    assert_eq!(
        jsx("img.logo"),
        "<img src=\"$1\" alt=\"$2\" className=\"logo\" />"
    );
}

#[test]
fn errors_and_limits() {
    let options = Options::default();
    for bad in [
        "", ">a", "a>", "a+", "a>>b", "(a", "a)", "a[b", "a{b", "a b", "1div", "h$", "a*0", ".",
        "#",
    ] {
        assert!(expand(bad, Syntax::Html, &options).is_err(), "{bad:?}");
    }
    assert_eq!(
        expand("a*1001", Syntax::Html, &options),
        Err(Error::TooLarge)
    );
    assert_eq!(
        expand("a*1000>b*1000", Syntax::Html, &options),
        Err(Error::TooLarge)
    );
    assert_eq!(
        expand(
            "a>".repeat(500).trim_end_matches('>'),
            Syntax::Html,
            &options
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(
        expand(&"(".repeat(500), Syntax::Html, &options),
        Err(Error::TooLarge)
    );
    assert!(expand("div*1000", Syntax::Html, &options).is_ok());
    let error = expand("a b", Syntax::Html, &options).unwrap_err();
    assert_eq!(
        error,
        Error::Syntax {
            position: 1,
            message: "unexpected character"
        }
    );
    assert_eq!(
        error.to_string(),
        "invalid abbreviation at 1: unexpected character"
    );
}

#[test]
fn wrap_with_abbreviation() {
    assert_eq!(wrap_html("p", "hello"), "<p>hello</p>");
    assert_eq!(
        wrap_html("div.x", "line1\nline2"),
        "<div class=\"x\">\n    line1\n    line2\n</div>"
    );
    assert_eq!(
        wrap_html("div>p", "  hi  \n"),
        "<div>\n    <p>hi</p>\n</div>"
    );
    assert_eq!(
        wrap_html("ul>li*", "one\n  two\n\nthree"),
        "<ul>\n    <li>one</li>\n    <li>two</li>\n    <li>three</li>\n</ul>"
    );
    assert_eq!(
        wrap_html("ul>li.i$*>a", "one\ntwo"),
        "<ul>\n    <li class=\"i1\"><a href=\"$1\">one</a></li>\n    <li class=\"i2\"><a href=\"$2\">two</a></li>\n</ul>"
    );
    assert_eq!(
        wrap_html("a[href=$#]", "http://x"),
        "<a href=\"http://x\">$1</a>"
    );
    assert_eq!(
        wrap_html("ul>li[title=$#]*", "a\nb"),
        "<ul>\n    <li title=\"a\">$1</li>\n    <li title=\"b\">$2</li>\n</ul>"
    );
    assert_eq!(
        wrap_html("div>h1+p", "text"),
        "<div>\n    <h1>$1</h1>\n    <p>text</p>\n</div>"
    );
    assert_eq!(wrap_html("a", "home"), "<a href=\"$1\">home</a>");
    assert_eq!(wrap_html("p", "price: $5 {x}"), "<p>price: \\$5 {x\\}</p>");
    assert_eq!(
        wrap_html("div", "<p>\n    text\n</p>"),
        "<div>\n    <p>\n        text\n    </p>\n</div>"
    );
    assert_eq!(
        wrap_html("blockquote", "<b>x</b>"),
        "<blockquote><b>x</b></blockquote>"
    );
    assert_eq!(
        wrap_html("div", "<section>x</section>"),
        "<div>\n    <section>x</section>\n</div>"
    );
    assert_eq!(wrap_html("p", ""), "<p>$1</p>");
    assert_eq!(wrap_html("img", "x"), "<img src=\"$1\" alt=\"$2\">x");
    assert_eq!(
        wrap(".a", "x", Syntax::Jsx, &Options::default()).unwrap(),
        "<div className=\"a\">x</div>"
    );
    assert!(wrap("m10", "x", Syntax::Css, &Options::default()).is_err());
    assert!(wrap("a>", "x", Syntax::Html, &Options::default()).is_err());
}

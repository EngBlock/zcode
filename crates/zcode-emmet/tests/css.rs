//! CSS expansion.

use zcode_emmet::{Error, Options, Syntax, expand};

fn css(abbreviation: &str) -> String {
    expand(abbreviation, Syntax::Css, &Options::default())
        .unwrap_or_else(|e| panic!("{abbreviation}: {e}"))
}

#[test]
fn numbers_and_units() {
    assert_eq!(css("m10"), "margin: 10px;");
    assert_eq!(css("m10-20"), "margin: 10px 20px;");
    assert_eq!(css("m10-20-30-40"), "margin: 10px 20px 30px 40px;");
    assert_eq!(css("m-10"), "margin: -10px;");
    assert_eq!(css("m10--20"), "margin: 10px -20px;");
    assert_eq!(css("m0-a"), "margin: 0 auto;");
    assert_eq!(css("p0"), "padding: 0;");
    assert_eq!(css("w100p"), "width: 100%;");
    assert_eq!(css("w50%"), "width: 50%;");
    assert_eq!(css("m2e"), "margin: 2em;");
    assert_eq!(css("m2x"), "margin: 2ex;");
    assert_eq!(css("fz1.5r"), "font-size: 1.5rem;");
    assert_eq!(css("h100vh"), "height: 100vh;");
    assert_eq!(css("w10ch"), "width: 10ch;");
    assert_eq!(css("gtc1fr"), "grid-template-columns: 1fr;");
    assert_eq!(css("m10px"), "margin: 10px;");
    assert_eq!(css("m1.5"), "margin: 1.5em;");
    assert_eq!(css("mt.5"), "margin-top: .5em;");
    assert_eq!(css("fz16"), "font-size: 16px;");
    assert_eq!(css("t0"), "top: 0;");
}

#[test]
fn unitless_properties() {
    assert_eq!(css("lh1.5"), "line-height: 1.5;");
    assert_eq!(css("lh2"), "line-height: 2;");
    assert_eq!(css("op.5"), "opacity: .5;");
    assert_eq!(css("z10"), "z-index: 10;");
    assert_eq!(css("fw700"), "font-weight: 700;");
    assert_eq!(css("fxg1"), "flex-grow: 1;");
    assert_eq!(css("ord2"), "order: 2;");
    assert_eq!(css("lh24px"), "line-height: 24px;");
}

#[test]
fn important_and_colors() {
    assert_eq!(css("m10!"), "margin: 10px !important;");
    assert_eq!(css("dn!"), "display: none !important;");
    assert_eq!(css("m!"), "margin: $1 !important;");
    assert_eq!(css("c#f"), "color: #fff;");
    assert_eq!(css("c#fc"), "color: #fcfcfc;");
    assert_eq!(css("c#fc0"), "color: #fc0;");
    assert_eq!(css("c#FFCC00"), "color: #fc0;");
    assert_eq!(css("c#123456"), "color: #123456;");
    assert_eq!(css("bgc#000"), "background-color: #000;");
    assert_eq!(css("bd1-s#f00"), "border: 1px solid #f00;");
    assert_eq!(css("bdn"), "border: none;");
    assert_eq!(css("bdt2-d"), "border-top: 2px dashed;");
    assert_eq!(css("c:r"), "color: rgb(${1:0}, ${2:0}, ${3:0});");
    assert_eq!(css("c:ra"), "color: rgba(${1:0}, ${2:0}, ${3:0}, ${4:.5});");
}

#[test]
fn keywords() {
    assert_eq!(css("d:f"), "display: flex;");
    assert_eq!(css("df"), "display: flex;");
    assert_eq!(css("dg"), "display: grid;");
    assert_eq!(css("dib"), "display: inline-block;");
    assert_eq!(css("dif"), "display: inline-flex;");
    assert_eq!(css("di"), "display: inline;");
    assert_eq!(css("dn"), "display: none;");
    assert_eq!(css("db"), "display: block;");
    assert_eq!(css("d:ib"), "display: inline-block;");
    assert_eq!(css("pos:a"), "position: absolute;");
    assert_eq!(css("posa"), "position: absolute;");
    assert_eq!(css("posr"), "position: relative;");
    assert_eq!(css("posf"), "position: fixed;");
    assert_eq!(css("poss"), "position: static;");
    assert_eq!(css("ta:c"), "text-align: center;");
    assert_eq!(css("tac"), "text-align: center;");
    assert_eq!(css("tar"), "text-align: right;");
    assert_eq!(css("fw:b"), "font-weight: bold;");
    assert_eq!(css("fwb"), "font-weight: bold;");
    assert_eq!(css("fsi"), "font-style: italic;");
    assert_eq!(css("jc:c"), "justify-content: center;");
    assert_eq!(css("jcc"), "justify-content: center;");
    assert_eq!(css("jcsb"), "justify-content: space-between;");
    assert_eq!(css("ai:c"), "align-items: center;");
    assert_eq!(css("aic"), "align-items: center;");
    assert_eq!(css("aifs"), "align-items: flex-start;");
    assert_eq!(css("fxd:c"), "flex-direction: column;");
    assert_eq!(css("fxdc"), "flex-direction: column;");
    assert_eq!(css("fxw:w"), "flex-wrap: wrap;");
    assert_eq!(css("ov:h"), "overflow: hidden;");
    assert_eq!(css("ovh"), "overflow: hidden;");
    assert_eq!(css("cur:p"), "cursor: pointer;");
    assert_eq!(css("bxz:bb"), "box-sizing: border-box;");
    assert_eq!(css("bxzbb"), "box-sizing: border-box;");
    assert_eq!(css("td:n"), "text-decoration: none;");
    assert_eq!(css("tdn"), "text-decoration: none;");
    assert_eq!(css("tdu"), "text-decoration: underline;");
    assert_eq!(css("tt:u"), "text-transform: uppercase;");
    assert_eq!(css("ttu"), "text-transform: uppercase;");
    assert_eq!(css("ws:nw"), "white-space: nowrap;");
    assert_eq!(css("wsnw"), "white-space: nowrap;");
    assert_eq!(css("fll"), "float: left;");
    assert_eq!(css("flr"), "float: right;");
    assert_eq!(css("clb"), "clear: both;");
    assert_eq!(css("vam"), "vertical-align: middle;");
    assert_eq!(css("m:a"), "margin: auto;");
    assert_eq!(css("bd:n"), "border: none;");
    assert_eq!(css("bg:n"), "background: none;");
    assert_eq!(css("usn"), "user-select: none;");
    assert_eq!(css("ap:n"), "appearance: none;");
    assert_eq!(css("v:h"), "visibility: hidden;");
    assert_eq!(css("vis:h"), "visibility: hidden;");
}

#[test]
fn defaults_and_tabstops() {
    assert_eq!(css("m"), "margin: $1;");
    assert_eq!(css("pos"), "position: ${1:relative};");
    assert_eq!(css("d"), "display: ${1:block};");
    assert_eq!(css("bd"), "border: ${1:1px} ${2:solid} ${3:#000};");
    assert_eq!(css("bd+"), "border: ${1:1px} ${2:solid} ${3:#000};");
    assert_eq!(css("bdt"), "border-top: ${1:1px} ${2:solid} ${3:#000};");
    assert_eq!(css("bdrs"), "border-radius: $1;");
    assert_eq!(css("bdrs4"), "border-radius: 4px;");
    assert_eq!(css("bdrs50p"), "border-radius: 50%;");
    assert_eq!(
        css("bxsh"),
        "box-shadow: ${1:inset }${2:hoff} ${3:voff} ${4:blur} ${5:#000};"
    );
    assert_eq!(css("trs"), "transition: ${1:prop} ${2:time};");
    assert_eq!(css("trsde"), "transition-delay: ${1:time};");
    assert_eq!(css("trf"), "transform: $1;");
    assert_eq!(
        css("anim"),
        "animation: ${1:name} ${2:duration} ${3:timing-function} ${4:delay} ${5:iteration-count} ${6:direction} ${7:fill-mode};"
    );
    assert_eq!(
        css("gtc"),
        "grid-template-columns: repeat(${1:2}, ${2:1fr});"
    );
    assert_eq!(css("gap"), "gap: $1;");
    assert_eq!(css("gap16"), "gap: 16px;");
    assert_eq!(css("ff"), "font-family: ${1:serif};");
    assert_eq!(css("fz"), "font-size: $1;");
    assert_eq!(css("fs"), "font-style: ${1:italic};");
    assert_eq!(css("lh"), "line-height: $1;");
    assert_eq!(css("ls"), "letter-spacing: ${1:normal};");
    assert_eq!(css("ls1"), "letter-spacing: 1px;");
    assert_eq!(css("op"), "opacity: $1;");
    assert_eq!(css("z"), "z-index: $1;");
    assert_eq!(css("t"), "top: $1;");
    assert_eq!(css("r"), "right: $1;");
    assert_eq!(css("b"), "bottom: $1;");
    assert_eq!(css("l"), "left: $1;");
    assert_eq!(css("w"), "width: $1;");
    assert_eq!(css("h"), "height: $1;");
    assert_eq!(css("maw"), "max-width: $1;");
    assert_eq!(css("mah"), "max-height: $1;");
    assert_eq!(css("miw"), "min-width: $1;");
    assert_eq!(css("mih"), "min-height: $1;");
    assert_eq!(css("bg"), "background: ${1:#000};");
    assert_eq!(css("bgi"), "background-image: url($1);");
    assert_eq!(css("bgr"), "background-repeat: ${1:no-repeat};");
    assert_eq!(css("bgp"), "background-position: ${1:0} ${2:0};");
    assert_eq!(css("bgs"), "background-size: ${1:cover};");
    assert_eq!(css("bgc"), "background-color: #${1:fff};");
    assert_eq!(css("c"), "color: ${1:#000};");
    assert_eq!(css("cnt"), "content: '$1';");
    assert_eq!(css("ct"), "content: '$1';");
    assert_eq!(css("ol"), "outline: $1;");
    assert_eq!(
        css("tsh"),
        "text-shadow: ${1:hoff} ${2:voff} ${3:blur} ${4:#000};"
    );
    assert_eq!(css("va"), "vertical-align: ${1:top};");
    assert_eq!(css("bdc"), "border-color: ${1:#000};");
    assert_eq!(css("bds"), "border-style: ${1:solid};");
    assert_eq!(css("bdw"), "border-width: $1;");
    assert_eq!(css("fl"), "float: ${1:left};");
    assert_eq!(css("cl"), "clear: ${1:both};");
    assert_eq!(css("vis"), "visibility: ${1:hidden};");
    assert_eq!(css("ap"), "appearance: ${1:none};");
    assert_eq!(css("us"), "user-select: ${1:none};");
    assert_eq!(css("cur"), "cursor: ${1:pointer};");
}

#[test]
fn several_and_full_names() {
    assert_eq!(css("m10+p5"), "margin: 10px;\npadding: 5px;");
    assert_eq!(css("m+p"), "margin: $1;\npadding: $2;");
    assert_eq!(
        css("pos+t0+l0"),
        "position: ${1:relative};\ntop: 0;\nleft: 0;"
    );
    assert_eq!(
        css("bd++m0"),
        "border: ${1:1px} ${2:solid} ${3:#000};\nmargin: 0;"
    );
    assert_eq!(
        css("df+aic+jcc"),
        "display: flex;\nalign-items: center;\njustify-content: center;"
    );
    let crlf = Options {
        newline: "\r\n".to_owned(),
        ..Options::default()
    };
    assert_eq!(
        expand("m+p", Syntax::Css, &crlf).unwrap(),
        "margin: $1;\r\npadding: $2;"
    );
    assert_eq!(css("color:red"), "color: red;");
    assert_eq!(css("display:inline-block"), "display: inline-block;");
    assert_eq!(css("foo:bar"), "foo: bar;");
    assert_eq!(css("--brand:#f00"), "--brand: #f00;");
    assert_eq!(css("line-height:1.5"), "line-height: 1.5;");
}

#[test]
fn errors() {
    let options = Options::default();
    for bad in [
        "", "foo", "zz10", "m10 20", "m10-", "m#", "m10+", "+m", "foo:", ":x", "m10x!y", "m@",
    ] {
        assert!(expand(bad, Syntax::Css, &options).is_err(), "{bad:?}");
    }
    assert_eq!(
        expand("foo", Syntax::Css, &options),
        Err(Error::UnknownAbbreviation("foo".to_owned()))
    );
}

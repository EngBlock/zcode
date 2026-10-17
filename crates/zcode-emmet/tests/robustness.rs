//! No input panics, limits hold, and typical expansions are fast.

use std::time::{Duration, Instant};

use zcode_emmet::{Options, Syntax, expand, extract_abbreviation, is_plausible, wrap};

const SYNTAXES: [Syntax; 3] = [Syntax::Html, Syntax::Jsx, Syntax::Css];

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Pieces abbreviations are made of, and some that they aren't.
const ALPHABET: &[&str] = &[
    "a",
    "div",
    "ul",
    "li",
    "p",
    "span",
    "img",
    "input",
    "!",
    "lorem",
    "lorem3",
    "c",
    "td",
    "tr",
    "table",
    "Foo",
    "m",
    "p",
    "d",
    "pos",
    "bd",
    "10",
    "0",
    "-",
    "--",
    ".5",
    "#",
    "#f",
    "%",
    "e",
    "r",
    "px",
    ".",
    "#",
    ">",
    "+",
    "^",
    "^^",
    "*",
    "*3",
    "*1000",
    "*99999999999999999999",
    "/",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "=",
    "\"",
    "'",
    "$",
    "$$",
    "$@-",
    "$@3",
    "${",
    "${1",
    "${1:x}",
    "${lang}",
    "$#",
    "\\",
    ":",
    "!",
    "@",
    " ",
    "\t",
    "\n",
    "é",
    "😀",
    "<",
    "</",
    "<div>",
    ",",
    ";",
    "x=",
    "[a=b c]",
    "{t}",
    "\u{0}",
    "\u{200b}",
];

fn generate(rng: &mut Rng) -> String {
    let len = rng.below(24);
    (0..len)
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

#[test]
fn random_input_never_panics() {
    let options = Options::default();
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    for _ in 0..10_000 {
        let input = generate(&mut rng);
        for syntax in SYNTAXES {
            let _ = expand(&input, syntax, &options);
            let _ = wrap(&input, "one\n  two\n\nthree", syntax, &options);
            let _ = is_plausible(&input, syntax);
            if let Some((start, abbreviation)) = extract_abbreviation(&input, syntax) {
                assert_eq!(&input[start..], abbreviation);
                let _ = expand(abbreviation, syntax, &options);
            }
        }
    }
}

#[test]
fn every_prefix_never_panics() {
    let options = Options::default();
    let samples = [
        "!",
        "nav.main>ul#list>li.item$$@-3*4>a[href=\"#s$\" title='x y']{Item $}+span^^footer",
        "(header>h1{${1:Title}})+main>(section>p>lorem5)*2",
        "form:post>fieldset>legend{Login}+label[for=u]+input:text#u+btn:s{Go}",
        "Foo.Bar>Baz[onClick={() => go(1)}]/",
        "m10-20!+p0+bd1-s#f00+c:r+d:ib+w100p+lh1.5",
    ];
    for sample in samples {
        for (end, _) in sample.char_indices().chain([(sample.len(), ' ')]) {
            let prefix = &sample[..end];
            for syntax in SYNTAXES {
                let _ = expand(prefix, syntax, &options);
                let _ = wrap(prefix, "x", syntax, &options);
                let _ = is_plausible(prefix, syntax);
                let _ = extract_abbreviation(prefix, syntax);
            }
        }
    }
}

#[test]
fn limits() {
    let options = Options::default();
    assert!(expand("div*1001", Syntax::Html, &options).is_err());
    assert!(expand("(div*1000)*1000", Syntax::Html, &options).is_err());
    assert!(expand("lorem10000*1000", Syntax::Html, &options).is_err());
    assert!(expand(&"x".repeat(9000), Syntax::Html, &options).is_err());
    // Big but within the limits: 1000 elements, fine.
    assert_eq!(
        expand("p*1000", Syntax::Html, &options)
            .unwrap()
            .matches("<p>")
            .count(),
        1000
    );
    // Output over 1 MB.
    let long_text = format!("p{{{}}}*1000", "x".repeat(2000));
    assert!(expand(&long_text, Syntax::Html, &options).is_err());
    let huge_content = "y".repeat(2 << 20);
    assert!(wrap("p", &huge_content, Syntax::Html, &options).is_err());
}

#[test]
fn typical_expansions_are_fast() {
    let options = Options::default();
    let cases = [
        (Syntax::Html, "!"),
        (Syntax::Html, "ul>li.item$*5>a{Item $}"),
        (
            Syntax::Html,
            ".card>.card-body>h5.card-title{Title}+p.card-text",
        ),
        (Syntax::Html, "form>input:email+input:password+btn:s"),
        (Syntax::Html, "table>tr*3>td*4"),
        (Syntax::Html, "p>lorem30"),
        (Syntax::Jsx, "section.hero>h1+p+button.btn"),
        (Syntax::Css, "m10-20+p5+d:f+aic+jcsb"),
    ];
    // Warm the snippet tables.
    for (syntax, abbreviation) in cases {
        expand(abbreviation, syntax, &options).unwrap();
    }
    let rounds = 50;
    let start = Instant::now();
    for _ in 0..rounds {
        for (syntax, abbreviation) in cases {
            expand(abbreviation, syntax, &options).unwrap();
            is_plausible(abbreviation, syntax);
        }
    }
    let each = start.elapsed() / (rounds * cases.len() as u32);
    // Well under a millisecond even unoptimized (an optimized build is
    // around ten times faster).
    assert!(each < Duration::from_millis(1), "{each:?} per expansion");
}

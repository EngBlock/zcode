//! Measures tree-sitter query compilation for Swift (it runs once per opened
//! editor inside the highlighter), per top-level pattern.
use std::time::Instant;

fn split_patterns(src: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0i32;
    let mut current = String::new();
    let mut in_string = false;
    let mut prev = ' ';
    for line in src.lines() {
        let trimmed = line.trim_start();
        if depth == 0 && (trimmed.starts_with(';') || trimmed.is_empty()) {
            continue;
        }
        for ch in line.chars() {
            if ch == '"' && prev != '\\' {
                in_string = !in_string;
            }
            if !in_string {
                match ch {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    _ => {}
                }
            }
            prev = ch;
        }
        current.push_str(line);
        current.push('\n');
        if depth == 0 && !in_string {
            out.push(std::mem::take(&mut current));
        }
    }
    out
}

fn main() {
    let lang: tree_sitter::Language = tree_sitter_swift::LANGUAGE.into();
    let src = std::env::args().nth(1).map(|p| std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|| tree_sitter_swift::HIGHLIGHTS_QUERY.to_string());
    for run in ["whole (first)", "whole (again)"] {
        let t = Instant::now();
        tree_sitter::Query::new(&lang, &src).unwrap();
        println!("{run}: {:?}", t.elapsed());
    }
    let mut timings: Vec<_> = split_patterns(&src)
        .into_iter()
        .map(|p| {
            let t = Instant::now();
            let ok = tree_sitter::Query::new(&lang, &p).is_ok();
            (t.elapsed(), ok, p)
        })
        .collect();
    timings.sort_by_key(|t| std::cmp::Reverse(t.0));
    for (d, ok, p) in timings.iter().take(8) {
        println!("{d:?} ok={ok} :: {}", p.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect::<String>());
    }
}

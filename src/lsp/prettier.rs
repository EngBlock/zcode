//! Formatting with a web app's own Prettier (`formatter`: `"prettier"`, or
//! `"auto"` for a web app that has it installed), as its editor
//! integrations do: `prettier --stdin-filepath <file>`, which reads the
//! project's configuration and ignore file for that path.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// The languages Prettier formats without plugins.
const EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "json", "jsonc", "json5", "css", "scss", "less", "html", "htm", "vue", "md", "mdx",
    "yaml", "yml", "graphql", "gql", "hbs", "handlebars",
];

const TIMEOUT: Duration = Duration::from_secs(10);

/// Whether Prettier formats `path` on its own.
pub fn supports(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    EXTENSIONS.contains(&ext) || matches!(name, ".prettierrc" | ".babelrc" | ".swcrc")
}

/// The web app's Prettier for `path`: `node_modules/.bin/prettier` in a
/// folder between it and the project's.
pub fn find(path: &Path, project_root: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(project_root))
        .map(|dir| dir.join("node_modules/.bin/prettier"))
        .find(|bin| bin.is_file())
}

/// `text` (the contents of `path`) as Prettier formats it. Blocking; gives
/// up after a few seconds.
pub fn format(prettier: &Path, path: &Path, text: &str) -> Result<String> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut child = Command::new(prettier)
        .arg("--stdin-filepath")
        .arg(path)
        .env("PATH", crate::acp::agents::login_path())
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", prettier.display()))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let input = text.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let (stdout, stderr) = (child.stdout.take().context("no stdout")?, child.stderr.take().context("no stderr")?);
    let reader = std::thread::spawn(move || std::io::read_to_string(stdout));
    let errors = std::thread::spawn(move || std::io::read_to_string(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            bail!("Prettier took longer than {} s", TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    writer.join().ok();
    let output = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    if !status.success() {
        let errors = errors.join().ok().and_then(Result::ok).unwrap_or_default();
        let first = errors.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        bail!("Prettier: {}", if first.is_empty() { status.to_string() } else { first.to_string() });
    }
    Ok(output)
}

/// The edits that make `before` into `after`, a run of whole lines each,
/// so what the formatter left alone keeps its breakpoints and the cursor.
pub fn line_edits(before: &str, after: &str) -> Vec<lsp_types::TextEdit> {
    use similar::{DiffOp, TextDiff};
    let diff = TextDiff::from_lines(before, after);
    let new_lines: Vec<&str> = after.split_inclusive('\n').collect();
    let line = |n: usize| lsp_types::Position::new(n as u32, 0);
    let ends_without_newline = !before.is_empty() && !before.ends_with('\n');
    let old_count = before.split_inclusive('\n').count();
    let mut edits = vec![];
    for op in diff.ops() {
        let (old, new) = match *op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete { old_index, old_len, .. } => (old_index..old_index + old_len, 0..0),
            DiffOp::Insert { old_index, new_index, new_len } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => (old_index..old_index + old_len, new_index..new_index + new_len),
        };
        // A last line without a newline ends at its end, not the next line's start.
        let end = if old.end == old_count && ends_without_newline {
            let last = before.rsplit('\n').next().unwrap_or("");
            lsp_types::Position::new(old.end.saturating_sub(1) as u32, last.encode_utf16().count() as u32)
        } else {
            line(old.end)
        };
        let mut text: String = new_lines[new].concat();
        if old.end == old_count && ends_without_newline && old.start < old.end && text.ends_with('\n') && !after.ends_with('\n') {
            text.pop();
        }
        edits.push(lsp_types::TextEdit { range: lsp_types::Range::new(line(old.start), end), new_text: text });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edits: &[lsp_types::TextEdit]) -> String {
        let rope = zcode_editor::Rope::from(text);
        let mut out = text.to_string();
        for edit in edits.iter().rev() {
            let range = super::super::lsp_range_to_offsets(&rope, edit.range);
            out.replace_range(range, &edit.new_text);
        }
        out
    }

    #[test]
    fn only_changed_lines_are_edited() {
        let before = "const a = {b:1}\nconst c = 2\nfunction f(){return 1}\n";
        let after = "const a = { b: 1 };\nconst c = 2\nfunction f() {\n  return 1;\n}\n";
        let edits = line_edits(before, after);
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!(apply(before, &edits), after);
        assert!(line_edits(after, after).is_empty());
        // No newline at the end, before or after.
        assert_eq!(apply("a\nb", &line_edits("a\nb", "a\nB\n")), "a\nB\n");
        assert_eq!(apply("x", &line_edits("x", "y")), "y");
    }

    #[test]
    fn what_prettier_formats() {
        assert!(supports(Path::new("web/src/App.tsx")) && supports(Path::new("web/src/App.vue")) && supports(Path::new("web/index.html")));
        assert!(!supports(Path::new("App/ContentView.swift")) && !supports(Path::new("web/src/App.svelte")));
    }
}

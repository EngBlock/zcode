//! A Time Profiler recording in brief: the functions with the most samples,
//! read from `xctrace export`'s XML (the trace's table of contents, then
//! its `time-profile` table). Values the export has already written are
//! referred to by `ref` to their first occurrence's `id`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use roxmltree::{Document, Node};

use super::xctrace::xctrace;

/// Tables of sampled stacks, by what records them: Time Profiler's, then
/// CPU Profiler's.
const SAMPLE_TABLES: &[&str] = &["time-profile", "cpu-profile"];

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCost {
    pub name: String,
    /// The library or executable it's in.
    pub binary: Option<String>,
    /// Weight of the samples it was running in (nanoseconds, or cycles for
    /// CPU Profiler).
    pub self_weight: u64,
    /// Weight of the samples it was anywhere on the stack.
    pub total_weight: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub table: String,
    /// The process the samples are from (all of them when `None`).
    pub process: Option<String>,
    pub samples: usize,
    pub total_weight: u64,
    /// Most self weight first.
    pub functions: Vec<FunctionCost>,
}

impl Summary {
    /// The top `count` functions as console lines.
    pub fn lines(&self, count: usize) -> Vec<String> {
        let percent = |weight: u64| 100. * weight as f64 / self.total_weight.max(1) as f64;
        let duration = match self.table.as_str() {
            "time-profile" => format!(", {:.2} s", self.total_weight as f64 / 1e9),
            _ => String::new(),
        };
        let mut lines = vec![format!(
            "Heaviest functions{} ({} samples{duration}):",
            self.process.as_ref().map(|p| format!(" in {p}")).unwrap_or_default(),
            self.samples,
        )];
        lines.push("    Self   Total  Function".to_string());
        for function in self.functions.iter().take(count) {
            lines.push(format!(
                "  {:5.1}%  {:5.1}%  {}{}",
                percent(function.self_weight),
                percent(function.total_weight),
                function.name,
                function.binary.as_ref().map(|b| format!("  ({b})")).unwrap_or_default(),
            ));
        }
        lines
    }
}

/// The sampled-stack tables in a trace's table of contents, as (run,
/// schema).
pub fn sample_tables(toc: &str) -> Result<Vec<(String, String)>> {
    let document = Document::parse(toc).context("reading the trace's table of contents")?;
    let mut tables = vec![];
    for run in document.descendants().filter(|n| n.has_tag_name("run")) {
        let number = run.attribute("number").unwrap_or("1");
        for table in run.descendants().filter(|n| n.has_tag_name("table")) {
            if let Some(schema) = table.attribute("schema").filter(|s| SAMPLE_TABLES.contains(s)) {
                tables.push((number.to_string(), schema.to_string()));
            }
        }
    }
    Ok(tables)
}

/// Exported elements by `id`, for the `ref`s that repeat them.
struct Refs<'a, 'input> {
    ids: HashMap<&'a str, Node<'a, 'input>>,
}

impl<'a, 'input> Refs<'a, 'input> {
    fn new(document: &'a Document<'input>) -> Self {
        let ids = document.descendants().filter_map(|n| Some((n.attribute("id")?, n))).collect();
        Self { ids }
    }

    fn resolve(&self, node: Node<'a, 'input>) -> Node<'a, 'input> {
        match node.attribute("ref").and_then(|r| self.ids.get(r)) {
            Some(target) => *target,
            None => node,
        }
    }

    /// The first element named `name` in `node` (itself included),
    /// following refs.
    fn find(&self, node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
        let node = self.resolve(node);
        if node.has_tag_name(name) {
            return Some(node);
        }
        node.children().filter(Node::is_element).find_map(|child| self.find(child, name))
    }
}

/// A sample's process: `HelloClassic` from `HelloClassic (1234)`.
fn process_name(fmt: &str) -> &str {
    fmt.rsplit_once(" (").map_or(fmt, |(name, _)| name).trim()
}

/// Reads an exported `time-profile` (or `cpu-profile`) table. `process`
/// keeps the samples of the process of that name when there are any.
pub fn parse_samples(xml: &str, process: Option<&str>) -> Result<Summary> {
    let document = Document::parse(xml).context("reading the exported samples")?;
    let refs = Refs::new(&document);
    let table = document
        .descendants()
        .find(|n| n.has_tag_name("schema"))
        .and_then(|s| s.attribute("name"))
        .unwrap_or("time-profile")
        .to_string();
    struct Sample<'a> {
        process: Option<&'a str>,
        weight: u64,
        /// Innermost first.
        frames: Vec<(String, Option<String>)>,
    }
    let mut samples = vec![];
    for row in document.descendants().filter(|n| n.has_tag_name("row")) {
        let weight = ["weight", "cycle-weight", "pmc-events"]
            .iter()
            .find_map(|name| refs.find(row, name))
            .and_then(|w| w.text())
            .and_then(|t| t.trim().parse::<u64>().ok())
            .unwrap_or(1);
        let process = refs.find(row, "process").and_then(|p| p.attribute("fmt")).map(process_name);
        let frames = refs
            .find(row, "backtrace")
            .map(|backtrace| {
                backtrace
                    .children()
                    .filter(|n| n.has_tag_name("frame"))
                    .map(|frame| refs.resolve(frame))
                    .map(|frame| {
                        let name = frame.attribute("name").or(frame.attribute("addr")).unwrap_or("???").to_string();
                        let binary = refs.find(frame, "binary").and_then(|b| b.attribute("name")).map(str::to_string);
                        (name, binary)
                    })
                    .collect()
            })
            .unwrap_or_default();
        samples.push(Sample { process, weight, frames });
    }
    let process = process.filter(|p| samples.iter().any(|s| s.process == Some(*p)));
    let mut summary = Summary { table, process: process.map(str::to_string), ..Summary::default() };
    let mut costs: HashMap<(String, Option<String>), (u64, u64)> = HashMap::new();
    for sample in samples.iter().filter(|s| process.is_none() || s.process == process) {
        summary.samples += 1;
        summary.total_weight += sample.weight;
        if let Some(leaf) = sample.frames.first() {
            costs.entry(leaf.clone()).or_default().0 += sample.weight;
        }
        // Recursion counts once per sample.
        let mut seen = HashSet::new();
        for frame in &sample.frames {
            if seen.insert(frame) {
                costs.entry(frame.clone()).or_default().1 += sample.weight;
            }
        }
    }
    summary.functions = costs
        .into_iter()
        .map(|((name, binary), (self_weight, total_weight))| FunctionCost { name, binary, self_weight, total_weight })
        .filter(|f| f.self_weight > 0)
        .collect();
    summary.functions.sort_by(|a, b| b.self_weight.cmp(&a.self_weight).then_with(|| a.name.cmp(&b.name)));
    Ok(summary)
}

fn export(trace: &Path, query: &[&str]) -> Result<String> {
    let output = xctrace()
        .args(["export", "--input"])
        .arg(trace)
        .args(query)
        .stdin(Stdio::null())
        .output()
        .context("running xctrace export")?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The heaviest functions of a trace's last sampled run (`None`: the
/// template records no samples, as Allocations or Leaks).
pub fn summarize(trace: &Path, process: Option<&str>) -> Result<Option<Summary>> {
    let toc = export(trace, &["--toc"])?;
    let Some((run, schema)) = sample_tables(&toc)?.pop() else { return Ok(None) };
    let xpath = format!("/trace-toc/run[@number=\"{run}\"]/data/table[@schema=\"{schema}\"]");
    let samples = export(trace, &["--xpath", &xpath])?;
    Ok(Some(parse_samples(&samples, process)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOC: &str = r#"<?xml version="1.0"?>
<trace-toc>
    <run number="1">
        <info>
            <target>
                <device platform="macOS" model="MacBook Pro" name="Mac" os-version="27.2" uuid="73BDC8FE"/>
                <process type="launched" return-exit-status="0" termination-reason="Terminated" name="HelloClassic" pid="4242"/>
            </target>
            <summary>
                <duration>2.000000</duration>
            </summary>
        </info>
        <data>
            <table schema="tick"/>
            <table schema="time-sample" target-pid="SINGLE"/>
            <table schema="time-profile" target-pid="SINGLE"/>
            <table schema="kdebug-signpost" codes="&quot;0x1,0x2&quot;"/>
        </data>
    </run>
</trace-toc>"#;

    // The shape of `xctrace export --xpath '…table[@schema="time-profile"]'`:
    // a value's first occurrence has an id, repeats refer to it.
    const SAMPLES: &str = r#"<?xml version="1.0"?>
<trace-query-result>
<node xpath='//trace-toc[1]/run[1]/data[1]/table[3]'><schema name="time-profile"><col><mnemonic>time</mnemonic><name>Sample Time</name><engineering-type>sample-time</engineering-type></col><col><mnemonic>thread</mnemonic><name>Thread</name><engineering-type>thread</engineering-type></col><col><mnemonic>weight</mnemonic><name>Weight</name><engineering-type>weight</engineering-type></col><col><mnemonic>stack</mnemonic><name>Backtrace</name><engineering-type>tagged-backtrace</engineering-type></col></schema>
<row><sample-time id="1" fmt="00:00.211.375">211375833</sample-time><thread id="2" fmt="Main Thread 0x8a7f1 (HelloClassic, pid: 4242)"><tid id="3" fmt="0x8a7f1">567281</tid><process id="4" fmt="HelloClassic (4242)"><pid id="5" fmt="4242">4242</pid></process></thread><core id="6" fmt="CPU 4 (P Core)">4</core><thread-state id="7" fmt="Running">Running</thread-state><weight id="8" fmt="1.00 ms">1000000</weight><tagged-backtrace id="9" fmt="Greeter.greeting.getter ← (2 other frames)"><backtrace id="10"><frame id="11" name="Greeter.greeting.getter" addr="0x100003f10"><binary id="12" name="HelloClassic" UUID="A1" arch="arm64" load-addr="0x100000000" path="/Build/HelloClassic.app/Contents/MacOS/HelloClassic"/></frame><frame id="13" name="ContentView.body.getter" addr="0x100004a20"><binary ref="12"/></frame><frame id="14" name="main" addr="0x100005000"><binary ref="12"/></frame></backtrace></tagged-backtrace></row>
<row><sample-time id="15" fmt="00:00.212.375">212375833</sample-time><thread ref="2"/><core ref="6"/><thread-state ref="7"/><weight ref="8"/><tagged-backtrace ref="9"/></row>
<row><sample-time id="16" fmt="00:00.213.375">213375833</sample-time><thread ref="2"/><core ref="6"/><thread-state ref="7"/><weight id="17" fmt="2.00 ms">2000000</weight><tagged-backtrace id="18" fmt="objc_msgSend"><backtrace id="19"><frame id="20" name="objc_msgSend" addr="0x18a000000"><binary id="21" name="libobjc.A.dylib" path="/usr/lib/libobjc.A.dylib"/></frame><frame ref="13"/><frame ref="14"/></backtrace></tagged-backtrace></row>
<row><sample-time id="22" fmt="00:00.214.375">214375833</sample-time><thread id="23" fmt="Thread 0x9 (WindowServer, pid: 99)"><tid id="24" fmt="0x9">9</tid><process id="25" fmt="WindowServer (99)"><pid id="26" fmt="99">99</pid></process></thread><core ref="6"/><thread-state ref="7"/><weight ref="17"/><tagged-backtrace id="27"><backtrace id="28"><frame id="29" name="0x1a2b3c" addr="0x1a2b3c"/></backtrace></tagged-backtrace></row>
<row><sample-time id="30" fmt="00:00.215.375">215375833</sample-time><thread ref="2"/><core ref="6"/><thread-state ref="7"/><weight ref="8"/><sentinel/></row>
</node></trace-query-result>"#;

    #[test]
    fn finds_the_sample_table() {
        assert_eq!(sample_tables(TOC).unwrap(), [("1".to_string(), "time-profile".to_string())]);
        assert!(sample_tables("<trace-toc><run number=\"1\"><data><table schema=\"leaks\"/></data></run></trace-toc>").unwrap().is_empty());
    }

    #[test]
    fn heaviest_functions_of_the_app() {
        let summary = parse_samples(SAMPLES, Some("HelloClassic")).unwrap();
        assert_eq!(summary.table, "time-profile");
        assert_eq!(summary.process.as_deref(), Some("HelloClassic"));
        // The WindowServer sample is another process's; one has no stack.
        assert_eq!((summary.samples, summary.total_weight), (4, 5_000_000));
        let top: Vec<_> = summary.functions.iter().map(|f| (f.name.as_str(), f.binary.as_deref(), f.self_weight, f.total_weight)).collect();
        assert_eq!(
            top,
            [
                ("Greeter.greeting.getter", Some("HelloClassic"), 2_000_000, 2_000_000),
                ("objc_msgSend", Some("libobjc.A.dylib"), 2_000_000, 2_000_000),
            ]
        );
        let lines = summary.lines(10);
        assert_eq!(lines[0], "Heaviest functions in HelloClassic (4 samples, 0.01 s):");
        assert_eq!(lines[2], "   40.0%   40.0%  Greeter.greeting.getter  (HelloClassic)");
    }

    #[test]
    fn every_process_without_a_match() {
        let summary = parse_samples(SAMPLES, Some("Elsewhere")).unwrap();
        assert_eq!(summary.process, None);
        assert_eq!(summary.samples, 5);
        assert!(summary.functions.iter().any(|f| f.name == "0x1a2b3c" && f.binary.is_none()));
    }
}

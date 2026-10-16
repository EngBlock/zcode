//! Reads and edits `project.pbxproj`, the old-style (OpenStep) property list
//! inside an `.xcodeproj`. Edits are made in place on the original text so
//! the rest of the file, comments and formatting included, stays exactly as
//! Xcode wrote it.

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Array(Vec<Node>),
    Dict(Vec<Entry>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: Value,
    /// Byte range of the value in the source text.
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub value: Node,
    /// From the start of the key through the terminating `;`.
    pub span: Range<usize>,
}

impl Node {
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        match &self.value {
            Value::Dict(entries) => entries,
            _ => &[],
        }
    }

    pub fn items(&self) -> &[Node] {
        match &self.value {
            Value::Array(items) => items,
            _ => &[],
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries().iter().find(|e| e.key == key).map(|e| &e.value)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Node::as_str)
    }
}

// ---------------------------------------------------------------------------
// Parsing

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    fn skip_trivia(&mut self) -> Result<()> {
        let bytes = self.bytes();
        loop {
            while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if bytes[self.pos..].starts_with(b"/*") {
                let end = self.text[self.pos + 2..]
                    .find("*/")
                    .ok_or_else(|| anyhow!("unterminated comment at byte {}", self.pos))?;
                self.pos += 2 + end + 2;
            } else if bytes[self.pos..].starts_with(b"//") {
                self.pos = self.text[self.pos..]
                    .find('\n')
                    .map_or(bytes.len(), |n| self.pos + n);
            } else {
                return Ok(());
            }
        }
    }

    fn peek(&mut self) -> Result<u8> {
        self.skip_trivia()?;
        self.bytes()
            .get(self.pos)
            .copied()
            .ok_or_else(|| anyhow!("unexpected end of file"))
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek()? != byte {
            bail!("expected '{}' at byte {}", byte as char, self.pos);
        }
        self.pos += 1;
        Ok(())
    }

    fn value(&mut self) -> Result<Node> {
        let start = {
            self.peek()?;
            self.pos
        };
        let value = match self.peek()? {
            b'{' => {
                self.pos += 1;
                let mut entries = vec![];
                while self.peek()? != b'}' {
                    let key_start = self.pos;
                    let key = self.string()?;
                    self.expect(b'=')?;
                    let value = self.value()?;
                    self.expect(b';')?;
                    entries.push(Entry { key, value, span: key_start..self.pos });
                }
                self.pos += 1;
                Value::Dict(entries)
            }
            b'(' => {
                self.pos += 1;
                let mut items = vec![];
                while self.peek()? != b')' {
                    items.push(self.value()?);
                    if self.peek()? == b',' {
                        self.pos += 1;
                    }
                }
                self.pos += 1;
                Value::Array(items)
            }
            _ => Value::String(self.string()?),
        };
        Ok(Node { value, span: start..self.pos })
    }

    fn string(&mut self) -> Result<String> {
        let bytes = self.bytes();
        if self.peek()? == b'"' {
            self.pos += 1;
            let mut out = String::new();
            loop {
                let rest = &self.text[self.pos..];
                let mut chars = rest.char_indices();
                let Some((_, c)) = chars.next() else { bail!("unterminated string") };
                self.pos += c.len_utf8();
                match c {
                    '"' => return Ok(out),
                    '\\' => {
                        let Some((_, e)) = chars.next() else { bail!("unterminated string") };
                        self.pos += e.len_utf8();
                        out.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                    c => out.push(c),
                }
            }
        }
        let start = self.pos;
        while self.pos < bytes.len() && is_unquoted_byte(bytes[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            bail!("unexpected '{}' at byte {}", bytes[start] as char, start);
        }
        Ok(self.text[start..self.pos].to_string())
    }
}

fn is_unquoted_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'/' | b':' | b'.' | b'-')
}

pub fn parse(text: &str) -> Result<Node> {
    let mut parser = Parser { text, pos: 0 };
    parser.value()
}

/// Quotes a value the way Xcode does: bare when it is a plain word.
pub fn quote(value: &str) -> String {
    let bare = !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'/' | b':' | b'.'))
        && !value.contains("//")
        && !value.contains("___");
    if bare {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Model

#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    /// Lists are joined with spaces, as Xcode shows them.
    pub value: String,
    pub is_list: bool,
}

#[derive(Clone, Debug)]
pub struct BuildConfiguration {
    pub id: String,
    pub name: String,
    /// The `.xcconfig` file the configuration is based on, if any.
    pub base_configuration: Option<String>,
    pub settings: Vec<Setting>,
}

impl BuildConfiguration {
    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub isa: String,
    /// e.g. `com.apple.product-type.application`.
    pub product_type: Option<String>,
    pub configurations: Vec<BuildConfiguration>,
}

impl Target {
    pub fn kind_label(&self) -> &'static str {
        match self.product_type.as_deref().unwrap_or("") {
            "com.apple.product-type.application" => "Application",
            "com.apple.product-type.application.watchapp2-container" => "Watch App",
            "com.apple.product-type.bundle.unit-test" => "Unit Tests",
            "com.apple.product-type.bundle.ui-testing" => "UI Tests",
            "com.apple.product-type.framework" => "Framework",
            "com.apple.product-type.library.static" => "Static Library",
            "com.apple.product-type.library.dynamic" => "Dynamic Library",
            "com.apple.product-type.app-extension" => "App Extension",
            "com.apple.product-type.extensionkit-extension" => "Extension",
            "com.apple.product-type.tool" => "Command Line Tool",
            "com.apple.product-type.bundle" => "Bundle",
            _ if self.isa == "PBXAggregateTarget" => "Aggregate",
            _ if self.isa == "PBXLegacyTarget" => "External Build",
            _ => "Target",
        }
    }
}

#[derive(Clone, Debug)]
pub struct XcodeProject {
    /// The `.xcodeproj` bundle.
    pub path: PathBuf,
    pub name: String,
    pub object_version: Option<String>,
    pub development_region: Option<String>,
    pub known_regions: Vec<String>,
    pub configurations: Vec<BuildConfiguration>,
    pub default_configuration: Option<String>,
    pub targets: Vec<Target>,
}

impl XcodeProject {
    pub fn pbxproj_path(path: &Path) -> PathBuf {
        path.join("project.pbxproj")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = Self::pbxproj_path(path);
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        Self::from_text(path, &text)
    }

    pub fn from_text(path: &Path, text: &str) -> Result<Self> {
        let root = parse(text).context("parsing project.pbxproj")?;
        let objects = root.get("objects").ok_or_else(|| anyhow!("no objects"))?;
        let object = |id: &str| objects.get(id);
        let project_id = root.get_str("rootObject").ok_or_else(|| anyhow!("no rootObject"))?;
        let project = object(project_id).ok_or_else(|| anyhow!("missing project object"))?;

        let configurations = |list_id: Option<&str>| -> (Vec<BuildConfiguration>, Option<String>) {
            let Some(list) = list_id.and_then(object) else { return (vec![], None) };
            let configs = list
                .get("buildConfigurations")
                .map(Node::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|id| {
                    let id = id.as_str()?;
                    let config = object(id)?;
                    let settings = config
                        .get("buildSettings")
                        .map(Node::entries)
                        .unwrap_or_default()
                        .iter()
                        .map(|e| match &e.value.value {
                            Value::Array(items) => Setting {
                                key: e.key.clone(),
                                value: items.iter().filter_map(Node::as_str).map(join_word).collect::<Vec<_>>().join(" "),
                                is_list: true,
                            },
                            Value::String(s) => Setting { key: e.key.clone(), value: s.clone(), is_list: false },
                            Value::Dict(_) => Setting { key: e.key.clone(), value: String::new(), is_list: false },
                        })
                        .collect();
                    let base_configuration = config
                        .get_str("baseConfigurationReference")
                        .and_then(object)
                        .and_then(|file| file.get_str("path").or_else(|| file.get_str("name")))
                        .map(str::to_string);
                    Some(BuildConfiguration {
                        id: id.to_string(),
                        name: config.get_str("name").unwrap_or("").to_string(),
                        base_configuration,
                        settings,
                    })
                })
                .collect();
            (configs, list.get_str("defaultConfigurationName").map(str::to_string))
        };

        let (project_configs, default_configuration) =
            configurations(project.get_str("buildConfigurationList"));
        let targets = project
            .get("targets")
            .map(Node::items)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| {
                let id = id.as_str()?;
                let target = object(id)?;
                Some(Target {
                    id: id.to_string(),
                    name: target.get_str("name").unwrap_or(id).to_string(),
                    isa: target.get_str("isa").unwrap_or("").to_string(),
                    product_type: target.get_str("productType").map(str::to_string),
                    configurations: configurations(target.get_str("buildConfigurationList")).0,
                })
            })
            .collect();

        Ok(Self {
            path: path.to_path_buf(),
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            object_version: root.get_str("objectVersion").map(str::to_string),
            development_region: project.get_str("developmentRegion").map(str::to_string),
            known_regions: project
                .get("knownRegions")
                .map(Node::items)
                .unwrap_or_default()
                .iter()
                .filter_map(Node::as_str)
                .map(str::to_string)
                .collect(),
            configurations: project_configs,
            default_configuration,
            targets,
        })
    }
}

/// List items containing spaces are quoted when shown joined.
fn join_word(item: &str) -> String {
    if item.contains(' ') { format!("\"{item}\"") } else { item.to_string() }
}

// ---------------------------------------------------------------------------
// Editing

/// Sets (or, with `None`, removes) a build setting in one build
/// configuration, returning the new file text. New keys are inserted in
/// sorted position, as Xcode keeps them.
pub fn set_build_setting(text: &str, config_id: &str, key: &str, value: Option<&str>, as_list: bool) -> Result<String> {
    let root = parse(text)?;
    let settings = root
        .get("objects")
        .and_then(|o| o.get(config_id))
        .and_then(|c| c.get("buildSettings"))
        .ok_or_else(|| anyhow!("no build settings for {config_id}"))?;
    let entries = settings.entries();
    let existing = entries.iter().find(|e| e.key == key);

    let (indent, close_indent) = {
        let line_indent = |offset: usize| {
            let line = &text[line_start(text, offset)..offset];
            line[..line.len() - line.trim_start().len()].to_string()
        };
        let close = line_indent(settings.span.end - 1);
        let indent = entries.first().map(|e| line_indent(e.span.start)).unwrap_or_else(|| format!("{close}\t"));
        (indent, close)
    };

    let rendered = value.map(|value| {
        let rendered_value = if as_list {
            let items = shlex::split(value).unwrap_or_else(|| value.split_whitespace().map(str::to_string).collect());
            let mut out = String::from("(\n");
            for item in items {
                out.push_str(&format!("{indent}\t{},\n", quote(&item)));
            }
            out.push_str(&format!("{indent})"));
            out
        } else {
            quote(value)
        };
        format!("{} = {rendered_value};", quote(key))
    });

    let mut out = text.to_string();
    match (existing, rendered) {
        (Some(entry), Some(rendered)) => out.replace_range(entry.span.clone(), &rendered),
        (Some(entry), None) => out.replace_range(removal_range(text, entry.span.clone()), ""),
        (None, Some(rendered)) => {
            match entries.iter().find(|e| e.key.as_str() > key) {
                Some(next) if starts_line(text, next.span.start) => {
                    out.insert_str(line_start(text, next.span.start), &format!("{indent}{rendered}\n"));
                }
                // A dictionary written on one line.
                Some(next) => out.insert_str(next.span.start, &format!("{rendered} ")),
                None => {
                    let close = settings.span.end - 1;
                    if entries.is_empty() && !text[settings.span.start..close].contains('\n') {
                        // `buildSettings = {};` on one line.
                        out.replace_range(
                            settings.span.start + 1..close,
                            &format!("\n{indent}{rendered}\n{close_indent}"),
                        );
                    } else if starts_line(text, close) {
                        out.insert_str(line_start(text, close), &format!("{indent}{rendered}\n"));
                    } else {
                        out.insert_str(close, &format!("{rendered} "));
                    }
                }
            }
        }
        (None, None) => {}
    }
    // Never write something Xcode couldn't open.
    parse(&out).context("edited project no longer parses")?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Text helpers for in-place edits

/// The offset of the start of the line containing `offset`.
pub(crate) fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |n| n + 1)
}

/// Whether only whitespace precedes `offset` on its line.
pub(crate) fn starts_line(text: &str, offset: usize) -> bool {
    text[line_start(text, offset)..offset].trim().is_empty()
}

/// What to delete to remove `span` (a dictionary entry, or a list item with
/// its comment and comma): the whole line when nothing else is on it,
/// otherwise the span and the spaces after it.
pub(crate) fn removal_range(text: &str, span: Range<usize>) -> Range<usize> {
    let start = line_start(text, span.start);
    let line_end = text[span.end..].find('\n').map_or(text.len(), |n| span.end + n);
    if text[start..span.start].trim().is_empty() && text[span.end..line_end].trim().is_empty() {
        start..(line_end + 1).min(text.len())
    } else {
        let rest = &text[span.end..line_end];
        span.start..span.end + rest.len() - rest.trim_start_matches([' ', '\t']).len()
    }
}

/// Writes `project.pbxproj` through a temporary file and a rename, so Xcode
/// or a crash never sees a half-written project.
pub fn write_project_file(file: &Path, text: &str) -> Result<()> {
    let tmp = file.with_extension("pbxproj.zcode-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))
}

/// The `.xcodeproj` bundles a workspace refers to, in order.
pub fn workspace_projects(workspace: &Path) -> Vec<PathBuf> {
    let base = workspace.parent().unwrap_or(workspace);
    let Ok(data) = std::fs::read_to_string(workspace.join("contents.xcworkspacedata")) else {
        return vec![];
    };
    let mut projects = vec![];
    for part in data.split("location = \"").skip(1) {
        let Some(location) = part.split('"').next() else { continue };
        let (kind, path) = location.split_once(':').unwrap_or(("group", location));
        if !path.ends_with(".xcodeproj") {
            continue;
        }
        let path = match kind {
            "absolute" => PathBuf::from(path),
            _ => base.join(path),
        };
        if path.exists() && !projects.contains(&path) {
            projects.push(path);
        }
    }
    projects
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../samples/HelloZ/HelloZ.xcodeproj/project.pbxproj");

    #[test]
    fn reads_targets_and_configurations() {
        let project = XcodeProject::from_text(Path::new("/x/HelloZ.xcodeproj"), SAMPLE).unwrap();
        assert_eq!(project.name, "HelloZ");
        assert_eq!(project.object_version.as_deref(), Some("77"));
        assert_eq!(
            project.configurations.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Debug", "Release"]
        );
        assert_eq!(project.targets.len(), 2);
        let app = &project.targets[0];
        assert_eq!(app.name, "HelloZ");
        assert_eq!(app.kind_label(), "Application");
        let debug = &app.configurations[0];
        assert_eq!(debug.get("PRODUCT_BUNDLE_IDENTIFIER").unwrap().value, "dev.zcode.HelloZ");
        assert_eq!(debug.get("PRODUCT_NAME").unwrap().value, "$(TARGET_NAME)");
        let paths = debug.get("LD_RUNPATH_SEARCH_PATHS").unwrap();
        assert!(paths.is_list);
        assert_eq!(paths.value, "$(inherited) @executable_path/Frameworks");
        assert_eq!(project.targets[1].kind_label(), "Unit Tests");
    }

    #[test]
    fn edits_existing_setting_in_place() {
        let out = set_build_setting(SAMPLE, "A1000000000000000000000E", "MARKETING_VERSION", Some("2.1"), false).unwrap();
        assert_eq!(out.len(), SAMPLE.len());
        let project = XcodeProject::from_text(Path::new("/x/HelloZ.xcodeproj"), &out).unwrap();
        let app = &project.targets[0];
        assert_eq!(app.configurations[0].get("MARKETING_VERSION").unwrap().value, "2.1");
        // Release is untouched.
        assert_eq!(app.configurations[1].get("MARKETING_VERSION").unwrap().value, "1.0");
    }

    #[test]
    fn inserts_sorted_and_removes() {
        let out = set_build_setting(SAMPLE, "A1000000000000000000000E", "DEVELOPMENT_TEAM", Some("ABC123"), false).unwrap();
        assert!(out.contains("\t\t\t\tCURRENT_PROJECT_VERSION = 1;\n\t\t\t\tDEVELOPMENT_TEAM = ABC123;\n\t\t\t\tENABLE_PREVIEWS = YES;\n"));
        let out = set_build_setting(&out, "A1000000000000000000000E", "ZZZ_LAST", Some("a b"), false).unwrap();
        assert!(out.contains("\t\t\t\tTARGETED_DEVICE_FAMILY = \"1,2\";\n\t\t\t\tZZZ_LAST = \"a b\";\n\t\t\t};"));
        let out = set_build_setting(&out, "A1000000000000000000000E", "ZZZ_LAST", None, false).unwrap();
        let out = set_build_setting(&out, "A1000000000000000000000E", "DEVELOPMENT_TEAM", None, false).unwrap();
        assert_eq!(out, SAMPLE);
    }

    #[test]
    fn writes_lists() {
        let out = set_build_setting(
            SAMPLE,
            "A1000000000000000000000E",
            "LD_RUNPATH_SEARCH_PATHS",
            Some("$(inherited) @executable_path/Frameworks \"/a b\""),
            true,
        )
        .unwrap();
        assert!(out.contains(
            "LD_RUNPATH_SEARCH_PATHS = (\n\t\t\t\t\t\"$(inherited)\",\n\t\t\t\t\t\"@executable_path/Frameworks\",\n\t\t\t\t\t\"/a b\",\n\t\t\t\t);"
        ));
    }

    #[test]
    fn fills_empty_build_settings() {
        let text = "{ objects = {\n\t\tC1 = {\n\t\t\tisa = XCBuildConfiguration;\n\t\t\tbuildSettings = {\n\t\t\t};\n\t\t};\n\t\tC2 = {isa = XCBuildConfiguration; buildSettings = {}; };\n\t}; }";
        let out = set_build_setting(text, "C1", "SWIFT_VERSION", Some("6.0"), false).unwrap();
        assert!(out.contains("buildSettings = {\n\t\t\t\tSWIFT_VERSION = 6.0;\n\t\t\t};"));
        let out = set_build_setting(&out, "C2", "SWIFT_VERSION", Some("6.0"), false).unwrap();
        let root = parse(&out).unwrap();
        let c2 = root.get("objects").unwrap().get("C2").unwrap();
        assert_eq!(c2.get("buildSettings").unwrap().get_str("SWIFT_VERSION"), Some("6.0"));
    }

    #[test]
    fn edits_one_line_dictionaries() {
        let text = "{ objects = {\n\t\tC1 = {isa = XCBuildConfiguration; buildSettings = {ALPHA = 1; GAMMA = 3; }; name = Debug; };\n\t}; }";
        let out = set_build_setting(text, "C1", "BETA", Some("2"), false).unwrap();
        assert!(out.contains("buildSettings = {ALPHA = 1; BETA = 2; GAMMA = 3; };"), "{out}");
        let out = set_build_setting(&out, "C1", "ZETA", Some("4"), false).unwrap();
        assert!(out.contains("GAMMA = 3; ZETA = 4; };"), "{out}");
        let out = set_build_setting(&out, "C1", "ZETA", None, false).unwrap();
        let out = set_build_setting(&out, "C1", "BETA", None, false).unwrap();
        assert_eq!(out, text);
    }

    #[test]
    fn rejects_unknown_configuration() {
        assert!(set_build_setting(SAMPLE, "FFFFFFFFFFFFFFFFFFFFFFFF", "A", Some("1"), false).is_err());
    }

    #[test]
    fn quoting_matches_xcode() {
        assert_eq!(quote("dev.zcode.HelloZ"), "dev.zcode.HelloZ");
        assert_eq!(quote("$(TARGET_NAME)"), "\"$(TARGET_NAME)\"");
        assert_eq!(quote("-Onone"), "\"-Onone\"");
        assert_eq!(quote("1,2"), "\"1,2\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    }
}

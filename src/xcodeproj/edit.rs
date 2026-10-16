//! Batched, in-place edits to `project.pbxproj` text. Callers queue new
//! objects, removed objects, list insertions and removals and value
//! replacements against one parse of the file; `finish` turns them into
//! byte-range edits so every byte outside them stays as it was.
//!
//! New objects are written the way Xcode writes them: in the section for
//! their `isa` (`/* Begin PBXFileReference section */`), sorted by ID, with
//! `ID /* name */` comments, one line for build files and file references
//! and one line per key for everything else.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::ops::Range;

use anyhow::{Result, anyhow, bail};

use super::{Node, line_start, parse, quote, removal_range, starts_line};

// ---------------------------------------------------------------------------
// Object IDs

/// Generates object IDs the way Xcode does: 24 uppercase hex characters (96
/// bits), unique within the file.
pub struct IdGenerator {
    used: HashSet<String>,
    hasher: RandomState,
    counter: u64,
}

impl IdGenerator {
    pub fn new(text: &str) -> Self {
        let mut used = HashSet::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_hexdigit() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
                let len = bytes[i..].iter().take_while(|b| b.is_ascii_alphanumeric()).count();
                if len == 24 {
                    used.insert(text[i..i + 24].to_ascii_uppercase());
                }
                i += len.max(1);
            } else {
                i += 1;
            }
        }
        Self { used, hasher: RandomState::new(), counter: 0 }
    }

    pub fn next(&mut self) -> String {
        loop {
            self.counter += 1;
            let hash = |salt: u64| {
                let mut h = self.hasher.build_hasher();
                h.write_u64(self.counter);
                h.write_u64(salt);
                h.write_u32(std::process::id());
                h.finish()
            };
            let (a, b) = (hash(1), hash(2));
            let id = format!("{:016X}{:08X}", a, b as u32);
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// New objects

#[derive(Clone, Debug)]
pub enum Field {
    Str(String),
    /// An object reference, written `ID /* comment */`.
    Ref(String, Option<String>),
    List(Vec<(String, Option<String>)>),
    /// A nested dictionary of strings, like a package's `requirement`.
    Dict(Vec<(&'static str, String)>),
}

#[derive(Clone, Debug)]
pub struct NewObject {
    pub id: String,
    pub isa: &'static str,
    pub comment: Option<String>,
    /// In Xcode's order: alphabetical after `isa`.
    pub fields: Vec<(&'static str, Field)>,
    /// Build files and file references are written on one line.
    pub one_line: bool,
}

impl NewObject {
    pub fn list_mut(&mut self, key: &str) -> Option<&mut Vec<(String, Option<String>)>> {
        self.fields.iter_mut().find(|(k, _)| *k == key).and_then(|(_, f)| match f {
            Field::List(items) => Some(items),
            _ => None,
        })
    }

    fn render(&self, indent: &str) -> String {
        let reference = |id: &str, comment: &Option<String>| match comment {
            Some(c) => format!("{id} /* {c} */"),
            None => id.to_string(),
        };
        let head = format!("{indent}{} = {{", reference(&self.id, &self.comment));
        if self.one_line {
            let mut out = format!("{head}isa = {}; ", self.isa);
            for (key, field) in &self.fields {
                let value = match field {
                    Field::Str(s) => quote(s),
                    Field::Ref(id, c) => reference(id, c),
                    Field::List(items) => {
                        let items: Vec<_> = items.iter().map(|(id, c)| format!("{}, ", reference(id, c))).collect();
                        format!("({})", items.concat())
                    }
                    Field::Dict(entries) => {
                        let entries: Vec<_> = entries.iter().map(|(k, v)| format!("{} = {}; ", quote(k), quote(v))).collect();
                        format!("{{{}}}", entries.concat())
                    }
                };
                out.push_str(&format!("{key} = {value}; "));
            }
            out.push_str("};\n");
            return out;
        }
        let mut out = format!("{head}\n{indent}\tisa = {};\n", self.isa);
        for (key, field) in &self.fields {
            match field {
                Field::Str(s) => out.push_str(&format!("{indent}\t{key} = {};\n", quote(s))),
                Field::Ref(id, c) => out.push_str(&format!("{indent}\t{key} = {};\n", reference(id, c))),
                Field::List(items) => {
                    out.push_str(&format!("{indent}\t{key} = (\n"));
                    for (id, c) in items {
                        out.push_str(&format!("{indent}\t\t{},\n", reference(id, c)));
                    }
                    out.push_str(&format!("{indent}\t);\n"));
                }
                Field::Dict(entries) => out.push_str(&format!("{indent}\t{key} = {};\n", render_dict(entries, &format!("{indent}\t")))),
            }
        }
        out.push_str(&format!("{indent}}};\n"));
        out
    }
}

/// A dictionary of strings written over several lines, closing at `indent`.
fn render_dict(entries: &[(&'static str, String)], indent: &str) -> String {
    let mut out = String::from("{\n");
    for (key, value) in entries {
        out.push_str(&format!("{indent}\t{} = {};\n", quote(key), quote(value)));
    }
    out.push_str(&format!("{indent}}}"));
    out
}

// ---------------------------------------------------------------------------
// The batch

/// Where a list item goes.
#[derive(Clone, Debug)]
pub enum Position {
    End,
    /// Before an existing item.
    Before(String),
}

#[derive(Clone, Debug)]
struct ListAdd {
    owner: String,
    key: &'static str,
    item: String,
    comment: Option<String>,
    position: Position,
}

pub struct Batch<'a> {
    pub text: &'a str,
    pub root: Node,
    /// Object ID → its index in `objects`.
    index: HashMap<String, usize>,
    pub ids: IdGenerator,
    pub new_objects: Vec<NewObject>,
    removed: HashSet<String>,
    list_adds: Vec<ListAdd>,
    list_removes: HashSet<(String, &'static str, String)>,
    replacements: Vec<(Range<usize>, String)>,
}

/// A `/* Begin X section */` … `/* End X section */` block.
struct Section {
    isa: String,
    /// Start of the Begin line.
    begin: usize,
    /// Start of the End line.
    end_line: usize,
    /// Just past the End line's newline.
    end: usize,
}

impl<'a> Batch<'a> {
    pub fn new(text: &'a str) -> Result<Self> {
        let root = parse(text)?;
        let objects = root.get("objects").ok_or_else(|| anyhow!("no objects"))?;
        let index = objects.entries().iter().enumerate().map(|(ix, e)| (e.key.clone(), ix)).collect();
        Ok(Self {
            text,
            root,
            index,
            ids: IdGenerator::new(text),
            new_objects: vec![],
            removed: HashSet::new(),
            list_adds: vec![],
            list_removes: HashSet::new(),
            replacements: vec![],
        })
    }

    pub fn objects(&self) -> &Node {
        self.root.get("objects").expect("checked in new")
    }

    pub fn object(&self, id: &str) -> Option<&Node> {
        self.index.get(id).map(|&ix| &self.objects().entries()[ix].value)
    }

    pub fn is_new(&self, id: &str) -> bool {
        self.new_objects.iter().any(|o| o.id == id)
    }

    pub fn new_object_mut(&mut self, id: &str) -> Option<&mut NewObject> {
        self.new_objects.iter_mut().find(|o| o.id == id)
    }

    pub fn add_object(&mut self, object: NewObject) {
        self.new_objects.push(object);
    }

    pub fn remove_object(&mut self, id: &str) {
        if let Some(ix) = self.new_objects.iter().position(|o| o.id == id) {
            self.new_objects.remove(ix);
        } else {
            self.removed.insert(id.to_string());
        }
    }

    pub fn is_removed(&self, id: &str) -> bool {
        self.removed.contains(id)
    }

    /// Adds `item` to the list `key` of object `owner` (new or existing).
    pub fn add_to_list(&mut self, owner: &str, key: &'static str, item: &str, comment: Option<String>, position: Position) {
        if let Some(object) = self.new_object_mut(owner) {
            if let Some(list) = object.list_mut(key) {
                let at = match &position {
                    Position::Before(next) => list.iter().position(|(id, _)| id == next).unwrap_or(list.len()),
                    Position::End => list.len(),
                };
                list.insert(at, (item.to_string(), comment));
            }
            return;
        }
        // Re-adding something removed in this batch just cancels the removal.
        if self.list_removes.remove(&(owner.to_string(), key, item.to_string())) {
            return;
        }
        self.list_adds.push(ListAdd { owner: owner.to_string(), key, item: item.to_string(), comment, position });
    }

    pub fn remove_from_list(&mut self, owner: &str, key: &'static str, item: &str) {
        if let Some(list) = self.new_object_mut(owner).and_then(|o| o.list_mut(key)) {
            list.retain(|(id, _)| id != item);
            return;
        }
        if let Some(ix) = self.list_adds.iter().position(|a| a.owner == owner && a.key == key && a.item == item) {
            self.list_adds.remove(ix);
            return;
        }
        self.list_removes.insert((owner.to_string(), key, item.to_string()));
    }

    /// Sets a string value in an existing object, adding the key in sorted
    /// position (after `isa`) when it's missing.
    pub fn set_value(&mut self, id: &str, key: &str, value: &str) {
        let Some(object) = self.object(id) else { return };
        if let Some(entry) = object.entries().iter().find(|e| e.key == key) {
            self.replacements.push((entry.value.span.clone(), quote(value)));
            return;
        }
        let edit = entry_insertion(self.text, object, key, |_| quote(value));
        self.replacements.push(edit);
    }

    pub fn remove_key(&mut self, id: &str, key: &str) {
        let Some(entry) = self.object(id).and_then(|o| o.entries().iter().find(|e| e.key == key)) else { return };
        let range = removal_range(self.text, entry.span.clone());
        self.replacements.push((range, String::new()));
    }

    /// Rewrites the `ID /* comment */` comments that follow `id` anywhere in
    /// the file, through `rename` (which returns `None` to leave one alone).
    pub fn rename_comments(&mut self, id: &str, rename: impl Fn(&str) -> Option<String>) {
        let text = self.text;
        for (at, _) in text.match_indices(id) {
            let after = at + id.len();
            if at > 0 && text.as_bytes()[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            let Some(rest) = text[after..].strip_prefix(" /* ") else { continue };
            let Some(len) = rest.find(" */") else { continue };
            if let Some(new) = rename(&rest[..len]) {
                let start = after + 4;
                self.replacements.push((start..start + len, new));
            }
        }
    }

    /// Turns the queued changes into the edited text, and checks it parses.
    pub fn finish(mut self) -> Result<String> {
        let text = self.text;
        let objects = self.root.get("objects").ok_or_else(|| anyhow!("no objects"))?.clone();
        let mut edits = std::mem::take(&mut self.replacements);
        let sections = sections(text, &objects);
        let indent = objects
            .entries()
            .first()
            .map(|e| indent_of(text, e.span.start))
            .unwrap_or_else(|| "\t\t".to_string());

        // Removed objects, and sections they leave empty.
        let section_of = |offset: usize| sections.iter().position(|s| s.begin < offset && offset < s.end_line);
        let mut emptied = HashSet::new();
        for (ix, section) in sections.iter().enumerate() {
            let inside: Vec<_> = objects
                .entries()
                .iter()
                .filter(|e| section.begin < e.span.start && e.span.start < section.end_line)
                .collect();
            let adding = self.new_objects.iter().any(|o| o.isa == section.isa);
            if !inside.is_empty() && !adding && inside.iter().all(|e| self.removed.contains(&e.key)) {
                emptied.insert(ix);
                let (mut begin, mut end) = (section.begin, section.end);
                // Take the blank line that separates it from the one before
                // (or, failing that, the next one).
                if text[..begin].ends_with("\n\n") {
                    begin -= 1;
                } else if text[end..].starts_with('\n') {
                    end += 1;
                }
                edits.push((begin..end, String::new()));
            }
        }
        for entry in objects.entries() {
            if self.removed.contains(&entry.key) && !section_of(entry.span.start).is_some_and(|ix| emptied.contains(&ix)) {
                edits.push((removal_range(text, entry.span.clone()), String::new()));
            }
        }

        // List changes on existing objects.
        let object = |id: &str| self.index.get(id).map(|&ix| &objects.entries()[ix].value);
        for (owner, key, item) in &self.list_removes {
            let Some(list) = object(owner).and_then(|o| o.get(key)) else { continue };
            for node in list.items().iter().filter(|n| n.as_str() == Some(item.as_str())) {
                edits.push((removal_range(text, item_extent(text, node)), String::new()));
            }
        }
        let mut adds_by_list: HashMap<(String, &'static str), Vec<&ListAdd>> = HashMap::new();
        for add in &self.list_adds {
            adds_by_list.entry((add.owner.clone(), add.key)).or_default().push(add);
        }
        for ((owner, key), adds) in adds_by_list {
            let owner_object = object(&owner).ok_or_else(|| anyhow!("no object {owner}"))?;
            match owner_object.get(key) {
                Some(list) => edits.extend(list_insertions(text, list, &adds)),
                // An object without the list yet (a target's first package
                // product): add the key with its items.
                None => edits.push(entry_insertion(text, owner_object, key, |indent| {
                    let mut out = String::from("(\n");
                    for add in &adds {
                        match &add.comment {
                            Some(c) => out.push_str(&format!("{indent}\t{} /* {c} */,\n", add.item)),
                            None => out.push_str(&format!("{indent}\t{},\n", add.item)),
                        }
                    }
                    out.push_str(&format!("{indent})"));
                    out
                })),
            }
        }

        // New objects, in their sections.
        let mut by_isa: Vec<(&'static str, Vec<&NewObject>)> = vec![];
        for object in &self.new_objects {
            match by_isa.iter_mut().find(|(isa, _)| *isa == object.isa) {
                Some((_, list)) => list.push(object),
                None => by_isa.push((object.isa, vec![object])),
            }
        }
        for (isa, mut list) in by_isa {
            list.sort_by(|a, b| a.id.cmp(&b.id));
            match sections.iter().find(|s| s.isa == isa) {
                Some(section) => {
                    let inside: Vec<_> = objects
                        .entries()
                        .iter()
                        .filter(|e| section.begin < e.span.start && e.span.start < section.end_line)
                        .collect();
                    for object in list {
                        let at = inside
                            .iter()
                            .find(|e| e.key > object.id)
                            .map_or(section.end_line, |e| line_start(text, e.span.start));
                        edits.push((at..at, object.render(&indent)));
                    }
                }
                None => {
                    let body: String = list.iter().map(|o| o.render(&indent)).collect();
                    let block = format!("/* Begin {isa} section */\n{body}/* End {isa} section */\n");
                    match sections.iter().find(|s| s.isa.as_str() > isa) {
                        Some(next) => edits.push((next.begin..next.begin, format!("{block}\n"))),
                        None => match sections.last() {
                            Some(last) => edits.push((last.end..last.end, format!("\n{block}"))),
                            // A file without section comments.
                            None => {
                                let close = objects.span.end - 1;
                                let at = if starts_line(text, close) { line_start(text, close) } else { close };
                                edits.push((at..at, body));
                            }
                        },
                    }
                }
            }
        }

        let out = apply(text, edits)?;
        parse(&out).map_err(|err| anyhow!("edited project no longer parses: {err:#}"))?;
        Ok(out)
    }
}

/// Leading whitespace of the line containing `offset`.
pub fn indent_of(text: &str, offset: usize) -> String {
    let start = line_start(text, offset);
    text[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Where and what to insert to add `key` to an existing object, in sorted
/// position after `isa`, as Xcode keeps keys. `value` renders the value
/// for the entry's indent.
fn entry_insertion(text: &str, object: &Node, key: &str, value: impl Fn(&str) -> String) -> (Range<usize>, String) {
    let entries = object.entries();
    let next = entries.iter().find(|e| e.key != "isa" && e.key.as_str() > key);
    let (at, indent, own_line) = match next {
        Some(next) if starts_line(text, next.span.start) => (line_start(text, next.span.start), indent_of(text, next.span.start), true),
        Some(next) => (next.span.start, String::new(), false),
        None => {
            let close = object.span.end - 1;
            if starts_line(text, close) {
                let indent = entries.last().map(|e| indent_of(text, e.span.start)).unwrap_or_default();
                (line_start(text, close), indent, true)
            } else {
                (close, String::new(), false)
            }
        }
    };
    let rendered = format!("{} = {};", quote(key), value(&indent));
    let insertion = if own_line { format!("{indent}{rendered}\n") } else { format!("{rendered} ") };
    (at..at, insertion)
}

fn sections(text: &str, objects: &Node) -> Vec<Section> {
    let body = &text[objects.span.clone()];
    let mut out: Vec<Section> = vec![];
    let mut begin: Option<(String, usize)> = None;
    let mut offset = objects.span.start;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(isa) = trimmed.strip_prefix("/* Begin ").and_then(|r| r.strip_suffix(" section */")) {
            begin = Some((isa.to_string(), offset));
        } else if let Some(isa) = trimmed.strip_prefix("/* End ").and_then(|r| r.strip_suffix(" section */"))
            && let Some((open, start)) = begin.take()
            && open == isa
        {
            out.push(Section { isa: open, begin: start, end_line: offset, end: offset + line.len() });
        }
        offset += line.len();
    }
    out
}

/// A list item's span extended over its trailing comment and comma.
fn item_extent(text: &str, node: &Node) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut end = node.span.end;
    let skip_spaces = |mut i: usize| {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        i
    };
    let mut i = skip_spaces(end);
    if text[i..].starts_with("/*")
        && let Some(close) = text[i..].find("*/")
    {
        i = skip_spaces(i + close + 2);
        end = i;
    }
    if bytes.get(i) == Some(&b',') {
        end = i + 1;
    }
    node.span.start..end
}

fn list_insertions(text: &str, list: &Node, adds: &[&ListAdd]) -> Vec<(Range<usize>, String)> {
    let items = list.items();
    let close = list.span.end - 1;
    let multiline = text[list.span.clone()].contains('\n');
    let item_indent = items
        .first()
        .filter(|n| starts_line(text, n.span.start))
        .map(|n| indent_of(text, n.span.start))
        .unwrap_or_else(|| format!("{}\t", indent_of(text, close)));
    let render = |add: &ListAdd| match &add.comment {
        Some(c) => format!("{} /* {c} */,", add.item),
        None => format!("{},", add.item),
    };
    if !multiline {
        // `files = ( );` or a one-line list: rewrite it as Xcode would.
        let mut lines = String::from("(\n");
        let mut pending: Vec<&ListAdd> = adds.to_vec();
        for node in items {
            let id = node.as_str().unwrap_or("");
            pending.retain(|add| {
                let before = matches!(&add.position, Position::Before(next) if next == id);
                if before {
                    lines.push_str(&format!("{item_indent}{}\n", render(add)));
                }
                !before
            });
            let extent = item_extent(text, node);
            let mut item = text[extent].to_string();
            if !item.ends_with(',') {
                item.push(',');
            }
            lines.push_str(&format!("{item_indent}{item}\n"));
        }
        for add in pending {
            lines.push_str(&format!("{item_indent}{}\n", render(add)));
        }
        let close_indent = &item_indent[..item_indent.len().saturating_sub(1)];
        lines.push_str(&format!("{close_indent})"));
        return vec![(list.span.clone(), lines)];
    }
    adds.iter()
        .map(|add| {
            let next = match &add.position {
                Position::Before(id) => items.iter().find(|n| n.as_str() == Some(id.as_str())),
                Position::End => None,
            };
            let at = match next {
                Some(node) if starts_line(text, node.span.start) => line_start(text, node.span.start),
                _ if starts_line(text, close) => line_start(text, close),
                _ => close,
            };
            (at..at, format!("{item_indent}{}\n", render(add)))
        })
        .collect()
}

/// Applies non-overlapping edits. Insertions at the same offset keep their
/// order; an edit inside a deleted range is dropped (the deletion wins).
pub fn apply(text: &str, mut edits: Vec<(Range<usize>, String)>) -> Result<String> {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    edits.dedup();
    let mut kept: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
    for (range, replacement) in edits {
        if let Some((last, last_text)) = kept.last_mut()
            && range.start < last.end
        {
            if last_text.is_empty() && range.end <= last.end {
                continue;
            }
            if last_text.is_empty() && replacement.is_empty() {
                last.end = range.end;
                continue;
            }
            bail!("conflicting edits at bytes {range:?} and {last:?}");
        }
        kept.push((range, replacement));
    }
    let mut out = String::with_capacity(text.len() + kept.iter().map(|(_, s)| s.len()).sum::<usize>());
    let mut at = 0;
    for (range, replacement) in kept {
        out.push_str(&text[at..range.start]);
        out.push_str(&replacement);
        at = range.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_24_hex() {
        let text = "A10000000000000000000001 = {};";
        let mut ids = IdGenerator::new(text);
        assert!(ids.used.contains("A10000000000000000000001"));
        let generated: HashSet<_> = (0..2000).map(|_| ids.next()).collect();
        assert_eq!(generated.len(), 2000);
        for id in &generated {
            assert_eq!(id.len(), 24);
            assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)), "{id}");
        }
    }

    #[test]
    fn apply_orders_and_drops_edits_inside_deletions() {
        let out = apply("abcdef", vec![(1..1, "X".into()), (1..1, "Y".into()), (2..4, String::new()), (3..3, "Z".into())]).unwrap();
        assert_eq!(out, "aXYbef");
        assert!(apply("abcdef", vec![(1..3, "X".into()), (2..4, "Y".into())]).is_err());
    }
}

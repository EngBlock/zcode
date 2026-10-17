//! Asset catalogs (`.xcassets`): folders of sets (`.imageset`,
//! `.appiconset`, `.colorset`, `.dataset`, `.symbolset`…), each described
//! by a `Contents.json`, and folders grouping them. Read from disk off the
//! main thread (`Catalog::load`); edits are `edit::Edit`s of files.
//!
//! Xcode's asset catalog editor is the model: the outline of sets, each
//! set's slots (an entry of its `images`, `colors`, `data` or `symbols`
//! array: an idiom, scale, appearance, size…) and their files.

pub mod color;
pub mod edit;
pub mod json;
pub mod slots;
pub mod thumbnails;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

pub use slots::Slot;

pub const CONTENTS: &str = "Contents.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetKind {
    Image,
    AppIcon,
    Color,
    Data,
    Symbol,
    /// Sets zcode shows by their files only (textures, launch images,
    /// complications, AR objects…).
    Other,
}

impl SetKind {
    pub fn of_extension(ext: &str) -> Option<Self> {
        Some(match ext {
            "imageset" => SetKind::Image,
            "appiconset" => SetKind::AppIcon,
            "colorset" => SetKind::Color,
            "dataset" => SetKind::Data,
            "symbolset" => SetKind::Symbol,
            "launchimage" | "textureset" | "cubetextureset" | "mipmapset" | "complicationset" | "arobject" | "sticker"
            | "stickersequence" | "gcdashboardimage" | "gcleaderboard" | "gcleaderboardset" | "iconset" | "icon" => SetKind::Other,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            SetKind::Image => "imageset",
            SetKind::AppIcon => "appiconset",
            SetKind::Color => "colorset",
            SetKind::Data => "dataset",
            SetKind::Symbol => "symbolset",
            SetKind::Other => "",
        }
    }

    /// The array of a set's `Contents.json` that lists its slots.
    pub fn array_key(self) -> &'static str {
        match self {
            SetKind::Image | SetKind::AppIcon => "images",
            SetKind::Color => "colors",
            SetKind::Data => "data",
            SetKind::Symbol => "symbols",
            SetKind::Other => "",
        }
    }

    /// The name the attributes inspector gives the type.
    pub fn title(self) -> &'static str {
        match self {
            SetKind::Image => "Image Set",
            SetKind::AppIcon => "App Icon",
            SetKind::Color => "Color Set",
            SetKind::Data => "Data Set",
            SetKind::Symbol => "Symbol Image Set",
            SetKind::Other => "Asset",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SetKind::Image => "image",
            SetKind::AppIcon => "appicon",
            SetKind::Color => "color",
            SetKind::Data => "data",
            SetKind::Symbol => "symbol",
            SetKind::Other => "other",
        }
    }
}

/// A file of a set, with what tells a changed file from its cached
/// thumbnail.
#[derive(Clone, Debug, PartialEq)]
pub struct SetFile {
    pub name: String,
    pub modified: Option<SystemTime>,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct AssetSet {
    pub kind: SetKind,
    pub contents: Value,
    /// Why `Contents.json` couldn't be read (the set shows its files).
    pub error: Option<String>,
    /// Its files other than `Contents.json`, by name.
    pub files: Vec<SetFile>,
}

impl AssetSet {
    /// The entries of its slots' array.
    pub fn entries(&self) -> &[Value] {
        self.contents.get(self.kind.array_key()).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn slots(&self) -> Vec<Slot> {
        slots::slots(self.kind, self.entries())
    }

    pub fn file(&self, name: &str) -> Option<&SetFile> {
        self.files.iter().find(|f| f.name == name)
    }

    /// Files no slot names: Xcode warns about them ("unassigned child").
    pub fn unassigned(&self) -> Vec<&SetFile> {
        let named: Vec<&str> = self.entries().iter().filter_map(|e| e.get("filename").and_then(Value::as_str)).collect();
        self.files.iter().filter(|f| !named.contains(&f.name.as_str())).collect()
    }

    /// `properties` (rendering, compression, vector data, namespace…).
    pub fn properties(&self) -> Vec<(String, String)> {
        self.contents
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    /// The `.xcassets` folder itself.
    Catalog,
    /// A folder of sets (also sprite atlases, image stacks, AR resource
    /// groups and sticker packs, which hold sets the same way).
    Folder { namespace: bool },
    Set(AssetSet),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub kind: NodeKind,
    pub children: Vec<Node>,
}

impl Node {
    pub fn set(&self) -> Option<&AssetSet> {
        match &self.kind {
            NodeKind::Set(set) => Some(set),
            _ => None,
        }
    }

    pub fn is_folder(&self) -> bool {
        !matches!(self.kind, NodeKind::Set(_))
    }

    pub fn find(&self, path: &Path) -> Option<&Node> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children.iter().find_map(|c| c.find(path))
    }

    /// Every set below, depth first in the outline's order.
    pub fn sets(&self) -> Vec<&Node> {
        let mut out = vec![];
        for child in &self.children {
            if child.set().is_some() {
                out.push(child);
            }
            out.extend(child.sets());
        }
        out
    }

    /// What the attributes inspector calls it.
    pub fn type_title(&self) -> &'static str {
        match &self.kind {
            NodeKind::Catalog => "Asset Catalog",
            NodeKind::Folder { .. } => match self.path.extension().and_then(|e| e.to_str()) {
                Some("spriteatlas") => "Sprite Atlas",
                Some("imagestack" | "solidimagestack") => "Image Stack",
                Some("brandassets") => "Brand Assets",
                Some("arresourcegroup") => "AR Resource Group",
                Some("stickerpack") => "Sticker Pack",
                _ => "Folder",
            },
            NodeKind::Set(set) => set.kind.title(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub root: Node,
    /// How many sets it has (for the header and tests).
    pub set_count: usize,
}

impl Catalog {
    /// Reads a catalog. Blocking: for a background thread.
    pub fn load(root: &Path) -> std::io::Result<Catalog> {
        if !root.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} isn't a folder", root.display())));
        }
        let name = display_name(root);
        let mut node = Node { path: root.to_path_buf(), name, kind: NodeKind::Catalog, children: vec![] };
        node.children = read_children(root);
        let set_count = node.sets().len();
        Ok(Catalog { root: node, set_count })
    }

    pub fn find(&self, path: &Path) -> Option<&Node> {
        self.root.find(path)
    }
}

/// A set's or folder's name as the outline shows it: without `.imageset`
/// and the like; plain folders keep theirs.
pub fn display_name(path: &Path) -> String {
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if SetKind::of_extension(ext).is_some() || GROUP_EXTENSIONS.contains(&ext) || ext == "xcassets" => {
            path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or(file)
        }
        _ => file,
    }
}

/// Folders with an extension that hold sets as plain folders do.
const GROUP_EXTENSIONS: &[&str] =
    &["spriteatlas", "imagestack", "solidimagestack", "imagestacklayer", "solidimagestacklayer", "brandassets", "arresourcegroup", "stickerpack"];

fn read_children(dir: &Path) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut children = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        let name = display_name(&path);
        match SetKind::of_extension(&ext) {
            Some(kind) => children.push(Node { kind: NodeKind::Set(read_set(&path, kind)), path, name, children: vec![] }),
            None => {
                let contents = read_contents(&path).ok();
                let namespace = contents
                    .as_ref()
                    .and_then(|c| c.pointer("/properties/provides-namespace"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let grandchildren = read_children(&path);
                children.push(Node { path, name, kind: NodeKind::Folder { namespace }, children: grandchildren });
            }
        }
    }
    // Xcode's outline: by name, case-insensitively, folders mixed in.
    children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    children
}

pub fn read_contents(dir: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(dir.join(CONTENTS)).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn read_set(path: &Path, kind: SetKind) -> AssetSet {
    let (contents, error) = match read_contents(path) {
        Ok(contents) => (contents, None),
        Err(err) => (Value::Null, Some(err)),
    };
    let mut files = vec![];
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == CONTENTS || name.starts_with('.') {
                continue;
            }
            let meta = entry.metadata().ok();
            files.push(SetFile { name, modified: meta.as_ref().and_then(|m| m.modified().ok()), len: meta.map(|m| m.len()).unwrap_or(0) });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    AssetSet { kind, contents, error, files }
}

/// The catalog a path is in (or is), and the set or folder in it the path
/// names (`None` for the catalog itself). Files inside a set aren't: they
/// open as files.
pub fn locate(path: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let mut catalog = None;
    for ancestor in path.ancestors() {
        if ancestor.extension().is_some_and(|e| e == "xcassets") {
            catalog = Some(ancestor.to_path_buf());
            break;
        }
    }
    let catalog = catalog?;
    if path == catalog {
        return Some((catalog, None));
    }
    // A set or folder in it, not a file.
    if !path.is_dir() {
        return None;
    }
    Some((catalog, Some(path.to_path_buf())))
}

pub fn is_catalog(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "xcassets")
}

/// A name for a new set or folder in `dir` that isn't taken: `base`, then
/// `base 1`, `base 2`…, as Xcode names new assets.
pub fn unique_name(dir: &Path, base: &str, ext: &str) -> String {
    let taken = |name: &str| {
        let file = if ext.is_empty() { name.to_string() } else { format!("{name}.{ext}") };
        dir.join(file).exists()
            || std::fs::read_dir(dir).ok().is_some_and(|entries| {
                // Asset names are unique across kinds in a folder.
                entries.flatten().any(|e| display_name(&e.path()).eq_ignore_ascii_case(name))
            })
    };
    if !taken(base) {
        return base.to_string();
    }
    (1..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `Contents.json` files as Xcode writes them (Xcode 27's templates and
    /// catalogs edited in Xcode), checked byte for byte in `json.rs`.
    pub const FIXTURES: &[&str] = &[
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/AppIcon.appiconset/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Brand.colorset/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Logo.imageset/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Icons/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Icons/Star.symbolset/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Icons/Badge.imageset/Contents.json"),
        include_str!("../../tests/fixtures/xcassets/Assets.xcassets/Seed.dataset/Contents.json"),
    ];

    pub fn fixture_catalog() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xcassets/Assets.xcassets")
    }

    #[test]
    fn reads_a_catalogs_sets_and_folders() {
        let catalog = Catalog::load(&fixture_catalog()).unwrap();
        let names: Vec<(String, &str)> = catalog.root.children.iter().map(|n| (n.name.clone(), n.type_title())).collect();
        assert_eq!(
            names,
            vec![
                ("AppIcon".to_string(), "App Icon"),
                ("Brand".to_string(), "Color Set"),
                ("Icons".to_string(), "Folder"),
                ("Logo".to_string(), "Image Set"),
                ("Seed".to_string(), "Data Set"),
            ]
        );
        let icons = &catalog.root.children[2];
        assert!(matches!(icons.kind, NodeKind::Folder { namespace: true }));
        assert_eq!(icons.children.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Badge", "Star"]);
        assert_eq!(catalog.set_count, 6);
        let logo = catalog.root.children[3].set().unwrap();
        assert_eq!(logo.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["logo.png", "logo@2x.png", "stray.png"]);
        assert_eq!(logo.unassigned().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["stray.png"]);
        assert_eq!(logo.properties(), vec![("template-rendering-intent".to_string(), "original".to_string())]);
    }

    #[test]
    fn locates_catalogs_and_sets() {
        let root = fixture_catalog();
        assert_eq!(locate(&root), Some((root.clone(), None)));
        let set = root.join("Logo.imageset");
        assert_eq!(locate(&set), Some((root.clone(), Some(set.clone()))));
        assert_eq!(locate(&set.join("logo.png")), None, "a file opens as a file");
        assert_eq!(locate(Path::new("/tmp/other")), None);
    }

    #[test]
    fn new_names_are_unique_across_kinds() {
        let root = fixture_catalog();
        assert_eq!(unique_name(&root, "Image", "imageset"), "Image");
        assert_eq!(unique_name(&root, "Logo", "colorset"), "Logo 1");
        assert_eq!(unique_name(&root, "Brand", "colorset"), "Brand 1");
    }
}

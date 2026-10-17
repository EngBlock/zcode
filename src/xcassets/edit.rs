//! Edits to a catalog as file operations that know how to undo themselves:
//! writing a `Contents.json`, copying a dropped image into a set, creating,
//! renaming and removing sets. A removed file or set is moved into a stash
//! folder outside the project (so builds don't see it) and moved back by
//! undo. Applied off the main thread, one edit at a time; an edit that
//! fails halfway is rolled back.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::json::to_xcode_json;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Writes a file (replacing it).
    Write { path: PathBuf, bytes: Vec<u8> },
    /// Copies a file in (refusing to replace one).
    Copy { from: PathBuf, to: PathBuf },
    CreateDir { path: PathBuf },
    /// Moves a file or folder (refusing to replace one).
    Move { from: PathBuf, to: PathBuf },
    /// Moves a file or folder into the stash.
    Remove { path: PathBuf },
}

impl Op {
    /// A `Contents.json` as Xcode writes it.
    pub fn write_contents(dir: &Path, contents: &Value) -> Op {
        Op::Write { path: dir.join(super::CONTENTS), bytes: to_xcode_json(contents).into_bytes() }
    }
}

/// One undoable step: what it's called in Undo's menu item, and its
/// operations.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub name: String,
    pub ops: Vec<Op>,
    /// What to select once it's applied.
    pub select: Option<PathBuf>,
}

/// Where removed files wait for undo.
#[derive(Clone, Debug)]
pub struct Stash {
    dir: PathBuf,
}

impl Stash {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn slot(&self, path: &Path) -> std::io::Result<PathBuf> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = self.dir.join(format!("{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(path.file_name().unwrap_or_default()))
    }
}

fn exists_error(path: &Path) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("“{}” already exists", path.file_name().unwrap_or_default().to_string_lossy()))
}

/// Moves a file or folder, across volumes too (the stash may be on another).
fn move_item(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(exists_error(to));
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
            copy_tree(from, to)?;
            if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }
        }
        Err(err) => Err(err),
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Applies one operation; its inverse.
fn apply_op(op: &Op, stash: &Stash) -> std::io::Result<Op> {
    match op {
        Op::Write { path, bytes } => {
            let old = match std::fs::read(path) {
                Ok(old) => Some(old),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(err),
            };
            std::fs::write(path, bytes)?;
            Ok(match old {
                Some(bytes) => Op::Write { path: path.clone(), bytes },
                None => Op::Remove { path: path.clone() },
            })
        }
        Op::Copy { from, to } => {
            if to.exists() {
                return Err(exists_error(to));
            }
            std::fs::copy(from, to)?;
            Ok(Op::Remove { path: to.clone() })
        }
        Op::CreateDir { path } => {
            if path.exists() {
                return Err(exists_error(path));
            }
            std::fs::create_dir(path)?;
            Ok(Op::Remove { path: path.clone() })
        }
        Op::Move { from, to } => {
            move_item(from, to)?;
            Ok(Op::Move { from: to.clone(), to: from.clone() })
        }
        Op::Remove { path } => {
            let stashed = stash.slot(path)?;
            move_item(path, &stashed)?;
            Ok(Op::Move { from: stashed, to: path.clone() })
        }
    }
}

/// Applies an edit's operations in order; the edit that undoes it. When an
/// operation fails, the ones before it are undone and the error returned.
pub fn apply(edit: &Edit, stash: &Stash) -> Result<Edit, String> {
    let mut inverse = vec![];
    for op in &edit.ops {
        match apply_op(op, stash) {
            Ok(undo) => inverse.push(undo),
            Err(err) => {
                for undo in inverse.iter().rev() {
                    if let Err(rollback) = apply_op(undo, stash) {
                        log::warn!("couldn’t roll back an asset catalog edit: {rollback}");
                    }
                }
                return Err(err.to_string());
            }
        }
    }
    inverse.reverse();
    Ok(Edit { name: edit.name.clone(), ops: inverse, select: None })
}

/// Where a file dropped into `set_dir` goes: its own name, or `name 1.png`,
/// `name 2.png`… when that's taken (by a file other slots keep).
pub fn destination_name(set_dir: &Path, file: &Path) -> String {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    if !set_dir.join(&name).exists() {
        return name;
    }
    let stem = file.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..).map(|n| format!("{stem} {n}{ext}")).find(|n| !set_dir.join(n).exists()).unwrap_or(name)
}

/// The scale a file's name asks for (`icon@2x.png` → 2x) and its name
/// without it, as Xcode reads dropped files.
pub fn scale_of(file: &Path) -> (String, Option<String>) {
    let stem = file.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for scale in ["@3x", "@2x", "@1x"] {
        if let Some(base) = stem.strip_suffix(scale) {
            return (base.to_string(), Some(scale[1..].to_string()));
        }
    }
    (stem, None)
}

/// Whether a dropped file is a vector image (a single-scale slot).
pub fn is_vector(file: &Path) -> bool {
    file.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "pdf" | "svg"))
}

/// Files Xcode takes into image wells.
pub fn is_image(file: &Path) -> bool {
    file.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "heic" | "heif" | "gif" | "tif" | "tiff" | "bmp" | "webp" | "pdf" | "svg" | "icns" | "ico" | "jp2" | "exr" | "dng"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zcode-xcassets-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_edit_and_its_undo_and_redo() {
        let dir = temp("undo");
        let stash = Stash::new(dir.join("stash"));
        let set = dir.join("Image.imageset");
        let image = dir.join("photo.png");
        std::fs::write(&image, b"png").unwrap();
        let edit = Edit {
            name: "New Image Set".into(),
            ops: vec![
                Op::CreateDir { path: set.clone() },
                Op::Copy { from: image.clone(), to: set.join("photo.png") },
                Op::write_contents(&set, &serde_json::json!({ "images": [{ "filename": "photo.png", "idiom": "universal" }], "info": super::super::json::info() })),
            ],
            select: None,
        };
        let undo = apply(&edit, &stash).unwrap();
        assert_eq!(std::fs::read(set.join("photo.png")).unwrap(), b"png");
        assert!(std::fs::read_to_string(set.join("Contents.json")).unwrap().contains("\"filename\" : \"photo.png\""));
        let redo = apply(&undo, &stash).unwrap();
        assert!(!set.exists(), "undone");
        apply(&redo, &stash).unwrap();
        assert!(set.join("photo.png").exists() && set.join("Contents.json").exists(), "redone");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_edit_is_rolled_back() {
        let dir = temp("rollback");
        let stash = Stash::new(dir.join("stash"));
        let a = dir.join("A.colorset");
        std::fs::create_dir(&a).unwrap();
        std::fs::write(a.join("Contents.json"), b"{}").unwrap();
        let edit = Edit {
            name: "Rename".into(),
            ops: vec![Op::Write { path: a.join("Contents.json"), bytes: b"new".to_vec() }, Op::CreateDir { path: a.clone() }],
            select: None,
        };
        assert!(apply(&edit, &stash).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read(a.join("Contents.json")).unwrap(), b"{}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dropped_file_names() {
        assert_eq!(scale_of(Path::new("/x/icon@2x.png")), ("icon".to_string(), Some("2x".to_string())));
        assert_eq!(scale_of(Path::new("/x/icon.png")), ("icon".to_string(), None));
        let dir = temp("names");
        std::fs::write(dir.join("a.png"), b"").unwrap();
        assert_eq!(destination_name(&dir, Path::new("/elsewhere/a.png")), "a 1.png");
        assert_eq!(destination_name(&dir, Path::new("/elsewhere/b.png")), "b.png");
        assert!(is_vector(Path::new("x.PDF")) && !is_vector(Path::new("x.png")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

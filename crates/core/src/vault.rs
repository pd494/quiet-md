//! The tree of a folder of Markdown notes (an Obsidian vault), which
//! `store` uses as the library.

use std::path::{Path, PathBuf};

pub struct Vault {
    root: PathBuf,
}

/// A row of the tree, in display order.
#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Folder,
    Note,
    /// Only in iCloud: `path` is the `.name.md.icloud` placeholder.
    Evicted,
}

fn note_name(file: &str) -> Option<(&str, ItemKind)> {
    if let Some(name) = file.strip_prefix('.').and_then(|f| f.strip_suffix(".icloud")) {
        return name.strip_suffix(".md").map(|n| (n, ItemKind::Evicted));
    }
    file.strip_suffix(".md").filter(|_| !file.starts_with('.')).map(|n| (n, ItemKind::Note))
}

impl Vault {
    pub fn open(path: &Path) -> std::io::Result<Vault> {
        let root = path.canonicalize()?;
        if !root.is_dir() { return Err(std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a folder")); }
        Ok(Vault { root })
    }

    pub fn root(&self) -> &Path { &self.root }

    /// Folders first, then notes, by name; hidden folders (`.obsidian`) are skipped.
    pub fn scan(&self) -> Vec<Item> {
        let mut out = vec![];
        walk(&self.root, 0, &mut out);
        out
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut folders = vec![];
    let mut notes = vec![];
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if !file.starts_with('.') { folders.push((file, path)); }
        } else if let Some((name, kind)) = note_name(&file) {
            notes.push(Item { path, name: name.to_string(), depth, kind });
        }
    }
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    notes.sort_by_key(|n| n.name.to_lowercase());
    for (name, path) in folders {
        out.push(Item { path: path.clone(), name, depth, kind: ItemKind::Folder });
        walk(&path, depth + 1, out);
    }
    out.extend(notes);
}

#[cfg(test)]
mod tests {
    use super::{ItemKind, Vault};

    #[test]
    fn scans_folders_first_and_skips_hidden_files() {
        let dir = std::env::temp_dir().join(format!("fdl-vault-{}", uuid::Uuid::new_v4()));
        for d in ["books", ".obsidian", "Writing/drafts"] { std::fs::create_dir_all(dir.join(d)).unwrap(); }
        for f in ["b.md", "A.md", "books/Dune.md", ".obsidian/app.json", "Writing/drafts/x.md", "notes.txt", ".DS_Store", ".quiet-md-1.tmp", "books/.Evicted.md.icloud"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let vault = Vault::open(&dir).unwrap();
        let rows: Vec<_> = vault.scan().into_iter().map(|i| (i.name, i.depth, i.kind)).collect();
        assert!(rows == vec![
            ("books".to_string(), 0, ItemKind::Folder),
            ("Dune".to_string(), 1, ItemKind::Note),
            ("Evicted".to_string(), 1, ItemKind::Evicted),
            ("Writing".to_string(), 0, ItemKind::Folder),
            ("drafts".to_string(), 1, ItemKind::Folder),
            ("x".to_string(), 2, ItemKind::Note),
            ("A".to_string(), 0, ItemKind::Note),
            ("b".to_string(), 0, ItemKind::Note),
        ]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

//! The files of a folder of Markdown notes (an Obsidian vault), which
//! `store` uses as the library.

use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
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

/// Every folder (before what it holds) and note; hidden folders (`.obsidian`)
/// are skipped.
pub fn scan(dir: &Path) -> Vec<Item> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if file.starts_with('.') { continue; }
            out.push(Item { path: path.clone(), name: file, kind: ItemKind::Folder });
            out.extend(scan(&path));
        } else if let Some((name, kind)) = note_name(&file) {
            out.push(Item { path, name: name.to_string(), kind });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ItemKind, scan};

    #[test]
    fn scans_notes_and_folders_and_skips_hidden_files() {
        let dir = std::env::temp_dir().join(format!("fdl-vault-{}", uuid::Uuid::new_v4()));
        for d in ["books", ".obsidian", "Writing/drafts"] { std::fs::create_dir_all(dir.join(d)).unwrap(); }
        for f in ["b.md", "A.md", "books/Dune.md", ".obsidian/app.json", "Writing/drafts/x.md", "notes.txt", ".DS_Store", ".quiet-md-1.tmp", "books/.Evicted.md.icloud"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let mut rows: Vec<_> = scan(&dir).into_iter().map(|i| (i.name, i.kind)).collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(rows == vec![
            ("A".to_string(), ItemKind::Note),
            ("Dune".to_string(), ItemKind::Note),
            ("Evicted".to_string(), ItemKind::Evicted),
            ("Writing".to_string(), ItemKind::Folder),
            ("b".to_string(), ItemKind::Note),
            ("books".to_string(), ItemKind::Folder),
            ("drafts".to_string(), ItemKind::Folder),
            ("x".to_string(), ItemKind::Note),
        ]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

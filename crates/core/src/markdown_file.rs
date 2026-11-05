//! Markdown file import and in-place export, shared by the library and local file windows.

use crate::model::{Block, BlockKind, Document, RichText};
use std::path::{Path, PathBuf};

/// Percent-escapes in a Markdown link (`%20` and friends).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Relative and `file://` image links of an imported file, made absolute.
fn resolve_images(blocks: &mut [Block], base: &Path) {
    for b in blocks.iter_mut() {
        resolve_images(&mut b.children, base);
        if let BlockKind::Table(t) = &mut b.kind {
            for cell in t.rows.iter_mut().flatten() {
                resolve_images(&mut cell.blocks, base);
            }
        }
        let BlockKind::Image { src, .. } = &mut b.kind else { continue };
        if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
            continue;
        }
        let path = match src.strip_prefix("file://") {
            Some(p) => PathBuf::from(percent_decode(p)),
            None => base.join(percent_decode(src)),
        };
        *src = path.to_string_lossy().to_string();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleFrom {
    /// The leading `# heading`, if any.
    FirstHeading,
    /// The file name; a leading `# heading` that repeats it is folded in.
    #[default]
    FileName,
}

/// Source text and the blocks it produced. `tail` is the verbatim text after
/// it: blank lines, and anything that produced no block.
#[derive(Clone)]
struct Segment {
    text: String,
    tail: String,
    blocks: Vec<Block>,
    /// Position in the file, to tell which segments were adjacent.
    ix: usize,
}

/// The file as read. A save writes back verbatim each segment whose blocks
/// are unchanged, so an edit rewrites only what was edited.
#[derive(Default)]
struct Layout {
    title_from: TitleFrom,
    /// Frontmatter and blank lines before the body.
    front: String,
    /// The `# heading` line the title came from, and that title.
    head: Option<String>,
    title: RichText,
    segments: Vec<Segment>,
    /// The file's nested-list indent and bullet, for what a save re-renders.
    list_indent: Option<String>,
    bullet: Option<char>,
}

/// The first nested list item's indent and the first bullet's marker.
fn list_style(text: &str) -> (Option<String>, Option<char>) {
    let item = |l: &str| l.trim_start().strip_prefix(['-', '*', '+']).is_some_and(|r| r.starts_with(' ') && !r.starts_with(" ["));
    let indent = text.lines().find(|l| l.starts_with([' ', '\t']) && item(l)).map(|l| l[..l.len() - l.trim_start().len()].to_string());
    let bullet = text.lines().find(|l| item(l)).and_then(|l| l.trim_start().chars().next());
    (indent, bullet)
}

/// Each unindented line starts a group, with the indented lines (and any
/// code fence) after it.
fn split_groups(text: &str) -> Vec<&str> {
    let (mut out, mut start, mut pos, mut fence) = (vec![], 0, 0, false);
    for line in text.split_inclusive('\n') {
        if pos > 0 && !fence && !line.starts_with([' ', '\t']) {
            out.push(&text[start..pos]);
            start = pos;
        }
        if is_fence(line) { fence = !fence; }
        pos += line.len();
    }
    if start < pos { out.push(&text[start..]); }
    out
}

/// Leading YAML frontmatter, verbatim, and the rest.
fn split_front(text: &str) -> (&str, &str) {
    let Some(first) = text.split_inclusive('\n').next() else { return ("", text) };
    if first.trim_end() != "---" { return ("", text); }
    let mut end = first.len();
    for line in text[first.len()..].split_inclusive('\n') {
        end += line.len();
        if matches!(line.trim_end(), "---" | "...") { return text.split_at(end); }
    }
    ("", text)
}

/// Length of the leading blank lines.
fn blank_prefix(text: &str) -> usize {
    text.split_inclusive('\n').take_while(|l| l.trim().is_empty()).map(str::len).sum()
}

/// Paragraph-sized pieces and the blank lines after each: a piece ends at
/// blank lines, unless inside a code fence or the next line is indented (it
/// continues a list item).
fn split_segments(text: &str) -> Vec<(&str, &str)> {
    let (mut out, mut start, mut end, mut pos, mut fence) = (vec![], 0, 0, 0, false);
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if end < pos && !t.is_empty() && !line.starts_with([' ', '\t']) {
            out.push((&text[start..end], &text[end..pos]));
            start = pos;
        }
        if is_fence(line) { fence = !fence; }
        pos += line.len();
        if !t.is_empty() || fence { end = pos; }
    }
    if start < pos { out.push((&text[start..end], &text[end..])); }
    out
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Blocks equal apart from their ids, which every parse makes anew.
fn same(a: &[Block], b: &[Block]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.kind == y.kind && x.text == y.text && x.align == y.align && x.bg == y.bg && x.folded == y.folded && same(&x.children, &y.children))
}

fn relative(blocks: &mut [Block], base: &Path) {
    for b in blocks {
        if let BlockKind::Image { src, .. } = &mut b.kind
            && let Ok(p) = Path::new(src).strip_prefix(base)
        { *src = p.to_string_lossy().replace('\\', "/"); }
        relative(&mut b.children, base);
        if let BlockKind::Table(t) = &mut b.kind { for cell in t.rows.iter_mut().flatten() { relative(&mut cell.blocks, base); } }
    }
}

fn parse(text: &str, path: &Path, title_from: TitleFrom) -> (Document, Layout) {
    let text = text.trim_start_matches('\u{feff}');
    let (front, body) = split_front(text);
    let (lead, mut body) = body.split_at(blank_prefix(body));
    let blocks_of = |src: &str| {
        let mut blocks = crate::paste::markdown_to_blocks(src, crate::paste::Off { file: true, ..Default::default() });
        if let Some(dir) = path.parent() { resolve_images(&mut blocks, dir); }
        blocks
    };
    let first = body.split_inclusive('\n').next().unwrap_or_default();
    let h1 = blocks_of(first).into_iter().next().filter(|b| matches!(b.kind, BlockKind::Heading { level: 1, .. }));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().trim();
    let title = match title_from {
        TitleFrom::FirstHeading => h1.as_ref().map(|b| b.text.clone()).unwrap_or_default(),
        TitleFrom::FileName => RichText::plain(stem),
    };
    let mut head = None;
    if let Some(h1) = h1 && (title_from == TitleFrom::FirstHeading || h1.text.text.trim() == stem) {
        let (h, rest) = body.split_at(first.len() + blank_prefix(&body[first.len()..]));
        head = Some(h.to_string());
        body = rest;
    }
    let (list_indent, bullet) = list_style(body);
    let mut layout = Layout { title_from, front: format!("{front}{lead}"), head, title: title.clone(), segments: vec![], list_indent, bullet };
    let mut push = |text: &str, tail: &str, blocks: Vec<Block>| {
        if !blocks.is_empty() {
            let ix = layout.segments.len();
            layout.segments.push(Segment { text: text.to_string(), tail: tail.to_string(), blocks, ix });
            return;
        }
        let orphan = format!("{text}{tail}");
        match (layout.segments.last_mut(), &mut layout.head) {
            (Some(last), _) => last.tail.push_str(&orphan),
            (None, Some(head)) => head.push_str(&orphan),
            (None, None) => layout.front.push_str(&orphan),
        }
    };
    for (src, tail) in split_segments(body) {
        let blocks = blocks_of(src);
        // Finer segments where each group of lines parses on its own to the
        // same blocks (lines of a paragraph, items of a list), so an edit
        // rewrites only its own lines.
        let groups = split_groups(src);
        if groups.len() > 1 {
            let parts: Vec<_> = groups.iter().map(|g| blocks_of(g)).collect();
            if parts.iter().all(|b| !b.is_empty()) && parts.iter().map(Vec::len).sum::<usize>() == blocks.len() {
                let last = groups.len() - 1;
                for (n, (group, blocks)) in groups.into_iter().zip(parts).enumerate() {
                    push(group, if n == last { tail } else { "" }, blocks);
                }
                continue;
            }
        }
        push(src, tail, blocks);
    }
    if layout.segments.is_empty() {
        layout.segments.push(Segment { text: String::new(), tail: String::new(), blocks: vec![Block::text("")], ix: 0 });
    }
    let blocks = layout.segments.iter().flat_map(|s| s.blocks.clone()).collect();
    (Document { title, icon: None, cover: None, blocks }, layout)
}

/// The file's new text, its title line and its segments now.
fn render(doc: &Document, layout: &Layout, base: &Path) -> (String, Option<String>, Vec<Segment>) {
    let export = |title: RichText, blocks: &[Block]| {
        let mut d = Document { title, icon: None, cover: None, blocks: blocks.to_vec() };
        relative(&mut d.blocks, base);
        let opts = crate::markdown::Options { assets: Some(""), preserve_paths: true, list_indent: layout.list_indent.as_deref(), bullet: layout.bullet, ..Default::default() };
        crate::markdown::export(&d, &opts).text
    };
    let head = match layout.title_from {
        TitleFrom::FirstHeading if doc.title != layout.title => (!doc.title.text.trim().is_empty()).then(|| export(doc.title.clone(), &[]) + "\n"),
        _ => layout.head.clone(),
    };
    let mut chunks: Vec<Segment> = vec![];
    // Edited blocks `start..end`, exported with their list numbers, take the
    // place and tail of the segment the last one came from.
    let flush = |start: usize, end: usize, chunks: &mut Vec<Segment>| {
        if start == end { return; }
        let mut run = doc.blocks[start..end].to_vec();
        let mut n = 0;
        for (k, b) in doc.blocks[..end].iter().enumerate() {
            n = match b.kind { BlockKind::Ordered { seq } => seq.unwrap_or(n + 1), _ => 0 };
            if let (true, BlockKind::Ordered { seq }) = (k >= start, &mut run[k.saturating_sub(start)].kind) { *seq = Some(n); }
        }
        let from = layout.segments.iter().find(|s| s.blocks.iter().any(|b| b.id == doc.blocks[end - 1].id));
        let (tail, ix) = from.map_or((String::new(), usize::MAX), |s| (s.tail.clone(), s.ix));
        chunks.push(Segment { text: export(RichText::default(), &run), tail, blocks: doc.blocks[start..end].to_vec(), ix });
    };
    let (mut i, mut start) = (0, 0);
    while i < doc.blocks.len() {
        let rest = &doc.blocks[i..];
        match layout.segments.iter().find(|s| rest.len() >= s.blocks.len() && same(&rest[..s.blocks.len()], &s.blocks)) {
            Some(s) => {
                flush(start, i, &mut chunks);
                chunks.push(Segment { blocks: rest[..s.blocks.len()].to_vec(), ..s.clone() });
                i += s.blocks.len();
                start = i;
            }
            None => i += 1,
        }
    }
    flush(start, i, &mut chunks);
    for n in 1..chunks.len() {
        let adjacent = chunks[n].ix == chunks[n - 1].ix.wrapping_add(1);
        let lists = chunks[n - 1].blocks.last().is_some_and(|b| b.kind.is_list()) && chunks[n].blocks.first().is_some_and(|b| b.kind.is_list());
        let c = &mut chunks[n - 1];
        if !c.text.is_empty() && !c.text.ends_with('\n') { c.text.push('\n'); }
        // Segments that were not adjacent in the file get a blank line
        // between, except list items, which stay one list as on export.
        if c.tail.is_empty() && !adjacent && !lists { c.tail.push('\n'); }
    }
    for (ix, c) in chunks.iter_mut().enumerate() { c.ix = ix; }
    let body: String = chunks.iter().flat_map(|c| [c.text.as_str(), c.tail.as_str()]).collect();
    (format!("{}{}{body}", layout.front, head.as_deref().unwrap_or_default()), head, chunks)
}

/// A new note holding `doc`, titled by its file name; never overwrites.
pub fn create(path: &Path, doc: &Document) -> std::io::Result<()> {
    use std::io::Write;
    let text = render(doc, &Layout::default(), path.parent().unwrap_or(Path::new("."))).0;
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(text.as_bytes())
}

/// Import: titled after the file.
pub fn read(path: &Path) -> std::io::Result<Document> {
    let bytes = std::fs::read(path)?;
    Ok(parse(&String::from_utf8_lossy(&bytes), path, TitleFrom::FileName).0)
}

pub enum SaveError {
    /// Another program wrote the file since it was read.
    Changed,
    Io(std::io::Error),
}

/// A Markdown file edited in place. A save checks the file still holds
/// `original`, so it never overwrites another program's write.
pub struct FileDoc {
    pub path: PathBuf,
    layout: Layout,
    original: Vec<u8>,
}

impl FileDoc {
    pub fn open(path: &Path, title_from: TitleFrom) -> std::io::Result<(FileDoc, Document)> {
        let original = std::fs::read(path)?;
        let text = std::str::from_utf8(&original).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let (doc, layout) = parse(text, path, title_from);
        Ok((FileDoc { path: path.to_path_buf(), layout, original }, doc))
    }

    pub fn changed_on_disk(&self) -> bool {
        std::fs::read(&self.path).map_or(true, |bytes| bytes != self.original)
    }

    pub fn save(&mut self, doc: &Document) -> Result<(), SaveError> {
        use std::io::Write;
        if self.changed_on_disk() { return Err(SaveError::Changed); }
        let (text, head, segments) = render(doc, &self.layout, self.path.parent().unwrap_or(Path::new(".")));
        let permissions = std::fs::metadata(&self.path).map_err(SaveError::Io)?.permissions();
        if permissions.readonly() { return Err(SaveError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "File is read-only"))); }
        let temp = self.path.with_file_name(format!(".quiet-md-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            file.write_all(text.as_bytes())?;
            file.set_permissions(permissions)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, &self.path)
        })();
        if result.is_err() { let _ = std::fs::remove_file(temp); }
        result.map_err(SaveError::Io)?;
        self.layout.head = head;
        self.layout.title = doc.title.clone();
        self.layout.segments = segments;
        self.original = text.into_bytes();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockKind, FileDoc, SaveError, TitleFrom, percent_decode, read};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fdl-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn imports_markdown_file() {
        let dir = temp("import");
        let path = dir.join("周报.md");
        std::fs::write(&path, "\u{feff}# 周报\n\n第一段\n\n- a\n- b\n\n![图](https://e.com/a.png)\n\n## 小结\n").unwrap();
        let doc = read(&path).unwrap();
        assert_eq!(doc.title.text, "周报");
        let kinds: Vec<_> = doc.blocks.iter().map(|b| std::mem::discriminant(&b.kind)).collect();
        assert_eq!(kinds.len(), 5);
        assert_eq!(doc.blocks[0].text.text, "第一段");
        assert!(matches!(doc.blocks[1].kind, BlockKind::Bullet));
        assert!(matches!(&doc.blocks[3].kind, BlockKind::Image { src, .. } if src == "https://e.com/a.png"));
        assert!(matches!(doc.blocks[4].kind, BlockKind::Heading { level: 2, .. }));
        assert_eq!(percent_decode("a%20b%E5%9B%BE.png"), "a b图.png");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn previews_mdx_without_changing_source_or_copying_images() {
        let dir = temp("preview");
        std::fs::create_dir_all(dir.join("images")).unwrap();
        let path = dir.join("本地预览.mdx");
        let image = dir.join("images/图 片.png");
        let source = "\u{feff}# 本地预览\r\n\r\n**正文**\r\n\r\n> ![图](images/%E5%9B%BE%20%E7%89%87.png)\r\n\r\n<Demo />\r\n";
        std::fs::write(&path, source).unwrap();
        std::fs::write(&image, b"preview image").unwrap();
        let (_, doc) = FileDoc::open(&path, TitleFrom::FirstHeading).unwrap();
        assert_eq!(doc.title.text, "本地预览");
        assert_eq!(doc.blocks[0].text.text, "正文");
        assert!(doc.blocks[0].text.runs.iter().any(|r| r.marks.bold));
        assert!(matches!(&doc.blocks[1].children[0].kind, BlockKind::Image { src, .. } if src.as_str() == image.to_string_lossy().as_ref()));
        assert!(doc.blocks.iter().any(|b| b.text.text == "<Demo />"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), source);
        assert_eq!(std::fs::read(image).unwrap(), b"preview image");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edits_local_markdown_and_keeps_relative_images() {
        use crate::editor::core::{Core, Pos, Selection};
        let dir = temp("edit");
        let path = dir.join("different-file-name.mdx");
        std::fs::write(&path, "# Actual title\n\nbody\n\n![image](photo.png)\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FirstHeading).unwrap();
        let mut core = Core::new(doc);
        assert_eq!(core.doc.title.text, "Actual title");
        core.sel = Selection::caret(Pos::new(core.doc.blocks[0].id, 0));
        core.insert_text("updated ");
        assert!(file.save(&core.doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Actual title\n\nupdated body\n\n![image](photo.png)\n");
        std::fs::write(&path, "body without a heading\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FirstHeading).unwrap();
        assert!(doc.title.text.is_empty());
        assert!(file.save(&doc).is_ok());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "body without a heading\n");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_an_obsidian_note_without_losing_its_markup() {
        let dir = temp("obsidian");
        let path = dir.join("Reading list.md");
        let source = "---\ntags: [books]\naliases:\n  - reads\n---\nSee [[Other note]] and [[Other note|that one]] #todo\n\n- one\n- [ ] two\n\n![[cover.png]]\n";
        std::fs::write(&path, source).unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        assert_eq!(doc.title.text, "Reading list");
        assert!(file.save(&doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_edit_rewrites_only_its_paragraph() {
        use crate::editor::core::{Core, Pos, Selection};
        let dir = temp("partial");
        let path = dir.join("note.md");
        std::fs::write(&path, "* star bullet\n\tnested with a tab  \n\nfirst line\nsecond line\n\nlast").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        let mut core = Core::new(doc);
        let last = core.doc.blocks.last().unwrap().id;
        core.sel = Selection::caret(Pos::new(last, 0));
        core.insert_text("the ");
        assert!(file.save(&core.doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "* star bullet\n\tnested with a tab  \n\nfirst line\nsecond line\n\nthe last\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_edit_keeps_the_files_list_style() {
        use crate::editor::core::{Core, Pos, Selection};
        let dir = temp("style");
        let path = dir.join("note.md");
        std::fs::write(&path, "* parent\n\t* child\n* other\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        let mut core = Core::new(doc);
        core.sel = Selection::caret(Pos::new(core.doc.blocks[0].children[0].id, 0));
        core.insert_text("the ");
        assert!(file.save(&core.doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "* parent\n\t* the child\n* other\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_edited_list_item_keeps_its_number() {
        use crate::editor::core::{Core, Pos, Selection};
        let dir = temp("ordered");
        let path = dir.join("note.md");
        std::fs::write(&path, "1. a\n2. b\n3. c\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        let mut core = Core::new(doc);
        core.sel = Selection::caret(Pos::new(core.doc.blocks[2].id, 0));
        core.insert_text("the ");
        assert!(file.save(&core.doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1. a\n2. b\n3. the c\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_heading_repeating_the_file_name_becomes_the_title_and_is_kept() {
        let dir = temp("heading");
        let path = dir.join("Plans.md");
        std::fs::write(&path, "# Plans\n\nbody\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        assert_eq!(doc.title.text, "Plans");
        assert_eq!(doc.blocks[0].text.text, "body");
        assert!(file.save(&doc).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Plans\n\nbody\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn does_not_overwrite_a_file_another_program_changed() {
        let dir = temp("conflict");
        let path = dir.join("note.md");
        std::fs::write(&path, "mine\n").unwrap();
        let (mut file, doc) = FileDoc::open(&path, TitleFrom::FileName).unwrap();
        assert!(!file.changed_on_disk());
        std::fs::write(&path, "from my phone\n").unwrap();
        assert!(file.changed_on_disk());
        assert!(matches!(file.save(&doc), Err(SaveError::Changed)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "from my phone\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

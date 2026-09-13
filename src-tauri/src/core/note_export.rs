//! Read-only Markdown export. All output belongs to a newly reserved root directory.
use crate::config::NotesSnapshot;
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteExportResult {
    pub output_path: String,
    pub folder_count: usize,
    pub note_count: usize,
}

// Leave room for extensions and collision suffixes on common filesystems.
const MAX_COMPONENT_BYTES: usize = 240;

fn truncate_utf8(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn sanitize_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = truncate_utf8(&cleaned, MAX_COMPONENT_BYTES)
        .trim_end_matches([' ', '.'])
        .to_string();
    if name.is_empty() {
        name = "Untitled".to_string();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    let numbered_device = ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
        name.insert(0, '_');
    }
    name
}

#[derive(Default)]
struct SiblingNames(HashSet<String>);

impl SiblingNames {
    fn unique(&mut self, title: &str, extension: &str) -> String {
        let base = sanitize_name(title);
        for index in 1_u64.. {
            let suffix = if index == 1 {
                String::new()
            } else {
                format!(" ({index})")
            };
            let stem = truncate_utf8(&base, MAX_COMPONENT_BYTES - extension.len() - suffix.len())
                .trim_end_matches([' ', '.']);
            let name = format!("{stem}{suffix}{extension}");
            if self.0.insert(name.to_uppercase()) {
                return name;
            }
        }
        unreachable!("exhausted filename suffixes")
    }
}

fn create_export_root(destination: &Path) -> io::Result<PathBuf> {
    let mut names = SiblingNames::default();
    for entry in fs::read_dir(destination)? {
        names
            .0
            .insert(entry?.file_name().to_string_lossy().to_uppercase());
    }
    loop {
        let root = destination.join(names.unique("NyaTerm Notes", ""));
        match fs::create_dir(&root) {
            Ok(()) => return Ok(root),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

pub fn export_notes(destination: &Path) -> AppResult<NoteExportResult> {
    let snapshot = crate::storage::load_notes_snapshot()?;
    export_snapshot(destination, &snapshot)
}

fn export_snapshot(destination: &Path, snapshot: &NotesSnapshot) -> AppResult<NoteExportResult> {
    let root = create_export_root(destination)?;
    let result = write_snapshot(&root, snapshot);
    if result.is_err() {
        // Only this operation's successfully created root is eligible for cleanup.
        if let Err(error) = fs::remove_dir_all(&root) {
            tracing::warn!("Failed to clean up incomplete notes export: {error}");
        }
    }
    result.map(|()| NoteExportResult {
        output_path: root.to_string_lossy().into_owned(),
        folder_count: snapshot.folders.len(),
        note_count: snapshot.notes.len(),
    })
}

fn write_snapshot(root: &Path, snapshot: &NotesSnapshot) -> AppResult<()> {
    let mut paths = HashMap::from([(None, root.to_path_buf())]);
    let mut names: HashMap<PathBuf, SiblingNames> = HashMap::new();
    let mut pending: Vec<_> = snapshot.folders.iter().collect();
    // Resolve parents iteratively: snapshot order is not necessarily parent-first.
    while !pending.is_empty() {
        let previous = pending.len();
        let mut unresolved = Vec::new();
        for folder in pending {
            let Some(parent) = paths.get(&folder.parent_id) else {
                unresolved.push(folder);
                continue;
            };
            if paths.contains_key(&Some(folder.id.clone())) {
                return Err(AppError::Config("Duplicate note folder ID".into()));
            }
            let name = names
                .entry(parent.clone())
                .or_default()
                .unique(&folder.name, "");
            let path = parent.join(name);
            fs::create_dir(&path)?;
            paths.insert(Some(folder.id.clone()), path);
        }
        if unresolved.len() == previous {
            return Err(AppError::Config("Invalid note folder hierarchy".into()));
        }
        pending = unresolved;
    }
    for note in &snapshot.notes {
        let parent = paths
            .get(&note.parent_id)
            .ok_or_else(|| AppError::Config("Missing note parent folder".into()))?;
        let name = names
            .entry(parent.clone())
            .or_default()
            .unique(&note.title, ".md");
        // Never truncate an existing file, even if one appears during the export.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(parent.join(name))?;
        file.write_all(note.markdown.as_bytes())?;
        file.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NoteDocument, NoteFolder};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("note-export-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn folder(id: &str, parent: Option<&str>, name: &str) -> NoteFolder {
        NoteFolder {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: name.into(),
            sort_order: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn note(parent: Option<&str>, title: &str, markdown: &str) -> NoteDocument {
        NoteDocument {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: parent.map(str::to_string),
            title: title.into(),
            markdown: markdown.into(),
            sort_order: 0,
            revision: 7,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn empty_library_creates_empty_root() {
        let temp = TestDirectory::new();
        let result = export_snapshot(&temp.0, &NotesSnapshot::default()).unwrap();
        assert_eq!((result.folder_count, result.note_count), (0, 0));
        assert_eq!(Path::new(&result.output_path), temp.0.join("NyaTerm Notes"));
        assert_eq!(fs::read_dir(result.output_path).unwrap().count(), 0);
    }

    #[test]
    fn exports_root_and_nested_notes_verbatim_without_changing_snapshot() {
        let temp = TestDirectory::new();
        let markdown = "# 中文 🐈\r\n\n---\n  text\t\n![image](./a.png)\n\0";
        let snapshot = NotesSnapshot {
            folders: vec![folder("b", Some("a"), "Child"), folder("a", None, "Parent")],
            notes: vec![
                note(None, "Root", markdown),
                note(Some("b"), "Nested", markdown),
            ],
        };
        let before = snapshot.clone();
        let result = export_snapshot(&temp.0, &snapshot).unwrap();
        assert_eq!((result.folder_count, result.note_count), (2, 2));
        let root = Path::new(&result.output_path);
        for path in ["Root.md", "Parent/Child/Nested.md"] {
            assert_eq!(fs::read(root.join(path)).unwrap(), markdown.as_bytes());
        }
        assert_eq!(snapshot, before);
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["noteCount"], 2);
        assert_eq!(json["folderCount"], 2);
        assert!(json["outputPath"].is_string());
    }

    #[test]
    fn sanitizes_illegal_control_trailing_and_empty_names() {
        assert_eq!(sanitize_name("<>:\"/\\|?*\0\n\u{7f}"), "____________");
        assert_eq!(sanitize_name("hello. . "), "hello");
        for value in ["", " ", ".", "..", "... "] {
            assert_eq!(sanitize_name(value), "Untitled");
        }
        assert_eq!(sanitize_name("../../outside"), ".._.._outside");
        assert_eq!(sanitize_name("C:\\outside"), "C__outside");
    }

    #[test]
    fn protects_windows_device_names_even_with_extensions() {
        let mut reserved = vec!["CON".to_string(), "prn".into(), "Aux".into(), "NUL".into()];
        for prefix in ["COM", "LPT"] {
            for number in 1..=9 {
                reserved.push(format!("{prefix}{number}"));
            }
        }
        for name in reserved {
            assert_eq!(sanitize_name(&name), format!("_{name}"));
            assert_eq!(
                sanitize_name(&format!("{name}.txt")),
                format!("_{name}.txt")
            );
        }
        assert_eq!(sanitize_name("COM10"), "COM10");
        assert_eq!(sanitize_name("LPT0"), "LPT0");
    }

    #[test]
    fn shares_case_insensitive_namespace_and_suffixes_after_sanitizing() {
        let mut names = SiblingNames::default();
        assert_eq!(names.unique("foo", ".md"), "foo.md");
        assert_eq!(names.unique("FOO", ".md"), "FOO (2).md");
        assert_eq!(names.unique("foo", ".md"), "foo (3).md");
        assert_eq!(names.unique("a/b", ".md"), "a_b.md");
        assert_eq!(names.unique("a?b", ".md"), "a_b (2).md");
        assert_eq!(names.unique("folder.md", ""), "folder.md");
        assert_eq!(names.unique("folder", ".md"), "folder (2).md");
        assert_eq!(names.unique("FOO.MD", ""), "FOO.MD (2)");
    }

    #[test]
    fn exports_collisions_and_long_utf8_components() {
        let temp = TestDirectory::new();
        let long = "猫🐈".repeat(100);
        let snapshot = NotesSnapshot {
            folders: vec![folder("f", None, "foo.md"), folder("long", None, &long)],
            notes: vec![
                note(None, "foo", "1"),
                note(None, "FOO", "2"),
                note(None, "a/b", "3"),
                note(None, "a?b", "4"),
                note(None, &long, "5"),
                note(None, &long, "6"),
            ],
        };
        let result = export_snapshot(&temp.0, &snapshot).unwrap();
        let root = Path::new(&result.output_path);
        assert!(root.join("foo.md").is_dir());
        for (name, content) in [
            ("foo (2).md", "1"),
            ("FOO (3).md", "2"),
            ("a_b.md", "3"),
            ("a_b (2).md", "4"),
        ] {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), content);
        }
        let entries: Vec<_> = fs::read_dir(root).unwrap().map(|e| e.unwrap()).collect();
        assert_eq!(entries.len(), 8);
        for entry in &entries {
            assert!(entry.file_name().to_str().unwrap().len() <= MAX_COMPONENT_BYTES);
        }
        assert!(
            entries
                .iter()
                .any(|e| e.file_name().to_str().unwrap().ends_with(" (2).md"))
        );
        assert!(
            entries
                .iter()
                .any(|e| fs::read_to_string(e.path()).ok().as_deref() == Some("6"))
        );
    }

    #[test]
    fn existing_roots_and_files_are_never_overwritten() {
        let temp = TestDirectory::new();
        let existing = temp.0.join("NyaTerm Notes");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep.md"), "keep").unwrap();
        fs::write(temp.0.join("NyaTerm Notes (2)"), "file").unwrap();
        let result = export_snapshot(&temp.0, &NotesSnapshot::default()).unwrap();
        assert_eq!(
            Path::new(&result.output_path),
            temp.0.join("NyaTerm Notes (3)")
        );
        assert_eq!(
            fs::read_to_string(existing.join("keep.md")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("NyaTerm Notes (2)")).unwrap(),
            "file"
        );
    }

    #[test]
    fn failure_after_writing_cleans_only_new_root() {
        let temp = TestDirectory::new();
        let existing = temp.0.join("NyaTerm Notes");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep"), "keep").unwrap();
        let snapshot = NotesSnapshot {
            folders: vec![folder("f", None, "Folder")],
            notes: vec![
                note(None, "Written", "partial"),
                note(Some("missing"), "Bad", ""),
            ],
        };
        assert!(export_snapshot(&temp.0, &snapshot).is_err());
        assert!(!temp.0.join("NyaTerm Notes (2)").exists());
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        assert_eq!(fs::read_to_string(existing.join("keep")).unwrap(), "keep");
    }

    #[test]
    fn rejects_cycles_and_missing_folder_parents_and_cleans_up() {
        let temp = TestDirectory::new();
        for folders in [
            vec![folder("f", Some("f"), "Cycle")],
            vec![folder("f", Some("missing"), "Orphan")],
        ] {
            assert!(
                export_snapshot(
                    &temp.0,
                    &NotesSnapshot {
                        folders,
                        notes: vec![]
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
        }
    }

    #[test]
    fn exclusive_note_creation_preserves_existing_file() {
        let temp = TestDirectory::new();
        fs::write(temp.0.join("foo.md"), "keep").unwrap();
        let snapshot = NotesSnapshot {
            folders: vec![],
            notes: vec![note(None, "foo", "replace")],
        };
        assert!(write_snapshot(&temp.0, &snapshot).is_err());
        assert_eq!(fs::read_to_string(temp.0.join("foo.md")).unwrap(), "keep");
    }
}

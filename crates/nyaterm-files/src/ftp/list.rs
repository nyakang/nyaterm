use std::time::SystemTime;

use suppaftp::list::{File as SuppaFile, ListParser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FtpEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtpEntry {
    pub name: String,
    pub path: String,
    pub kind: FtpEntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl FtpEntry {
    pub fn is_directory(&self) -> bool {
        self.kind == FtpEntryKind::Directory
    }
}

pub fn entries_from_lines(lines: &[String], parent: &str, machine_listing: bool) -> Vec<FtpEntry> {
    lines
        .iter()
        .filter_map(|line| {
            let file = if machine_listing {
                ListParser::parse_mlsd(&normalize_mlsd_line(line)).ok()
            } else {
                ListParser::parse_posix(line)
                    .ok()
                    .or_else(|| ListParser::parse_dos(line).ok())
            }?;
            Some(entry_from_file(&file, parent))
        })
        .filter(|entry| entry.name != "." && entry.name != "..")
        .collect()
}

pub fn entry_from_mlst_line(line: &str, path: &str) -> Option<FtpEntry> {
    let file = ListParser::parse_mlst(&normalize_mlsd_line(line)).ok()?;
    let mut entry = entry_from_file(&file, "/");
    entry.path = path.to_string();
    Some(entry)
}

/// suppaftp only understands `type=link`, while many servers use the RFC 3659
/// `type=OS.unix=symlink` or `type=OS.unix=slink` spellings.
fn normalize_mlsd_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    for needle in ["type=os.unix=symlink", "type=os.unix=slink"] {
        if let Some(index) = lower.find(needle) {
            let end = index + needle.len();
            let token_end = line[end..]
                .find(';')
                .map(|offset| end + offset)
                .unwrap_or(line.len());
            return format!("{}type=link{}", &line[..index], &line[token_end..]);
        }
    }
    line.to_string()
}

fn entry_from_file(file: &SuppaFile, parent: &str) -> FtpEntry {
    let kind = if file.is_directory() {
        FtpEntryKind::Directory
    } else if file.is_symlink() {
        FtpEntryKind::Symlink
    } else {
        FtpEntryKind::File
    };
    FtpEntry {
        name: file.name().to_string(),
        path: join_remote_path(parent, file.name()),
        kind,
        size: file.size() as u64,
        modified: Some(file.modified()),
    }
}

pub fn join_remote_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

pub fn parent_and_name(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", name)) => ("/".to_string(), name.to_string()),
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => ("/".to_string(), trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{FtpEntryKind, entries_from_lines, join_remote_path, parent_and_name};

    #[test]
    fn mlsd_entries_are_typed_and_joined() {
        let lines = vec![
            "type=dir;modify=20260101120000;UNIX.mode=0755; sub".to_string(),
            "type=file;size=12;modify=20260101120000; a.txt".to_string(),
            "type=OS.unix=symlink; link".to_string(),
        ];
        let entries = entries_from_lines(&lines, "/home", true);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "sub");
        assert_eq!(entries[0].path, "/home/sub");
        assert_eq!(entries[0].kind, FtpEntryKind::Directory);
        assert_eq!(entries[1].kind, FtpEntryKind::File);
        assert_eq!(entries[1].size, 12);
        assert_eq!(entries[2].kind, FtpEntryKind::Symlink);
    }

    #[test]
    fn posix_and_dos_listings_fall_back() {
        let lines = vec![
            "-rw-r--r-- 1 user group 12 Nov 5 2025 readme.txt".to_string(),
            "10-19-20  03:19PM <DIR> pub".to_string(),
        ];
        let entries = entries_from_lines(&lines, "/", false);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["readme.txt", "pub"]
        );
        assert_eq!(entries[1].kind, FtpEntryKind::Directory);
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(join_remote_path("", "a"), "/a");
        assert_eq!(join_remote_path("/", "a"), "/a");
        assert_eq!(join_remote_path("/home/", "a"), "/home/a");
        assert_eq!(parent_and_name("/a.txt"), ("/".into(), "a.txt".into()));
        assert_eq!(
            parent_and_name("/home/a.txt"),
            ("/home".into(), "a.txt".into())
        );
    }
}

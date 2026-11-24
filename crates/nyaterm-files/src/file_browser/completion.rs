//! Address-bar completion. Paths are data, never shell command fragments.
use crate::file_browser::FileBrowserService;
use crate::file_browser::listing::natural_compare;
use crate::sftp::{RemoteFilePath, SftpFileEntry, SftpTransferControl};
use std::time::{Duration, Instant};

pub const CANDIDATE_LIMIT: usize = 100;
pub const LINK_LIMIT: usize = 12;

#[derive(Clone, Copy)]
pub struct CompletionLimits {
    pub entries: usize,
    pub name_bytes: usize,
}

impl Default for CompletionLimits {
    fn default() -> Self {
        Self {
            entries: 20_000,
            name_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct CompletionListing {
    pub entries: Vec<SftpFileEntry>,
    pub complete: bool,
}

impl CompletionListing {
    pub fn byte_size(&self) -> usize {
        self.entries
            .iter()
            .map(|e| {
                e.name.len()
                    + e.path.len()
                    + e.raw_path_token.as_ref().map_or(0, String::len)
                    + std::mem::size_of::<SftpFileEntry>()
            })
            .sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathStyle {
    pub windows: bool,
    pub ignore_case: bool,
}

impl PathStyle {
    pub const POSIX: Self = Self {
        windows: false,
        ignore_case: false,
    };

    pub fn separator(self, ch: char) -> bool {
        ch == '/' || (self.windows && ch == '\\')
    }

    pub fn matches(self, name: &str, prefix: &str) -> bool {
        if self.ignore_case {
            name.to_lowercase().starts_with(&prefix.to_lowercase())
        } else {
            name.starts_with(prefix)
        }
    }

    pub fn directory_text(self, path: &RemoteFilePath) -> String {
        let mut text = path.display_path.clone();
        if !text.ends_with(|c| self.separator(c)) {
            text.push(if self.windows && !text.contains('/') {
                '\\'
            } else {
                '/'
            });
        }
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionQuery {
    pub parent: RemoteFilePath,
    pub prefix: String,
    pub style: PathStyle,
}

/// An accepted identity survives edits to its child segment, not edits to itself.
#[derive(Clone)]
pub struct PathAnchor {
    pub text: String,
    pub path: RemoteFilePath,
}

pub fn query_parts(text: &str, style: PathStyle) -> (&str, &str) {
    match text.rfind(|c| style.separator(c)) {
        Some(index) => (&text[..index + 1], &text[index + 1..]),
        None => ("", text),
    }
}

pub fn candidates(
    query: &CompletionQuery,
    listing: &CompletionListing,
    hidden: bool,
) -> Vec<SftpFileEntry> {
    let mut entries: Vec<_> = listing
        .entries
        .iter()
        .filter(|entry| entry.is_directory() && matches_entry(query, entry, hidden))
        .cloned()
        .collect();
    entries.sort_by(|a, b| {
        b.name
            .starts_with(&query.prefix)
            .cmp(&a.name.starts_with(&query.prefix))
            .then_with(|| natural_compare(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.identity_key().cmp(&b.identity_key()))
    });
    entries
}

pub fn matches_entry(query: &CompletionQuery, entry: &SftpFileEntry, hidden: bool) -> bool {
    entry.name != "."
        && entry.name != ".."
        && !entry.name.is_empty()
        && (hidden || query.prefix.starts_with('.') || !entry.name.starts_with('.'))
        && query.style.matches(&entry.name, &query.prefix)
}

pub(crate) fn check(control: &SftpTransferControl, deadline: Instant) -> anyhow::Result<()> {
    control.check_cancelled()?;
    anyhow::ensure!(Instant::now() < deadline, CompletionTimeout);
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("directory completion timed out")]
pub struct CompletionTimeout;

fn resolve_input(
    text: &str,
    current: &RemoteFilePath,
    home: &str,
    anchor: Option<&PathAnchor>,
    style: PathStyle,
    encode: impl Fn(&str) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<RemoteFilePath> {
    anyhow::ensure!(!text.contains('\0'), "invalid path");
    if let Some(anchor) = anchor {
        if text == anchor.text || text == anchor.path.display_path {
            return Ok(anchor.path.clone());
        }
        let prefix = style.directory_text(&RemoteFilePath::new(&anchor.text));
        if let Some(suffix) = text.strip_prefix(&prefix) {
            return join(&anchor.path, suffix, style, &encode);
        }
    }
    if text == current.display_path || text == style.directory_text(current) {
        return Ok(current.clone());
    }
    // Editing a raw path can query one of its ancestors. Display text alone
    // cannot reconstruct those bytes, even when the edited final segment is UTF-8.
    if !style.windows {
        let mut ancestor = current.clone();
        for _ in 0..128 {
            if ancestor.raw_path_token.is_none() {
                break;
            }
            let parent = ancestor.parent()?;
            if parent == ancestor {
                break;
            }
            if text == parent.display_path || text == style.directory_text(&parent) {
                return Ok(parent);
            }
            ancestor = parent;
        }
    }
    if text == "~"
        || text
            .strip_prefix('~')
            .is_some_and(|s| s.starts_with(|c| style.separator(c)))
    {
        anyhow::ensure!(!home.is_empty(), "home directory unavailable");
        return join(
            &RemoteFilePath::new(home),
            text.get(2..).unwrap_or(""),
            style,
            &encode,
        );
    }
    if style.windows {
        // Drive-relative paths depend on process-wide per-drive cwd; never guess it.
        if text.as_bytes().get(1) == Some(&b':') {
            anyhow::ensure!(
                text.get(2..)
                    .is_some_and(|s| s.starts_with(|c| style.separator(c))),
                "drive-relative path unavailable"
            );
            return Ok(RemoteFilePath::new(text));
        }
        if text.starts_with("\\\\") || text.starts_with("//") {
            return Ok(RemoteFilePath::new(text));
        }
        if text.starts_with(|c| style.separator(c)) {
            anyhow::ensure!(
                current.display_path.as_bytes().get(1) == Some(&b':'),
                "drive unavailable"
            );
            return Ok(RemoteFilePath::new(format!(
                "{}{text}",
                &current.display_path[..2]
            )));
        }
    } else if text.starts_with('/') {
        return Ok(RemoteFilePath::new(text));
    }
    join(current, text, style, &encode)
}

fn join(
    base: &RemoteFilePath,
    suffix: &str,
    style: PathStyle,
    encode: &impl Fn(&str) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<RemoteFilePath> {
    if suffix.is_empty() {
        return Ok(base.clone());
    }
    let display = format!("{}{suffix}", style.directory_text(base));
    if let Some(mut raw) = base.raw_path()? {
        anyhow::ensure!(!style.windows, "native raw path unavailable");
        if !raw.ends_with(b"/") {
            raw.push(b'/');
        }
        raw.extend(encode(suffix)?);
        Ok(RemoteFilePath::from_raw(display, &raw))
    } else {
        Ok(RemoteFilePath::new(display))
    }
}

impl FileBrowserService {
    pub fn completion_style(&self) -> PathStyle {
        if matches!(self, Self::Local(_)) {
            PathStyle {
                windows: cfg!(windows),
                ignore_case: cfg!(any(windows, target_os = "macos")),
            }
        } else {
            PathStyle::POSIX
        }
    }

    pub fn resolve_browser_input(
        &self,
        text: &str,
        current: &RemoteFilePath,
        home: &str,
        anchor: Option<&PathAnchor>,
    ) -> anyhow::Result<RemoteFilePath> {
        resolve_input(
            text,
            current,
            home,
            anchor,
            self.completion_style(),
            |text| match self {
                Self::Remote(service) => service.encode_completion_suffix(text),
                _ => Ok(text.as_bytes().to_vec()),
            },
        )
    }

    pub fn completion_query(
        &self,
        text: &str,
        current: &RemoteFilePath,
        home: &str,
        anchor: Option<&PathAnchor>,
    ) -> anyhow::Result<CompletionQuery> {
        let style = self.completion_style();
        let (parent, prefix) = query_parts(text, style);
        Ok(CompletionQuery {
            parent: self.resolve_browser_input(parent, current, home, anchor)?,
            prefix: prefix.into(),
            style,
        })
    }

    pub fn list_completion_directory(
        &self,
        path: &RemoteFilePath,
        limits: CompletionLimits,
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<CompletionListing> {
        check(&control, deadline)?;
        match self {
            Self::Local(_) => list_local(path, limits, &control, deadline),
            Self::Remote(service) => {
                service.list_completion_directory(path, limits, control, deadline)
            }
            Self::Adb(service) => {
                service.list_completion_directory(path, limits, &control, deadline)
            }
            Self::Ftp(service) => {
                service.list_completion_directory(path, limits, &control, deadline)
            }
        }
    }

    pub fn probe_completion_links(
        &self,
        query: &CompletionQuery,
        listing: &mut CompletionListing,
        hidden: bool,
        control: &SftpTransferControl,
        deadline: Instant,
    ) {
        if listing
            .entries
            .iter()
            .filter(|e| {
                e.is_symlink() && !e.symlink_target_is_directory && matches_entry(query, e, hidden)
            })
            .count()
            > LINK_LIMIT
        {
            listing.complete = false;
        }
        for entry in listing
            .entries
            .iter_mut()
            .filter(|e| {
                e.is_symlink() && !e.symlink_target_is_directory && matches_entry(query, e, hidden)
            })
            .take(LINK_LIMIT)
        {
            if check(control, deadline).is_err() {
                break;
            }
            let path = entry.remote_path();
            entry.symlink_target_is_directory = match self {
                Self::Local(_) => std::fs::metadata(&path.display_path).is_ok_and(|m| m.is_dir()),
                Self::Remote(service) => service
                    .completion_target_is_directory(&path, control.clone(), deadline)
                    .unwrap_or(false),
                Self::Adb(service) => service
                    .completion_target_is_directory(&path, control, deadline)
                    .unwrap_or(false),
                Self::Ftp(service) => service
                    .completion_target_is_directory(&path.display_path, control, deadline)
                    .unwrap_or(false),
            };
        }
    }
}

fn list_local(
    path: &RemoteFilePath,
    limits: CompletionLimits,
    control: &SftpTransferControl,
    deadline: Instant,
) -> anyhow::Result<CompletionListing> {
    use crate::sftp::SftpFileType;
    let mut result = CompletionListing {
        entries: Vec::new(),
        complete: true,
    };
    let mut bytes = 0usize;
    for (index, entry) in std::fs::read_dir(&path.display_path)?.enumerate() {
        check(control, deadline)?;
        if index >= limits.entries {
            result.complete = false;
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let entry_path = entry.path();
        let Some(display_path) = entry_path.to_str() else {
            continue;
        };
        bytes = bytes.saturating_add(name.len());
        if bytes > limits.name_bytes {
            result.complete = false;
            break;
        }
        let kind = entry.file_type()?;
        result.entries.push(SftpFileEntry {
            name: name.into(),
            path: display_path.into(),
            file_type: if kind.is_dir() {
                SftpFileType::Directory
            } else if kind.is_symlink() {
                SftpFileType::Symlink
            } else {
                SftpFileType::File
            },
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        });
    }
    Ok(result)
}

pub const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::{CompletionLimits, PathAnchor, PathStyle, candidates, query_parts, resolve_input};
    use crate::file_browser::FileBrowserService;
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    use std::time::{Duration, Instant};

    #[test]
    fn paths_keep_target_separators_home_relative_and_link_parent_semantics() {
        let current = RemoteFilePath::new("/work/link");
        let resolve = |s| {
            resolve_input(s, &current, "/home/nya", None, PathStyle::POSIX, |s| {
                Ok(s.as_bytes().to_vec())
            })
            .unwrap()
            .display_path
        };
        assert_eq!(resolve("../src"), "/work/link/../src");
        assert_eq!(resolve("~/文 件"), "/home/nya/文 件");
        assert_eq!(resolve("~\\folder"), "/work/link/~\\folder");
        assert_eq!(
            query_parts("/home/alice/", PathStyle::POSIX),
            ("/home/alice/", "")
        );
        assert_eq!(
            query_parts(
                r"D:\Recent\t",
                PathStyle {
                    windows: true,
                    ignore_case: true
                }
            ),
            (r"D:\Recent\", "t")
        );
        assert_eq!(
            query_parts(r"/a/back\slash", PathStyle::POSIX),
            ("/a/", r"back\slash")
        );
    }

    #[test]
    fn windows_roots_and_matching_do_not_depend_on_the_build_host() {
        let style = PathStyle {
            windows: true,
            ignore_case: true,
        };
        let current = RemoteFilePath::new(r"C:\Users\nya");
        for path in [
            r"D:\",
            r"D:\中文 folder",
            r"\\server\share\",
            r"\\server\share\中文 folder",
        ] {
            assert_eq!(
                resolve_input(path, &current, &current.display_path, None, style, |s| Ok(
                    s.as_bytes().to_vec()
                ))
                .unwrap()
                .display_path,
                path
            );
        }
        assert_eq!(
            resolve_input(
                r"~\中文 folder",
                &current,
                &current.display_path,
                None,
                style,
                |s| Ok(s.as_bytes().to_vec())
            )
            .unwrap()
            .display_path,
            r"C:\Users\nya\中文 folder"
        );
        assert!(style.matches("Trusted", "t"));
        assert!(!PathStyle::POSIX.matches("Trusted", "t"));
        assert!(
            resolve_input("D:relative", &current, "", None, style, |s| Ok(s
                .as_bytes()
                .to_vec()))
            .is_err()
        );
        assert!(
            resolve_input("~/dir", &current, "", None, style, |s| Ok(s
                .as_bytes()
                .to_vec()))
            .is_err()
        );
    }

    #[test]
    fn accepted_raw_identity_survives_child_input_but_not_edits_to_the_anchor() {
        let anchor = PathAnchor {
            text: "/bad/�/".into(),
            path: RemoteFilePath::from_raw("/bad/�", b"/bad/\xff"),
        };
        let resolve = |s| {
            resolve_input(
                s,
                &RemoteFilePath::new("/"),
                "",
                Some(&anchor),
                PathStyle::POSIX,
                |s| Ok(s.as_bytes().to_vec()),
            )
            .unwrap()
        };
        assert_eq!(
            resolve("/bad/�/next").raw_path().unwrap().unwrap(),
            b"/bad/\xff/next"
        );
        assert_eq!(
            resolve("/bad/�/").raw_path().unwrap().unwrap(),
            b"/bad/\xff"
        );
        assert!(resolve("/other/�/next").raw_path_token.is_none());
    }

    #[test]
    fn bounded_local_listing_filters_prefixes_and_hidden_directories() {
        let root = tempfile::tempdir().unwrap();
        for name in ["tmp", "trusted", ".hidden", "tmp2", "tmp10"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        std::fs::write(root.path().join("tmp-file"), []).unwrap();
        let service = FileBrowserService::local();
        let path = RemoteFilePath::new(root.path().to_str().unwrap());
        let listing = service
            .list_completion_directory(
                &path,
                CompletionLimits::default(),
                SftpTransferControl::new(),
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
        let query = service.completion_query("tmp", &path, "", None).unwrap();
        assert_eq!(
            candidates(&query, &listing, false)
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["tmp", "tmp2", "tmp10"]
        );
        let query = service.completion_query(".", &path, "", None).unwrap();
        assert_eq!(candidates(&query, &listing, false).len(), 1);
        let limited = service
            .list_completion_directory(
                &path,
                CompletionLimits {
                    entries: 1,
                    name_bytes: 100,
                },
                SftpTransferControl::new(),
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
        assert!(!limited.complete);
        assert_eq!(limited.entries.len(), 1);
    }
}

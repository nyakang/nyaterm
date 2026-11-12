//! Reads user-written completion spec files (`<config dir>/completions/*.json`).
//!
//! Everything here is blocking file I/O and runs only on the completion worker.
//! Reading is defensive: only regular files, hard size and count limits, and a
//! file that cannot be used is reported by name and reason, never by content.
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use nyaterm_core::completion::CompletionEngine;

pub(super) const MAX_SPEC_FILE_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_SPEC_FILES: usize = 64;
/// Bounds memory when many near-limit files are present.
pub(super) const MAX_SPEC_TOTAL_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct SpecEntry {
    path: PathBuf,
    name: OsString,
    len: u64,
    modified: Option<SystemTime>,
}

/// Identity of the spec directory contents; equal fingerprints mean no reload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Fingerprint(Vec<(OsString, u64, Option<SystemTime>)>);

impl Fingerprint {
    pub(super) fn of(entries: &[SpecEntry]) -> Self {
        Self(
            entries
                .iter()
                .map(|entry| (entry.name.clone(), entry.len, entry.modified))
                .collect(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    TooLarge,
    TooManyFiles,
    TotalTooLarge,
    Unreadable,
    Invalid,
}

impl SkipReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "file exceeds the size limit",
            Self::TooManyFiles => "too many spec files",
            Self::TotalTooLarge => "spec files exceed the total size limit",
            Self::Unreadable => "file could not be read",
            Self::Invalid => "not a valid completion spec",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct LoadedSpecs {
    /// `(file name, bytes)` in file-name order, so later names win merges.
    pub files: Vec<(String, Vec<u8>)>,
    pub skipped: Vec<(String, SkipReason)>,
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// Lists candidate files in name order. A missing or unreadable directory is
/// simply "no specs". Symlinks and other non-regular files are ignored, so a
/// link cannot point the reader at an arbitrary path or device.
pub(super) fn list_entries(dir: &Path) -> Vec<SpecEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<SpecEntry> = read_dir
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            (metadata.is_file() && is_json(&path)).then(|| SpecEntry {
                name: entry.file_name(),
                len: metadata.len(),
                modified: metadata.modified().ok(),
                path,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_SPEC_FILE_BYTES + 1)
        .read_to_end(&mut data)
        .ok()?;
    Some(data)
}

pub(super) fn load(entries: &[SpecEntry]) -> LoadedSpecs {
    let mut loaded = LoadedSpecs::default();
    let mut total = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        let name = entry.name.to_string_lossy().into_owned();
        if index >= MAX_SPEC_FILES {
            loaded.skipped.push((name, SkipReason::TooManyFiles));
            continue;
        }
        if entry.len > MAX_SPEC_FILE_BYTES {
            loaded.skipped.push((name, SkipReason::TooLarge));
            continue;
        }
        if total + entry.len > MAX_SPEC_TOTAL_BYTES {
            loaded.skipped.push((name, SkipReason::TotalTooLarge));
            continue;
        }
        match read_limited(&entry.path) {
            // The file may have grown between the listing and the read.
            Some(data) if data.len() as u64 > MAX_SPEC_FILE_BYTES => {
                loaded.skipped.push((name, SkipReason::TooLarge));
            }
            Some(data) => {
                total += data.len() as u64;
                loaded.files.push((name, data));
            }
            None => loaded.skipped.push((name, SkipReason::Unreadable)),
        }
    }
    loaded
}

/// How often a search re-checks the directory for edits.
const RECHECK_INTERVAL: Duration = Duration::from_secs(3);

/// Owned by the completion worker; applies the spec directory to its engine.
pub(super) struct UserSpecs {
    dir: Option<PathBuf>,
    fingerprint: Option<Fingerprint>,
    checked_at: Option<Instant>,
}

impl UserSpecs {
    pub(super) fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            fingerprint: None,
            checked_at: None,
        }
    }

    /// Cheap enough for the search path: one directory listing every few
    /// seconds, and file contents are only read when something changed.
    pub(super) fn refresh_if_due(&mut self, engine: &mut CompletionEngine) {
        if self
            .checked_at
            .is_some_and(|at| at.elapsed() < RECHECK_INTERVAL)
        {
            return;
        }
        self.refresh(engine);
    }

    /// Applies the directory when its fingerprint changed. Never panics into
    /// the worker: a failure leaves the previous catalog in place.
    pub(super) fn refresh(&mut self, engine: &mut CompletionEngine) {
        self.checked_at = Some(Instant::now());
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        let entries = list_entries(dir);
        let fingerprint = Fingerprint::of(&entries);
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            return;
        }
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut loaded = load(&entries);
            let datas: Vec<Vec<u8>> = loaded.files.iter().map(|(_, data)| data.clone()).collect();
            for index in engine.set_user_specs(&datas) {
                if let Some((name, _)) = loaded.files.get(index) {
                    loaded.skipped.push((name.clone(), SkipReason::Invalid));
                }
            }
            loaded
        }));
        // Remember the fingerprint even on failure so a broken file warns once
        // per edit instead of on every refresh.
        self.fingerprint = Some(fingerprint);
        match applied {
            Ok(loaded) => {
                for (name, reason) in &loaded.skipped {
                    tracing::warn!(file = %name, reason = reason.as_str(), "completion spec skipped");
                }
                if !loaded.files.is_empty() {
                    tracing::info!(
                        files = loaded.files.len().saturating_sub(
                            loaded
                                .skipped
                                .iter()
                                .filter(|(_, reason)| *reason == SkipReason::Invalid)
                                .count()
                        ),
                        "completion specs loaded"
                    );
                }
            }
            Err(_) => tracing::warn!("completion specs could not be applied"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const VALID: &str = r#"{"version":1,"roots":[{"token":"deploy","description":"ship"}]}"#;

    fn write(dir: &Path, name: &str, content: &[u8]) {
        let mut file = fs::File::create(dir.join(name)).unwrap();
        file.write_all(content).unwrap();
    }

    #[test]
    fn a_missing_directory_means_no_specs() {
        let dir = tempfile::tempdir().unwrap();
        let entries = list_entries(&dir.path().join("does-not-exist"));
        assert!(entries.is_empty());
        assert_eq!(Fingerprint::of(&entries), Fingerprint::default());
        assert!(load(&entries).files.is_empty());
    }

    #[test]
    fn only_regular_json_files_are_read_in_name_order() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.json", VALID.as_bytes());
        write(dir.path(), "a.JSON", VALID.as_bytes());
        write(dir.path(), "notes.txt", b"ignored");
        fs::create_dir(dir.path().join("sub.json")).unwrap();
        let loaded = load(&list_entries(dir.path()));
        let names: Vec<_> = loaded.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["a.JSON", "b.json"]);
        assert!(loaded.skipped.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.json", VALID.as_bytes());
        std::os::unix::fs::symlink(
            outside.path().join("secret.json"),
            dir.path().join("link.json"),
        )
        .unwrap();
        assert!(list_entries(dir.path()).is_empty());
    }

    #[test]
    fn an_oversized_file_is_skipped_without_blocking_the_others() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "good.json", VALID.as_bytes());
        write(
            dir.path(),
            "huge.json",
            &vec![b' '; MAX_SPEC_FILE_BYTES as usize + 1],
        );
        let loaded = load(&list_entries(dir.path()));
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(
            loaded.skipped,
            vec![("huge.json".to_string(), SkipReason::TooLarge)]
        );
    }

    #[test]
    fn files_beyond_the_count_limit_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..MAX_SPEC_FILES + 3 {
            write(dir.path(), &format!("spec-{n:03}.json"), VALID.as_bytes());
        }
        let loaded = load(&list_entries(dir.path()));
        assert_eq!(loaded.files.len(), MAX_SPEC_FILES);
        assert_eq!(loaded.skipped.len(), 3);
        assert!(
            loaded
                .skipped
                .iter()
                .all(|(_, reason)| *reason == SkipReason::TooManyFiles)
        );
    }

    #[test]
    fn the_total_size_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b' '; MAX_SPEC_FILE_BYTES as usize];
        for n in 0..(MAX_SPEC_TOTAL_BYTES / MAX_SPEC_FILE_BYTES) as usize + 2 {
            write(dir.path(), &format!("big-{n}.json"), &big);
        }
        let loaded = load(&list_entries(dir.path()));
        assert_eq!(
            loaded.files.len() as u64,
            MAX_SPEC_TOTAL_BYTES / MAX_SPEC_FILE_BYTES
        );
        assert!(
            loaded
                .skipped
                .iter()
                .all(|(_, reason)| *reason == SkipReason::TotalTooLarge)
        );
    }

    #[test]
    fn the_fingerprint_changes_when_a_file_is_added_or_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.json", VALID.as_bytes());
        let first = Fingerprint::of(&list_entries(dir.path()));
        assert_eq!(first, Fingerprint::of(&list_entries(dir.path())));
        write(dir.path(), "a.json", format!("{VALID} ").as_bytes());
        let rewritten = Fingerprint::of(&list_entries(dir.path()));
        assert_ne!(first, rewritten);
        write(dir.path(), "b.json", VALID.as_bytes());
        assert_ne!(rewritten, Fingerprint::of(&list_entries(dir.path())));
    }

    fn complete(engine: &mut CompletionEngine, input: &str) -> Vec<(String, String)> {
        use nyaterm_core::completion::TokenCompletionRequest;
        use nyaterm_core::completion::history::TokenHistoryIndex;
        use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
        let history = TokenHistoryIndex::default();
        engine
            .complete(TokenCompletionRequest {
                input,
                cursor: input.len(),
                dialect: ShellDialect::Bash,
                scope: CompletionScope::legacy(),
                history: &history,
                quick: &history,
                limit: 50,
                fill_policy: None,
            })
            .into_iter()
            .map(|c| (c.display, c.description))
            .collect()
    }

    #[test]
    fn the_worker_applies_edits_removals_and_bad_files_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = CompletionEngine::embedded();
        let mut specs = UserSpecs::new(Some(dir.path().to_path_buf()));

        specs.refresh(&mut engine);
        assert!(complete(&mut engine, "deploy ").is_empty());

        write(dir.path(), "10-good.json", br#"{"version":1,"roots":[{"token":"deploy","flags":[{"long":"force","description":"skip checks"}]}]}"#);
        write(dir.path(), "20-bad.json", b"{ not json");
        specs.refresh(&mut engine);
        let flags = complete(&mut engine, "deploy --");
        assert!(
            flags
                .iter()
                .any(|(text, desc)| text == "--force" && desc == "skip checks")
        );
        // The built-in catalog is untouched for other commands.
        assert!(
            complete(&mut engine, "git ch")
                .iter()
                .any(|(text, _)| text == "checkout")
        );

        // An unchanged directory is not re-read.
        let recorded = specs.fingerprint.clone();
        specs.refresh(&mut engine);
        assert_eq!(specs.fingerprint, recorded);

        // Removing the files restores the built-in catalog.
        fs::remove_file(dir.path().join("10-good.json")).unwrap();
        fs::remove_file(dir.path().join("20-bad.json")).unwrap();
        specs.refresh(&mut engine);
        assert!(complete(&mut engine, "deploy --").is_empty());
    }

    #[test]
    fn a_later_file_overrides_an_earlier_one_and_the_built_in_entry() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.json", br#"{"version":1,"roots":[{"token":"ls","flags":[{"long":"all","short":"a","description":"first"}]}]}"#);
        write(dir.path(), "b.json", br#"{"version":1,"roots":[{"token":"ls","flags":[{"long":"all","short":"a","description":"second"}]}]}"#);
        let mut engine = CompletionEngine::embedded();
        UserSpecs::new(Some(dir.path().to_path_buf())).refresh(&mut engine);
        let flags = complete(&mut engine, "ls --al");
        assert_eq!(
            flags
                .iter()
                .find(|(text, _)| text == "--all")
                .map(|(_, d)| d.as_str()),
            Some("second")
        );
    }

    #[test]
    fn without_a_directory_nothing_changes() {
        let mut engine = CompletionEngine::embedded();
        let mut specs = UserSpecs::new(None);
        specs.refresh(&mut engine);
        specs.refresh_if_due(&mut engine);
        assert!(
            complete(&mut engine, "git ch")
                .iter()
                .any(|(text, _)| text == "checkout")
        );
    }
}

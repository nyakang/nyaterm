//! Path candidates for the suggestion popup. Listing is blocking SFTP/local
//! I/O, so it runs on the blocking-job pool with a hard timeout and a short
//! cache; the UI thread only builds the request and publishes the result.
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use futures::future::{Either, select};
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::path::{
    PathEntry, PathQuery, path_candidates, path_query_for_input, resolve_directory,
};
use nyaterm_core::completion::types::{CompletionCandidate, CompletionScope, ShellDialect};
use nyaterm_files::file_browser::{FileBrowserBackendKind, FileBrowserService};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Directory listings are reused while typing a path segment by segment.
const LISTING_TTL: Duration = Duration::from_secs(4);
const LISTING_TIMEOUT: Duration = Duration::from_millis(700);
const MAX_CACHED_LISTINGS: usize = 48;
pub(super) const PATH_CANDIDATE_LIMIT: usize = 12;

type CacheKey = (String, Option<String>, String);
type Listing = Arc<Vec<PathEntry>>;

fn cache() -> &'static Mutex<HashMap<CacheKey, (Instant, Listing)>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, (Instant, Listing)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached(key: &CacheKey) -> Option<Listing> {
    let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get(key)
        .filter(|(at, _)| at.elapsed() < LISTING_TTL)
        .map(|(_, listing)| listing.clone())
}

fn store(key: CacheKey, listing: Listing) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (at, _)| at.elapsed() < LISTING_TTL);
    if cache.len() >= MAX_CACHED_LISTINGS {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), listing));
}

/// Everything the background task needs; contains no GPUI handles.
#[derive(Clone)]
pub(super) struct PathSource {
    service: FileBrowserService,
    query: PathQuery,
    session_id: String,
    cwd: Option<String>,
    jobs: BlockingJobScheduler,
}

impl PathSource {
    pub(super) fn query(&self) -> &PathQuery {
        &self.query
    }

    /// Returns candidates, or an empty list on timeout/error. Completion must
    /// never block typing, so every failure degrades to "no path suggestions".
    pub(super) async fn candidates(
        &self,
        input: &str,
        timer: impl std::future::Future<Output = ()>,
    ) -> Vec<CompletionCandidate> {
        let key = (
            self.session_id.clone(),
            self.cwd.clone(),
            self.query.dir_text.clone(),
        );
        let listing = match cached(&key) {
            Some(listing) => listing,
            None => {
                let service = self.service.clone();
                let query = self.query.clone();
                let cwd = self.cwd.clone();
                let task = self.jobs.submit_task("path-completion", move |cancel| {
                    let home = query
                        .dir_text
                        .starts_with('~')
                        .then(|| service.home_dir().ok())
                        .flatten();
                    let dir = resolve_directory(&query, cwd.as_deref(), home.as_deref())?;
                    if cancel.is_cancelled() {
                        return None;
                    }
                    let entries = service.list_dir(&dir).ok()?;
                    Some(
                        entries
                            .into_iter()
                            .map(|entry| PathEntry {
                                is_dir: entry.is_directory(),
                                size: entry.size,
                                name: entry.name,
                            })
                            .collect::<Vec<_>>(),
                    )
                });
                let Ok(task) = task else {
                    return Vec::new();
                };
                let Either::Left((Ok(Some(entries)), _)) =
                    select(Box::pin(task), Box::pin(timer)).await
                else {
                    return Vec::new();
                };
                let listing = Arc::new(entries);
                store(key, listing.clone());
                listing
            }
        };
        path_candidates(
            &self.query,
            &listing,
            input,
            &mut TextMatcher::default(),
            PATH_CANDIDATE_LIMIT,
        )
    }
}

pub(super) fn listing_timeout() -> Duration {
    LISTING_TIMEOUT
}

impl NyaTermApp {
    /// None when the token is not a path or the session cannot list files
    /// (Telnet/Serial, Android, disconnected SSH). Never connects by itself:
    /// the remote service is created lazily and shared with the file panel.
    pub(super) fn path_source_for_session(
        &mut self,
        session_id: &str,
        input: &str,
        dialect: ShellDialect,
    ) -> Option<PathSource> {
        let backend = self.session.file_browser_backend_for_session(session_id)?;
        // Windows consoles use drive letters and backslashes, and WSL guest
        // paths need translation, so only POSIX-style hosts are completed.
        if backend == FileBrowserBackendKind::Adb
            || (backend == FileBrowserBackendKind::Local && cfg!(windows))
            || self.session.is_disconnected(session_id)
        {
            return None;
        }
        let query = path_query_for_input(input, dialect, CompletionScope::legacy())?;
        let service = self.file_browser_service_for_session(session_id).ok()?;
        Some(PathSource {
            service,
            query,
            session_id: session_id.to_string(),
            cwd: self.session.cwd(session_id).map(str::to_owned),
            jobs: self.blocking_jobs.clone(),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nyaterm_core::completion::path::path_query_for_input;

    fn source(input: &str, dir: &std::path::Path) -> PathSource {
        PathSource {
            service: FileBrowserService::local(),
            query: path_query_for_input(input, ShellDialect::Bash, CompletionScope::legacy())
                .unwrap(),
            session_id: format!("test-{}", dir.display()),
            cwd: Some(dir.to_string_lossy().into_owned()),
            jobs: BlockingJobScheduler::new(),
        }
    }

    #[test]
    fn lists_the_working_directory_for_relative_paths_and_reuses_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("projects")).unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"hi").unwrap();
        let input = "cd pr";
        let source = source(input, dir.path());
        let first = futures::executor::block_on(source.candidates(input, std::future::pending()));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].target_line(input).unwrap(), "cd projects/");

        // A cached listing answers without the pool or a timer.
        std::fs::remove_dir(dir.path().join("projects")).unwrap();
        let again = futures::executor::block_on(source.candidates(input, std::future::pending()));
        assert_eq!(again.len(), 1);
    }

    #[test]
    fn a_missing_directory_degrades_to_no_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let input = "ls /definitely/not/here/x";
        let source = source(input, dir.path());
        assert!(
            futures::executor::block_on(source.candidates(input, std::future::pending()))
                .is_empty()
        );
    }
}

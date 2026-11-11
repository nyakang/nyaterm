//! Path candidates for the suggestion popup. Listing is blocking SFTP/local
//! I/O, so it runs on the blocking-job pool with a hard timeout and a short
//! cache; the UI thread only builds the request and publishes the result.
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::path::{
    PathEntry, PathQuery, path_candidates, path_query_for_input, resolve_directory,
};
use nyaterm_core::completion::types::{CompletionCandidate, CompletionScope, ShellDialect};
use nyaterm_files::file_browser::{FileBrowserBackendKind, FileBrowserService};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Directory listings are reused while typing a path segment by segment.
const LISTING_TTL: Duration = Duration::from_secs(4);
/// The first listing of a session may have to open the SFTP channel, which
/// costs about as much as the SSH login itself; later ones reuse it.
const COLD_LISTING_BUDGET: Duration = Duration::from_millis(2500);
const WARM_LISTING_BUDGET: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(30);
const MAX_CACHED_LISTINGS: usize = 48;
pub(super) const PATH_CANDIDATE_LIMIT: usize = 12;

type CacheKey = (String, Option<String>, String);
type Listing = Arc<Vec<PathEntry>>;

struct Shared {
    listings: HashMap<CacheKey, (Instant, Listing)>,
    in_flight: HashSet<CacheKey>,
    /// Sessions that completed one listing, so their channel is open.
    warm_sessions: HashSet<String>,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(|| {
        Mutex::new(Shared {
            listings: HashMap::new(),
            in_flight: HashSet::new(),
            warm_sessions: HashSet::new(),
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Shared> {
    shared().lock().unwrap_or_else(|e| e.into_inner())
}

fn cached(key: &CacheKey) -> Option<Listing> {
    lock()
        .listings
        .get(key)
        .filter(|(at, _)| at.elapsed() < LISTING_TTL)
        .map(|(_, listing)| listing.clone())
}

/// Called by the listing job itself, so a result that arrives after the
/// popup stopped waiting still serves the next keystroke.
fn finish(key: CacheKey, listing: Option<Listing>) {
    let mut shared = lock();
    shared.in_flight.remove(&key);
    let Some(listing) = listing else { return };
    shared.warm_sessions.insert(key.0.clone());
    shared
        .listings
        .retain(|_, (at, _)| at.elapsed() < LISTING_TTL);
    if shared.listings.len() >= MAX_CACHED_LISTINGS {
        shared.listings.clear();
    }
    shared.listings.insert(key, (Instant::now(), listing));
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

    fn start_listing(&self, key: CacheKey) -> bool {
        {
            let mut shared = lock();
            if !shared.in_flight.insert(key.clone()) {
                return true;
            }
        }
        let service = self.service.clone();
        let query = self.query.clone();
        let cwd = self.cwd.clone();
        let started = Instant::now();
        let job_key = key.clone();
        let task = self.jobs.submit_task("path-completion", move |_| {
            let home = query
                .dir_text
                .starts_with('~')
                .then(|| service.home_dir().ok())
                .flatten();
            let listing = resolve_directory(&query, cwd.as_deref(), home.as_deref())
                .and_then(|dir| match service.list_dir(&dir) {
                    Ok(entries) => Some(entries),
                    Err(error) => {
                        tracing::info!(%error, "path completion listing failed");
                        None
                    }
                })
                .map(|entries| {
                    Arc::new(
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
            tracing::debug!(
                entries = listing.as_ref().map(|l| l.len()),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "path completion listing finished"
            );
            finish(job_key, listing);
        });
        if task.is_err() {
            lock().in_flight.remove(&key);
            return false;
        }
        // The pool owns the job; its result goes through `finish`.
        drop(task);
        true
    }

    /// Returns candidates, or an empty list on timeout/error. Completion must
    /// never block typing, so every failure degrades to "no path suggestions".
    pub(super) async fn candidates<F, Fut>(&self, input: &str, sleep: F) -> Vec<CompletionCandidate>
    where
        F: Fn(Duration) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let key = (
            self.session_id.clone(),
            self.cwd.clone(),
            self.query.dir_text.clone(),
        );
        let listing = match cached(&key) {
            Some(listing) => listing,
            None => {
                if !self.start_listing(key.clone()) {
                    return Vec::new();
                }
                let budget = if lock().warm_sessions.contains(&self.session_id) {
                    WARM_LISTING_BUDGET
                } else {
                    COLD_LISTING_BUDGET
                };
                let started = Instant::now();
                let listing = loop {
                    if let Some(listing) = cached(&key) {
                        break Some(listing);
                    }
                    if !lock().in_flight.contains(&key) || started.elapsed() >= budget {
                        break None;
                    }
                    sleep(POLL_INTERVAL).await;
                };
                match listing {
                    Some(listing) => listing,
                    None => {
                        tracing::info!(
                            waited_ms = started.elapsed().as_millis() as u64,
                            "path completion listing not ready; it will be cached for the next keystroke"
                        );
                        return Vec::new();
                    }
                }
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
        let first =
            futures::executor::block_on(source.candidates(input, |_| std::future::ready(())));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].target_line(input).unwrap(), "cd projects/");

        // A cached listing answers without the pool or a timer.
        std::fs::remove_dir(dir.path().join("projects")).unwrap();
        let again =
            futures::executor::block_on(source.candidates(input, |_| std::future::ready(())));
        assert_eq!(again.len(), 1);
    }

    #[test]
    fn a_missing_directory_degrades_to_no_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let input = "ls /definitely/not/here/x";
        let source = source(input, dir.path());
        assert!(
            futures::executor::block_on(source.candidates(input, |_| std::future::ready(())))
                .is_empty()
        );
    }
}

//! Per-application bounded path queries; no filesystem work runs in GPUI updates.
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use crate::features::session::remote_files::FileAccessPurpose;
use futures::{
    FutureExt,
    channel::oneshot,
    future::{BoxFuture, Shared},
};
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::path::{
    MAX_LISTED_ENTRIES, PathEntry, PathQuery, path_candidates, path_query_for_input,
    resolve_directory,
};
use nyaterm_core::completion::types::{CompletionCandidate, CompletionScope, ShellDialect};
use nyaterm_files::file_browser::lightweight::DirectoryLimits;
use nyaterm_files::file_browser::{FileBrowserBackendKind, FileBrowserService};
use nyaterm_files::sftp::{SftpFileType, SftpTransferControl};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LISTING_TTL: Duration = Duration::from_secs(2);
const FAILURE_TTL: Duration = Duration::from_millis(500);
const LISTING_BUDGET: Duration = Duration::from_millis(2500);
const MAX_NAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHED_LISTINGS: usize = 48;
pub(super) const PATH_CANDIDATE_LIMIT: usize = 12;
type Listing = Arc<Vec<PathEntry>>;
type Answer = Shared<BoxFuture<'static, Option<Listing>>>;
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    session: String,
    context: String,
    directory: String,
}
struct Running {
    id: u64,
    key: CacheKey,
    control: SftpTransferControl,
    answer: Answer,
}
#[derive(Default)]
struct State {
    listings: HashMap<CacheKey, (Instant, Option<Listing>)>,
    running: HashMap<String, Running>,
    epochs: HashMap<String, Arc<()>>,
    next_id: u64,
}

#[derive(Default)]
pub(in crate::features::terminal) struct PathCompletionRuntime {
    state: Arc<Mutex<State>>,
    jobs: Option<BlockingJobScheduler>,
}
impl PathCompletionRuntime {
    pub(in crate::features::terminal) fn cancel_session(&self, session: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = state.running.get(session) {
            job.control.cancel();
        }
        state.listings.retain(|key, _| key.session != session);
        state.epochs.remove(session);
    }
    pub(in crate::features::terminal) fn begin_shutdown(
        &mut self,
    ) -> Option<impl FnOnce() + Send + 'static + use<>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.epochs.clear();
        for job in state.running.values() {
            job.control.cancel();
        }
        drop(state);
        self.jobs.take().map(|jobs| jobs.begin_shutdown())
    }
}
impl Drop for PathCompletionRuntime {
    fn drop(&mut self) {
        if let Some(cleanup) = self.begin_shutdown() {
            let _ = nyaterm_core::runtime::worker::spawn("path-completion-cleanup".into(), cleanup);
        }
    }
}

struct Claim {
    state: Arc<Mutex<State>>,
    key: CacheKey,
    id: u64,
    response: Option<oneshot::Sender<Option<Listing>>>,
    control: SftpTransferControl,
}
impl Claim {
    fn finish(&mut self, listing: Option<Listing>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .running
            .get(&self.key.session)
            .is_some_and(|job| job.id == self.id)
        {
            state.running.remove(&self.key.session);
            if !self.control.is_cancelled() {
                state.listings.retain(|_, (at, value)| {
                    at.elapsed()
                        < if value.is_some() {
                            LISTING_TTL
                        } else {
                            FAILURE_TTL
                        }
                });
                let bytes = |value: &Option<Listing>| {
                    value
                        .as_ref()
                        .map_or(0, |items| items.iter().map(|e| e.name.len()).sum::<usize>())
                };
                while state.listings.len() >= MAX_CACHED_LISTINGS
                    || state
                        .listings
                        .values()
                        .map(|(_, value)| bytes(value))
                        .sum::<usize>()
                        + bytes(&listing)
                        > MAX_CACHE_BYTES
                {
                    let Some(oldest) = state
                        .listings
                        .iter()
                        .min_by_key(|(_, (at, _))| *at)
                        .map(|(key, _)| key.clone())
                    else {
                        break;
                    };
                    state.listings.remove(&oldest);
                }
                state
                    .listings
                    .insert(self.key.clone(), (Instant::now(), listing.clone()));
            }
        }
        drop(state);
        if let Some(sender) = self.response.take() {
            let _ = sender.send(if self.control.is_cancelled() {
                None
            } else {
                listing
            });
        }
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if self.response.is_some() {
            self.finish(None);
        }
    }
}

#[derive(Clone)]
pub(super) struct PathSource {
    service: FileBrowserService,
    query: PathQuery,
    key: CacheKey,
    state: Arc<Mutex<State>>,
    jobs: BlockingJobScheduler,
    epoch: Arc<()>,
}
impl PathSource {
    pub(super) fn query(&self) -> &PathQuery {
        &self.query
    }
    pub(super) fn context(&self) -> &str {
        &self.key.context
    }
    async fn listing(&self) -> Option<Listing> {
        loop {
            let (wait, retry, claim) = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if !state
                    .epochs
                    .get(&self.key.session)
                    .is_some_and(|epoch| Arc::ptr_eq(epoch, &self.epoch))
                {
                    return None;
                }
                if let Some((at, result)) = state.listings.get(&self.key)
                    && at.elapsed()
                        < if result.is_some() {
                            LISTING_TTL
                        } else {
                            FAILURE_TTL
                        }
                {
                    return result.clone();
                }
                if let Some(running) = state.running.get(&self.key.session) {
                    let retry = running.key != self.key || running.control.is_cancelled();
                    if retry {
                        running.control.cancel();
                    }
                    (running.answer.clone(), retry, None)
                } else {
                    state.next_id = state.next_id.wrapping_add(1);
                    let id = state.next_id;
                    let control = SftpTransferControl::default();
                    let (sender, receiver) = oneshot::channel();
                    let answer = receiver
                        .map(|result| result.unwrap_or(None))
                        .boxed()
                        .shared();
                    state.running.insert(
                        self.key.session.clone(),
                        Running {
                            id,
                            key: self.key.clone(),
                            control: control.clone(),
                            answer: answer.clone(),
                        },
                    );
                    let claim = Claim {
                        state: self.state.clone(),
                        key: self.key.clone(),
                        id,
                        response: Some(sender),
                        control,
                    };
                    (answer, false, Some(claim))
                }
            };
            if let Some(mut claim) = claim {
                let service = self.service.clone();
                let directory = self.key.directory.clone();
                // Capture the guard before enqueueing: rejected and cancelled
                // queued closures release the claim even if never executed.
                let _ = self.jobs.submit_task("path-completion", move |cancel| {
                    if cancel.is_cancelled() {
                        claim.control.cancel();
                        return;
                    }
                    let listing = service
                        .list_directory_lightweight(
                            &directory,
                            DirectoryLimits {
                                entries: MAX_LISTED_ENTRIES,
                                name_bytes: MAX_NAME_BYTES,
                            },
                            claim.control.clone(),
                            Instant::now() + LISTING_BUDGET,
                        )
                        .ok()
                        .map(|listing| {
                            Arc::new(
                                listing
                                    .entries
                                    .into_iter()
                                    .map(|entry| PathEntry {
                                        name: entry.name,
                                        is_dir: entry.file_type == SftpFileType::Directory,
                                        size: entry.size,
                                    })
                                    .collect(),
                            )
                        });
                    claim.finish(listing);
                });
            }
            let result = wait.await;
            if !retry {
                return result;
            }
        }
    }
    pub(super) async fn candidates(&self, input: &str) -> Vec<CompletionCandidate> {
        let Some(listing) = self.listing().await else {
            return Vec::new();
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
    pub(super) fn path_source_for_session(
        &mut self,
        session_id: &str,
        input: &str,
        dialect: ShellDialect,
    ) -> Option<PathSource> {
        let backend = self.session.file_browser_backend_for_session(session_id)?;
        if backend == FileBrowserBackendKind::Adb
            || (backend == FileBrowserBackendKind::Local && cfg!(windows))
            || self.session.is_disconnected(session_id)
        {
            return None;
        }
        let context = self
            .terminal
            .editing
            .sessions
            .get(session_id)?
            .file_context
            .current()?
            .clone();
        let query = path_query_for_input(input, dialect, CompletionScope::legacy())?;
        let directory = resolve_directory(&query, Some(&context.cwd), Some(&context.home))?;
        let service = match backend {
            FileBrowserBackendKind::Local => FileBrowserService::local(),
            FileBrowserBackendKind::Remote => {
                let config = self.session.metadata(session_id)?.ssh_config.clone()?;
                FileBrowserService::remote(
                    self.remote_file_service_for_session(
                        session_id,
                        config,
                        FileAccessPurpose::Automation,
                    )
                    .ok()?,
                )
            }
            FileBrowserBackendKind::Adb => return None,
        };
        let runtime = &mut self.terminal.assist.path_completion;
        if runtime.jobs.is_none() {
            runtime.jobs = BlockingJobScheduler::with_limits(2, 8).ok();
        }
        let epoch = runtime
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .epochs
            .entry(session_id.into())
            .or_default()
            .clone();
        Some(PathSource {
            epoch,
            service,
            query,
            key: CacheKey {
                session: session_id.into(),
                context: context.cache_key(),
                directory,
            },
            state: runtime.state.clone(),
            jobs: runtime.jobs.as_ref()?.clone(),
        })
    }
}

#[cfg(test)]
mod tests;

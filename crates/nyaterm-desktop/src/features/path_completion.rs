//! Shared address-bar state and bounded worker; each editor owns one instance.
use crate::blocking_jobs::BlockingJobScheduler;
use futures::{
    FutureExt,
    channel::oneshot,
    future::{BoxFuture, Shared},
};
use gpui::{ScrollHandle, Task, point, px};
use nyaterm_file_browser_gpui::completion::{
    CompletionPresentation, CompletionStatus, PathCandidate,
};
use nyaterm_files::file_browser::FileBrowserService;
use nyaterm_files::file_browser::completion::{
    CANDIDATE_LIMIT, CompletionLimits, CompletionListing, CompletionQuery, PathAnchor, PathStyle,
    QUERY_TIMEOUT, candidates,
};
use nyaterm_files::sftp::{RemoteFilePath, SftpTransferControl};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Answer = Result<Arc<CompletionListing>, CompletionStatus>;
type Pending = Shared<BoxFuture<'static, Answer>>;

struct CacheEntry {
    path: RemoteFilePath,
    at: Instant,
    listing: Arc<CompletionListing>,
}
struct Running {
    id: u64,
    epoch: u64,
    query: CompletionQuery,
    control: SftpTransferControl,
    answer: Pending,
}
#[derive(Default)]
struct WorkerState {
    epoch: u64,
    serial: u64,
    running: Option<Running>,
    cache: VecDeque<CacheEntry>,
}

#[derive(Clone, Default)]
pub(in crate::features) struct CompletionWorker(Arc<Mutex<WorkerState>>);

impl CompletionWorker {
    pub fn invalidate(&self) {
        self.cancel();
    }

    fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.epoch = state.epoch.wrapping_add(1);
        state.cache.clear();
        if let Some(running) = &state.running {
            running.control.cancel();
        }
    }

    pub fn seed(&self, path: RemoteFilePath, entries: &[nyaterm_files::sftp::SftpFileEntry]) {
        let limits = CompletionLimits::default();
        let mut listing = CompletionListing {
            entries: Vec::new(),
            complete: true,
        };
        let mut bytes = 0usize;
        for entry in entries {
            bytes = bytes.saturating_add(entry.name.len());
            if listing.entries.len() >= limits.entries || bytes > limits.name_bytes {
                listing.complete = false;
                break;
            }
            listing.entries.push(entry.clone());
        }
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if listing.byte_size() <= 8 * 1024 * 1024 {
            state.cache.push_front(CacheEntry {
                path,
                at: Instant::now(),
                listing: Arc::new(listing),
            });
        }
    }

    pub fn request(
        &self,
        service: FileBrowserService,
        query: CompletionQuery,
        hidden: bool,
        jobs: BlockingJobScheduler,
    ) -> impl Future<Output = Answer> + Send + 'static {
        let state = self.0.clone();
        let epoch = state.lock().unwrap_or_else(|e| e.into_inner()).epoch;
        async move {
            loop {
                let (answer, retry, job) = {
                    let mut inner = state.lock().unwrap_or_else(|e| e.into_inner());
                    if inner.epoch != epoch {
                        return Err(CompletionStatus::Inactive);
                    }
                    if let Some(running) = &inner.running {
                        let retry = running.epoch != epoch
                            || running.query != query
                            || running.control.is_cancelled();
                        if running.query.parent != query.parent {
                            running.control.cancel();
                        }
                        (running.answer.clone(), retry, None)
                    } else {
                        let cached = inner
                            .cache
                            .iter()
                            .find(|e| {
                                e.path == query.parent && e.at.elapsed() < Duration::from_secs(2)
                            })
                            .map(|e| (e.listing.clone(), e.at));
                        inner.serial = inner.serial.wrapping_add(1);
                        let id = inner.serial;
                        let control = SftpTransferControl::new();
                        let (sender, receiver) = oneshot::channel();
                        let answer = receiver
                            .map(|r| r.unwrap_or(Err(CompletionStatus::Unavailable)))
                            .boxed()
                            .shared();
                        inner.running = Some(Running {
                            id,
                            epoch,
                            query: query.clone(),
                            control: control.clone(),
                            answer: answer.clone(),
                        });
                        let claim = Claim {
                            state: state.clone(),
                            id,
                            epoch,
                            path: query.parent.clone(),
                            control,
                            sender: Some(sender),
                            listed_at: cached.as_ref().map(|(_, at)| *at),
                        };
                        (answer, false, Some((claim, cached)))
                    }
                };
                if let Some((mut claim, cached)) = job {
                    let service = service.clone();
                    let query = query.clone();
                    let _ = jobs.submit_task("browser-path-completion", move |cancel| {
                        if cancel.is_cancelled() {
                            claim.control.cancel();
                            return;
                        }
                        let deadline = Instant::now() + QUERY_TIMEOUT;
                        let result = cached
                            .map(|(listing, _)| Ok((*listing).clone()))
                            .unwrap_or_else(|| {
                                service.list_completion_directory(
                                    &query.parent,
                                    CompletionLimits::default(),
                                    claim.control.clone(),
                                    deadline,
                                )
                            });
                        let result = result
                            .map(|mut listing| {
                                service.probe_completion_links(
                                    &query,
                                    &mut listing,
                                    hidden,
                                    &claim.control,
                                    deadline,
                                );
                                Arc::new(listing)
                            })
                            .map_err(|_| {
                                if Instant::now() >= deadline {
                                    CompletionStatus::TimedOut
                                } else {
                                    CompletionStatus::Unavailable
                                }
                            });
                        claim.finish(result);
                    });
                }
                let result = answer.await;
                if !retry {
                    return result;
                }
            }
        }
    }
}

struct Claim {
    state: Arc<Mutex<WorkerState>>,
    id: u64,
    epoch: u64,
    path: RemoteFilePath,
    control: SftpTransferControl,
    sender: Option<oneshot::Sender<Answer>>,
    listed_at: Option<Instant>,
}
impl Claim {
    fn finish(&mut self, result: Answer) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.running.as_ref().is_some_and(|r| r.id == self.id) {
            state.running = None;
        }
        if state.epoch == self.epoch
            && !self.control.is_cancelled()
            && let Ok(listing) = &result
        {
            state
                .cache
                .retain(|e| e.path != self.path && e.at.elapsed() < Duration::from_secs(2));
            if listing.byte_size() <= 8 * 1024 * 1024 {
                state.cache.push_front(CacheEntry {
                    path: self.path.clone(),
                    at: self.listed_at.unwrap_or_else(Instant::now),
                    listing: listing.clone(),
                });
                while state.cache.len() > 8
                    || state
                        .cache
                        .iter()
                        .map(|e| e.listing.byte_size())
                        .sum::<usize>()
                        > 8 * 1024 * 1024
                {
                    state.cache.pop_back();
                }
            }
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if self.sender.is_some() {
            self.finish(Err(CompletionStatus::Unavailable));
        }
    }
}

#[derive(Default)]
pub(in crate::features) struct PathCompletionState {
    pub presentation: CompletionPresentation,
    pub scroll: ScrollHandle,
    pub anchor: Option<PathAnchor>,
    pub worker: CompletionWorker,
    pub task: Option<Task<()>>,
    pub subscription: Option<gpui::Subscription>,
    pub generation: u64,
    pub edited: bool,
    pub suppressed: bool,
    pub style: Option<PathStyle>,
    pub source: Option<(
        crate::features::transfers::file_operations::FileOperationEndpoint,
        Option<u64>,
        u64,
    )>,
}

impl PathCompletionState {
    pub fn reset(&mut self) {
        self.worker.cancel();
        self.task = None;
        self.generation = self.generation.wrapping_add(1);
        self.presentation = CompletionPresentation::default();
        self.anchor = None;
        self.edited = false;
        self.suppressed = false;
        self.style = None;
        self.source = None;
    }
    pub fn begin(&mut self, path: RemoteFilePath) {
        self.reset();
        self.anchor = Some(PathAnchor {
            text: path.display_path.clone(),
            path,
        });
    }
    pub fn changed(&mut self) {
        self.edited = true;
        self.suppressed = false;
    }
    pub fn prepare(&mut self) -> u64 {
        self.task = None;
        self.generation = self.generation.wrapping_add(1);
        self.presentation = CompletionPresentation {
            status: CompletionStatus::Loading,
            ..Default::default()
        };
        self.scroll.set_offset(Default::default());
        self.generation
    }
    pub fn hide(&mut self) -> bool {
        let visible = self.presentation.visible();
        self.task = None;
        self.worker.cancel();
        self.generation = self.generation.wrapping_add(1);
        self.presentation = CompletionPresentation::default();
        self.suppressed = true;
        visible
    }
    pub fn apply(
        &mut self,
        generation: u64,
        query: &CompletionQuery,
        result: Answer,
        hidden: bool,
    ) -> bool {
        if self.generation != generation || self.suppressed {
            return false;
        }
        self.style = Some(query.style);
        self.presentation = match result {
            Ok(listing) => {
                let entries = candidates(query, &listing, hidden);
                CompletionPresentation {
                    status: CompletionStatus::Ready,
                    truncated: !listing.complete || entries.len() > CANDIDATE_LIMIT,
                    candidates: entries
                        .into_iter()
                        .take(CANDIDATE_LIMIT)
                        .map(|e| PathCandidate {
                            path: e.remote_path(),
                            label: e.name,
                        })
                        .collect(),
                    selected: None,
                }
            }
            Err(status) => CompletionPresentation {
                status,
                ..Default::default()
            },
        };
        true
    }
    pub fn move_selection(&mut self, down: bool) {
        let count = self.presentation.candidates.len();
        if count == 0 {
            return;
        }
        let index = match self.presentation.selected {
            None => {
                if down {
                    0
                } else {
                    count - 1
                }
            }
            Some(i) => {
                if down {
                    (i + 1).min(count - 1)
                } else {
                    i.saturating_sub(1)
                }
            }
        };
        self.presentation.selected = Some(index);
        self.scroll
            .set_offset(point(px(0.), px(-(index.saturating_sub(4) as f32) * 28.)));
    }
    pub fn accept(&mut self, index: usize) -> Option<String> {
        let candidate = self.presentation.candidates.get(index)?;
        let text = self.style?.directory_text(&candidate.path);
        self.anchor = Some(PathAnchor {
            text: text.clone(),
            path: candidate.path.clone(),
        });
        self.changed();
        Some(text)
    }
    pub fn selected_path(&self) -> Option<RemoteFilePath> {
        self.presentation
            .selected
            .and_then(|i| self.presentation.candidates.get(i))
            .map(|c| c.path.clone())
    }
}
impl Drop for PathCompletionState {
    fn drop(&mut self) {
        self.worker.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletionWorker, PathCompletionState};
    use crate::blocking_jobs::BlockingJobScheduler;
    use futures::FutureExt;
    use nyaterm_file_browser_gpui::completion::CompletionStatus;
    use nyaterm_files::file_browser::FileBrowserService;
    use nyaterm_files::file_browser::completion::{CompletionListing, CompletionQuery, PathStyle};
    use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
    use std::sync::Arc;

    fn listing() -> Arc<CompletionListing> {
        Arc::new(CompletionListing {
            complete: true,
            entries: vec![SftpFileEntry {
                name: "child".into(),
                path: "/base/child".into(),
                raw_path_token: RemoteFilePath::from_raw("", b"/base/child").raw_path_token,
                file_type: SftpFileType::Directory,
                size: None,
                permissions: None,
                owner: String::new(),
                group: String::new(),
                modified_at: None,
                symlink_target_is_directory: false,
            }],
        })
    }

    #[test]
    fn completion_rejects_stale_results_and_preserves_selected_identity() {
        let mut state = PathCompletionState::default();
        state.begin(RemoteFilePath::new("/base"));
        state.changed();
        let old = state.prepare();
        let current = state.prepare();
        let query = CompletionQuery {
            parent: RemoteFilePath::new("/base"),
            prefix: "ch".into(),
            style: PathStyle::POSIX,
        };
        assert!(!state.apply(old, &query, Ok(listing()), false));
        assert!(state.apply(current, &query, Ok(listing()), false));
        assert!(state.selected_path().is_none());
        state.move_selection(true);
        assert!(state.selected_path().unwrap().raw_path_token.is_some());
        assert_eq!(state.accept(0).unwrap(), "/base/child/");
        assert!(state.anchor.as_ref().unwrap().path.raw_path_token.is_some());
        assert!(state.hide());
        assert!(!state.apply(current, &query, Ok(listing()), false));
        assert!(!state.presentation.visible());
        state.changed();
        let generation = state.prepare();
        assert!(state.apply(generation, &query, Err(CompletionStatus::TimedOut), false));
        state.reset();
        assert!(state.anchor.is_none());
        assert!(!state.edited);
    }

    #[test]
    fn completion_merges_pending_requests_and_reuses_parent_listing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("child")).unwrap();
        let service = FileBrowserService::local();
        let current = RemoteFilePath::new(root.path().to_str().unwrap());
        let query = service.completion_query("ch", &current, "", None).unwrap();
        let worker = CompletionWorker::default();
        let jobs = BlockingJobScheduler::with_limits(1, 8).unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let blocked = jobs
            .submit_task("held-worker", move |_| wait.recv().unwrap())
            .unwrap();
        for _ in 0..20 {
            assert!(
                worker
                    .request(service.clone(), query.clone(), false, jobs.clone())
                    .now_or_never()
                    .is_none()
            );
        }
        assert_eq!(jobs.metrics().submitted, 2);
        release.send(()).unwrap();
        futures::executor::block_on(blocked).unwrap();
        let result = futures::executor::block_on(worker.request(
            service.clone(),
            query.clone(),
            false,
            jobs.clone(),
        ))
        .ok()
        .unwrap();
        assert_eq!(result.entries.len(), 1);
        std::fs::remove_dir(root.path().join("child")).unwrap();
        let mut next = query.clone();
        next.prefix = "chi".into();
        assert_eq!(
            futures::executor::block_on(worker.request(service.clone(), next, false, jobs.clone()))
                .ok()
                .unwrap()
                .entries
                .len(),
            1
        );
        let stale = worker.request(service.clone(), query.clone(), false, jobs.clone());
        worker.invalidate();
        assert!(matches!(
            futures::executor::block_on(stale),
            Err(CompletionStatus::Inactive)
        ));
        assert!(
            futures::executor::block_on(worker.request(service, query, false, jobs.clone()))
                .ok()
                .unwrap()
                .entries
                .is_empty()
        );
        jobs.shutdown();
    }
}

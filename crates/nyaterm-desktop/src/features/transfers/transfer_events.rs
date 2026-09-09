use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _, select_biased};
use gpui::{Context, Window};
use nyaterm_core::{AiAction, truncate_preview};
use nyaterm_transport::{
    SFTP_TRANSFER_CANCELLED, SftpFileEntry, SftpFileType, SftpTransferProgress,
};
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::NyaTermApp;
use crate::features::formatting::format_permissions_octal;
use crate::models::{
    AiPreparedRequest, NavItem, TransferBrowserChildrenMenuStatus, TransferBrowserPathMenuKind,
    TransferBrowserPathMenuState, TransferExternalSyncPromptState, TransferJobEvent,
    TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobStatus,
};

/// How long the event drain holds a progress batch open before entering GPUI.
///
/// A bounded UI coalescing cadence, not a display frame: it is about one frame at
/// 60Hz and rather less than one at 120 or 144Hz, and nothing here observes the
/// actual refresh rate. What it buys is the consumer-side bound that makes the UI
/// update rate independent of how many transfers are running -- the 50ms throttle
/// in `TransferProgressEventSender` is per job and cannot provide that. The value
/// is kept well under that 50ms so a lone transfer is no chunkier than before.
const TRANSFER_UI_COALESCE_WINDOW: Duration = Duration::from_millis(16);

type ExternalEditorSyncStart = (Option<String>, String, String, Option<String>, PathBuf);
type ExternalEditorWatchStart = (Option<String>, String, String, Option<String>, PathBuf);

use super::state::{TransferEditorSaveOutcome, TransferFeatureState};
use super::transfer_widgets::{format_file_size, format_transfer_progress};

#[derive(Clone)]
struct TransferBrowserEventSnapshot {
    remote_path: String,
    browser_path: String,
    home_dir: String,
    home_dir_pending: bool,
    entries: Arc<Vec<SftpFileEntry>>,
    loading: bool,
    error: Option<String>,
    status: String,
    history: VecDeque<String>,
    history_index: usize,
    visited_history: VecDeque<String>,
    selected_path: Option<String>,
    selected_paths: HashSet<String>,
}

/// Save/restore of everything a browser event may rewind.
///
/// Taking `&TransferFeatureState` rather than `&NyaTermApp` means a snapshot
/// can only capture and replay transfer state, which is what the callers
/// intend. The captured fields are unchanged.
impl TransferFeatureState {
    fn browser_event_snapshot(&self) -> TransferBrowserEventSnapshot {
        TransferBrowserEventSnapshot {
            remote_path: self.remote_path().to_string(),
            browser_path: self.browser.path.clone(),
            home_dir: self.browser.home_dir.clone(),
            home_dir_pending: self.browser.home_dir_pending,
            entries: self.browser.entries.clone(),
            loading: self.browser.loading,
            error: self.browser.error.clone(),
            status: self.browser.status.clone(),
            history: self.browser.history.clone(),
            history_index: self.browser.history_index,
            visited_history: self.browser.visited_history.clone(),
            selected_path: self.browser.selected_remote_path.clone(),
            selected_paths: self.browser.selected_remote_paths.clone(),
        }
    }

    fn restore_browser_event_snapshot(&mut self, snapshot: TransferBrowserEventSnapshot) {
        self.set_remote_path(snapshot.remote_path);
        self.browser.path = snapshot.browser_path;
        self.browser.home_dir = snapshot.home_dir;
        self.browser.home_dir_pending = snapshot.home_dir_pending;
        self.browser.entries = snapshot.entries;
        self.browser.loading = snapshot.loading;
        self.browser.error = snapshot.error;
        self.browser.status = snapshot.status;
        self.browser.history = snapshot.history;
        self.browser.history_index = snapshot.history_index;
        self.browser.visited_history = snapshot.visited_history;
        self.browser.selected_remote_path = snapshot.selected_path;
        self.browser.selected_remote_paths = snapshot.selected_paths;
    }
}

impl NyaTermApp {
    fn load_transfer_browser_event_session(&mut self, session_id: &str) {
        let Some(cache) = self.transfer.browser.session_cache.get(session_id).cloned() else {
            self.transfer.set_remote_path(".");
            self.transfer.browser.path = ".".to_string();
            self.transfer.browser.home_dir.clear();
            self.transfer.browser.home_dir_pending = false;
            self.transfer.browser.entries = Arc::new(Vec::new());
            self.transfer.browser.loading = false;
            self.transfer.browser.error = None;
            self.transfer.browser.status.clear();
            self.transfer.browser.history.clear();
            self.transfer.browser.history_index = 0;
            self.transfer.browser.visited_history.clear();
            self.transfer.browser.selected_remote_path = None;
            self.transfer.browser.selected_remote_paths.clear();
            self.transfer.browser.path_menu = None;
            return;
        };

        self.transfer.set_remote_path(cache.current_path.clone());
        self.transfer.browser.path = cache.current_path;
        self.transfer.browser.home_dir = cache.home_dir;
        self.transfer.browser.home_dir_pending = false;
        self.transfer.browser.entries = cache.entries;
        self.transfer.browser.loading = false;
        self.transfer.browser.error = None;
        self.transfer.browser.status.clear();
        self.transfer.browser.history = cache.history;
        self.transfer.browser.history_index = cache.history_index;
        self.transfer.browser.visited_history = cache.visited_history;
        self.transfer.browser.selected_remote_path = None;
        self.transfer.browser.selected_remote_paths.clear();
        self.transfer.browser.path_menu = None;
    }

    /// Deliver transfer job events -- start, progress, finish -- as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `try_recv_transfer_event`, so a progress update waited for the next tick
    /// and `runtime_quiet_tick_allowed` had to carry a `transfer` term to keep
    /// that wait short.
    ///
    /// `update_in` rather than `update`: the handler opens dialogs and external
    /// editor windows, so it needs the `Window` the app entity is rendered in.
    pub(in crate::features) fn start_transfer_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.transfer.take_transfer_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                // Progress is already throttled per job at the source, but each job
                // gets its own 50ms phase and those phases stagger, so N concurrent
                // transfers wake this loop ~20N times a second. Draining only what is
                // already queued does not help: each wake finds one event and an empty
                // tail. Holding the batch open for a bounded window is what makes the
                // UI rate independent of the job count.
                let mut batch = vec![first];
                if batch
                    .iter()
                    .all(|event| matches!(event.event, TransferJobEvent::Progress(_)))
                {
                    // One deadline, taken from the first event of the batch and never
                    // extended. Re-arming it per event would let a steady stream hold
                    // the batch open indefinitely.
                    let mut deadline = cx
                        .background_executor()
                        .timer(TRANSFER_UI_COALESCE_WINDOW)
                        .fuse();
                    loop {
                        // The deadline is polled FIRST, and that ordering is the whole
                        // termination argument. `select_biased!` takes branches in
                        // written order, so with the receiver first a producer that
                        // keeps it ready would never let the deadline be polled at all,
                        // and the batch would grow without bound. Deadline-first means
                        // that once it is ready it wins the very next poll; while it is
                        // still pending it falls through to the receiver, so nothing is
                        // lost by checking it first.
                        select_biased! {
                            _ = deadline => break,
                            queued = rx.next().fuse() => {
                                let Some(event) = queued else { break };
                                // Anything that is not progress ends the batch now.
                                // `Finished` carries the directory listing, the resolved
                                // home dir, the synced cwd and editor payloads, and it
                                // drives the follow-up jobs and dialogs those open;
                                // delaying a lifecycle transition to smooth a progress
                                // bar is the wrong trade.
                                let terminal =
                                    !matches!(event.event, TransferJobEvent::Progress(_));
                                batch.push(event);
                                if terminal {
                                    break;
                                }
                            }
                        }
                    }
                }

                if this
                    .update_in(cx, |this, window, cx| {
                        // One transaction, one notify, however many events it took.
                        #[cfg(test)]
                        this.transfer.note_ui_batch(batch.len());
                        let mut dirty = false;
                        for event in batch {
                            dirty |= this.apply_transfer_event(event, window, cx);
                        }
                        if dirty {
                            // Only the transfers panel. An app-wide notify here would
                            // repaint the whole shell twenty times a second for a
                            // progress bar, which is the pump this batching exists to
                            // remove; `set_snapshot` notifies the panel and nothing
                            // else. Events that change something outside the panel
                            // still notify the app from their own handlers.
                            this.flush_transfer_panel_snapshot(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Apply one job event, reporting whether the UI needs a repaint.
    fn apply_transfer_event(
        &mut self,
        event: TransferJobResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((job_index, mut job)) = self.transfer.take_transfer_job_for_event(&event.id)
        else {
            // The job was cancelled or removed while its worker was still running.
            return false;
        };
        let mut dirty = false;
        let event_id = event.id.clone();
        let job_session_id = job.session_id.clone();
        let navigation_job_key = matches!(
            &job.kind,
            TransferJobKind::ListDir { .. } | TransferJobKind::SyncCwd
        )
        .then(|| job_session_id.clone().unwrap_or_default());
        if transfer_navigation_job_is_stale(
            &self.transfer.browser.navigation_jobs,
            navigation_job_key.as_deref(),
            &event_id,
        ) {
            self.transfer.browser.pending_navigations.remove(&event_id);
            return false;
        }
        dirty |= transfer_event_needs_ui_refresh(
            self.session.active_id(),
            job_session_id.as_deref(),
            &event.event,
        );
        let inactive_browser_snapshot = job_session_id
            .as_deref()
            .filter(|session_id| self.session.active_id() != Some(*session_id))
            .filter(|_| transfer_event_needs_browser_context(&job.kind, &event.event))
            .map(|session_id| {
                let snapshot = self.transfer.browser_event_snapshot();
                self.load_transfer_browser_event_session(session_id);
                snapshot
            });
        let mut external_sync_to_start: Option<ExternalEditorSyncStart> = None;
        let mut external_watch_to_start: Option<ExternalEditorWatchStart> = None;
        let mut external_sync_prompt_to_open: Option<String> = None;
        let mut zmodem_upload_after_probe: Option<(String, Vec<PathBuf>)> = None;
        let mut open_after_create: Option<SftpFileEntry> = None;
        let mut browser_navigation_rollback = None;
        let mut browser_listing_completed = false;
        let mut remote_path_to_set = None;
        let mut sync_properties_inputs = false;
        let mut forget_properties_inputs = false;
        let event_failed = matches!(&event.event, TransferJobEvent::Finished(Err(_)));
        let event_succeeded = matches!(&event.event, TransferJobEvent::Finished(Ok(_)));
        let event_finished = event_failed || event_succeeded;
        let cleanup_internal_job = event_finished
            && !job.is_user_transfer()
            && (!matches!(&job.kind, TransferJobKind::OpenExternal { .. }) || event_failed);
        match event.event {
            TransferJobEvent::Started { detail } => {
                job.status = TransferJobStatus::Running;
                job.detail = detail;
                job.progress = None;
                job.summary = None;
            }
            TransferJobEvent::ExternalModified {
                remote_path,
                raw_path_token,
                local_path,
            } => {
                job.detail = format!("External edit changed {}", local_path.display());
                let watch_key = format!("{remote_path}\n{}", local_path.display());
                if self.transfer.external_sync_always_uploads(&watch_key) {
                    external_sync_to_start = Some((
                        job_session_id.clone(),
                        job.id.clone(),
                        remote_path.clone(),
                        raw_path_token.clone(),
                        local_path.clone(),
                    ));
                } else if let Some(session_id) = job_session_id.clone() {
                    let prompt_id = job.id.clone();
                    self.transfer.insert_external_sync_prompt(
                        prompt_id.clone(),
                        TransferExternalSyncPromptState {
                            session_id: Some(session_id),
                            job_id: job.id.clone(),
                            remote_path: remote_path.clone(),
                            raw_path_token,
                            local_path: local_path.clone(),
                        },
                    );
                    external_sync_prompt_to_open = Some(prompt_id);
                    self.shell
                        .set_status(format!("external edit changed: {}", local_path.display()));
                }
            }
            TransferJobEvent::Progress(progress) => {
                if job.status == TransferJobStatus::Running {
                    job.detail = format_transfer_progress(&progress);
                }
                job.progress = Some(progress);
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Entries(entries))) => {
                browser_listing_completed = true;
                let (listed_path, select_after) = match &job.kind {
                    TransferJobKind::ListDir {
                        remote_path,
                        select_after,
                    } => (remote_path.clone(), select_after.clone()),
                    _ => (self.transfer.browser.path.clone(), None),
                };
                job.status = TransferJobStatus::Completed;
                job.detail = format!("{} item(s)", entries.len());
                self.transfer.browser.path = listed_path;
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.loading = false;
                self.transfer.browser.error = None;
                self.transfer.browser.status = job.detail.clone();
                self.transfer
                    .browser
                    .selected_remote_paths
                    .retain(|identity| {
                        entries.iter().any(|entry| entry.matches_identity(identity))
                    });
                if let Some(select_after) = select_after
                    && entries.iter().any(|entry| entry.path == select_after)
                {
                    self.transfer.browser.selected_remote_path = Some(select_after.clone());
                    self.transfer.browser.selected_remote_paths.clear();
                    self.transfer
                        .browser
                        .selected_remote_paths
                        .insert(select_after.clone());
                    remote_path_to_set = Some(select_after);
                }
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.shell
                    .set_status(format!("remote file list completed: {}", job.detail));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::ChildEntries {
                remote_path,
                mut entries,
            })) => {
                entries.retain(|entry| {
                    entry.is_directory()
                        && entry.name != "."
                        && entry.name != ".."
                        && (self.settings.summary().ui_file_explorer_show_hidden_files
                            || !entry.name.starts_with('.'))
                });
                entries.sort_by(|left, right| {
                    left.name
                        .to_lowercase()
                        .cmp(&right.name.to_lowercase())
                        .then_with(|| left.name.cmp(&right.name))
                });
                job.status = TransferJobStatus::Completed;
                job.detail = if entries.len() == 1 {
                    "1 child directory".to_string()
                } else {
                    format!("{} child directories", entries.len())
                };
                job.entries = entries.clone();
                job.summary = None;
                job.progress = None;
                job.control = None;

                if job_session_id.as_deref() == self.session.active_id()
                    && let Some(TransferBrowserPathMenuState {
                        session_id,
                        kind:
                            TransferBrowserPathMenuKind::Children {
                                path,
                                request_id,
                                status,
                                ..
                            },
                        ..
                    }) = self.transfer.browser.path_menu.as_mut()
                    && *session_id == job_session_id
                    && path == &remote_path
                    && request_id.as_deref() == Some(event_id.as_str())
                {
                    *request_id = None;
                    *status = TransferBrowserChildrenMenuStatus::Ready(entries);
                }
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::HomeDir(home_dir))) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Home {home_dir}");
                self.transfer.browser.home_dir = home_dir.clone();
                self.transfer.browser.home_dir_pending = false;
                self.transfer.browser.loading = false;
                self.transfer.browser.error = None;
                self.transfer.browser.status =
                    format!("remote home resolved: {}", truncate_preview(&home_dir, 72));
                job.entries.clear();
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.shell.set_status("remote home resolved".to_string());
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::CwdSynced {
                remote_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Synced cwd {remote_path}");
                remote_path_to_set = Some(remote_path.clone());
                self.transfer.browser.list_scroll = gpui::UniformListScrollHandle::new();
                self.transfer.browser.horizontal_scroll = gpui::ScrollHandle::new();
                self.transfer.browser.path = remote_path;
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.loading = false;
                self.transfer.browser.error = None;
                self.transfer.browser.status =
                    format!("remote exec cwd · {} item(s)", entries.len());
                self.transfer.browser.selected_remote_path = None;
                self.transfer.browser.selected_remote_paths.clear();
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.shell
                    .set_status("remote cwd sync completed".to_string());
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Renamed {
                old_path,
                new_path,
                parent_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Renamed {old_path} -> {new_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(new_path.clone());
                self.transfer
                    .browser
                    .selected_remote_paths
                    .remove(&old_path);
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(new_path.clone());
                self.transfer.set_remote_path(new_path.clone());
                self.shell.set_status(format!(
                    "remote rename completed in {parent_path}: {new_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Moved {
                old_path,
                new_path,
                parent_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Moved {old_path} -> {new_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(new_path.clone());
                self.transfer
                    .browser
                    .selected_remote_paths
                    .remove(&old_path);
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(new_path.clone());
                self.transfer.set_remote_path(new_path.clone());
                self.shell.set_status(format!(
                    "remote move completed from {parent_path}: {new_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Deleted {
                remote_path,
                parent_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Deleted {remote_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                if self.transfer.browser.selected_remote_path.as_deref()
                    == Some(remote_path.as_str())
                {
                    self.transfer.browser.selected_remote_path = None;
                }
                self.transfer
                    .browser
                    .selected_remote_paths
                    .remove(&remote_path);
                self.shell.set_status(format!(
                    "remote delete completed in {parent_path}: {remote_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Sent {
                source_path,
                target_session_id,
                target_path,
                target_parent_path,
                bytes,
                used_local_staging,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("{} sent", format_file_size(Some(bytes)));
                // Keep the row's kind pointing at the resolved destination so a
                // subsequent read shows where it actually landed.
                job.kind = TransferJobKind::SendTo {
                    source_path: source_path.clone(),
                    target_session_id: target_session_id.clone(),
                    target_path: target_path.clone(),
                    target_parent_path: target_parent_path.clone(),
                };
                job.entries.clear();
                job.summary = None;
                job.progress = None;
                job.control = None;
                // Refresh the target session's cached listing in place if it is
                // showing the destination directory — never switch the active
                // session, and never touch the source browser.
                let refreshed = self.transfer.refresh_browser_session_cache_listing(
                    &target_session_id,
                    &target_parent_path,
                    entries,
                );
                let staging_note = if used_local_staging { " (staged)" } else { "" };
                self.transfer.browser.status = format!(
                    "sent {} to {}{}",
                    truncate_preview(&source_path, 48),
                    truncate_preview(&target_path, 48),
                    staging_note,
                );
                self.shell.set_status(format!(
                    "remote send completed: {source_path} -> {target_path}{}{}",
                    staging_note,
                    if refreshed {
                        " · target refreshed"
                    } else {
                        ""
                    }
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::CreatedDirectory {
                remote_path,
                parent_path,
                entries,
                open_after_create,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Created {remote_path}");
                self.transfer.browser.path = if open_after_create {
                    remote_path.clone()
                } else {
                    parent_path.clone()
                };
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(remote_path.clone());
                self.transfer.browser.selected_remote_paths.clear();
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(remote_path.clone());
                self.transfer.set_remote_path(remote_path.clone());
                self.shell.set_status(format!(
                    "remote directory created in {parent_path}: {remote_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::CreatedFile {
                remote_path,
                parent_path,
                entries,
                open_after_create: should_open,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Created {remote_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries.clone();
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(remote_path.clone());
                self.transfer.browser.selected_remote_paths.clear();
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(remote_path.clone());
                self.transfer.set_remote_path(remote_path.clone());
                self.shell.set_status(format!(
                    "remote file created in {parent_path}: {remote_path}"
                ));
                if should_open && job_session_id.as_deref() == self.session.active_id() {
                    open_after_create = Some(
                        entries
                            .iter()
                            .find(|entry| entry.path == remote_path)
                            .cloned()
                            .unwrap_or_else(|| SftpFileEntry {
                                name: remote_path
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or(remote_path.as_str())
                                    .to_string(),
                                path: remote_path.clone(),
                                file_type: SftpFileType::File,
                                size: Some(0),
                                permissions: None,
                                owner: String::new(),
                                group: String::new(),
                                modified_at: None,
                                raw_path_token: None,
                                symlink_target_is_directory: false,
                            }),
                    );
                }
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::CreatedSymlink {
                link_path,
                target_path,
                parent_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Linked {link_path} -> {target_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(link_path.clone());
                self.transfer.browser.selected_remote_paths.clear();
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(link_path.clone());
                self.transfer.set_remote_path(link_path.clone());
                self.shell.set_status(format!(
                    "remote symlink created in {parent_path}: {link_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::PropertiesLoaded {
                remote_path,
                properties,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Loaded properties for {remote_path}");
                job.summary = None;
                job.progress = None;
                job.control = None;
                let mode_value = properties
                    .permissions
                    .map(format_permissions_octal)
                    .unwrap_or_else(|| "0644".to_string());
                let owner_value = if properties.owner.is_empty() {
                    properties
                        .uid
                        .map(|uid| uid.to_string())
                        .unwrap_or_default()
                } else {
                    properties.owner.clone()
                };
                let group_value = if properties.group.is_empty() {
                    properties
                        .gid
                        .map(|gid| gid.to_string())
                        .unwrap_or_default()
                } else {
                    properties.group.clone()
                };
                sync_properties_inputs = self.transfer.complete_properties_load(
                    job_session_id.as_deref(),
                    &remote_path,
                    properties,
                    mode_value,
                    owner_value,
                    group_value,
                );
                self.transfer.browser.status = format!("properties loaded for {remote_path}");
                self.shell
                    .set_status(format!("remote properties loaded: {remote_path}"));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::PropertiesUpdated {
                remote_path,
                parent_path,
                properties,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Updated properties for {remote_path}");
                self.transfer.browser.path = parent_path.clone();
                self.transfer.browser.entries = Arc::new(entries.clone());
                self.transfer.browser.status = format!("{} item(s)", entries.len());
                job.entries = entries;
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.selected_remote_path = Some(remote_path.clone());
                self.transfer.browser.selected_remote_paths.clear();
                self.transfer
                    .browser
                    .selected_remote_paths
                    .insert(remote_path.clone());
                self.transfer.set_remote_path(remote_path.clone());
                if self.transfer.complete_properties_update(
                    job_session_id.as_deref(),
                    &remote_path,
                    properties,
                ) {
                    forget_properties_inputs = true;
                    window.close_nya_dialog(cx);
                }
                self.shell.set_status(format!(
                    "remote properties updated in {parent_path}: {remote_path}"
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::EditorLoaded {
                tab_id,
                remote_path,
                generation,
                file,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Opened {remote_path}");
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer
                    .complete_editor_load_tab(&tab_id, generation, file);
                self.transfer.browser.status = format!("opened text file {remote_path}");
                self.shell
                    .set_status(format!("remote text file opened: {remote_path}"));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::PreviewLoaded {
                tab_id,
                remote_path,
                generation,
                content,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Previewed {remote_path}");
                job.summary = None;
                job.progress = None;
                job.control = None;
                // Generation guard inside: a result for a superseded generation
                // (the user refreshed or closed the tab) is dropped.
                self.transfer
                    .complete_preview_tab(&tab_id, generation, content);
                self.transfer.browser.status = format!("previewed {remote_path}");
                self.shell
                    .set_status(format!("remote file preview ready: {remote_path}"));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::PdfPageRendered {
                tab_id,
                generation,
                page_index,
                page,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Rendered PDF page {}", page_index + 1);
                job.summary = None;
                job.progress = None;
                job.control = None;
                // Generation guard inside: a page for a superseded generation
                // (refresh/rotate) is discarded rather than cached.
                match page {
                    Ok(page) => {
                        self.transfer
                            .complete_pdf_page(&tab_id, generation, page_index, page);
                    }
                    Err(_) => {
                        self.transfer.fail_pdf_page(&tab_id, generation, page_index);
                    }
                }
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::AiFileActionLoaded {
                remote_path,
                action_id,
                action_name,
                prompt,
                file,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Prepared AI action {action_name} for {remote_path}");
                job.summary = None;
                job.progress = None;
                job.control = None;

                if job_session_id.as_deref() == self.session.active_id() {
                    let mut context = self.ai_terminal_context();
                    context.selected_text = file.content;
                    context.cwd = Some(transfer_event_remote_parent_path(&remote_path));
                    let request = AiPreparedRequest {
                        action: AiAction::CustomFileAction,
                        context,
                        source_label: format!("{action_name} · {remote_path}"),
                    };
                    self.set_ai_prompt_draft(prompt, cx);
                    self.ai.prepare_external_request(
                        request,
                        format!(
                            "Loaded {} byte(s) from {remote_path} for AI action {action_name}",
                            file.size
                        ),
                        format!("AI file action ready: {action_name} ({action_id})"),
                        true,
                    );
                    self.ensure_panel_open(NavItem::AiAssistant);
                    self.transfer.browser.status = format!("AI action ready for {remote_path}");
                    self.shell.set_status(format!(
                        "AI assistant opened for remote file: {remote_path}"
                    ));
                }
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::EditorSaved {
                tab_id,
                remote_path,
                generation,
                result,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Saved {remote_path}");
                job.summary = None;
                job.progress = None;
                job.control = None;
                if let Some(outcome) = self
                    .transfer
                    .complete_editor_save_tab(&tab_id, generation, result)
                {
                    self.shell.set_status(match outcome {
                        TransferEditorSaveOutcome::Saved => {
                            format!("remote text file saved: {remote_path}")
                        }
                        TransferEditorSaveOutcome::Conflict => {
                            format!("remote text save conflict: {remote_path}")
                        }
                        TransferEditorSaveOutcome::SavedAndClosed => {
                            format!("remote text file saved and closed: {remote_path}")
                        }
                    });
                }
                self.transfer.browser.status =
                    format!("text editor save finished for {remote_path}");
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::ExternalOpened {
                remote_path,
                raw_path_token,
                local_path,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("Opened {}", local_path.display());
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.transfer.browser.status = format!("opened external {remote_path}");
                self.shell.set_status(format!(
                    "remote file opened externally: {}",
                    local_path.display()
                ));
                external_watch_to_start = Some((
                    job_session_id.clone(),
                    job.id.clone(),
                    remote_path,
                    raw_path_token,
                    local_path,
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Summary(summary))) => {
                job.status = TransferJobStatus::Completed;
                job.detail = if summary.skipped {
                    "Skipped duplicate".to_string()
                } else {
                    format!("{} transferred", format_file_size(Some(summary.bytes)))
                };
                job.entries.clear();
                job.progress = Some(SftpTransferProgress {
                    remote_path: summary.remote_path.clone(),
                    local_path: summary.local_path.clone(),
                    bytes_transferred: summary.bytes,
                    total_bytes: Some(summary.bytes),
                    item_count_completed: job
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.item_count_total),
                    item_count_total: job
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.item_count_total),
                });
                job.summary = Some(summary);
                self.shell
                    .set_status(format!("remote transfer completed: {}", job.detail));
                job.control = None;
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::Uploaded {
                summary,
                parent_path,
                entries,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = format!("{} uploaded", format_file_size(Some(summary.bytes)));
                job.progress = Some(SftpTransferProgress {
                    remote_path: summary.remote_path.clone(),
                    local_path: summary.local_path.clone(),
                    bytes_transferred: summary.bytes,
                    total_bytes: Some(summary.bytes),
                    item_count_completed: job
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.item_count_total),
                    item_count_total: job
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.item_count_total),
                });
                job.summary = Some(summary.clone());
                job.control = None;

                if transfer_event_paths_match(&self.transfer.browser.path, &parent_path) {
                    self.transfer.browser.path = parent_path.clone();
                    self.transfer.browser.entries = Arc::new(entries.clone());
                    self.transfer.browser.status = format!("{} item(s)", entries.len());
                    self.transfer.browser.selected_remote_path = Some(summary.remote_path.clone());
                    self.transfer.browser.selected_remote_paths.clear();
                    self.transfer
                        .browser
                        .selected_remote_paths
                        .insert(summary.remote_path.clone());
                    remote_path_to_set = Some(summary.remote_path.clone());
                } else {
                    self.transfer.browser.status =
                        format!("uploaded to {}", truncate_preview(&parent_path, 48));
                }

                job.entries = entries;
                self.shell.set_status(format!(
                    "remote upload completed in {parent_path}: {}",
                    job.detail
                ));
            }
            TransferJobEvent::Finished(Ok(TransferJobOutput::ZmodemProbeReady {
                session_id,
                files,
                probe_skipped,
            })) => {
                job.status = TransferJobStatus::Completed;
                job.detail = if probe_skipped {
                    format!("ZMODEM probe skipped; uploading {} file(s)", files.len())
                } else {
                    format!("ZMODEM probe ready; uploading {} file(s)", files.len())
                };
                job.entries.clear();
                job.summary = None;
                job.progress = None;
                job.control = None;
                self.shell.set_status(job.detail.clone());
                if files.is_empty() {
                    self.shell.set_status(
                        "ZMODEM upload cancelled — all conflicting files skipped".to_string(),
                    );
                } else {
                    zmodem_upload_after_probe = Some((session_id, files));
                }
            }
            TransferJobEvent::Finished(Err(error)) => {
                if matches!(&job.kind, TransferJobKind::ListDir { .. }) {
                    browser_navigation_rollback = self
                        .transfer
                        .browser
                        .pending_navigations
                        .remove(&event_id)
                        .map(|snapshot| (snapshot, error.clone()));
                }
                let browser_load_failed = matches!(
                    &job.kind,
                    TransferJobKind::ListDir { .. }
                        | TransferJobKind::ResolveHome
                        | TransferJobKind::SyncCwd
                );
                let property_remote_path = match &job.kind {
                    TransferJobKind::LoadProperties { remote_path }
                    | TransferJobKind::UpdateProperties { remote_path, .. }
                    | TransferJobKind::LoadEditor { remote_path, .. }
                    | TransferJobKind::SaveEditor { remote_path, .. }
                    | TransferJobKind::OpenExternal { remote_path, .. }
                    | TransferJobKind::AiFileAction { remote_path, .. } => {
                        Some(remote_path.clone())
                    }
                    _ => None,
                };
                if let TransferJobKind::ListChildren { remote_path } = &job.kind
                    && job_session_id.as_deref() == self.session.active_id()
                    && let Some(TransferBrowserPathMenuState {
                        session_id,
                        kind:
                            TransferBrowserPathMenuKind::Children {
                                path,
                                request_id,
                                status,
                                ..
                            },
                        ..
                    }) = self.transfer.browser.path_menu.as_mut()
                    && *session_id == job_session_id
                    && path == remote_path
                    && request_id.as_deref() == Some(event_id.as_str())
                {
                    *request_id = None;
                    *status = TransferBrowserChildrenMenuStatus::Error(error.clone());
                }
                if error == SFTP_TRANSFER_CANCELLED {
                    job.status = TransferJobStatus::Cancelled;
                    job.detail = "Cancelled".to_string();
                    self.shell
                        .set_status(format!("remote transfer cancelled: {}", job.id));
                } else {
                    job.status = TransferJobStatus::Failed;
                    job.detail = error.clone();
                    self.shell
                        .set_status(format!("remote transfer failed: {error}"));
                }
                if browser_load_failed {
                    self.transfer.browser.loading = false;
                    self.transfer.browser.error = self
                        .transfer
                        .browser
                        .entries
                        .is_empty()
                        .then_some(error.clone());
                }
                if let Some(remote_path) = property_remote_path.as_ref() {
                    self.transfer.fail_properties_operation(
                        job_session_id.as_deref(),
                        remote_path,
                        error.clone(),
                    );
                }
                if let TransferJobKind::LoadEditor {
                    tab_id, generation, ..
                }
                | TransferJobKind::SaveEditor {
                    tab_id, generation, ..
                } = &job.kind
                {
                    self.transfer
                        .fail_editor_operation_tab(tab_id, *generation, error);
                }
                job.summary = None;
                job.control = None;
            }
        }
        if !cleanup_internal_job {
            self.transfer
                .restore_transfer_job_after_event((job_index, job));
        }
        if let Some(remote_path) = remote_path_to_set {
            self.transfer.set_remote_path(remote_path);
        }
        if sync_properties_inputs {
            self.sync_transfer_properties_inputs(cx);
        }
        if forget_properties_inputs {
            self.forget_text_inputs("transfer.properties.");
        }
        if let Some((snapshot, error)) = browser_navigation_rollback {
            self.restore_transfer_browser_navigation(snapshot);
            self.transfer.browser.status = format!("directory load failed: {error}");
            if self.transfer.browser.entries.is_empty() {
                self.transfer.browser.error = Some(error);
            }
        }
        if let Some((session_id, job_id, remote_path, raw_path_token, local_path)) =
            external_sync_to_start
        {
            self.spawn_external_editor_sync_upload(
                session_id,
                job_id,
                remote_path,
                raw_path_token,
                local_path,
            );
        }
        if let Some((session_id, job_id, remote_path, raw_path_token, local_path)) =
            external_watch_to_start
            && session_id
                .as_deref()
                .is_none_or(|session_id| self.session.has_session(session_id))
            && let Err(error) = self.transfer.start_external_editor_watcher(
                session_id,
                job_id,
                remote_path,
                raw_path_token,
                local_path,
            )
        {
            self.shell.set_status(error);
        }
        if let Some((session_id, files)) = zmodem_upload_after_probe {
            self.begin_zmodem_upload_after_probe(session_id, files, cx);
        }
        if let Some(entry) = open_after_create
            && inactive_browser_snapshot.is_none()
            && self.session.active_id() == job_session_id.as_deref()
        {
            self.open_transfer_default(entry, window, cx);
        }
        if browser_listing_completed && let Some(session_id) = job_session_id.as_deref() {
            self.cache_transfer_browser_session(session_id);
        }
        if let Some(snapshot) = inactive_browser_snapshot {
            self.transfer.restore_browser_event_snapshot(snapshot);
        }
        if event_finished
            && let Some(key) = navigation_job_key
            && self
                .transfer
                .browser
                .navigation_jobs
                .get(&key)
                .is_some_and(|latest_id| latest_id == &event_id)
        {
            self.transfer.browser.navigation_jobs.remove(&key);
        }
        if event_finished {
            self.transfer.browser.pending_navigations.remove(&event_id);
        }
        if event_succeeded {
            self.transfer.release_transfer_job_path_options(&event_id);
        }
        if let Some(prompt_id) = external_sync_prompt_to_open {
            self.open_transfer_external_sync_window(prompt_id, cx);
        }
        dirty
    }
}

fn transfer_event_paths_match(left: &str, right: &str) -> bool {
    transfer_event_normalized_path(left) == transfer_event_normalized_path(right)
}

fn transfer_event_needs_browser_context(kind: &TransferJobKind, event: &TransferJobEvent) -> bool {
    matches!(event, TransferJobEvent::Finished(_))
        && !matches!(kind, TransferJobKind::ListChildren { .. })
}

fn transfer_event_needs_ui_refresh(
    active_session_id: Option<&str>,
    job_session_id: Option<&str>,
    event: &TransferJobEvent,
) -> bool {
    !matches!(event, TransferJobEvent::Progress(_)) || job_session_id == active_session_id
}

fn transfer_event_remote_parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => ".".to_string(),
    }
}

fn transfer_event_normalized_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        ".".to_string()
    } else if trimmed == "/" {
        "/".to_string()
    } else {
        trimmed.trim_end_matches('/').to_string()
    }
}

fn transfer_navigation_job_is_stale(
    latest_jobs: &HashMap<String, String>,
    session_key: Option<&str>,
    event_id: &str,
) -> bool {
    session_key.is_some_and(|key| {
        latest_jobs
            .get(key)
            .is_none_or(|latest_id| latest_id != event_id)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use nyaterm_transport::{SftpPathTransferOptions, SftpTransferProgress, SftpTransferSummary};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::{
        TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
        TransferJobStatus,
    };
    use crate::test_support::TestConfigDir;

    use super::{
        TRANSFER_UI_COALESCE_WINDOW, transfer_event_needs_browser_context,
        transfer_event_needs_ui_refresh, transfer_navigation_job_is_stale,
    };

    fn app(cx: &mut TestAppContext, root: &Path) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    /// The drain reaches the app through `update_in`, which resolves the window the
    /// app is displayed in. That mapping is only populated by a draw, so these tests
    /// need a real window that reads the app -- not just the entity.
    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let armed = self.app.read(cx).transfer.transfer_jobs().len();
            div().size_full().child(format!("{armed}"))
        }
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        (app, vcx)
    }

    fn browser_entry(name: &str) -> nyaterm_transport::SftpFileEntry {
        nyaterm_transport::SftpFileEntry {
            name: name.to_string(),
            path: format!("/remote/{name}"),
            file_type: nyaterm_transport::SftpFileType::File,
            size: Some(1),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    fn running_job(id: &str) -> TransferJobState {
        TransferJobState {
            id: id.to_string(),
            session_id: None,
            kind: TransferJobKind::Download {
                remote_path: format!("/remote/{id}"),
                raw_path_token: None,
                local_path: PathBuf::from(format!("/local/{id}")),
            },
            status: TransferJobStatus::Running,
            detail: String::new(),
            created_at_ms: 0,
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        }
    }

    fn progress(id: &str, bytes: u64) -> TransferJobResult {
        TransferJobResult {
            id: id.to_string(),
            event: TransferJobEvent::Progress(SftpTransferProgress {
                remote_path: format!("/remote/{id}"),
                local_path: PathBuf::from(format!("/local/{id}")),
                bytes_transferred: bytes,
                total_bytes: Some(1_000_000),
                item_count_completed: None,
                item_count_total: None,
            }),
        }
    }

    /// The span every case is simulated over. Ten coalescing windows.
    const STRESS_SPAN: Duration = Duration::from_millis(160);
    /// What one job's source-side throttle emits at.
    const SOURCE_PERIOD: Duration = Duration::from_millis(50);
    const STRESS_STEP: Duration = Duration::from_millis(1);

    /// Run `producers` jobs over a *fixed* simulated span, each emitting at the
    /// source throttle period but phase-shifted so their events never coincide.
    ///
    /// Returns (GPUI batches, events sent). The span is deliberately held constant
    /// while the producer count varies: that is the only way the batch count can be
    /// shown to track the window rather than the number of transfers.
    fn stress(cx: &mut TestAppContext, root: &Path, producers: usize) -> (usize, usize) {
        let (app, vcx) = hosted(cx, root);
        let sender = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                for index in 0..producers {
                    app.transfer
                        .enqueue_transfer_job(running_job(&format!("job-{index}")));
                }
                app.start_transfer_event_drain(cx);
                app.transfer.transfer_event_sender()
            })
        });
        vcx.run_until_parked();

        // Phase offsets spread the producers evenly inside one source period, so no
        // two ever fire on the same millisecond. Sending them all at one instant
        // would only prove that same-instant bursts collapse.
        let offsets: Vec<u128> = (0..producers)
            .map(|index| (SOURCE_PERIOD.as_millis() * index as u128) / producers.max(1) as u128)
            .collect();
        let period = SOURCE_PERIOD.as_millis();
        let mut sent = 0;
        let mut elapsed = 0u128;
        while elapsed < STRESS_SPAN.as_millis() {
            for (index, offset) in offsets.iter().enumerate() {
                if elapsed >= *offset && (elapsed - offset).is_multiple_of(period) {
                    let _ = sender
                        .unbounded_send(progress(&format!("job-{index}"), elapsed as u64 * 100));
                    sent += 1;
                }
            }
            vcx.executor().advance_clock(STRESS_STEP);
            vcx.run_until_parked();
            elapsed += STRESS_STEP.as_millis();
        }
        vcx.executor()
            .advance_clock(TRANSFER_UI_COALESCE_WINDOW * 2);
        vcx.run_until_parked();

        let batches = vcx.update(|_, cx| app.read(cx).transfer.ui_batch_count());
        (batches, sent)
    }

    /// The property that matters: the UI update rate is bounded by the coalescing
    /// window, not by how many transfers are running.
    ///
    /// Eight phase-shifted producers push eight times the events of one over the
    /// *same* simulated span. Per-event entry into GPUI would put that factor of
    /// eight straight into the batch count; the window is what stops it.
    #[test]
    fn the_ui_update_rate_does_not_scale_with_the_number_of_transfers() {
        let one_dir = TestConfigDir::new("nyaterm-transfer-drain");
        let many_dir = TestConfigDir::new("nyaterm-transfer-drain");
        let mut cx = TestAppContext::single();
        let (one_batch, one_sent) = stress(&mut cx, one_dir.path(), 1);
        let (many_batch, many_sent) = stress(&mut cx, many_dir.path(), 8);

        // The bound: a span of N windows cannot produce more than about N batches,
        // whatever the producer count. One batch of slack for the trailing flush.
        let bound =
            (STRESS_SPAN.as_millis() / TRANSFER_UI_COALESCE_WINDOW.as_millis()) as usize + 1;

        assert!(one_batch > 0, "the drain must have run at all");
        // Not exactly 8x: the producers with the largest phase offsets fit one fewer
        // emission inside a fixed span. A real multiple is the point, not a precise one.
        assert!(
            many_sent >= one_sent * 5,
            "the stress case must actually push several times the events ({many_sent} vs {one_sent})"
        );
        assert!(
            one_batch <= bound,
            "one producer: {one_batch} batches exceeds the {bound}-window bound"
        );
        assert!(
            many_batch <= bound,
            "eight staggered producers sent {many_sent} events in {many_batch} GPUI batches, \
             over a span that bounds them at {bound}. The batch count is tracking the \
             producer count, not the window"
        );
        assert!(
            many_batch < many_sent,
            "coalescing did nothing: {many_batch} batches for {many_sent} events"
        );
    }

    /// The deadline belongs to the batch, not to the last event in it.
    ///
    /// Re-arming per event would let any steady stream hold a batch open for as long
    /// as it kept arriving -- unbounded latency and an unbounded batch, which is the
    /// failure the fixed deadline exists to prevent. A stream spread over several
    /// windows therefore has to produce several batches.
    ///
    /// This covers the deadline being fixed. It does not cover the branch ordering in
    /// `select_biased!`: starving the deadline needs the queue to be non-empty at the
    /// instant it fires, and virtual time only advances once every task has parked, so
    /// a finite queue always drains first. The ordering is argued structurally at the
    /// call site instead.
    #[test]
    fn the_batch_deadline_is_not_extended_by_later_events() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-drain");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let sender = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.transfer.enqueue_transfer_job(running_job("job-0"));
                app.start_transfer_event_drain(cx);
                app.transfer.transfer_event_sender()
            })
        });
        vcx.run_until_parked();

        // A steady stream at a quarter of the window, run for six windows.
        let step = TRANSFER_UI_COALESCE_WINDOW / 4;
        let windows = 6;
        let sends = windows * 4;
        for index in 0..sends {
            let _ = sender.unbounded_send(progress("job-0", index as u64 + 1));
            vcx.executor().advance_clock(step);
            vcx.run_until_parked();
        }
        vcx.executor()
            .advance_clock(TRANSFER_UI_COALESCE_WINDOW * 2);
        vcx.run_until_parked();

        let (batches, largest) = vcx.update(|_, cx| {
            let transfer = &app.read(cx).transfer;
            (transfer.ui_batch_count(), transfer.ui_batch_max())
        });
        assert!(
            batches >= windows / 2,
            "{sends} events over {windows} windows produced only {batches} batch(es), \
             largest {largest}: the deadline is being extended by later events"
        );
        assert!(
            largest < sends,
            "one batch took {largest} of {sends} events, so the stream held it open"
        );
    }

    /// A progress batch must not touch the derived browser listing.
    ///
    /// This is the whole reason the memo exists. Progress moves job byte counts;
    /// the listing is keyed on the entry `Arc`, the filter text, the hidden-files
    /// setting and the sort, none of which a transfer touches. Without the memo each
    /// coalesced batch would clone and re-sort the entire directory.
    #[test]
    fn a_progress_batch_does_not_recompute_the_browser_listing() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-drain");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let sender = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.transfer
                    .replace_browser_entries_for_test(vec![browser_entry("alpha")]);
                app.transfer.enqueue_transfer_job(running_job("job-0"));
                app.start_transfer_event_drain(cx);
                app.transfer.transfer_event_sender()
            })
        });
        vcx.run_until_parked();

        // Take the baseline from a real flush, not a hand-rolled derive: the flush is
        // what the batches below will run, so its key is the one that must stay put.
        let baseline = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.flush_transfer_panel_snapshot(cx);
                app.transfer.browser_filter_recomputes()
            })
        });
        assert!(
            baseline > 0,
            "the listing must have been derived at least once"
        );

        for round in 0..24 {
            let _ = sender.unbounded_send(progress("job-0", (round + 1) * 1_000));
            vcx.executor()
                .advance_clock(TRANSFER_UI_COALESCE_WINDOW * 2);
            vcx.run_until_parked();
        }

        let (batches, recomputes) = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                // Flush again: a hit must still be a hit after all that progress.
                app.flush_transfer_panel_snapshot(cx);
                (
                    app.transfer.ui_batch_count(),
                    app.transfer.browser_filter_recomputes(),
                )
            })
        });
        assert!(batches > 0, "the drain must have applied the progress");
        assert_eq!(
            recomputes,
            baseline,
            "{batches} progress batches recomputed the browser listing {} time(s)",
            recomputes - baseline
        );
    }

    /// Coalescing must not delay a lifecycle transition.
    /// directory listing, the resolved home dir and editor payloads, and opens the
    /// follow-up jobs and dialogs that depend on them.
    #[test]
    fn a_terminal_event_ends_the_batch_without_waiting_for_the_window() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-drain");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let sender = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.transfer.enqueue_transfer_job(running_job("job-0"));
                app.start_transfer_event_drain(cx);
                app.transfer.transfer_event_sender()
            })
        });
        vcx.run_until_parked();

        let _ = sender.unbounded_send(progress("job-0", 1_000));
        let _ = sender.unbounded_send(TransferJobResult {
            id: "job-0".to_string(),
            event: TransferJobEvent::Finished(Err("boom".to_string())),
        });
        // Deliberately less than the window: the batch must have closed on the
        // terminal event rather than sitting out the remaining time.
        vcx.executor()
            .advance_clock(TRANSFER_UI_COALESCE_WINDOW / 4);
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                app.transfer.ui_batch_count() > 0,
                "the terminal event must have been applied before the window elapsed"
            );
            let status = app
                .transfer
                .transfer_jobs()
                .iter()
                .find(|job| job.id == "job-0")
                .map(|job| job.status);
            assert_eq!(
                status,
                Some(TransferJobStatus::Failed),
                "the failure must have landed, not still be queued"
            );
        });
    }

    #[test]
    fn successful_transfer_releases_retry_state_while_failure_keeps_it() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-retry-state");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let sender = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                for id in ["completed", "failed"] {
                    app.transfer.enqueue_transfer_job(running_job(id));
                    app.transfer
                        .bind_transfer_job_path_options(id, SftpPathTransferOptions::default());
                }
                app.start_transfer_event_drain(cx);
                app.transfer.transfer_event_sender()
            })
        });
        vcx.run_until_parked();

        sender
            .unbounded_send(TransferJobResult {
                id: "completed".to_string(),
                event: TransferJobEvent::Finished(Ok(TransferJobOutput::Summary(
                    SftpTransferSummary {
                        remote_path: "/remote/completed".to_string(),
                        local_path: PathBuf::from("/local/completed"),
                        bytes: 1,
                        skipped: false,
                    },
                ))),
            })
            .expect("send completed event");
        sender
            .unbounded_send(TransferJobResult {
                id: "failed".to_string(),
                event: TransferJobEvent::Finished(Err("connection reset".to_string())),
            })
            .expect("send failed event");
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let transfer = &app.read(cx).transfer;
            assert!(!transfer.has_transfer_job_path_options("completed"));
            assert!(transfer.has_transfer_job_path_options("failed"));
        });
    }

    #[test]
    fn superseded_transfer_navigation_result_is_stale_per_session() {
        let latest_jobs = HashMap::from([
            ("session-a".to_string(), "job-a2".to_string()),
            ("session-b".to_string(), "job-b1".to_string()),
        ]);

        assert!(transfer_navigation_job_is_stale(
            &latest_jobs,
            Some("session-a"),
            "job-a1"
        ));
        assert!(!transfer_navigation_job_is_stale(
            &latest_jobs,
            Some("session-a"),
            "job-a2"
        ));
        assert!(!transfer_navigation_job_is_stale(
            &latest_jobs,
            Some("session-b"),
            "job-b1"
        ));
        assert!(!transfer_navigation_job_is_stale(
            &latest_jobs,
            None,
            "unrelated-job"
        ));
    }

    #[test]
    fn transfer_progress_does_not_swap_inactive_browser_context() {
        assert!(!transfer_event_needs_browser_context(
            &TransferJobKind::Download {
                remote_path: "/tmp/file".to_string(),
                raw_path_token: None,
                local_path: PathBuf::from("/tmp/file"),
            },
            &TransferJobEvent::Progress(SftpTransferProgress {
                remote_path: "/tmp/file".to_string(),
                local_path: PathBuf::from("/tmp/file"),
                bytes_transferred: 1,
                total_bytes: Some(2),
                item_count_completed: None,
                item_count_total: None,
            },)
        ));
        assert!(transfer_event_needs_browser_context(
            &TransferJobKind::Upload {
                local_path: PathBuf::from("/tmp/file"),
                remote_path: "/tmp/file".to_string(),
            },
            &TransferJobEvent::Finished(Ok(TransferJobOutput::Uploaded {
                summary: nyaterm_transport::SftpTransferSummary {
                    remote_path: "/tmp/file".to_string(),
                    local_path: PathBuf::from("/tmp/file"),
                    bytes: 2,
                    skipped: false,
                },
                parent_path: "/tmp".to_string(),
                entries: Vec::new(),
            }))
        ));
    }

    #[test]
    fn child_directory_results_do_not_swap_browser_session_context() {
        assert!(!transfer_event_needs_browser_context(
            &TransferJobKind::ListChildren {
                remote_path: "/tmp".to_string(),
            },
            &TransferJobEvent::Finished(Ok(TransferJobOutput::ChildEntries {
                remote_path: "/tmp".to_string(),
                entries: Vec::new(),
            })),
        ));
    }

    #[test]
    fn inactive_transfer_progress_does_not_request_ui_refresh() {
        let progress = TransferJobEvent::Progress(SftpTransferProgress {
            remote_path: "/tmp/file".to_string(),
            local_path: PathBuf::from("/tmp/file"),
            bytes_transferred: 1,
            total_bytes: Some(2),
            item_count_completed: None,
            item_count_total: None,
        });

        assert!(transfer_event_needs_ui_refresh(
            Some("session-a"),
            Some("session-a"),
            &progress,
        ));
        assert!(!transfer_event_needs_ui_refresh(
            Some("session-a"),
            Some("session-b"),
            &progress,
        ));
        assert!(transfer_event_needs_ui_refresh(
            Some("session-a"),
            Some("session-b"),
            &TransferJobEvent::Finished(Err("failed".to_string())),
        ));
    }
}

//! Grouped transfer feature state.
//!
//! The transfer feature is really five things sharing one panel: the job
//! queue, the SFTP browser, the file operation dialogs, the remote editor
//! workspace, and external-editor sync. Splitting them apart makes each
//! lifetime visible; the flat `transfer_*` prefix did not.

mod admission;
mod browser;
mod clipboard;
mod delete;
mod privilege;
pub(in crate::features) use browser::TransferSessionTransferBundle;
pub(in crate::features) use clipboard::TransferFileClipboard;
pub(super) use delete::TransferDeleteOutcome;
mod browser_logic;
mod tree;
pub(in crate::features) use tree::TransferTreePresentation;

#[cfg(test)]
pub(in crate::features) use self::browser_logic::transfer_browser_entry_is_visible;
pub(in crate::features) use self::browser_logic::{
    BrowserFilterCache, BrowserFilterKey, natural_compare_ascii,
};
mod editor;
mod preview;
pub(in crate::features) use self::preview::{PdfPageRequest, TransferPreviewCloseOutcome};

use super::external_sync_runtime::ExternalEditorWatcher;
use crate::models::{
    TransferBrowserColumnResizeState, TransferBrowserColumnWidths, TransferBrowserContextTarget,
    TransferBrowserDragSelectionState, TransferBrowserFavoritesMenuState,
    TransferBrowserNavigationSnapshot, TransferBrowserPathMenuState,
    TransferBrowserPendingRenameState, TransferBrowserSessionCacheState, TransferBrowserSortColumn,
    TransferBrowserSortDirection, TransferBrowserUploadMenuState, TransferEditorWorkspaceState,
    TransferExternalSyncPromptState, TransferHeightResizeState, TransferJobMenuState,
    TransferJobResult, TransferJobState, TransferJobStatus, TransferMoveState,
    TransferNewFileState, TransferNewFolderState, TransferNewSymlinkState, TransferPathPromptKind,
    TransferPreviewWorkspaceState, TransferPropertiesState, TransferRenameState,
    TransferUnknownFileState,
};
use crate::thread_owner::BackgroundCleanup;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{BackgroundExecutor, FocusHandle, Pixels, ScrollHandle, UniformListScrollHandle};
use nyaterm_files::sftp::{SftpFileEntry, SftpFileProperties, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{SftpDuplicatePolicy, SftpPathTransferOptions};
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

struct TransferRetryContext {
    options: SftpPathTransferOptions,
    source: Option<nyaterm_files::file_browser::WeakFileBrowserService>,
    local: bool,
    upload_target_lock: Option<Arc<std::sync::Mutex<()>>>,
}

pub(in crate::features) struct TransferFeatureState {
    pub(super) sudo_prompt: Option<super::privilege_runtime::FileSudoPrompt>,
    external_file_tasks: HashMap<String, std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    file_operations: HashMap<String, super::file_operations::FileOperationContext>,
    clipboard: Option<TransferFileClipboard>,
    clipboard_generation: u64,
    cut_jobs: HashMap<String, (u64, String)>,
    tree: tree::TransferTreeState,
    tree_focus: FocusHandle,
    queue: TransferQueueState,
    bulk_executor: Option<crate::blocking_jobs::BlockingJobScheduler>,
    admission: admission::TransferAdmission,
    worker_cleanup: BackgroundCleanup,
    paths: TransferPathState,
    pub(super) browser: TransferBrowserState,
    /// The memo over `browser.entries`.
    ///
    /// A coalesced progress batch moves job byte counts and nothing this listing is
    /// derived from, so the high-rate path must not pay to clone and sort a whole
    /// directory. Owned here rather than exposed for the view to drive, so there is
    /// no cache protocol for a caller to get wrong.
    browser_filter: BrowserFilterCache,
    file_ops: TransferFileOpsState,
    editor: TransferEditorFeatureState,
    preview: TransferPreviewFeatureState,
    external_sync: TransferExternalSyncState,
    panel: TransferPanelState,
    /// 失败和取消的 SFTP 任务保留原策略及 Ask 决定，供手动重试复用。
    path_options_by_job: HashMap<String, TransferRetryContext>,
    /// How many times the event drain has entered GPUI.
    ///
    /// The coalescing test needs the batch count, not the event count: the point
    /// it proves is that the first is bounded while the second is not.
    #[cfg(test)]
    ui_batch_count: usize,
    /// The largest single batch the drain has applied. A batch that outgrew the
    /// coalescing window is how deadline starvation shows up.
    #[cfg(test)]
    ui_batch_max: usize,
}

/// Focus handles the transfer feature needs at construction time.
pub(in crate::features) struct TransferFeatureFocus {
    pub tree: FocusHandle,
    pub queue: FocusHandle,
    pub browser: FocusHandle,
    pub editor: FocusHandle,
    pub preview: FocusHandle,
    pub external_sync: FocusHandle,
}

/// Upload/download job queue.
struct TransferQueueState {
    tx: UnboundedSender<TransferJobResult>,
    /// Taken once by `NyaTermApp::start_transfer_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<TransferJobResult>>,
    jobs: Vec<TransferJobState>,
    next_job_sequence: u64,
    selected_job_id: Option<String>,
    job_menu: Option<TransferJobMenuState>,
    focus: FocusHandle,
}

/// Manual transfer endpoints and the duplicate policy that applies to them.
struct TransferPathState {
    remote: String,
    local: String,
    duplicate_policy: SftpDuplicatePolicy,
    prompt: Option<TransferPathPromptKind>,
}

/// Borrowed presentation state for the SFTP browser.
///
/// Mutations stay on `TransferFeatureState`; renderers and app-level adapters
/// can inspect browser state without receiving the authoritative child.
pub(in crate::features) struct TransferBrowserView<'a> {
    pub path: &'a String,
    pub raw_path_token: &'a Option<String>,
    pub home_dir: &'a String,
    pub home_dir_pending: bool,
    pub path_draft: &'a String,
    pub path_editing: bool,
    pub entries: &'a Arc<Vec<SftpFileEntry>>,
    pub loading: bool,
    pub error: &'a Option<String>,
    pub search: &'a String,
    pub list_scroll: &'a UniformListScrollHandle,
    pub horizontal_scroll: &'a ScrollHandle,
    pub search_expanded: bool,
    pub history: &'a VecDeque<String>,
    pub history_index: usize,
    pub visited_history: &'a VecDeque<String>,
    pub favorites: &'a VecDeque<String>,
    pub sort_column: TransferBrowserSortColumn,
    pub sort_direction: TransferBrowserSortDirection,
    pub column_widths: TransferBrowserColumnWidths,
    pub column_resize: &'a Option<TransferBrowserColumnResizeState>,
    pub selected_remote_path: &'a Option<String>,
    pub selected_remote_paths: &'a HashSet<String>,
    pub drag_selection: &'a Option<TransferBrowserDragSelectionState>,
    pub external_drop_hover: bool,
    pub context_target: &'a TransferBrowserContextTarget,
    pub favorites_menu: &'a Option<TransferBrowserFavoritesMenuState>,
    pub path_menu: &'a Option<TransferBrowserPathMenuState>,
    pub path_menu_scroll: &'a ScrollHandle,
    pub upload_menu: &'a Option<TransferBrowserUploadMenuState>,
    pub focus: &'a FocusHandle,
}

/// SFTP browser: current listing, navigation history, selection and menus.
pub(super) struct TransferBrowserState {
    pub(super) path: String,
    pub(super) raw_path_token: Option<String>,
    pub(super) home_dir: String,
    pub(super) home_dir_pending: bool,
    pub(super) path_draft: String,
    pub(super) path_editing: bool,
    /// Shared, and always replaced whole.
    ///
    /// The browser listing is swapped in and out of caches and navigation snapshots,
    /// and the filter/sort memo keys on this pointer. Both want the same thing: one
    /// allocation handed around rather than deep-copied, and a pointer that changes
    /// exactly when the listing does.
    pub(super) entries: Arc<Vec<SftpFileEntry>>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) status: String,
    pub(super) search: String,
    pub(super) list_scroll: UniformListScrollHandle,
    pub(super) horizontal_scroll: ScrollHandle,
    pub(super) search_expanded: bool,
    pub(super) history: VecDeque<String>,
    pub(super) history_index: usize,
    pub(super) visited_history: VecDeque<String>,
    pub(super) session_cache: HashMap<String, TransferBrowserSessionCacheState>,
    /// Latest SFTP navigation job per session; older results must not rewind the browser.
    pub(super) navigation_jobs: HashMap<String, String>,
    pub(super) pending_navigations: HashMap<String, TransferBrowserNavigationSnapshot>,
    pub(super) navigation_handles: HashMap<String, crate::blocking_jobs::JobHandle>,
    pub(super) pending_cwd_sync: bool,
    #[cfg(test)]
    pub(super) directory_requests: Option<Vec<(String, nyaterm_files::sftp::RemoteFilePath)>>,
    pub(super) favorites: VecDeque<String>,
    pub(super) sort_column: TransferBrowserSortColumn,
    pub(super) sort_direction: TransferBrowserSortDirection,
    pub(super) column_widths: TransferBrowserColumnWidths,
    pub(super) column_resize: Option<TransferBrowserColumnResizeState>,
    pub(super) selected_remote_path: Option<String>,
    pub(super) selected_remote_paths: HashSet<String>,
    pub(super) drag_selection: Option<TransferBrowserDragSelectionState>,
    pub(super) pending_selection_click: Option<String>,
    pub(super) external_drop_hover: bool,
    pub(super) context_target: TransferBrowserContextTarget,
    pub(super) rename_click_candidate: Option<String>,
    pub(super) pending_rename: Option<TransferBrowserPendingRenameState>,
    pub(super) pending_rename_token: u64,
    pub(super) favorites_menu: Option<TransferBrowserFavoritesMenuState>,
    pub(super) path_menu: Option<TransferBrowserPathMenuState>,
    pub(super) path_menu_scroll: ScrollHandle,
    pub(super) upload_menu: Option<TransferBrowserUploadMenuState>,
    pub(super) focus: FocusHandle,
}

/// Rename/move/delete/create/properties dialogs over browser entries.
struct TransferFileOpsState {
    delete_batches: HashMap<String, delete::TransferDeleteBatch>,
    delete_refresh_pending: HashSet<String>,
    rename: Option<TransferRenameState>,
    rename_focus_pending: bool,
    move_to: Option<TransferMoveState>,
    new_folder: Option<TransferNewFolderState>,
    new_file: Option<TransferNewFileState>,
    new_symlink: Option<TransferNewSymlinkState>,
    properties: Option<TransferPropertiesState>,
    unknown_file: Option<TransferUnknownFileState>,
}

/// Built-in remote file editor workspace.
struct TransferEditorFeatureState {
    views: HashMap<String, gpui::Entity<super::workspace_editor::WorkspaceEditorView>>,
    workspace: Option<TransferEditorWorkspaceState>,
    window: ChildWindowSlot,
}

/// Built-in read-only remote file preview workspace.
///
/// A sibling of the editor state, kept as its own authoritative owner rather
/// than folded into the editor: a file can be open for editing and for preview
/// at once, the preview carries no dirty/save lifecycle, and the two windows are
/// independent. Nothing outside `TransferFeatureState` holds preview state.
struct TransferPreviewFeatureState {
    workspace: Option<TransferPreviewWorkspaceState>,
    focus: FocusHandle,
    window: ChildWindowSlot,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum TransferEditorCloseOutcome {
    Missing,
    ConfirmationRequired,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(in crate::features) enum TransferEditorDiscardOutcome {
    Missing,
    TabDiscarded,
    WorkspaceDiscarded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) enum TransferEditorSaveOutcome {
    Saved,
    SavedWithWarning(String),
    Conflict,
    SavedAndClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub(in crate::features) enum TransferEditorCloseAfterSave {
    Missing,
    Loading,
    Saving,
    Ready(String),
    All,
}

/// Handing a remote file to an external editor and syncing it back.
struct TransferExternalSyncState {
    prompts: HashMap<String, TransferExternalSyncPromptState>,
    /// One slot per prompt id: several files can be modified at once, so
    /// several prompt windows can be open at once.
    windows: HashMap<String, ChildWindowSlot>,
    always_uploads: HashSet<String>,
    watchers: HashMap<String, ExternalEditorWatcherEntry>,
    // Parent control also covers overlapping uploads whose queue row has finished.
    upload_control: SftpTransferControl,
    focus: FocusHandle,
}

struct ExternalEditorWatcherEntry {
    session_id: Option<String>,
    _watcher: ExternalEditorWatcher,
}

/// 传输面板的高度及拖动状态；全局冲突对话框的焦点由 NyaDialog 管理。
struct TransferPanelState {
    height: f32,
    height_resize: Option<TransferHeightResizeState>,
}

impl TransferFeatureState {
    pub(in crate::features) fn track_file_operation(
        &mut self,
        id: String,
        context: super::file_operations::FileOperationContext,
    ) {
        self.file_operations.insert(id, context);
    }

    pub(in crate::features) fn file_operation(
        &self,
        id: &str,
    ) -> Option<&super::file_operations::FileOperationContext> {
        self.file_operations.get(id)
    }

    pub(in crate::features) fn file_operation_references_session(&self, session: &str) -> bool {
        self.file_operations.iter().any(|(id, context)| {
            let (source, target) = context.endpoints();
            (source.session_id() == Some(session) || target.session_id() == Some(session))
                && self.queue.jobs.iter().any(|job| {
                    job.id == *id
                        && matches!(
                            job.status,
                            TransferJobStatus::Queued
                                | TransferJobStatus::Running
                                | TransferJobStatus::Paused
                                | TransferJobStatus::Cancelling
                        )
                })
        })
    }

    pub(in crate::features) fn session_has_active_transfer(&self, session_ids: &[String]) -> bool {
        let session_ids = session_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        self.queue.jobs.iter().any(|job| {
            job.session_id
                .as_deref()
                .is_some_and(|session_id| session_ids.contains(session_id))
                && matches!(
                    job.status,
                    TransferJobStatus::Queued
                        | TransferJobStatus::Running
                        | TransferJobStatus::Paused
                        | TransferJobStatus::Cancelling
                )
        })
    }

    pub(in crate::features) fn new(
        remote_path: String,
        local_path: String,
        duplicate_policy: SftpDuplicatePolicy,
        panel_height: f32,
        focus: TransferFeatureFocus,
        background_executor: BackgroundExecutor,
    ) -> Self {
        let (tx, rx) = unbounded();
        Self {
            sudo_prompt: None,
            external_file_tasks: HashMap::new(),
            file_operations: HashMap::new(),
            clipboard: None,
            clipboard_generation: 0,
            cut_jobs: HashMap::new(),
            tree: tree::TransferTreeState::default(),
            tree_focus: focus.tree,
            #[cfg(test)]
            ui_batch_count: 0,
            #[cfg(test)]
            ui_batch_max: 0,
            file_ops: TransferFileOpsState::new(),
            queue: TransferQueueState::new(tx, rx, focus.queue),
            bulk_executor: None,
            admission: admission::TransferAdmission::default(),
            worker_cleanup: BackgroundCleanup::new(background_executor),
            paths: TransferPathState::new(remote_path, local_path, duplicate_policy),
            browser_filter: BrowserFilterCache::default(),
            browser: TransferBrowserState {
                path: ".".to_string(),
                raw_path_token: None,
                home_dir: String::new(),
                home_dir_pending: false,
                path_draft: String::new(),
                path_editing: false,
                entries: Arc::new(Vec::new()),
                loading: false,
                error: None,
                status: "List a remote directory to browse files.".to_string(),
                search: String::new(),
                list_scroll: UniformListScrollHandle::new(),
                horizontal_scroll: ScrollHandle::new(),
                search_expanded: false,
                history: VecDeque::new(),
                history_index: 0,
                visited_history: VecDeque::new(),
                session_cache: HashMap::new(),
                navigation_jobs: HashMap::new(),
                pending_navigations: HashMap::new(),
                navigation_handles: HashMap::new(),
                pending_cwd_sync: false,
                #[cfg(test)]
                directory_requests: None,
                favorites: VecDeque::new(),
                sort_column: TransferBrowserSortColumn::Name,
                sort_direction: TransferBrowserSortDirection::Ascending,
                column_widths: TransferBrowserColumnWidths::default(),
                column_resize: None,
                selected_remote_path: None,
                selected_remote_paths: HashSet::new(),
                drag_selection: None,
                external_drop_hover: false,
                context_target: TransferBrowserContextTarget::default(),
                rename_click_candidate: None,
                pending_selection_click: None,
                pending_rename: None,
                pending_rename_token: 0,
                favorites_menu: None,
                path_menu: None,
                path_menu_scroll: ScrollHandle::default(),
                upload_menu: None,
                focus: focus.browser,
            },
            editor: TransferEditorFeatureState::new(focus.editor),
            preview: TransferPreviewFeatureState::new(focus.preview),
            external_sync: TransferExternalSyncState::new(focus.external_sync),
            panel: TransferPanelState {
                height: panel_height,
                height_resize: None,
            },
            path_options_by_job: HashMap::new(),
        }
    }

    pub(in crate::features) fn queue_focus(&self) -> &FocusHandle {
        self.queue.focus()
    }

    pub(in crate::features) fn transfer_jobs(&self) -> &[TransferJobState] {
        self.queue.jobs()
    }

    pub(in crate::features) fn transfer_job(&self, job_id: &str) -> Option<&TransferJobState> {
        self.queue.job(job_id)
    }

    pub(in crate::features) fn transfer_job_mut(
        &mut self,
        job_id: &str,
    ) -> Option<&mut TransferJobState> {
        self.queue.job_mut(job_id)
    }

    pub(in crate::features) fn manages_bulk_job(&self, id: &str) -> bool {
        self.admission.manages(id)
    }

    pub(in crate::features) fn supports_bulk_admission(&self, id: &str) -> bool {
        self.file_operations
            .get(id)
            .is_none_or(|context| context.can_pause() || context.is_delete())
            && self
                .queue
                .job(id)
                .is_some_and(|job| admission::TransferAdmission::supports(&job.kind))
    }

    pub(in crate::features) fn admit_bulk_job(
        &mut self,
        id: &str,
        name: &'static str,
        tx: UnboundedSender<TransferJobResult>,
        work: Box<dyn FnOnce() + Send>,
    ) {
        if let Some(job) = self.queue.jobs.iter_mut().find(|job| job.id == id) {
            if self
                .file_operations
                .get(id)
                .is_some_and(|context| context.is_delete())
            {
                self.admission.enqueue_delete(job, name, tx, work);
            } else {
                self.admission.enqueue(job, name, tx, work);
            }
        }
    }

    pub(in crate::features) fn reconcile_bulk_jobs(&mut self, download: u32, upload: u32) {
        let retired = self
            .admission
            .reconcile(&mut self.queue.jobs, download, upload);
        if !retired.is_empty() {
            self.worker_cleanup.submit(move || {
                for thread in retired {
                    let _ = thread.join();
                }
            });
        }
    }

    pub(in crate::features) fn cancel_bulk_jobs_for_session(&mut self, session: &str) {
        self.cancel_session_jobs(session, false);
    }

    pub(in crate::features) fn cancel_bulk_jobs_for_closed_view(
        &mut self,
        session: &str,
        adb: bool,
    ) {
        self.cancel_session_jobs(session, adb);
    }

    fn cancel_session_jobs(&mut self, session: &str, preserve_adb_transfers: bool) {
        for job in &mut self.queue.jobs {
            let file_transfer = matches!(
                job.kind,
                crate::models::TransferJobKind::Upload { .. }
                    | crate::models::TransferJobKind::Download { .. }
                    | crate::models::TransferJobKind::DragExport { .. }
            );
            if preserve_adb_transfers && file_transfer && job.session_id.as_deref() == Some(session)
            {
                continue;
            }
            let target_matches = matches!(&job.kind, crate::models::TransferJobKind::SendTo { target_session_id, .. } if target_session_id == session);
            if (self.admission.manages(&job.id) || file_transfer)
                && (job.session_id.as_deref() == Some(session) || target_matches)
                && let Some(control) = &job.control
            {
                control.cancel();
                job.status = TransferJobStatus::Cancelling;
            }
        }
    }

    pub(in crate::features) fn visit_transfer_jobs_mut(
        &mut self,
        visit: impl FnMut(&mut TransferJobState),
    ) {
        self.queue.visit_jobs_mut(visit);
    }

    pub(in crate::features) fn transfer_jobs_mut_for_protocol(
        &mut self,
        session_id: &str,
        xymodem: bool,
    ) -> impl Iterator<Item = &mut TransferJobState> {
        self.queue.jobs.iter_mut().filter(move |job| {
            job.session_id.as_deref() == Some(session_id)
                && if xymodem {
                    matches!(
                        job.kind,
                        crate::models::TransferJobKind::XmodemUpload { .. }
                            | crate::models::TransferJobKind::YmodemUpload { .. }
                    )
                } else {
                    false
                }
        })
    }

    pub(in crate::features) fn enqueue_transfer_job(&mut self, mut job: TransferJobState) {
        job.ensure_presentation_fields();
        if let Some(progress) = &job.progress {
            job.speed.record(progress.bytes_transferred, Instant::now());
        }
        self.queue.enqueue(job);
    }

    /// 保存任务自己的路径选项；普通 Clone 已为每个新任务创建独立运行态。
    pub(in crate::features) fn bind_transfer_job_path_options(
        &mut self,
        job_id: &str,
        path_options: SftpPathTransferOptions,
    ) -> SftpPathTransferOptions {
        let retry_options =
            path_options.with_transfer_options(path_options.transfer_options().clone());
        self.path_options_by_job.insert(
            job_id.to_string(),
            TransferRetryContext {
                options: retry_options,
                source: None,
                local: false,
                upload_target_lock: None,
            },
        );
        path_options
    }

    pub(in crate::features) fn bind_promised_download(
        &mut self,
        job_id: &str,
        options: SftpPathTransferOptions,
        source: impl Into<nyaterm_files::file_browser::WeakFileBrowserService>,
    ) {
        self.path_options_by_job.insert(
            job_id.to_owned(),
            TransferRetryContext {
                options,
                source: Some(source.into()),
                local: false,
                upload_target_lock: None,
            },
        );
    }

    pub(in crate::features) fn promised_download_source(
        &self,
        job_id: &str,
    ) -> Option<nyaterm_files::file_browser::WeakFileBrowserService> {
        self.path_options_by_job
            .get(job_id)
            .and_then(|context| context.source.clone())
    }

    pub(in crate::features) fn bind_transfer_job_source(
        &mut self,
        job_id: &str,
        service: &nyaterm_files::file_browser::FileBrowserService,
    ) {
        if let Some(context) = self.path_options_by_job.get_mut(job_id) {
            context.source = service.downgrade();
            context.local =
                service.kind() == nyaterm_files::file_browser::FileBrowserBackendKind::Local;
        }
    }

    pub(in crate::features) fn transfer_job_is_local(&self, job_id: &str) -> bool {
        self.path_options_by_job
            .get(job_id)
            .is_some_and(|context| context.local)
    }

    pub(in crate::features) fn bind_upload_target_lock(
        &mut self,
        job_id: &str,
        lock: Arc<std::sync::Mutex<()>>,
    ) {
        if let Some(context) = self.path_options_by_job.get_mut(job_id) {
            context.upload_target_lock = Some(lock);
        }
    }

    pub(in crate::features) fn upload_target_lock(
        &self,
        job_id: &str,
    ) -> Arc<std::sync::Mutex<()>> {
        self.path_options_by_job
            .get(job_id)
            .and_then(|context| context.upload_target_lock.clone())
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(())))
    }

    pub(in crate::features) fn is_promised_download(&self, job_id: &str) -> bool {
        self.path_options_by_job
            .get(job_id)
            .is_some_and(|context| context.options.is_promised_download())
    }

    /// 已有任务沿用原策略和 Ask 决定，只接受当前的执行参数；旧任务才使用回退值。
    pub(in crate::features) fn transfer_job_retry_path_options(
        &mut self,
        job_id: &str,
        fallback: SftpPathTransferOptions,
    ) -> SftpPathTransferOptions {
        if let Some(path_options) = self.path_options_by_job.get(job_id) {
            return path_options.options.with_transfer_options(
                if path_options.options.is_promised_download() {
                    path_options.options.transfer_options().clone()
                } else {
                    fallback.transfer_options().clone()
                },
            );
        }
        self.bind_transfer_job_path_options(job_id, fallback)
    }

    /// 成功任务不能再次重试，立即释放可能很大的目录冲突决定表。
    pub(in crate::features) fn release_transfer_job_path_options(&mut self, job_id: &str) {
        self.path_options_by_job.remove(job_id);
    }

    #[cfg(test)]
    pub(in crate::features) fn has_transfer_job_path_options(&self, job_id: &str) -> bool {
        self.path_options_by_job.contains_key(job_id)
    }

    #[cfg(test)]
    pub(in crate::features) fn note_ui_batch(&mut self, size: usize) {
        self.ui_batch_count += 1;
        self.ui_batch_max = self.ui_batch_max.max(size);
    }

    #[cfg(test)]
    pub(in crate::features) fn ui_batch_max(&self) -> usize {
        self.ui_batch_max
    }

    #[cfg(test)]
    pub(in crate::features) fn ui_batch_count(&self) -> usize {
        self.ui_batch_count
    }

    pub(in crate::features) fn transfer_event_sender(&self) -> UnboundedSender<TransferJobResult> {
        self.queue.event_sender()
    }

    pub(in crate::features) fn take_transfer_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<TransferJobResult>> {
        self.queue.take_event_receiver()
    }

    pub(in crate::features) fn take_transfer_job_for_event(
        &mut self,
        job_id: &str,
    ) -> Option<(usize, TransferJobState)> {
        self.queue.take_job(job_id)
    }

    pub(in crate::features) fn restore_transfer_job_after_event(
        &mut self,
        queued: (usize, TransferJobState),
    ) {
        self.queue.restore_job(queued);
    }

    pub(in crate::features) fn next_transfer_job_id(&mut self, prefix: &str) -> String {
        self.queue.next_job_id(prefix)
    }

    pub(in crate::features) fn selected_transfer_job_id(&self) -> Option<&str> {
        self.queue.selected_job_id()
    }

    pub(in crate::features) fn select_transfer_job_id(&mut self, job_id: &str) -> bool {
        self.queue.select_job(job_id)
    }

    pub(in crate::features) fn selected_or_latest_visible_transfer_job_id(
        &self,
        session_id: Option<&str>,
    ) -> Option<String> {
        self.queue.selected_or_latest_visible_job_id(session_id)
    }

    pub(in crate::features) fn delete_transfer_job(&mut self, job_id: &str) -> bool {
        let removed = self.queue.remove_job(job_id);
        if removed {
            self.path_options_by_job.remove(job_id);
        }
        removed
    }

    pub(in crate::features) fn transfer_job_menu(&self) -> Option<&TransferJobMenuState> {
        self.queue.job_menu()
    }

    pub(in crate::features) fn open_transfer_job_menu_at(
        &mut self,
        job_id: &str,
        x: Pixels,
        y: Pixels,
    ) -> bool {
        self.queue.open_job_menu(job_id, x, y)
    }

    pub(in crate::features) fn close_transfer_job_menu(&mut self) {
        self.queue.close_job_menu();
    }

    pub(in crate::features) fn reset_transfer_queue_interaction(&mut self) {
        self.queue.reset_interaction();
    }

    pub(in crate::features) fn transfer_job_can_be_deleted(
        &self,
        job_id: &str,
        session_id: Option<&str>,
    ) -> bool {
        self.queue.can_delete_job(job_id, session_id)
    }

    pub(in crate::features) fn pause_visible_transfer_jobs(
        &mut self,
        session_id: Option<&str>,
    ) -> usize {
        let excluded: std::collections::HashSet<_> = self
            .file_operations
            .iter()
            .filter(|(_, context)| !context.can_pause())
            .map(|(id, _)| id.clone())
            .collect();
        self.queue
            .pause_visible_jobs_excluding(session_id, &excluded)
    }

    pub(in crate::features) fn resume_visible_transfer_jobs(
        &mut self,
        session_id: Option<&str>,
    ) -> usize {
        let mut queued = 0;
        for job in &mut self.queue.jobs {
            if self.admission.manages(&job.id)
                && job.is_visible_for_session(session_id)
                && job.status == TransferJobStatus::Paused
            {
                job.status = TransferJobStatus::Queued;
                job.detail = "Queued".into();
                queued += 1;
            }
        }
        queued + self.queue.resume_visible_jobs(session_id)
    }

    pub(in crate::features) fn cancel_visible_transfer_jobs(
        &mut self,
        session_id: Option<&str>,
    ) -> usize {
        self.queue.cancel_visible_jobs(session_id)
    }

    pub(in crate::features) fn bulk_executor(
        &mut self,
    ) -> Result<
        &crate::blocking_jobs::BlockingJobScheduler,
        crate::blocking_jobs::SchedulerStartError,
    > {
        if self.bulk_executor.is_none() {
            self.bulk_executor = Some(crate::blocking_jobs::BlockingJobScheduler::try_new()?);
        }
        Ok(self.bulk_executor.as_ref().expect("executor initialized"))
    }

    pub(in crate::features) fn begin_bulk_shutdown(&mut self) -> impl FnOnce() + Send + 'static {
        self.external_sync.upload_control.cancel();
        for job in &self.queue.jobs {
            if let Some(control) = &job.control {
                control.cancel();
            }
        }
        let cleanup = self
            .bulk_executor
            .as_ref()
            .map(|executor| executor.begin_shutdown());
        let threads = self.admission.shutdown();
        move || {
            if let Some(cleanup) = cleanup {
                cleanup();
            }
            for thread in threads {
                let _ = thread.join();
            }
        }
    }

    pub(in crate::features) fn external_editor_upload_control(&self) -> SftpTransferControl {
        self.external_sync.upload_control.child()
    }

    pub(in crate::features) fn clear_completed_transfer_jobs_for_session(
        &mut self,
        session_id: Option<&str>,
    ) -> usize {
        let removed = self.queue.clear_completed_jobs(session_id);
        self.prune_transfer_job_path_options();
        removed
    }

    pub(in crate::features) fn clear_stopped_transfer_jobs_for_session(
        &mut self,
        session_id: Option<&str>,
    ) -> usize {
        let removed = self.queue.clear_stopped_jobs(session_id);
        self.prune_transfer_job_path_options();
        removed
    }

    fn prune_transfer_job_path_options(&mut self) {
        self.file_operations
            .retain(|id, _| self.queue.jobs.iter().any(|job| job.id == *id));
        self.path_options_by_job
            .retain(|job_id, _| self.queue.jobs().iter().any(|job| job.id == *job_id));
    }

    pub(in crate::features) fn rename_dialog(&self) -> Option<&TransferRenameState> {
        self.file_ops.rename()
    }

    pub(in crate::features) fn rename_dialog_mut(&mut self) -> Option<&mut TransferRenameState> {
        self.file_ops.rename_mut()
    }

    pub(in crate::features) fn rename_dialog_is_open(&self) -> bool {
        self.file_ops.rename().is_some()
    }

    pub(in crate::features) fn open_rename_dialog(&mut self, state: TransferRenameState) {
        self.browser.cancel_pending_rename();
        self.file_ops.open_rename(state);
    }

    pub(in crate::features) fn close_rename_dialog(&mut self) {
        self.browser.cancel_pending_rename();
        self.file_ops.close_rename();
    }

    pub(in crate::features) fn set_rename_value(&mut self, value: String) -> bool {
        self.file_ops.set_rename_value(value)
    }

    pub(in crate::features) fn schedule_rename_focus(&mut self) {
        self.file_ops.schedule_rename_focus();
    }

    pub(in crate::features) fn rename_focus_is_pending(&self) -> bool {
        self.file_ops.rename_focus_pending
    }

    pub(in crate::features) fn pending_rename_input_id(&self) -> Option<String> {
        self.file_ops.pending_rename_input_id()
    }

    pub(in crate::features) fn finish_rename_focus(&mut self) {
        self.file_ops.rename_focus_pending = false;
    }

    pub(in crate::features) fn move_dialog(&self) -> Option<&TransferMoveState> {
        self.file_ops.move_to.as_ref()
    }

    pub(in crate::features) fn open_move_dialog(&mut self, state: TransferMoveState) {
        self.file_ops.move_to = Some(state);
    }

    pub(in crate::features) fn close_move_dialog(&mut self) {
        self.file_ops.move_to = None;
    }

    pub(in crate::features) fn set_move_value(&mut self, value: String) -> bool {
        let Some(state) = self.file_ops.move_to.as_mut() else {
            return false;
        };
        state.value = value;
        true
    }

    pub(in crate::features) fn new_folder_dialog(&self) -> Option<&TransferNewFolderState> {
        self.file_ops.new_folder.as_ref()
    }

    pub(in crate::features) fn open_new_folder_dialog(&mut self, state: TransferNewFolderState) {
        self.file_ops.new_folder = Some(state);
    }

    pub(in crate::features) fn close_new_folder_dialog(&mut self) {
        self.file_ops.new_folder = None;
    }

    pub(in crate::features) fn set_new_folder_name(&mut self, value: String) -> bool {
        let Some(state) = self.file_ops.new_folder.as_mut() else {
            return false;
        };
        state.value = value;
        true
    }

    pub(in crate::features) fn toggle_new_folder_open_after_create(&mut self) -> bool {
        let Some(state) = self.file_ops.new_folder.as_mut() else {
            return false;
        };
        state.open_after_create = !state.open_after_create;
        true
    }

    pub(in crate::features) fn toggle_new_folder_mode_bit(&mut self, bit: u32) -> bool {
        let Some(state) = self.file_ops.new_folder.as_mut() else {
            return false;
        };
        state.mode ^= bit;
        true
    }

    pub(in crate::features) fn new_file_dialog(&self) -> Option<&TransferNewFileState> {
        self.file_ops.new_file.as_ref()
    }

    pub(in crate::features) fn open_new_file_dialog(&mut self, state: TransferNewFileState) {
        self.file_ops.new_file = Some(state);
    }

    pub(in crate::features) fn close_new_file_dialog(&mut self) {
        self.file_ops.new_file = None;
    }

    pub(in crate::features) fn set_new_file_name(&mut self, value: String) -> bool {
        let Some(state) = self.file_ops.new_file.as_mut() else {
            return false;
        };
        state.value = value;
        true
    }

    pub(in crate::features) fn toggle_new_file_open_after_create(&mut self) -> bool {
        let Some(state) = self.file_ops.new_file.as_mut() else {
            return false;
        };
        state.open_after_create = !state.open_after_create;
        true
    }

    pub(in crate::features) fn toggle_new_file_mode_bit(&mut self, bit: u32) -> bool {
        let Some(state) = self.file_ops.new_file.as_mut() else {
            return false;
        };
        state.mode ^= bit;
        true
    }

    pub(in crate::features) fn new_symlink_dialog(&self) -> Option<&TransferNewSymlinkState> {
        self.file_ops.new_symlink.as_ref()
    }

    pub(in crate::features) fn open_new_symlink_dialog(&mut self, state: TransferNewSymlinkState) {
        self.file_ops.new_symlink = Some(state);
    }

    pub(in crate::features) fn close_new_symlink_dialog(&mut self) {
        self.file_ops.new_symlink = None;
    }

    pub(in crate::features) fn set_new_symlink_input(
        &mut self,
        field: crate::models::TransferSymlinkField,
        value: String,
    ) -> bool {
        let Some(state) = self.file_ops.new_symlink.as_mut() else {
            return false;
        };
        state.focused_field = field;
        match field {
            crate::models::TransferSymlinkField::Name => state.name = value,
            crate::models::TransferSymlinkField::Target => state.target = value,
        }
        true
    }

    pub(in crate::features) fn properties_dialog(&self) -> Option<&TransferPropertiesState> {
        self.file_ops.properties.as_ref()
    }

    pub(in crate::features) fn open_properties_dialog(&mut self, state: TransferPropertiesState) {
        self.file_ops.properties = Some(state);
    }

    pub(in crate::features) fn close_properties_dialog(&mut self) {
        self.file_ops.properties = None;
    }

    pub(in crate::features) fn close_properties_dialog_for_session(
        &mut self,
        session_id: &str,
    ) -> bool {
        if !self.file_ops.properties_matches(Some(session_id), None) {
            return false;
        }
        self.file_ops.properties = None;
        true
    }

    pub(in crate::features) fn set_properties_input(
        &mut self,
        field: crate::models::TransferPropertiesField,
        value: String,
    ) -> bool {
        let Some(state) = self.file_ops.properties.as_mut() else {
            return false;
        };
        match field {
            crate::models::TransferPropertiesField::SymlinkTarget => {
                state.symlink_target_value = value
            }
            crate::models::TransferPropertiesField::Mode => state.mode_value = value,
            crate::models::TransferPropertiesField::Owner => state.owner_value = value,
            crate::models::TransferPropertiesField::Group => state.group_value = value,
        }
        state.error = None;
        true
    }

    pub(in crate::features) fn properties_input_values(&self) -> Option<(String, String, String)> {
        self.file_ops.properties.as_ref().map(|state| {
            (
                state.mode_value.clone(),
                state.owner_value.clone(),
                state.group_value.clone(),
            )
        })
    }

    pub(in crate::features) fn set_properties_mode_value(&mut self, value: String) -> bool {
        let Some(state) = self.file_ops.properties.as_mut() else {
            return false;
        };
        state.mode_value = value;
        true
    }

    pub(in crate::features) fn toggle_properties_recursive(&mut self) -> bool {
        let Some(state) = self.file_ops.properties.as_mut() else {
            return false;
        };
        state.recursive = !state.recursive;
        true
    }

    pub(in crate::features) fn set_properties_error(&mut self, error: String) -> bool {
        let Some(state) = self.file_ops.properties.as_mut() else {
            return false;
        };
        state.saving = false;
        state.error = Some(error);
        true
    }

    pub(in crate::features) fn begin_properties_save(&mut self) -> bool {
        let Some(state) = self.file_ops.properties.as_mut() else {
            return false;
        };
        state.saving = true;
        state.error = None;
        true
    }

    pub(in crate::features) fn complete_properties_load(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
        properties: SftpFileProperties,
        mode_value: String,
        owner_value: String,
        group_value: String,
    ) -> bool {
        let Some(state) = self
            .file_ops
            .matching_properties_mut(session_id, remote_path)
        else {
            return false;
        };
        state.symlink_target_value = properties.symlink_target.clone().unwrap_or_default();
        state.mode_value = mode_value;
        state.owner_value = owner_value;
        state.group_value = group_value;
        state.properties = Some(properties);
        state.error = None;
        true
    }

    pub(in crate::features) fn complete_properties_update(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
        properties: SftpFileProperties,
    ) -> bool {
        let Some(state) = self
            .file_ops
            .matching_properties_mut(session_id, remote_path)
        else {
            return false;
        };
        state.properties = Some(properties);
        state.saving = false;
        state.error = None;
        self.file_ops.properties = None;
        true
    }

    pub(in crate::features) fn fail_properties_operation(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
        error: String,
    ) -> bool {
        let Some(state) = self
            .file_ops
            .matching_properties_mut(session_id, remote_path)
        else {
            return false;
        };
        state.saving = false;
        state.error = Some(error);
        true
    }

    pub(in crate::features) fn unknown_file_dialog(&self) -> Option<&TransferUnknownFileState> {
        self.file_ops.unknown_file.as_ref()
    }

    pub(in crate::features) fn open_unknown_file_dialog(
        &mut self,
        state: TransferUnknownFileState,
    ) {
        self.file_ops.unknown_file = Some(state);
    }

    pub(in crate::features) fn close_unknown_file_dialog(&mut self) {
        self.file_ops.unknown_file = None;
    }

    pub(in crate::features) fn take_unknown_file_dialog(
        &mut self,
    ) -> Option<TransferUnknownFileState> {
        self.file_ops.unknown_file.take()
    }

    pub(in crate::features) fn external_sync_focus(&self) -> &FocusHandle {
        &self.external_sync.focus
    }

    pub(in crate::features) fn external_sync_prompt(
        &self,
        prompt_id: &str,
    ) -> Option<&TransferExternalSyncPromptState> {
        self.external_sync.prompts.get(prompt_id)
    }

    pub(in crate::features) fn insert_external_sync_prompt(
        &mut self,
        prompt_id: String,
        prompt: TransferExternalSyncPromptState,
    ) {
        self.external_sync.prompts.insert(prompt_id, prompt);
    }

    pub(in crate::features) fn active_external_sync_prompt(
        &self,
        session_id: &str,
    ) -> Option<(String, TransferExternalSyncPromptState)> {
        self.external_sync
            .prompts
            .iter()
            .find(|(prompt_id, prompt)| {
                prompt.session_id.as_deref() == Some(session_id)
                    && !self.external_sync.windows.contains_key(*prompt_id)
            })
            .map(|(prompt_id, prompt)| (prompt_id.clone(), prompt.clone()))
    }

    pub(in crate::features) fn external_sync_always_uploads(&self, watch_key: &str) -> bool {
        self.external_sync.always_uploads.contains(watch_key)
    }

    pub(in crate::features) fn take_external_sync_prompt_for_upload(
        &mut self,
        prompt_id: &str,
        always_watch_key: Option<String>,
    ) -> Option<TransferExternalSyncPromptState> {
        let prompt = self.external_sync.prompts.remove(prompt_id)?;
        self.external_sync.windows.remove(prompt_id);
        if let Some(watch_key) = always_watch_key {
            self.external_sync.always_uploads.insert(watch_key);
        }
        Some(prompt)
    }

    pub(in crate::features) fn dismiss_external_sync_prompt(&mut self, prompt_id: &str) -> bool {
        let removed = self.external_sync.prompts.remove(prompt_id).is_some();
        self.external_sync.windows.remove(prompt_id);
        removed
    }

    pub(in crate::features) fn clear_external_sync_for_session(
        &mut self,
        session_id: &str,
    ) -> usize {
        let before = self.external_sync.prompts.len();
        self.external_sync
            .prompts
            .retain(|_, prompt| prompt.session_id.as_deref() != Some(session_id));
        let prompts = &self.external_sync.prompts;
        self.external_sync
            .windows
            .retain(|prompt_id, _| prompts.contains_key(prompt_id));
        let watcher_ids: Vec<String> = self
            .external_sync
            .watchers
            .iter()
            .filter(|(_, entry)| entry.session_id.as_deref() == Some(session_id))
            .map(|(job_id, _)| job_id.clone())
            .collect();
        for job_id in watcher_ids {
            self.external_sync.watchers.remove(&job_id);
        }
        before.saturating_sub(self.external_sync.prompts.len())
    }

    pub(in crate::features) fn start_external_editor_watcher(
        &mut self,
        session_id: Option<String>,
        job_id: String,
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: std::path::PathBuf,
        baseline: nyaterm_files::local_revision::LocalFileVersion,
    ) -> Result<(), String> {
        let watcher = ExternalEditorWatcher::spawn(
            job_id.clone(),
            remote_path,
            raw_path_token,
            local_path,
            self.transfer_event_sender(),
            baseline,
            self.worker_cleanup.clone(),
        )
        .map_err(|error| format!("failed to start external editor watcher: {error}"))?;
        self.external_sync.watchers.insert(
            job_id,
            ExternalEditorWatcherEntry {
                session_id,
                _watcher: watcher,
            },
        );
        Ok(())
    }

    pub(in crate::features) fn begin_external_editor_shutdown(
        &mut self,
    ) -> impl Future<Output = ()> + Send + 'static {
        // Dropping each watcher signals stop and registers its join with the
        // same owner as watchers already removed with their sessions.
        self.external_sync.watchers.clear();
        self.worker_cleanup.drain()
    }

    /// The window slot for one prompt, if that prompt still exists.
    ///
    /// Deliberately does not insert: a deferred callback can arrive after the
    /// prompt was answered or its session closed, and creating an entry for a
    /// dead prompt id would leave the map growing entries nothing ever reads.
    pub(in crate::features::transfers) fn external_sync_window_slot(
        &mut self,
        prompt_id: &str,
    ) -> Option<&mut ChildWindowSlot> {
        self.external_sync.windows.get_mut(prompt_id)
    }

    pub(in crate::features::transfers) fn external_sync_window(
        &self,
        prompt_id: &str,
    ) -> Option<NyaWindowHandle> {
        self.external_sync.windows.get(prompt_id)?.handle()
    }

    pub(in crate::features) fn owns_external_sync_window(
        &self,
        handle: gpui::AnyWindowHandle,
    ) -> bool {
        self.external_sync.windows.values().any(|slot| {
            slot.handle()
                .is_some_and(|window| gpui::AnyWindowHandle::from(window) == handle)
        })
    }

    pub(in crate::features) fn external_sync_window_open_is_pending(
        &self,
        prompt_id: &str,
    ) -> bool {
        self.external_sync
            .windows
            .get(prompt_id)
            .is_some_and(ChildWindowSlot::is_pending)
    }

    pub(in crate::features) fn begin_external_sync_window_open(&mut self, prompt_id: &str) -> bool {
        if !self.external_sync.prompts.contains_key(prompt_id) {
            return false;
        }
        self.external_sync
            .windows
            .entry(prompt_id.to_string())
            .or_default()
            .begin_open()
    }

    pub(in crate::features::transfers) fn finish_external_sync_window_open(
        &mut self,
        prompt_id: String,
        handle: NyaWindowHandle,
    ) {
        self.external_sync
            .windows
            .entry(prompt_id)
            .or_default()
            .finish_open(handle);
    }

    pub(in crate::features) fn clear_external_sync_window_tracking(
        &mut self,
        prompt_id: &str,
    ) -> bool {
        self.external_sync
            .windows
            .remove(prompt_id)
            .is_some_and(|slot| slot.is_open_or_pending())
    }

    pub(in crate::features) fn remote_path(&self) -> &str {
        self.paths.remote_path()
    }

    pub(in crate::features) fn set_remote_path(&mut self, path: impl Into<String>) {
        self.paths.set_remote_path(path);
    }

    pub(in crate::features) fn normalized_remote_path(&self) -> String {
        self.paths.normalized_remote_path()
    }

    pub(in crate::features) fn local_path(&self) -> &str {
        self.paths.local_path()
    }

    pub(in crate::features) fn duplicate_policy(&self) -> SftpDuplicatePolicy {
        self.paths.duplicate_policy()
    }

    pub(in crate::features) fn set_duplicate_policy(&mut self, policy: SftpDuplicatePolicy) {
        self.paths.set_duplicate_policy(policy);
    }

    pub(in crate::features) fn path_prompt_is_open(&self) -> bool {
        self.paths.prompt.is_some()
    }

    pub(in crate::features) fn begin_path_prompt(&mut self, kind: TransferPathPromptKind) -> bool {
        self.paths.begin_prompt(kind)
    }

    pub(in crate::features) fn finish_path_prompt(&mut self, kind: TransferPathPromptKind) -> bool {
        self.paths.finish_prompt(kind)
    }

    pub(in crate::features) fn panel_height(&self) -> f32 {
        self.panel.height
    }

    pub(in crate::features) fn start_panel_height_resize(&mut self, start_y: Pixels) {
        self.panel.start_height_resize(start_y);
    }

    pub(in crate::features) fn update_panel_height_resize(
        &mut self,
        current_y: Pixels,
    ) -> Option<f32> {
        self.panel.update_height_resize(current_y)
    }

    pub(in crate::features) fn finish_panel_height_resize(&mut self) -> bool {
        self.panel.finish_height_resize()
    }

    pub(in crate::features) fn panel_height_is_resizing(&self) -> bool {
        self.panel.height_resize.is_some()
    }

    pub(in crate::features) fn replace_session_id(&mut self, old_id: &str, new_id: &str) -> bool {
        if old_id == new_id {
            return false;
        }

        let mut changed = false;
        if let Some(workspace) = &mut self.editor.workspace {
            for document in &mut workspace.tabs {
                if document.session_id.as_deref() == Some(old_id) {
                    document.session_id = Some(new_id.to_string());
                    changed = true;
                }
            }
        }
        for context in self.file_operations.values_mut() {
            let (source, target) = context.endpoints();
            changed |= source.session_id() == Some(old_id) || target.session_id() == Some(old_id);
            context.replace_session(old_id, new_id);
        }
        if self.invalidate_browser_navigation(old_id).is_some() {
            changed = true;
        }
        self.tree.replace_session(old_id, new_id);
        if self.file_ops.delete_refresh_pending.remove(old_id) {
            self.file_ops
                .delete_refresh_pending
                .insert(new_id.to_owned());
            changed = true;
        }
        if let Some(cache) = self.browser.session_cache.remove(old_id) {
            self.browser.session_cache.insert(new_id.to_string(), cache);
            changed = true;
        }

        self.prune_unreferenced_browser_navigation_snapshots();

        for job in &mut self.queue.jobs {
            if let crate::models::TransferJobKind::SendTo {
                target_session_id, ..
            } = &mut job.kind
                && target_session_id == old_id
            {
                *target_session_id = new_id.to_string();
                changed = true;
            }
            if (job.is_user_transfer()
                || matches!(job.kind, crate::models::TransferJobKind::Delete { .. }))
                && job.session_id.as_deref() == Some(old_id)
            {
                job.session_id = Some(new_id.to_string());
                changed = true;
            }
        }

        changed
    }

    fn prune_unreferenced_browser_navigation_snapshots(&mut self) {
        let referenced: HashSet<&str> = self
            .browser
            .navigation_jobs
            .values()
            .map(String::as_str)
            .collect();
        self.browser
            .pending_navigations
            .retain(|job_id, _| referenced.contains(job_id.as_str()));
    }
}

impl TransferFileOpsState {
    fn new() -> Self {
        Self {
            delete_batches: HashMap::new(),
            delete_refresh_pending: HashSet::new(),
            rename: None,
            rename_focus_pending: false,
            move_to: None,
            new_folder: None,
            new_file: None,
            new_symlink: None,
            properties: None,
            unknown_file: None,
        }
    }

    fn rename(&self) -> Option<&TransferRenameState> {
        self.rename.as_ref()
    }

    fn rename_mut(&mut self) -> Option<&mut TransferRenameState> {
        self.rename.as_mut()
    }

    fn open_rename(&mut self, state: TransferRenameState) {
        self.rename = Some(state);
        self.rename_focus_pending = false;
    }

    fn close_rename(&mut self) {
        self.rename = None;
        self.rename_focus_pending = false;
    }

    fn set_rename_value(&mut self, value: String) -> bool {
        let Some(state) = self.rename.as_mut() else {
            return false;
        };
        state.value = value;
        true
    }

    fn schedule_rename_focus(&mut self) {
        self.rename_focus_pending = self.rename.is_some();
    }

    fn pending_rename_input_id(&self) -> Option<String> {
        self.rename_focus_pending
            .then_some(self.rename.as_ref())
            .flatten()
            .map(|state| format!("transfer.rename.{}", state.old_path))
    }

    fn properties_matches(&self, session_id: Option<&str>, remote_path: Option<&str>) -> bool {
        self.properties.as_ref().is_some_and(|state| {
            state.session_id.as_deref() == session_id
                && remote_path.is_none_or(|path| state.entry.path == path)
        })
    }

    fn matching_properties_mut(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
    ) -> Option<&mut TransferPropertiesState> {
        self.properties.as_mut().filter(|state| {
            state.session_id.as_deref() == session_id && state.entry.path == remote_path
        })
    }
}

impl TransferExternalSyncState {
    fn new(focus: FocusHandle) -> Self {
        Self {
            prompts: HashMap::new(),
            windows: HashMap::new(),
            always_uploads: HashSet::new(),
            watchers: HashMap::new(),
            upload_control: SftpTransferControl::new(),
            focus,
        }
    }
}

impl Drop for TransferFeatureState {
    fn drop(&mut self) {
        let stop_bulk = self.begin_bulk_shutdown();
        self.external_sync.watchers.clear();
        self.worker_cleanup.submit(stop_bulk);
    }
}

impl TransferPathState {
    fn new(remote: String, local: String, duplicate_policy: SftpDuplicatePolicy) -> Self {
        Self {
            remote,
            local,
            duplicate_policy,
            prompt: None,
        }
    }

    fn remote_path(&self) -> &str {
        &self.remote
    }

    fn set_remote_path(&mut self, path: impl Into<String>) {
        self.remote = path.into();
    }

    fn normalized_remote_path(&self) -> String {
        let path = self.remote.trim();
        if path.is_empty() {
            ".".to_string()
        } else {
            path.to_string()
        }
    }

    fn local_path(&self) -> &str {
        &self.local
    }

    #[cfg(test)]
    fn set_local_path(&mut self, path: impl Into<String>) {
        self.local = path.into();
    }

    fn duplicate_policy(&self) -> SftpDuplicatePolicy {
        self.duplicate_policy
    }

    fn set_duplicate_policy(&mut self, policy: SftpDuplicatePolicy) {
        self.duplicate_policy = policy;
    }

    fn begin_prompt(&mut self, kind: TransferPathPromptKind) -> bool {
        if self.prompt.is_some() {
            return false;
        }
        self.prompt = Some(kind);
        true
    }

    fn finish_prompt(&mut self, kind: TransferPathPromptKind) -> bool {
        if self.prompt != Some(kind) {
            return false;
        }
        self.prompt = None;
        true
    }
}

impl TransferPanelState {
    const HEIGHT_MIN: f32 = 60.;
    const HEIGHT_MAX: f32 = 600.;

    fn start_height_resize(&mut self, start_y: Pixels) {
        self.height_resize = Some(TransferHeightResizeState {
            start_y,
            start_height: gpui::px(self.height),
        });
    }

    fn update_height_resize(&mut self, current_y: Pixels) -> Option<f32> {
        let state = self.height_resize?;
        let delta = f32::from(current_y - state.start_y);
        self.height =
            (f32::from(state.start_height) - delta).clamp(Self::HEIGHT_MIN, Self::HEIGHT_MAX);
        Some(self.height)
    }

    fn finish_height_resize(&mut self) -> bool {
        self.height_resize.take().is_some()
    }
}

impl TransferQueueState {
    fn new(
        tx: UnboundedSender<TransferJobResult>,
        rx: UnboundedReceiver<TransferJobResult>,
        focus: FocusHandle,
    ) -> Self {
        Self {
            tx,
            rx: Some(rx),
            jobs: Vec::new(),
            next_job_sequence: 0,
            selected_job_id: None,
            job_menu: None,
            focus,
        }
    }

    fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    fn jobs(&self) -> &[TransferJobState] {
        &self.jobs
    }

    fn job(&self, job_id: &str) -> Option<&TransferJobState> {
        self.jobs.iter().find(|job| job.id == job_id)
    }

    fn job_mut(&mut self, job_id: &str) -> Option<&mut TransferJobState> {
        self.jobs.iter_mut().find(|job| job.id == job_id)
    }

    fn visit_jobs_mut(&mut self, visit: impl FnMut(&mut TransferJobState)) {
        self.jobs.iter_mut().for_each(visit);
    }

    fn enqueue(&mut self, job: TransferJobState) {
        self.jobs.push(job);
    }

    fn event_sender(&self) -> UnboundedSender<TransferJobResult> {
        self.tx.clone()
    }

    fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<TransferJobResult>> {
        self.rx.take()
    }

    fn remove_job(&mut self, job_id: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|job| job.id != job_id);
        let removed = self.jobs.len() != before;
        if removed {
            self.clear_job_interaction(job_id);
        }
        removed
    }

    fn take_job(&mut self, job_id: &str) -> Option<(usize, TransferJobState)> {
        let index = self.jobs.iter().position(|job| job.id == job_id)?;
        Some((index, self.jobs.remove(index)))
    }

    fn restore_job(&mut self, queued: (usize, TransferJobState)) {
        let (index, job) = queued;
        self.jobs.insert(index.min(self.jobs.len()), job);
    }

    fn next_job_id(&mut self, prefix: &str) -> String {
        self.next_job_sequence = self.next_job_sequence.max(self.jobs.len() as u64) + 1;
        format!("{prefix}-{}", self.next_job_sequence)
    }

    fn selected_job_id(&self) -> Option<&str> {
        self.selected_job_id.as_deref()
    }

    fn select_job(&mut self, job_id: &str) -> bool {
        if self.job(job_id).is_none() {
            return false;
        }
        self.selected_job_id = Some(job_id.to_string());
        true
    }

    fn selected_or_latest_visible_job_id(&self, session_id: Option<&str>) -> Option<String> {
        self.selected_job_id
            .as_ref()
            .filter(|job_id| {
                self.job(job_id)
                    .is_some_and(|job| job.is_visible_for_session(session_id))
            })
            .cloned()
            .or_else(|| {
                self.jobs
                    .iter()
                    .rev()
                    .find(|job| job.is_visible_for_session(session_id))
                    .map(|job| job.id.clone())
            })
    }

    fn job_menu(&self) -> Option<&TransferJobMenuState> {
        self.job_menu.as_ref()
    }

    fn open_job_menu(&mut self, job_id: &str, x: Pixels, y: Pixels) -> bool {
        if !self.select_job(job_id) {
            self.job_menu = None;
            return false;
        }
        self.job_menu = Some(TransferJobMenuState {
            job_id: job_id.to_string(),
            x,
            y,
        });
        true
    }

    fn close_job_menu(&mut self) {
        self.job_menu = None;
    }

    fn reset_interaction(&mut self) {
        self.selected_job_id = None;
        self.job_menu = None;
    }

    fn clear_job_interaction(&mut self, job_id: &str) {
        if self.selected_job_id.as_deref() == Some(job_id) {
            self.selected_job_id = None;
        }
        if self
            .job_menu
            .as_ref()
            .is_some_and(|menu| menu.job_id == job_id)
        {
            self.job_menu = None;
        }
    }

    fn prune_missing_interaction(&mut self) {
        let selected_missing = self
            .selected_job_id
            .as_deref()
            .is_some_and(|job_id| self.job(job_id).is_none());
        let menu_missing = self
            .job_menu
            .as_ref()
            .is_some_and(|menu| self.job(&menu.job_id).is_none());
        if selected_missing {
            self.selected_job_id = None;
        }
        if menu_missing {
            self.job_menu = None;
        }
    }

    fn can_delete_job(&self, job_id: &str, session_id: Option<&str>) -> bool {
        self.job(job_id).is_some_and(|job| {
            job.is_visible_for_session(session_id)
                && !matches!(
                    job.status,
                    TransferJobStatus::Queued
                        | TransferJobStatus::Running
                        | TransferJobStatus::Paused
                        | TransferJobStatus::Cancelling
                )
        })
    }

    #[cfg(test)]
    fn pause_visible_jobs(&mut self, session_id: Option<&str>) -> usize {
        self.pause_visible_jobs_excluding(session_id, &HashSet::new())
    }

    fn pause_visible_jobs_excluding(
        &mut self,
        session_id: Option<&str>,
        excluded: &HashSet<String>,
    ) -> usize {
        let mut changed = 0;
        for job in &mut self.jobs {
            if !excluded.contains(&job.id)
                && job.is_visible_for_session(session_id)
                && matches!(
                    job.status,
                    TransferJobStatus::Queued | TransferJobStatus::Running
                )
                && let Some(control) = job.control.as_ref()
            {
                control.pause();
                job.status = TransferJobStatus::Paused;
                job.speed.reset();
                job.detail = "Paused".to_string();
                changed += 1;
            }
        }
        changed
    }

    fn resume_visible_jobs(&mut self, session_id: Option<&str>) -> usize {
        let mut changed = 0;
        for job in &mut self.jobs {
            if job.is_visible_for_session(session_id)
                && job.status == TransferJobStatus::Paused
                && let Some(control) = job.control.as_ref()
            {
                control.resume();
                job.status = TransferJobStatus::Running;
                job.speed.reset();
                job.detail = "Resuming".to_string();
                changed += 1;
            }
        }
        changed
    }

    fn cancel_visible_jobs(&mut self, session_id: Option<&str>) -> usize {
        let mut changed = 0;
        for job in &mut self.jobs {
            if job.is_visible_for_session(session_id)
                && matches!(
                    job.status,
                    TransferJobStatus::Queued
                        | TransferJobStatus::Running
                        | TransferJobStatus::Paused
                )
                && let Some(control) = job.control.as_ref()
            {
                control.cancel();
                job.status = TransferJobStatus::Cancelling;
                job.detail = "Cancelling".to_string();
                changed += 1;
            }
        }
        changed
    }

    fn clear_completed_jobs(&mut self, session_id: Option<&str>) -> usize {
        let before = self.jobs.len();
        self.jobs.retain(|job| {
            !job.is_visible_for_session(session_id) || job.status != TransferJobStatus::Completed
        });
        let removed = before.saturating_sub(self.jobs.len());
        self.prune_missing_interaction();
        removed
    }

    fn clear_stopped_jobs(&mut self, session_id: Option<&str>) -> usize {
        let before = self.jobs.len();
        self.jobs.retain(|job| {
            !job.is_visible_for_session(session_id)
                || matches!(
                    job.status,
                    TransferJobStatus::Queued
                        | TransferJobStatus::Running
                        | TransferJobStatus::Paused
                        | TransferJobStatus::Cancelling
                )
        });
        let removed = before.saturating_sub(self.jobs.len());
        self.prune_missing_interaction();
        removed
    }
}

#[cfg(test)]
mod tests;

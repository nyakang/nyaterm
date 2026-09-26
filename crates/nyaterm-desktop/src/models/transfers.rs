use gpui::{Pixels, ScrollHandle, UniformListScrollHandle, px};
use nyaterm_core::transfer_speed::TransferSpeed;
use nyaterm_transport::{
    RemoteTextDocument, RemoteTextGeneration, RemoteTextWriteResult, SftpFileEntry,
    SftpFileProperties, SftpRemoteTextFile, SftpTransferControl, SftpTransferProgress,
    SftpTransferSummary,
};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferJobKind {
    ListDir {
        remote_path: String,
        select_after: Option<String>,
    },
    ListChildren {
        remote_path: String,
    },
    ListTree {
        path: nyaterm_transport::RemoteFilePath,
        generation: u64,
    },
    ResolveHome,
    SyncCwd,
    Download {
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
    },
    Upload {
        local_path: PathBuf,
        remote_path: String,
    },
    Rename {
        old_path: String,
        new_path: String,
        parent_path: String,
    },
    Move {
        old_path: String,
        new_path: String,
        parent_path: String,
    },
    Delete {
        remote_path: String,
        parent_path: String,
    },
    /// Copy a remote entry from the active (source) session to another connected
    /// SSH session's directory. Reuses `FileCopyRequest` under the hood; the copy
    /// runs SFTP-to-SFTP directly or via local staging for mixed backends.
    SendTo {
        source_path: String,
        target_session_id: String,
        target_path: String,
        target_parent_path: String,
    },
    Mkdir {
        remote_path: String,
        parent_path: String,
    },
    CreateFile {
        remote_path: String,
        parent_path: String,
    },
    Symlink {
        link_path: String,
        target_path: String,
        parent_path: String,
    },
    LoadProperties {
        remote_path: String,
    },
    UpdateProperties {
        remote_path: String,
        parent_path: String,
    },
    LoadEditor {
        remote_path: String,
        tab_id: String,
        generation: RemoteTextGeneration,
    },
    LoadPreview {
        remote_path: String,
        tab_id: String,
        generation: RemoteTextGeneration,
    },
    /// Rasterize a single PDF page for a preview tab, on demand. Only the pages
    /// the user actually views are rendered, keyed by `generation` so a result
    /// for a superseded generation (refresh/rotate) is discarded.
    RasterizePdfPage {
        remote_path: String,
        tab_id: String,
        generation: RemoteTextGeneration,
        page_index: usize,
    },
    SaveEditor {
        remote_path: String,
        tab_id: String,
        generation: RemoteTextGeneration,
    },
    OpenExternal {
        remote_path: String,
        local_path: PathBuf,
    },
    AiFileAction {
        remote_path: String,
        action_id: String,
        action_name: String,
    },
    /// In-band ZMODEM upload (local files -> remote `rz`).
    ZmodemUpload {
        session_id: String,
        file_name: String,
    },
    XmodemUpload {
        session_id: String,
        file_name: String,
    },
    YmodemUpload {
        session_id: String,
        file_name: String,
    },
    /// In-band ZMODEM download (remote `sz` -> local directory).
    ZmodemDownload {
        session_id: String,
        file_name: String,
    },
    /// In-band trzsz download (remote `tsz` -> local download directory).
    TrzszDownload {
        session_id: String,
        file_name: String,
    },
    RdpClipboard {
        file_name: String,
    },
    /// In-band trzsz upload (local files -> remote `trz`).
    TrzszUpload {
        session_id: String,
        file_name: String,
    },
    /// Pre-upload SFTP name conflict probe before remote `rz` (Tauri parity).
    ZmodemConflictProbe {
        session_id: String,
        remote_dir: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferJobStatus {
    Running,
    Paused,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub(crate) struct TransferJobState {
    pub(crate) id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) kind: TransferJobKind,
    pub(crate) status: TransferJobStatus,
    pub(crate) detail: String,
    pub(crate) created_at_ms: u128,
    pub(crate) display_name: String,
    pub(crate) entries: Vec<SftpFileEntry>,
    pub(crate) summary: Option<SftpTransferSummary>,
    pub(crate) progress: Option<SftpTransferProgress>,
    pub(crate) control: Option<SftpTransferControl>,
    pub(crate) speed: TransferSpeed,
}

/// What a queue row actually draws.
///
/// `TransferJobState` carries `entries`, the whole directory listing a navigation
/// job returned, and the queue used to deep-copy every visible job twice per render
/// -- once to filter and once to sort. No row reads that field. This carries the
/// presentation fields it does read, so a snapshot rebuilt on every coalesced progress batch
/// costs a handful of small clones instead of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferJobRowSnapshot {
    pub(crate) id: String,
    pub(crate) kind: TransferJobKind,
    pub(crate) status: TransferJobStatus,
    pub(crate) detail: String,
    pub(crate) display_name: String,
    pub(crate) created_at_ms: u128,
    pub(crate) progress: Option<SftpTransferProgress>,
    pub(crate) summary: Option<SftpTransferSummary>,
    pub(crate) speed_bytes_per_sec: f64,
}

impl TransferJobState {
    /// The row projection, dropping `entries`, `session_id` and the control handle.
    pub(crate) fn row_snapshot(&self) -> TransferJobRowSnapshot {
        TransferJobRowSnapshot {
            id: self.id.clone(),
            kind: self.kind.clone(),
            status: self.status,
            detail: self.detail.clone(),
            display_name: self.display_name.clone(),
            created_at_ms: self.created_at_ms,
            progress: self.progress.clone(),
            summary: self.summary.clone(),
            speed_bytes_per_sec: self.speed.bytes_per_second(),
        }
    }

    pub(crate) fn update_progress(&mut self, progress: SftpTransferProgress) {
        if self.status == TransferJobStatus::Running {
            self.speed
                .record(progress.bytes_transferred, Instant::now());
        }
        self.progress = Some(progress);
    }

    pub(crate) fn now_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0)
    }

    pub(crate) fn display_name_for_kind(kind: &TransferJobKind) -> String {
        match kind {
            TransferJobKind::ListTree { path, .. } => remote_file_name(&path.display_path),
            TransferJobKind::Download { remote_path, .. }
            | TransferJobKind::OpenExternal { remote_path, .. }
            | TransferJobKind::LoadEditor { remote_path, .. }
            | TransferJobKind::LoadPreview { remote_path, .. }
            | TransferJobKind::RasterizePdfPage { remote_path, .. }
            | TransferJobKind::SaveEditor { remote_path, .. }
            | TransferJobKind::LoadProperties { remote_path }
            | TransferJobKind::UpdateProperties { remote_path, .. }
            | TransferJobKind::AiFileAction { remote_path, .. }
            | TransferJobKind::Delete { remote_path, .. }
            | TransferJobKind::Mkdir { remote_path, .. }
            | TransferJobKind::CreateFile { remote_path, .. }
            | TransferJobKind::Symlink {
                link_path: remote_path,
                ..
            }
            | TransferJobKind::ListChildren { remote_path }
            | TransferJobKind::ListDir { remote_path, .. } => remote_file_name(remote_path),
            TransferJobKind::Upload { local_path, .. } => local_file_name(local_path),
            TransferJobKind::Rename { new_path, .. } | TransferJobKind::Move { new_path, .. } => {
                remote_file_name(new_path)
            }
            TransferJobKind::SendTo { target_path, .. } => remote_file_name(target_path),
            TransferJobKind::ZmodemUpload { file_name, .. }
            | TransferJobKind::XmodemUpload { file_name, .. }
            | TransferJobKind::YmodemUpload { file_name, .. }
            | TransferJobKind::ZmodemDownload { file_name, .. }
            | TransferJobKind::TrzszDownload { file_name, .. }
            | TransferJobKind::RdpClipboard { file_name }
            | TransferJobKind::TrzszUpload { file_name, .. } => file_name.clone(),
            TransferJobKind::ResolveHome | TransferJobKind::SyncCwd => String::new(),
            TransferJobKind::ZmodemConflictProbe { remote_dir, .. } => remote_file_name(remote_dir),
        }
    }

    pub(crate) fn ensure_presentation_fields(&mut self) {
        if self.created_at_ms == 0 {
            self.created_at_ms = Self::now_ms();
        }
        if self.display_name.trim().is_empty() {
            self.display_name = Self::display_name_for_kind(&self.kind);
        }
    }

    pub(crate) fn is_user_transfer(&self) -> bool {
        matches!(
            &self.kind,
            TransferJobKind::Download { .. }
                | TransferJobKind::Upload { .. }
                | TransferJobKind::SendTo { .. }
                | TransferJobKind::OpenExternal { .. }
                | TransferJobKind::ZmodemUpload { .. }
                | TransferJobKind::XmodemUpload { .. }
                | TransferJobKind::YmodemUpload { .. }
                | TransferJobKind::ZmodemDownload { .. }
                | TransferJobKind::TrzszDownload { .. }
                | TransferJobKind::RdpClipboard { .. }
                | TransferJobKind::TrzszUpload { .. }
        )
    }

    pub(crate) fn is_visible_for_session(&self, session_id: Option<&str>) -> bool {
        self.is_user_transfer()
            && session_id.is_none_or(|session_id| self.session_id.as_deref() == Some(session_id))
    }
}

fn remote_file_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn local_file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod transfer_job_state_tests {
    use std::path::PathBuf;

    use super::{TransferJobKind, TransferJobState, TransferJobStatus};

    fn job(kind: TransferJobKind, session_id: Option<&str>) -> TransferJobState {
        TransferJobState {
            id: "job-1".to_string(),
            session_id: session_id.map(ToString::to_string),
            kind,
            status: TransferJobStatus::Completed,
            detail: String::new(),
            created_at_ms: 1,
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        }
    }

    #[test]
    fn progress_updates_seed_speed_and_snapshot_projects_the_sampled_rate() {
        use std::time::{Duration, Instant};

        use nyaterm_transport::SftpTransferProgress;

        for kind in [
            TransferJobKind::Download {
                remote_path: "/remote/file".to_string(),
                raw_path_token: None,
                local_path: PathBuf::from("file"),
            },
            TransferJobKind::ZmodemDownload {
                session_id: "session-a".to_string(),
                file_name: "file".to_string(),
            },
            TransferJobKind::TrzszDownload {
                session_id: "session-a".to_string(),
                file_name: "file".to_string(),
            },
        ] {
            let mut job = job(kind, Some("session-a"));
            job.status = TransferJobStatus::Running;
            job.update_progress(SftpTransferProgress {
                remote_path: "/remote/file".to_string(),
                local_path: PathBuf::from("file"),
                bytes_transferred: 0,
                total_bytes: None,
                item_count_completed: None,
                item_count_total: None,
            });
            assert_eq!(job.row_snapshot().speed_bytes_per_sec, 0.);
            job.speed
                .record(2048, Instant::now() + Duration::from_secs(1));
            assert!(
                job.speed.bytes_per_second() > 0.,
                "progress must seed sampling"
            );
            let snapshot = job.row_snapshot();
            assert_eq!(snapshot.speed_bytes_per_sec, job.speed.bytes_per_second());
            assert_eq!(snapshot.progress, job.progress);
        }
    }

    #[test]
    fn user_transfers_are_scoped_to_the_active_session() {
        let download = job(
            TransferJobKind::Download {
                remote_path: "/remote/file".to_string(),
                raw_path_token: None,
                local_path: PathBuf::from("/local/file"),
            },
            Some("session-a"),
        );

        assert!(download.is_visible_for_session(None));
        assert!(download.is_visible_for_session(Some("session-a")));
        assert!(!download.is_visible_for_session(Some("session-b")));
    }

    #[test]
    fn internal_jobs_do_not_appear_in_the_transfer_queue() {
        let list = job(
            TransferJobKind::ListDir {
                remote_path: "/remote".to_string(),
                select_after: None,
            },
            Some("session-a"),
        );

        assert!(!list.is_user_transfer());
        assert!(!list.is_visible_for_session(None));
        assert!(!list.is_visible_for_session(Some("session-a")));
    }

    #[test]
    fn external_editor_downloads_are_visible_user_transfers() {
        let external = job(
            TransferJobKind::OpenExternal {
                remote_path: "/remote/file.txt".to_string(),
                local_path: PathBuf::from("/tmp/file.txt"),
            },
            Some("session-a"),
        );

        assert!(external.is_user_transfer());
        assert!(external.is_visible_for_session(None));
        assert!(external.is_visible_for_session(Some("session-a")));
        assert!(!external.is_visible_for_session(Some("session-b")));
    }

    #[test]
    fn send_to_jobs_are_visible_user_transfers_scoped_to_the_source_session() {
        let send = job(
            TransferJobKind::SendTo {
                source_path: "/remote/file.txt".to_string(),
                target_session_id: "session-b".to_string(),
                target_path: "/srv/file.txt".to_string(),
                target_parent_path: "/srv".to_string(),
            },
            Some("session-a"),
        );

        assert!(send.is_user_transfer());
        assert!(send.is_visible_for_session(None));
        // Scoped to the source session's queue, not the target session.
        assert!(send.is_visible_for_session(Some("session-a")));
        assert!(!send.is_visible_for_session(Some("session-b")));
        assert_eq!(
            TransferJobState::display_name_for_kind(&send.kind),
            "file.txt"
        );
    }

    #[test]
    fn display_names_are_stable_from_initial_transfer_kind() {
        assert_eq!(
            TransferJobState::display_name_for_kind(&TransferJobKind::OpenExternal {
                remote_path: "/remote/project/file.txt".to_string(),
                local_path: PathBuf::from("/tmp/file.txt"),
            }),
            "file.txt"
        );
        assert_eq!(
            TransferJobState::display_name_for_kind(&TransferJobKind::Download {
                remote_path: "/remote/project/".to_string(),
                raw_path_token: None,
                local_path: PathBuf::from("/tmp/project"),
            }),
            "project"
        );
        assert_eq!(
            TransferJobState::display_name_for_kind(&TransferJobKind::Upload {
                local_path: PathBuf::from("/tmp/local.bin"),
                remote_path: "/remote/local.bin".to_string(),
            }),
            "local.bin"
        );
    }
}

#[derive(Debug)]
pub(crate) struct TransferJobResult {
    pub(crate) id: String,
    pub(crate) event: TransferJobEvent,
}

#[derive(Debug)]
pub(crate) enum TransferJobEvent {
    Started {
        detail: String,
    },
    ExternalModified {
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
    },
    Progress(SftpTransferProgress),
    Finished(Result<TransferJobOutput, String>),
}

#[derive(Debug)]
pub(crate) enum TransferJobOutput {
    Entries(Vec<SftpFileEntry>),
    TreeEntries(Vec<SftpFileEntry>),
    ChildEntries {
        remote_path: String,
        entries: Vec<SftpFileEntry>,
    },
    HomeDir(String),
    CwdSynced {
        remote_path: String,
        entries: Vec<SftpFileEntry>,
    },
    Summary(SftpTransferSummary),
    Uploaded {
        summary: SftpTransferSummary,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
    },
    Renamed {
        old_path: String,
        new_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
    },
    Moved {
        old_path: String,
        new_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
    },
    Deleted {
        remote_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
    },
    /// A cross-session "Send to" copy finished. `entries` is the refreshed listing
    /// of the target session's destination directory, used to update that
    /// session's browser cache without switching the active session.
    Sent {
        source_path: String,
        /// Set only after a verified cut has deleted the source.
        source_parent_path: Option<String>,
        source_entries: Option<Vec<SftpFileEntry>>,
        target_session_id: String,
        target_path: String,
        target_parent_path: String,
        bytes: u64,
        used_local_staging: bool,
        entries: Vec<SftpFileEntry>,
    },
    CreatedDirectory {
        remote_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
        open_after_create: bool,
    },
    CreatedFile {
        remote_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
        open_after_create: bool,
    },
    CreatedSymlink {
        link_path: String,
        target_path: String,
        parent_path: String,
        entries: Vec<SftpFileEntry>,
    },
    PropertiesLoaded {
        remote_path: String,
        properties: SftpFileProperties,
    },
    PropertiesUpdated {
        remote_path: String,
        parent_path: String,
        properties: SftpFileProperties,
        entries: Vec<SftpFileEntry>,
    },
    EditorLoaded {
        tab_id: String,
        remote_path: String,
        generation: RemoteTextGeneration,
        file: RemoteTextDocument,
    },
    PreviewLoaded {
        tab_id: String,
        remote_path: String,
        generation: RemoteTextGeneration,
        content: crate::models::PreviewContent,
    },
    /// A single lazily-rasterized PDF page finished rendering.
    PdfPageRendered {
        tab_id: String,
        generation: RemoteTextGeneration,
        page_index: usize,
        page: Result<crate::models::PreviewPdfPage, String>,
    },
    EditorSaved {
        tab_id: String,
        remote_path: String,
        generation: RemoteTextGeneration,
        result: RemoteTextWriteResult,
    },
    ExternalOpened {
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
    },
    AiFileActionLoaded {
        remote_path: String,
        action_id: String,
        action_name: String,
        prompt: String,
        file: SftpRemoteTextFile,
    },

    ZmodemProbeReady {
        session_id: String,
        files: Vec<PathBuf>,
        probe_skipped: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferBrowserSessionCacheState {
    /// Shared, and always replaced whole.
    ///
    /// The browser listing is swapped in and out of caches and navigation snapshots,
    /// and the filter/sort memo keys on this pointer. Both want the same thing: one
    /// allocation handed around rather than deep-copied, and a pointer that changes
    /// exactly when the listing does.
    pub(crate) entries: Arc<Vec<SftpFileEntry>>,
    pub(crate) current_path: String,
    pub(crate) current_raw_path_token: Option<String>,
    pub(crate) home_dir: String,
    pub(crate) history: VecDeque<String>,
    pub(crate) history_index: usize,
    pub(crate) visited_history: VecDeque<String>,
}

#[derive(Clone)]
pub(crate) struct TransferBrowserNavigationSnapshot {
    pub(crate) remote_path: String,
    pub(crate) browser_path: String,
    pub(crate) browser_raw_path_token: Option<String>,
    /// Shared, and always replaced whole.
    ///
    /// The browser listing is swapped in and out of caches and navigation snapshots,
    /// and the filter/sort memo keys on this pointer. Both want the same thing: one
    /// allocation handed around rather than deep-copied, and a pointer that changes
    /// exactly when the listing does.
    pub(crate) entries: Arc<Vec<SftpFileEntry>>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) status: String,
    pub(crate) history: VecDeque<String>,
    pub(crate) history_index: usize,
    pub(crate) visited_history: VecDeque<String>,
    pub(crate) selected_path: Option<String>,
    pub(crate) selected_paths: HashSet<String>,
    pub(crate) list_scroll: UniformListScrollHandle,
    pub(crate) horizontal_scroll: ScrollHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferBrowserSortColumn {
    Name,
    Modified,
    Size,
    Permissions,
    Owner,
    Group,
}

impl TransferBrowserSortColumn {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Size => "Size",
            Self::Permissions => "Perms",
            Self::Owner => "Owner",
            Self::Group => "Group",
        }
    }

    pub(crate) fn default_direction(self) -> TransferBrowserSortDirection {
        match self {
            Self::Name | Self::Permissions | Self::Owner | Self::Group => {
                TransferBrowserSortDirection::Ascending
            }
            Self::Size | Self::Modified => TransferBrowserSortDirection::Descending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferBrowserSortDirection {
    Ascending,
    Descending,
}

impl TransferBrowserSortDirection {
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::Ascending => "up",
            Self::Descending => "down",
        }
    }

    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransferBrowserColumnWidths {
    pub(crate) name: Pixels,
    pub(crate) modified: Pixels,
    pub(crate) size: Pixels,
    pub(crate) permissions: Pixels,
    pub(crate) owner: Pixels,
    pub(crate) group: Pixels,
}

impl Default for TransferBrowserColumnWidths {
    fn default() -> Self {
        Self {
            name: px(220.),
            modified: px(128.),
            size: px(80.),
            permissions: px(112.),
            owner: px(96.),
            group: px(96.),
        }
    }
}

impl TransferBrowserColumnWidths {
    pub(crate) fn get(self, column: TransferBrowserSortColumn) -> Pixels {
        match column {
            TransferBrowserSortColumn::Name => self.name,
            TransferBrowserSortColumn::Modified => self.modified,
            TransferBrowserSortColumn::Size => self.size,
            TransferBrowserSortColumn::Permissions => self.permissions,
            TransferBrowserSortColumn::Owner => self.owner,
            TransferBrowserSortColumn::Group => self.group,
        }
    }

    pub(crate) fn set(&mut self, column: TransferBrowserSortColumn, width: Pixels) {
        let width = if width < Self::min_width(column) {
            Self::min_width(column)
        } else {
            width
        };
        match column {
            TransferBrowserSortColumn::Name => self.name = width,
            TransferBrowserSortColumn::Modified => self.modified = width,
            TransferBrowserSortColumn::Size => self.size = width,
            TransferBrowserSortColumn::Permissions => self.permissions = width,
            TransferBrowserSortColumn::Owner => self.owner = width,
            TransferBrowserSortColumn::Group => self.group = width,
        }
    }

    pub(crate) fn min_width(column: TransferBrowserSortColumn) -> Pixels {
        match column {
            TransferBrowserSortColumn::Name => px(140.),
            TransferBrowserSortColumn::Modified => px(112.),
            TransferBrowserSortColumn::Size => px(72.),
            TransferBrowserSortColumn::Permissions => px(92.),
            TransferBrowserSortColumn::Owner => px(76.),
            TransferBrowserSortColumn::Group => px(76.),
        }
    }
}

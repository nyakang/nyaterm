use crate::features::NyaTermApp;
use crate::features::session::UploadBatchDuplicateResolver;
use crate::features::transfers::FileTransferSession;
use crate::models::{TransferPathPromptKind, TransferPathPromptResult};
use gpui::{Context, PathPromptOptions, SharedString, Window};
use nyaterm_core::truncate_preview;
use nyaterm_files::file_browser::FileTransferEntryError;
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
};
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct PendingBrowserUpload {
    kind: TransferPathPromptKind,
    remote_path: String,
    session_id: Option<String>,
    service: nyaterm_files::file_browser::FileBrowserService,
    path_options: SftpPathTransferOptions,
}

struct BrowserUploadRequest {
    paths: Vec<PathBuf>,
    remote_path: String,
    session_id: Option<String>,
    service: nyaterm_files::file_browser::FileBrowserService,
    path_options: SftpPathTransferOptions,
    fallback_name: &'static str,
}

impl NyaTermApp {
    pub(in crate::features) fn prompt_transfer_download_path_setting(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.path_prompt_is_open() {
            self.report_browser_transfer_error(
                "path-picker",
                &FileTransferEntryError::PickerAlreadyOpen.into(),
                cx,
            );
            return;
        }
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Select default download directory")),
        };
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting default download directory".to_string());
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = path {
                    let path = path.display().to_string();
                    this.settings.set_transfer_download_path(path.clone());
                    this.reset_text_input("settings.transfer.download-path", &path, cx);
                    this.save_transfer_settings("transfer download path saved", cx);
                } else {
                    this.shell
                        .set_status("download path selection cancelled".to_string());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn prompt_recording_path_setting(&mut self, cx: &mut Context<Self>) {
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Select recording directory")),
        };
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting recording directory".to_string());
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = path {
                    let path = path.display().to_string();
                    this.settings.set_recording_path(path.clone());
                    this.reset_text_input("settings.recording.path", &path, cx);
                    this.save_recording_settings(cx);
                } else {
                    this.shell
                        .set_status("recording path selection cancelled".to_string());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn prompt_transfer_default_editor_setting(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Select default editor executable")),
        };
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting default editor".to_string());
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = path {
                    let path = path.display().to_string();
                    this.settings.set_transfer_default_editor(path.clone());
                    this.reset_text_input("settings.transfer.default-editor", &path, cx);
                    this.save_transfer_settings("transfer editor path saved", cx);
                } else {
                    this.shell
                        .set_status("editor path selection cancelled".to_string());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn resolved_transfer_download_dir(&self) -> Option<PathBuf> {
        let configured = self.settings.summary().transfer_download_path.trim();
        if configured.is_empty() {
            return default_transfer_download_dir();
        }
        Some(expand_transfer_home_path(configured))
    }

    pub(in crate::features) fn reveal_transfer_download_dir(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.resolved_transfer_download_dir() else {
            self.shell
                .set_status("cannot determine system download directory".to_string());
            cx.notify();
            return;
        };

        let prepare = cx
            .background_executor()
            .spawn(async move { std::fs::create_dir_all(&path).map(|()| path) });
        cx.spawn(async move |this, cx| {
            let result = prepare.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        cx.reveal_path(&path);
                        this.shell
                            .set_status(format!("opened download directory {}", path.display()));
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("failed to prepare download directory: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Resolve targets for both download entry points; an explicit directory bypasses
    /// the single-file target override.
    pub(in crate::features) fn resolve_transfer_download_targets(
        &self,
        remote_paths: Vec<RemoteFilePath>,
        directory: Option<PathBuf>,
    ) -> anyhow::Result<Vec<(RemoteFilePath, PathBuf)>> {
        let explicit = directory
            .is_none()
            .then(|| self.transfer.local_path().trim())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let directory = directory
            .or_else(|| self.resolved_transfer_download_dir())
            .unwrap_or_else(|| PathBuf::from("."));
        transfer_download_targets(remote_paths, directory, explicit)
    }

    pub(in crate::features) fn prompt_transfer_download_directory_and_start(
        &mut self,
        remote_paths: Vec<RemoteFilePath>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if remote_paths.is_empty() {
            self.report_browser_transfer_error(
                "download",
                &FileTransferEntryError::NoSelection.into(),
                cx,
            );
            return;
        }
        if self.transfer.path_prompt_is_open() {
            self.report_browser_transfer_error(
                "path-picker",
                &FileTransferEntryError::PickerAlreadyOpen.into(),
                cx,
            );
            return;
        }
        let session = match self.browser_transfer_session() {
            Ok(session) => session,
            Err(error) => {
                self.report_browser_transfer_error("transfer-start", &error, cx);
                return;
            }
        };
        let session_id = session.session_id;
        let service = session.service;

        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask)
            .then(|| self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>);
        let path_options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Select download directory")),
        };
        if !self
            .transfer
            .begin_path_prompt(TransferPathPromptKind::DownloadDirectory)
        {
            self.report_browser_transfer_error(
                "path-picker",
                &FileTransferEntryError::PickerAlreadyOpen.into(),
                cx,
            );
            return;
        }
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting download directory".to_string());
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => {
                    if paths.is_empty() {
                        TransferPathPromptResult::Cancelled
                    } else {
                        TransferPathPromptResult::Selected(paths)
                    }
                }
                Ok(Ok(None)) => TransferPathPromptResult::Cancelled,
                Ok(Err(error)) => TransferPathPromptResult::Failed(error.to_string()),
                Err(_) => TransferPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_transfer_download_start_prompt_result(
                    remote_paths,
                    session_id,
                    service,
                    path_options,
                    result,
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn prompt_transfer_browser_upload_path(
        &mut self,
        kind: TransferPathPromptKind,
        cx: &mut Context<Self>,
    ) {
        let remote_path = self.normalized_transfer_browser_upload_target();
        self.prompt_transfer_browser_upload_path_at(kind, remote_path, cx);
    }

    pub(in crate::features) fn prompt_transfer_browser_upload_path_at(
        &mut self,
        kind: TransferPathPromptKind,
        remote_path: String,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            kind,
            TransferPathPromptKind::UploadFile
                | TransferPathPromptKind::UploadDirectory
                | TransferPathPromptKind::UploadDirectoryContents
        ) {
            self.report_browser_transfer_error(
                "upload",
                &FileTransferEntryError::InvalidDestination.into(),
                cx,
            );
            return;
        }
        if self.transfer.path_prompt_is_open() {
            self.report_browser_transfer_error(
                "path-picker",
                &FileTransferEntryError::PickerAlreadyOpen.into(),
                cx,
            );
            return;
        }
        let session = match self.browser_transfer_session() {
            Ok(session) => session,
            Err(error) => {
                self.report_browser_transfer_error("transfer-start", &error, cx);
                return;
            }
        };
        let session_id = session.session_id;
        let service = session.service;
        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask)
            .then(|| self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>);
        let path_options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );

        let options = match kind {
            TransferPathPromptKind::UploadFile => PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some(SharedString::from("Select upload files")),
            },
            TransferPathPromptKind::UploadDirectory
            | TransferPathPromptKind::UploadDirectoryContents => PathPromptOptions {
                files: false,
                directories: true,
                multiple: true,
                prompt: Some(SharedString::from("Select upload directories")),
            },
            TransferPathPromptKind::DownloadDirectory => unreachable!(),
        };
        let remote_path = if remote_path == "/" {
            remote_path
        } else {
            remote_path.trim_end_matches('/').to_string()
        };
        if !self.transfer.begin_path_prompt(kind) {
            self.report_browser_transfer_error(
                "path-picker",
                &FileTransferEntryError::PickerAlreadyOpen.into(),
                cx,
            );
            return;
        }
        let receiver = cx.prompt_for_paths(options);
        self.shell.set_status(match kind {
            TransferPathPromptKind::UploadFile => "selecting upload file".to_string(),
            TransferPathPromptKind::UploadDirectory => "selecting upload directories".to_string(),
            TransferPathPromptKind::UploadDirectoryContents => {
                "selecting upload directory contents".to_string()
            }
            TransferPathPromptKind::DownloadDirectory => unreachable!(),
        });
        let pending = PendingBrowserUpload {
            kind,
            remote_path,
            session_id,
            service,
            path_options,
        };
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => {
                    if paths.is_empty() {
                        TransferPathPromptResult::Cancelled
                    } else if kind == TransferPathPromptKind::UploadDirectoryContents {
                        let task = scheduler.submit_task("upload-directory-children", move |_| {
                            local_directory_children(&paths)
                        });
                        match crate::features::runtime_jobs::await_blocking_job(task).await {
                            Ok(Ok(paths)) => TransferPathPromptResult::Selected(paths),
                            Ok(Err(error)) => TransferPathPromptResult::Failed(error),
                            Err(error) => TransferPathPromptResult::Failed(error.to_string()),
                        }
                    } else {
                        TransferPathPromptResult::Selected(paths)
                    }
                }
                Ok(Ok(None)) => TransferPathPromptResult::Cancelled,
                Ok(Err(error)) => TransferPathPromptResult::Failed(error.to_string()),
                Err(_) => TransferPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_transfer_browser_upload_path_prompt_result(pending, result, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_transfer_download_start_prompt_result(
        &mut self,
        remote_paths: Vec<RemoteFilePath>,
        session_id: Option<String>,
        service: nyaterm_files::file_browser::FileBrowserService,
        path_options: SftpPathTransferOptions,
        result: TransferPathPromptResult,
        cx: &mut Context<Self>,
    ) {
        if !self
            .transfer
            .finish_path_prompt(TransferPathPromptKind::DownloadDirectory)
        {
            return;
        }
        // 回调直接更新 App；延迟发布能覆盖成功、取消、失败及下面所有提前返回。
        self.defer_transfer_panel_snapshot_flush(cx);
        let source_session_id = session_id.clone();
        self.with_transfer_browser_session(source_session_id.as_deref(), |this| {
        match result {
            TransferPathPromptResult::Selected(paths) => {
                let Some(directory) = paths.into_iter().next() else {
                    this.shell.set_status("path picker cancelled".to_string());
                    return;
                };
                let total = remote_paths.len();
                if session_id.as_deref().is_none_or(|id| this.session.metadata(id).is_none() || this.session.is_disconnected(id))
                    || matches!(&service, nyaterm_files::file_browser::FileBrowserService::Adb(service) if service.control.is_cancelled()) {
                    this.report_transfer_entry_error("download", service.kind(), &FileTransferEntryError::Unavailable.into(), cx);
                    return;
                }
                let targets = match this
                    .resolve_transfer_download_targets(remote_paths, Some(directory.clone()))
                {
                    Ok(targets) => targets,
                    Err(error) => {
                        this.report_transfer_entry_error(
                            "download-target",
                            service.kind(),
                            &error,
                            cx,
                        );
                        return;
                    }
                };
                let session = FileTransferSession {
                    session_id,
                    service,
                };
                for (remote_path, local_path) in targets {
                    this.enqueue_sftp_download_job_for_target(
                        session.clone(),
                        remote_path,
                        local_path,
                        path_options.clone_for_download_batch(),
                        cx,
                    );
                }
                this.shell
                    .set_status(format!("{total} remote download job(s) started"));
                this.transfer.browser.status =
                    format!("Downloading {total} item(s) to {}", directory.display());
            }
            TransferPathPromptResult::Cancelled => {
                this.shell
                    .set_status("download directory selection cancelled".to_string());
                this.transfer.browser.status = "download selection cancelled".to_string();
            }
            TransferPathPromptResult::Failed(_) => {
                this.report_transfer_entry_error(
                    "path-picker",
                    service.kind(),
                    &FileTransferEntryError::PickerFailed.into(),
                    cx,
                );
            }
            TransferPathPromptResult::Closed => {
                this.report_transfer_entry_error(
                    "path-picker",
                    service.kind(),
                    &FileTransferEntryError::PickerClosed.into(),
                    cx,
                );
            }
        }
        });
    }

    fn apply_transfer_browser_upload_path_prompt_result(
        &mut self,
        pending: PendingBrowserUpload,
        result: TransferPathPromptResult,
        cx: &mut Context<Self>,
    ) {
        let PendingBrowserUpload {
            kind,
            remote_path,
            session_id,
            service,
            path_options,
        } = pending;
        if !self.transfer.finish_path_prompt(kind) {
            return;
        }
        // 同一次延迟发布读取回调结束后的最终状态，也覆盖空选择的提前返回。
        self.defer_transfer_panel_snapshot_flush(cx);
        let source_session_id = session_id.clone();
        self.with_transfer_browser_session(source_session_id.as_deref(), |this| match result {
            TransferPathPromptResult::Selected(paths) => {
                if paths.is_empty() {
                    let status = if kind == TransferPathPromptKind::UploadDirectoryContents {
                        t!("fileExplorer.uploadFolderContentsEmpty").to_string()
                    } else {
                        "upload selection cancelled".to_string()
                    };
                    this.shell.set_status(status.clone());
                    this.transfer.browser.status = status;
                    return;
                }
                let fallback = match kind {
                    TransferPathPromptKind::UploadFile => "uploaded_file",
                    TransferPathPromptKind::UploadDirectory => "uploaded_folder",
                    TransferPathPromptKind::UploadDirectoryContents => "uploaded_item",
                    TransferPathPromptKind::DownloadDirectory => unreachable!(),
                };
                this.enqueue_transfer_browser_upload_paths(
                    BrowserUploadRequest {
                        paths,
                        remote_path,
                        session_id,
                        service,
                        path_options,
                        fallback_name: fallback,
                    },
                    cx,
                );
            }
            TransferPathPromptResult::Cancelled => {
                this.shell.set_status("path picker cancelled".to_string());
                this.transfer.browser.status = "upload selection cancelled".to_string();
            }
            TransferPathPromptResult::Failed(_) => {
                this.report_transfer_entry_error(
                    "path-picker",
                    service.kind(),
                    &FileTransferEntryError::PickerFailed.into(),
                    cx,
                );
            }
            TransferPathPromptResult::Closed => {
                this.report_transfer_entry_error(
                    "path-picker",
                    service.kind(),
                    &FileTransferEntryError::PickerClosed.into(),
                    cx,
                );
            }
        });
    }

    pub(in crate::features) fn set_transfer_browser_external_drop_hover(
        &mut self,
        hover: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.set_browser_external_drop_hover(hover) {
            cx.notify();
        }
        self.ensure_drop_hover_clock(cx);
    }

    pub(in crate::features) fn handle_transfer_browser_external_file_drop(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.transfer.set_browser_external_drop_hover(false);
        if self.session.active_file_browser_backend()
            == Some(nyaterm_files::file_browser::FileBrowserBackendKind::Local)
        {
            self.report_browser_transfer_error(
                "drop-upload",
                &FileTransferEntryError::UnsupportedBackend.into(),
                cx,
            );
            return;
        }
        if paths.is_empty() {
            self.report_browser_transfer_error(
                "drop-upload",
                &FileTransferEntryError::NoSelection.into(),
                cx,
            );
            return;
        }
        self.start_transfer_browser_upload_paths(paths, cx);
    }

    pub(in crate::features) fn upload_external_paths_to_endpoint(
        &mut self,
        endpoint: crate::features::file_manager::PaneEndpoint,
        directory: RemoteFilePath,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let service = match self.file_browser_service_for_endpoint(&endpoint) {
            Ok(service) => service,
            Err(error) => {
                self.report_transfer_entry_error("drop-upload", endpoint.kind(), &error, cx);
                return;
            }
        };
        if service.kind() != nyaterm_files::file_browser::FileBrowserBackendKind::Local
            && let Err(error) = directory.raw_path().and_then(|raw| {
                anyhow::ensure!(
                    raw.is_none_or(|raw| raw == directory.display_path.as_bytes()),
                    FileTransferEntryError::InvalidDestination
                );
                Ok(())
            })
        {
            self.report_transfer_entry_error("drop-upload", service.kind(), &error, cx);
            return;
        }
        let options = self.sftp_download_path_options();
        let session_id = match &endpoint {
            crate::features::file_manager::PaneEndpoint::Adb(key) => self
                .session
                .session_ids()
                .find(|id| {
                    self.android_config_for_session(id)
                        .is_some_and(|config| config.device_key == *key)
                })
                .map(str::to_string),
            _ => endpoint.session_id().map(str::to_string),
        };
        self.enqueue_transfer_browser_upload_paths(
            BrowserUploadRequest {
                paths,
                remote_path: directory.display_path,
                session_id,
                service,
                path_options: options,
                fallback_name: "uploaded_item",
            },
            cx,
        );
    }

    fn start_transfer_browser_upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let session = match self.browser_transfer_session() {
            Ok(session) => session,
            Err(error) => {
                self.report_browser_transfer_error("transfer-start", &error, cx);
                return;
            }
        };
        let session_id = session.session_id;
        let service = session.service;
        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask)
            .then(|| self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>);
        let path_options = SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        );
        let remote_path = self.normalized_transfer_browser_upload_target();
        self.enqueue_transfer_browser_upload_paths(
            BrowserUploadRequest {
                paths,
                remote_path,
                session_id,
                service,
                path_options,
                fallback_name: "uploaded_item",
            },
            cx,
        );
    }

    fn enqueue_transfer_browser_upload_paths(
        &mut self,
        request: BrowserUploadRequest,
        cx: &mut Context<Self>,
    ) {
        let BrowserUploadRequest {
            paths,
            remote_path,
            session_id,
            service,
            mut path_options,
            fallback_name,
        } = request;
        if paths.is_empty() {
            self.report_transfer_entry_error(
                "upload",
                service.kind(),
                &FileTransferEntryError::NoSelection.into(),
                cx,
            );
            return;
        }
        if session_id.as_deref().is_some_and(|id| {
            self.session.metadata(id).is_none() || self.session.is_disconnected(id)
        }) || matches!(&service, nyaterm_files::file_browser::FileBrowserService::Adb(service) if service.control.is_cancelled())
        {
            self.report_transfer_entry_error(
                "upload",
                service.kind(),
                &FileTransferEntryError::Unavailable.into(),
                cx,
            );
            return;
        }
        let total = paths.len();
        if total > 1 && path_options.duplicate_policy() == SftpDuplicatePolicy::Ask {
            path_options = SftpPathTransferOptions::new(
                SftpDuplicatePolicy::Ask,
                Some(Arc::new(UploadBatchDuplicateResolver::new(
                    self.session.prompt_duplicate_broker(),
                ))),
                path_options.transfer_options().clone(),
            );
        }
        if session_id.as_deref() == self.session.active_id() {
            self.transfer.set_remote_path(remote_path.clone());
            self.transfer.browser.status = if total == 1 {
                format!(
                    "Uploading {} to {}",
                    paths[0].display(),
                    truncate_preview(&remote_path, 48)
                )
            } else {
                format!(
                    "Uploading {total} items to {}",
                    truncate_preview(&remote_path, 48)
                )
            };
        }
        let session = FileTransferSession {
            session_id,
            service,
        };
        let mut target_locks: HashMap<String, Arc<Mutex<()>>> = HashMap::new();
        for path in paths {
            let upload_name = transfer_upload_local_name(&path, fallback_name);
            let target_path = match session
                .service
                .child_path(&RemoteFilePath::new(&remote_path), &upload_name)
            {
                Ok(path) => path.display_path,
                Err(error) => {
                    self.report_transfer_entry_error(
                        "upload-target",
                        session.service.kind(),
                        &error,
                        cx,
                    );
                    continue;
                }
            };
            let target_lock = Arc::clone(
                target_locks
                    .entry(target_path.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            );
            self.enqueue_sftp_upload_job_for_target(
                session.clone(),
                path,
                target_path,
                path_options.clone(),
                target_lock,
                cx,
            );
        }
    }

    fn normalized_transfer_browser_upload_target(&self) -> String {
        let target = self.transfer_browser_operation_target_directory();
        let value = target.trim();
        if value.is_empty() {
            self.transfer.normalized_remote_path()
        } else if value == "/" {
            "/".to_string()
        } else {
            value.trim_end_matches('/').to_string()
        }
    }
}

/// Validate all generated names before enqueueing; an explicit single-file target
/// does not depend on the remote display name.
fn transfer_download_targets(
    remote_paths: Vec<RemoteFilePath>,
    directory: PathBuf,
    explicit: Option<PathBuf>,
) -> anyhow::Result<Vec<(RemoteFilePath, PathBuf)>> {
    if remote_paths.len() == 1
        && let Some(target) = explicit
    {
        return Ok(remote_paths
            .into_iter()
            .map(|path| (path, target.clone()))
            .collect());
    }
    let mut names = std::collections::HashSet::new();
    remote_paths
        .into_iter()
        .map(|remote_path| {
            let name = nyaterm_files::download_path::file_name(&remote_path.display_path)
                .map_err(|_| FileTransferEntryError::InvalidDestination)?;
            anyhow::ensure!(
                names.insert(nyaterm_files::download_path::target_key(name)),
                FileTransferEntryError::InvalidDestination
            );
            let target = directory.join(name);
            Ok((remote_path, target))
        })
        .collect()
}

fn default_transfer_download_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
}

fn expand_transfer_home_path(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    path.strip_prefix("~/")
        .and_then(|suffix| dirs::home_dir().map(|home| home.join(suffix)))
        .unwrap_or_else(|| PathBuf::from(path))
}

fn transfer_upload_local_name(path: &std::path::Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn local_directory_children(directories: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut children = Vec::new();
    let mut seen = HashSet::new();
    for directory in directories {
        if !seen.insert(directory) {
            continue;
        }
        let metadata = std::fs::metadata(directory)
            .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
        if !metadata.is_dir() {
            return Err(format!("not a directory: {}", directory.display()));
        }
        for entry in std::fs::read_dir(directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        {
            children.push(entry.map_err(|error| error.to_string())?.path());
        }
    }
    children.sort();
    Ok(children)
}

#[cfg(test)]
fn transfer_upload_remote_child_path(remote_dir: &str, name: &str) -> String {
    let remote_dir = remote_dir.trim();
    if remote_dir == "/" {
        return format!("/{name}");
    }
    match remote_dir.trim_end_matches('/') {
        "" | "." => name.to_string(),
        parent => format!("{parent}/{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        local_directory_children, transfer_upload_local_name, transfer_upload_remote_child_path,
    };
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::SettingsTab;
    use gpui::{AppContext as _, Context, TestAppContext};
    use nyaterm_core::{AppRuntime, AppSettingsSummary, RuntimeMode, test_support::TestTempDir};
    use std::path::PathBuf;

    #[test]
    fn adb_upload_picker_keeps_original_session_and_directory_when_active_tab_changes() {
        use crate::models::{TransferJobKind, TransferPathPromptKind};
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-picker-source");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "adb-source",
            "/device",
        );
        let input = root.path().join("fixture.txt");
        std::fs::write(&input, b"fixture").unwrap();
        cx.update_entity(&app, |app, cx| {
            app.prompt_transfer_browser_upload_path_at(
                TransferPathPromptKind::UploadFile,
                "/device/uploads".into(),
                cx,
            );
            let metadata = app.session.metadata("adb-source").unwrap().clone();
            app.session.register_session_metadata("other", metadata);
            app.session.select_active_session("other");
            app.transfer.browser.path = "/other".into();
            app.transfer.set_remote_path("/other");
        });
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files && options.multiple);
            Some(vec![input.clone()])
        });
        cx.run_until_parked();
        app.update(&mut cx, |app, _| {
            let job = app
                .transfer
                .transfer_jobs()
                .iter()
                .find(|job| matches!(job.kind, TransferJobKind::Upload { .. }))
                .unwrap();
            assert_eq!(job.session_id.as_deref(), Some("adb-source"));
            let TransferJobKind::Upload {
                remote_path,
                local_path,
            } = &job.kind
            else {
                panic!("expected upload")
            };
            assert_eq!(remote_path, "/device/uploads/fixture.txt");
            assert_eq!(local_path, &input);
            assert_eq!(app.transfer.browser.path, "/other");
        });
    }

    #[test]
    fn adb_download_jobs_use_queue_controls_and_offline_entries_report_visible_error() {
        use crate::models::TransferJobStatus;
        use nyaterm_files::sftp::RemoteFilePath;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-download-controls");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "adb",
            "/device",
        );
        app.update(&mut cx, |app, cx| {
            let options = app.sftp_download_path_options();
            assert!(app.start_sftp_download_job_for_target(
                RemoteFilePath::new("/device/file"),
                root.path().join("file"),
                options,
                cx
            ));
            let job = app.transfer.transfer_jobs().last().unwrap();
            let id = job.id.clone();
            assert_eq!(job.session_id.as_deref(), Some("adb"));
            assert!(job.control.is_some());
            app.pause_transfer_job(&id, cx);
            assert_eq!(
                app.transfer.transfer_job(&id).unwrap().status,
                TransferJobStatus::Paused
            );
            app.resume_transfer_job(&id, cx);
            assert!(matches!(
                app.transfer.transfer_job(&id).unwrap().status,
                TransferJobStatus::Queued | TransferJobStatus::Running
            ));
            app.cancel_transfer_job(&id, cx);
            assert!(
                app.transfer
                    .transfer_job(&id)
                    .unwrap()
                    .control
                    .as_ref()
                    .unwrap()
                    .is_cancelled()
            );
            app.active_file_browser_service()
                .unwrap()
                .downgrade()
                .unwrap()
                .upgrade()
                .unwrap();
            let nyaterm_files::file_browser::FileBrowserService::Adb(service) =
                app.active_file_browser_service().unwrap()
            else {
                panic!("expected ADB")
            };
            service.cancel();
            let count = app.transfer.transfer_jobs().len();
            app.handle_transfer_browser_external_file_drop(vec![root.path().join("input")], cx);
            assert_eq!(app.transfer.transfer_jobs().len(), count);
            assert_eq!(
                app.transfer.browser.status,
                rust_i18n::t!("fileExplorer.transferSourceUnavailable").as_ref()
            );
        });
    }

    #[test]
    fn adb_picker_failure_is_localized_and_cancellation_does_not_create_a_job() {
        use crate::models::{TransferPathPromptKind, TransferPathPromptResult};
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-picker-error");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "adb",
            "/device",
        );
        app.update(&mut cx, |app, cx| {
            for result in [
                TransferPathPromptResult::Failed("sensitive picker details".into()),
                TransferPathPromptResult::Cancelled,
            ] {
                let session = app.browser_transfer_session().unwrap();
                let kind = TransferPathPromptKind::UploadFile;
                assert!(app.transfer.begin_path_prompt(kind));
                app.apply_transfer_browser_upload_path_prompt_result(
                    super::PendingBrowserUpload {
                        kind,
                        remote_path: "/device".into(),
                        session_id: session.session_id,
                        service: session.service,
                        path_options: app.sftp_download_path_options(),
                    },
                    result,
                    cx,
                );
                assert!(!app.transfer.browser.status.contains("sensitive"));
                assert!(app.transfer.transfer_jobs().is_empty());
            }
        });
    }

    #[test]
    fn adb_download_picker_keeps_original_source_and_preserves_the_new_tabs_browser_status() {
        use crate::models::{TransferJobKind, TransferPathPromptKind, TransferPathPromptResult};
        use nyaterm_files::sftp::RemoteFilePath;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-download-picker-switch");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "original",
            "/device",
        );
        app.update(&mut cx, |app, cx| {
            let session = app.browser_transfer_session().unwrap();
            let metadata = app.session.metadata("original").unwrap().clone();
            app.session.register_session_metadata("other", metadata);
            app.session.select_active_session("other");
            app.transfer.browser.path = "/other".into();
            app.transfer.browser.status = "other status".into();
            app.transfer.set_remote_path("/other");
            assert!(
                app.transfer
                    .begin_path_prompt(TransferPathPromptKind::DownloadDirectory)
            );
            let options = app.sftp_download_path_options();
            app.apply_transfer_download_start_prompt_result(
                vec![RemoteFilePath::new("/device/file")],
                session.session_id,
                session.service,
                options,
                TransferPathPromptResult::Selected(vec![root.path().join("downloads")]),
                cx,
            );
            let job = app.transfer.transfer_jobs().last().unwrap();
            assert_eq!(job.session_id.as_deref(), Some("original"));
            let TransferJobKind::Download {
                remote_path,
                local_path,
                ..
            } = &job.kind
            else {
                panic!("expected download")
            };
            assert_eq!(remote_path, "/device/file");
            assert_eq!(local_path, &root.path().join("downloads/file"));
            assert_eq!(app.transfer.browser.path, "/other");
            assert_eq!(app.transfer.browser.status, "other status");
        });
    }

    #[test]
    fn adb_external_drop_and_retry_keep_the_original_endpoint_and_report_its_disconnect() {
        use crate::features::file_manager::PaneEndpoint;
        use crate::models::TransferJobStatus;
        use nyaterm_files::sftp::RemoteFilePath;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-drop-retry");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "original",
            "/device",
        );
        let vcx = crate::features::test_support::session_focus_test_window(&mut cx, &app);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let key = app
                    .android_config_for_session("original")
                    .unwrap()
                    .device_key;
                app.upload_external_paths_to_endpoint(
                    PaneEndpoint::Adb(key),
                    RemoteFilePath::new("/device/drop-target"),
                    vec![root.path().join("input")],
                    cx,
                );
                let job = app.transfer.transfer_jobs().last().unwrap();
                let id = job.id.clone();
                let source = app.transfer.promised_download_source(&id).unwrap();
                app.transfer.transfer_job_mut(&id).unwrap().status = TransferJobStatus::Failed;
                let metadata = app.session.metadata("original").unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.select_active_session("other");
                app.retry_transfer_job(id.clone(), window, cx);
                assert_eq!(
                    app.transfer
                        .transfer_job(&id)
                        .unwrap()
                        .session_id
                        .as_deref(),
                    Some("original")
                );
                let current_source = app.transfer.promised_download_source(&id).unwrap();
                let (
                    nyaterm_files::file_browser::WeakFileBrowserService::Adb(original),
                    nyaterm_files::file_browser::WeakFileBrowserService::Adb(retry),
                ) = (source, current_source)
                else {
                    panic!("expected ADB sources")
                };
                assert!(std::sync::Weak::ptr_eq(&original, &retry));
                app.transfer.transfer_job_mut(&id).unwrap().status = TransferJobStatus::Failed;
                app.session.metadata_mut("original").unwrap().disconnected = true;
                app.retry_transfer_job(id.clone(), window, cx);
                assert_eq!(
                    app.transfer.transfer_job(&id).unwrap().status,
                    TransferJobStatus::Failed
                );
                assert_eq!(
                    app.transfer.browser.status,
                    rust_i18n::t!("fileExplorer.transferSourceUnavailable").as_ref()
                );
            })
        });
    }

    #[test]
    fn closing_an_adb_view_keeps_submitted_transfers_alive_until_explicit_cancellation() {
        use nyaterm_files::sftp::RemoteFilePath;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-adb-close-transfer");
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            "original",
            "/device",
        );
        app.update(&mut cx, |app, cx| {
            let options = app.sftp_download_path_options();
            assert!(app.start_sftp_download_job_for_target(
                RemoteFilePath::new("/device/file"),
                root.path().join("file"),
                options,
                cx
            ));
            let job = app.transfer.transfer_jobs().last().unwrap();
            let id = job.id.clone();
            let control = job.control.as_ref().unwrap().clone();
            app.remove_session_state("original", cx);
            assert!(!control.is_cancelled());
            app.cancel_transfer_job(&id, cx);
            assert!(control.is_cancelled());
        });
    }

    fn assert_settings_path_picker_updates_existing_input(
        input_id: &str,
        tab: SettingsTab,
        directories: bool,
        prompt: fn(&mut NyaTermApp, &mut Context<NyaTermApp>),
        value: fn(&AppSettingsSummary) -> &str,
    ) {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-settings-path-picker");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().to_path_buf(),
            root.path().join("config"),
            root.path().join("logs"),
            root.path().join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        let input = cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.ensure_settings_tab_inputs(tab, cx);
            app.existing_text_input(input_id).unwrap()
        });

        let first = root.path().join("first path");
        let second = root.path().join("second path");
        for (selected, expected) in [
            (Some(first.clone()), &first),
            (None, &first),
            (Some(second.clone()), &second),
        ] {
            cx.update_entity(&app, prompt);
            assert!(cx.did_prompt_for_paths());
            cx.simulate_path_prompt_response(|options| {
                assert_eq!(options.directories, directories);
                assert_eq!(options.files, !directories);
                assert!(!options.multiple);
                selected.map(|path| vec![path])
            });
            cx.run_until_parked();

            cx.update_entity(&app, |app, cx| {
                let expected = expected.display().to_string();
                assert_eq!(value(app.settings.summary()), expected);
                assert_eq!(app.existing_text_input(input_id).unwrap(), input);
                assert_eq!(input.read(cx).value(cx), expected);
                assert!(app.settings_draft_dirty());
            });
        }
    }

    #[test]
    fn download_directory_picker_updates_existing_settings_input() {
        assert_settings_path_picker_updates_existing_input(
            "settings.transfer.download-path",
            SettingsTab::Transfer,
            true,
            NyaTermApp::prompt_transfer_download_path_setting,
            |summary| &summary.transfer_download_path,
        );
    }

    #[test]
    fn recording_directory_picker_updates_existing_settings_input() {
        assert_settings_path_picker_updates_existing_input(
            "settings.recording.path",
            SettingsTab::TerminalGeneral,
            true,
            NyaTermApp::prompt_recording_path_setting,
            |summary| &summary.recording_path,
        );
    }

    #[test]
    fn default_editor_picker_updates_existing_settings_input() {
        assert_settings_path_picker_updates_existing_input(
            "settings.transfer.default-editor",
            SettingsTab::Transfer,
            false,
            NyaTermApp::prompt_transfer_default_editor_setting,
            |summary| &summary.transfer_default_editor,
        );
    }

    #[test]
    fn upload_folder_contents_collects_direct_children_once() {
        let root = std::env::temp_dir().join(format!("nyaterm-upload-{}", uuid::Uuid::new_v4()));
        let first = root.join("first");
        let second = root.join("second");
        let empty = root.join("empty");
        std::fs::create_dir_all(first.join("nested")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(first.join("file.txt"), b"data").unwrap();
        std::fs::write(second.join("other.txt"), b"data").unwrap();
        std::fs::write(second.join("file.txt"), b"other data").unwrap();

        let children =
            local_directory_children(&[first.clone(), second.clone(), first.clone()]).unwrap();
        assert_eq!(
            children,
            vec![
                first.join("file.txt"),
                first.join("nested"),
                second.join("file.txt"),
                second.join("other.txt")
            ]
        );
        assert!(local_directory_children(&[empty]).unwrap().is_empty());
        assert!(local_directory_children(&[first.join("file.txt")]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn download_targets_preserve_raw_identity_and_respect_explicit_single_target() {
        use nyaterm_files::sftp::RemoteFilePath;
        let source = RemoteFilePath::from_raw("/remote/a\\b", b"/remote/a\\b");
        let explicit = PathBuf::from("chosen.txt");
        let targets = super::transfer_download_targets(
            vec![source.clone()],
            PathBuf::from("."),
            Some(explicit.clone()),
        )
        .unwrap();
        assert_eq!(targets, vec![(source.clone(), explicit)]);
        assert!(super::transfer_download_targets(vec![source], PathBuf::from("."), None).is_err());
        assert!(
            super::transfer_download_targets(
                vec![
                    RemoteFilePath::new("/a/same"),
                    RemoteFilePath::new("/b/same")
                ],
                PathBuf::from("."),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn upload_remote_child_path_joins_browser_directory_and_local_name() {
        assert_eq!(
            transfer_upload_remote_child_path("/remote", "local.txt"),
            "/remote/local.txt"
        );
        assert_eq!(
            transfer_upload_remote_child_path("/", "local.txt"),
            "/local.txt"
        );
        assert_eq!(
            transfer_upload_remote_child_path(".", "local.txt"),
            "local.txt"
        );
    }

    #[test]
    fn upload_remote_child_path_maps_multiple_local_paths_to_browser_directory() {
        let paths = [PathBuf::from("one.txt"), PathBuf::from("two.txt")];
        let targets = paths
            .iter()
            .map(|path| {
                let name = transfer_upload_local_name(path, "uploaded_item");
                transfer_upload_remote_child_path("/remote", &name)
            })
            .collect::<Vec<_>>();

        assert_eq!(targets, ["/remote/one.txt", "/remote/two.txt"]);
    }

    #[test]
    fn upload_local_name_uses_fallback_for_empty_terminal_component() {
        assert_eq!(
            transfer_upload_local_name(&PathBuf::from("/"), "uploaded_item"),
            "uploaded_item"
        );
    }
}

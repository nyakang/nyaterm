use rust_i18n::t;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, PathPromptOptions, SharedString, Window};
use nyaterm_core::truncate_preview;
use nyaterm_transport::{
    RemoteFilePath, SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
    SshSessionConfig,
};

use crate::features::NyaTermApp;
use crate::features::formatting::download_file_name_from_remote_path;
use crate::features::transfers::SftpJobSession;
use crate::models::{NavItem, TransferPathPromptKind, TransferPathPromptResult};

struct PendingBrowserUpload {
    kind: TransferPathPromptKind,
    remote_path: String,
    session_id: Option<String>,
    config: SshSessionConfig,
    path_options: SftpPathTransferOptions,
}

struct BrowserUploadRequest {
    paths: Vec<PathBuf>,
    remote_path: String,
    session_id: Option<String>,
    config: SshSessionConfig,
    path_options: SftpPathTransferOptions,
    fallback_name: &'static str,
}

impl NyaTermApp {
    pub(in crate::features) fn prompt_transfer_download_path_setting(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.path_prompt_is_open() {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
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
                    this.settings
                        .set_transfer_download_path(path.display().to_string());
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
                    this.settings.set_recording_path(path.display().to_string());
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
                    this.settings
                        .set_transfer_default_editor(path.display().to_string());
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

        if path.exists() && !path.is_dir() {
            self.shell.set_status(format!(
                "configured download path is not a directory: {}",
                path.display()
            ));
            cx.notify();
            return;
        }

        match std::fs::create_dir_all(&path) {
            Ok(()) => {
                cx.reveal_path(&path);
                self.shell
                    .set_status(format!("opened download directory {}", path.display()));
            }
            Err(error) => {
                self.shell
                    .set_status(format!("failed to prepare download directory: {error}"));
            }
        }
        cx.notify();
    }

    /// Builds a single-file download target from an explicit remote display path; the
    /// selection identity key must not participate in naming.
    pub(in crate::features) fn normalized_transfer_local_path(&self, remote_path: &str) -> PathBuf {
        let value = self.transfer.local_path().trim();
        if value.is_empty() {
            let file_name = download_file_name_from_remote_path(remote_path);
            self.resolved_transfer_download_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(file_name)
        } else {
            PathBuf::from(value)
        }
    }

    pub(in crate::features) fn prompt_transfer_download_directory_and_start(
        &mut self,
        remote_paths: Vec<RemoteFilePath>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if remote_paths.is_empty() {
            self.shell
                .set_status("select remote items before downloading".to_string());
            cx.notify();
            return;
        }
        if self.transfer.path_prompt_is_open() {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
            return;
        }
        let Some(config) = self.session.active_ssh_config_owned() else {
            self.shell
                .set_status("start an SSH session first".to_string());
            self.shell.select_nav(NavItem::Transfers);
            cx.notify();
            return;
        };
        let session_id = self.session.active_id_owned();

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
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
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
                    config,
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
        if !matches!(
            kind,
            TransferPathPromptKind::UploadFile | TransferPathPromptKind::UploadDirectory
        ) {
            self.shell
                .set_status("browser upload requires a file or directory".to_string());
            cx.notify();
            return;
        }
        if self.transfer.path_prompt_is_open() {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
            return;
        }
        let Some(config) = self.session.active_ssh_config_owned() else {
            self.shell
                .set_status("start an SSH session first".to_string());
            cx.notify();
            return;
        };
        let session_id = self.session.active_id_owned();
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
            TransferPathPromptKind::UploadDirectory => PathPromptOptions {
                files: false,
                directories: true,
                multiple: true,
                prompt: Some(SharedString::from("Select upload directories")),
            },
            TransferPathPromptKind::DownloadDirectory => unreachable!(),
        };
        let remote_path = self.normalized_transfer_browser_upload_target();
        if !self.transfer.begin_path_prompt(kind) {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(options);
        self.shell.set_status(match kind {
            TransferPathPromptKind::UploadFile => "selecting upload file".to_string(),
            TransferPathPromptKind::UploadDirectory => "selecting upload directories".to_string(),
            TransferPathPromptKind::DownloadDirectory => unreachable!(),
        });
        let pending = PendingBrowserUpload {
            kind,
            remote_path,
            session_id,
            config,
            path_options,
        };
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
        config: SshSessionConfig,
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
        match result {
            TransferPathPromptResult::Selected(paths) => {
                let Some(directory) = paths.into_iter().next() else {
                    self.shell.set_status("path picker cancelled".to_string());
                    return;
                };
                let total = remote_paths.len();
                let Some(service_session_id) = session_id.as_deref() else {
                    self.shell
                        .set_status("source session is unavailable".to_string());
                    return;
                };
                let service = match self.remote_file_service_for_session(service_session_id, config)
                {
                    Ok(service) => service,
                    Err(error) => {
                        self.shell.set_status(error.to_string());
                        return;
                    }
                };
                let session = SftpJobSession {
                    session_id,
                    service,
                };
                for remote_path in remote_paths {
                    let local_path = directory.join(download_file_name_from_remote_path(
                        &remote_path.display_path,
                    ));
                    self.enqueue_sftp_download_job_for_target(
                        session.clone(),
                        remote_path,
                        local_path,
                        path_options.clone(),
                        cx,
                    );
                }
                self.shell
                    .set_status(format!("{total} remote download job(s) started"));
                self.transfer.browser.status =
                    format!("Downloading {total} item(s) to {}", directory.display());
            }
            TransferPathPromptResult::Cancelled => {
                self.shell
                    .set_status("download directory selection cancelled".to_string());
                self.transfer.browser.status = "download selection cancelled".to_string();
            }
            TransferPathPromptResult::Failed(error) => {
                self.shell
                    .set_status(format!("path picker failed: {error}"));
                self.transfer.browser.status = self.shell.status().to_string();
            }
            TransferPathPromptResult::Closed => {
                self.shell
                    .set_status("path picker closed before returning".to_string());
                self.transfer.browser.status = self.shell.status().to_string();
            }
        }
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
            config,
            path_options,
        } = pending;
        if !self.transfer.finish_path_prompt(kind) {
            return;
        }
        // 同一次延迟发布读取回调结束后的最终状态，也覆盖空选择的提前返回。
        self.defer_transfer_panel_snapshot_flush(cx);
        match result {
            TransferPathPromptResult::Selected(paths) => {
                if paths.is_empty() {
                    self.shell.set_status("path picker cancelled".to_string());
                    self.transfer.browser.status = "upload selection cancelled".to_string();
                    return;
                }
                let fallback = match kind {
                    TransferPathPromptKind::UploadFile => "uploaded_file",
                    TransferPathPromptKind::UploadDirectory => "uploaded_folder",
                    TransferPathPromptKind::DownloadDirectory => unreachable!(),
                };
                self.enqueue_transfer_browser_upload_paths(
                    BrowserUploadRequest {
                        paths,
                        remote_path,
                        session_id,
                        config,
                        path_options,
                        fallback_name: fallback,
                    },
                    cx,
                );
            }
            TransferPathPromptResult::Cancelled => {
                self.shell.set_status("path picker cancelled".to_string());
                self.transfer.browser.status = "upload selection cancelled".to_string();
            }
            TransferPathPromptResult::Failed(error) => {
                self.shell
                    .set_status(format!("path picker failed: {error}"));
                self.transfer.browser.status = self.shell.status().to_string();
            }
            TransferPathPromptResult::Closed => {
                self.shell
                    .set_status("path picker closed before returning".to_string());
                self.transfer.browser.status = self.shell.status().to_string();
            }
        }
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
            == Some(nyaterm_transport::FileBrowserBackendKind::Local)
        {
            let status = "upload by dropping files is unavailable for local browsing".to_string();
            self.shell.set_status(status.clone());
            self.transfer.set_browser_status(status);
            cx.notify();
            return;
        }
        if paths.is_empty() {
            let status = t!("fileExplorer.externalDropPathsRequired").to_string();
            self.shell.set_status(status.clone());
            self.transfer.set_browser_status(status);
            cx.notify();
            return;
        }
        self.start_transfer_browser_upload_paths(paths, cx);
    }

    fn start_transfer_browser_upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(config) = self.session.active_ssh_config_owned() else {
            self.shell
                .set_status("start an SSH session first".to_string());
            cx.notify();
            return;
        };
        let session_id = self.session.active_id_owned();
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
                config,
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
            config,
            path_options,
            fallback_name,
        } = request;
        if paths.is_empty() {
            return;
        }
        let total = paths.len();
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
        let Some(service_session_id) = session_id.as_deref() else {
            self.shell
                .set_status("source session is unavailable".to_string());
            return;
        };
        let service = match self.remote_file_service_for_session(service_session_id, config) {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                return;
            }
        };
        let session = SftpJobSession {
            session_id,
            service,
        };
        for path in paths {
            let upload_name = transfer_upload_local_name(&path, fallback_name);
            let target_path = transfer_upload_remote_child_path(&remote_path, &upload_name);
            self.enqueue_sftp_upload_job_for_target(
                session.clone(),
                path,
                target_path,
                path_options.clone(),
                cx,
            );
        }
    }

    fn normalized_transfer_browser_upload_target(&self) -> String {
        let value = self.transfer.browser.path.trim();
        if value.is_empty() {
            self.transfer.normalized_remote_path()
        } else if value == "/" {
            "/".to_string()
        } else {
            value.trim_end_matches('/').to_string()
        }
    }
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
    use super::{transfer_upload_local_name, transfer_upload_remote_child_path};
    use std::path::PathBuf;

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

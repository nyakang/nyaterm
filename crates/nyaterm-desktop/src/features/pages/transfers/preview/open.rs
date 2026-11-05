//! Opening remote files in the read-only preview and loading them off-thread.
//!
//! The load runs on a worker thread: the fetch is blocking network I/O and the
//! decode (image/PDF) is CPU-heavy, so neither can touch a render path. The
//! worker sends the finished [`PreviewContent`] back as a `TransferJobResult`,
//! and the event drain applies it under the tab's generation guard.

use crate::features::NyaTermApp;
use crate::features::transfers::preview::decode::{decode_binary_content, decode_text_content};
use crate::models::{
    PreviewContent, PreviewViewport, TransferJobEvent, TransferJobKind, TransferJobOutput,
    TransferJobResult, TransferJobState, TransferJobStatus, TransferPreviewState,
};
use gpui::{Context, Window};
use nyaterm_core::{PreviewCategory, classify_preview, preview_within_limit};
use nyaterm_files::remote_file::RemoteTextGeneration;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use rust_i18n::t;

impl NyaTermApp {
    /// Whether the entry menu should offer a Preview action for `entry`.
    ///
    /// Every non-directory file gets a Preview action, matching the Tauri
    /// client. Unrenderable formats still open the window and show the
    /// unsupported message; only directories are excluded.
    pub(in crate::features) fn show_transfer_preview_menu_entry(
        &self,
        entry: &SftpFileEntry,
    ) -> bool {
        !entry.is_directory()
    }

    pub(in crate::features) fn open_selected_transfer_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.selected_transfer_entry() else {
            self.shell
                .set_status(t!("filePreview.statusSelectFile").to_string());
            cx.notify();
            return;
        };
        self.open_transfer_preview(entry, window, cx);
    }

    pub(in crate::features) fn open_transfer_preview(
        &mut self,
        entry: SftpFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = self.session.active_id_owned();
        self.open_transfer_preview_for_session(entry, session_id, window, cx);
    }

    pub(in crate::features) fn open_transfer_preview_for_session(
        &mut self,
        entry: SftpFileEntry,
        session_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.shell
                .set_status(t!("filePreview.statusDirectory").to_string());
            cx.notify();
            return;
        }
        // Every non-directory file resolves to a category; an unrenderable
        // format maps to `Unsupported`, which opens the window and shows the
        // unsupported message without fetching any bytes.
        let category = classify_preview(&entry.name);

        let remote_file_path = entry.remote_path();
        let tab_id =
            TransferPreviewState::tab_id_for_remote_path(session_id.as_deref(), &remote_file_path);

        // Already open: focus it, and re-run the load so a stale preview is
        // refreshed rather than shown as-is.
        if self
            .transfer
            .preview_workspace()
            .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.id == tab_id))
        {
            self.transfer.activate_preview_tab(&tab_id);
            self.open_remote_file_preview_window(cx);
            if !self.transfer.preview_window_is_open()
                && !self.transfer.preview_window_open_is_pending()
            {
                window.focus(self.transfer.preview_focus(), cx);
            }
            let status = t!("filePreview.statusAlreadyOpen", path = entry.path.clone()).to_string();
            self.transfer.set_browser_status(status.clone());
            self.shell.set_status(status);
            self.refresh_transfer_preview_tab(&tab_id, window, cx);
            cx.notify();
            return;
        }

        let generation = RemoteTextGeneration::next();
        // An unsupported file is terminal at open: no fetch, just the message.
        let initial_content = if category.is_fetchable() {
            PreviewContent::Loading
        } else {
            PreviewContent::Unsupported
        };
        let tab = TransferPreviewState {
            id: tab_id.clone(),
            session_id: session_id.clone(),
            remote_path: entry.path.clone(),
            raw_path_token: entry.raw_path_token.clone(),
            name: entry.name.clone(),
            size: entry.size,
            modified_at: entry.modified_at,
            category,
            generation,
            content: initial_content,
            viewport: PreviewViewport::default(),
        };
        self.transfer.open_preview_tab(tab);
        self.shell
            .set_status(t!("filePreview.statusPreviewing", path = entry.path.clone()).to_string());
        self.open_remote_file_preview_window(cx);
        if !self.transfer.preview_window_is_open()
            && !self.transfer.preview_window_open_is_pending()
        {
            window.focus(self.transfer.preview_focus(), cx);
        }
        if category.is_fetchable() {
            self.start_sftp_preview_load_job(
                session_id,
                tab_id,
                remote_file_path,
                category,
                generation,
                window,
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::features) fn refresh_active_transfer_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.transfer.active_preview_tab().map(|tab| tab.id.clone()) else {
            return;
        };
        self.refresh_transfer_preview_tab(&tab_id, window, cx);
    }

    pub(in crate::features) fn refresh_transfer_preview_tab(
        &mut self,
        tab_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.transfer.preview_tab_snapshot(tab_id) else {
            return;
        };
        // Unsupported files are terminal; refresh re-shows the message without
        // a network read.
        if !snapshot.category.is_fetchable() {
            cx.notify();
            return;
        }
        let Some(generation) = self.transfer.begin_preview_tab_reload(tab_id) else {
            return;
        };
        self.start_sftp_preview_load_job(
            snapshot.session_id.clone(),
            tab_id.to_string(),
            snapshot.remote_file_path(),
            snapshot.category,
            generation,
            window,
            cx,
        );
        cx.notify();
    }

    /// Close a preview tab. Read-only, so there is nothing to confirm.
    pub(in crate::features) fn close_transfer_preview_tab(
        &mut self,
        tab_id: &str,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.transfer.close_preview_tab(tab_id),
            crate::features::transfers::TransferPreviewCloseOutcome::Closed
        ) {
            self.shell
                .set_status(t!("filePreview.statusTabClosed").to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_preview(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.transfer.close_preview(),
            crate::features::transfers::TransferPreviewCloseOutcome::Closed
        ) {
            self.shell
                .set_status(t!("filePreview.statusClosed").to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn activate_transfer_preview_tab(
        &mut self,
        tab_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.activate_preview_tab(tab_id) {
            cx.notify();
        }
    }

    /// Open the currently previewed file in the external editor as a fallback.
    ///
    /// Uses the tab's own session/config, not the active session, so opening a
    /// preview that belongs to a background session's tab does not accidentally
    /// route through whatever session happens to be active.
    pub(in crate::features) fn open_active_transfer_preview_external(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.transfer.active_preview_tab().cloned() else {
            return;
        };
        let entry = SftpFileEntry {
            name: tab.name,
            path: tab.remote_path,
            file_type: nyaterm_files::sftp::SftpFileType::File,
            size: tab.size,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: tab.modified_at,
            raw_path_token: tab.raw_path_token,
            symlink_target_is_directory: false,
        };
        self.open_transfer_external_for_session(entry, tab.session_id, cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::features) fn start_sftp_preview_load_job(
        &mut self,
        session_id: Option<String>,
        tab_id: String,
        remote_file_path: RemoteFilePath,
        category: PreviewCategory,
        generation: RemoteTextGeneration,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_path = remote_file_path.display_path.clone();
        let service = match self.transfer_file_browser_service(session_id.as_deref()) {
            Ok(service) => service,
            Err(error) => {
                let error = error.to_string();
                self.transfer
                    .fail_preview_tab(&tab_id, generation, error.clone());
                self.shell.set_status(error);
                cx.notify();
                return;
            }
        };
        let max_bytes = category.max_bytes();
        let id = self.transfer.next_transfer_job_id("sftp-preview");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::LoadPreview {
                remote_path: remote_path.clone(),
                tab_id: tab_id.clone(),
                generation,
            },
            status: TransferJobStatus::Running,
            detail: t!("filePreview.detailPreviewing", path = remote_path.clone()).to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-load-preview",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let content =
                    load_preview_content(&service, &remote_file_path, category, max_bytes);
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(Ok(TransferJobOutput::PreviewLoaded {
                        tab_id,
                        remote_path,
                        generation,
                        content,
                    })),
                });
            },
        );
        cx.notify();
    }

    /// If the active PDF preview's current page is not yet rasterized, spawn a
    /// background job to rasterize just that page and return it through the
    /// transfer event channel. Called from the view whenever the active tab is a
    /// PDF, so paging simply requests the newly-visible page.
    pub(in crate::features) fn request_active_pdf_page_render(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.transfer.pdf_page_request_for_active_tab() else {
            return;
        };
        self.start_pdf_page_render_job(request, cx);
    }

    pub(in crate::features) fn start_pdf_page_render_job(
        &mut self,
        request: crate::features::transfers::PdfPageRequest,
        cx: &mut Context<Self>,
    ) {
        let crate::features::transfers::PdfPageRequest {
            tab_id,
            session_id,
            remote_path,
            generation,
            page_index,
            bytes,
        } = request;
        let id = self.transfer.next_transfer_job_id("pdf-page");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id,
            kind: TransferJobKind::RasterizePdfPage {
                remote_path: remote_path.clone(),
                tab_id: tab_id.clone(),
                generation,
                page_index,
            },
            status: TransferJobStatus::Running,
            detail: t!(
                "filePreview.detailRenderingPage",
                page = page_index + 1,
                path = remote_path.clone()
            )
            .to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "pdf-render-page",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let page = crate::features::transfers::preview::decode::rasterize_pdf_page(
                    &bytes, page_index,
                );
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(Ok(TransferJobOutput::PdfPageRendered {
                        tab_id,
                        generation,
                        page_index,
                        page,
                    })),
                });
            },
        );
        cx.notify();
    }
}

/// Fetch and decode a preview on the worker thread.
///
/// A size check runs before the fetch using the category ceiling, and a second
/// authoritative check runs against the returned byte count for text (whose
/// length is not known until read). Failures return an error card rather than
/// propagating, so the tab always resolves.
fn load_preview_content(
    service: &nyaterm_files::file_browser::FileBrowserService,
    remote_file_path: &RemoteFilePath,
    category: PreviewCategory,
    max_bytes: u64,
) -> PreviewContent {
    if category.is_binary() {
        match service.read_file_bytes_path(remote_file_path, max_bytes) {
            Ok(file) => {
                if !preview_within_limit(category, file.size) {
                    return PreviewContent::Error(preview_too_large_message(category));
                }
                decode_binary_content(category, file.content_bytes)
            }
            Err(error) => PreviewContent::Error(error.to_string()),
        }
    } else {
        match service.read_text_file_path(remote_file_path, max_bytes) {
            Ok(file) => {
                if !preview_within_limit(category, file.size) {
                    return PreviewContent::Error(preview_too_large_message(category));
                }
                decode_text_content(category, file.content)
            }
            Err(error) => PreviewContent::Error(error.to_string()),
        }
    }
}

fn preview_too_large_message(category: PreviewCategory) -> String {
    let mib = category.max_bytes() / (1024 * 1024);
    t!("filePreview.statusTooLarge", mib = mib).to_string()
}

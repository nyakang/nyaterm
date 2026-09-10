use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use gpui::{Context, Image};
use nyaterm_transport::{FileBrowserService, SftpTransferControl};

impl NyaTermApp {
    pub(super) fn paste_clipboard_image(&mut self, image: Image, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let service = match self.file_browser_service_for_session(&session_id) {
            Ok(service) => service,
            Err(_) => {
                self.clipboard_image_failure(cx);
                return;
            }
        };
        let local = matches!(service, FileBrowserService::Local(_));
        let cwd = self.session.cwd(&session_id).map(str::to_owned);
        let options = self.sftp_transfer_options();
        let name = format!(
            "nyaterm-clipboard-{}.{}",
            nyaterm_core::uuid(),
            image.format.extension()
        );
        let local_path = std::env::temp_dir().join(&name);
        let control = SftpTransferControl::new();
        let id = self.transfer.next_transfer_job_id("clipboard-image");
        if !local {
            self.transfer.enqueue_transfer_job(TransferJobState {
                id: id.clone(),
                session_id: Some(session_id.clone()),
                kind: TransferJobKind::Upload {
                    local_path: local_path.clone(),
                    remote_path: name.clone(),
                },
                status: TransferJobStatus::Running,
                detail: rust_i18n::t!("terminal.uploadingClipboardImage").to_string(),
                created_at_ms: TransferJobState::now_ms(),
                display_name: name.clone(),
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: Some(control.clone()),
            });
        }
        let progress_tx = self.transfer.transfer_event_sender();
        let finish_tx = progress_tx.clone();
        let progress_id = id.clone();
        let worker_control = control.clone();
        let jobs = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(jobs.submit_task("clipboard-image", move |_| {
                if image.bytes.len() > 32 * 1024 * 1024 {
                    return Err("clipboard image is too large".to_string());
                }
                std::fs::write(&local_path, &image.bytes)
                    .map_err(|_| "cannot write clipboard image")?;
                if let FileBrowserService::Remote(service) = service {
                    let result = (|| {
                        worker_control
                            .check_cancelled()
                            .map_err(|error| error.to_string())?;
                        let directory = match cwd {
                            Some(cwd) => cwd,
                            None => service.home_dir().map_err(|error| error.to_string())?,
                        };
                        let remote_path = format!("{}/{}", directory.trim_end_matches('/'), name);
                        let summary = service
                            .upload_file_with_progress_and_control_options(
                                local_path.clone(),
                                &remote_path,
                                worker_control,
                                options,
                                move |progress| {
                                    let _ = progress_tx.unbounded_send(TransferJobResult {
                                        id: progress_id.clone(),
                                        event: TransferJobEvent::Progress(progress),
                                    });
                                },
                            )
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>((remote_path, Some(summary)))
                    })();
                    let _ = std::fs::remove_file(local_path);
                    result
                } else {
                    Ok((local_path.to_string_lossy().into_owned(), None))
                }
            }))
            .await
            .and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok((path, summary)) => {
                        if let Some(summary) = summary {
                            let _ = finish_tx.unbounded_send(TransferJobResult {
                                id,
                                event: TransferJobEvent::Finished(Ok(TransferJobOutput::Summary(
                                    summary,
                                ))),
                            });
                        }
                        if !control.is_cancelled()
                            && app.session.has_session(&session_id)
                            && !app.session.is_disconnected(&session_id)
                        {
                            let quoted = if local {
                                nyaterm_core::terminal::file_drop::quote_local_path(&path)
                            } else {
                                format!("'{}'", path.replace('\'', "'\\''"))
                            };
                            app.send_terminal_input_to_session(session_id, quoted.into_bytes(), cx);
                        }
                    }
                    Err(error) => {
                        if !local {
                            let _ = finish_tx.unbounded_send(TransferJobResult {
                                id,
                                event: TransferJobEvent::Finished(Err(error)),
                            });
                        }
                        if !control.is_cancelled() {
                            app.clipboard_image_failure(cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn clipboard_image_failure(&self, cx: &mut Context<Self>) {
        self.notify_background_operation(
            "clipboard-image",
            nyaterm_ui::notification::NyaNotificationKind::Error,
            rust_i18n::t!("terminal.clipboardImageFailed").to_string(),
            cx,
        );
    }
}

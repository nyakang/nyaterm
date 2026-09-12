use rust_i18n::t;

use gpui::{Context, KeyDownEvent, MouseDownEvent, Window};

use super::super::transfer_widgets::transfer_job_title;
use super::helpers::transfer_job_local_target_path;
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn select_transfer_job(
        &mut self,
        job_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.select_transfer_job_id(&job_id) {
            self.shell.set_status(format!("selected transfer {job_id}"));
        } else {
            self.shell.set_status("transfer job not found".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn open_transfer_job_menu(
        &mut self,
        job_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(self.transfer.queue_focus(), cx);
        if self
            .transfer
            .open_transfer_job_menu_at(&job_id, event.position.x, event.position.y)
        {
            self.shell.set_status("transfer menu opened".to_string());
        } else {
            self.shell.set_status("transfer job not found".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_job_menu(&mut self, cx: &mut Context<Self>) {
        self.transfer.close_transfer_job_menu();
        cx.notify();
    }

    pub(in crate::features) fn request_delete_transfer_job(
        &mut self,
        job_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.transfer.transfer_job(&job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };
        if !self.can_delete_transfer_job(&job_id) {
            self.shell
                .set_status(format!("transfer {} cannot be deleted yet", job.id));
            cx.notify();
            return;
        }
        let title = transfer_job_title(&job.kind);
        let description = t!("fileTransfer.deleteConfirmDesc", name = title).to_string();
        self.transfer.select_transfer_job_id(&job_id);
        self.defer_transfer_panel_snapshot_flush(cx);
        self.open_confirm_dialog(
            (
                t!("fileTransfer.deleteConfirmTitle").to_string(),
                description,
                t!("fileTransfer.delete").to_string(),
                true,
                move |app, _, cx| {
                    let removed = app.transfer.delete_transfer_job(&job_id);
                    app.shell.set_status(if removed {
                        format!("deleted transfer {job_id}")
                    } else {
                        "transfer job not found".to_string()
                    });
                    app.defer_transfer_panel_snapshot_flush(cx);
                    cx.notify();
                    true
                },
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn request_delete_selected_transfer_job(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_session_id = self.session.active_id();
        let job_id = self
            .transfer
            .selected_or_latest_visible_transfer_job_id(active_session_id);
        let Some(job_id) = job_id else {
            self.shell.set_status("transfer queue is empty".to_string());
            cx.notify();
            return;
        };
        self.request_delete_transfer_job(job_id, window, cx);
    }

    pub(in crate::features) fn reveal_transfer_job_target_directory(
        &mut self,
        job_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.transfer.transfer_job(&job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };
        let Some(target_path) =
            transfer_job_local_target_path(job).filter(|path| !path.as_os_str().is_empty())
        else {
            self.shell
                .set_status(format!("transfer {} has no local target", job.id));
            cx.notify();
            return;
        };
        // Keep filesystem checks and file-manager requests off the UI thread.
        let request = cx.background_executor().spawn(async move {
            let mut target_path = std::path::absolute(target_path)?;
            // Follow directory symlinks, preserving the existing open-directory behavior.
            let target_is_dir = match std::fs::metadata(&target_path) {
                Ok(metadata) => metadata.is_dir(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    anyhow::ensure!(target_path.pop(), "transfer target has no parent directory");
                    anyhow::ensure!(
                        std::fs::metadata(&target_path)?.is_dir(),
                        "transfer target directory is unavailable"
                    );
                    true
                }
                Err(error) => return Err(error.into()),
            };

            #[cfg(target_os = "macos")]
            if target_is_dir {
                // Explicitly use Finder: the default handler could execute an app bundle.
                let status = std::process::Command::new("/usr/bin/open")
                    .args(["-b", "com.apple.finder", "--"])
                    .arg(&target_path)
                    .output()?
                    .status;
                anyhow::ensure!(status.success(), "Finder request failed: {status}");
                return Ok(None);
            }

            #[cfg(target_os = "linux")]
            if !target_is_dir {
                use gtk::gio;
                use gtk::gio::prelude::FileExt;

                // GPUI opens the file for the portal. Check access without opening a FIFO/device.
                let info = gio::File::for_path(&target_path).query_info(
                    "standard::type,access::can-read",
                    gio::FileQueryInfoFlags::NONE,
                    gio::Cancellable::NONE,
                )?;
                if info.file_type() != gio::FileType::Regular || !info.boolean("access::can-read") {
                    anyhow::ensure!(target_path.pop(), "transfer target has no parent directory");
                    anyhow::ensure!(
                        std::fs::metadata(&target_path)?.is_dir(),
                        "transfer target directory is unavailable"
                    );
                    return Ok(Some((target_path, true)));
                }
            }
            Ok::<_, anyhow::Error>(Some((target_path, target_is_dir)))
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(target) => {
                        if let Some((target_path, target_is_dir)) = target {
                            if target_is_dir {
                                #[cfg(target_os = "linux")]
                                {
                                    use gtk::gio::prelude::FileExt;
                                    // The URI API preserves GPUI's Wayland activation-token handling.
                                    cx.open_url(
                                        gtk::gio::File::for_path(&target_path).uri().as_str(),
                                    );
                                }
                                #[cfg(not(target_os = "linux"))]
                                cx.open_with_system(&target_path);
                            } else {
                                cx.reveal_path(&target_path);
                            }
                        }
                        this.shell
                            .set_status("requested transfer location in file manager".to_string());
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("cannot show transfer location: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn handle_transfer_queue_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let unmodified = !keystroke.modifiers.alt
            && !keystroke.modifiers.control
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.shift;
        if unmodified && keystroke.key == "delete" {
            cx.stop_propagation();
            self.request_delete_selected_transfer_job(window, cx);
        }
    }

    pub(in crate::features) fn can_delete_transfer_job(&self, job_id: &str) -> bool {
        let active_session_id = self.session.active_id();
        self.transfer
            .transfer_job_can_be_deleted(job_id, active_session_id)
    }
}

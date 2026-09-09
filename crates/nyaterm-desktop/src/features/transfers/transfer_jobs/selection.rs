use rust_i18n::t;

use gpui::{Context, KeyDownEvent, MouseDownEvent, Window};

use super::super::transfer_widgets::transfer_job_title;
use super::helpers::{transfer_job_local_target_path, transfer_job_reveal_dir};
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
        let Some(target_path) = transfer_job_local_target_path(job) else {
            self.shell
                .set_status(format!("transfer {} has no local target", job.id));
            cx.notify();
            return;
        };
        let target_dir = transfer_job_reveal_dir(target_path);
        cx.reveal_path(&target_dir);
        self.shell.set_status(format!(
            "opened transfer directory {}",
            target_dir.display()
        ));
        cx.notify();
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

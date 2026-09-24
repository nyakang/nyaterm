use rust_i18n::t;

use gpui::{Context, KeyDownEvent, Window};
use nyaterm_core::{
    DiagnosticsExportOptions, DiagnosticsRuntimeSnapshot, export_diagnostics_archive,
};
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_transport::SessionKind;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};
use crate::models::DiagnosticsPathPromptResult;
use crate::models::TransferJobStatus;

impl NyaTermApp {
    pub(in crate::features) fn lock_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_shared_screen_lock(true, window, cx);
        self.broadcast_screen_lock(true, cx);
    }

    pub(crate) fn apply_shared_screen_lock(
        &mut self,
        locked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !locked {
            self.security.deactivate_screen_lock();
            let restore_focus = self.security.take_screen_lock_restore_focus();
            self.ensure_idle_lock_clock(cx);
            self.forget_text_inputs("lock-screen.password");
            self.shell.set_status("screen unlocked".to_string());
            if let Some(focus) = restore_focus {
                window.focus(&focus, cx);
            }
            self.ensure_pending_focus_clock(cx);
            self.resume_startup_restore_if_unlocked(window, cx);
            cx.notify();
            return;
        }
        let lock_status = if self.settings.summary().has_master_password {
            t!("lockScreen.passwordPlaceholder").to_string()
        } else {
            String::new()
        };
        self.security.remember_screen_lock_focus(window.focused(cx));
        self.security.activate_screen_lock(lock_status);
        self.forget_text_inputs("lock-screen.password");
        self.shell.set_status("screen locked".to_string());
        if self.settings.summary().has_master_password {
            let field = self.text_input("lock-screen.password", "", TextInputSetup::masked(), cx);
            window.focus(&field.read(cx).focus_handle(), cx);
        } else {
            window.focus(self.security.screen_lock_focus(), cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn unlock_app(&mut self, cx: &mut Context<Self>) {
        self.security.deactivate_screen_lock();
        let restore_focus = self.security.take_screen_lock_restore_focus();
        // Unlocking resets the idle timer, so the clock starts counting again.
        self.ensure_idle_lock_clock(cx);
        self.forget_text_inputs("lock-screen.password");
        self.shell.set_status("screen unlocked".to_string());
        self.broadcast_screen_lock(false, cx);
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.security.screen_locked() {
                    if let Some(focus) = restore_focus {
                        window.focus(&focus, cx);
                    }
                    this.resume_startup_restore_if_unlocked(window, cx);
                }
            });
        })
        .detach();
        self.ensure_pending_focus_clock(cx);
        cx.notify();
    }

    fn broadcast_screen_lock(&self, locked: bool, cx: &mut Context<Self>) {
        let Some(controller) = self.desktop_controller.clone() else {
            return;
        };
        let workspace_id = self.workspace_id;
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.set_screen_locked(locked, workspace_id, cx)
            });
        });
    }

    pub(in crate::features) fn submit_lock_unlock(&mut self, cx: &mut Context<Self>) {
        if !self.settings.summary().has_master_password {
            self.unlock_app(cx);
            return;
        }

        let password: nyaterm_core::SecretString =
            self.security.screen_lock_password_draft().to_owned().into();
        let request_password = password.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.verify_master_password(request_password.expose_secret())
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(true)
                        if this.security.screen_lock_password_draft()
                            == password.expose_secret() =>
                    {
                        this.unlock_app(cx);
                    }
                    Ok(true) => {}
                    Ok(false)
                        if this.security.screen_lock_password_draft()
                            == password.expose_secret() =>
                    {
                        let status = t!("lockScreen.wrongPassword").to_string();
                        this.security.clear_screen_lock_password_with_status(status);
                        this.reset_text_input("lock-screen.password", "", cx);
                        this.shell.set_status("screen unlock rejected".to_string());
                    }
                    Ok(false) => {}
                    Err(error)
                        if this.security.screen_lock_password_draft()
                            == password.expose_secret() =>
                    {
                        let status = format!("{}: {error}", t!("lockScreen.unlockFailed"));
                        this.security.clear_screen_lock_password_with_status(status);
                        this.reset_text_input("lock-screen.password", "", cx);
                        this.shell.set_status("screen unlock failed".to_string());
                    }
                    Err(_) => {}
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn handle_lock_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        match keystroke.key.as_str() {
            "enter" => self.submit_lock_unlock(cx),
            "escape" if !self.settings.summary().has_master_password => self.unlock_app(cx),
            "escape" => {
                let status = t!("lockScreen.passwordPlaceholder").to_string();
                self.security.clear_screen_lock_password_with_status(status);
                self.reset_text_input("lock-screen.password", "", cx);
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn apply_lock_password_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let status = t!("lockScreen.passwordPlaceholder").to_string();
        self.security.set_screen_lock_password_draft(text, status);
        cx.notify();
    }

    pub(in crate::features) fn reveal_log_dir(&mut self, cx: &mut Context<Self>) {
        match std::fs::create_dir_all(self.runtime.log_dir()) {
            Ok(()) => {
                cx.reveal_path(self.runtime.log_dir());
                self.shell.set_status(format!(
                    "opened log directory {}",
                    self.runtime.log_dir().display()
                ));
            }
            Err(error) => {
                self.shell
                    .set_status(format!("failed to prepare log directory: {error}"));
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn prompt_diagnostics_export(&mut self, cx: &mut Context<Self>) {
        if !self.settings.begin_diagnostics_path_prompt() {
            self.shell
                .set_status("diagnostics path picker is already open".to_string());
            cx.notify();
            return;
        }

        let directory = self.runtime.log_dir().to_path_buf();
        let receiver = cx.prompt_for_new_path(&directory, Some("nyaterm-diagnostics.zip"));
        let runtime = self.runtime.clone();
        let options = self.diagnostics_export_options();
        let scheduler = self.blocking_jobs.clone();
        self.shell
            .set_status("selecting diagnostics export destination".to_string());
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(path))) => {
                    let task = scheduler.submit_task("diagnostics-export", move |_| {
                        match export_diagnostics_archive(&runtime, &options, &path) {
                            Ok(info) => DiagnosticsPathPromptResult::Exported(info),
                            Err(error) => DiagnosticsPathPromptResult::Failed(error.to_string()),
                        }
                    });
                    await_blocking_job(task)
                        .await
                        .unwrap_or_else(DiagnosticsPathPromptResult::Failed)
                }
                Ok(Ok(None)) => DiagnosticsPathPromptResult::Cancelled,
                Ok(Err(error)) => DiagnosticsPathPromptResult::Failed(error.to_string()),
                Err(_) => DiagnosticsPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_diagnostics_path_prompt_result(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn apply_diagnostics_path_prompt_result(
        &mut self,
        result: DiagnosticsPathPromptResult,
    ) {
        if !self.settings.finish_diagnostics_path_prompt() {
            return;
        }
        match result {
            DiagnosticsPathPromptResult::Exported(info) => {
                self.shell.set_status(format!(
                    "diagnostics exported to {} ({} log file(s), {} bytes)",
                    info.output_path.display(),
                    info.log_files,
                    info.bytes
                ));
            }
            DiagnosticsPathPromptResult::Cancelled => {
                self.shell
                    .set_status("diagnostics export cancelled".to_string());
            }
            DiagnosticsPathPromptResult::Failed(error) => {
                self.shell
                    .set_status(format!("diagnostics export failed: {error}"));
            }
            DiagnosticsPathPromptResult::Closed => {
                self.shell
                    .set_status("diagnostics path picker closed before returning".to_string());
            }
        }
    }

    pub(in crate::features) fn diagnostics_export_options(&self) -> DiagnosticsExportOptions {
        DiagnosticsExportOptions {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            language: self.settings.summary().language.clone(),
            log_level: self.settings.summary().diagnostics_level.clone(),
            retention_days: self.settings.summary().diagnostics_retention_days,
            runtime_snapshot: self.diagnostics_runtime_snapshot(),
        }
    }

    pub(in crate::features) fn diagnostics_runtime_snapshot(&self) -> DiagnosticsRuntimeSnapshot {
        let sessions = self.session.ordered_sessions();
        let mut local_sessions = 0;
        let mut ssh_sessions = 0;
        let mut telnet_sessions = 0;
        let mut raw_tcp_sessions = 0;
        let mut serial_sessions = 0;
        for session in &sessions {
            match session.kind {
                SessionKind::LocalPty => local_sessions += 1,
                SessionKind::Ssh => ssh_sessions += 1,
                SessionKind::Telnet => telnet_sessions += 1,
                SessionKind::RawTcp => raw_tcp_sessions += 1,
                SessionKind::Serial => serial_sessions += 1,
                SessionKind::Rdp => {}
                SessionKind::Vnc => {}
            }
        }

        let open_tunnels = self.tunnel_state.open_count();
        let mut running_transfers = 0;
        let mut paused_transfers = 0;
        let mut completed_transfers = 0;
        let mut failed_transfers = 0;
        for job in self.transfer.transfer_jobs() {
            match job.status {
                TransferJobStatus::Running | TransferJobStatus::Cancelling => {
                    running_transfers += 1
                }
                TransferJobStatus::Paused => paused_transfers += 1,
                TransferJobStatus::Completed => completed_transfers += 1,
                TransferJobStatus::Failed => failed_transfers += 1,
                TransferJobStatus::Cancelled => {}
            }
        }

        DiagnosticsRuntimeSnapshot {
            active_sessions: sessions.len(),
            local_sessions,
            ssh_sessions,
            telnet_sessions,
            raw_tcp_sessions,
            serial_sessions,
            open_tunnels,
            pending_tunnels: self.tunnel_state.pending_count(),
            saved_connections: self.connection_state.connections().len(),
            saved_tunnels: self.tunnel_state.tunnels().len(),
            running_transfers,
            paused_transfers,
            completed_transfers,
            failed_transfers,
        }
    }
}

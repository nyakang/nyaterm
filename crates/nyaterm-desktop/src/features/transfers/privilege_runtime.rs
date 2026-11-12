//! Temporary manual-file authorization. The session owns the grant; this owns only the prompt.
use crate::features::session::remote_files::{FileAccessPurpose, FilePrivilegeStatus};
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};
use crate::models::{TransferJobKind, TransferJobStatus};
use gpui::{AnyWindowHandle, Context, Window};
use nyaterm_core::SecretString;
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_ui::{NyaDialogHandle, NyaDialogWindowExt as _};
use rust_i18n::t;

pub(in crate::features) const PASSWORD_INPUT: &str = "transfers.sudo.password";

pub(in crate::features) struct FileSudoPrompt {
    id: uuid::Uuid,
    session_id: String,
    origin: AnyWindowHandle,
    dialog: Option<NyaDialogHandle>,
    generation: Option<u64>,
    pub(in crate::features) error: Option<String>,
}

pub(super) fn disposable_read(kind: &TransferJobKind) -> bool {
    matches!(
        kind,
        TransferJobKind::InitialDirectory
            | TransferJobKind::ListDir { .. }
            | TransferJobKind::ListChildren { .. }
            | TransferJobKind::ListTree { .. }
            | TransferJobKind::ResolveHome
            | TransferJobKind::SyncCwd { .. }
            | TransferJobKind::LoadProperties { .. }
            | TransferJobKind::LoadEditor { .. }
            | TransferJobKind::LoadPreview { .. }
            | TransferJobKind::RasterizePdfPage { .. }
    )
}

impl NyaTermApp {
    pub(super) fn refresh_manual_transfer_session(
        &mut self,
        session: &mut super::FileTransferSession,
    ) -> anyhow::Result<()> {
        if matches!(
            session.service,
            nyaterm_files::file_browser::FileBrowserService::Remote(_)
        ) {
            let id = session
                .session_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("source SSH session is unavailable"))?;
            session.service = self.file_browser_service_for_session(id)?;
        }
        Ok(())
    }

    pub(super) fn mark_file_job_access(&mut self, id: &str) {
        let sudo = self
            .transfer
            .transfer_job(id)
            .and_then(|job| job.session_id.as_deref())
            .is_some_and(|session| self.file_sudo_status(session).is_sudo())
            || self.transfer.file_operation(id).is_some_and(|operation| {
                let (source, target) = operation.endpoints();
                [source.session_id(), target.session_id()]
                    .into_iter()
                    .flatten()
                    .any(|session| self.file_sudo_status(session).is_sudo())
            });
        if let Some(job) = self.transfer.transfer_job_mut(id) {
            job.ensure_presentation_fields();
            let name = job
                .display_name
                .strip_prefix("[sudo] ")
                .unwrap_or(&job.display_name)
                .to_owned();
            job.display_name = if sudo { format!("[sudo] {name}") } else { name };
        }
    }

    pub(in crate::features) fn file_access_generation(&self, session: &str) -> Option<u64> {
        self.session
            .file_access_generation(&self.ssh_scoped_session_id(session))
    }

    pub(in crate::features) fn file_sudo_status(&self, session: &str) -> FilePrivilegeStatus {
        self.session
            .file_privilege_status(&self.ssh_scoped_session_id(session))
    }

    pub(in crate::features) fn file_sudo_available(&self, session: &str) -> bool {
        let session = self.ssh_scoped_session_id(session);
        self.session
            .metadata(&session)
            .and_then(|m| m.ssh_config.as_ref())
            .is_some_and(|config| {
                config.remote_file_browser_enabled()
                    && config.effective_runtime_mode()
                        != nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp
            })
            && self
                .session
                .ssh_connection_for_session(&session)
                .is_some_and(|connection| {
                    connection.handle().shell_availability()
                        != nyaterm_transport::connection_attempt::ShellAvailability::Unavailable
                })
    }

    fn file_sudo_busy(&self, session: &str) -> bool {
        let matches = |id: &str| self.ssh_scoped_session_id(id) == session;
        self.transfer.has_external_file_tasks(&self.file_privilege_aliases(session)) || self.transfer.transfer_jobs().iter().any(|job| {
            if disposable_read(&job.kind) || !matches!(job.status, TransferJobStatus::Queued | TransferJobStatus::Running | TransferJobStatus::Paused | TransferJobStatus::Cancelling) {
                return false;
            }
            job.session_id.as_deref().is_some_and(matches)
                || matches!(&job.kind, TransferJobKind::SendTo { target_session_id, .. } if matches(target_session_id))
                || self.transfer.file_operation(&job.id).is_some_and(|operation| {
                    let (source, target) = operation.endpoints();
                    source.session_id().is_some_and(matches) || target.session_id().is_some_and(matches)
                })
        })
    }

    pub(in crate::features) fn toggle_file_sudo(
        &mut self,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.ssh_scoped_session_id(&session);
        if self.transfer.sudo_prompt.is_some() || self.file_sudo_status(&session).is_busy() {
            return;
        }
        if self.file_sudo_busy(&session) {
            self.transfer
                .set_browser_status(t!("fileSudo.busy").to_string());
            self.open_form_dialog(
                (
                    t!("fileSudo.title").to_string(),
                    420.,
                    t!("common.confirm").to_string(),
                    |app, _, cx| app.file_sudo_busy_view(cx),
                    |_, _, _| true,
                    |_, _| {},
                ),
                window,
                cx,
            );
            return;
        }
        if self.file_sudo_status(&session).is_sudo() {
            let Ok((generation, grant)) = self.session.begin_disable_file_sudo(&session) else {
                return;
            };
            self.invalidate_file_privilege_views(&session, cx);
            let scheduler = self.blocking_jobs.clone();
            cx.spawn_in(window, async move |this, cx| {
                let task = scheduler.submit_task("file-sudo-close", move |_| grant.close());
                let _ = await_blocking_job(task).await;
                let _ = this.update(cx, |this, cx| {
                    if this.session.finish_disable_file_sudo(&session, generation) {
                        this.refresh_file_privilege_views(&session, cx);
                    }
                });
            })
            .detach();
            cx.notify();
            return;
        }
        if !self.file_sudo_available(&session) {
            return;
        }
        self.forget_text_inputs(PASSWORD_INPUT);
        let field = self.text_input(PASSWORD_INPUT, "", TextInputSetup::masked(), cx);
        let prompt_id = uuid::Uuid::new_v4();
        self.transfer.sudo_prompt = Some(FileSudoPrompt {
            id: prompt_id,
            session_id: session,
            origin: window.window_handle(),
            dialog: None,
            generation: None,
            error: None,
        });
        let dialog = self.open_cancellable_form_dialog(
            (
                t!("fileSudo.title").to_string(),
                440.,
                t!("fileSudo.enable").to_string(),
                |app, _, cx| app.file_sudo_prompt_view(cx),
                move |app, window, cx| {
                    if app
                        .transfer
                        .sudo_prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.id == prompt_id)
                    {
                        app.submit_file_sudo(window, cx);
                    }
                    false
                },
                move |app, cx| {
                    if app
                        .transfer
                        .sudo_prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.id == prompt_id)
                    {
                        app.cancel_file_sudo_prompt(cx);
                    }
                },
                |app| {
                    app.transfer
                        .sudo_prompt
                        .as_ref()
                        .is_some_and(|p| p.generation.is_some())
                },
            ),
            window,
            cx,
        );
        if let Some(prompt) = self.transfer.sudo_prompt.as_mut() {
            prompt.dialog = Some(dialog);
        }
        window.focus(&field.read(cx).focus_handle(), cx);
        cx.notify();
    }

    pub(in crate::features) fn file_sudo_prompt_error(&self) -> Option<&str> {
        self.transfer
            .sudo_prompt
            .as_ref()
            .and_then(|prompt| prompt.error.as_deref())
    }

    pub(in crate::features) fn cancel_file_sudo_for_sessions(
        &mut self,
        sessions: &[String],
        cx: &mut Context<Self>,
    ) {
        if self.transfer.sudo_prompt.as_ref().is_some_and(|prompt| {
            sessions
                .iter()
                .any(|id| self.ssh_scoped_session_id(id) == prompt.session_id)
        }) {
            self.cancel_file_sudo_prompt(cx);
        }
    }

    pub(in crate::features) fn cancel_file_sudo_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.transfer.sudo_prompt.take() {
            if let Some(generation) = prompt.generation {
                self.session
                    .cancel_file_sudo(&prompt.session_id, generation);
                self.refresh_file_privilege_views(&prompt.session_id, cx);
            }
            if let Some(dialog) = prompt.dialog {
                let origin = prompt.origin;
                cx.defer(move |cx| {
                    let _ = origin.update(cx, |_, window, cx| {
                        if dialog.is_top(window, cx) {
                            window.close_nya_dialog(cx);
                        }
                    });
                });
            }
        }
        self.forget_text_inputs(PASSWORD_INPUT);
        cx.notify();
    }

    fn submit_file_sudo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.transfer.sudo_prompt.as_ref() else {
            return;
        };
        if prompt.generation.is_some() || prompt.origin != window.window_handle() {
            return;
        }
        let session = prompt.session_id.clone();
        if self.file_sudo_busy(&session) {
            self.transfer.sudo_prompt.as_mut().unwrap().error =
                Some(t!("fileSudo.busy").to_string());
            cx.notify();
            return;
        }
        let Some(config) = self
            .session
            .metadata(&session)
            .and_then(|m| m.ssh_config.clone())
        else {
            self.cancel_file_sudo_prompt(cx);
            return;
        };
        let Some(connection) = self.session.ssh_multiplex_handle_for_session(&session) else {
            self.cancel_file_sudo_prompt(cx);
            return;
        };
        if self
            .remote_file_service_for_session(
                &session,
                config.clone(),
                FileAccessPurpose::Automation,
            )
            .is_err()
        {
            self.cancel_file_sudo_prompt(cx);
            return;
        }
        let password = self
            .existing_text_input(PASSWORD_INPUT)
            .map(|field| {
                let password = SecretString::from(field.read(cx).value(cx));
                field.update(cx, |field, cx| {
                    field.set_content_silent("", cx);
                    field.set_disabled(true, cx);
                    field.hide_secret(cx);
                });
                password
            })
            .filter(|password| !password.expose_secret().is_empty());
        let Ok((generation, lease)) =
            self.session
                .begin_file_sudo(&session, connection.clone(), password)
        else {
            return;
        };
        let prompt = self.transfer.sudo_prompt.as_mut().unwrap();
        prompt.generation = Some(generation);
        prompt.error = None;
        self.invalidate_file_privilege_views(&session, cx);
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("file-sudo-authorize", move |_| {
                let service = RemoteFileService::with_sudo(config, connection, lease)
                    .map_err(|error| file_sudo_error(&error))?;
                service
                    .list_dir(".")
                    .map_err(|error| file_sudo_error(&error))?;
                Ok::<_, String>(service)
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_file_sudo_prompt(session, generation, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_file_sudo_prompt(
        &mut self,
        session: String,
        generation: u64,
        result: Result<RemoteFileService, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.transfer.sudo_prompt.as_ref() else {
            return;
        };
        if prompt.session_id != session
            || prompt.generation != Some(generation)
            || prompt.origin != window.window_handle()
        {
            return;
        }
        let Some(dialog) = prompt.dialog.as_ref() else {
            return;
        };
        if !dialog.is_active() {
            self.cancel_file_sudo_prompt(cx);
            return;
        }
        if !dialog.is_top(window, cx) {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_file_sudo_prompt(session, generation, result, window, cx)
                });
            })
            .detach();
            return;
        }
        let result = result.and_then(|service| {
            if service.authorization_is_valid() {
                Ok(service)
            } else {
                Err(t!("fileSudo.expired").to_string())
            }
        });
        let error = result.as_ref().err().cloned();
        let accepted = self
            .session
            .finish_file_sudo(&session, generation, result.ok());
        if !accepted {
            self.cancel_file_sudo_prompt(cx);
            window.close_nya_dialog(cx);
            return;
        }
        if let Some(error) = error {
            let prompt = self.transfer.sudo_prompt.as_mut().unwrap();
            prompt.generation = None;
            prompt.error = Some(error);
            if let Some(field) = self.existing_text_input(PASSWORD_INPUT) {
                field.update(cx, |field, cx| field.set_disabled(false, cx));
                window.focus(&field.read(cx).focus_handle(), cx);
            }
        } else {
            self.transfer.sudo_prompt = None;
            self.forget_text_inputs(PASSWORD_INPUT);
            window.close_nya_dialog(cx);
        }
        self.refresh_file_privilege_views(&session, cx);
        cx.notify();
    }

    fn file_privilege_aliases(&self, session: &str) -> Vec<String> {
        self.session
            .session_ids()
            .filter(|id| self.ssh_scoped_session_id(id) == session)
            .map(str::to_owned)
            .collect()
    }

    fn invalidate_file_privilege_views(&mut self, session: &str, cx: &mut Context<Self>) {
        let aliases = self.file_privilege_aliases(session);
        let reads: Vec<_> = self
            .transfer
            .transfer_jobs()
            .iter()
            .filter(|job| {
                disposable_read(&job.kind)
                    && job
                        .session_id
                        .as_ref()
                        .is_some_and(|id| aliases.contains(id))
            })
            .map(|job| (job.id.clone(), job.control.clone()))
            .collect();
        for (id, control) in reads {
            if let Some(control) = control {
                control.cancel();
            }
            self.transfer.delete_transfer_job(&id);
        }
        for alias in &aliases {
            self.transfer.invalidate_browser_navigation(alias);
            self.transfer.remove_browser_session_cache(alias);
        }
        self.transfer.invalidate_file_readers(&aliases);
        self.flush_transfer_panel_snapshot(cx);
        let workspaces: Vec<_> = self.file_manager.all_workspaces().collect();
        for workspace in workspaces {
            workspace.update(cx, |workspace, cx| {
                for pane in [
                    crate::features::file_manager::PaneId::Left,
                    crate::features::file_manager::PaneId::Right,
                ] {
                    workspace
                        .pane_mut(pane)
                        .source_history
                        .retain(|endpoint, _| {
                            !endpoint
                                .session_id()
                                .is_some_and(|id| aliases.iter().any(|alias| alias == id))
                        });
                }
                for alias in &aliases {
                    for pane in workspace.pane_ids_for_session(alias) {
                        let pane = workspace.pane_mut(pane);
                        pane.generation = pane.generation.wrapping_add(1);
                        pane.entries = Default::default();
                        pane.loading = false;
                    }
                }
                cx.notify();
            });
        }
    }

    fn refresh_file_privilege_views(&mut self, session: &str, cx: &mut Context<Self>) {
        for alias in self.file_privilege_aliases(session) {
            self.refresh_file_manager_session_panes(&alias, cx);
        }
        if self
            .session
            .active_id()
            .is_some_and(|id| self.ssh_scoped_session_id(id) == session)
        {
            self.reload_transfer_browser(cx);
        }
        self.flush_transfer_panel_snapshot(cx);
        cx.notify();
    }
}

pub(in crate::features) fn sudo_label(status: FilePrivilegeStatus) -> String {
    match status {
        FilePrivilegeStatus::Normal => t!("fileSudo.enable"),
        FilePrivilegeStatus::Authorizing | FilePrivilegeStatus::Disabling => {
            t!("fileSudo.changing")
        }
        FilePrivilegeStatus::Elevated => t!("fileSudo.active"),
        FilePrivilegeStatus::Unavailable => t!("fileSudo.expired"),
    }
    .to_string()
}

#[cfg(test)]
mod tests;

fn file_sudo_error(error: &anyhow::Error) -> String {
    use nyaterm_files::sftp::sudo::SftpSudoError;
    use nyaterm_transport::ssh_execution::sudo::SudoError;
    match error.downcast_ref::<SudoError>() {
        Some(SudoError::PasswordRequired) => t!("fileSudo.passwordRequired"),
        Some(SudoError::AuthenticationRejected) => t!("fileSudo.rejected"),
        Some(SudoError::ExecUnavailable) => t!("fileSudo.execUnavailable"),
        Some(SudoError::TerminalRequired) => t!("fileSudo.terminalRequired"),
        Some(SudoError::PolicyDenied) => t!("fileSudo.policyDenied"),
        Some(SudoError::TimedOut) => t!("fileSudo.timeout"),
        Some(SudoError::Cancelled) => t!("fileSudo.cancelled"),
        Some(SudoError::OutputLimit) => t!("fileSudo.outputLimit"),
        Some(SudoError::InvalidPassword) => t!("fileSudo.invalidPassword"),
        Some(SudoError::CommandFailed(_)) => t!("fileSudo.startupFailed"),
        None => match error.downcast_ref::<SftpSudoError>() {
            Some(SftpSudoError::InitializationTimeout) => t!("fileSudo.timeout"),
            Some(SftpSudoError::ProtocolInitialization) => t!("fileSudo.startupFailed"),
            None => t!("fileSudo.validationFailed"),
        },
    }
    .to_string()
}

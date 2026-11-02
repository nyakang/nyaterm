mod state;
mod view;

use std::time::Duration;

use gpui::{Context, Window};
use nyaterm_core::activation::{
    ActivationAction, ExternalConnectionRequest, ParsedActivationRequest, normalize_external_host,
};
use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection};
use nyaterm_transport::LocalSessionConfig;
use nyaterm_ui::NyaDialogWindowExt as _;

use super::NyaTermApp;
use super::session::SavedConnectionStartOptions;
use crate::models::SessionLaunchConfig;
use crate::temporary_ssh_link::{TemporarySshLinkConfig, TemporaryTelnetLinkConfig};

pub(in crate::features) use state::ActivationFeatureState;
use state::ExternalDialog;

impl NyaTermApp {
    pub(crate) fn handle_activation(
        &mut self,
        request: ParsedActivationRequest,
        cx: &mut Context<Self>,
    ) {
        let mut errors = Vec::new();
        for (index, result) in request.actions.into_iter().enumerate() {
            let error = match result {
                Ok(ActivationAction::Activate) => None,
                Ok(ActivationAction::Connect(request)) => (!self.activation.enqueue(request))
                    .then(|| rust_i18n::t!("externalOpen.queueFull").to_string()),
                Err(_) => Some(rust_i18n::t!("externalOpen.invalidUrl").to_string()),
            };
            if let Some(error) = error {
                errors.push(format!("#{}: {error}", index + 1));
            }
        }
        if !errors.is_empty() {
            let message = errors.join("\n");
            self.shell.set_status(message.clone());
            self.notify_background_operation(
                "external-open-errors",
                nyaterm_ui::notification::NyaNotificationKind::Error,
                message,
                cx,
            );
        }
        self.ensure_external_activation_clock(cx);
        cx.notify();
    }

    fn ensure_external_activation_clock(&mut self, cx: &mut Context<Self>) {
        if self.activation.clock_armed || self.activation.is_empty() {
            return;
        }
        let Some(window) = self.shell.main_window() else {
            return;
        };
        self.activation.clock_armed = true;
        cx.spawn(async move |this, cx| {
            loop {
                let keep_running = window
                    .update(cx, |_, window, cx| {
                        this.update(cx, |app, cx| app.pump_external_activation(window, cx))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }

    fn pump_external_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.activation.is_empty() {
            self.activation.clock_armed = false;
            return false;
        }
        if self.security.screen_locked()
            || !self.session.restore_is_complete()
            || !self.shell.terminal_windows_restore_is_complete()
            || self.session.external_start_is_busy()
            || window.has_active_nya_dialog(cx)
        {
            return true;
        }
        self.activation.waiting_for_start = false;
        if let Some(dialog) = self.activation.dialog.clone() {
            match dialog {
                ExternalDialog::Choose { .. } => self.open_external_match_dialog(window, cx),
                ExternalDialog::Confirm { connection, mode } => {
                    self.open_confirm_dialog_with_cancel(
                        (
                            rust_i18n::t!("dialog.postLoginExternalTitle").to_string(),
                            rust_i18n::t!(
                                "dialog.postLoginExternalMessage",
                                name = connection.name.clone()
                            )
                            .to_string(),
                            rust_i18n::t!("common.confirm").to_string(),
                            true,
                            move |app, _, cx| {
                                app.activation.dialog = None;
                                app.start_external_saved(connection.as_ref().clone(), mode, cx);
                                true
                            },
                            |app, _| {
                                app.activation.dialog = None;
                            },
                        ),
                        window,
                        cx,
                    );
                }
            }
            return true;
        }
        let Some(request) = self.activation.pending.pop_front() else {
            return true;
        };
        let matches = self
            .connection_state
            .connections()
            .iter()
            .filter(|connection| external_request_matches_connection(&request, &connection.config))
            .cloned()
            .collect::<Vec<_>>();
        let mode = match &request {
            ExternalConnectionRequest::Ssh { runtime_mode, .. } => *runtime_mode,
            _ => None,
        };
        match matches.as_slice() {
            [connection] => self.prepare_external_saved(connection.clone(), mode, cx),
            [] => self.start_external_temporary(request, cx),
            _ => {
                self.activation.dialog = Some(ExternalDialog::Choose {
                    request,
                    connections: matches,
                })
            }
        }
        true
    }

    fn prepare_external_saved(
        &mut self,
        connection: SavedConnection,
        mode: Option<SshRuntimeMode>,
        cx: &mut Context<Self>,
    ) {
        if mode != Some(SshRuntimeMode::Sftp)
            && connection
                .post_login
                .as_ref()
                .is_some_and(|command| command.enabled && !command.command.trim().is_empty())
        {
            self.activation.dialog = Some(ExternalDialog::Confirm {
                connection: Box::new(connection),
                mode,
            });
        } else {
            self.start_external_saved(connection, mode, cx);
        }
    }

    fn start_external_saved(
        &mut self,
        connection: SavedConnection,
        mode: Option<SshRuntimeMode>,
        cx: &mut Context<Self>,
    ) {
        self.activation.waiting_for_start = true;
        self.continue_saved_connection_start(
            connection,
            SavedConnectionStartOptions {
                ssh_runtime_mode: mode,
                ..Default::default()
            },
            cx,
        );
    }

    fn start_external_temporary(
        &mut self,
        request: ExternalConnectionRequest,
        cx: &mut Context<Self>,
    ) {
        self.activation.waiting_for_start = true;
        match request {
            ExternalConnectionRequest::Local { cwd } => {
                let mut config = LocalSessionConfig {
                    working_dir: cwd.map(Into::into),
                    ..Default::default()
                };
                self.apply_desired_geometry_to_local_config(&mut config);
                self.begin_background_session_start(
                    config.name.clone(),
                    SessionLaunchConfig::Local(config),
                    None,
                    AiExecutionProfile::Posix,
                    Default::default(),
                    cx,
                );
            }
            ExternalConnectionRequest::Ssh {
                host,
                port,
                username,
                password,
                runtime_mode,
            } => {
                let username = username.unwrap_or_else(|| "root".to_string());
                self.start_external_ssh_link(
                    TemporarySshLinkConfig {
                        name: format!("{username}@{host}:{port}"),
                        host,
                        port,
                        username,
                    },
                    password,
                    runtime_mode.unwrap_or_default(),
                    cx,
                );
            }
            ExternalConnectionRequest::Telnet { host, port } => self.start_external_telnet_link(
                TemporaryTelnetLinkConfig {
                    name: format!("telnet://{host}:{port}"),
                    host,
                    port,
                },
                cx,
            ),
        }
    }

    pub(crate) fn cancel_external_activations(&mut self) {
        self.activation.clear();
    }
}

fn external_request_matches_connection(
    request: &ExternalConnectionRequest,
    connection: &ConnectionType,
) -> bool {
    let same_host = |left: &str, right: &str| {
        normalize_external_host(left)
            .ok()
            .zip(normalize_external_host(right.trim()).ok())
            .is_some_and(|(left, right)| left == right)
    };
    match (request, connection) {
        (
            ExternalConnectionRequest::Ssh {
                host,
                port,
                username,
                password,
                ..
            },
            ConnectionType::Ssh {
                host: saved_host,
                port: saved_port,
                username: saved_username,
                ..
            },
        ) => {
            password.is_none()
                && same_host(host, saved_host)
                && port == saved_port
                && username
                    .as_ref()
                    .is_none_or(|username| username == saved_username)
        }
        (
            ExternalConnectionRequest::Telnet { host, port },
            ConnectionType::Telnet {
                host: saved_host,
                port: saved_port,
                ..
            },
        ) => same_host(host, saved_host) && port == saved_port,
        _ => false,
    }
}

#[cfg(test)]
mod tests;

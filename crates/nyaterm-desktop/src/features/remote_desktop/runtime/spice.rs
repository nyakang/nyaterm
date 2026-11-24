mod actions;
mod events;
mod input;
mod pointer_capture;
use gpui::Context;
use nyaterm_core::SavedConnection;
use nyaterm_remote_desktop::spice::protocol::SpiceErrorKind;
use nyaterm_remote_desktop::{
    RemoteDesktopError, RemoteDesktopErrorCategory, RemoteDesktopViewState,
};

use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use crate::models::{SessionLaunchConfig, session::SpiceLaunchConfig};

pub(super) fn spice_error(error: SpiceErrorKind) -> RemoteDesktopError {
    use RemoteDesktopErrorCategory as Category;
    let (category, key) = match error {
        SpiceErrorKind::Configuration => (Category::Protocol, "configuration"),
        SpiceErrorKind::RuntimeMissing => (Category::HelperMissing, "runtimeMissing"),
        SpiceErrorKind::Connection => (Category::Transport, "connection"),
        SpiceErrorKind::Authentication => (Category::Authentication, "authentication"),
        SpiceErrorKind::Certificate => (Category::Certificate, "certificate"),
        SpiceErrorKind::TicketExpired => (Category::Authentication, "ticketExpired"),
        SpiceErrorKind::Protocol => (Category::Protocol, "protocol"),
        SpiceErrorKind::ResourceLimit => (Category::ResourceExhausted, "resourceLimit"),
        SpiceErrorKind::AgentUnavailable => (Category::Unsupported, "agentUnavailable"),
        SpiceErrorKind::Clipboard => (Category::Clipboard, "clipboard"),
        SpiceErrorKind::FileTransfer => (Category::Session, "fileTransfer"),
        SpiceErrorKind::Audio => (Category::Session, "audio"),
        SpiceErrorKind::HelperCrash => (Category::HelperCrashed, "helperCrash"),
        SpiceErrorKind::Timeout => (Category::Timeout, "timeout"),
    };
    let key = format!("spice.errors.{key}");
    RemoteDesktopError {
        category,
        message: rust_i18n::t!(&key).to_string(),
    }
}

impl NyaTermApp {
    pub(in crate::features) fn start_temporary_spice(
        &mut self,
        document: nyaterm_core::spice::virt_viewer::VirtViewerFile,
        fullscreen: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let id = nyaterm_core::uuid();
        let config = SpiceLaunchConfig {
            // A provider title can contain a ticket too. Keep it out of runtime
            // tab names used by diagnostics and workspace persistence.
            name: rust_i18n::t!("spice.temporaryTitle").to_string(),
            session: nyaterm_remote_desktop::spice::protocol::SpiceSessionConfig {
                session_id: id.clone(),
                endpoint: document.endpoint,
                username: document.username,
                password: document.password,
                settings: document.settings,
                credential_lifetime:
                    nyaterm_remote_desktop::spice::protocol::SpiceCredentialLifetime::Temporary,
                tcp_relay: None,
                tls_relay: None,
            },
        };
        self.remote_desktop.insert_connecting(id.clone());
        if let Some(session) = self.remote_desktop.sessions.get_mut(&id) {
            session.spice = Some(Default::default());
        }
        self.register_session(
            &id,
            crate::models::SessionRuntimeMetadata {
                confirmed_ssh_runtime_mode: None,
                pending_ssh_startup_command: None,
                ssh_config: None,
                ssh_multiplex_key: None,
                source_connection_id: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Disabled,
                launch_config: SessionLaunchConfig::Spice(config.clone()),
                disconnected: false,
            },
        );
        self.activate_session_id(&id, cx);
        self.prepare_and_launch_spice(&id, config, None, cx);
        self.ensure_remote_desktop_periodic_clock(cx);
        if fullscreen && !window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.focus_user_started_session_surface(
            crate::features::session::SessionStartFocusTarget::RemoteDesktop,
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn retry_spice_runtime(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(metadata) = self.session.metadata(id) else {
            return;
        };
        let SessionLaunchConfig::Spice(current) = &metadata.launch_config else {
            return;
        };
        if current.session.credential_lifetime
            == nyaterm_remote_desktop::spice::protocol::SpiceCredentialLifetime::Temporary
        {
            return;
        }
        let saved = metadata
            .source_connection_id
            .as_ref()
            .and_then(|id| self.connection_state.connection_by_id(id))
            .cloned();
        let result = match saved.as_ref() {
            Some(connection)
                if metadata
                    .launch_config
                    .matches_saved_protocol(&connection.config) =>
            {
                self.saved_remote_desktop_launch_config(connection, None)
            }
            Some(_) => Err(rust_i18n::t!("terminal.savedConnectionProtocolChanged").to_string()),
            None => Err(rust_i18n::t!("terminal.savedConnectionDeleted").to_string()),
        };
        let mut config = match result {
            Ok(SessionLaunchConfig::Spice(config)) => config,
            _ => {
                if let Some(session) = self.remote_desktop.sessions.get_mut(id) {
                    session.reconnect_at = None;
                    super::events::set_remote_view_error(
                        session,
                        spice_error(SpiceErrorKind::Configuration),
                    );
                }
                self.settle_remote_desktop_restore(cx);
                cx.notify();
                return;
            }
        };
        config.session.session_id = id.to_owned();
        let password_id = saved
            .as_ref()
            .and_then(|c| super::lifecycle::remote_desktop_password_id(c.auth.as_ref()));
        if let Some(metadata) = self.session.metadata_mut(id) {
            metadata.launch_config = SessionLaunchConfig::Spice(config);
        }
        self.remote_desktop.renew_start(id);
        if let Some(password_id) = password_id {
            self.request_remote_desktop_restart_password(
                id.to_owned(),
                password_id,
                true,
                false,
                cx,
            );
        } else {
            self.restart_spice_runtime(id, true, cx);
        }
    }

    pub(in crate::features) fn schedule_spice_runtime(
        &mut self,
        config: SpiceLaunchConfig,
        connection: SavedConnection,
        request_id: String,
        attempt: nyaterm_transport::connection_attempt::ConnectionAttempt,
        cx: &mut Context<Self>,
    ) -> String {
        let id = config.session.session_id.clone();
        self.remote_desktop
            .insert_connecting_for_request(id.clone(), request_id, attempt);
        if let Some(session) = self.remote_desktop.sessions.get_mut(&id) {
            session.spice = Some(Default::default());
        }
        self.prepare_and_launch_spice(&id, config, Some(connection), cx);
        self.ensure_remote_desktop_periodic_clock(cx);
        id
    }

    fn prepare_and_launch_spice(
        &mut self,
        id: &str,
        mut config: SpiceLaunchConfig,
        connection: Option<SavedConnection>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.helper_launches.contains_key(id) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get(id) else {
            return;
        };
        let token = session.start_token.clone();
        let mut context = self.ssh_session_config_build_context();
        context.attempt = session.start_attempt.clone();
        context.host_key_prompts = std::sync::Arc::new(
            context
                .host_key_prompts
                .for_attempt(context.attempt.clone()),
        );
        context.credential_prompts = std::sync::Arc::new(
            context
                .credential_prompts
                .for_attempt(context.attempt.clone()),
        );
        self.remote_desktop
            .helper_launches
            .insert(id.to_owned(), token.clone());
        let manager = self.remote_desktop.spice_manager.clone();
        let owned_id = id.to_owned();
        let task = self.blocking_jobs.submit_task("spice-route", move |_| {
            context.attempt.check()?;
            // Reap the old generation before installing a replacement with the
            // same public tab identity. This may block and belongs on this worker.
            manager.close(&owned_id);
            config.session.tcp_relay = None;
            config.session.tls_relay = None;
            let routes = if let Some(connection) = connection {
                context.prepare_spice_routes(&connection, &mut config.session)?
            } else {
                Vec::new()
            };
            context.attempt.check()?;
            Ok::<_, String>((config, routes))
        });
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.remote_desktop.helper_launches.get(&id) != Some(&token) {
                    return;
                }
                this.remote_desktop.helper_launches.remove(&id);
                if !this.remote_desktop.start_matches(&id, &token) {
                    return;
                }
                match result {
                    Ok((config, routes)) => {
                        this.remote_desktop.spice_routes.insert(id.clone(), routes);
                        if let Some(metadata) = this.session.metadata_mut(&id) {
                            metadata.launch_config = SessionLaunchConfig::Spice(config.clone());
                        }
                        this.spawn_remote_desktop_helper(
                            &id,
                            SessionLaunchConfig::Spice(config),
                            cx,
                        );
                    }
                    Err(_) => {
                        this.remote_desktop.spice_routes.remove(&id);
                        if let Some(session) = this.remote_desktop.sessions.get_mut(&id) {
                            session.start_attempt.cancel();
                            super::events::set_remote_view_error(
                                session,
                                spice_error(SpiceErrorKind::Connection),
                            );
                        }
                        if let Some(metadata) = this.session.metadata_mut(&id) {
                            metadata.disconnected = true;
                        }
                    }
                }
                this.settle_remote_desktop_restore(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn restart_spice_runtime(
        &mut self,
        id: &str,
        reset_attempts: bool,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.helper_launches.contains_key(id)
            || self.remote_start_blocked_by_configuration_replacement(cx)
        {
            return;
        }
        let Some(metadata) = self.session.metadata(id) else {
            return;
        };
        let SessionLaunchConfig::Spice(config) = &metadata.launch_config else {
            return;
        };
        if config.session.credential_lifetime
            == nyaterm_remote_desktop::spice::protocol::SpiceCredentialLifetime::Temporary
        {
            return;
        }
        let config = config.clone();
        let saved = metadata
            .source_connection_id
            .as_ref()
            .and_then(|id| self.connection_state.connection_by_id(id))
            .cloned();
        self.remote_desktop.renew_start(id);
        if let Some(session) = self.remote_desktop.sessions.get_mut(id) {
            session.state = RemoteDesktopViewState::Connecting;
            session.error = None;
            session.reconnect_at = None;
            session.uploads_stopped = false;
            session.framebuffer = None;
            if let Some(texture) = session.texture.take() {
                self.remote_desktop.pending_texture_removals.push(
                    crate::features::remote_desktop::state::PendingTextureRemoval {
                        owner: session.texture_window,
                        texture,
                    },
                );
            }
            if let Some(texture) = session.cursor_texture.take() {
                self.remote_desktop.pending_texture_removals.push(
                    crate::features::remote_desktop::state::PendingTextureRemoval {
                        owner: session.texture_window,
                        texture,
                    },
                );
            }
            session.cursor_shape = None;
            session.spice = Some(Default::default());
            if reset_attempts {
                session.reconnect_attempts = 0;
            }
        }
        self.prepare_and_launch_spice(id, config, saved, cx);
    }
}

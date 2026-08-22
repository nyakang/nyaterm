use std::sync::Arc;
use std::time::Instant;

use futures::channel::oneshot;
use gpui::{AppContext as _, Context, Window};
use nyaterm_core::{
    AiExecutionProfile, ConnectionAuth, ConnectionType, SavedConnection, SftpCwdFollowMode,
    SftpSettings, SshAgentForwardingConfig as CoreSshAgentForwardingConfig, SshAlgorithmMode,
    SshAlgorithmPreferences, SshProfile, normalize_ssh_agent_endpoint, resolve_ssh_terminal_type,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain, store_request};
use nyaterm_transport::{
    LocalSessionConfig, RdpClipboardConfig, RdpDisplayConfig, RdpReconnectConfig, RdpSessionConfig,
    SerialSessionConfig, SessionKind, SshAgentStoredKey, SshAgentStoredKeyProvider,
    SshAgentStoredKeySnapshot, SshKeyAuthConfig, SshProxyConfig, SshSessionConfig,
    SshSessionProfile, TelnetAutoLoginConfig, TelnetSessionConfig, VncClipboardConfig,
    VncDisplayConfig, VncReconnectConfig, VncSecurityConfig, VncSessionConfig,
    parse_rdp_certificate_policy, parse_rdp_clipboard_mode, parse_rdp_display_mode,
    parse_vnc_scale_mode, parse_vnc_security_mode,
};

use super::super::NativeHostKeyVerifier;
use super::PendingSessionStartRegistration;
use crate::features::formatting::{non_empty_string, parse_telnet_enter_mode, split_shell_args};
use crate::features::{
    NyaTermApp, runtime_jobs::SessionStartResult, runtime_jobs::SessionStartSuccess,
    session::AgentPromptBroker, session::CredentialPromptBroker, session::HostKeyPromptBroker,
    session::NativeOtpProvider, session::SavedConnectionStartOptions,
};
use crate::models::SessionLaunchConfig;

#[derive(Clone)]
pub(in crate::features) struct SshSessionConfigBuildContext {
    pub(in crate::features) store: StoreBlockingClient,
    pub(in crate::features) host_key_policy: String,
    pub(in crate::features) x11_display: String,
    pub(in crate::features) default_encoding: String,
    pub(in crate::features) keep_alive_interval_secs: u32,
    pub(in crate::features) terminal_shell_integration: bool,
    pub(in crate::features) host_key_prompts: Arc<HostKeyPromptBroker>,
    pub(in crate::features) credential_prompts: Arc<CredentialPromptBroker>,
    pub(in crate::features) agent_prompts: Arc<AgentPromptBroker>,
    pub(in crate::features) otp_provider: Arc<NativeOtpProvider>,
    pub(in crate::features) shell_environment: Arc<nyaterm_transport::ShellEnvironmentCache>,
}

/// Loads stored SSH keys only when the transport broker needs identities.
#[derive(Clone)]
struct StoreSshAgentKeyProvider {
    store: StoreBlockingClient,
}

impl SshAgentStoredKeyProvider for StoreSshAgentKeyProvider {
    fn revision(&self) -> Result<u64, String> {
        self.store
            .request_fn(StoreDomain::Security, |store| Ok(store.ssh_key_revision()))
            .map_err(|error| error.to_string())
    }

    fn load_snapshot(&self) -> Result<SshAgentStoredKeySnapshot, String> {
        let revision = self.revision()?;
        let keys = self
            .store
            .request_fn(StoreDomain::Security, |store| store.list_ssh_keys())
            .map_err(|error| error.to_string())?;
        let mut result = Vec::new();
        for key in keys {
            let key_id = key.id.clone();
            let Some(decrypted) = self
                .store
                .request_fn(StoreDomain::Security, move |store| {
                    store.load_decrypted_ssh_key_by_id(&key_id)
                })
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            let Some(key_data) = decrypted.key_data.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            result.push(SshAgentStoredKey {
                key_data,
                cert_data: decrypted.cert_data.filter(|value| !value.trim().is_empty()),
                passphrase: decrypted
                    .passphrase
                    .filter(|value| !value.trim().is_empty()),
                comment: decrypted.name,
            });
        }
        if self.revision()? != revision {
            return Err("stored SSH keys changed while loading".to_string());
        }
        Ok(SshAgentStoredKeySnapshot {
            revision,
            keys: result,
        })
    }
}

impl NyaTermApp {
    pub(in crate::features) fn start_local_session(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut config = LocalSessionConfig::default();
        self.apply_desired_geometry_to_local_config(&mut config);
        let name = config.name.clone();
        self.begin_background_session_start(
            name,
            SessionLaunchConfig::Local(config),
            None,
            AiExecutionProfile::Posix,
            SavedConnectionStartOptions::default(),
            cx,
        );
    }

    pub(in crate::features) fn start_saved_connection(
        &mut self,
        connection: SavedConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_saved_connection_with_options(
            connection,
            SavedConnectionStartOptions::default(),
            window,
            cx,
        );
    }

    pub(in crate::features) fn start_saved_connection_with_options(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.continue_saved_connection_start(connection, options, cx);
    }

    pub(in crate::features) fn continue_saved_connection_start(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let options = self.prepare_session_start_options(options);
        let Some(placement) = options.tab_placement else {
            self.shell.set_status(format!(
                "{} could not reserve a tab position",
                connection.name
            ));
            self.shell.show_workspace();
            cx.notify();
            return;
        };
        if !self
            .session
            .start_reserve_saved_connection(&connection.id, placement)
        {
            self.shell
                .set_status(format!("{} is already connecting", connection.name));
            self.shell.show_workspace();
            cx.notify();
            return;
        }

        if let Some(password_id) = saved_connection_password_id(&connection) {
            let connection_id = connection.id.clone();
            let connection_name = connection.name.clone();
            self.shell
                .set_status(format!("loading saved credentials for {connection_name}"));
            self.submit_store_request(
                0,
                store_request(StoreDomain::Security, move |store| {
                    store.load_decrypted_password_by_id(&password_id)
                }),
                move |this, event, cx| {
                    let password = match event.outcome {
                        Ok(Some(entry)) => entry
                            .password
                            .filter(|password| !password.trim().is_empty()),
                        Ok(None) => {
                            this.session
                                .start_release_saved_connection(&connection_id);
                            this.shell.set_status(format!(
                                "failed to start {connection_name}: saved password was not found"
                            ));
                            cx.notify();
                            return;
                        }
                        Err(error) => {
                            this.session
                                .start_release_saved_connection(&connection_id);
                            this.shell.set_status(format!(
                                "failed to start {connection_name}: could not load saved password: {error}"
                            ));
                            cx.notify();
                            return;
                        }
                    };
                    let Some(password) = password else {
                        this.session
                            .start_release_saved_connection(&connection_id);
                        this.shell.set_status(format!(
                            "failed to start {connection_name}: saved password is empty or locked"
                        ));
                        cx.notify();
                        return;
                    };
                    let mut connection = connection;
                    if let Some(auth) = connection.auth.as_mut() {
                        auth.password = Some(password);
                        auth.has_password = false;
                    }
                    this.start_saved_connection_ready(connection, options, cx);
                },
                cx,
            );
            cx.notify();
            return;
        }

        self.start_saved_connection_ready(connection, options, cx);
    }

    fn start_saved_connection_ready(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let connection_id = connection.id.clone();
        let workspace_split = options.workspace_split.clone();
        let tab_placement = options.tab_placement;
        let fallback_insert_index = options.insert_index.filter(|_| tab_placement.is_none());
        match connection.config.clone() {
            ConnectionType::LocalTerminal {
                shell_path,
                shell_args,
                working_dir,
                ai_execution_profile,
                encoding,
            } => {
                let encoding = resolve_effective_connection_encoding(&encoding, self);
                let mut config = LocalSessionConfig {
                    name: connection.name.clone(),
                    shell_path: non_empty_string(shell_path),
                    shell_args: split_shell_args(&shell_args),
                    working_dir: working_dir
                        .filter(|value| !value.trim().is_empty())
                        .map(Into::into),
                    encoding,
                    cols: 80,
                    rows: 24,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                self.apply_desired_geometry_to_local_config(&mut config);
                self.begin_background_session_start(
                    connection.name,
                    SessionLaunchConfig::Local(config),
                    Some(connection.id),
                    ai_execution_profile,
                    options,
                    cx,
                );
            }
            ConnectionType::Telnet {
                host,
                port,
                ai_execution_profile,
                raw_tcp_cli,
                enter_mode,
                force_character_at_a_time,
                send_naws,
                send_sga,
                username,
                backspace_mode,
                auto_login,
                encoding,
                local_echo,
                local_line_edit,
                ..
            } => {
                let password = inline_connection_password(connection.auth.as_ref());
                let encoding = resolve_effective_connection_encoding(&encoding, self);
                let config = TelnetSessionConfig {
                    name: connection.name.clone(),
                    host,
                    port,
                    username,
                    password,
                    backspace_mode,
                    raw_tcp: raw_tcp_cli,
                    enter_mode: parse_telnet_enter_mode(&enter_mode),
                    local_echo,
                    local_line_edit,
                    force_character_at_a_time,
                    send_naws,
                    send_sga,
                    auto_login: map_telnet_auto_login_config(&auto_login),
                    encoding,
                    cols: 80,
                    rows: 24,
                };
                self.begin_background_session_start(
                    connection.name,
                    SessionLaunchConfig::Telnet(config),
                    Some(connection.id),
                    ai_execution_profile,
                    options,
                    cx,
                );
            }
            ConnectionType::Ssh {
                ai_execution_profile,
                ..
            } => {
                self.begin_background_saved_ssh_start(
                    connection,
                    ai_execution_profile,
                    options,
                    cx,
                );
            }
            ConnectionType::Serial {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                ai_execution_profile,
                backspace_mode,
                encoding,
            } => {
                let encoding = resolve_effective_connection_encoding(&encoding, self);
                let config = SerialSessionConfig {
                    name: connection.name.clone(),
                    port_name,
                    baud_rate,
                    data_bits,
                    parity,
                    stop_bits,
                    backspace_mode,
                    encoding,
                };
                self.begin_background_session_start(
                    connection.name,
                    SessionLaunchConfig::Serial(config),
                    Some(connection.id),
                    ai_execution_profile,
                    options,
                    cx,
                );
            }
            ConnectionType::Rdp {
                host,
                port,
                username,
                domain,
                security,
                display,
                clipboard,
                reconnect,
            } => {
                let config = RdpSessionConfig {
                    name: connection.name.clone(),
                    host,
                    port,
                    username,
                    domain,
                    password: inline_connection_password(connection.auth.as_ref()),
                    use_nla: security.use_nla,
                    certificate_policy: parse_rdp_certificate_policy(&security.certificate_policy),
                    display: RdpDisplayConfig {
                        mode: parse_rdp_display_mode(&display.mode),
                        width: display.width,
                        height: display.height,
                        color_depth: display.color_depth,
                    },
                    clipboard: RdpClipboardConfig {
                        mode: parse_rdp_clipboard_mode(&clipboard.mode),
                    },
                    reconnect: RdpReconnectConfig {
                        enabled: reconnect.enabled,
                        max_attempts: reconnect.max_attempts,
                    },
                };
                match self.create_rdp_runtime(config.clone()) {
                    Ok(session_id) => {
                        let source_connection_id = Some(connection.id.clone());
                        self.register_session_for_start(
                            &session_id,
                            crate::models::SessionRuntimeMetadata {
                                ssh_config: None,
                                ssh_multiplex_key: None,
                                source_connection_id: source_connection_id.clone(),
                                ai_execution_profile: AiExecutionProfile::Disabled,
                                launch_config: SessionLaunchConfig::Rdp(config),
                                disconnected: false,
                            },
                            tab_placement,
                            fallback_insert_index,
                        );
                        if let Some(custom_name) = options.custom_name {
                            self.session
                                .set_custom_name(session_id.clone(), custom_name);
                        }
                        if let Some(color) = options.tab_color {
                            self.session.set_tab_color(&session_id, Some(color));
                        }
                        if options.locked {
                            self.session.set_tab_locked(&session_id, true);
                        }
                        if tab_placement.is_none()
                            && fallback_insert_index.is_none()
                            && let Some(after_session_id) = options.after_session_id
                        {
                            self.session
                                .move_session_after(&session_id, &after_session_id);
                        }
                        self.persist_connection_used(connection.id.clone(), cx);
                        self.activate_session_id(&session_id);
                        self.apply_workspace_split_for_duplicate(
                            workspace_split.clone(),
                            &session_id,
                        );
                        self.shell
                            .set_status(format!("connecting RDP {}", connection.name));
                    }
                    Err(error) => {
                        let message = format!("RDP connection failed: {error}");
                        let session_id = self.create_failed_rdp_runtime(error);
                        self.register_session_for_start(
                            &session_id,
                            crate::models::SessionRuntimeMetadata {
                                ssh_config: None,
                                ssh_multiplex_key: None,
                                source_connection_id: Some(connection.id.clone()),
                                ai_execution_profile: AiExecutionProfile::Disabled,
                                launch_config: SessionLaunchConfig::Rdp(config),
                                disconnected: false,
                            },
                            tab_placement,
                            fallback_insert_index,
                        );
                        if let Some(custom_name) = options.custom_name {
                            self.session
                                .set_custom_name(session_id.clone(), custom_name);
                        }
                        if let Some(color) = options.tab_color {
                            self.session.set_tab_color(&session_id, Some(color));
                        }
                        if options.locked {
                            self.session.set_tab_locked(&session_id, true);
                        }
                        self.activate_session_id(&session_id);
                        self.shell.set_status(message);
                    }
                }
                self.shell.show_workspace();
                cx.notify();
            }
            ConnectionType::Vnc {
                host,
                port,
                security,
                display,
                clipboard,
                reconnect,
                shared,
                view_only,
            } => {
                let config = VncSessionConfig {
                    name: connection.name.clone(),
                    host,
                    port,
                    password: inline_connection_password(connection.auth.as_ref()),
                    security: VncSecurityConfig {
                        mode: parse_vnc_security_mode(&security.mode),
                    },
                    display: VncDisplayConfig {
                        scale_mode: parse_vnc_scale_mode(&display.scale_mode),
                    },
                    clipboard: VncClipboardConfig {
                        enabled: clipboard.enabled,
                    },
                    reconnect: VncReconnectConfig {
                        enabled: reconnect.enabled,
                        max_attempts: reconnect.max_attempts,
                    },
                    shared,
                    view_only,
                };
                match self.create_vnc_runtime(config.clone()) {
                    Ok(session_id) => {
                        self.register_session_for_start(
                            &session_id,
                            crate::models::SessionRuntimeMetadata {
                                ssh_config: None,
                                ssh_multiplex_key: None,
                                source_connection_id: Some(connection.id.clone()),
                                ai_execution_profile: AiExecutionProfile::Disabled,
                                launch_config: SessionLaunchConfig::Vnc(config),
                                disconnected: false,
                            },
                            tab_placement,
                            fallback_insert_index,
                        );
                        if let Some(custom_name) = options.custom_name {
                            self.session
                                .set_custom_name(session_id.clone(), custom_name);
                        }
                        if let Some(color) = options.tab_color {
                            self.session.set_tab_color(&session_id, Some(color));
                        }
                        if options.locked {
                            self.session.set_tab_locked(&session_id, true);
                        }
                        if tab_placement.is_none()
                            && fallback_insert_index.is_none()
                            && let Some(after_session_id) = options.after_session_id
                        {
                            self.session
                                .move_session_after(&session_id, &after_session_id);
                        }
                        self.persist_connection_used(connection.id.clone(), cx);
                        self.activate_session_id(&session_id);
                        self.apply_workspace_split_for_duplicate(
                            workspace_split.clone(),
                            &session_id,
                        );
                        self.shell
                            .set_status(format!("connecting VNC {}", connection.name));
                    }
                    Err(error) => {
                        let message = format!("VNC connection failed: {error}");
                        let session_id = self.create_failed_vnc_runtime(error);
                        self.register_session_for_start(
                            &session_id,
                            crate::models::SessionRuntimeMetadata {
                                ssh_config: None,
                                ssh_multiplex_key: None,
                                source_connection_id: Some(connection.id.clone()),
                                ai_execution_profile: AiExecutionProfile::Disabled,
                                launch_config: SessionLaunchConfig::Vnc(config),
                                disconnected: false,
                            },
                            tab_placement,
                            fallback_insert_index,
                        );
                        if let Some(custom_name) = options.custom_name {
                            self.session
                                .set_custom_name(session_id.clone(), custom_name);
                        }
                        if let Some(color) = options.tab_color {
                            self.session.set_tab_color(&session_id, Some(color));
                        }
                        if options.locked {
                            self.session.set_tab_locked(&session_id, true);
                        }
                        self.activate_session_id(&session_id);
                        self.shell.set_status(message);
                    }
                }
                self.shell.show_workspace();
                cx.notify();
            }
        }
        self.session.start_release_saved_connection(&connection_id);
    }

    pub(in crate::features) fn begin_background_saved_ssh_start(
        &mut self,
        connection: SavedConnection,
        ai_execution_profile: AiExecutionProfile,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let SavedConnectionStartOptions {
            custom_name,
            tab_color,
            locked,
            after_session_id,
            insert_index,
            seed_output,
            startup_command,
            reconnect_session_id,
            workspace_split,
            tab_placement,
        } = options;
        let connection_name = connection.name.clone();
        let source_connection_id = Some(connection.id.clone());
        let geometry_session_hint = after_session_id
            .as_deref()
            .or(reconnect_session_id.as_deref());
        let desired_geometry =
            self.desired_terminal_resize_geometry_for_session_hint(geometry_session_hint);
        let build_context = self.ssh_session_config_build_context();
        let multiplex_key = match &connection.config {
            ConnectionType::Ssh {
                host,
                port,
                username,
                ..
            } => Some(format!(
                "{}@{}:{}",
                username.trim(),
                host.trim().to_ascii_lowercase(),
                port
            )),
            _ => None,
        };
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                connection_name: connection_name.clone(),
                launch_config: None,
                kind: SessionKind::Ssh,
                ai_execution_profile,
                custom_name,
                tab_color,
                locked,
                after_session_id,
                insert_index,
                seed_output,
                startup_command,
                multiplex_key,
                source_connection_id,
                reconnect_session_id,
                workspace_split,
                tab_placement,
                status_message: format!("connecting to {connection_name}"),
                append_start_log: true,
            },
            cx,
        );

        let session_manager = self.session.manager_handle();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        std::thread::spawn(move || {
            let worker_started_at = Instant::now();
            let result = (|| {
                let mut config = build_ssh_session_config_with_context(
                    &connection,
                    &mut Vec::new(),
                    &build_context,
                )?;
                // Open the PTY as soon as the registration barrier is released. The terminal
                // surface sends its actual viewport resize after layout; waiting for that command
                // here adds a fixed startup delay before the first remote bytes can be read.
                config.deferred_pty = false;
                if let Some(geometry) = desired_geometry {
                    config.cols = geometry.cols;
                    config.rows = geometry.rows;
                    config.pixel_width = geometry.pixel_width;
                    config.pixel_height = geometry.pixel_height;
                }
                let (session_info, multiplex, ssh_start_handle) = session_manager
                    .create_ssh_session_with_shared_handle_deferred(config.clone())
                    .map_err(|error| error.to_string())?;
                Ok(SessionStartSuccess {
                    session_info,
                    multiplex_handle: Some(multiplex),
                    ssh_start_handle: Some(ssh_start_handle),
                    launch_config: Some(SessionLaunchConfig::Ssh(Box::new(config))),
                })
            })();
            let worker_finished_at = Instant::now();
            let _ = session_start_tx.unbounded_send(SessionStartResult {
                request_id: request_id_for_worker,
                connection_name,
                kind: SessionKind::Ssh,
                worker_started_at,
                worker_finished_at,
                result,
            });
        });
    }

    pub(in crate::features) fn ssh_session_config_build_context(
        &self,
    ) -> SshSessionConfigBuildContext {
        let keep_alive_interval_secs =
            if self.settings.summary().terminal_keep_alive_mode == "disabled" {
                0
            } else {
                self.settings.summary().terminal_keep_alive_interval
            };
        SshSessionConfigBuildContext {
            store: self.store_blocking_client(),
            host_key_policy: self.settings.summary().host_key_policy.clone(),
            x11_display: self.settings.summary().x11_display.clone(),
            default_encoding: self.settings.summary().interaction_default_encoding.clone(),
            keep_alive_interval_secs,
            terminal_shell_integration: self.settings.summary().terminal_zebra_stripes_enabled,
            host_key_prompts: self.session.prompts.host_key_broker(),
            credential_prompts: self.session.prompts.credential_broker(),
            agent_prompts: self.session.prompts.agent_broker(),
            otp_provider: self.session.prompts.otp_provider(),
            shell_environment: self.session.manager_handle().shell_environment(),
        }
    }

    pub(in crate::features) fn persist_connection_used(
        &mut self,
        connection_id: String,
        cx: &mut Context<Self>,
    ) {
        let request_id = connection_id.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                store.mark_connection_used(&request_id)?;
                store.get_connection(&request_id)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(Some(updated)) => {
                        this.connection_state.update_connection(updated);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let message = format!(
                            "failed to record recently used connection '{connection_id}': {error}"
                        );
                        this.shell.set_status(message.clone());
                        this.settings.update_store_status(message, false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn build_ssh_session_config(
        &self,
        connection: &SavedConnection,
        visited_proxy_jumps: &mut Vec<String>,
    ) -> Result<SshSessionConfig, String> {
        build_ssh_session_config_with_context(
            connection,
            visited_proxy_jumps,
            &self.ssh_session_config_build_context(),
        )
    }

    pub(in crate::features) fn refresh_connection_editor_agent_preview(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some((generation, editor)) = self.connection_state.begin_editor_agent_preview() else {
            return;
        };
        let runtime_config = map_agent_forwarding_config(editor.agent_forwarding_config);
        let provider: Arc<dyn SshAgentStoredKeyProvider> = Arc::new(StoreSshAgentKeyProvider {
            store: self.store_blocking_client(),
        });
        let shell_environment = self.ssh_session_config_build_context().shell_environment;
        cx.notify();
        let task = cx.background_spawn(async move {
            let (sender, receiver) = oneshot::channel();
            let worker = std::thread::Builder::new()
                .name("nyaterm-ssh-agent-preview".to_string())
                .spawn(move || {
                    let preview = nyaterm_transport::preview_identities_blocking_with_environment(
                        &runtime_config,
                        Some(provider),
                        shell_environment,
                    );
                    let _ = sender.send(preview);
                });
            if let Err(error) = worker {
                tracing::error!(%error, "failed to start SSH Agent preview worker");
                return nyaterm_transport::SshAgentIdentityPreviewResponse::default();
            }
            receiver.await.unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let preview = task.await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .connection_state
                    .set_editor_agent_preview(generation, preview)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn apply_desired_geometry_to_local_config(
        &self,
        config: &mut LocalSessionConfig,
    ) {
        if let Some(geometry) = self.desired_terminal_resize_geometry() {
            config.cols = geometry.cols;
            config.rows = geometry.rows;
            config.pixel_width = geometry.pixel_width;
            config.pixel_height = geometry.pixel_height;
        }
    }
}

pub(in crate::features) fn build_ssh_session_config_with_context(
    connection: &SavedConnection,
    visited_proxy_jumps: &mut Vec<String>,
    context: &SshSessionConfigBuildContext,
) -> Result<SshSessionConfig, String> {
    let ConnectionType::Ssh {
        host,
        port,
        username,
        backspace_mode,
        ai_execution_profile: _,
        x11_forwarding,
        auth_agent_endpoint,
        agent_forwarding_config,
        encoding,
        ..
    } = connection.config.clone()
    else {
        return Err("only SSH connections can be used for SSH sessions".to_string());
    };
    let auth = connection.auth.clone().unwrap_or_default();
    let allow_none_auth = auth.mode == "none";
    let password = load_ssh_connection_password_with_context(context, &auth)?;
    let key_auth = load_ssh_key_auth_with_context(context, auth.key_id.as_deref(), &auth.mode)?;
    let proxy_jump = load_proxy_jump_config_with_context(context, connection, visited_proxy_jumps)?;
    let proxy = load_proxy_config_with_context(context, connection)?;
    let encoding = if encoding.trim().is_empty() {
        context.default_encoding.clone()
    } else {
        encoding
    };

    Ok(SshSessionConfig {
        name: connection.name.clone(),
        host,
        port,
        username,
        password,
        key_auth,
        agent_auth: auth.mode == "agent",
        agent_endpoint: match normalize_ssh_agent_endpoint(auth_agent_endpoint.unwrap_or_default())
        {
            nyaterm_core::SshAgentEndpoint::Auto => nyaterm_transport::SshAgentEndpoint::Auto,
            nyaterm_core::SshAgentEndpoint::Environment { variable } => {
                nyaterm_transport::SshAgentEndpoint::Environment { variable }
            }
            nyaterm_core::SshAgentEndpoint::UnixSocket { path } => {
                nyaterm_transport::SshAgentEndpoint::UnixSocket { path }
            }
            nyaterm_core::SshAgentEndpoint::Pageant => nyaterm_transport::SshAgentEndpoint::Pageant,
            nyaterm_core::SshAgentEndpoint::WindowsOpenSsh => {
                nyaterm_transport::SshAgentEndpoint::WindowsOpenSsh
            }
        },
        agent_forwarding: agent_forwarding_config
            .as_ref()
            .is_some_and(|config| config.enabled),
        agent_forwarding_config: agent_forwarding_config.map(map_agent_forwarding_config),
        otp_id: auth.otp_id.filter(|value| !value.trim().is_empty()),
        auto_fill_otp: auth.auto_fill_otp,
        proxy_jump,
        proxy,
        allow_none_auth,
        backspace_mode,
        profile: match connection.ssh_profile {
            SshProfile::Standard => SshSessionProfile::Standard,
            SshProfile::NetworkDevice => SshSessionProfile::NetworkDevice,
        },
        term: resolve_ssh_terminal_type(connection.ssh_profile, connection.terminal_type)
            .as_str()
            .to_string(),
        x11_forwarding,
        x11_display: context.x11_display.clone(),
        encoding,
        ssh_algorithms: connection
            .ssh_algorithms
            .as_ref()
            .map(map_ssh_algorithm_preferences),
        sftp: map_sftp_settings(&connection.sftp),
        terminal_shell_integration: context.terminal_shell_integration,
        deferred_pty: false,
        keep_alive_interval_secs: context.keep_alive_interval_secs,
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        host_key_verifier: Some(Arc::new(NativeHostKeyVerifier {
            store: context.store.clone(),
            policy: context.host_key_policy.clone(),
            prompt_broker: context.host_key_prompts.clone(),
        })),
        credential_provider: Some(context.credential_prompts.clone()),
        agent_prompt_provider: Some(context.agent_prompts.clone()),
        agent_stored_key_provider: Some(Arc::new(StoreSshAgentKeyProvider {
            store: context.store.clone(),
        })),
        otp_provider: Some(context.otp_provider.clone()),
    })
}

fn load_ssh_connection_password_with_context(
    context: &SshSessionConfigBuildContext,
    auth: &ConnectionAuth,
) -> Result<Option<String>, String> {
    if let Some(password) = auth
        .password
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if auth.has_password {
            return Err("saved SSH password is locked or could not be decrypted".to_string());
        }
        return Ok(Some(password.to_string()));
    }

    let Some(password_id) = auth
        .password_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let password_id = password_id.to_string();
    let password = context
        .store
        .request_fn(StoreDomain::Security, move |store| {
            store.load_decrypted_password_by_id(&password_id)
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "saved password was not found".to_string())?
        .password
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "saved password is empty or locked".to_string())?;
    Ok(Some(password))
}

fn map_agent_forwarding_config(
    config: CoreSshAgentForwardingConfig,
) -> nyaterm_transport::SshAgentForwardingConfig {
    nyaterm_transport::SshAgentForwardingConfig {
        enabled: config.enabled,
        sources: nyaterm_transport::SshAgentForwardingSources {
            external_agent: config.sources.external_agent,
            external_agent_endpoints: config
                .sources
                .external_agent_endpoints
                .into_iter()
                .map(map_runtime_agent_endpoint)
                .collect(),
            stored_keys: config.sources.stored_keys,
        },
        policy: match config.policy {
            nyaterm_core::SshAgentForwardingPolicy::All => {
                nyaterm_transport::SshAgentForwardingPolicy::All
            }
            nyaterm_core::SshAgentForwardingPolicy::Allowlist { fingerprints } => {
                nyaterm_transport::SshAgentForwardingPolicy::Allowlist { fingerprints }
            }
        },
    }
}

fn map_runtime_agent_endpoint(
    endpoint: nyaterm_core::SshAgentEndpoint,
) -> nyaterm_transport::SshAgentEndpoint {
    match endpoint {
        nyaterm_core::SshAgentEndpoint::Auto => nyaterm_transport::SshAgentEndpoint::Auto,
        nyaterm_core::SshAgentEndpoint::Environment { variable } => {
            nyaterm_transport::SshAgentEndpoint::Environment { variable }
        }
        nyaterm_core::SshAgentEndpoint::UnixSocket { path } => {
            nyaterm_transport::SshAgentEndpoint::UnixSocket { path }
        }
        nyaterm_core::SshAgentEndpoint::Pageant => nyaterm_transport::SshAgentEndpoint::Pageant,
        nyaterm_core::SshAgentEndpoint::WindowsOpenSsh => {
            nyaterm_transport::SshAgentEndpoint::WindowsOpenSsh
        }
    }
}

fn inline_connection_password(auth: Option<&ConnectionAuth>) -> Option<String> {
    let auth = auth?;
    if auth.mode == "none" {
        return None;
    }
    if let Some(password) = auth
        .password
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return (!auth.has_password).then(|| password.to_string());
    }
    None
}

fn saved_connection_password_id(connection: &SavedConnection) -> Option<String> {
    if !matches!(
        &connection.config,
        ConnectionType::Telnet { .. } | ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. }
    ) {
        return None;
    }
    stored_connection_password_id(connection.auth.as_ref())
}

fn stored_connection_password_id(auth: Option<&ConnectionAuth>) -> Option<String> {
    let auth = auth?;
    if auth.mode == "none"
        || auth
            .password
            .as_deref()
            .is_some_and(|password| !password.trim().is_empty() && !auth.has_password)
    {
        return None;
    }
    auth.password_id
        .as_deref()
        .map(str::trim)
        .filter(|password_id| !password_id.is_empty())
        .map(ToString::to_string)
}

fn resolve_effective_connection_encoding(value: &str, app: &NyaTermApp) -> String {
    if value.trim().is_empty() {
        app.settings.summary().interaction_default_encoding.clone()
    } else {
        value.trim().to_string()
    }
}

fn map_ssh_algorithm_preferences(
    preferences: &SshAlgorithmPreferences,
) -> nyaterm_transport::SshAlgorithmPreferences {
    nyaterm_transport::SshAlgorithmPreferences {
        mode: match preferences.mode {
            SshAlgorithmMode::Compatible => nyaterm_transport::SshAlgorithmMode::Compatible,
            SshAlgorithmMode::Secure => nyaterm_transport::SshAlgorithmMode::Secure,
            SshAlgorithmMode::Custom => nyaterm_transport::SshAlgorithmMode::Custom,
        },
        kex: preferences.kex.clone(),
        ciphers: preferences.ciphers.clone(),
        macs: preferences.macs.clone(),
        host_keys: preferences.host_keys.clone(),
    }
}

fn map_sftp_settings(settings: &SftpSettings) -> nyaterm_transport::SftpSettings {
    nyaterm_transport::SftpSettings {
        enabled: settings.enabled,
        cwd_follow_mode: match settings.cwd_follow_mode {
            SftpCwdFollowMode::Off => nyaterm_transport::SftpCwdFollowMode::Off,
            SftpCwdFollowMode::ShellIntegration => {
                nyaterm_transport::SftpCwdFollowMode::ShellIntegration
            }
            SftpCwdFollowMode::RcFile => nyaterm_transport::SftpCwdFollowMode::RcFile,
        },
        shell_detection_timeout_ms: settings.shell_detection_timeout_ms,
        filename_encoding: settings.filename_encoding.clone(),
    }
}

fn map_telnet_auto_login_config(
    config: &nyaterm_core::TelnetAutoLoginConfig,
) -> TelnetAutoLoginConfig {
    TelnetAutoLoginConfig {
        enabled: config.enabled,
        send_wake_enter: config.send_wake_enter,
        timeout_ms: config.timeout_ms,
        username_prompt_regex: config.username_prompt_regex.clone(),
        password_prompt_regex: config.password_prompt_regex.clone(),
        success_prompt_regex: config.success_prompt_regex.clone(),
        failure_prompt_regex: config.failure_prompt_regex.clone(),
        max_retries: config.max_retries,
    }
}

fn load_ssh_key_auth_with_context(
    context: &SshSessionConfigBuildContext,
    key_id: Option<&str>,
    auth_mode: &str,
) -> Result<Option<SshKeyAuthConfig>, String> {
    if auth_mode != "key" {
        return Ok(None);
    }
    let key_id = key_id
        .filter(|key_id| !key_id.trim().is_empty())
        .ok_or_else(|| "connection is set to key auth but has no key_id".to_string())?;
    let key_id = key_id.to_string();
    let key = context
        .store
        .request_fn(StoreDomain::Security, move |store| {
            store.load_decrypted_ssh_key_by_id(&key_id)
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "SSH key was not found".to_string())?;
    let key_data = key
        .key_data
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("SSH key '{}' has no private key data", key.name))?;
    Ok(Some(SshKeyAuthConfig {
        key_data,
        cert_data: key.cert_data.filter(|value| !value.trim().is_empty()),
        passphrase: key.passphrase.filter(|value| !value.trim().is_empty()),
    }))
}

fn load_proxy_config_with_context(
    context: &SshSessionConfigBuildContext,
    connection: &SavedConnection,
) -> Result<Option<SshProxyConfig>, String> {
    let Some(proxy_id) = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_id.as_deref())
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let proxy = context
        .store
        .request_fn(StoreDomain::Tunnels, |store| store.list_proxies())
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|proxy| proxy.id == proxy_id)
        .ok_or_else(|| format!("Proxy '{proxy_id}' was not found"))?;
    let protocol = match proxy.protocol.as_str() {
        "http" | "proxycommand" => proxy.protocol,
        _ => "socks5".to_string(),
    };
    Ok(Some(SshProxyConfig {
        protocol,
        host: proxy.host,
        port: proxy.port,
        command: proxy.command.filter(|value| !value.trim().is_empty()),
        username: proxy.username.filter(|value| !value.trim().is_empty()),
        password: proxy.password.filter(|value| !value.is_empty()),
    }))
}

fn load_proxy_jump_config_with_context(
    context: &SshSessionConfigBuildContext,
    connection: &SavedConnection,
    visited_proxy_jumps: &mut Vec<String>,
) -> Result<Option<Box<SshSessionConfig>>, String> {
    let Some(proxy_jump_id) = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref())
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    if visited_proxy_jumps
        .iter()
        .any(|visited| visited == proxy_jump_id)
    {
        return Err(format!(
            "ProxyJump chain contains a cycle at '{proxy_jump_id}'"
        ));
    }
    visited_proxy_jumps.push(proxy_jump_id.to_string());
    let proxy_jump_id = proxy_jump_id.to_string();
    let jump_connection = context
        .store
        .request_fn(StoreDomain::Connections, move |store| {
            store.get_connection(&proxy_jump_id)
        })
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "ProxyJump connection was not found".to_string())?;
    if !matches!(jump_connection.config, ConnectionType::Ssh { .. }) {
        return Err("Only SSH connections can be used as jump hosts".to_string());
    }
    let jump_config =
        build_ssh_session_config_with_context(&jump_connection, visited_proxy_jumps, context)?;
    visited_proxy_jumps.pop();
    Ok(Some(Box::new(jump_config)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use nyaterm_core::{
        AiExecutionProfile, ConnectionAuth, ConnectionType, SavedConnection, SftpCwdFollowMode,
        SftpSettings, SshAlgorithmMode, SshAlgorithmPreferences, SshProfile, SshTerminalType, uuid,
    };
    use nyaterm_store::{StoreConfig, StoreDomain, StoreRuntime};
    use nyaterm_transport::SshSessionProfile;

    use super::{
        SshSessionConfigBuildContext, build_ssh_session_config_with_context,
        inline_connection_password, load_ssh_connection_password_with_context,
        stored_connection_password_id,
    };
    use crate::features::{
        session::AgentPromptBroker, session::CredentialPromptBroker, session::HostKeyPromptBroker,
        session::NativeOtpProvider,
    };

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nyaterm-desktop-{name}-{}-{}",
            std::process::id(),
            uuid()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn test_ssh_build_context(config_dir: PathBuf) -> SshSessionConfigBuildContext {
        let store = StoreRuntime::spawn(StoreConfig {
            config_dir: config_dir.clone(),
            portable_key_path: None,
        })
        .expect("spawn test store")
        .blocking_client();
        SshSessionConfigBuildContext {
            store: store.clone(),
            host_key_policy: "accept".to_string(),
            x11_display: String::new(),
            default_encoding: "UTF-8".to_string(),
            terminal_shell_integration: true,
            keep_alive_interval_secs: 30,
            host_key_prompts: Arc::new(HostKeyPromptBroker::default()),
            credential_prompts: Arc::new(CredentialPromptBroker::default()),
            agent_prompts: Arc::new(AgentPromptBroker::default()),
            otp_provider: Arc::new(NativeOtpProvider::new(store)),
            shell_environment: nyaterm_transport::ShellEnvironmentCache::new(),
        }
    }

    #[test]
    fn ssh_password_loader_uses_decrypted_inline_password() {
        let dir = unique_temp_dir("ssh-inline-password");
        let context = test_ssh_build_context(dir.clone());
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password: Some("secret".to_string()),
            has_password: false,
            ..ConnectionAuth::default()
        };

        let password = load_ssh_connection_password_with_context(&context, &auth).unwrap();

        assert_eq!(password.as_deref(), Some("secret"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_password_loader_resolves_saved_password_id() {
        let dir = unique_temp_dir("ssh-password-id");
        let context = test_ssh_build_context(dir.clone());
        let password_id = context
            .store
            .request_fn(StoreDomain::Security, |store| {
                store.save_password(nyaterm_core::SavedPassword {
                    id: "pw-1".to_string(),
                    name: "Primary".to_string(),
                    password: Some("stored-secret".to_string()),
                    has_password: false,
                })
            })
            .expect("save password");
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password_id: Some(password_id),
            ..ConnectionAuth::default()
        };

        let password = load_ssh_connection_password_with_context(&context, &auth).unwrap();

        assert_eq!(password.as_deref(), Some("stored-secret"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_ssh_password_selection_never_treats_locked_values_as_plaintext() {
        let inline = ConnectionAuth {
            mode: "password".to_string(),
            password: Some("plain-secret".to_string()),
            password_id: Some("pw-inline".to_string()),
            has_password: false,
            ..ConnectionAuth::default()
        };
        assert_eq!(
            inline_connection_password(Some(&inline)).as_deref(),
            Some("plain-secret")
        );
        assert_eq!(stored_connection_password_id(Some(&inline)), None);

        let locked = ConnectionAuth {
            mode: "password".to_string(),
            password: Some("masked-or-encrypted".to_string()),
            password_id: Some("pw-stored".to_string()),
            has_password: true,
            ..ConnectionAuth::default()
        };
        assert_eq!(inline_connection_password(Some(&locked)), None);
        assert_eq!(
            stored_connection_password_id(Some(&locked)).as_deref(),
            Some("pw-stored")
        );
    }

    #[test]
    fn ssh_password_loader_rejects_locked_inline_password() {
        let dir = unique_temp_dir("ssh-locked-password");
        let context = test_ssh_build_context(dir.clone());
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password: Some("encrypted".to_string()),
            has_password: true,
            ..ConnectionAuth::default()
        };

        let error = load_ssh_connection_password_with_context(&context, &auth).unwrap_err();

        assert!(error.contains("locked"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_session_config_uses_context_keep_alive_interval() {
        let dir = unique_temp_dir("ssh-keepalive");
        let mut context = test_ssh_build_context(dir.clone());
        context.keep_alive_interval_secs = 45;
        let connection = SavedConnection {
            id: "conn-1".to_string(),
            name: "SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "user".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Posix,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "none".to_string(),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let config =
            build_ssh_session_config_with_context(&connection, &mut Vec::new(), &context).unwrap();

        assert_eq!(config.keep_alive_interval_secs, 45);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ssh_session_config_maps_encoding_algorithms_and_sftp_settings() {
        let dir = unique_temp_dir("ssh-mapping");
        let mut context = test_ssh_build_context(dir.clone());
        context.default_encoding = "GB18030".to_string();
        let mut connection = SavedConnection {
            id: "conn-1".to_string(),
            name: "SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "user".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Posix,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "none".to_string(),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: Some(SshAlgorithmPreferences {
                mode: SshAlgorithmMode::Secure,
                ..Default::default()
            }),
            ssh_profile: SshProfile::NetworkDevice,
            terminal_type: None,
            sftp: SftpSettings {
                enabled: true,
                cwd_follow_mode: SftpCwdFollowMode::RcFile,
                shell_detection_timeout_ms: 5000,
                filename_encoding: "GBK".to_string(),
            },
            network: None,
            post_login: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let config =
            build_ssh_session_config_with_context(&connection, &mut Vec::new(), &context).unwrap();

        assert_eq!(config.encoding, "GB18030");
        assert_eq!(
            config
                .ssh_algorithms
                .as_ref()
                .map(|preferences| preferences.mode),
            Some(nyaterm_transport::SshAlgorithmMode::Secure)
        );
        assert!(config.sftp.enabled);
        assert_eq!(
            config.sftp.cwd_follow_mode,
            nyaterm_transport::SftpCwdFollowMode::RcFile
        );
        assert_eq!(config.sftp.shell_detection_timeout_ms, 5000);
        assert_eq!(config.sftp.filename_encoding, "GBK");
        assert_eq!(config.profile, SshSessionProfile::NetworkDevice);
        assert_eq!(config.term, "vt100");
        assert!(!config.remote_file_browser_enabled());
        assert!(!config.remote_stats_enabled());

        connection.terminal_type = Some(SshTerminalType::Ansi);
        let explicit =
            build_ssh_session_config_with_context(&connection, &mut Vec::new(), &context).unwrap();
        assert_eq!(explicit.term, "ansi");
        let _ = std::fs::remove_dir_all(dir);
    }
}

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Window};
use nyaterm_core::models::credentials::AccountAuthError;
use nyaterm_core::{ConnectionType, SavedConnection, SecretString};
use nyaterm_files::ftp::{
    FtpConfig, FtpFileService, FtpFilenameEncodingMode, FtpLogin, FtpPassiveMode, FtpSecurityMode,
    FtpTlsVersion,
};
use nyaterm_store::StoreDomain;
use nyaterm_transport::SerialSessionConfig;
use nyaterm_transport::SshSessionConfig;
use nyaterm_transport::TelnetProbeOutcome;
use nyaterm_transport::TelnetSessionConfig;
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use nyaterm_transport::network_route::NetworkRoute;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::formatting::parse_telnet_enter_mode;
use crate::features::session::{NativeHostKeyVerifier, SshSessionConfigBuildContext};
use crate::models::ConnectionKindTab;

use super::helpers::{build_saved_connection_from_editor, validate_editor_account};

/// One connection test is capped so a black-holed host cannot keep the editor
/// busy forever. The worker observes cancellation through `ConnectionAttempt`.
const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECTION_TEST_TCP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionTestSuccess {
    SshAuthenticated,
    TcpReachable,
    FtpAuthenticated,
    TelnetAuthenticated,
    VncAuthenticated,
    RdpAuthenticated,
    SpiceAuthenticated,
    SerialOpened,
}

/// Whether the editor can probe this connection kind.
///
/// Local shells have nothing to dial, and Android devices are selected through
/// the device tracker rather than a saved host/port.
pub(in crate::features) fn connection_kind_supports_test(kind: ConnectionKindTab) -> bool {
    matches!(
        kind,
        ConnectionKindTab::Ssh
            | ConnectionKindTab::Telnet
            | ConnectionKindTab::Serial
            | ConnectionKindTab::Ftp
            | ConnectionKindTab::Rdp
            | ConnectionKindTab::Vnc
            | ConnectionKindTab::Spice
    )
}

impl NyaTermApp {
    /// Test the current editor draft without saving it.
    ///
    /// The draft is validated with the same rules as Save, then a temporary
    /// connection is opened and closed. Nothing is written to the connection
    /// store and no terminal tab is created.
    pub(in crate::features) fn test_connection_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.connection_state.active_editor_draft() else {
            return;
        };
        if editor.defaults_mode || !connection_kind_supports_test(editor.kind) {
            self.shell
                .set_status(t!("dialog.testConnectionUnavailable").to_string());
            cx.notify();
            return;
        }
        if self.connection_state.editor_test_in_flight() {
            self.shell
                .set_status(t!("dialog.testingConnection").to_string());
            cx.notify();
            return;
        }

        let built = match build_saved_connection_from_editor(&editor).and_then(|built| {
            validate_editor_account(&editor, self.security.passwords()).map(|()| built)
        }) {
            Ok(connection) => connection,
            Err(error) => {
                let message = self.connection_editor_validation_message(&error);
                let location = error.location(&editor);
                self.reveal_connection_editor_validation(location, message, window, cx);
                cx.notify();
                return;
            }
        };

        let Some((generation, attempt)) = self.connection_state.begin_editor_test() else {
            return;
        };
        let mut context = self.ssh_session_config_build_context();
        context.attempt = attempt.clone();
        context.host_key_prompts = Arc::new(context.host_key_prompts.for_attempt(attempt.clone()));
        context.credential_prompts =
            Arc::new(context.credential_prompts.for_attempt(attempt.clone()));

        self.shell
            .set_status(t!("dialog.testingConnection").to_string());
        cx.notify();

        // Packaged builds install the RDP helper as a native component rather
        // than beside the executable. Resolve that component before the
        // blocking test starts; portable and debug builds find the helper
        // directly beside the executable.
        if let Some(component) = needed_native_component(&built)
            && !helper_available_beside_exe(component)
        {
            let built = built.clone();
            let context = context.clone();
            let attempt_for_start = attempt.clone();
            let valid_attempt = attempt.clone();
            self.ensure_native_component(
                component,
                move |app| {
                    app.connection_state.editor_test_in_flight() && !valid_attempt.is_cancelled()
                },
                move |app, result, cx| match result {
                    Ok(launch) => app.start_connection_editor_test(
                        generation,
                        built,
                        context,
                        attempt_for_start,
                        Some(launch),
                        cx,
                    ),
                    Err(error) => {
                        app.finish_connection_editor_test(generation, Ok(Err(error.message)), cx)
                    }
                },
                cx,
            );
            return;
        }

        self.start_connection_editor_test(generation, built, context, attempt, None, cx);
    }

    fn start_connection_editor_test(
        &mut self,
        generation: u64,
        built: SavedConnection,
        context: SshSessionConfigBuildContext,
        attempt: ConnectionAttempt,
        native_launch: Option<nyaterm_core::plugins::native::NativeLaunch>,
        cx: &mut Context<Self>,
    ) {
        let task = self.blocking_jobs.submit_task("connection-test", move |_| {
            let timeout = attempt.clone();
            std::thread::spawn(move || {
                std::thread::sleep(CONNECTION_TEST_TIMEOUT);
                timeout.cancel();
            });
            run_connection_test(&built, &context, attempt, native_launch)
        });
        let Ok(task) = task else {
            self.connection_state.finish_editor_test(generation);
            let message = t!("dialog.testConnectionFailed", error = JOB_START_ERROR).to_string();
            self.shell.set_status(message.clone());
            self.notify_operation(
                "connection-test",
                nyaterm_ui::notification::NyaNotificationKind::Error,
                message,
                cx,
            );
            cx.notify();
            return;
        };

        cx.spawn(async move |this, cx| {
            let result = crate::features::runtime_jobs::await_blocking_job(Ok(task)).await;
            let _ = this.update(cx, |this, cx| {
                this.finish_connection_editor_test(generation, result, cx);
            });
        })
        .detach();
    }

    fn finish_connection_editor_test(
        &mut self,
        generation: u64,
        result: Result<Result<ConnectionTestSuccess, String>, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.connection_state.finish_editor_test(generation) {
            return;
        }
        let (message, kind) = match result {
            Ok(Ok(ConnectionTestSuccess::SshAuthenticated)) => (
                t!("dialog.testConnectionSshOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::TcpReachable)) => (
                t!("dialog.testConnectionTcpOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::FtpAuthenticated)) => (
                t!("dialog.testConnectionFtpOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::TelnetAuthenticated)) => (
                t!("dialog.testConnectionTelnetOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::VncAuthenticated)) => (
                t!("dialog.testConnectionVncOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::RdpAuthenticated)) => (
                t!("dialog.testConnectionRdpOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::SpiceAuthenticated)) => (
                t!("dialog.testConnectionSpiceOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Ok(ConnectionTestSuccess::SerialOpened)) => (
                t!("dialog.testConnectionSerialOk").to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Success,
            ),
            Ok(Err(error)) => {
                let message = if error.contains("cancelled") {
                    t!("dialog.testConnectionTimedOut").to_string()
                } else {
                    t!("dialog.testConnectionFailed", error = error).to_string()
                };
                (
                    message,
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                )
            }
            Err(error) => (
                t!("dialog.testConnectionFailed", error = error).to_string(),
                nyaterm_ui::notification::NyaNotificationKind::Error,
            ),
        };
        self.shell.set_status(message.clone());
        self.notify_operation("connection-test", kind, message, cx);
        cx.notify();
    }
}

fn run_connection_test(
    connection: &SavedConnection,
    context: &SshSessionConfigBuildContext,
    attempt: ConnectionAttempt,
    native_launch: Option<nyaterm_core::plugins::native::NativeLaunch>,
) -> Result<ConnectionTestSuccess, String> {
    let mut context = context.clone();
    context.attempt = attempt.clone();
    match &connection.config {
        ConnectionType::Ssh { .. } => {
            // The builder normally refreshes a saved connection by id. A test
            // must use the draft, including unsaved edits, so give it an id
            // that cannot resolve and let the passed snapshot win.
            let mut draft = connection.clone();
            draft.id = nyaterm_core::uuid();
            let mut config = context.build_config(&draft)?;
            use_test_only_host_keys(&mut config, &context);
            config.bind_attempt(attempt.clone());
            let handle = nyaterm_transport::open_ssh_multiplex_handle(config)
                .map_err(|error| error.to_string())?;
            let _ = handle.disconnect();
            Ok(ConnectionTestSuccess::SshAuthenticated)
        }
        ConnectionType::Telnet { .. } => {
            let config = telnet_config_from_draft(connection, &context)?;
            match nyaterm_transport::probe_telnet_connection(&config, &attempt)? {
                TelnetProbeOutcome::Reachable => Ok(ConnectionTestSuccess::TcpReachable),
                TelnetProbeOutcome::Authenticated => Ok(ConnectionTestSuccess::TelnetAuthenticated),
            }
        }
        ConnectionType::Ftp {
            host,
            port,
            username,
            initial_directory,
            passive_mode,
            ftp_filename_encoding_mode,
            ftp_filename_encoding,
            ftp_ascii_mode,
            ftp_security_mode,
            ftp_tls_version,
            connect_timeout_ms,
            ..
        } => {
            let login = ftp_login_from_draft(connection, username, &context)?;
            let config = FtpConfig {
                host: host.clone(),
                port: *port,
                initial_directory: initial_directory.clone(),
                connect_timeout: Duration::from_millis((*connect_timeout_ms).clamp(1_000, 120_000)),
                passive_mode: FtpPassiveMode::parse(passive_mode),
                filename_encoding_mode: FtpFilenameEncodingMode::parse(ftp_filename_encoding_mode),
                filename_encoding: ftp_filename_encoding.clone(),
                ascii_mode: *ftp_ascii_mode,
                security_mode: FtpSecurityMode::parse(ftp_security_mode)
                    .map_err(|error| error.to_string())?,
                tls_version: FtpTlsVersion::parse(ftp_tls_version)
                    .map_err(|error| error.to_string())?,
            };
            let cancel = attempt.clone();
            let service =
                FtpFileService::connect_with_cancel(config, login, move || cancel.is_cancelled())
                    .map_err(|error| error.to_string())?;
            service.disconnect();
            Ok(ConnectionTestSuccess::FtpAuthenticated)
        }
        ConnectionType::Rdp {
            host,
            port,
            username,
            domain,
            backend,
            security,
            display,
            clipboard,
            redirection,
            reconnect: _,
        } => {
            let backend = nyaterm_remote_desktop::parse_rdp_backend(backend)
                .map_err(|error| error.to_string())?;
            if backend == nyaterm_remote_desktop::RdpBackend::Windows {
                // The Windows Native backend is hosted by the ActiveX component
                // rather than the isolated IronRDP helper, so keep the
                // reachability check for it.
                probe_network(connection, host, *port, &context, &attempt)?;
                return Ok(ConnectionTestSuccess::TcpReachable);
            }
            let (username, password) =
                connection_credentials_from_draft(connection, username, &context)?;
            let route = start_test_route(connection, host, *port, &context)?;
            let config = nyaterm_remote_desktop::RdpSessionConfig {
                relay: route.as_ref().map(|route| route.endpoint.clone()),
                backend,
                windows_authentication: security.windows_authentication.clone(),
                name: connection.name.clone(),
                host: host.clone(),
                port: *port,
                username,
                domain: domain.clone(),
                password,
                use_nla: security.use_nla,
                certificate_policy: nyaterm_remote_desktop::parse_rdp_certificate_policy(
                    &security.certificate_policy,
                ),
                administrative_session: security.administrative_session,
                display: nyaterm_remote_desktop::RdpDisplayConfig::from(display),
                clipboard: nyaterm_remote_desktop::RdpClipboardConfig {
                    mode: nyaterm_remote_desktop::parse_rdp_clipboard_mode(&clipboard.mode),
                },
                redirection: redirection.into(),
                reconnect: nyaterm_remote_desktop::RdpReconnectConfig {
                    enabled: false,
                    max_attempts: 0,
                },
            };
            let result = nyaterm_remote_desktop::probe_rdp_connection_with_launch(
                config,
                native_launch,
                CONNECTION_TEST_TIMEOUT,
                || attempt.is_cancelled(),
            );
            drop(route);
            result.map_err(|error| error.message)?;
            Ok(ConnectionTestSuccess::RdpAuthenticated)
        }
        ConnectionType::Vnc {
            host,
            port,
            security,
            ..
        } => {
            let route = start_test_route(connection, host, *port, &context)?;
            let relay = route.as_ref().map(|route| route.endpoint.clone());
            let password = vnc_password_from_draft(connection, &context)?;
            let password = password.map(|password| password.expose_secret().to_string());
            let outcome = nyaterm_remote_desktop::probe_vnc_connection(
                host,
                *port,
                relay.as_ref(),
                password.as_deref(),
                nyaterm_remote_desktop::parse_vnc_security_mode(&security.mode),
                CONNECTION_TEST_TIMEOUT,
                &|| attempt.is_cancelled(),
            );
            let result = match outcome {
                Ok(nyaterm_remote_desktop::VncProbeOutcome::Authenticated) => {
                    Ok(ConnectionTestSuccess::VncAuthenticated)
                }
                Ok(nyaterm_remote_desktop::VncProbeOutcome::Reachable) => {
                    // The server only offered a security type the lightweight
                    // probe cannot evaluate. Let the isolated helper negotiate
                    // RA2/VeNCrypt/Tight and verify authentication there.
                    match vnc_session_config_from_draft(connection, relay, &context) {
                        Ok(config) => {
                            match nyaterm_remote_desktop::probe_vnc_connection_with_launch(
                                config,
                                native_launch,
                                CONNECTION_TEST_TIMEOUT,
                                || attempt.is_cancelled(),
                            ) {
                                Ok(()) => Ok(ConnectionTestSuccess::VncAuthenticated),
                                Err(error)
                                    if matches!(
                                        error.kind,
                                        nyaterm_remote_desktop::VncErrorKind::HelperMissing
                                    ) =>
                                {
                                    Ok(ConnectionTestSuccess::TcpReachable)
                                }
                                Err(error) => Err(error.message),
                            }
                        }
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            };
            drop(route);
            result
        }
        ConnectionType::Spice {
            endpoint,
            username,
            settings,
        } => {
            let (_, password) = connection_credentials_from_draft(connection, username, &context)?;
            let mut settings = settings.clone();
            settings.reconnect_attempts = 0;
            let config = nyaterm_remote_desktop::spice::protocol::SpiceSessionConfig {
                session_id: nyaterm_core::uuid(),
                endpoint: endpoint.clone(),
                username: username.clone().into(),
                password: password.unwrap_or_default(),
                settings,
                credential_lifetime:
                    nyaterm_remote_desktop::spice::protocol::SpiceCredentialLifetime::Temporary,
                // Routed SPICE needs separate TCP/TLS relays; the direct path
                // still verifies the ticket and TLS identity.
                tcp_relay: None,
                tls_relay: None,
            };
            config.validate().map_err(str::to_owned)?;
            nyaterm_remote_desktop::probe_spice_connection(
                config,
                native_launch,
                CONNECTION_TEST_TIMEOUT,
                || attempt.is_cancelled(),
            )
            .map_err(|kind| format!("SPICE connection failed: {kind:?}"))?;
            Ok(ConnectionTestSuccess::SpiceAuthenticated)
        }
        ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            flow_control,
            stop_bits,
            backspace_mode,
            encoding,
            ..
        } => {
            if port_name.trim().is_empty() {
                return Err(t!("dialog.testConnectionSerialPortRequired").to_string());
            }
            let config = SerialSessionConfig {
                name: connection.name.clone(),
                port_name: port_name.clone(),
                baud_rate: *baud_rate,
                data_bits: *data_bits,
                parity: parity.clone(),
                flow_control: *flow_control,
                stop_bits: stop_bits.clone(),
                backspace_mode: backspace_mode.clone(),
                encoding: encoding.clone(),
            };
            nyaterm_transport::probe_serial_connection(&config)?;
            Ok(ConnectionTestSuccess::SerialOpened)
        }
        ConnectionType::LocalTerminal { .. } | ConnectionType::Adb { .. } => {
            Err(t!("dialog.testConnectionUnavailable").to_string())
        }
    }
}

fn telnet_config_from_draft(
    connection: &SavedConnection,
    context: &SshSessionConfigBuildContext,
) -> Result<TelnetSessionConfig, String> {
    let ConnectionType::Telnet {
        host,
        port,
        username,
        backspace_mode,
        raw_tcp_cli,
        enter_mode,
        local_echo,
        local_line_edit,
        force_character_at_a_time,
        send_naws,
        send_sga,
        keep_alive_interval_secs,
        auto_login,
        encoding,
        ..
    } = &connection.config
    else {
        return Err("connection is not Telnet".to_string());
    };
    let (username, password) = connection_credentials_from_draft(connection, username, context)?;
    let encoding = if encoding.trim().is_empty() || encoding.trim().eq_ignore_ascii_case("global") {
        context.default_encoding.clone()
    } else {
        encoding.trim().to_string()
    };
    let mut config = TelnetSessionConfig {
        name: connection.name.clone(),
        host: host.clone(),
        port: *port,
        username,
        password,
        proxy: None,
        proxy_jump: None,
        backspace_mode: backspace_mode.clone(),
        raw_tcp: *raw_tcp_cli,
        enter_mode: parse_telnet_enter_mode(enter_mode),
        local_echo: *local_echo,
        local_line_edit: *local_line_edit,
        force_character_at_a_time: *force_character_at_a_time,
        send_naws: *send_naws,
        send_sga: *send_sga,
        keep_alive_interval_secs: *keep_alive_interval_secs,
        auto_login: map_telnet_auto_login_config(auto_login),
        encoding,
        cols: 80,
        rows: 24,
    };
    context.prepare_telnet_network(connection, &mut config)?;
    if let Some(jump) = config.proxy_jump.as_mut() {
        use_test_only_host_keys(jump, context);
    }
    Ok(config)
}

fn connection_credentials_from_draft(
    connection: &SavedConnection,
    connection_username: &str,
    context: &SshSessionConfigBuildContext,
) -> Result<(String, Option<SecretString>), String> {
    let auth = connection.auth.clone().unwrap_or_default();
    if auth.mode == "none" {
        return Ok((connection_username.to_string(), None));
    }
    if auth.uses_account_password() && auth.saved_account_id().is_some() {
        let auth_for_store = auth.clone();
        let account = context
            .store
            .request_fn(StoreDomain::Security, move |store| {
                store.load_account_for_auth(&auth_for_store)
            })
            .map_err(|error| error.to_string())?;
        let (resolved, password) = auth
            .resolve_account_auth(connection_username, account.as_ref())
            .map_err(ftp_account_error)?;
        return Ok((resolved, password.filter(|value| !value.trim().is_empty())));
    }
    if let Some(password) = auth
        .password
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        if auth.has_password {
            return Err(t!("passwordManager.lockedConnectionPassword").to_string());
        }
        return Ok((connection_username.to_string(), Some(password)));
    }
    if let Some(password_id) = auth
        .password_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        let entry = context
            .store
            .request_fn(StoreDomain::Security, move |store| {
                store.load_decrypted_password_by_id(&password_id)
            })
            .map_err(|error| error.to_string())?;
        return Ok((
            connection_username.to_string(),
            entry
                .and_then(|entry| entry.password)
                .filter(|value| !value.trim().is_empty()),
        ));
    }
    Ok((connection_username.to_string(), None))
}

fn map_telnet_auto_login_config(
    config: &nyaterm_core::TelnetAutoLoginConfig,
) -> nyaterm_transport::TelnetAutoLoginConfig {
    nyaterm_transport::TelnetAutoLoginConfig {
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

fn ftp_login_from_draft(
    connection: &SavedConnection,
    username: &str,
    context: &SshSessionConfigBuildContext,
) -> Result<FtpLogin, String> {
    let auth = connection.auth.clone().unwrap_or_default();
    if auth.mode == "none" {
        return Ok(FtpLogin::anonymous(username.to_string()));
    }
    if auth.uses_account_password() && auth.saved_account_id().is_some() {
        let auth_for_store = auth.clone();
        let account = context
            .store
            .request_fn(StoreDomain::Security, move |store| {
                store.load_account_for_auth(&auth_for_store)
            })
            .map_err(|error| error.to_string())?;
        let (resolved_username, password) = auth
            .resolve_account_auth(username, account.as_ref())
            .map_err(ftp_account_error)?;
        let password = password
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| t!("dialog.ftpPasswordRequired").to_string())?;
        return Ok(FtpLogin::password(resolved_username, password));
    }
    if let Some(password) = auth
        .password
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        if auth.has_password {
            return Err(t!("passwordManager.lockedConnectionPassword").to_string());
        }
        return Ok(FtpLogin::password(username.to_string(), password));
    }
    if let Some(password_id) = auth
        .password_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        let entry = context
            .store
            .request_fn(StoreDomain::Security, move |store| {
                store.load_decrypted_password_by_id(&password_id)
            })
            .map_err(|error| error.to_string())?;
        let password = entry
            .and_then(|entry| entry.password)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| t!("passwordManager.lockedConnectionPassword").to_string())?;
        return Ok(FtpLogin::password(username.to_string(), password));
    }
    Err(t!("dialog.ftpPasswordRequired").to_string())
}

fn ftp_account_error(error: AccountAuthError) -> String {
    let key = match error {
        AccountAuthError::MissingAccount => "passwordManager.missingAccount",
        AccountAuthError::EmptyOrLockedAccountPassword => "passwordManager.emptyOrLockedPassword",
        AccountAuthError::LockedConnectionPassword => "passwordManager.lockedConnectionPassword",
    };
    t!(key).to_string()
}

fn vnc_password_from_draft(
    connection: &SavedConnection,
    context: &SshSessionConfigBuildContext,
) -> Result<Option<SecretString>, String> {
    let ConnectionType::Vnc { username, .. } = &connection.config else {
        return Ok(None);
    };
    let (_, password) = connection_credentials_from_draft(connection, username, context)?;
    Ok(password)
}

fn vnc_session_config_from_draft(
    connection: &SavedConnection,
    relay: Option<nyaterm_core::connection_route::RelayEndpoint>,
    context: &SshSessionConfigBuildContext,
) -> Result<nyaterm_remote_desktop::VncSessionConfig, String> {
    let ConnectionType::Vnc {
        username,
        host,
        port,
        security,
        display,
        clipboard,
        shared,
        view_only,
        ..
    } = &connection.config
    else {
        return Err("connection is not VNC".to_string());
    };
    let (username, password) = connection_credentials_from_draft(connection, username, context)?;
    Ok(nyaterm_remote_desktop::VncSessionConfig {
        username,
        relay,
        name: connection.name.clone(),
        host: host.clone(),
        port: *port,
        password,
        security: nyaterm_remote_desktop::VncSecurityConfig {
            mode: nyaterm_remote_desktop::parse_vnc_security_mode(&security.mode),
        },
        display: nyaterm_remote_desktop::VncDisplayConfig {
            scale_mode: nyaterm_remote_desktop::parse_vnc_scale_mode(&display.scale_mode),
        },
        clipboard: nyaterm_remote_desktop::VncClipboardConfig {
            enabled: clipboard.enabled,
        },
        reconnect: nyaterm_remote_desktop::VncReconnectConfig {
            enabled: false,
            max_attempts: 0,
        },
        shared: *shared,
        view_only: *view_only,
    })
}

fn use_test_only_host_keys(config: &mut SshSessionConfig, context: &SshSessionConfigBuildContext) {
    config.host_key_verifier = Some(Arc::new(NativeHostKeyVerifier {
        store: context.store.clone(),
        policy: context.host_key_policy.clone(),
        prompt_broker: context.host_key_prompts.clone(),
        // A test can verify a fingerprint but must not make it trusted for
        // later connections. The next real connection asks again.
        persist_known_hosts: false,
    }));
    if let Some(jump) = config.proxy_jump.as_mut() {
        use_test_only_host_keys(jump, context);
    }
}

fn start_test_route(
    connection: &SavedConnection,
    host: &str,
    port: u16,
    context: &SshSessionConfigBuildContext,
) -> Result<Option<Arc<NetworkRoute>>, String> {
    if !has_network_route(connection) {
        return Ok(None);
    }
    let config = context.network_route_config(connection, host.to_string(), port)?;
    NetworkRoute::start(config)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn needed_native_component(
    connection: &SavedConnection,
) -> Option<nyaterm_core::plugins::native::NativeComponentId> {
    use nyaterm_core::plugins::native::NativeComponentId;

    match &connection.config {
        ConnectionType::Rdp { backend, .. }
            if matches!(
                nyaterm_remote_desktop::parse_rdp_backend(backend),
                Ok(nyaterm_remote_desktop::RdpBackend::Builtin)
            ) =>
        {
            Some(NativeComponentId::Rdp)
        }
        ConnectionType::Spice { .. } => Some(NativeComponentId::Spice),
        ConnectionType::Vnc { .. } => Some(NativeComponentId::Vnc),
        _ => None,
    }
}

fn helper_available_beside_exe(
    component: nyaterm_core::plugins::native::NativeComponentId,
) -> bool {
    use nyaterm_core::plugins::native::NativeComponentId;

    match component {
        NativeComponentId::Rdp => nyaterm_remote_desktop::resolve_helper_path().is_ok(),
        NativeComponentId::Spice => {
            nyaterm_remote_desktop::spice::session::SpiceSessionManager::resolve_spice_helper_path()
                .is_ok()
        }
        NativeComponentId::Vnc => nyaterm_remote_desktop::resolve_vnc_helper_path().is_ok(),
        NativeComponentId::Android => false,
    }
}

fn has_network_route(connection: &SavedConnection) -> bool {
    connection.network.as_ref().is_some_and(|network| {
        network
            .proxy_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || network
                .proxy_jump_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    })
}

fn probe_network(
    connection: &SavedConnection,
    host: &str,
    port: u16,
    context: &SshSessionConfigBuildContext,
    attempt: &ConnectionAttempt,
) -> Result<(), String> {
    if has_network_route(connection) {
        let mut config = context.network_route_config(connection, host.to_string(), port)?;
        if let Some(jump) = config.jump.as_mut() {
            use_test_only_host_keys(jump, context);
        }
        NetworkRoute::probe(config, attempt.clone()).map_err(|error| error.to_string())
    } else {
        probe_tcp_direct(host, port, attempt)
    }
}

fn probe_tcp_direct(host: &str, port: u16, attempt: &ConnectionAttempt) -> Result<(), String> {
    attempt.check()?;
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("could not resolve {host}:{port}"));
    }
    let mut last_error = None;
    for address in addresses {
        attempt.check()?;
        match TcpStream::connect_timeout(&address, CONNECTION_TEST_TCP_TIMEOUT) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(|error| error.to_string())
        .unwrap_or_else(|| format!("could not connect to {host}:{port}")))
}

/// Kept as a distinct value so the failure text never exposes a scheduler
/// implementation detail.
const JOB_START_ERROR: &str = "connection test could not be queued";

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use nyaterm_transport::connection_attempt::ConnectionAttempt;

    use super::{connection_kind_supports_test, probe_tcp_direct};
    use crate::models::{
        ConnectionEditorPasswordSource, ConnectionKindTab, ConnectionPasswordEdit,
    };

    #[test]
    fn only_kinds_with_a_network_or_device_target_offer_a_test() {
        for kind in [
            ConnectionKindTab::Ssh,
            ConnectionKindTab::Telnet,
            ConnectionKindTab::Serial,
            ConnectionKindTab::Ftp,
            ConnectionKindTab::Rdp,
            ConnectionKindTab::Vnc,
            ConnectionKindTab::Spice,
        ] {
            assert!(connection_kind_supports_test(kind), "{kind:?}");
        }
        for kind in [ConnectionKindTab::Local, ConnectionKindTab::Adb] {
            assert!(!connection_kind_supports_test(kind), "{kind:?}");
        }
    }

    #[test]
    fn tcp_probe_accepts_a_reachable_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let port = listener.local_addr().expect("listener address").port();
        let attempt = ConnectionAttempt::default();
        assert!(probe_tcp_direct("127.0.0.1", port, &attempt).is_ok());
    }

    #[test]
    fn tcp_probe_stops_before_dialing_a_cancelled_attempt() {
        let attempt = ConnectionAttempt::default();
        attempt.cancel();
        assert!(probe_tcp_direct("127.0.0.1", 9, &attempt).is_err());
    }

    #[test]
    fn telnet_test_uses_the_draft_auto_login_credentials() {
        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-telnet-draft");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Telnet;
            editor.host = "telnet.example.test".into();
            editor.port = "23".into();
            editor.username = "telnet-user".into();
            editor.auth_mode = "password".into();
            editor.password_source = ConnectionEditorPasswordSource::Direct;
            editor.password_edit = ConnectionPasswordEdit::Replace;
            editor.password = "telnet-secret".into();

            let built = build_saved_connection_from_editor(&editor).expect("draft builds");
            let mut context = app.ssh_session_config_build_context();
            context.attempt = ConnectionAttempt::default();
            let config = super::telnet_config_from_draft(&built, &context).expect("draft Telnet");
            assert_eq!(config.host, "telnet.example.test");
            assert_eq!(config.port, 23);
            assert_eq!(config.username, "telnet-user");
            assert_eq!(
                config.password.as_ref().map(|value| value.expose_secret()),
                Some("telnet-secret")
            );
            assert!(config.auto_login.enabled);
        });
    }

    #[test]
    fn vnc_test_uses_the_draft_password() {
        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-vnc-draft");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Vnc;
            editor.host = "vnc.example.test".into();
            editor.port = "5900".into();
            editor.auth_mode = "password".into();
            editor.password_source = ConnectionEditorPasswordSource::Direct;
            editor.password_edit = ConnectionPasswordEdit::Replace;
            editor.password = "vnc-secret".into();
            editor.vnc_security.mode = "vnc-auth".into();

            let built = build_saved_connection_from_editor(&editor).expect("draft builds");
            let context = app.ssh_session_config_build_context();
            let password =
                super::vnc_password_from_draft(&built, &context).expect("draft VNC password");
            assert_eq!(
                password.as_ref().map(|value| value.expose_secret()),
                Some("vnc-secret")
            );
            let nyaterm_core::ConnectionType::Vnc { security, .. } = &built.config else {
                panic!("draft must stay VNC");
            };
            assert_eq!(
                nyaterm_remote_desktop::parse_vnc_security_mode(&security.mode),
                nyaterm_remote_desktop::VncSecurityMode::VncAuth
            );
        });
    }

    #[test]
    fn ftp_test_uses_the_draft_password_instead_of_skipping_authentication() {
        use nyaterm_files::ftp::FtpLogin;

        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-ftp-draft");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Ftp;
            editor.host = "ftp.example.test".into();
            editor.port = "21".into();
            editor.username = "ftp-user".into();
            editor.auth_mode = "password".into();
            editor.password_source = ConnectionEditorPasswordSource::Direct;
            editor.password_edit = ConnectionPasswordEdit::Replace;
            editor.password = "ftp-secret".into();

            let built = build_saved_connection_from_editor(&editor).expect("draft builds");
            let context = app.ssh_session_config_build_context();
            let login =
                super::ftp_login_from_draft(&built, "ftp-user", &context).expect("draft FTP login");
            assert_eq!(login.username(), "ftp-user");
            let FtpLogin::Password { password, .. } = login else {
                panic!("a password draft must not become an anonymous login");
            };
            assert_eq!(password.expose_secret(), "ftp-secret");
        });
    }

    #[test]
    fn ssh_test_builds_from_the_unsaved_draft_instead_of_the_saved_connection() {
        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-ssh-draft");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Ssh;
            editor.host = "draft.example.test".into();
            editor.port = "2222".into();
            editor.username = "draft-user".into();
            editor.auth_mode = "password".into();
            editor.password_source = ConnectionEditorPasswordSource::Direct;
            editor.password_edit = ConnectionPasswordEdit::Replace;
            editor.password = "draft-secret".into();

            let mut built = build_saved_connection_from_editor(&editor).expect("draft builds");
            built.id = nyaterm_core::uuid();
            let mut context = app.ssh_session_config_build_context();
            context.attempt = ConnectionAttempt::default();
            let config = context.build_config(&built).expect("draft SSH config");
            assert_eq!(config.host, "draft.example.test");
            assert_eq!(config.port, 2222);
            assert_eq!(config.username, "draft-user");
            assert_eq!(
                config.password.as_ref().map(|value| value.expose_secret()),
                Some("draft-secret"),
                "the unsaved password must reach the probe"
            );
        });
    }

    #[test]
    fn ssh_test_uses_the_selected_private_key() {
        use nyaterm_store::StoreDomain;

        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-ssh-key");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let key_id = app
                .store_blocking_client()
                .request_mutation_fn(StoreDomain::Security, |store| {
                    store.save_ssh_key(nyaterm_core::models::credentials::SshKey {
                        id: String::new(),
                        sort_order: 0,
                        name: "draft key".into(),
                        key: Some("fixture-key-data".into()),
                        cert: None,
                        passphrase: None,
                        key_file_path: None,
                        cert_file_path: None,
                        has_key_data: true,
                        has_cert_data: false,
                    })
                })
                .expect("save key");
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Ssh;
            editor.host = "key.example.test".into();
            editor.port = "22".into();
            editor.username = "demo".into();
            editor.auth_mode = "key".into();
            editor.key_id = Some(key_id);

            let mut built = build_saved_connection_from_editor(&editor).expect("draft builds");
            built.id = nyaterm_core::uuid();
            let context = app.ssh_session_config_build_context();
            let config = context.build_config(&built).expect("key auth config");
            assert!(
                config.key_auth.is_some(),
                "the selected private key must reach the probe"
            );
            assert!(!config.agent_auth);
        });
    }

    #[test]
    fn ssh_test_uses_the_selected_agent() {
        use crate::features::connections::connection_runtime::helpers::build_saved_connection_from_editor;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        let config_dir = TestConfigDir::new("nyaterm-connection-test-ssh-agent");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, config_dir.path(), "local");
        app.update(&mut cx, |app, cx| {
            let mut editor = app.new_preferred_connection_editor_state(None, false, cx);
            editor.kind = ConnectionKindTab::Ssh;
            editor.host = "agent.example.test".into();
            editor.port = "22".into();
            editor.username = "demo".into();
            editor.auth_mode = "agent".into();
            editor.agent_endpoint = nyaterm_core::SshAgentEndpoint::WindowsOpenSsh;

            let mut built = build_saved_connection_from_editor(&editor).expect("draft builds");
            built.id = nyaterm_core::uuid();
            let context = app.ssh_session_config_build_context();
            let config = context.build_config(&built).expect("agent auth config");
            assert!(config.agent_auth, "agent auth must reach the probe");
            assert!(config.key_auth.is_none());
            assert_eq!(
                config.agent_endpoint,
                nyaterm_transport::SshAgentEndpoint::WindowsOpenSsh
            );
        });
    }
}

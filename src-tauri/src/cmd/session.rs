use crate::config;
use crate::core::monitoring::stats::RemoteStatsSampler;
use crate::core::ssh::{
    self, HostKeyVerifyManager, PendingAuthManager, PendingSshAgentAuthManager,
    PendingSshAuthManager, SshAgentAuthAction, SshAuthResponse,
};
use crate::core::zmodem::ZmodemUploadConflictMode;
use crate::core::{
    self, ExistingFileBehavior, InputOrigin, InputSensitivity, QuickCommandsStore,
    RecordingContext, RecordingManager, RecordingMode, RecordingProfile, SessionCommand,
    SessionInfo, SessionManager, SessionReadyHook, TerminalHistorySearchRequest,
    TerminalHistorySearchResponse,
};
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use crate::utils::fuzzy::{
    FuzzyCandidateResult, FuzzyResult, FuzzySearchCandidate,
    fuzzy_search_candidates as fuzzy_search_candidate_items, fuzzy_search_items,
};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupCommandPayload {
    command: String,
    delay_ms: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRecordingRequest {
    session_id: String,
    mode: RecordingMode,
    explicit_path: Option<String>,
}

#[tauri::command]
pub async fn create_ssh_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    connection_id: String,
    create_request_id: Option<String>,
    startup_command: Option<StartupCommandPayload>,
    runtime_mode: Option<crate::config::SshRuntimeMode>,
) -> AppResult<String> {
    let mut ssh_config = ssh::load_saved_ssh_config(&app, &connection_id)?;
    if let Some(runtime_mode) = runtime_mode {
        ssh_config.runtime_mode = runtime_mode;
    }
    let pending_creation = state.begin_session_creation(create_request_id).await;
    let (guard, cancel_rx) = match pending_creation {
        Some((guard, cancel_rx)) => (Some(guard), Some(cancel_rx)),
        None => (None, None),
    };

    let session_id = ssh::create_ssh_session(
        app.clone(),
        state.inner().clone(),
        ssh_config,
        Some(connection_id.clone()),
        Some(window.label().to_string()),
        cancel_rx,
        startup_command.map(startup_command_payload_to_ssh),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    drop(guard);
    mark_connection_used(&app, &connection_id);
    Ok(session_id)
}

#[tauri::command]
pub async fn create_temporary_ssh_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    config: ssh::SshConfig,
    create_request_id: Option<String>,
    startup_command: Option<StartupCommandPayload>,
) -> AppResult<String> {
    let encoding = crate::config::load_app_settings(&app)
        .map(|settings| settings.interaction.default_encoding)
        .unwrap_or_else(|_| "UTF-8".to_string());
    let ssh_config = normalize_temporary_ssh_config(config, &encoding);
    let pending_creation = state.begin_session_creation(create_request_id).await;
    let (guard, cancel_rx) = match pending_creation {
        Some((guard, cancel_rx)) => (Some(guard), Some(cancel_rx)),
        None => (None, None),
    };

    let session_id = ssh::create_ssh_session(
        app.clone(),
        state.inner().clone(),
        ssh_config,
        None,
        Some(window.label().to_string()),
        cancel_rx,
        startup_command.map(startup_command_payload_to_ssh),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    drop(guard);
    Ok(session_id)
}

fn startup_command_payload_to_ssh(command: StartupCommandPayload) -> ssh::SshStartupCommand {
    ssh::SshStartupCommand {
        command: command.command,
        delay_ms: command.delay_ms,
    }
}

fn normalize_temporary_ssh_config(mut config: ssh::SshConfig, encoding: &str) -> ssh::SshConfig {
    config.connection_id = None;
    config.owner_window_label = None;
    config.backspace_mode = if config.backspace_mode.trim().is_empty() {
        "del".to_string()
    } else {
        config.backspace_mode
    };
    config.x11_forwarding = false;
    config.x11_display = String::new();
    config.auth_agent_endpoint = matches!(&config.auth, ssh::SshAuth::Agent)
        .then_some(crate::config::SshAgentEndpoint::Auto);
    config.agent_forwarding_config = crate::config::SshAgentForwardingConfig::default();
    config.proxy = None;
    config.proxy_jump = None;
    config.post_login = None;
    config.ssh_algorithms = None;
    config.dynamic_tab_title = false;
    if config.ssh_profile == crate::config::SshProfile::NetworkDevice
        && config.terminal_type == crate::config::SshTerminalType::Xterm256Color
    {
        config.terminal_type = crate::config::resolve_ssh_terminal_type(&config.ssh_profile, None);
    }
    // Inherit encoding from global settings only when no explicit value was provided.
    if config.encoding.trim().is_empty() {
        config.encoding = encoding.to_string();
    }
    config
}

#[tauri::command]
pub async fn create_multiplexed_ssh_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    source_session_id: String,
    startup_command: Option<StartupCommandPayload>,
) -> AppResult<String> {
    let session_id = ssh::create_multiplexed_ssh_session(
        app.clone(),
        state.inner().clone(),
        &source_session_id,
        startup_command.map(|command| ssh::SshStartupCommand {
            command: command.command,
            delay_ms: command.delay_ms,
        }),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    Ok(session_id)
}

#[tauri::command]
pub async fn create_local_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    connection_id: Option<String>,
    create_request_id: Option<String>,
    working_dir: Option<String>,
) -> AppResult<String> {
    let pending_creation = state.begin_session_creation(create_request_id).await;
    let (guard, _cancel_rx) = match pending_creation {
        Some((guard, cancel_rx)) => (Some(guard), Some(cancel_rx)),
        None => (None, None),
    };
    let config = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        let encoding = config::resolve_connection_encoding(&app, &conn);
        match conn.config {
            config::ConnectionType::LocalTerminal {
                shell_path,
                shell_args,
                working_dir: saved_working_dir,
                dynamic_tab_title,
                ..
            } => {
                let (working_dir, fail_on_missing_working_dir) =
                    resolve_local_working_dir(saved_working_dir, working_dir);
                Some(core::LocalSessionConfig {
                    connection_id: Some(cid.clone()),
                    shell_path,
                    shell_args,
                    working_dir,
                    fail_on_missing_working_dir,
                    name: conn.name,
                    encoding,
                    dynamic_tab_title,
                })
            }
            _ => None,
        }
    } else if working_dir.is_some() {
        let encoding = crate::config::load_app_settings(&app)
            .map(|s| s.interaction.default_encoding)
            .unwrap_or_else(|_| "UTF-8".to_string());
        Some(core::LocalSessionConfig {
            connection_id: None,
            shell_path: String::new(),
            shell_args: String::new(),
            working_dir,
            fail_on_missing_working_dir: true,
            name: "Local Terminal".to_string(),
            encoding,
            dynamic_tab_title: false,
        })
    } else {
        None
    };
    let session_id = core::create_local_session(
        app.clone(),
        state.inner().clone(),
        config,
        Some(window.label().to_string()),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    drop(guard);
    if let Some(connection_id) = connection_id {
        mark_connection_used(&app, &connection_id);
    }
    Ok(session_id)
}

fn resolve_local_working_dir(
    saved_working_dir: Option<String>,
    explicit_working_dir: Option<String>,
) -> (Option<String>, bool) {
    match explicit_working_dir {
        Some(working_dir) => (Some(working_dir), true),
        None => (saved_working_dir, false),
    }
}

#[tauri::command]
pub async fn create_telnet_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    connection_id: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    name: Option<String>,
    create_request_id: Option<String>,
    startup_command: Option<StartupCommandPayload>,
) -> AppResult<String> {
    let pending_creation = state.begin_session_creation(create_request_id).await;
    let (guard, _cancel_rx) = match pending_creation {
        Some((guard, cancel_rx)) => (Some(guard), Some(cancel_rx)),
        None => (None, None),
    };
    let cfg = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        let telnet_password = resolve_telnet_connection_password(&app, conn.auth.as_ref())?;
        let encoding = config::resolve_connection_encoding(&app, &conn);
        match conn.config {
            config::ConnectionType::Telnet {
                host: ref ch,
                port: cp,
                username,
                backspace_mode,
                raw_tcp_cli,
                enter_mode,
                local_echo,
                local_line_edit,
                force_character_at_a_time,
                send_naws,
                send_sga,
                auto_login,
                ..
            } => core::TelnetSessionConfig {
                host: ch.clone(),
                port: cp,
                name: conn.name.clone(),
                username,
                password: telnet_password,
                backspace_mode,
                raw_tcp_cli,
                enter_mode: core::TelnetEnterMode::from_config_value(&enter_mode),
                local_echo,
                local_line_edit,
                force_character_at_a_time,
                send_naws,
                send_sga,
                auto_login: core::TelnetAutoLoginConfig::from(auto_login),
                encoding,
            },
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Telnet connection".to_string(),
                ));
            }
        }
    } else {
        core::TelnetSessionConfig {
            host: host.ok_or_else(|| AppError::Config("host is required".to_string()))?,
            port: port.unwrap_or(23),
            name: name.unwrap_or_else(|| "Telnet".to_string()),
            ..Default::default()
        }
    };
    let marked_connection_id = connection_id.clone();
    let session_id = core::create_telnet_session(
        app.clone(),
        state.inner().clone(),
        cfg,
        connection_id,
        Some(window.label().to_string()),
        startup_command.map(|command| core::TelnetStartupCommand {
            command: command.command,
            delay_ms: command.delay_ms,
        }),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    drop(guard);
    if let Some(connection_id) = marked_connection_id {
        mark_connection_used(&app, &connection_id);
    }
    Ok(session_id)
}

fn resolve_telnet_connection_password(
    app: &tauri::AppHandle,
    auth: Option<&config::ConnectionAuth>,
) -> AppResult<Option<String>> {
    resolve_telnet_connection_password_with(auth, |password_id| {
        Ok(config::load_password_by_id(app, password_id)?.password)
    })
}

fn resolve_telnet_connection_password_with<F>(
    auth: Option<&config::ConnectionAuth>,
    mut load_saved_password: F,
) -> AppResult<Option<String>>
where
    F: FnMut(&str) -> AppResult<Option<String>>,
{
    let Some(auth) = auth else {
        return Ok(None);
    };
    if auth.mode != "password" {
        return Ok(None);
    }

    if let Some(password_id) = auth.password_id.as_deref().filter(|id| !id.is_empty()) {
        return load_saved_password(password_id);
    }

    crate::utils::crypto::decrypt_optional(&auth.password)
}

#[tauri::command]
pub async fn create_serial_session(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<SessionManager>>,
    recording_state: tauri::State<'_, Arc<RecordingManager>>,
    connection_id: Option<String>,
    port_name: Option<String>,
    baud_rate: Option<u32>,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<String>,
    name: Option<String>,
    create_request_id: Option<String>,
) -> AppResult<String> {
    let pending_creation = state.begin_session_creation(create_request_id).await;
    let (guard, _cancel_rx) = match pending_creation {
        Some((guard, cancel_rx)) => (Some(guard), Some(cancel_rx)),
        None => (None, None),
    };
    let cfg = if let Some(ref cid) = connection_id {
        let conn = config::load_connection_by_id(&app, cid)?;
        let encoding = config::resolve_connection_encoding(&app, &conn);
        match conn.config {
            config::ConnectionType::Serial {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                backspace_mode,
                ..
            } => core::SerialConfig {
                port_name,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                name: conn.name,
                backspace_mode,
                encoding,
            },
            _ => {
                return Err(AppError::Config(
                    "Connection is not a Serial connection".to_string(),
                ));
            }
        }
    } else {
        let encoding = crate::config::load_app_settings(&app)
            .map(|s| s.interaction.default_encoding)
            .unwrap_or_else(|_| "UTF-8".to_string());
        core::SerialConfig {
            port_name: port_name
                .ok_or_else(|| AppError::Config("port_name is required".to_string()))?,
            baud_rate: baud_rate.unwrap_or(115_200),
            data_bits: data_bits.unwrap_or(8),
            parity: parity.unwrap_or_else(|| "none".to_string()),
            stop_bits: stop_bits.unwrap_or_else(|| "1".to_string()),
            name: name.unwrap_or_else(|| "Serial".to_string()),
            backspace_mode: "ctrl_h".to_string(),
            encoding,
        }
    };
    let marked_connection_id = connection_id.clone();
    let session_id = core::create_serial_session(
        app.clone(),
        state.inner().clone(),
        cfg,
        connection_id,
        Some(window.label().to_string()),
        Some(build_auto_recording_hook(
            app.clone(),
            recording_state.inner().clone(),
        )),
    )
    .await?;
    drop(guard);
    if let Some(connection_id) = marked_connection_id {
        mark_connection_used(&app, &connection_id);
    }
    Ok(session_id)
}

fn mark_connection_used(app: &tauri::AppHandle, connection_id: &str) {
    if let Err(error) = crate::storage::mark_connection_used(connection_id) {
        tracing::warn!(connection_id, %error, "Failed to mark connection as recently used");
        return;
    }
    let _ = app.emit("connections-changed", ());
}

fn build_auto_recording_hook(
    app: tauri::AppHandle,
    recording_manager: Arc<RecordingManager>,
) -> SessionReadyHook {
    Arc::new(move |session_info: &SessionInfo| {
        let settings = match config::load_app_settings(&app) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_info.id,
                    %error,
                    "Failed to load settings for auto recording"
                );
                return;
            }
        };
        if !profile_auto_start(&app, session_info, &settings.recording) {
            return;
        }

        let (profile, context) = match build_recording_profile_and_context(&app, session_info, None)
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_info.id,
                    %error,
                    "Failed to resolve auto recording profile"
                );
                return;
            }
        };

        match recording_manager.start_with_profile(&session_info.id, context, profile, None) {
            Ok(_path) => {
                let _ = app.emit("sessions-changed", ());
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_info.id,
                    %error,
                    "Failed to auto-start recording"
                );
            }
        }
    })
}

fn profile_auto_start(
    app: &tauri::AppHandle,
    session_info: &SessionInfo,
    global: &config::RecordingSettings,
) -> bool {
    let Some(connection_id) = session_info.connection_id.as_deref() else {
        return global.auto_start;
    };
    let Ok(connection) = config::load_connection_by_id(app, connection_id) else {
        return global.auto_start;
    };
    connection
        .recording
        .and_then(|recording| recording.auto_start)
        .unwrap_or(global.auto_start)
}

fn build_recording_profile_and_context(
    app: &tauri::AppHandle,
    session_info: &SessionInfo,
    requested_mode: Option<RecordingMode>,
) -> AppResult<(RecordingProfile, RecordingContext)> {
    let settings = config::load_app_settings(app)?;
    let connection = session_info
        .connection_id
        .as_deref()
        .and_then(|id| config::load_connection_by_id(app, id).ok());
    let recording_override = connection.as_ref().and_then(|conn| conn.recording.clone());

    let base_path = if settings.recording.base_path.trim().is_empty() {
        default_recording_dir(app)?
    } else {
        PathBuf::from(&settings.recording.base_path)
    };
    let mode = requested_mode
        .or_else(|| {
            recording_override
                .as_ref()
                .and_then(|override_| override_.mode)
        })
        .unwrap_or(settings.recording.default_mode);
    let path_template = recording_override
        .as_ref()
        .and_then(|override_| override_.path_template.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| settings.recording.path_template.clone());
    let include_timestamps = recording_override
        .as_ref()
        .and_then(|override_| override_.include_timestamps)
        .unwrap_or(settings.recording.include_timestamps);
    let rotation = recording_override
        .as_ref()
        .and_then(|override_| override_.rotation.clone())
        .unwrap_or_else(|| settings.recording.rotation.clone());

    let context = build_recording_context(app, session_info, connection.as_ref());
    let profile = RecordingProfile {
        mode,
        base_path,
        path_template,
        include_timestamps,
        include_io_labels: settings.recording.include_io_labels,
        include_session_metadata: settings.recording.include_session_metadata,
        rotation,
        existing_file_behavior: settings.recording.existing_file_behavior,
        include_binary_transfer_payloads: settings.recording.include_binary_transfer_payloads,
    };

    Ok((profile, context))
}

fn build_recording_context(
    app: &tauri::AppHandle,
    session_info: &SessionInfo,
    connection: Option<&config::SavedConnection>,
) -> RecordingContext {
    let (protocol, host, port, username) = match connection.map(|conn| &conn.config) {
        Some(config::ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        }) => (
            "ssh".to_string(),
            Some(host.clone()),
            Some(*port),
            Some(username.clone()),
        ),
        Some(config::ConnectionType::Telnet {
            host,
            port,
            username,
            ..
        }) => (
            "telnet".to_string(),
            Some(host.clone()),
            Some(*port),
            Some(username.clone()),
        ),
        Some(config::ConnectionType::Rdp {
            host,
            port,
            username,
            ..
        }) => (
            "rdp".to_string(),
            Some(host.clone()),
            Some(*port),
            Some(username.clone()),
        ),
        Some(config::ConnectionType::Vnc { host, port, .. }) => {
            ("vnc".to_string(), Some(host.clone()), Some(*port), None)
        }
        Some(config::ConnectionType::Serial { port_name, .. }) => {
            ("serial".to_string(), Some(port_name.clone()), None, None)
        }
        Some(config::ConnectionType::LocalTerminal { .. }) => {
            ("local".to_string(), None, None, None)
        }
        None => (
            match session_info.session_type {
                core::SessionType::SSH => "ssh",
                core::SessionType::Local => "local",
                core::SessionType::Telnet => "telnet",
                core::SessionType::Serial => "serial",
            }
            .to_string(),
            None,
            None,
            None,
        ),
    };

    RecordingContext {
        session_id: session_info.id.clone(),
        session_name: session_info.name.clone(),
        connection_id: session_info.connection_id.clone(),
        connection_name: connection.map(|conn| conn.name.clone()),
        group_path: connection.and_then(|conn| resolve_group_path(app, conn.group_id.as_deref())),
        protocol,
        host,
        port,
        username,
        started_at: time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc()),
    }
}

fn resolve_group_path(app: &tauri::AppHandle, group_id: Option<&str>) -> Option<String> {
    let mut current = group_id?;
    let config = config::load_config(app).ok()?;
    let mut names = Vec::new();
    for _ in 0..32 {
        let group = config.groups.iter().find(|group| group.id == current)?;
        names.push(group.name.clone());
        let Some(parent) = group.parent_id.as_deref() else {
            break;
        };
        current = parent;
    }
    names.reverse();
    (!names.is_empty()).then(|| names.join("/"))
}

fn default_recording_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    match app.path().download_dir() {
        Ok(path) => Ok(path),
        Err(_) => dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
            .ok_or_else(|| AppError::Config("Failed to resolve Downloads directory".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StartupCommandPayload, normalize_temporary_ssh_config, resolve_local_working_dir,
        resolve_telnet_connection_password_with, startup_command_payload_to_ssh,
    };
    use crate::config::{ConnectionAuth, SshRuntimeMode};

    #[test]
    fn temporary_ssh_config_drops_saved_connection_features() {
        let config = serde_json::from_value(serde_json::json!({
            "connection_id": "saved-1",
            "owner_window_label": "main",
            "name": "root@example.com:22",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "auth": { "type": "none" },
            "backspace_mode": "",
            "x11_forwarding": true,
            "x11_display": ":0",
            "proxy": {
                "enabled": true,
                "protocol": "socks5",
                "host": "127.0.0.1",
                "port": 1080
            },
            "proxy_jump": {
                "name": "jump",
                "host": "jump.example.com",
                "port": 22,
                "username": "root",
                "auth": { "type": "none" }
            },
            "post_login": {
                "command": "uptime",
                "delay_ms": 1000
            },
            "runtime_mode": "terminal"
        }))
        .expect("temporary ssh config");

        let normalized = normalize_temporary_ssh_config(config, "UTF-8");

        assert!(normalized.connection_id.is_none());
        assert!(normalized.owner_window_label.is_none());
        assert_eq!(normalized.backspace_mode, "del");
        assert!(!normalized.x11_forwarding);
        assert!(normalized.x11_display.is_empty());
        assert!(normalized.proxy.is_none());
        assert!(normalized.proxy_jump.is_none());
        assert!(normalized.post_login.is_none());
        assert_eq!(normalized.runtime_mode, SshRuntimeMode::Terminal);
    }

    #[test]
    fn maps_startup_command_payload_to_ssh_command() {
        let command = startup_command_payload_to_ssh(StartupCommandPayload {
            command: "uptime".to_string(),
            delay_ms: 750,
        });

        assert_eq!(command.command, "uptime");
        assert_eq!(command.delay_ms, 750);
    }

    #[test]
    fn explicit_local_working_dir_overrides_saved_working_dir() {
        let (working_dir, fail_on_missing_working_dir) =
            resolve_local_working_dir(Some("saved".to_string()), Some("explicit".to_string()));

        assert_eq!(working_dir.as_deref(), Some("explicit"));
        assert!(fail_on_missing_working_dir);
    }

    #[test]
    fn missing_local_working_dir_override_preserves_saved_working_dir() {
        let (working_dir, fail_on_missing_working_dir) =
            resolve_local_working_dir(Some("saved".to_string()), None);

        assert_eq!(working_dir.as_deref(), Some("saved"));
        assert!(!fail_on_missing_working_dir);
    }

    #[test]
    fn telnet_password_resolution_ignores_non_password_auth() {
        let auth = ConnectionAuth {
            mode: "none".to_string(),
            password: Some("ignored".to_string()),
            ..ConnectionAuth::default()
        };

        let resolved =
            resolve_telnet_connection_password_with(Some(&auth), |_| Ok(Some("saved".to_string())))
                .expect("password resolution");

        assert_eq!(resolved, None);
    }

    #[test]
    fn telnet_password_resolution_decrypts_inline_password() {
        crate::utils::crypto::set_master_password(None);
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password: Some(crate::utils::crypto::encrypt("inline-secret").expect("encrypt")),
            ..ConnectionAuth::default()
        };

        let resolved = resolve_telnet_connection_password_with(Some(&auth), |_| Ok(None))
            .expect("password resolution");

        assert_eq!(resolved.as_deref(), Some("inline-secret"));
    }

    #[test]
    fn telnet_password_resolution_prefers_saved_password_id() {
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password_id: Some("saved-1".to_string()),
            password: Some("ignored".to_string()),
            ..ConnectionAuth::default()
        };

        let resolved = resolve_telnet_connection_password_with(Some(&auth), |id| {
            assert_eq!(id, "saved-1");
            Ok(Some("saved-secret".to_string()))
        })
        .expect("password resolution");

        assert_eq!(resolved.as_deref(), Some("saved-secret"));
    }
}

#[tauri::command]
pub async fn cancel_session_creation(
    state: tauri::State<'_, Arc<SessionManager>>,
    create_request_id: String,
) -> AppResult<bool> {
    Ok(state.cancel_session_creation(&create_request_id).await)
}

#[tauri::command]
pub fn list_serial_ports() -> AppResult<Vec<String>> {
    core::list_serial_ports()
}

#[tauri::command]
pub async fn write_to_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    data: String,
    origin: Option<InputOrigin>,
    sensitivity: Option<InputSensitivity>,
) -> AppResult<()> {
    let origin = origin.unwrap_or(InputOrigin::Keyboard);
    let sensitivity = sensitivity.unwrap_or_default();
    let automated = !matches!(origin, InputOrigin::Keyboard | InputOrigin::SyncInput);
    let result = state
        .send_command(
            &session_id,
            SessionCommand::Write {
                data: data.into_bytes(),
                automated,
                origin,
                sensitivity,
            },
        )
        .await;

    if let Err(error) = &result {
        match error {
            AppError::SessionNotFound(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    reason = "session_not_found",
                    "Terminal input rejected because SSH session is no longer active"
                );
            }
            AppError::Channel(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    reason = "command_channel_closed",
                    "Terminal input rejected because SSH session is no longer active"
                );
            }
            _ => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "Terminal input rejected"
                );
            }
        }
    }

    result
}

#[tauri::command]
pub async fn set_session_output_paused(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    paused: bool,
) -> AppResult<()> {
    let command = if paused {
        SessionCommand::PauseOutput
    } else {
        SessionCommand::ResumeOutput
    };
    state.send_command(&session_id, command).await
}

#[tauri::command]
pub async fn ack_session_output(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    bytes: usize,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::AckOutput { bytes })
        .await
}

#[tauri::command]
pub async fn zmodem_accept_download(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    save_dir: String,
) -> AppResult<()> {
    state
        .send_command(
            &session_id,
            SessionCommand::ZmodemAcceptDownload {
                save_dir: std::path::PathBuf::from(save_dir),
            },
        )
        .await
}

#[tauri::command]
pub async fn zmodem_accept_upload(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    file_paths: Vec<String>,
    conflict_mode: Option<String>,
) -> AppResult<()> {
    let transfer_settings = match config::load_app_settings(&app) {
        Ok(settings) => settings.transfer,
        Err(error) => {
            tracing::warn!(
                %error,
                "Failed to load transfer settings for ZMODEM upload; using defaults"
            );
            config::TransferSettings::default()
        }
    };

    state
        .send_command(
            &session_id,
            SessionCommand::ZmodemAcceptUpload {
                files: file_paths
                    .into_iter()
                    .map(std::path::PathBuf::from)
                    .collect(),
                conflict_mode: ZmodemUploadConflictMode::from_wire(conflict_mode.as_deref()),
                preserve_timestamps: transfer_settings.preserve_timestamps,
            },
        )
        .await
}

#[tauri::command]
pub async fn zmodem_cancel(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::ZmodemCancel)
        .await
}

#[tauri::command]
pub async fn resize_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<()> {
    state
        .send_command(&session_id, SessionCommand::Resize { cols, rows })
        .await
}

#[tauri::command]
pub async fn attach_session(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    observability::log_event(StructuredLog {
        level: StructuredLogLevel::Debug,
        domain: "session.lifecycle".to_string(),
        event: "session.attach_requested".to_string(),
        message: "Attaching session renderer".to_string(),
        ids: Some(serde_json::json!({ "session_id": session_id.clone() })),
        data: None,
        error: None,
        client_timestamp: None,
    });

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    state
        .send_command(&session_id, SessionCommand::AttachConfirmed { ack: ack_tx })
        .await?;
    ack_rx
        .await
        .map_err(|_| AppError::Channel("Session attach acknowledgement was dropped".to_string()))
}

#[tauri::command]
pub async fn detach_session_renderer(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    observability::log_event(StructuredLog {
        level: StructuredLogLevel::Debug,
        domain: "session.lifecycle".to_string(),
        event: "session.detach_renderer_requested".to_string(),
        message: "Detaching session renderer".to_string(),
        ids: Some(serde_json::json!({ "session_id": session_id.clone() })),
        data: None,
        error: None,
        client_timestamp: None,
    });

    state
        .send_command(&session_id, SessionCommand::DetachRenderer)
        .await
}

#[tauri::command]
pub async fn close_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<SessionManager>>,
    stats_sampler: tauri::State<'_, Arc<RemoteStatsSampler>>,
    session_id: String,
) -> AppResult<()> {
    let session_id_clone = session_id.clone();

    observability::log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "session.lifecycle".to_string(),
        event: "session.close_requested".to_string(),
        message: "Closing session".to_string(),
        ids: Some(serde_json::json!({ "session_id": session_id.clone() })),
        data: None,
        error: None,
        client_timestamp: None,
    });

    let res = match state.send_command(&session_id, SessionCommand::Close).await {
        Err(AppError::SessionNotFound(_)) => Ok(()),
        other => other,
    };

    stats_sampler.clear_session(&session_id).await;

    // Concurrently tidy up any downloaded/watcher temporary files stored in the OS temp directory
    tauri::async_runtime::spawn(async move {
        if let Ok(temp_dir) = app.path().temp_dir() {
            let session_temp_dir = temp_dir.join("nyaterm").join(&session_id_clone);
            if session_temp_dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&session_temp_dir).await {
                    observability::log_event(StructuredLog {
                        level: StructuredLogLevel::Warn,
                        domain: "session.lifecycle".to_string(),
                        event: "session.temp_cleanup_failed".to_string(),
                        message: "Failed to clean up session temp directory".to_string(),
                        ids: Some(serde_json::json!({ "session_id": session_id_clone })),
                        data: Some(serde_json::json!({
                            "temp_dir": session_temp_dir,
                        })),
                        error: Some(serde_json::json!({ "message": e.to_string() })),
                        client_timestamp: None,
                    });
                } else {
                    observability::log_event(StructuredLog {
                        level: StructuredLogLevel::Info,
                        domain: "session.lifecycle".to_string(),
                        event: "session.temp_cleanup_succeeded".to_string(),
                        message: "Cleaned up session temp directory".to_string(),
                        ids: Some(serde_json::json!({ "session_id": session_id_clone })),
                        data: Some(serde_json::json!({
                            "temp_dir": session_temp_dir,
                        })),
                        error: None,
                        client_timestamp: None,
                    });
                }
            }
        }
    });

    res
}

#[tauri::command]
pub async fn list_sessions(
    state: tauri::State<'_, Arc<SessionManager>>,
) -> AppResult<Vec<SessionInfo>> {
    Ok(state.list_sessions().await)
}

#[tauri::command]
pub async fn get_session_cwd_presentation(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<Option<crate::core::CwdPresentation>> {
    Ok(state.session_cwd_presentation(&session_id).await)
}

#[tauri::command]
pub async fn add_command_history(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<()> {
    state.add_command(&session_id, command).await;
    Ok(())
}

#[tauri::command]
pub async fn register_command_submission(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<()> {
    state
        .register_command_submission(&session_id, command)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn register_command_confirmation_candidate(
    state: tauri::State<'_, Arc<SessionManager>>,
    session_id: String,
    command: String,
) -> AppResult<bool> {
    Ok(state
        .register_confirmation_candidate(&session_id, command)
        .await)
}

#[tauri::command]
pub async fn get_command_history(
    state: tauri::State<'_, Arc<SessionManager>>,
) -> AppResult<Vec<String>> {
    Ok(state.get_all_history().await)
}

#[tauri::command]
pub async fn delete_command_history(
    state: tauri::State<'_, Arc<SessionManager>>,
    command: String,
) -> AppResult<()> {
    state.delete_history_command(command).await;
    Ok(())
}

#[tauri::command]
pub async fn fuzzy_search_history(
    state: tauri::State<'_, Arc<SessionManager>>,
    pattern: String,
    limit: usize,
    min_command_length: Option<usize>,
    max_command_length: Option<usize>,
) -> AppResult<Vec<FuzzyResult>> {
    Ok(state
        .fuzzy_search(&pattern, limit, min_command_length, max_command_length)
        .await)
}

#[tauri::command]
pub async fn fuzzy_search_commands(
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    pattern: String,
    limit: usize,
) -> AppResult<Vec<FuzzyResult>> {
    let cfg = state.snapshot();
    let items: Vec<(&str, &str)> = cfg
        .commands
        .iter()
        .map(|c| (c.label.as_str(), c.command.as_str()))
        .collect();
    Ok(fuzzy_search_items(
        &items,
        &pattern,
        "quickCommand",
        limit,
        None,
        None,
    ))
}

#[tauri::command]
pub async fn fuzzy_search_candidates(
    pattern: String,
    items: Vec<FuzzySearchCandidate>,
    limit: usize,
) -> AppResult<Vec<FuzzyCandidateResult>> {
    Ok(fuzzy_search_candidate_items(&items, &pattern, limit))
}

#[tauri::command]
pub async fn start_recording(
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, Arc<SessionManager>>,
    state: tauri::State<'_, Arc<RecordingManager>>,
    request: StartRecordingRequest,
) -> AppResult<String> {
    let session_info = session_manager.session_info(&request.session_id).await?;
    let (mut profile, context) =
        build_recording_profile_and_context(&app, &session_info, Some(request.mode))?;
    let explicit_path = request.explicit_path.map(PathBuf::from);
    if explicit_path.is_some() {
        profile.existing_file_behavior = ExistingFileBehavior::Unique;
    }
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        mgr.start_with_profile(&request.session_id, context, profile, explicit_path)
    })
    .await
    .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn stop_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
) -> AppResult<String> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || mgr.stop(&session_id))
        .await
        .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn is_recording(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
) -> AppResult<bool> {
    Ok(state.is_recording(&session_id))
}

#[tauri::command]
pub async fn save_session_transcript(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
    file_path: String,
    include_io_labels: bool,
    include_timestamps: bool,
) -> AppResult<String> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        mgr.save_transcript(
            &session_id,
            &file_path,
            include_io_labels,
            include_timestamps,
        )
    })
    .await
    .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn terminal_history_search(
    state: tauri::State<'_, Arc<RecordingManager>>,
    request: TerminalHistorySearchRequest,
) -> AppResult<TerminalHistorySearchResponse> {
    let mgr = state.inner().clone();
    tokio::task::spawn_blocking(move || mgr.search_history(request))
        .await
        .map_err(|e| AppError::Config(format!("Task join error: {e}")))?
}

#[tauri::command]
pub async fn list_recording_sessions(
    state: tauri::State<'_, Arc<RecordingManager>>,
) -> AppResult<Vec<String>> {
    Ok(state.list_recording_sessions())
}

#[tauri::command]
pub async fn get_recording_status(
    state: tauri::State<'_, Arc<RecordingManager>>,
    session_id: String,
) -> AppResult<Option<core::RecordingStatus>> {
    Ok(state.get_recording_status(&session_id))
}

#[tauri::command]
pub async fn list_recording_statuses(
    state: tauri::State<'_, Arc<RecordingManager>>,
) -> AppResult<Vec<core::RecordingStatus>> {
    Ok(state.list_recording_statuses())
}

#[tauri::command]
pub async fn open_recording_file(file_path: String) -> AppResult<()> {
    open::that(file_path)
        .map_err(|error| AppError::Config(format!("Failed to open recording file: {error}")))
}

#[tauri::command]
pub async fn show_recording_in_folder(file_path: String) -> AppResult<()> {
    let path = PathBuf::from(file_path);
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    open::that(dir)
        .map_err(|error| AppError::Config(format!("Failed to show recording folder: {error}")))
}

#[tauri::command]
pub async fn set_recording_memory_limit(
    state: tauri::State<'_, Arc<RecordingManager>>,
    max_bytes: usize,
) -> AppResult<()> {
    state.set_memory_limit(max_bytes);
    Ok(())
}

#[tauri::command]
pub async fn submit_otp_response(
    state: tauri::State<'_, Arc<PendingAuthManager>>,
    request_id: String,
    responses: Vec<String>,
) -> AppResult<()> {
    if state.respond(&request_id, Some(responses)).await {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Info,
            domain: "security.flow".to_string(),
            event: "otp.response_received".to_string(),
            message: "Received OTP response from frontend".to_string(),
            ids: Some(serde_json::json!({ "request_id": request_id })),
            data: None,
            error: None,
            client_timestamp: None,
        });
        Ok(())
    } else {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Warn,
            domain: "security.flow".to_string(),
            event: "otp.response_rejected".to_string(),
            message: "Rejected OTP response for missing request".to_string(),
            ids: Some(serde_json::json!({ "request_id": request_id.clone() })),
            data: None,
            error: None,
            client_timestamp: None,
        });
        Err(AppError::Auth(format!(
            "No pending OTP request with id '{}'",
            request_id
        )))
    }
}

#[tauri::command]
pub async fn cancel_otp_request(
    state: tauri::State<'_, Arc<PendingAuthManager>>,
    request_id: String,
) -> AppResult<()> {
    let cancelled = state.respond(&request_id, None).await;
    observability::log_event(StructuredLog {
        level: if cancelled {
            StructuredLogLevel::Info
        } else {
            StructuredLogLevel::Warn
        },
        domain: "security.flow".to_string(),
        event: if cancelled {
            "otp.request_cancelled".to_string()
        } else {
            "otp.request_cancel_missing".to_string()
        },
        message: if cancelled {
            "Cancelled OTP request".to_string()
        } else {
            "OTP request was already missing when cancellation arrived".to_string()
        },
        ids: Some(serde_json::json!({ "request_id": request_id })),
        data: None,
        error: None,
        client_timestamp: None,
    });
    Ok(())
}

#[tauri::command]
pub async fn submit_ssh_auth_response(
    state: tauri::State<'_, Arc<PendingSshAuthManager>>,
    request_id: String,
    response: SshAuthResponse,
) -> AppResult<()> {
    if state.respond(&request_id, Some(response)).await {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Info,
            domain: "security.flow".to_string(),
            event: "ssh_auth.response_received".to_string(),
            message: "Received SSH credential response from frontend".to_string(),
            ids: Some(serde_json::json!({ "request_id": request_id })),
            data: None,
            error: None,
            client_timestamp: None,
        });
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending SSH authentication request with id '{}'",
            request_id
        )))
    }
}

#[tauri::command]
pub async fn cancel_ssh_auth_request(
    state: tauri::State<'_, Arc<PendingSshAuthManager>>,
    request_id: String,
) -> AppResult<()> {
    let cancelled = state.respond(&request_id, None).await;
    observability::log_event(StructuredLog {
        level: if cancelled {
            StructuredLogLevel::Info
        } else {
            StructuredLogLevel::Warn
        },
        domain: "security.flow".to_string(),
        event: if cancelled {
            "ssh_auth.request_cancelled".to_string()
        } else {
            "ssh_auth.request_cancel_missing".to_string()
        },
        message: if cancelled {
            "Cancelled SSH credential request".to_string()
        } else {
            "SSH credential request was already missing when cancellation arrived".to_string()
        },
        ids: Some(serde_json::json!({ "request_id": request_id })),
        data: None,
        error: None,
        client_timestamp: None,
    });
    Ok(())
}

#[tauri::command]
pub async fn respond_ssh_agent_auth(
    state: tauri::State<'_, Arc<PendingSshAgentAuthManager>>,
    request_id: String,
    action: String,
) -> AppResult<()> {
    let action = match action.as_str() {
        "retry" => SshAgentAuthAction::Retry,
        "cancel" => SshAgentAuthAction::Cancel,
        _ => {
            return Err(AppError::Config(
                "Unknown SSH Agent auth action".to_string(),
            ));
        }
    };
    if state.respond(&request_id, action).await {
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending SSH Agent authentication request with id '{}'",
            request_id
        )))
    }
}

#[tauri::command]
pub async fn cancel_ssh_agent_auth(
    state: tauri::State<'_, Arc<PendingSshAgentAuthManager>>,
    request_id: String,
) -> AppResult<()> {
    if state.respond(&request_id, SshAgentAuthAction::Cancel).await {
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending SSH Agent authentication request with id '{}'",
            request_id
        )))
    }
}

#[tauri::command]
pub async fn respond_host_key_verify(
    state: tauri::State<'_, Arc<HostKeyVerifyManager>>,
    request_id: String,
    accepted: bool,
) -> AppResult<()> {
    let resolved = state.respond(&request_id, accepted).await;
    observability::log_event(StructuredLog {
        level: if resolved {
            StructuredLogLevel::Info
        } else {
            StructuredLogLevel::Warn
        },
        domain: "security.flow".to_string(),
        event: if accepted {
            "host_key.accepted".to_string()
        } else {
            "host_key.rejected".to_string()
        },
        message: if resolved {
            format!(
                "Host key verification response received (accepted={})",
                accepted
            )
        } else {
            "Host key verification response for missing request".to_string()
        },
        ids: Some(serde_json::json!({ "request_id": request_id })),
        data: None,
        error: None,
        client_timestamp: None,
    });
    if resolved {
        Ok(())
    } else {
        Err(AppError::Auth(format!(
            "No pending host key verification with id '{}'",
            request_id
        )))
    }
}

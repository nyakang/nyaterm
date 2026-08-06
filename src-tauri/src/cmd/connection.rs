use crate::config::{
    self, AssetAccelerator, AssetAcceleratorType, AssetMetadata, Group, QuickCommandsConfig,
    SavedConnection, SavedPassword, SshKey,
};
use crate::core::{QuickCommandsImportResult, QuickCommandsImportSource, QuickCommandsStore};
use crate::error::{AppError, AppResult};
use crate::utils::crypto;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tauri::Emitter;

fn schedule_cloud_sync_notify(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app).await;
    });
}

#[tauri::command]
pub fn get_saved_connections(app: tauri::AppHandle) -> AppResult<Vec<SavedConnection>> {
    let cfg = config::load_config(&app)?;
    let mut connections = cfg.connections;
    for conn in &mut connections {
        if let Some(ref mut auth) = conn.auth {
            auth.has_password = auth.password.is_some();
            auth.password = None;
        }
    }
    Ok(connections)
}

#[tauri::command]
pub fn get_supported_ssh_algorithms() -> crate::core::ssh::SupportedSshAlgorithms {
    crate::core::ssh::get_supported_ssh_algorithms()
}

#[tauri::command]
pub fn save_connection(
    app: tauri::AppHandle,
    mut connection: SavedConnection,
) -> AppResult<String> {
    let mut cfg = config::load_config(&app)?;

    if connection.id.is_empty() {
        connection.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = connection.id.clone();
    let existing = cfg.connections.iter().find(|c| c.id == target_id).cloned();

    validate_proxy_jump_config(&connection, &cfg.connections)?;
    validate_local_terminal_config(&connection)?;
    validate_ssh_algorithm_config(&connection)?;
    validate_sftp_settings_config(&connection)?;

    if let Some(ref mut auth) = connection.auth {
        // password_id: Some("") means explicitly cleared, None means preserve existing
        match auth.password_id.as_deref() {
            Some("") => auth.password_id = None,
            None => {
                auth.password_id = existing
                    .as_ref()
                    .and_then(|e| e.auth.as_ref())
                    .and_then(|a| a.password_id.clone());
            }
            _ => {}
        }

        // password: non-empty = encrypt new value, "" = explicitly clear, None = preserve
        auth.password = match auth.password.as_deref() {
            Some(plain) if !plain.is_empty() => Some(crypto::encrypt(plain)?),
            Some("") => None,
            None => existing
                .as_ref()
                .and_then(|e| e.auth.as_ref())
                .and_then(|a| a.password.clone()),
            _ => None,
        };
        auth.has_password = false;
    }

    if let Some(existing_connection) = existing.as_ref() {
        connection.asset = existing_connection.asset.clone();
    }

    if let Some(ex) = cfg.connections.iter_mut().find(|c| c.id == target_id) {
        *ex = connection;
    } else {
        cfg.connections.push(connection);
    }
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(target_id)
}

fn update_connection_icon_in_config(
    cfg: &mut config::AppConfig,
    connection_id: &str,
    icon: Option<String>,
    icon_auto_detect: bool,
) -> AppResult<bool> {
    let connection = cfg
        .connections
        .iter_mut()
        .find(|connection| connection.id == connection_id)
        .ok_or_else(|| {
            AppError::SessionNotFound(format!("Connection '{}' not found", connection_id))
        })?;

    let normalized_icon = icon.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let next_auto_detect = Some(icon_auto_detect);

    if connection.icon == normalized_icon && connection.icon_auto_detect == next_auto_detect {
        return Ok(false);
    }

    connection.icon = normalized_icon;
    connection.icon_auto_detect = next_auto_detect;
    Ok(true)
}

fn update_connection_asset_from_monitoring_in_config(
    cfg: &mut config::AppConfig,
    connection_id: &str,
    asset_patch: AssetMetadata,
) -> AppResult<bool> {
    let connection = cfg
        .connections
        .iter_mut()
        .find(|connection| connection.id == connection_id)
        .ok_or_else(|| {
            AppError::SessionNotFound(format!("Connection '{}' not found", connection_id))
        })?;

    let mut next_asset = connection.asset.clone().unwrap_or_default();
    merge_monitoring_asset_patch(&mut next_asset, asset_patch);

    if connection.asset.as_ref() == Some(&next_asset) {
        return Ok(false);
    }

    connection.asset = Some(next_asset);
    Ok(true)
}

fn merge_monitoring_asset_patch(target: &mut AssetMetadata, patch: AssetMetadata) {
    if patch.hostname.is_some() {
        target.hostname = patch.hostname;
    }
    if patch.os_name.is_some() {
        target.os_name = patch.os_name;
    }
    if patch.architecture.is_some() {
        target.architecture = patch.architecture;
    }
    if patch.cpu_model.is_some() {
        target.cpu_model = patch.cpu_model;
    }
    if patch.cpu_cores.is_some() {
        target.cpu_cores = patch.cpu_cores;
    }
    if patch.memory_bytes.is_some() {
        target.memory_bytes = patch.memory_bytes;
    }
    if patch.disks.is_some() {
        target.disks = patch.disks;
    }
    if patch.updated_at.is_some() {
        target.updated_at = patch.updated_at;
    }
    if let Some(accelerators) = patch.accelerators {
        target.accelerators = Some(merge_monitoring_accelerators(
            target.accelerators.clone(),
            accelerators,
        ));
    }
}

fn merge_monitoring_accelerators(
    current: Option<Vec<AssetAccelerator>>,
    patch: Vec<AssetAccelerator>,
) -> Vec<AssetAccelerator> {
    if patch.is_empty() {
        return current.unwrap_or_default();
    }

    let patch_types = patch
        .iter()
        .map(|accelerator| accelerator.r#type.clone())
        .collect::<Vec<AssetAcceleratorType>>();
    let mut merged = current.unwrap_or_default();
    merged.retain(|accelerator| {
        !patch_types
            .iter()
            .any(|patch_type| patch_type == &accelerator.r#type)
    });
    merged.extend(patch);
    merged
}

#[tauri::command]
pub fn update_connection_asset_from_monitoring(
    app: tauri::AppHandle,
    connection_id: String,
    asset_patch: AssetMetadata,
) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    let changed =
        update_connection_asset_from_monitoring_in_config(&mut cfg, &connection_id, asset_patch)?;

    if !changed {
        return Ok(());
    }

    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn update_connection_icon(
    app: tauri::AppHandle,
    connection_id: String,
    icon: Option<String>,
    icon_auto_detect: bool,
) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    let changed =
        update_connection_icon_in_config(&mut cfg, &connection_id, icon, icon_auto_detect)?;

    if !changed {
        return Ok(());
    }

    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

fn validate_ssh_algorithm_config(connection: &SavedConnection) -> AppResult<()> {
    if !matches!(connection.config, config::ConnectionType::Ssh { .. }) {
        return Ok(());
    }

    let Some(preferences) = connection.ssh_algorithms.as_ref() else {
        return Ok(());
    };

    crate::core::ssh::validate_ssh_algorithm_preferences(preferences)
}

fn validate_sftp_settings_config(connection: &SavedConnection) -> AppResult<()> {
    if !matches!(connection.config, config::ConnectionType::Ssh { .. }) {
        return Ok(());
    }

    let timeout_ms = connection.sftp.shell_detection_timeout_ms;
    if !(config::MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS..=config::MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS)
        .contains(&timeout_ms)
    {
        return Err(AppError::Config(format!(
            "SFTP shell detection timeout must be between {} and {} ms",
            config::MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS,
            config::MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS
        )));
    }

    Ok(())
}

fn validate_local_terminal_config(connection: &SavedConnection) -> AppResult<()> {
    let config::ConnectionType::LocalTerminal {
        shell_path,
        shell_args,
        ..
    } = &connection.config
    else {
        return Ok(());
    };

    let trimmed = shell_path.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("Shell path is required".to_string()));
    }

    let path = Path::new(trim_wrapping_quotes(trimmed));
    if should_validate_shell_path(trimmed) {
        let metadata = std::fs::metadata(path)
            .map_err(|e| AppError::Config(format!("Shell path is not a valid file: {e}")))?;
        if metadata.is_dir() {
            return Err(AppError::Config(
                "Shell path must be a file, not a directory".to_string(),
            ));
        }
    }

    crate::core::terminal_session::local::parse_shell_args(shell_args).map_err(AppError::Config)?;

    Ok(())
}

fn should_validate_shell_path(value: &str) -> bool {
    let path = Path::new(trim_wrapping_quotes(value));
    path.is_absolute() || value.contains('\\') || value.contains('/')
}

fn trim_wrapping_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn resolve_text_secret_input(
    inline_value: Option<&str>,
    file_path: Option<&str>,
    file_error_label: &str,
) -> AppResult<Option<String>> {
    if let Some(value) = inline_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(value.to_string()));
    }

    let Some(path) = file_path.map(str::trim).filter(|path| !path.is_empty()) else {
        return Ok(None);
    };

    std::fs::read_to_string(trim_wrapping_quotes(path))
        .map(Some)
        .map_err(|e| AppError::Config(format!("failed to read {file_error_label} file: {e}")))
}

fn missing_private_key_passphrase_error(error: &russh::keys::Error) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("encrypted")
        || message.contains("passphrase")
        || message.contains("password")
        || message.contains("cipher")
}

fn validate_private_key_content(content: &str, passphrase: Option<&str>) -> AppResult<()> {
    let usable_passphrase = passphrase.filter(|value| !value.is_empty());
    match russh::keys::decode_secret_key(content, usable_passphrase) {
        Ok(_) => Ok(()),
        Err(error)
            if usable_passphrase.is_none() && missing_private_key_passphrase_error(&error) =>
        {
            Ok(())
        }
        Err(error) => Err(AppError::Config(format!(
            "invalid SSH private key: {error}"
        ))),
    }
}

fn validate_certificate_content(content: &str) -> AppResult<()> {
    russh::keys::Certificate::from_openssh(content)
        .map(|_| ())
        .map_err(|error| AppError::Config(format!("invalid OpenSSH certificate: {error}")))
}

fn resolve_private_key_for_save(
    key: &SshKey,
    existing: Option<&SshKey>,
) -> AppResult<Option<String>> {
    let Some(content) =
        resolve_text_secret_input(key.key_data.as_deref(), key.key_file_path.as_deref(), "key")?
    else {
        return Ok(existing.and_then(|e| e.key.clone()));
    };

    validate_private_key_content(&content, key.passphrase.as_deref())?;
    crypto::encrypt(&content).map(Some)
}

fn resolve_certificate_for_save(
    key: &SshKey,
    existing: Option<&SshKey>,
) -> AppResult<Option<String>> {
    let Some(content) = resolve_text_secret_input(
        key.cert_data.as_deref(),
        key.cert_file_path.as_deref(),
        "certificate",
    )?
    else {
        return Ok(existing.and_then(|e| e.cert.clone()));
    };

    validate_certificate_content(&content)?;
    crypto::encrypt(&content).map(Some)
}

fn validate_proxy_jump_config(
    connection: &SavedConnection,
    existing_connections: &[SavedConnection],
) -> AppResult<()> {
    let proxy_jump_id = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref());

    let Some(proxy_jump_id) = proxy_jump_id else {
        return Ok(());
    };

    if !matches!(connection.config, config::ConnectionType::Ssh { .. }) {
        return Err(AppError::Config(
            "ProxyJump is only supported for SSH connections".to_string(),
        ));
    }

    let mut visited = HashSet::new();
    visited.insert(connection.id.as_str());
    let mut current_jump_id = proxy_jump_id;

    loop {
        if !visited.insert(current_jump_id) {
            if connection.id == current_jump_id {
                return Err(AppError::Config(
                    "A connection cannot use itself as a jump host".to_string(),
                ));
            }
            return Err(AppError::Config(format!(
                "ProxyJump chain contains a cycle at '{}'",
                current_jump_id
            )));
        }

        let jump_connection =
            find_connection_for_proxy_jump(connection, existing_connections, current_jump_id)
                .ok_or_else(|| {
                    AppError::Config(format!("Jump host '{}' not found", current_jump_id))
                })?;

        if !matches!(jump_connection.config, config::ConnectionType::Ssh { .. }) {
            return Err(AppError::Config(
                "Only SSH connections can be used as jump hosts".to_string(),
            ));
        }

        let Some(next_jump_id) = jump_connection
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.as_deref())
        else {
            break;
        };

        current_jump_id = next_jump_id;
    }

    Ok(())
}

fn find_connection_for_proxy_jump<'a>(
    edited_connection: &'a SavedConnection,
    existing_connections: &'a [SavedConnection],
    id: &str,
) -> Option<&'a SavedConnection> {
    if edited_connection.id == id {
        return Some(edited_connection);
    }

    existing_connections
        .iter()
        .find(|candidate| candidate.id == id)
}

#[cfg(test)]
mod tests {
    use super::{
        delete_group_from_config, resolve_private_key_for_save, resolve_text_secret_input,
        update_connection_asset_from_monitoring_in_config, update_connection_icon_in_config,
        validate_certificate_content, validate_local_terminal_config, validate_private_key_content,
        validate_proxy_jump_config, validate_sftp_settings_config,
    };
    use crate::config::{
        AiExecutionProfile, AssetAccelerator, AssetAcceleratorType, AssetDisk, AssetDiskPurpose,
        AssetMetadata, ConnectionAuth, ConnectionNetwork, ConnectionType, Group, SavedConnection,
        SessionsConfig, SftpSettings, SshKey,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEINTuctv5E1hK1bbY8fdp+K06/nwoy/HU++CXqI9EdVhC
-----END PRIVATE KEY-----";

    const TEST_ENCRYPTED_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jYmMAAAAGYmNyeXB0AAAAGAAAABDLGyfA39
J2FcJygtYqi5ISAAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIN+Wjn4+4Fcvl2Jl
KpggT+wCRxpSvtqqpVrQrKN1/A22AAAAkOHDLnYZvYS6H9Q3S3Nk4ri3R2jAZlQlBbUos5
FkHpYgNw65KCWCTXtP7ye2czMC3zjn2r98pJLobsLYQgRiHIv/CUdAdsqbvMPECB+wl/UQ
e+JpiSq66Z6GIt0801skPh20jxOO3F52SoX1IeO5D5PXfZrfSZlw6S8c7bwyp2FHxDewRx
7/wNsnDM0T7nLv/Q==
-----END OPENSSH PRIVATE KEY-----";

    fn ssh_connection(id: &str, proxy_jump_id: Option<&str>) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: format!("SSH {id}"),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                x11_forwarding: false,
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            network: proxy_jump_id.map(|jump_id| ConnectionNetwork {
                proxy_id: None,
                proxy_jump_id: Some(jump_id.to_string()),
            }),
            post_login: None,
            ssh_algorithms: None,
            sftp: SftpSettings::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn telnet_connection(id: &str, proxy_jump_id: Option<&str>) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: format!("Telnet {id}"),
            config: ConnectionType::Telnet {
                host: "example.com".to_string(),
                port: 23,
                username: String::new(),
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: "del".to_string(),
                raw_tcp_cli: false,
                enter_mode: "cr".to_string(),
                local_echo: false,
                local_line_edit: false,
                force_character_at_a_time: false,
                send_naws: true,
                send_sga: true,
                auto_login: Default::default(),
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            network: proxy_jump_id.map(|jump_id| ConnectionNetwork {
                proxy_id: None,
                proxy_jump_id: Some(jump_id.to_string()),
            }),
            post_login: None,
            ssh_algorithms: None,
            sftp: SftpSettings::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn local_terminal_connection(id: &str, shell_path: String) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: format!("Local {id}"),
            config: ConnectionType::LocalTerminal {
                shell_path,
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: AiExecutionProfile::Auto,
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            network: None,
            post_login: None,
            ssh_algorithms: None,
            sftp: SftpSettings::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn group(id: &str, parent_id: Option<&str>) -> Group {
        Group {
            id: id.to_string(),
            name: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    fn grouped_connection(id: &str, group_id: Option<&str>) -> SavedConnection {
        let mut connection = ssh_connection(id, None);
        connection.group_id = group_id.map(str::to_string);
        connection
    }

    #[test]
    fn validates_sftp_shell_detection_timeout_range() {
        let mut connection = ssh_connection("conn-1", None);
        connection.sftp.shell_detection_timeout_ms = 3000;
        assert!(validate_sftp_settings_config(&connection).is_ok());

        connection.sftp.shell_detection_timeout_ms = 0;
        let error = validate_sftp_settings_config(&connection).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SFTP shell detection timeout must be between")
        );

        connection.sftp.shell_detection_timeout_ms = 60_001;
        let error = validate_sftp_settings_config(&connection).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SFTP shell detection timeout must be between")
        );
    }

    fn ssh_key_with_sources(
        id: &str,
        key_data: Option<&str>,
        key_file_path: Option<&str>,
    ) -> SshKey {
        SshKey {
            id: id.to_string(),
            name: format!("Key {id}"),
            key: None,
            cert: None,
            passphrase: None,
            key_data: key_data.map(str::to_string),
            cert_data: None,
            key_file_path: key_file_path.map(str::to_string),
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        }
    }

    fn create_test_certificate() -> ssh_key::Certificate {
        use ssh_key::certificate;

        let mut rng = rand::thread_rng();
        let ca_key = ssh_key::PrivateKey::random(&mut rng, ssh_key::Algorithm::Ed25519).unwrap();
        let user_key = ssh_key::PrivateKey::random(&mut rng, ssh_key::Algorithm::Ed25519).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut builder = certificate::Builder::new_with_random_nonce(
            &mut rng,
            user_key.public_key(),
            now.saturating_sub(3600),
            now + 86400,
        )
        .unwrap();
        builder.serial(1).unwrap();
        builder.key_id("test-cert").unwrap();
        builder.cert_type(certificate::CertType::User).unwrap();
        builder.valid_principal("testuser").unwrap();
        builder.sign(&ca_key).unwrap()
    }

    #[test]
    fn key_data_takes_priority_over_key_file_path() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nyaterm-key-source-{nanos}.pem"));
        fs::write(&path, "file content").expect("write test key file");

        let content = resolve_text_secret_input(
            Some(" inline content "),
            Some(&path.to_string_lossy()),
            "key",
        )
        .expect("resolve key input");

        assert_eq!(content.as_deref(), Some("inline content"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn key_file_path_is_still_supported() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nyaterm-key-file-{nanos}.pem"));
        fs::write(&path, TEST_PRIVATE_KEY).expect("write test key file");

        let content = resolve_text_secret_input(None, Some(&path.to_string_lossy()), "key")
            .expect("resolve key file input");

        assert_eq!(content.as_deref(), Some(TEST_PRIVATE_KEY));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn existing_key_data_is_preserved_without_new_source() {
        let mut existing = ssh_key_with_sources("existing", None, None);
        existing.key = Some("encrypted-existing-key".to_string());
        let edited = ssh_key_with_sources("existing", Some("   "), Some("   "));

        let resolved =
            resolve_private_key_for_save(&edited, Some(&existing)).expect("preserve old key");

        assert_eq!(resolved.as_deref(), Some("encrypted-existing-key"));
    }

    #[test]
    fn valid_private_key_data_is_accepted() {
        validate_private_key_content(TEST_PRIVATE_KEY, None).expect("valid private key");
    }

    #[test]
    fn encrypted_private_key_without_passphrase_is_accepted() {
        validate_private_key_content(TEST_ENCRYPTED_PRIVATE_KEY, None)
            .expect("encrypted private key can be saved without passphrase");
    }

    #[test]
    fn invalid_private_key_data_is_rejected() {
        let error = validate_private_key_content("not a private key", None).unwrap_err();

        assert!(error.to_string().contains("invalid SSH private key"));
    }

    #[test]
    fn valid_certificate_data_is_accepted() {
        let cert = create_test_certificate()
            .to_openssh()
            .expect("render test certificate");

        validate_certificate_content(&cert).expect("valid certificate");
    }

    #[test]
    fn invalid_certificate_data_is_rejected() {
        let error = validate_certificate_content("not a certificate").unwrap_err();

        assert!(error.to_string().contains("invalid OpenSSH certificate"));
    }

    #[test]
    fn rejects_proxy_jump_on_non_ssh_connections() {
        let connection = telnet_connection("telnet-1", Some("jump-1"));
        let jump = ssh_connection("jump-1", None);

        let error = validate_proxy_jump_config(&connection, &[jump]).unwrap_err();

        assert!(error.to_string().contains("ProxyJump is only supported"));
    }

    #[test]
    fn rejects_self_reference() {
        let connection = ssh_connection("self", Some("self"));

        let error = validate_proxy_jump_config(&connection, &[]).unwrap_err();

        assert!(error.to_string().contains("cannot use itself"));
    }

    #[test]
    fn rejects_non_ssh_jump_hosts() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = telnet_connection("jump", None);

        let error = validate_proxy_jump_config(&connection, &[jump]).unwrap_err();

        assert!(error.to_string().contains("Only SSH connections"));
    }

    #[test]
    fn accepts_multi_hop_ssh_jump_hosts() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", Some("another"));
        let another = ssh_connection("another", None);

        validate_proxy_jump_config(&connection, &[jump, another]).unwrap();
    }

    #[test]
    fn rejects_missing_jump_in_multi_hop_chain() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", Some("missing"));

        let error = validate_proxy_jump_config(&connection, &[jump]).unwrap_err();

        assert!(error.to_string().contains("Jump host 'missing' not found"));
    }

    #[test]
    fn rejects_indirect_proxy_jump_cycles() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", Some("another"));
        let another = ssh_connection("another", Some("target"));

        let error = validate_proxy_jump_config(&connection, &[jump, another]).unwrap_err();

        assert!(error.to_string().contains("cannot use itself"));
    }

    #[test]
    fn rejects_cycles_between_jump_hosts() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", Some("another"));
        let another = ssh_connection("another", Some("jump"));

        let error = validate_proxy_jump_config(&connection, &[jump, another]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("ProxyJump chain contains a cycle")
        );
    }

    #[test]
    fn validates_proxy_jump_against_edited_connection() {
        let connection = ssh_connection("jump", Some("another"));
        let stale_connection = ssh_connection("jump", None);
        let another = ssh_connection("another", Some("jump"));

        let error =
            validate_proxy_jump_config(&connection, &[stale_connection, another]).unwrap_err();

        assert!(error.to_string().contains("cannot use itself"));
    }

    #[test]
    fn accepts_single_hop_ssh_jump_hosts() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", None);

        validate_proxy_jump_config(&connection, &[jump]).unwrap();
    }

    #[test]
    fn rejects_directory_as_local_terminal_shell_path() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nyaterm-shell-dir-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        let connection = local_terminal_connection("local-dir", dir.to_string_lossy().to_string());

        let error = validate_local_terminal_config(&connection).unwrap_err();

        assert!(error.to_string().contains("must be a file"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_group_removes_descendant_groups_and_contained_connections() {
        let mut config = SessionsConfig {
            groups: vec![
                group("root", None),
                group("child", Some("root")),
                group("sibling", None),
            ],
            connections: vec![
                grouped_connection("root-connection", Some("root")),
                grouped_connection("child-connection", Some("child")),
                grouped_connection("sibling-connection", Some("sibling")),
                grouped_connection("ungrouped-connection", None),
            ],
        };

        delete_group_from_config(&mut config, "root");

        let group_ids = config
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect::<Vec<_>>();
        let connection_ids = config
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(group_ids, vec!["sibling"]);
        assert_eq!(
            connection_ids,
            vec!["sibling-connection", "ungrouped-connection"]
        );
    }

    #[test]
    fn update_connection_icon_changes_only_icon_fields() {
        let mut config = SessionsConfig {
            groups: vec![],
            connections: vec![ssh_connection("target", None)],
        };

        let changed =
            update_connection_icon_in_config(&mut config, "target", Some(" ubuntu ".into()), true)
                .unwrap();

        assert!(changed);
        assert_eq!(config.connections[0].icon.as_deref(), Some("ubuntu"));
        assert_eq!(config.connections[0].icon_auto_detect, Some(true));

        let unchanged =
            update_connection_icon_in_config(&mut config, "target", Some("ubuntu".into()), true)
                .unwrap();

        assert!(!unchanged);
    }

    #[test]
    fn update_connection_asset_from_monitoring_changes_only_asset_fields() {
        let mut target = ssh_connection("target", Some("proxy"));
        target.group_id = Some("group".to_string());
        target.description = Some("keep description".to_string());
        target.sort_order = 42;
        target.auth = Some(ConnectionAuth {
            mode: "key".to_string(),
            password_id: Some("password-id".to_string()),
            password: Some("encrypted".to_string()),
            has_password: true,
            key_id: Some("key-id".to_string()),
            otp_id: Some("otp-id".to_string()),
            auto_fill_otp: true,
        });
        target.asset = Some(AssetMetadata {
            hostname: Some("old-host".to_string()),
            os_name: Some("Debian".to_string()),
            architecture: Some("x86_64".to_string()),
            cpu_model: Some("Old CPU".to_string()),
            cpu_cores: Some(8),
            memory_bytes: Some(8 * 1024 * 1024 * 1024),
            accelerators: Some(vec![
                AssetAccelerator {
                    r#type: AssetAcceleratorType::Gpu,
                    vendor: Some("NVIDIA".to_string()),
                    model: Some("A100".to_string()),
                    count: Some(2),
                    memory_bytes: Some(40 * 1024 * 1024 * 1024),
                },
                AssetAccelerator {
                    r#type: AssetAcceleratorType::Npu,
                    vendor: Some("Huawei".to_string()),
                    model: Some("Ascend 910B".to_string()),
                    count: Some(1),
                    memory_bytes: Some(32 * 1024 * 1024 * 1024),
                },
            ]),
            disks: Some(vec![AssetDisk {
                kind: None,
                model: Some("/dev/sda".to_string()),
                capacity_bytes: Some(100 * 1024 * 1024 * 1024),
                count: Some(1),
                purpose: Some(AssetDiskPurpose::System),
            }]),
            tags: Some(vec!["preserve".to_string()]),
            notes: Some("preserve notes".to_string()),
            updated_at: Some("2026-08-03T01:00:00.000Z".to_string()),
            ..Default::default()
        });
        let original = target.clone();
        let mut config = SessionsConfig {
            groups: vec![group("group", None)],
            connections: vec![target, ssh_connection("other", None)],
        };

        let changed = update_connection_asset_from_monitoring_in_config(
            &mut config,
            "target",
            AssetMetadata {
                hostname: Some("new-host".to_string()),
                os_name: Some("Ubuntu".to_string()),
                cpu_model: Some("New CPU".to_string()),
                cpu_cores: Some(16),
                memory_bytes: Some(64 * 1024 * 1024 * 1024),
                accelerators: Some(vec![AssetAccelerator {
                    r#type: AssetAcceleratorType::Gpu,
                    vendor: Some("NVIDIA".to_string()),
                    model: Some("H100".to_string()),
                    count: Some(4),
                    memory_bytes: Some(80 * 1024 * 1024 * 1024),
                }]),
                tags: Some(vec!["ignored".to_string()]),
                notes: Some("ignored".to_string()),
                updated_at: Some("2026-08-03T02:00:00.000Z".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(changed);
        let updated = &config.connections[0];
        assert_eq!(updated.name, original.name);
        assert_eq!(updated.config, original.config);
        assert_eq!(updated.group_id, original.group_id);
        assert_eq!(updated.description, original.description);
        assert_eq!(updated.sort_order, original.sort_order);
        assert_eq!(
            updated.auth.as_ref().unwrap().key_id,
            Some("key-id".to_string())
        );
        assert_eq!(
            updated
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref()),
            original
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref())
        );

        let asset = updated.asset.as_ref().expect("asset");
        assert_eq!(asset.hostname.as_deref(), Some("new-host"));
        assert_eq!(asset.os_name.as_deref(), Some("Ubuntu"));
        assert_eq!(asset.cpu_model.as_deref(), Some("New CPU"));
        assert_eq!(asset.cpu_cores, Some(16));
        assert_eq!(asset.memory_bytes, Some(64 * 1024 * 1024 * 1024));
        assert_eq!(asset.tags.as_deref(), Some(&["preserve".to_string()][..]));
        assert_eq!(asset.notes.as_deref(), Some("preserve notes"));
        assert_eq!(
            asset.updated_at.as_deref(),
            Some("2026-08-03T02:00:00.000Z")
        );

        let accelerators = asset.accelerators.as_ref().expect("accelerators");
        assert_eq!(accelerators.len(), 2);
        assert!(accelerators.iter().any(|accelerator| accelerator.r#type
            == AssetAcceleratorType::Gpu
            && accelerator.model.as_deref() == Some("H100")));
        assert!(accelerators.iter().any(|accelerator| accelerator.r#type
            == AssetAcceleratorType::Npu
            && accelerator.model.as_deref() == Some("Ascend 910B")));
        assert!(config.connections[1].asset.is_none());
    }
}

#[tauri::command]
pub fn delete_connection(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    cfg.connections.retain(|c| c.id != id);
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn get_connection_password_value(
    app: tauri::AppHandle,
    id: String,
) -> AppResult<Option<String>> {
    let connection = config::load_connection_by_id(&app, &id)?;
    let Some(auth) = connection.auth else {
        return Ok(None);
    };

    crypto::decrypt_optional(&auth.password)
}

#[derive(serde::Deserialize)]
pub struct SortOrderUpdate {
    pub id: String,
    pub sort_order: i32,
}

#[tauri::command]
pub fn reorder_items(
    app: tauri::AppHandle,
    connections: Vec<SortOrderUpdate>,
    groups: Vec<SortOrderUpdate>,
) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    for update in &connections {
        if let Some(conn) = cfg.connections.iter_mut().find(|c| c.id == update.id) {
            conn.sort_order = update.sort_order;
        }
    }
    for update in &groups {
        if let Some(grp) = cfg.groups.iter_mut().find(|g| g.id == update.id) {
            grp.sort_order = update.sort_order;
        }
    }
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn get_ssh_keys(app: tauri::AppHandle) -> AppResult<Vec<SshKey>> {
    let mut cfg = config::load_keys(&app)?;
    for k in &mut cfg.keys {
        k.key = None;
        k.cert = None;
        k.passphrase = None;
    }
    Ok(cfg.keys)
}

#[tauri::command]
pub fn get_ssh_key_passphrase(app: tauri::AppHandle, id: String) -> AppResult<Option<String>> {
    Ok(config::load_key_by_id(&app, &id)?.passphrase)
}

#[tauri::command]
pub fn get_ssh_key_private_key(app: tauri::AppHandle, id: String) -> AppResult<Option<String>> {
    let key = config::load_key_by_id(&app, &id)?;
    config::decrypt_key_pem(&key)
}

#[tauri::command]
pub fn save_ssh_key(app: tauri::AppHandle, mut key: SshKey) -> AppResult<String> {
    let mut cfg = config::load_keys(&app)?;

    if key.id.is_empty() {
        key.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = key.id.clone();
    let existing = cfg.keys.iter().find(|k| k.id == target_id);

    key.key = resolve_private_key_for_save(&key, existing)?;
    if key.key.is_none() {
        return Err(AppError::Config("SSH private key is required".to_string()));
    }

    key.cert = resolve_certificate_for_save(&key, existing)?;

    key.passphrase = match key.passphrase.as_deref() {
        Some(plain) if !plain.is_empty() => Some(crypto::encrypt(plain)?),
        Some("") => None,
        _ => existing.and_then(|e| e.passphrase.clone()),
    };

    if let Some(ex) = cfg.keys.iter_mut().find(|k| k.id == target_id) {
        *ex = key;
    } else {
        cfg.keys.push(key);
    }
    config::save_keys(&app, &cfg)?;
    schedule_cloud_sync_notify(app.clone());
    Ok(target_id)
}

#[tauri::command]
pub fn delete_ssh_key(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let mut cfg = config::load_keys(&app)?;
    cfg.keys.retain(|k| k.id != id);
    config::save_keys(&app, &cfg)?;
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn get_groups(app: tauri::AppHandle) -> AppResult<Vec<Group>> {
    let cfg = config::load_config(&app)?;
    Ok(cfg.groups)
}

#[tauri::command]
pub fn save_group(app: tauri::AppHandle, mut group: Group) -> AppResult<String> {
    let mut cfg = config::load_config(&app)?;

    if group.id.is_empty() {
        group.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = group.id.clone();

    if let Some(existing) = cfg.groups.iter_mut().find(|g| g.id == target_id) {
        *existing = group;
    } else {
        cfg.groups.push(group);
    }
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(target_id)
}

fn delete_group_from_config(cfg: &mut config::AppConfig, id: &str) {
    // Collect the target group and all descendant groups.
    let mut ids_to_remove = vec![id.to_string()];
    let mut i = 0;
    while i < ids_to_remove.len() {
        let parent = ids_to_remove[i].clone();
        for group in &cfg.groups {
            if group.parent_id.as_deref() == Some(&parent) && !ids_to_remove.contains(&group.id) {
                ids_to_remove.push(group.id.clone());
            }
        }
        i += 1;
    }

    cfg.groups
        .retain(|group| !ids_to_remove.contains(&group.id));
    cfg.connections.retain(|connection| {
        connection
            .group_id
            .as_ref()
            .is_none_or(|group_id| !ids_to_remove.contains(group_id))
    });
}

#[tauri::command]
pub fn delete_group(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    delete_group_from_config(&mut cfg, &id);
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn clear_all_connections(app: tauri::AppHandle) -> AppResult<()> {
    let mut cfg = config::load_config(&app)?;
    cfg.connections.clear();
    cfg.groups.clear();
    config::save_config(&app, &cfg)?;
    let _ = app.emit("connections-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn get_quick_commands(
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
) -> AppResult<QuickCommandsConfig> {
    Ok(state.snapshot())
}

#[tauri::command]
pub fn save_quick_commands(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    config: QuickCommandsConfig,
) -> AppResult<()> {
    state.save_all(&app, config)?;
    let _ = app.emit("quick-commands-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn upsert_quick_command(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    command: config::QuickCommand,
    new_category: Option<config::QuickCommandCategory>,
) -> AppResult<()> {
    state.upsert(&app, command, new_category)?;
    let _ = app.emit("quick-commands-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

#[tauri::command]
pub fn increment_quick_command_use_count(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    id: String,
) -> AppResult<()> {
    state.increment_use_count(&app, &id)?;
    Ok(())
}

#[tauri::command]
pub fn import_quick_commands(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<QuickCommandsStore>>,
    file_path: String,
    source: QuickCommandsImportSource,
) -> AppResult<QuickCommandsImportResult> {
    let result = state.import_from_file(&app, &file_path, source)?;
    let _ = app.emit("quick-commands-changed", ());
    schedule_cloud_sync_notify(app.clone());
    Ok(result)
}

// --- Password management ---

#[tauri::command]
pub fn get_saved_passwords(app: tauri::AppHandle) -> AppResult<Vec<SavedPassword>> {
    let mut cfg = config::load_passwords(&app)?;
    for p in &mut cfg.passwords {
        p.password = None;
    }
    Ok(cfg.passwords)
}

#[tauri::command]
pub fn get_saved_password_value(app: tauri::AppHandle, id: String) -> AppResult<Option<String>> {
    Ok(config::load_password_by_id(&app, &id)?.password)
}

#[tauri::command]
pub fn save_password(app: tauri::AppHandle, mut entry: SavedPassword) -> AppResult<String> {
    let mut cfg = config::load_passwords(&app)?;

    if entry.id.is_empty() {
        entry.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = entry.id.clone();
    let existing = cfg.passwords.iter().find(|p| p.id == target_id);

    entry.password = match entry.password.as_deref() {
        Some(plain) if !plain.is_empty() => Some(crypto::encrypt(plain)?),
        _ => existing.and_then(|e| e.password.clone()),
    };

    if let Some(ex) = cfg.passwords.iter_mut().find(|p| p.id == target_id) {
        *ex = entry;
    } else {
        cfg.passwords.push(entry);
    }
    config::save_passwords(&app, &cfg)?;
    schedule_cloud_sync_notify(app.clone());
    Ok(target_id)
}

#[tauri::command]
pub fn delete_password(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let mut cfg = config::load_passwords(&app)?;
    cfg.passwords.retain(|p| p.id != id);
    config::save_passwords(&app, &cfg)?;
    schedule_cloud_sync_notify(app.clone());
    Ok(())
}

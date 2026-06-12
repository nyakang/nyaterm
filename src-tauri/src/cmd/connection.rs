use crate::config::{self, Group, QuickCommandsConfig, SavedConnection, SavedPassword, SshKey};
use crate::core::{QuickCommandsImportResult, QuickCommandsImportSource, QuickCommandsStore};
use crate::error::{AppError, AppResult};
use crate::utils::crypto;
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
pub fn save_connection(
    app: tauri::AppHandle,
    mut connection: SavedConnection,
) -> AppResult<String> {
    let mut cfg = config::load_config(&app)?;

    if connection.id.is_empty() {
        connection.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = connection.id.clone();
    let existing = cfg.connections.iter().find(|c| c.id == target_id);

    validate_proxy_jump_config(&connection, &cfg.connections)?;
    validate_local_terminal_config(&connection)?;

    if let Some(ref mut auth) = connection.auth {
        // password_id: Some("") means explicitly cleared, None means preserve existing
        match auth.password_id.as_deref() {
            Some("") => auth.password_id = None,
            None => {
                auth.password_id = existing
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
                .and_then(|e| e.auth.as_ref())
                .and_then(|a| a.password.clone()),
            _ => None,
        };
        auth.has_password = false;
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

    crate::core::pty::parse_shell_args(shell_args).map_err(AppError::Config)?;

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

    if connection.id == proxy_jump_id {
        return Err(AppError::Config(
            "A connection cannot use itself as a jump host".to_string(),
        ));
    }

    let jump_connection = existing_connections
        .iter()
        .find(|candidate| candidate.id == proxy_jump_id)
        .ok_or_else(|| AppError::Config(format!("Jump host '{}' not found", proxy_jump_id)))?;

    if !matches!(jump_connection.config, config::ConnectionType::Ssh { .. }) {
        return Err(AppError::Config(
            "Only SSH connections can be used as jump hosts".to_string(),
        ));
    }

    if jump_connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref())
        .is_some()
    {
        return Err(AppError::Config(
            "A connection that already uses a jump host cannot be selected as a jump host"
                .to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        delete_group_from_config, validate_local_terminal_config, validate_proxy_jump_config,
    };
    use crate::config::{
        AiExecutionProfile, ConnectionNetwork, ConnectionType, Group, SavedConnection,
        SessionsConfig,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn ssh_connection(id: &str, proxy_jump_id: Option<&str>) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: format!("SSH {id}"),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            auth: None,
            network: proxy_jump_id.map(|jump_id| ConnectionNetwork {
                proxy_id: None,
                proxy_jump_id: Some(jump_id.to_string()),
            }),
            post_login: None,
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
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: "del".to_string(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            auth: None,
            network: proxy_jump_id.map(|jump_id| ConnectionNetwork {
                proxy_id: None,
                proxy_jump_id: Some(jump_id.to_string()),
            }),
            post_login: None,
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
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            auth: None,
            network: None,
            post_login: None,
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
    fn rejects_multi_hop_jump_hosts() {
        let connection = ssh_connection("target", Some("jump"));
        let jump = ssh_connection("jump", Some("another"));

        let error = validate_proxy_jump_config(&connection, &[jump]).unwrap_err();

        assert!(error.to_string().contains("already uses a jump host"));
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
pub fn save_ssh_key(app: tauri::AppHandle, mut key: SshKey) -> AppResult<String> {
    let mut cfg = config::load_keys(&app)?;

    if key.id.is_empty() {
        key.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = key.id.clone();
    let existing = cfg.keys.iter().find(|k| k.id == target_id);

    key.key = match key.key_file_path.as_deref() {
        Some(path) if !path.is_empty() => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| AppError::Config(format!("failed to read key file: {e}")))?;
            Some(crypto::encrypt(&content)?)
        }
        _ => existing.and_then(|e| e.key.clone()),
    };

    key.cert = match key.cert_file_path.as_deref() {
        Some(path) if !path.is_empty() => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| AppError::Config(format!("failed to read certificate file: {e}")))?;
            Some(crypto::encrypt(&content)?)
        }
        _ => existing.and_then(|e| e.cert.clone()),
    };

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

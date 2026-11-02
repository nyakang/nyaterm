use std::collections::HashMap;

use serde::Deserialize;

use crate::{AiExecutionProfile, ConnectionAuth, ConnectionType, SavedPassword, SshKey};

use super::{
    AppError, AppResult, PreparedSessionConnection, PreparedSessionImport,
    normalize_optional_secret, normalize_optional_string,
};

#[derive(Deserialize)]
struct NyatermJsonImportFile {
    #[serde(default = "default_import_version")]
    version: u32,
    #[serde(default)]
    passwords: Vec<NyatermJsonPassword>,
    #[serde(default)]
    ssh_keys: Vec<NyatermJsonSshKey>,
    #[serde(default)]
    groups: Vec<NyatermJsonGroup>,
    #[serde(default)]
    sessions: Vec<serde_json::Value>,
    #[serde(default)]
    custom_icons: Vec<crate::models::sessions::ConnectionCustomIcon>,
}

fn default_telnet_keep_alive_interval_secs() -> u32 {
    crate::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS
}

fn default_import_version() -> u32 {
    1
}

#[derive(Deserialize)]
struct NyatermJsonPassword {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    #[serde(default)]
    sort_order: i32,
    #[serde(default)]
    username: String,
    password: crate::SecretString,
}

#[derive(Deserialize)]
struct NyatermJsonSshKey {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    #[serde(default)]
    sort_order: i32,
    private_key: crate::SecretString,
    #[serde(default)]
    certificate: Option<crate::SecretString>,
    #[serde(default)]
    passphrase: Option<crate::SecretString>,
}

#[derive(Deserialize)]
struct NyatermJsonGroup {
    path: Vec<String>,
}

#[derive(Deserialize)]
struct NyatermJsonSshAuth {
    mode: String,
    #[serde(default)]
    account_ref: Option<String>,
    #[serde(default)]
    password_source: Option<crate::models::credentials::ConnectionPasswordSource>,
    #[serde(default)]
    password: Option<crate::SecretString>,
    #[serde(default)]
    password_ref: Option<String>,
    #[serde(default)]
    key_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NyatermJsonSession {
    Ssh {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default = "default_ssh_user")]
        username: String,
        #[serde(default)]
        auth: Option<NyatermJsonSshAuth>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    LocalTerminal {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        #[serde(default)]
        shell_path: String,
        #[serde(default)]
        shell_args: String,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    Telnet {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        auth: Option<NyatermJsonSshAuth>,
        #[serde(default = "default_telnet_keep_alive_interval_secs")]
        keep_alive_interval_secs: u32,
        #[serde(default = "default_telnet_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    Serial {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        port_name: String,
        #[serde(default = "default_serial_baud_rate")]
        baud_rate: u32,
        #[serde(default = "default_serial_data_bits")]
        data_bits: u8,
        #[serde(default = "default_serial_parity")]
        parity: String,
        #[serde(default)]
        flow_control: crate::models::connection::SerialFlowControl,
        #[serde(default = "default_serial_stop_bits")]
        stop_bits: String,
        #[serde(default = "default_serial_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
}

fn default_ssh_port() -> u16 {
    22
}

fn default_ssh_user() -> String {
    "root".to_string()
}

fn default_telnet_port() -> u16 {
    23
}

fn default_telnet_backspace_mode() -> String {
    "del".to_string()
}

fn default_serial_baud_rate() -> u32 {
    115_200
}

fn default_serial_data_bits() -> u8 {
    8
}

fn default_serial_parity() -> String {
    "none".to_string()
}

fn default_serial_stop_bits() -> String {
    "1".to_string()
}

fn default_serial_backspace_mode() -> String {
    "ctrl_h".to_string()
}

// NyaTerm JSON (.json)

pub(super) fn parse_nyaterm_json_content(content: &str) -> AppResult<PreparedSessionImport> {
    let file: NyatermJsonImportFile = serde_json::from_str(content)
        .map_err(|e| AppError::Config(format!("Invalid NyaTerm JSON: {e}")))?;
    prepare_nyaterm_json_import(file)
}

fn prepare_nyaterm_json_import(file: NyatermJsonImportFile) -> AppResult<PreparedSessionImport> {
    if file.version != 1 {
        return Err(AppError::Config(format!(
            "Unsupported NyaTerm JSON import version: {}",
            file.version
        )));
    }

    let mut password_ref_map: HashMap<String, String> = HashMap::new();
    let mut passwords = Vec::new();
    for entry in file.passwords {
        let ref_name = required_string(entry.ref_name, "password ref", "passwords")?;
        if password_ref_map.contains_key(&ref_name) {
            return Err(AppError::Config(format!(
                "Duplicate password ref in import file: {ref_name}"
            )));
        }
        if entry.password.is_empty() {
            return Err(AppError::Config(format!(
                "Password entry '{ref_name}' cannot have an empty password"
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        password_ref_map.insert(ref_name, id.clone());
        passwords.push(SavedPassword {
            sort_order: entry.sort_order,
            id,
            username: entry.username,
            name: required_string(entry.name, "password name", "passwords")?,
            password: Some(entry.password),
            has_password: false,
        });
    }

    let mut key_ref_map: HashMap<String, String> = HashMap::new();
    let mut ssh_keys = Vec::new();
    for entry in file.ssh_keys {
        let ref_name = required_string(entry.ref_name, "ssh key ref", "ssh_keys")?;
        if key_ref_map.contains_key(&ref_name) {
            return Err(AppError::Config(format!(
                "Duplicate ssh key ref in import file: {ref_name}"
            )));
        }
        if entry.private_key.trim().is_empty() {
            return Err(AppError::Config(format!(
                "SSH key entry '{ref_name}' cannot have empty private_key"
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        key_ref_map.insert(ref_name, id.clone());
        ssh_keys.push(SshKey {
            sort_order: entry.sort_order,
            id,
            name: required_string(entry.name, "ssh key name", "ssh_keys")?,
            key: Some(entry.private_key),
            cert: normalize_optional_secret(entry.certificate),
            passphrase: normalize_optional_secret(entry.passphrase),
            key_file_path: None,
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        });
    }

    let mut groups = Vec::new();
    for group in file.groups {
        let path = normalize_required_group_path(group.path, "groups.path")?;
        if !groups.contains(&path) {
            groups.push(path);
        }
    }

    let mut connections = Vec::new();
    for mut raw in file.sessions {
        let session: NyatermJsonSession = serde_json::from_value(raw.clone())
            .map_err(|error| AppError::Config(format!("Invalid session: {error}")))?;
        let mut prepared = prepare_nyaterm_json_session(session, &password_ref_map, &key_ref_map)?;
        if let Some(auth) = raw
            .get_mut("auth")
            .and_then(serde_json::Value::as_object_mut)
        {
            auth.remove("password_ref");
            auth.remove("account_ref");
            auth.remove("key_ref");
        }
        let mut saved: crate::SavedConnection = serde_json::from_value(raw)
            .map_err(|error| AppError::Config(format!("Invalid saved connection: {error}")))?;
        saved.auth = prepared.auth.clone();
        prepared.saved = Some(saved);
        connections.push(prepared);
    }

    Ok(PreparedSessionImport {
        warnings: Vec::new(),
        custom_icons: file.custom_icons,
        groups,
        passwords,
        ssh_keys,
        connections,
    })
}

fn prepare_nyaterm_json_session(
    session: NyatermJsonSession,
    password_ref_map: &HashMap<String, String>,
    key_ref_map: &HashMap<String, String>,
) -> AppResult<PreparedSessionConnection> {
    match session {
        NyatermJsonSession::Ssh {
            name,
            group_path,
            host,
            port,
            username,
            auth,
            description,
            sort_order,
            icon,
        } => {
            validate_port(port, "ssh session")?;
            let context = format!("ssh session '{name}'");
            Ok(PreparedSessionConnection {
                saved: None,
                name: required_string(name, "name", "ssh session")?,
                config: ConnectionType::Ssh {
                    host: required_string(host, "host", &context)?,
                    port,
                    username: required_string(username, "username", &context)?,
                    backspace_mode: "del".to_string(),
                    ai_execution_profile: AiExecutionProfile::Auto,
                    x11_forwarding: false,
                    auth_agent_endpoint: None,
                    agent_forwarding_config: None,
                    legacy_agent_forwarding: None,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: Some(prepare_json_ssh_auth(
                    auth,
                    password_ref_map,
                    key_ref_map,
                    &context,
                )?),
            })
        }
        NyatermJsonSession::LocalTerminal {
            name,
            group_path,
            shell_path,
            shell_args,
            working_dir,
            description,
            sort_order,
            icon,
        } => {
            let context = format!("local_terminal session '{name}'");
            Ok(PreparedSessionConnection {
                saved: None,
                name: required_string(name, "name", "local_terminal session")?,
                config: ConnectionType::LocalTerminal {
                    shell_path: required_string(shell_path, "shell_path", &context)?,
                    shell_args,
                    working_dir: normalize_optional_string(working_dir),
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
        NyatermJsonSession::Telnet {
            name,
            group_path,
            host,
            port,
            username,
            auth,
            backspace_mode,
            keep_alive_interval_secs,
            description,
            sort_order,
            icon,
        } => {
            validate_port(port, "telnet session")?;
            validate_backspace_mode(&backspace_mode, "telnet session")?;
            let context = format!("telnet session '{name}'");
            Ok(PreparedSessionConnection {
                saved: None,
                name: required_string(name, "name", "telnet session")?,
                config: ConnectionType::Telnet {
                    host: required_string(host, "host", &context)?,
                    port,
                    username,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    backspace_mode,
                    raw_tcp_cli: false,
                    enter_mode: "cr".to_string(),
                    local_echo: false,
                    local_line_edit: false,
                    force_character_at_a_time: false,
                    send_naws: true,
                    send_sga: true,
                    keep_alive_interval_secs,
                    auto_login: Default::default(),
                    encoding: String::new(),
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: auth
                    .map(|auth| {
                        prepare_json_ssh_auth(Some(auth), password_ref_map, key_ref_map, &context)
                    })
                    .transpose()?,
            })
        }
        NyatermJsonSession::Serial {
            name,
            group_path,
            port_name,
            baud_rate,
            data_bits,
            parity,
            flow_control,
            stop_bits,
            backspace_mode,
            description,
            sort_order,
            icon,
        } => {
            validate_serial_config(baud_rate, data_bits, &parity, &stop_bits, &backspace_mode)?;
            let context = format!("serial session '{name}'");
            Ok(PreparedSessionConnection {
                saved: None,
                name: required_string(name, "name", "serial session")?,
                config: ConnectionType::Serial {
                    port_name: required_string(port_name, "port_name", &context)?,
                    baud_rate,
                    data_bits,
                    parity,
                    flow_control,
                    stop_bits,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    backspace_mode,
                    encoding: String::new(),
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
    }
}

fn prepare_json_ssh_auth(
    auth: Option<NyatermJsonSshAuth>,
    password_ref_map: &HashMap<String, String>,
    key_ref_map: &HashMap<String, String>,
    context: &str,
) -> AppResult<ConnectionAuth> {
    let Some(auth) = auth else {
        return Ok(ConnectionAuth {
            mode: "none".to_string(),
            ..ConnectionAuth::default()
        });
    };

    let account_id = normalize_optional_string(auth.account_ref.clone())
        .map(|reference| {
            password_ref_map
                .get(&reference)
                .cloned()
                .ok_or_else(|| AppError::Config(format!("{context}: account_ref was not found")))
        })
        .transpose()?;
    let password_source = auth.password_source;
    match auth.mode.trim() {
        "none" => {
            if auth.password.is_some() || auth.password_ref.is_some() || auth.key_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: auth.mode 'none' cannot include password, password_ref, or key_ref"
                )));
            }
            Ok(ConnectionAuth {
                account_id,
                password_source,
                mode: "none".to_string(),
                ..ConnectionAuth::default()
            })
        }
        "password" => {
            let has_password = auth
                .password
                .as_ref()
                .is_some_and(|value| !value.is_empty());
            let password_ref = normalize_optional_string(auth.password_ref);
            let account_password = password_source
                == Some(crate::models::credentials::ConnectionPasswordSource::Account);
            if account_password && (account_id.is_none() || has_password || password_ref.is_some())
            {
                return Err(AppError::Config(format!(
                    "{context}: account password requires only account_ref"
                )));
            }
            let connection_password = password_source
                == Some(crate::models::credentials::ConnectionPasswordSource::Connection);
            if connection_password && password_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: connection password cannot use password_ref"
                )));
            }
            if !account_password && !connection_password && has_password == password_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: password auth must include exactly one of password or password_ref"
                )));
            }
            let password_id = if let Some(ref_name) = password_ref {
                Some(password_ref_map.get(&ref_name).cloned().ok_or_else(|| {
                    AppError::Config(format!(
                        "{context}: password_ref '{ref_name}' was not found"
                    ))
                })?)
            } else {
                None
            };
            let password = auth.password;

            Ok(ConnectionAuth {
                account_id,
                password_source,
                mode: "password".to_string(),
                password_id,
                password,
                ..ConnectionAuth::default()
            })
        }
        "key" => {
            if auth.password.is_some() || auth.password_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: key auth cannot include password or password_ref"
                )));
            }
            let key_ref = normalize_optional_string(auth.key_ref)
                .ok_or_else(|| AppError::Config(format!("{context}: key auth requires key_ref")))?;
            let key_id = key_ref_map.get(&key_ref).cloned().ok_or_else(|| {
                AppError::Config(format!("{context}: key_ref '{key_ref}' was not found"))
            })?;

            Ok(ConnectionAuth {
                account_id,
                password_source,
                mode: "key".to_string(),
                key_id: Some(key_id),
                ..ConnectionAuth::default()
            })
        }
        mode => Err(AppError::Config(format!(
            "{context}: unsupported SSH auth mode '{mode}'"
        ))),
    }
}

fn required_string(value: String, field: &str, context: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{context}: {field} is required")));
    }
    Ok(trimmed.to_string())
}

fn normalize_required_group_path(path: Vec<String>, context: &str) -> AppResult<Vec<String>> {
    normalize_optional_group_path(path, context)?.ok_or_else(|| {
        AppError::Config(format!(
            "{context}: group path must contain at least one segment"
        ))
    })
}

fn normalize_optional_group_path(
    path: Vec<String>,
    context: &str,
) -> AppResult<Option<Vec<String>>> {
    if path.is_empty() {
        return Ok(None);
    }

    let mut segments = Vec::new();
    for segment in path {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(AppError::Config(format!(
                "{context}: group path segments cannot be empty"
            )));
        }
        segments.push(trimmed.to_string());
    }
    Ok(Some(segments))
}

fn validate_port(port: u16, context: &str) -> AppResult<()> {
    if port == 0 {
        return Err(AppError::Config(format!(
            "{context}: port must be between 1 and 65535"
        )));
    }
    Ok(())
}

fn validate_backspace_mode(value: &str, context: &str) -> AppResult<()> {
    match value {
        "ctrl_h" | "del" => Ok(()),
        _ => Err(AppError::Config(format!(
            "{context}: backspace_mode must be 'ctrl_h' or 'del'"
        ))),
    }
}

fn validate_serial_config(
    baud_rate: u32,
    data_bits: u8,
    parity: &str,
    stop_bits: &str,
    backspace_mode: &str,
) -> AppResult<()> {
    if baud_rate == 0 {
        return Err(AppError::Config(
            "serial session: baud_rate must be greater than 0".to_string(),
        ));
    }
    if !(5..=8).contains(&data_bits) {
        return Err(AppError::Config(
            "serial session: data_bits must be between 5 and 8".to_string(),
        ));
    }
    match parity {
        "none" | "even" | "odd" => {}
        _ => {
            return Err(AppError::Config(
                "serial session: parity must be 'none', 'even', or 'odd'".to_string(),
            ));
        }
    }
    match stop_bits {
        "1" | "1.5" | "2" => {}
        _ => {
            return Err(AppError::Config(
                "serial session: stop_bits must be '1', '1.5', or '2'".to_string(),
            ));
        }
    }
    validate_backspace_mode(backspace_mode, "serial session")
}

#[cfg(test)]
mod account_tests {
    use super::parse_nyaterm_json_content;
    use crate::models::credentials::ConnectionPasswordSource;

    #[test]
    fn native_session_tags_import_and_keep_top_level_authority() {
        let content = serde_json::json!({"sessions":[
            {"type":"ssh", "name":"Legacy", "host":"test.invalid", "asset":{"tags":["legacy"]}},
            {"type":"ssh", "name":"New", "host":"test.invalid", "tags":["canonical"], "asset":{"tags":["stale"], "future_asset":true}}
        ]}).to_string();
        let prepared = parse_nyaterm_json_content(&content).unwrap();
        assert_eq!(
            prepared.connections[0].saved.as_ref().unwrap().tags,
            ["legacy"]
        );
        assert_eq!(
            prepared.connections[1].saved.as_ref().unwrap().tags,
            ["canonical"]
        );
        let raw = serde_json::to_value(prepared.connections[1].saved.as_ref().unwrap()).unwrap();
        assert_eq!(raw["asset"]["tags"], raw["tags"]);
        assert_eq!(raw["asset"]["future_asset"], true);
    }

    #[test]
    fn ssh_and_telnet_account_refs_resolve_to_imported_ids_and_keep_username() {
        let content = serde_json::json!({
            "version":1,
            "passwords":[{"ref":"shared", "name":"Shared", "username":"shared-user", "password":"synthetic-secret"}],
            "sessions":[
                {"type":"ssh", "name":"SSH", "host":"test.invalid", "auth":{"mode":"password", "account_ref":"shared", "password_source":"account"}},
                {"type":"telnet", "name":"Telnet", "host":"test.invalid", "auth":{"mode":"password", "account_ref":"shared", "password_source":"connection"}}
            ]
        }).to_string();
        let prepared = parse_nyaterm_json_content(&content).unwrap();
        assert_eq!(prepared.passwords[0].username, "shared-user");
        for connection in &prepared.connections {
            assert_eq!(
                connection.auth.as_ref().unwrap().account_id.as_deref(),
                Some(prepared.passwords[0].id.as_str())
            );
            let raw = serde_json::to_value(connection.saved.as_ref().unwrap()).unwrap();
            assert!(raw["auth"].get("account_ref").is_none());
        }
        assert_eq!(
            prepared.connections[0]
                .auth
                .as_ref()
                .unwrap()
                .password_source,
            Some(ConnectionPasswordSource::Account)
        );
        assert_eq!(
            prepared.connections[1]
                .auth
                .as_ref()
                .unwrap()
                .password_source,
            Some(ConnectionPasswordSource::Connection)
        );
    }

    #[test]
    fn unknown_account_refs_and_connection_source_password_refs_are_rejected() {
        for auth in [
            serde_json::json!({"mode":"password", "account_ref":"missing", "password_source":"account"}),
            serde_json::json!({"mode":"password", "password_ref":"shared", "password_source":"connection"}),
        ] {
            let content = serde_json::json!({
                "passwords":[{"ref":"shared", "name":"Shared", "password":"synthetic-secret"}],
                "sessions":[{"type":"ssh", "name":"SSH", "host":"test.invalid", "auth":auth}]
            })
            .to_string();
            assert!(parse_nyaterm_json_content(&content).is_err());
        }
    }
}

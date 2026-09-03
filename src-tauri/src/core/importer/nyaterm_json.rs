fn parse_nyaterm_json_content(content: &str) -> AppResult<PreparedJsonImport> {
    let file: NyatermJsonImportFile = serde_json::from_str(content)
        .map_err(|e| AppError::Config(format!("Invalid NyaTerm JSON: {e}")))?;
    prepare_nyaterm_json_import(file)
}

fn prepare_nyaterm_json_import(file: NyatermJsonImportFile) -> AppResult<PreparedJsonImport> {
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
        passwords.push(config::SavedPassword {
            id,
            name: required_string(entry.name, "password name", "passwords")?,
            password: Some(encrypt_import_secret(&entry.password)?),
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
        ssh_keys.push(config::SshKey {
            id,
            name: required_string(entry.name, "ssh key name", "ssh_keys")?,
            key: Some(encrypt_import_secret(&entry.private_key)?),
            cert: encrypt_optional_secret(entry.certificate)?,
            passphrase: encrypt_optional_secret(entry.passphrase)?,
            key_data: None,
            cert_data: None,
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
    for session in file.sessions {
        connections.push(prepare_nyaterm_json_session(
            session,
            &password_ref_map,
            &key_ref_map,
        )?);
    }

    Ok(PreparedJsonImport {
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
) -> AppResult<PreparedJsonConnection> {
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
            dynamic_tab_title,
        } => {
            validate_port(port, "ssh session")?;
            let context = format!("ssh session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "ssh session")?,
                config: ConnectionType::Ssh {
                    host: required_string(host, "host", &context)?,
                    port,
                    username: required_string(username, "username", &context)?,
                    backspace_mode: "del".to_string(),
                    x11_forwarding: false,
                    auth_agent_endpoint: None,
                    legacy_agent_forwarding: None,
                    agent_forwarding_config: None,
                    encoding: String::new(),
                    dynamic_tab_title,
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
            dynamic_tab_title,
        } => {
            let context = format!("local_terminal session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "local_terminal session")?,
                config: ConnectionType::LocalTerminal {
                    shell_path: required_string(shell_path, "shell_path", &context)?,
                    shell_args,
                    working_dir: normalize_optional_string(working_dir),
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title,
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
            backspace_mode,
            description,
            sort_order,
            icon,
        } => {
            validate_port(port, "telnet session")?;
            validate_backspace_mode(&backspace_mode, "telnet session")?;
            let context = format!("telnet session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "telnet session")?,
                config: ConnectionType::Telnet {
                    host: required_string(host, "host", &context)?,
                    port,
                    username: String::new(),
                    ai_execution_profile: AiExecutionProfile::Auto,
                    backspace_mode,
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
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
        NyatermJsonSession::Serial {
            name,
            group_path,
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            backspace_mode,
            description,
            sort_order,
            icon,
        } => {
            validate_serial_config(baud_rate, data_bits, &parity, &stop_bits, &backspace_mode)?;
            let context = format!("serial session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "serial session")?,
                config: ConnectionType::Serial {
                    port_name: required_string(port_name, "port_name", &context)?,
                    baud_rate,
                    data_bits,
                    parity,
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
            password_id: None,
            password: None,
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        });
    };

    match auth.mode.trim() {
        "none" => {
            if auth.password.is_some() || auth.password_ref.is_some() || auth.key_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: auth.mode 'none' cannot include password, password_ref, or key_ref"
                )));
            }
            Ok(ConnectionAuth {
                mode: "none".to_string(),
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            })
        }
        "password" => {
            let has_password = auth
                .password
                .as_ref()
                .is_some_and(|value| !value.is_empty());
            let password_ref = normalize_optional_string(auth.password_ref);
            if has_password == password_ref.is_some() {
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
            let password = auth
                .password
                .map(|plain| encrypt_import_secret(&plain))
                .transpose()?;

            Ok(ConnectionAuth {
                mode: "password".to_string(),
                password_id,
                password,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
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
                mode: "key".to_string(),
                password_id: None,
                password: None,
                key_id: Some(key_id),
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            })
        }
        "agent" => {
            if auth.password.is_some()
                || auth.password_ref.is_some()
                || auth.key_ref.is_some()
            {
                return Err(AppError::Config(format!(
                    "{context}: agent auth cannot include password, password_ref, or key_ref"
                )));
            }
            Ok(ConnectionAuth {
                mode: "agent".to_string(),
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
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

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn encrypt_optional_secret(value: Option<String>) -> AppResult<Option<String>> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| encrypt_import_secret(&value))
        .transpose()
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

// ── Shared import persistence helpers ──────────────────────────────────────

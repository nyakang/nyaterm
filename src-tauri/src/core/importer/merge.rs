fn build_group_path(groups: &[Group], id: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = id;
    loop {
        if let Some(g) = groups.iter().find(|g| g.id == current) {
            segments.push(g.name.clone());
            if let Some(ref pid) = g.parent_id {
                current = pid;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    segments.reverse();
    segments
}

fn build_group_path_map(groups: &[Group]) -> HashMap<Vec<String>, String> {
    let mut path_map = HashMap::new();
    for group in groups {
        let path = build_group_path(groups, &group.id);
        path_map.insert(path, group.id.clone());
    }
    path_map
}

fn ensure_group_path(
    cfg: &mut config::AppConfig,
    path_map: &mut HashMap<Vec<String>, String>,
    next_sort: &mut i32,
    segments: &[String],
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    let mut leaf_id = String::new();
    for depth in 1..=segments.len() {
        let prefix: Vec<String> = segments[..depth].to_vec();
        if let Some(existing) = path_map.get(&prefix) {
            leaf_id = existing.clone();
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let parent_id = if depth > 1 {
                let parent_prefix: Vec<String> = segments[..depth - 1].to_vec();
                path_map.get(&parent_prefix).cloned()
            } else {
                None
            };
            cfg.groups.push(Group {
                id: id.clone(),
                name: segments[depth - 1].clone(),
                parent_id,
                sort_order: *next_sort,
                created_at_ms: None,
                updated_at_ms: None,
            });
            *next_sort += 1;
            path_map.insert(prefix, id.clone());
            leaf_id = id;
        }
    }
    Some(leaf_id)
}

fn import_legacy_sessions(
    app: &tauri::AppHandle,
    imported: Vec<ImportedSession>,
) -> AppResult<usize> {
    if imported.is_empty() {
        return Ok(0);
    }

    let mut cfg = config::load_config(app)?;
    let count = imported.len();
    let mut path_map = build_group_path_map(&cfg.groups);
    let mut next_sort = cfg.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;

    for sess in imported {
        let group_id = sess.group_path.as_ref().and_then(|segments| {
            ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, segments)
        });

        cfg.connections.push(SavedConnection {
            id: uuid::Uuid::new_v4().to_string(),
            name: sess.name,
            config: ConnectionType::Ssh {
                host: sess.host,
                port: sess.port,
                username: sess.username,
                backspace_mode: "del".to_string(),
                x11_forwarding: false,
                auth_agent_endpoint: None,
                legacy_agent_forwarding: None,
                agent_forwarding_config: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id,
            description: sess.description,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: sess.auth_type,
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            }),
            network: None,
            post_login: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: config::SftpSettings::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        });
    }

    config::save_config(app, &cfg)?;
    Ok(count)
}

fn import_prepared_nyaterm_json(
    app: &tauri::AppHandle,
    prepared: PreparedJsonImport,
) -> AppResult<usize> {
    if prepared.connections.is_empty() {
        return Ok(0);
    }

    let mut cfg = config::load_config(app)?;
    let mut path_map = build_group_path_map(&cfg.groups);
    let mut next_sort = cfg.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;

    for group_path in &prepared.groups {
        ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, group_path);
    }

    let count = prepared.connections.len();
    for conn in prepared.connections {
        let group_id = conn.group_path.as_ref().and_then(|segments| {
            ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, segments)
        });

        cfg.connections.push(SavedConnection {
            id: uuid::Uuid::new_v4().to_string(),
            name: conn.name,
            config: conn.config,
            group_id,
            description: conn.description,
            sort_order: conn.sort_order,
            icon: conn.icon,
            icon_auto_detect: None,
            auth: conn.auth,
            network: None,
            post_login: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: config::SftpSettings::default(),
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        });
    }

    let mut passwords = config::load_passwords(app)?;
    passwords.passwords.extend(prepared.passwords);
    config::save_passwords(app, &passwords)?;

    let mut keys = config::load_keys(app)?;
    keys.keys.extend(prepared.ssh_keys);
    config::save_keys(app, &keys)?;

    config::save_config(app, &cfg)?;
    Ok(count)
}

fn parse_json_import(path: &str) -> AppResult<PreparedJsonImport> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;
    parse_json_import_content(&content)
}

fn parse_json_import_content(content: &str) -> AppResult<PreparedJsonImport> {
    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| AppError::Config(format!("Invalid JSON import file: {e}")))?;

    if is_electerm_bookmarks_json(&value) {
        return parse_electerm_json_content(content);
    }
    if is_nyaterm_json_import(&value) {
        return parse_nyaterm_json_content(content);
    }

    Err(AppError::Config(
        "Unsupported JSON import file. Expected NyaTerm JSON or Electerm bookmarks JSON."
            .to_string(),
    ))
}

fn is_electerm_bookmarks_json(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("bookmarkGroups") && object.contains_key("bookmarks"))
}

fn is_nyaterm_json_import(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.contains_key("version")
            || object.contains_key("passwords")
            || object.contains_key("ssh_keys")
            || object.contains_key("groups")
            || object.contains_key("sessions")
    })
}

// ── Tauri Command ───────────────────────────────────────────────────────────

pub fn import_sessions(
    app: tauri::AppHandle,
    file_path: String,
    windterm_master_password: Option<String>,
) -> AppResult<usize> {
    let path = Path::new(&file_path);
    if path.is_dir() {
        let count = import_legacy_sessions(&app, parse_finalshell(&file_path)?)?;
        if count > 0 {
            let _ = app.emit("connections-changed", ());
        }
        return Ok(count);
    }

    let lower = file_path.to_lowercase();
    let count = if lower.ends_with(".xts") {
        import_legacy_sessions(&app, parse_xshell(&file_path)?)?
    } else if lower.ends_with(".mxtsessions") {
        import_legacy_sessions(&app, parse_mobaxterm(&file_path)?)?
    } else if lower.ends_with(".sessions") {
        import_prepared_nyaterm_json(
            &app,
            parse_windterm(&file_path, windterm_master_password.as_deref())?,
        )?
    } else if lower.ends_with(".xml") {
        import_legacy_sessions(&app, parse_securecrt(&file_path)?)?
    } else if lower.ends_with(".json") {
        import_prepared_nyaterm_json(&app, parse_json_import(&file_path)?)?
    } else {
        return Err(AppError::Config(
            "Unsupported file format. Please use .xts (Xshell), .mxtsessions (MobaXterm), .sessions (WindTerm), .xml (SecureCRT), .json (NyaTerm JSON or Electerm bookmarks), or a FinalShell conn directory."
                .to_string(),
        ));
    };

    if count > 0 {
        let _ = app.emit("connections-changed", ());
    }
    Ok(count)
}

pub fn import_termius_sessions(
    app: tauri::AppHandle,
    indexed_db_path: Option<String>,
) -> AppResult<usize> {
    let count = import_prepared_nyaterm_json(&app, parse_termius_indexed_db(indexed_db_path)?)?;
    if count > 0 {
        let _ = app.emit("connections-changed", ());
    }
    Ok(count)
}

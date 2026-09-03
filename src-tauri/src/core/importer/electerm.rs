#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElectermBookmarksFile {
    #[serde(default)]
    bookmark_groups: Vec<ElectermBookmarkGroup>,
    #[serde(default)]
    bookmarks: Vec<ElectermBookmark>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElectermBookmarkGroup {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    bookmark_ids: Vec<String>,
    #[serde(default)]
    bookmark_group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElectermBookmark {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    auth_type: String,
    #[serde(default)]
    port: Option<i64>,
    #[serde(default, rename = "type")]
    session_type: String,
    #[serde(default)]
    enable_ssh: Option<bool>,
}

fn parse_electerm_json_content(content: &str) -> AppResult<PreparedJsonImport> {
    let file: ElectermBookmarksFile = serde_json::from_str(content)
        .map_err(|e| AppError::Config(format!("Invalid Electerm bookmarks JSON: {e}")))?;
    prepare_electerm_import(file)
}

fn prepare_electerm_import(file: ElectermBookmarksFile) -> AppResult<PreparedJsonImport> {
    let groups_by_id: HashMap<String, ElectermBookmarkGroup> = file
        .bookmark_groups
        .into_iter()
        .map(|group| (group.id.clone(), group))
        .collect();
    let bookmark_group_ids = build_electerm_bookmark_group_map(&groups_by_id);
    let mut groups = Vec::new();
    let mut connections = Vec::new();

    for bookmark in file.bookmarks {
        let Some(connection) =
            prepare_electerm_bookmark(bookmark, &groups_by_id, &bookmark_group_ids, &mut groups)?
        else {
            continue;
        };
        connections.push(connection);
    }

    Ok(PreparedJsonImport {
        groups,
        passwords: Vec::new(),
        ssh_keys: Vec::new(),
        connections,
    })
}

fn build_electerm_bookmark_group_map(
    groups_by_id: &HashMap<String, ElectermBookmarkGroup>,
) -> HashMap<String, String> {
    let mut bookmark_group_ids = HashMap::new();

    for (group_id, group) in groups_by_id {
        for bookmark_id in &group.bookmark_ids {
            bookmark_group_ids
                .entry(bookmark_id.clone())
                .or_insert_with(|| group_id.clone());
        }
    }

    bookmark_group_ids
}

fn prepare_electerm_bookmark(
    bookmark: ElectermBookmark,
    groups_by_id: &HashMap<String, ElectermBookmarkGroup>,
    bookmark_group_ids: &HashMap<String, String>,
    groups: &mut Vec<Vec<String>>,
) -> AppResult<Option<PreparedJsonConnection>> {
    if !bookmark.session_type.eq_ignore_ascii_case("ssh") || bookmark.enable_ssh == Some(false) {
        return Ok(None);
    }

    let host = bookmark.host.trim();
    if host.is_empty() {
        return Ok(None);
    }

    let port = match bookmark.port {
        Some(port) if (1..=i64::from(u16::MAX)).contains(&port) => port as u16,
        Some(_) => return Ok(None),
        None => 22,
    };

    let username = if bookmark.username.trim().is_empty() {
        "root".to_string()
    } else {
        bookmark.username.trim().to_string()
    };
    let name = if bookmark.title.trim().is_empty() {
        host.to_string()
    } else {
        bookmark.title.trim().to_string()
    };
    let group_path = bookmark_group_ids
        .get(&bookmark.id)
        .and_then(|group_id| electerm_group_path(group_id, groups_by_id));

    if let Some(path) = &group_path {
        if !groups.contains(path) {
            groups.push(path.clone());
        }
    }

    let auth_mode = if bookmark.auth_type.trim().eq_ignore_ascii_case("password") {
        "password"
    } else {
        "none"
    };

    Ok(Some(PreparedJsonConnection {
        name,
        config: ConnectionType::Ssh {
            host: host.to_string(),
            port,
            username,
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            legacy_agent_forwarding: None,
            agent_forwarding_config: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_path,
        description: None,
        sort_order: 0,
        icon: None,
        auth: Some(ConnectionAuth {
            mode: auth_mode.to_string(),
            password_id: None,
            password: None,
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        }),
    }))
}

fn electerm_group_path(
    group_id: &str,
    groups_by_id: &HashMap<String, ElectermBookmarkGroup>,
) -> Option<Vec<String>> {
    let mut child_to_parent = HashMap::new();
    for (parent_id, group) in groups_by_id {
        for child_id in &group.bookmark_group_ids {
            child_to_parent
                .entry(child_id.as_str())
                .or_insert(parent_id.as_str());
        }
    }

    let mut path = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut current = group_id;

    loop {
        if !visited.insert(current) {
            break;
        }

        let group = groups_by_id.get(current)?;
        let title = group.title.trim();
        if !title.is_empty() {
            path.push(title.to_string());
        }

        let Some(parent_id) = child_to_parent.get(current) else {
            break;
        };
        current = *parent_id;
    }

    path.reverse();
    if path.is_empty() { None } else { Some(path) }
}

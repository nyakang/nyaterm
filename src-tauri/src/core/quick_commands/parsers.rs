fn parse_nyaterm_import(raw: &str) -> AppResult<ImportConfig> {
    let import_file: ImportFile = serde_json::from_str(raw)?;
    Ok(match import_file {
        ImportFile::Config(config) => config,
        ImportFile::Commands(commands) => ImportConfig {
            commands,
            categories: Vec::new(),
        },
    })
}

fn parse_windterm_quickbar(raw: &str) -> AppResult<ImportConfig> {
    let entries: Vec<Value> = serde_json::from_str(raw)
        .map_err(|e| AppError::Config(format!("Invalid WindTerm quickbar JSON: {e}")))?;
    let mut commands = Vec::new();

    for entry in entries {
        let label = entry
            .get("quick.label")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let raw_command = entry
            .get("quick.text")
            .and_then(Value::as_str)
            .unwrap_or("");
        if label.is_empty() || raw_command.trim().is_empty() {
            continue;
        }

        let (command, has_terminal_newline) = split_windterm_command(raw_command);

        let id = entry
            .get("quick.uuid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let category = entry
            .get("quick.group")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let icon_tag = entry
            .get("quick.icon")
            .and_then(Value::as_str)
            .and_then(map_windterm_icon);
        let quick_type = entry
            .get("quick.type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let execution_mode = if has_terminal_newline {
            "execute"
        } else if quick_type.eq_ignore_ascii_case("Send Text") {
            "append"
        } else {
            "execute"
        }
        .to_string();

        commands.push(ImportCommand {
            id,
            label: label.to_string(),
            command: command.to_string(),
            preserve_command_text: true,
            category_id: None,
            category,
            description: None,
            color_tag: None,
            icon_tag,
            pinned: false,
            execution_mode,
            source: Some("manual".to_string()),
            risk_level: None,
            sort_order: None,
            target_connection_ids: None,
        });
    }

    Ok(ImportConfig {
        commands,
        categories: Vec::new(),
    })
}

fn split_windterm_command(raw: &str) -> (&str, bool) {
    const TERMINATORS: [&str; 6] = [
        "\\r\\n",
        "\\n",
        "\\r",
        "\r\n",
        "\n",
        "\r",
    ];

    for terminator in TERMINATORS {
        if let Some(command) = raw.strip_suffix(terminator) {
            return (command, true);
        }
    }

    (raw, false)
}

fn parse_xshell_xts_quick_buttons(path: &str) -> AppResult<ImportConfig> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Config(format!("Cannot open Xshell XTS file: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Config(format!("Invalid ZIP/XTS file: {e}")))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::Config(format!("ZIP entry error: {e}")))?;
        let entry_path = decode_text(entry.name_raw()).replace('\\', "/");
        let normalized_path = entry_path.trim_start_matches("./").trim_start_matches('/');
        let lookup_path = normalized_path.to_ascii_lowercase();
        if lookup_path != "xsl/quickbutton files/commands.qbl"
            && !lookup_path.ends_with("/xsl/quickbutton files/commands.qbl")
        {
            continue;
        }

        let mut raw = Vec::new();
        entry
            .read_to_end(&mut raw)
            .map_err(|e| AppError::Config(format!("Failed to read {entry_path}: {e}")))?;
        return Ok(parse_xshell_quick_buttons_content(&decode_text(&raw)));
    }

    Err(AppError::Config(
        "Xshell quick button file not found: xsl/QuickButton Files/commands.qbl".to_string(),
    ))
}

fn parse_xshell_quick_buttons_content(raw: &str) -> ImportConfig {
    let sections = parse_ini_sections(raw);
    let Some(quick_button) = sections.get("QuickButton") else {
        return ImportConfig::default();
    };

    let mut buttons: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
    for (key, value) in quick_button {
        let Some(rest) = key.strip_prefix("Button_") else {
            continue;
        };
        let Some((index, field)) = rest.split_once('_') else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else {
            continue;
        };

        buttons
            .entry(index)
            .or_default()
            .insert(field.to_string(), value.clone());
    }

    let commands = buttons
        .into_iter()
        .filter_map(|(_, fields)| {
            let button_type = fields.get("Type").map(String::as_str).unwrap_or("");
            if button_type.trim() != "1" {
                return None;
            }

            let label = fields.get("Name").map(String::as_str).unwrap_or("").trim();
            let command = fields
                .get("Action")
                .map(String::as_str)
                .unwrap_or("")
                .trim();
            if label.is_empty() || command.is_empty() {
                return None;
            }

            Some(ImportCommand {
                id: None,
                label: label.to_string(),
                command: command.to_string(),
                preserve_command_text: false,
                category_id: None,
                category: None,
                description: trim_optional(fields.get("Desc").cloned()),
                color_tag: None,
                icon_tag: None,
                pinned: false,
                execution_mode: "append".to_string(),
                source: Some("manual".to_string()),
                risk_level: None,
                sort_order: None,
                target_connection_ids: None,
            })
        })
        .collect();

    ImportConfig {
        commands,
        categories: Vec::new(),
    }
}

fn parse_ini_sections(raw: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            sections.entry(current_section.clone()).or_default();
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current_section.clone())
                .or_default()
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    sections
}

fn decode_text(raw: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(raw) {
        let (decoded, _, _) = encoding.decode(&raw[bom_len..]);
        return decoded.into_owned();
    }

    match std::str::from_utf8(raw) {
        Ok(value) => value.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GBK.decode(raw);
            decoded.into_owned()
        }
    }
}

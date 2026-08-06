fn merge_import(
    config: &mut QuickCommandsConfig,
    import_config: ImportConfig,
) -> AppResult<ImportStats> {
    let mut stats = ImportStats::default();
    let mut category_names: BTreeMap<(Option<String>, String), String> = BTreeMap::new();
    for category in &config.categories {
        category_names.insert(
            (category.parent_id.clone(), category.name.clone()),
            category.id.clone(),
        );
    }

    for category in import_config.categories {
        let name = require_text(&category.name, "category.name")?;
        let parent_id = trim_optional(category.parent_id)
            .map(|value| normalize_id(&value, "category.parent_id"))
            .transpose()?;
        let id = match category.id {
            Some(id_input) => normalize_id(&id_input, "category.id")?,
            None => category_names
                .get(&(parent_id.clone(), name.clone()))
                .cloned()
                .unwrap_or_else(|| unique_category_id(config, &slugify(&name))),
        };
        let existing_sort_order = config
            .categories
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.sort_order);
        let sort_order = category
            .sort_order
            .or(existing_sort_order)
            .unwrap_or_else(|| next_import_category_sort_order(config, parent_id.as_deref()));
        if upsert_category(
            config,
            QuickCommandCategory {
                id: id.clone(),
                name: name.clone(),
                parent_id: parent_id.clone(),
                sort_order,
            },
        ) {
            stats.added_categories += 1;
        }
        category_names.insert((parent_id, name), id);
    }

    let mut seen_ids = BTreeSet::new();
    let now = now_millis();

    for command in import_config.commands {
        let label = require_text(&command.label, "command.label")?;
        let command_text = if command.preserve_command_text {
            require_preserved_text(&command.command, "command.command")?
        } else {
            require_text(&command.command, "command.command")?
        };
        let id_input = command
            .id
            .unwrap_or_else(|| format!("cmd-{}", Uuid::new_v4()));
        let id = normalize_id(&id_input, "command.id")?;

        if !seen_ids.insert(id.clone()) {
            return Err(AppError::Config(format!(
                "Duplicate command id in import file: {id}"
            )));
        }

        let category_id = match (command.category_id, command.category) {
            (Some(category_id), _) => {
                let category_id = normalize_id(&category_id, "command.category_id")?;
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_id,
                    None,
                    &mut stats,
                );
                Some(category_id)
            }
            (None, Some(category_name)) => {
                let category_name = require_text(&category_name, "command.category")?;
                let category_id = category_names
                    .get(&(None, category_name.clone()))
                    .cloned()
                    .unwrap_or_else(|| unique_category_id(config, &slugify(&category_name)));
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_name,
                    None,
                    &mut stats,
                );
                Some(category_id)
            }
            (None, None) => None,
        };

        let execution_mode = command.execution_mode.trim().to_string();
        let source = trim_optional(command.source);
        let risk_level = trim_optional(command.risk_level);

        validate_one_of(
            &execution_mode,
            &["execute", "append"],
            "command.execution_mode",
        )?;
        if let Some(source) = source.as_deref() {
            validate_one_of(source, &["manual", "ai"], "command.source")?;
        }
        if let Some(risk_level) = risk_level.as_deref() {
            validate_one_of(
                risk_level,
                &["low", "medium", "high", "critical"],
                "command.risk_level",
            )?;
        }

        let imported = QuickCommand {
            id,
            label,
            command: command_text,
            category_id,
            description: trim_optional(command.description),
            color_tag: trim_optional(command.color_tag),
            icon_tag: trim_optional(command.icon_tag),
            pinned: command.pinned,
            execution_mode,
            source,
            risk_level,
            updated_at: Some(now),
            created_at: Some(now),
            use_count: None,
        };

        if upsert_command(config, imported) {
            stats.added_commands += 1;
        } else {
            stats.updated_commands += 1;
        }
    }

    Ok(stats)
}

fn ensure_category(
    config: &mut QuickCommandsConfig,
    category_names: &mut BTreeMap<(Option<String>, String), String>,
    id: &str,
    name: &str,
    parent_id: Option<String>,
    stats: &mut ImportStats,
) {
    if config.categories.iter().any(|category| category.id == id) {
        category_names.insert((parent_id, name.to_string()), id.to_string());
        return;
    }

    let sort_order = next_import_category_sort_order(config, parent_id.as_deref());
    config.categories.push(QuickCommandCategory {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: parent_id.clone(),
        sort_order,
    });
    category_names.insert((parent_id, name.to_string()), id.to_string());
    stats.added_categories += 1;
}

fn upsert_category(config: &mut QuickCommandsConfig, category: QuickCommandCategory) -> bool {
    if let Some(existing) = config
        .categories
        .iter_mut()
        .find(|item| item.id == category.id)
    {
        *existing = category;
        false
    } else {
        config.categories.push(category);
        true
    }
}

fn upsert_command(config: &mut QuickCommandsConfig, command: QuickCommand) -> bool {
    if let Some(existing) = config
        .commands
        .iter_mut()
        .find(|item| item.id == command.id)
    {
        let created_at = existing.created_at;
        let use_count = existing.use_count;
        *existing = command;
        existing.created_at = created_at.or(existing.created_at);
        existing.use_count = use_count.or(existing.use_count);
        false
    } else {
        config.commands.push(command);
        true
    }
}

fn unique_category_id(config: &QuickCommandsConfig, preferred_id: &str) -> String {
    if !config
        .categories
        .iter()
        .any(|category| category.id == preferred_id)
    {
        return preferred_id.to_string();
    }

    for index in 2.. {
        let candidate = format!("{preferred_id}-{index}");
        if !config
            .categories
            .iter()
            .any(|category| category.id == candidate)
        {
            return candidate;
        }
    }

    unreachable!("unbounded category id suffix search should always return")
}

fn next_import_category_sort_order(config: &QuickCommandsConfig, parent_id: Option<&str>) -> i32 {
    config
        .categories
        .iter()
        .filter(|category| category.parent_id.as_deref() == parent_id)
        .map(|category| category.sort_order)
        .max()
        .unwrap_or(-1)
        + 1
}

fn require_text(value: &str, field: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn require_preserved_text(value: &str, field: &str) -> AppResult<String> {
    if value.trim().is_empty() {
        return Err(AppError::Config(format!("{field} cannot be empty")));
    }
    Ok(value.to_string())
}

fn normalize_id(value: &str, field: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_one_of(value: &str, allowed: &[&str], field: &str) -> AppResult<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

fn slugify(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            output.push(ch);
        } else if ch.is_whitespace() && !output.ends_with('-') {
            output.push('-');
        }
    }

    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        format!("category-{}", Uuid::new_v4())
    } else {
        output
    }
}

fn map_windterm_icon(value: &str) -> Option<String> {
    let normalized = value.to_ascii_lowercase();
    let mappings = [
        ("kubernetes", "k8s"),
        ("k8s", "k8s"),
        ("docker", "docker"),
        ("linux", "linux"),
        ("ubuntu", "ubuntu"),
        ("debian", "debian"),
        ("centos", "centos"),
        ("fedora", "fedora"),
        ("apple", "apple"),
        ("github", "github"),
        ("gitlab", "gitlab"),
        ("nginx", "nginx"),
        ("redis", "redis"),
        ("postgres", "postgres"),
        ("mysql", "mysql"),
        ("mongo", "mongodb"),
        ("python", "python"),
        ("javascript", "js"),
        ("typescript", "ts"),
        ("rust", "rust"),
        ("node", "node"),
        ("php", "php"),
        ("aws", "aws"),
        ("gcp", "gcp"),
    ];

    mappings
        .iter()
        .find_map(|(needle, icon)| normalized.contains(needle).then(|| (*icon).to_string()))
}

use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::{QuickCommand, QuickCommandCategory, QuickCommandsConfig, uuid};

use super::helpers::{
    current_time_ms, normalize_id, require_preserved_text, require_text, slugify, trim_optional,
    validate_one_of,
};
use super::{ImportConfig, ImportSummary};

pub(super) fn merge_import(
    config: &mut QuickCommandsConfig,
    import_config: ImportConfig,
) -> Result<ImportSummary, String> {
    let mut summary = ImportSummary::default();
    let mut seen_import_ids = BTreeSet::new();
    for command in &import_config.commands {
        if let Some(id_input) = command.id.as_ref() {
            let id = normalize_id(id_input, "command.id")?;
            if !seen_import_ids.insert(id.clone()) {
                return Err(format!("Duplicate command id in import file: {id}"));
            }
        }
    }

    let mut category_names = BTreeMap::new();
    for category in &config.categories {
        category_names.insert(category.name.clone(), category.id.clone());
    }

    for category in import_config.categories {
        let name = require_text(&category.name, "category.name")?;
        let id_input = category.id.unwrap_or_else(|| slugify(&name));
        let id = normalize_id(&id_input, "category.id")?;
        if upsert_category(
            config,
            QuickCommandCategory {
                id: id.clone(),
                name: name.clone(),
                parent_id: category.parent_id,
                sort_order: category.sort_order,
            },
        ) {
            summary.imported_categories += 1;
        }
        category_names.insert(name, id);
    }

    let now = current_time_ms();
    for command in import_config.commands {
        let label = require_text(&command.label, "command.label")?;
        let command_text = if command.preserve_command_text {
            require_preserved_text(&command.command, "command.command")?
        } else {
            require_text(&command.command, "command.command")?
        };
        let id_input = command.id.unwrap_or_else(|| format!("cmd-{}", uuid()));
        let id = normalize_id(&id_input, "command.id")?;

        let category_id = match (command.category_id, command.category) {
            (Some(category_id), _) => {
                let category_id = normalize_id(&category_id, "command.category_id")?;
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_id,
                    &mut summary,
                );
                Some(category_id)
            }
            (None, Some(category_name)) => {
                let category_name = require_text(&category_name, "command.category")?;
                let category_id = category_names
                    .get(&category_name)
                    .cloned()
                    .unwrap_or_else(|| slugify(&category_name));
                ensure_category(
                    config,
                    &mut category_names,
                    &category_id,
                    &category_name,
                    &mut summary,
                );
                Some(category_id)
            }
            (None, None) => None,
        };

        let execution_mode = command
            .execution_mode
            .as_deref()
            .unwrap_or("execute")
            .trim()
            .to_string();
        validate_one_of(
            &execution_mode,
            &["execute", "append"],
            "command.execution_mode",
        )?;
        if let Some(source) = command.source.as_deref().map(str::trim) {
            validate_one_of(source, &["manual", "ai"], "command.source")?;
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
            execution_mode: Some(execution_mode),
            raw_send: command.raw_send,
            source: trim_optional(command.source),
            risk_level: command.risk_level,
            updated_at: Some(now),
            created_at: Some(now),
            use_count: None,
            sort_order: command.sort_order,
        };
        if upsert_command(config, imported) {
            summary.imported_commands += 1;
        } else {
            summary.updated_commands += 1;
        }
    }
    Ok(summary)
}

pub(super) fn ensure_category(
    config: &mut QuickCommandsConfig,
    category_names: &mut BTreeMap<String, String>,
    id: &str,
    name: &str,
    summary: &mut ImportSummary,
) {
    if config.categories.iter().any(|category| category.id == id) {
        category_names.insert(name.to_string(), id.to_string());
        return;
    }
    config.categories.push(QuickCommandCategory {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: None,
        sort_order: 0,
    });
    category_names.insert(name.to_string(), id.to_string());
    summary.imported_categories += 1;
}

pub(super) fn upsert_category(
    config: &mut QuickCommandsConfig,
    category: QuickCommandCategory,
) -> bool {
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

pub(super) fn upsert_command(config: &mut QuickCommandsConfig, command: QuickCommand) -> bool {
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

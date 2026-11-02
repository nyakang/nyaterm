use nyaterm_core::RiskLevel;
use serde_json::Value;

use super::{ImportCategory, ImportCommand, ImportConfig};

pub(super) fn parse_import_value(value: Value) -> Result<ImportConfig, String> {
    match value {
        Value::Array(commands) => Ok(ImportConfig {
            commands: commands
                .into_iter()
                .map(parse_import_command)
                .collect::<Result<Vec<_>, _>>()?,
            categories: Vec::new(),
        }),
        Value::Object(mut object) => {
            let categories = match object.remove("categories") {
                Some(Value::Array(categories)) => categories
                    .into_iter()
                    .map(parse_import_category)
                    .collect::<Result<Vec<_>, _>>()?,
                Some(Value::Null) | None => Vec::new(),
                Some(_) => return Err("categories must be an array".to_string()),
            };
            let commands = match object.remove("commands") {
                Some(Value::Array(commands)) => commands
                    .into_iter()
                    .map(parse_import_command)
                    .collect::<Result<Vec<_>, _>>()?,
                Some(Value::Null) | None => Vec::new(),
                Some(_) => return Err("commands must be an array".to_string()),
            };
            Ok(ImportConfig {
                commands,
                categories,
            })
        }
        _ => Err("quick command import must be an object or command array".to_string()),
    }
}

pub(super) fn parse_import_category(value: Value) -> Result<ImportCategory, String> {
    let Value::Object(object) = value else {
        return Err("category must be an object".to_string());
    };
    let name = required_string_field(&object, "name")?;
    Ok(ImportCategory {
        id: optional_string_field(&object, "id")?,
        name,
        parent_id: optional_string_field(&object, "parent_id")?,
        sort_order: optional_i32_field(&object, "sort_order")?.unwrap_or_default(),
    })
}

pub(super) fn parse_import_command(value: Value) -> Result<ImportCommand, String> {
    let Value::Object(object) = value else {
        return Err("command must be an object".to_string());
    };
    let raw_send = optional_bool_field(&object, "raw_send")?;
    Ok(ImportCommand {
        id: optional_string_field(&object, "id")?,
        label: required_string_field(&object, "label")?,
        command: required_string_field(&object, "command")?,
        preserve_command_text: raw_send.unwrap_or_default(),
        category_id: optional_string_field(&object, "category_id")?,
        category: optional_string_field(&object, "category")?,
        description: optional_string_field(&object, "description")?,
        color_tag: optional_string_field(&object, "color_tag")?,
        icon_tag: optional_string_field(&object, "icon_tag")?,
        pinned: optional_bool_field(&object, "pinned")?,
        execution_mode: optional_string_field(&object, "execution_mode")?,
        raw_send,
        source: optional_string_field(&object, "source")?,
        risk_level: optional_risk_field(&object, "risk_level")?,
        sort_order: optional_i32_field(&object, "sort_order")?,
    })
}

pub(super) fn optional_i32_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<i32>, String> {
    match object.get(field) {
        Some(Value::Number(value)) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("{field} must be a 32-bit integer")),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("{field} must be an integer")),
    }
}

pub(super) fn required_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, String> {
    let Some(value) = object.get(field) else {
        return Err(format!("{field} cannot be empty"));
    };
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Null => Err(format!("{field} cannot be empty")),
        _ => Err(format!("{field} must be a string")),
    }
}

pub(super) fn optional_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("{field} must be a string")),
    }
}

pub(super) fn optional_bool_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<bool>, String> {
    match object.get(field) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("{field} must be a boolean")),
    }
}

pub(super) fn optional_risk_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<RiskLevel>, String> {
    let Some(value) = optional_string_field(object, field)? else {
        return Ok(None);
    };
    match value.trim() {
        "low" => Ok(Some(RiskLevel::Low)),
        "medium" => Ok(Some(RiskLevel::Medium)),
        "high" => Ok(Some(RiskLevel::High)),
        "critical" => Ok(Some(RiskLevel::Critical)),
        _ => Err(format!(
            "{field} must be one of: low, medium, high, critical"
        )),
    }
}

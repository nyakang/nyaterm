//! AI configuration policy. Secrets, headers, runtime discovery and permissions
//! are excluded even when the optional AI configuration scope is enabled.
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::sync_schema::{SyncFieldId, SyncProjection, SyncResourceId, SyncValues, pick};

pub const KINDS: &[&str] = &[
    "ai_provider",
    "ai_profile",
    "ai_model",
    "ai_action",
    "ai_preference",
];
pub const COLLECTIONS: &[(&str, &str)] = &[
    ("provider_credentials", "ai_provider"),
    ("provider_profiles", "ai_profile"),
    ("models", "ai_model"),
    ("terminal_ai_actions", "ai_action"),
    ("file_ai_actions", "ai_action"),
];
pub const PREFERENCES: &[&str] = &[
    "default_model_id",
    "default_mode",
    "default_reasoning_effort",
];

pub fn fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "ai_provider" => &["name", "transport", "enabled", "icon_data_url"],
        "ai_profile" => &["name", "transport", "model", "enabled"],
        "ai_model" => &["identity", "enabled"],
        "ai_action" => &["name", "prompt", "enabled"],
        "ai_preference" => &["value"],
        _ => &[],
    }
}

fn grouped_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "ai_provider" => &["provider_kind", "api_format", "api_protocol", "base_url"],
        "ai_profile" => &["provider_kind", "base_url"],
        "ai_model" => &[
            "name",
            "backend",
            "provider_kind",
            "credential_id",
            "source",
        ],
        _ => &[],
    }
}

pub fn project_record(kind: &str, source: &Value) -> Result<Value, String> {
    if kind == "ai_model" && source.get("source").and_then(Value::as_str) != Some("manual") {
        return Err("Automatically discovered AI models stay local".into());
    }
    let mut value = pick(source, fields(kind));
    if matches!(kind, "ai_provider" | "ai_profile") {
        // A URL can itself carry credentials. Such transports are unsupported
        // in the nonsecret scope, rather than uploading their query/userinfo.
        if let Some(url) = source.get("base_url").and_then(Value::as_str) {
            let url = url::Url::parse(url).map_err(|_| "Unsupported AI endpoint")?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("AI endpoint contains unsupported private components".into());
            }
        }
        value["transport"] = pick(source, grouped_fields(kind));
    } else if kind == "ai_model" {
        value["identity"] = pick(source, grouped_fields(kind));
    }
    Ok(value)
}

pub fn apply_record(kind: &str, original: &Value, projected: &Value) -> Result<Value, String> {
    let mut result = original.as_object().cloned().unwrap_or_default();
    let projected = projected.as_object().ok_or("Invalid AI sync record")?;
    for field in fields(kind) {
        if matches!(*field, "transport" | "identity") {
            for nested in grouped_fields(kind) {
                result.remove(*nested);
            }
            if let Some(group) = projected.get(*field) {
                if !group.is_object() {
                    return Err("Invalid AI atomic group".into());
                }
                for (key, value) in pick(group, grouped_fields(kind))
                    .as_object()
                    .expect("object")
                {
                    result.insert(key.clone(), value.clone());
                }
            }
        } else {
            result.remove(*field);
            if let Some(value) = projected.get(*field) {
                result.insert((*field).into(), value.clone());
            }
        }
    }
    Ok(Value::Object(result))
}

pub fn project_settings(settings: &Value, projection: &mut SyncProjection) -> Result<(), String> {
    let Some(ai) = settings.get("ai").filter(|value| value.is_object()) else {
        return Ok(());
    };
    let mut incomplete = BTreeSet::new();
    for (collection, kind) in COLLECTIONS {
        let Some(records) = ai.get(*collection) else {
            continue;
        };
        let records = records
            .as_array()
            .ok_or("Invalid AI configuration collection")?;
        projection.complete.insert((*kind).into());
        for record in records {
            if *kind == "ai_model"
                && record
                    .get("source")
                    .and_then(Value::as_str)
                    .is_none_or(|source| source == "rust-genai")
            {
                continue;
            }
            let Some(id) = record
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                incomplete.insert(*kind);
                continue;
            };
            let Ok(fields) = project_record(kind, record) else {
                incomplete.insert(*kind);
                continue;
            };
            let id = if *kind == "ai_action" {
                format!("{collection}/{id}")
            } else {
                id.to_owned()
            };
            projection.record(kind, &id, &fields)?;
        }
    }
    for kind in incomplete {
        projection.complete.remove(kind);
    }
    // Both action catalogs must have been read before absence proves deletion.
    if !["terminal_ai_actions", "file_ai_actions"]
        .iter()
        .all(|name| ai.get(name).is_some_and(Value::is_array))
    {
        projection.complete.remove("ai_action");
    }
    projection.complete.insert("ai_preference".into());
    for field in PREFERENCES {
        if let Some(value) = ai.get(*field) {
            projection.record("ai_preference", field, &json!({"value": value}))?;
        }
    }
    Ok(())
}

pub fn default_resources(values: &SyncValues) -> BTreeSet<SyncResourceId> {
    let defaults = serde_json::to_value(crate::ai::AiSettings::default()).expect("AI defaults");
    let mut projection = SyncProjection::default();
    if project_settings(&json!({"ai":defaults}), &mut projection).is_err() {
        return BTreeSet::new();
    }
    let mut resources = projection
        .values
        .keys()
        .filter_map(|key| SyncFieldId::from_key(key))
        .map(|field| field.resource)
        .collect::<BTreeSet<_>>();
    for (key, value) in values {
        if let Some(field) = SyncFieldId::from_key(key)
            && projection.values.get(key) != Some(value)
        {
            resources.remove(&field.resource);
        }
    }
    resources
}

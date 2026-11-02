//! Host identities require an explicit local confirmation before becoming trust.
use serde_json::{Value, json};

use crate::sync_schema::pick;

pub const KINDS: &[&str] = &["known_host", "vnc_known_host"];

pub fn project_record(kind: &str, source: &Value) -> Result<Value, String> {
    let fields: &[&str] = match kind {
        "known_host" => &[
            "marker",
            "host_identifier",
            "host_patterns",
            "key_type",
            "key_base64",
            "comment",
        ],
        "vnc_known_host" => &["host", "port", "sha256_fingerprint"],
        _ => return Err("Unsupported host identity".into()),
    };
    if !source.is_object() {
        return Err("Invalid host identity".into());
    }
    Ok(json!({"identity":pick(source, fields)}))
}

pub fn apply_record(kind: &str, original: &Value, projected: &Value) -> Result<Value, String> {
    let identity = projected.get("identity").ok_or("Missing host identity")?;
    let selected = project_record(kind, identity)?;
    let mut result = original.as_object().cloned().unwrap_or_default();
    let original = if original.is_object() {
        original.clone()
    } else {
        json!({})
    };
    for field in project_record(kind, &original)?
        .get("identity")
        .and_then(Value::as_object)
        .ok_or("Invalid host identity")?
        .keys()
    {
        result.remove(field);
    }
    for (field, value) in selected["identity"]
        .as_object()
        .ok_or("Invalid host identity")?
    {
        result.insert(field.clone(), value.clone());
    }
    Ok(Value::Object(result))
}

pub fn describe(kind: &str, projected: &Value) -> String {
    let identity = &projected["identity"];
    if projected.is_null() {
        return "∅".into();
    }
    if kind == "vnc_known_host" {
        return format!(
            "{}:{} · {}",
            identity["host"].as_str().unwrap_or("?"),
            identity["port"],
            identity["sha256_fingerprint"].as_str().unwrap_or("?")
        );
    }
    let key = format!(
        "{} {}",
        identity["key_type"].as_str().unwrap_or_default(),
        identity["key_base64"].as_str().unwrap_or_default()
    );
    let fingerprint = ssh_key::PublicKey::from_openssh(&key)
        .map(|key| key.fingerprint(ssh_key::HashAlg::Sha256).to_string())
        .unwrap_or_else(|_| "?".into());
    let hosts = identity["host_patterns"]
        .as_array()
        .map(|patterns| {
            patterns
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|hosts| !hosts.is_empty())
        .unwrap_or_else(|| identity["host_identifier"].as_str().unwrap_or("?").into());
    let marker = identity["marker"]
        .as_str()
        .filter(|marker| !marker.is_empty())
        .map(|marker| format!("{marker} "))
        .unwrap_or_default();
    format!(
        "{marker}{hosts} · {} · {fingerprint}",
        identity["key_type"].as_str().unwrap_or("?")
    )
}

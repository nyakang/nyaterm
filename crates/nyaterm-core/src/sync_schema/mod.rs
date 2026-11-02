//! Explicit cloud data boundary. Portable backups deliberately do not use this projection.
pub mod ai;
pub mod encryption;
pub mod secrets;
pub mod trust;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyncResourceId {
    pub kind: String,
    pub id: String,
}

impl SyncResourceId {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyncFieldId {
    pub resource: SyncResourceId,
    pub field: String,
}

impl SyncFieldId {
    pub fn new(resource: SyncResourceId, field: impl Into<String>) -> Self {
        Self {
            resource,
            field: field.into(),
        }
    }

    pub fn key(&self) -> String {
        // Identity components are JSON strings, so separators inside IDs cannot collide.
        serde_json::to_string(self).expect("serializable field identity")
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let field: Self = serde_json::from_str(key).ok()?;
        (!field.resource.kind.is_empty()
            && !field.resource.id.is_empty()
            && !field.field.is_empty()
            && field.key() == key)
            .then_some(field)
    }

    pub fn presence(resource: SyncResourceId) -> Self {
        Self::new(resource, "$present")
    }
}

/// Values and candidates must never appear in diagnostic output.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncValue(Zeroizing<Vec<u8>>);

impl Serialize for SyncValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SyncValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(serde::de::Error::custom)?;
        let canonical = Self::new(&value);
        if canonical.bytes() != bytes {
            return Err(serde::de::Error::custom("noncanonical sync value"));
        }
        Ok(canonical)
    }
}

impl SyncValue {
    pub fn new(value: &Value) -> Self {
        let mut value = value.clone();
        value.sort_all_objects();
        Self(Zeroizing::new(
            serde_json::to_vec(&value).expect("JSON value"),
        ))
    }

    pub fn decode(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_slice(&self.0)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SyncValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SyncValue(<redacted>)")
    }
}

pub type SyncValues = BTreeMap<String, SyncValue>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncScope {
    pub secrets: bool,
    pub preferences: bool,
    pub ai: bool,
    pub keybindings: bool,
    pub trusted_hosts: bool,
}

pub const CORE_KINDS: &[&str] = &[
    "connection",
    "connection_group",
    "connection_icon",
    "proxy",
    "proxy_group",
    "tunnel",
    "tunnel_group",
    "command",
    "command_category",
    "note",
    "note_folder",
    "theme",
];
pub const SECRET_KINDS: &[&str] = &[
    "key",
    "password",
    "credential",
    "otp",
    "inline_secret",
    "ai_secret",
];
pub const PREFERENCE_FIELDS: &[(&str, &str)] = &[
    ("appearance", "theme"),
    ("appearance", "font_family"),
    ("appearance", "font_size"),
    ("appearance", "font_weight"),
    ("appearance", "font_weight_bold"),
    ("appearance", "cursor_style"),
    ("appearance", "cursor_blink"),
    ("appearance", "terminal_theme"),
    ("appearance", "minimum_contrast_ratio"),
];

pub fn valid_preference(id: &str, value: &Value) -> bool {
    match id {
        "appearance/font_size" => value
            .as_u64()
            .is_some_and(|size| (1..=u16::MAX as u64).contains(&size)),
        "appearance/font_weight" | "appearance/font_weight_bold" => value
            .as_u64()
            .is_some_and(|weight| matches!(weight, 300 | 400 | 500 | 600 | 700 | 800)),
        "appearance/cursor_style" => value
            .as_str()
            .is_some_and(|style| matches!(style, "block" | "underline" | "bar")),
        "appearance/cursor_blink" => value.is_boolean(),
        "appearance/terminal_theme" => value.is_null() || value.is_string(),
        "appearance/minimum_contrast_ratio" => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
            .is_some_and(|ratio| [1.0, 3.0, 4.5, 7.0, 21.0].contains(&ratio)),
        "appearance/theme" | "appearance/font_family" => value.is_string(),
        _ => false,
    }
}

impl SyncScope {
    pub fn includes(&self, kind: &str) -> bool {
        CORE_KINDS.contains(&kind)
            || (self.secrets && SECRET_KINDS.contains(&kind))
            || (self.preferences && kind == "preference")
            || (self.ai && ai::KINDS.contains(&kind))
            || (self.keybindings && kind == "keybinding")
            || (self.trusted_hosts && matches!(kind, "known_host" | "vnc_known_host"))
    }
}

pub fn is_default_preference(resource: &SyncResourceId, value: &SyncValue) -> bool {
    let Some((section, field)) = resource.id.split_once('/') else {
        return false;
    };
    if resource.kind != "preference" || !PREFERENCE_FIELDS.contains(&(section, field)) {
        return false;
    }
    let defaults = serde_json::to_value(crate::models::settings::AppSettingsSummary::default())
        .expect("settings defaults");
    let field = match field {
        "font_family" => "terminal_font_family",
        "font_size" => "terminal_font_size",
        "font_weight" => "terminal_font_weight",
        "font_weight_bold" => "terminal_font_weight_bold",
        field => field,
    };
    defaults
        .get(field)
        .is_some_and(|default| SyncValue::new(default) == *value)
}

/// Only complete reads may establish absence. Unsupported/excluded/failed reads
/// never enter `complete`, and therefore cannot manufacture deletion operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncProjection {
    pub values: SyncValues,
    pub complete: BTreeSet<String>,
    /// Durable field edits since the effective baseline, including edits that
    /// returned to the same value. IDs contain no secret-bearing values.
    #[serde(default)]
    pub edited: BTreeSet<SyncFieldId>,
}

impl SyncProjection {
    pub fn insert(&mut self, resource: &SyncResourceId, field: &str, value: &Value) {
        self.values.insert(
            SyncFieldId::new(resource.clone(), field).key(),
            SyncValue::new(value),
        );
    }

    pub fn record(&mut self, kind: &str, id: &str, fields: &Value) -> Result<(), String> {
        if id.is_empty() || !fields.is_object() {
            return Err("Invalid sync record".into());
        }
        let resource = SyncResourceId::new(kind, id);
        if self
            .values
            .contains_key(&SyncFieldId::presence(resource.clone()).key())
        {
            return Err("Duplicate sync resource identity".into());
        }
        self.insert(&resource, "$present", &Value::Bool(true));
        for (field, value) in fields.as_object().expect("checked object") {
            if field.starts_with('$') {
                return Err("Reserved sync field".into());
            }
            self.insert(&resource, field, value);
        }
        Ok(())
    }
}

/// Field lists are deliberately allowlists: additions to a business model do
/// not silently expand the cloud data contract.
pub fn project_record(kind: &str, source: &Value) -> Result<Value, String> {
    if !source.is_object() {
        return Err("Invalid sync resource".into());
    }
    if ai::KINDS.contains(&kind) {
        return ai::project_record(kind, source);
    }
    if trust::KINDS.contains(&kind) {
        return trust::project_record(kind, source);
    }
    let fields: &[&str] = match kind {
        "connection_group" => &["name", "parent_id", "sort_order"],
        "connection_icon" => &["name", "data_url"],
        "proxy_group" | "tunnel_group" => &["name", "sort_order"],
        "command_category" => &["name", "parent_id", "sort_order"],
        "proxy" | "tunnel" => {
            let mut result = pick(source, &["name", "group_id"]);
            result["endpoint"] = pick(source, network_endpoint_fields(kind));
            if kind == "tunnel"
                && let Some(value) = source.get("auto_open")
            {
                result["auto_open"] = value.clone();
            }
            return Ok(result);
        }
        "command" => &[
            "label",
            "command",
            "description",
            "category_id",
            "sort_order",
            "color_tag",
            "icon_tag",
            "pinned",
            "execution_mode",
        ],
        "note" => &["title", "markdown", "parent_id", "sort_order"],
        "note_folder" => &["name", "parent_id", "sort_order"],
        "theme" => {
            let mut theme: crate::themes::ThemeConfig =
                serde_json::from_value(source.clone()).map_err(|_| "Unsupported theme")?;
            theme.extra.clear();
            theme.colors.extra.clear();
            theme.colors.terminal.extra.clear();
            return Ok(pick(
                &serde_json::to_value(theme).map_err(|_| "Invalid theme")?,
                &["name", "label", "swatch", "colors"],
            ));
        }
        "connection" => return project_connection(source),
        _ => return Err("Unsupported sync resource kind".into()),
    };
    Ok(pick(source, fields))
}

fn pick(source: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|field| {
                source
                    .get(*field)
                    .map(|value| ((*field).to_string(), value.clone()))
            })
            .collect(),
    )
}

fn project_connection(source: &Value) -> Result<Value, String> {
    let mut result = pick(source, &["name", "description", "tags"]);
    result["placement"] = pick(source, &["group_id", "sort_order"]);
    // The discriminator is named `type` in released persisted connections.
    let kind = source
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Connection type is missing")?;
    let endpoint_fields: &[&str] = match kind {
        "ssh" => &["type", "host", "port", "username"],
        "telnet" => &["type", "host", "port", "username"],
        "rdp" => &["type", "host", "port", "username", "domain"],
        "vnc" => &["type", "host", "port", "username"],
        "local" | "local_terminal" | "serial" => &["type"],
        _ => return Err("Unsupported connection protocol".into()),
    };
    result["endpoint"] = pick(source, endpoint_fields);
    result["terminal"] = pick(source, connection_fields("terminal"));
    for name in ["sftp", "ssh_algorithms"] {
        if let Some(value) = source.get(name).filter(|value| value.is_object()) {
            result[name] = pick(value, connection_fields(name));
        }
    }
    if kind == "telnet" {
        result["telnet"] = pick(source, connection_fields("telnet"));
        if let Some(auto_login) = source.get("auto_login").filter(|value| value.is_object()) {
            result["auto_login"] = pick(auto_login, connection_fields("auto_login"));
        }
    }
    if let Some(post_login) = source.get("post_login").filter(|value| !value.is_null()) {
        result["post_login"] = pick(post_login, &["enabled", "command", "delay_ms"]);
    }
    if kind == "serial" {
        result["serial"] = pick(
            source,
            &[
                "baud_rate",
                "data_bits",
                "parity",
                "stop_bits",
                "flow_control",
            ],
        );
    }
    if let Some(asset) = source.get("asset") {
        let portable = pick(asset, &["tags", "notes"]);
        if portable
            .as_object()
            .is_some_and(|fields| !fields.is_empty())
        {
            result["asset"] = portable;
        }
    }
    // Automatically detected icons must not turn monitoring into a user edit.
    let detect_icon = source
        .get("icon_auto_detect")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            source
                .get("icon")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        });
    if !detect_icon {
        result["icon"] = pick(source, &["icon", "icon_auto_detect"]);
    }
    // Local credential material and agent endpoints are never embedded here.
    if let Some(auth) = source.get("auth").filter(|value| !value.is_null()) {
        result["authentication"] = pick(
            auth,
            &[
                "mode",
                "key_id",
                "password_id",
                "account_id",
                "password_source",
                "username_source",
                "otp_id",
                "auto_fill_otp",
            ],
        );
    }
    if let Some(network) = source.get("network").filter(|value| !value.is_null()) {
        result["network"] = pick(network, &["proxy_id", "proxy_jump_id", "host_key_alias"]);
    }
    Ok(result)
}

pub fn apply_record(kind: &str, original: &Value, projected: &Value) -> Result<Value, String> {
    if trust::KINDS.contains(&kind) {
        return trust::apply_record(kind, original, projected);
    }
    if ai::KINDS.contains(&kind) {
        return ai::apply_record(kind, original, projected);
    }
    let mut result = if original.is_object() {
        original.clone()
    } else {
        serde_json::json!({})
    };
    let Some(fields) = projected.as_object() else {
        return Err("Invalid sync projection".into());
    };
    // Remove the previous portable fields before overlaying. An absent field
    // represents an explicit field tombstone, rather than retaining a stale value.
    if original.is_object() {
        for field in project_record(kind, original)?
            .as_object()
            .expect("projection object")
            .keys()
        {
            if matches!(kind, "proxy" | "tunnel") && field == "endpoint" {
                if let Some(object) = result.as_object_mut() {
                    for key in network_endpoint_fields(kind) {
                        object.remove(*key);
                    }
                }
            } else if kind == "connection" {
                clear_connection_field(&mut result, field);
            } else if kind != "theme" || field != "colors" {
                result.as_object_mut().expect("object").remove(field);
            }
        }
    }
    for (field, value) in fields {
        if !supports_field(kind, field) {
            continue;
        }
        if matches!(kind, "proxy" | "tunnel") && field == "endpoint" {
            if !value.is_object() {
                return Err("Invalid network endpoint group".into());
            }
            for (key, value) in pick(value, network_endpoint_fields(kind))
                .as_object()
                .expect("endpoint object")
            {
                result[key] = value.clone();
            }
        } else if kind == "connection"
            && matches!(
                field.as_str(),
                "endpoint" | "placement" | "terminal" | "serial" | "icon" | "telnet"
            )
        {
            let Some(group) = value.as_object() else {
                return Err("Invalid connection field group".into());
            };
            for (key, value) in group {
                if connection_fields(field).contains(&key.as_str()) {
                    result[key] = value.clone();
                }
            }
        } else if kind == "connection"
            && matches!(
                field.as_str(),
                "asset" | "authentication" | "network" | "sftp" | "ssh_algorithms" | "auto_login"
            )
        {
            let name = if field == "authentication" {
                "auth"
            } else {
                field
            };
            if !result.get(name).is_some_and(Value::is_object) {
                result[name] = serde_json::json!({});
            }
            let Some(group) = value.as_object() else {
                return Err("Invalid connection field group".into());
            };
            for (key, value) in group {
                if connection_fields(field).contains(&key.as_str()) {
                    result[name][key] = value.clone();
                }
            }
        } else if kind == "connection" && field == "post_login" {
            let before = original
                .get(field)
                .map(|value| pick(value, &["enabled", "command", "delay_ms"]))
                .unwrap_or_else(|| serde_json::json!({}));
            let after = pick(value, &["enabled", "command", "delay_ms"]);
            result[field] =
                overlay_classified(original.get(field).unwrap_or(&Value::Null), &before, &after);
        } else if kind == "theme" && field == "colors" {
            let mut colors: crate::themes::ThemeColors =
                serde_json::from_value(value.clone()).map_err(|_| "Invalid theme colors")?;
            colors.extra.clear();
            colors.terminal.extra.clear();
            let after = serde_json::to_value(colors).map_err(|_| "Invalid theme colors")?;
            let before = if original.is_object() {
                project_record(kind, original)?[field].clone()
            } else {
                serde_json::json!({})
            };
            result[field] =
                overlay_classified(original.get(field).unwrap_or(&Value::Null), &before, &after);
        } else {
            result[field] = value.clone();
        }
    }
    Ok(result)
}

/// Unknown registers remain in the causal document but never become business
/// fields or local deletion observations on a client that cannot classify them.
pub fn supports_field(kind: &str, field: &str) -> bool {
    if trust::KINDS.contains(&kind) {
        return matches!(field, "$present" | "identity");
    }
    if SECRET_KINDS.contains(&kind) {
        return matches!(field, "$present" | "name" | "sort_order" | "value");
    }
    if ai::KINDS.contains(&kind) {
        return field == "$present" || ai::fields(kind).contains(&field);
    }
    if field == "$present" {
        return CORE_KINDS.contains(&kind) || matches!(kind, "preference" | "keybinding");
    }
    if kind == "connection" {
        return matches!(
            field,
            "name"
                | "description"
                | "tags"
                | "endpoint"
                | "placement"
                | "terminal"
                | "serial"
                | "icon"
                | "asset"
                | "authentication"
                | "network"
                | "post_login"
                | "sftp"
                | "ssh_algorithms"
                | "telnet"
                | "auto_login"
        );
    }
    if matches!(kind, "preference" | "keybinding") {
        return field == "value";
    }
    if matches!(kind, "proxy" | "tunnel") {
        return matches!(field, "name" | "group_id" | "endpoint")
            || (kind == "tunnel" && field == "auto_open");
    }
    if kind == "theme" {
        return matches!(field, "name" | "label" | "swatch" | "colors");
    }
    let source = serde_json::json!({field: true});
    project_record(kind, &source)
        .ok()
        .is_some_and(|value| value.get(field).is_some())
}

fn network_endpoint_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "proxy" => &["protocol", "host", "port", "username", "password_id"],
        "tunnel" => &[
            "tunnel_type",
            "connection_id",
            "listen_port",
            "target_host",
            "target_port",
            "bind_localhost",
        ],
        _ => &[],
    }
}

fn overlay_classified(original: &Value, before: &Value, after: &Value) -> Value {
    let Some(after) = after.as_object() else {
        return after.clone();
    };
    let mut result = original.as_object().cloned().unwrap_or_default();
    if let Some(before) = before.as_object() {
        for key in before.keys() {
            if !after.contains_key(key) {
                result.remove(key);
            }
        }
    }
    for (key, value) in after {
        result.insert(
            key.clone(),
            overlay_classified(
                original.get(key).unwrap_or(&Value::Null),
                before.get(key).unwrap_or(&Value::Null),
                value,
            ),
        );
    }
    Value::Object(result)
}

fn connection_fields(field: &str) -> &'static [&'static str] {
    match field {
        "endpoint" => &["type", "host", "port", "username", "domain"],
        "placement" => &["group_id", "sort_order"],
        "terminal" => &[
            "encoding",
            "backspace_mode",
            "terminal_type",
            "ssh_profile",
            "ai_execution_profile",
            "dynamic_tab_title",
        ],
        "sftp" => &[
            "compatibility_mode",
            "enabled",
            "cwd_follow_mode",
            "filename_encoding",
        ],
        "ssh_algorithms" => &["mode", "kex", "ciphers", "macs", "host_keys"],
        "telnet" => &[
            "raw_tcp_cli",
            "enter_mode",
            "local_echo",
            "local_line_edit",
            "force_character_at_a_time",
            "send_naws",
            "send_sga",
        ],
        "auto_login" => &[
            "enabled",
            "send_wake_enter",
            "timeout_ms",
            "username_prompt_regex",
            "password_prompt_regex",
            "success_prompt_regex",
            "failure_prompt_regex",
            "max_retries",
        ],
        "serial" => &[
            "baud_rate",
            "data_bits",
            "parity",
            "stop_bits",
            "flow_control",
        ],
        "icon" => &["icon", "icon_auto_detect"],
        "asset" => &["tags", "notes"],
        "authentication" => &[
            "mode",
            "key_id",
            "password_id",
            "account_id",
            "password_source",
            "username_source",
            "otp_id",
            "auto_fill_otp",
        ],
        "network" => &["proxy_id", "proxy_jump_id", "host_key_alias"],
        "post_login" => &["enabled", "command", "delay_ms"],
        _ => &[],
    }
}

fn clear_connection_field(result: &mut Value, field: &str) {
    let nested = match field {
        "asset" | "network" | "post_login" | "sftp" | "ssh_algorithms" | "auto_login" => {
            Some(field)
        }
        "authentication" => Some("auth"),
        _ => None,
    };
    let target = if let Some(name) = nested {
        result.get_mut(name)
    } else {
        Some(result)
    };
    if let Some(object) = target.and_then(Value::as_object_mut) {
        if connection_fields(field).is_empty() {
            object.remove(field);
        } else {
            for key in connection_fields(field) {
                object.remove(*key);
            }
        }
    }
}

/// Migration inputs are decoded by the existing compatibility readers first.
/// Missing legacy entities are unsupported, not empty collections.
pub fn project_entities(
    entities: &BTreeMap<String, String>,
    scope: &SyncScope,
) -> Result<SyncProjection, String> {
    let mut result = SyncProjection::default();
    for (entity, sections) in [
        (
            "sessions",
            vec![
                ("connections", "connection"),
                ("groups", "connection_group"),
                ("custom_icons", "connection_icon"),
            ],
        ),
        ("proxies", vec![("", "proxy")]),
        ("proxy_groups", vec![("", "proxy_group")]),
        ("tunnels", vec![("", "tunnel")]),
        ("tunnel_groups", vec![("", "tunnel_group")]),
        (
            "quick_commands",
            vec![("commands", "command"), ("categories", "command_category")],
        ),
        ("notes", vec![("notes", "note"), ("folders", "note_folder")]),
    ] {
        let Some(raw) = entities.get(entity) else {
            continue;
        };
        let value: Value = serde_json::from_str(raw).map_err(|_| "Invalid sync collection")?;
        for (section, kind) in sections {
            let collection = if section.is_empty() {
                Some(&value)
            } else {
                value.get(section)
            };
            let Some(collection) = collection else {
                continue;
            };
            let records = collection.as_array().ok_or("Invalid sync collection")?;
            result.complete.insert(kind.into());
            for record in records {
                let id = record
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Missing resource identity")?;
                result.record(kind, id, &project_record(kind, record)?)?;
            }
        }
    }
    if let Some(raw) = entities.get("settings") {
        let settings: Value = serde_json::from_str(raw).map_err(|_| "Invalid sync settings")?;
        if scope.ai {
            ai::project_settings(&settings, &mut result)?;
        }
        if let Some(themes) = settings
            .pointer("/appearance/custom_themes")
            .and_then(Value::as_array)
        {
            result.complete.insert("theme".into());
            for theme in themes {
                // Theme catalogs intentionally preserve opaque future entries.
                if serde_json::from_value::<crate::themes::ThemeConfig>(theme.clone()).is_err() {
                    // An unsupported record is not evidence that a formerly
                    // readable resource was deleted by the user.
                    result.complete.remove("theme");
                    continue;
                }
                let id = theme
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("Invalid theme identity")?;
                result.record("theme", id, &project_record("theme", theme)?)?;
            }
        }
        if scope.preferences {
            result.complete.insert("preference".into());
            for (section, field) in PREFERENCE_FIELDS {
                if let Some(value) = settings.get(section).and_then(|value| value.get(field)) {
                    result.record(
                        "preference",
                        &format!("{section}/{field}"),
                        &serde_json::json!({"value":value}),
                    )?;
                }
            }
        }
        if scope.keybindings {
            result.complete.insert("keybinding".into());
            if let Some(bindings) = settings.get("keybindings") {
                let bindings = bindings.as_object().ok_or("Invalid keybinding catalog")?;
                for (action, binding) in bindings {
                    if !binding.is_string() {
                        // Preserve an opaque future or malformed entry locally;
                        // it cannot establish absence for this category.
                        result.complete.remove("keybinding");
                        continue;
                    }
                    result.record("keybinding", action, &serde_json::json!({"value":binding}))?;
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

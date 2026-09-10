use serde::{Deserialize, Serialize};

use super::{
    AssetMetadata, ConnectionAuth, ConnectionNetwork, ConnectionType, RecordingMode,
    RecordingRotationPolicy, SftpSettings, SshAlgorithmPreferences, SshProfile, SshTerminalType,
    default_post_login_delay_ms, is_default_sftp_settings, is_standard_ssh_profile, uuid_v4,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConnectionPostLogin {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default = "default_post_login_delay_ms")]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConnectionRecordingSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RecordingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_timestamps: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RecordingRotationPolicy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    pub config: ConnectionType,
    pub group_id: Option<String>,
    pub description: Option<String>,
    pub sort_order: i32,
    pub icon: Option<String>,
    /// Whether `icon` may be replaced by one detected from the remote system.
    ///
    /// `None` means "not configured", which reads as enabled only while no icon
    /// has been chosen — see [`SavedConnection::icon_auto_detect_enabled`]. Kept
    /// as an `Option` and skipped when empty so files round-trip unchanged
    /// through builds that predate the field.
    pub icon_auto_detect: Option<bool>,
    pub auth: Option<ConnectionAuth>,
    pub network: Option<ConnectionNetwork>,
    pub post_login: Option<ConnectionPostLogin>,
    pub recording: Option<ConnectionRecordingSettings>,
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
    pub ssh_profile: SshProfile,
    pub terminal_type: Option<SshTerminalType>,
    pub sftp: SftpSettings,
    /// Static asset facts (hardware, OS, tags) mirrored from the Tauri contract.
    ///
    /// Skipped when absent so connections written by builds predating the field
    /// round-trip unchanged.
    pub asset: Option<AssetMetadata>,
    pub created_at_ms: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub last_used_at_ms: Option<u64>,
    pub extensions: ConnectionExtensions,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "SavedConnection")]
struct SavedConnectionKnown {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub config: ConnectionType,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub icon: Option<String>,
    /// Whether `icon` may be replaced by one detected from the remote system.
    ///
    /// `None` means "not configured", which reads as enabled only while no icon
    /// has been chosen — see [`SavedConnection::icon_auto_detect_enabled`]. Kept
    /// as an `Option` and skipped when empty so files round-trip unchanged
    /// through builds that predate the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_auto_detect: Option<bool>,
    #[serde(default)]
    pub auth: Option<ConnectionAuth>,
    #[serde(default)]
    pub network: Option<ConnectionNetwork>,
    #[serde(default)]
    pub post_login: Option<ConnectionPostLogin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<ConnectionRecordingSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
    #[serde(default, skip_serializing_if = "is_standard_ssh_profile")]
    pub ssh_profile: SshProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_type: Option<SshTerminalType>,
    #[serde(default, skip_serializing_if = "is_default_sftp_settings")]
    pub sftp: SftpSettings,
    /// Static asset facts (hardware, OS, tags) mirrored from the Tauri contract.
    ///
    /// Skipped when absent so connections written by builds predating the field
    /// round-trip unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<u64>,
    #[serde(skip)]
    pub extensions: ConnectionExtensions,
}

impl SavedConnection {
    /// Whether the icon may be replaced by one inferred from the remote system.
    ///
    /// An unset flag defaults to "yes, until the user picks something", which is
    /// what keeps auto-detection from ever overwriting a deliberate choice made
    /// before this field existed.
    pub fn icon_auto_detect_enabled(&self) -> bool {
        self.icon_auto_detect
            .unwrap_or_else(|| self.icon.as_deref().is_none_or(str::is_empty))
    }

    pub fn kind_label(&self) -> &'static str {
        match self.config {
            ConnectionType::Ssh { .. } => "SSH",
            ConnectionType::LocalTerminal { .. } => "Local",
            ConnectionType::Telnet { .. } => "Telnet",
            ConnectionType::Serial { .. } => "Serial",
            ConnectionType::Rdp { .. } => "RDP",
            ConnectionType::Vnc { .. } => "VNC",
        }
    }

    pub fn endpoint(&self) -> String {
        match &self.config {
            ConnectionType::Ssh {
                host,
                port,
                username,
                ..
            } => format!("{username}@{host}:{port}"),
            ConnectionType::LocalTerminal {
                shell_path,
                working_dir,
                ..
            } => {
                let shell = if shell_path.is_empty() {
                    "system shell"
                } else {
                    shell_path
                };
                match working_dir {
                    Some(dir) if !dir.is_empty() => format!("{shell} in {dir}"),
                    _ => shell.to_string(),
                }
            }
            ConnectionType::Telnet { host, port, .. } => format!("{host}:{port}"),
            ConnectionType::Serial {
                port_name,
                baud_rate,
                ..
            } => format!("{port_name} @ {baud_rate}"),
            ConnectionType::Rdp {
                host,
                port,
                username,
                domain,
                ..
            } => {
                let account = if username.is_empty() {
                    String::new()
                } else if domain.is_empty() {
                    format!("{username}@")
                } else {
                    format!("{domain}\\{username}@")
                };
                format!("{account}{host}:{port}")
            }
            ConnectionType::Vnc { host, port, .. } => format!("{host}:{port}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionsConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_icons: Vec<ConnectionCustomIcon>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    #[serde(alias = "sessions")]
    pub connections: Vec<SavedConnection>,
}

/// The shared Tauri-compatible icon library; connections reference an entry through `icon`.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionCustomIcon {
    pub id: String,
    pub name: String,
    pub data_url: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl std::fmt::Debug for ConnectionCustomIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionCustomIcon")
            .field("id", &self.id)
            .field("data_url", &"[IMAGE]")
            .finish_non_exhaustive()
    }
}

/// Unrecognised connection properties survive edits and portable round trips.
/// Debug never prints opaque fields, which may contain future secret values.
#[derive(Clone, PartialEq)]
pub struct ConnectionExtensions(serde_json::Value);

impl Default for ConnectionExtensions {
    fn default() -> Self {
        Self(serde_json::Value::Object(Default::default()))
    }
}

impl std::fmt::Debug for ConnectionExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[CONNECTION EXTENSIONS]")
    }
}

impl Serialize for SavedConnection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut known = SavedConnectionKnown::serialize(self, serde_json::value::Serializer)
            .map_err(serde::ser::Error::custom)?;
        merge_extensions(&mut known, &self.extensions.0);
        known.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SavedConnection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let mut connection =
            SavedConnectionKnown::deserialize(raw.clone()).map_err(serde::de::Error::custom)?;
        let known = SavedConnectionKnown::serialize(&connection, serde_json::value::Serializer)
            .map_err(serde::de::Error::custom)?;
        let mut extensions = unknown_properties(&raw, &known, "");
        if let Some(fields) = extensions.as_object_mut() {
            // These input-only aliases are handled by the established agent migration.
            fields.remove("agent_endpoint");
            fields.remove("agent_forwarding");
        }
        connection.extensions = ConnectionExtensions(extensions);
        Ok(connection)
    }
}

/// Known fields that serde may omit at their default value. They must not be
/// mistaken for extensions, or clearing an option would resurrect its old value.
fn omitted_fields(path: &str) -> &'static [&'static str] {
    match path {
        "" => &[
            "icon_auto_detect",
            "recording",
            "ssh_algorithms",
            "ssh_profile",
            "terminal_type",
            "sftp",
            "asset",
            "created_at_ms",
            "updated_at_ms",
            "last_used_at_ms",
            "dynamic_tab_title",
            "auth_agent_endpoint",
            "agent_endpoint",
            "agent_forwarding_config",
            "agent_forwarding",
            "auto_login",
            "encoding",
        ],
        "/sftp" => &[
            "enabled",
            "cwd_follow_mode",
            "shell_detection_timeout_ms",
            "filename_encoding",
            "pipeline_depth",
        ],
        "/recording" => &[
            "auto_start",
            "mode",
            "path_template",
            "include_timestamps",
            "rotation",
        ],
        "/ssh_algorithms" => &["mode", "kex", "ciphers", "macs", "host_keys"],
        "/agent_forwarding_config/sources" => {
            &["external_agent", "external_agent_endpoints", "stored_keys"]
        }
        "/auto_login" => &[
            "enabled",
            "send_wake_enter",
            "timeout_ms",
            "username_prompt_regex",
            "password_prompt_regex",
            "success_prompt_regex",
            "failure_prompt_regex",
            "max_retries",
        ],
        "/asset" => &[
            "device_type",
            "os_name",
            "os_version",
            "architecture",
            "kernel_version",
            "hostname",
            "cpu_model",
            "cpu_sockets",
            "cpu_cores",
            "cpu_threads",
            "memory_bytes",
            "accelerators",
            "disks",
            "tags",
            "notes",
            "updated_at",
        ],
        _ => &[],
    }
}

fn unknown_properties(
    raw: &serde_json::Value,
    known: &serde_json::Value,
    path: &str,
) -> serde_json::Value {
    let mut unknown = serde_json::Map::new();
    if let Some(raw) = raw.as_object() {
        for (key, value) in raw {
            let recognized =
                known.get(key).is_some() || omitted_fields(path).contains(&key.as_str());
            if !recognized {
                unknown.insert(key.clone(), value.clone());
            } else if value.is_object() {
                let nested_path = format!("{path}/{key}");
                let nested = unknown_properties(
                    value,
                    known.get(key).unwrap_or(&serde_json::Value::Null),
                    &nested_path,
                );
                if nested.as_object().is_some_and(|object| !object.is_empty()) {
                    unknown.insert(key.clone(), nested);
                }
            }
        }
    }
    serde_json::Value::Object(unknown)
}

fn merge_extensions(known: &mut serde_json::Value, extensions: &serde_json::Value) {
    if let (Some(known), Some(extensions)) = (known.as_object_mut(), extensions.as_object()) {
        for (key, value) in extensions {
            match known.get_mut(key) {
                Some(current) => merge_extensions(current, value),
                None => {
                    known.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

impl ConnectionCustomIcon {
    /// Legacy embedded raster icons use a content-addressed library identifier.
    pub fn from_legacy_data_url(value: &str, name: String, now_ms: u64) -> Option<Self> {
        use sha2::{Digest as _, Sha256};
        let value = value.trim();
        let (header, payload) = value.split_once(',')?;
        if !matches!(
            header.to_ascii_lowercase().as_str(),
            "data:image/png;base64"
                | "data:image/jpeg;base64"
                | "data:image/jpg;base64"
                | "data:image/webp;base64"
                | "data:image/bmp;base64"
                | "data:image/gif;base64"
        ) || payload.is_empty()
            || !payload
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        {
            return None;
        }
        Some(Self {
            id: format!(
                "custom-icon-{}",
                hex::encode(Sha256::digest(value.as_bytes()))
            ),
            name,
            data_url: value.to_string(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

#[cfg(test)]
mod extension_tests {
    use super::SavedConnection;
    #[test]
    fn defaulted_known_fields_do_not_become_extensions() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id":"c", "name":"c", "type":"ssh", "host":"host", "dynamic_tab_title":false,
            "icon_auto_detect":null, "sftp":{"pipeline_depth":null,"filename_encoding":"","unknown":true},
            "recording":{"auto_start":null,"future":5}, "asset":{"cpu_model":null,"future":7}
        })).unwrap();
        connection.sftp.pipeline_depth = None;
        let serialized = serde_json::to_value(&connection).unwrap();
        assert!(serialized.get("dynamic_tab_title").is_none());
        assert!(serialized.get("icon_auto_detect").is_none());
        assert!(serialized["sftp"].get("pipeline_depth").is_none());
        assert!(serialized["recording"].get("auto_start").is_none());
        assert_eq!(serialized["recording"]["future"], 5);
        assert_eq!(serialized["asset"]["future"], 7);
    }
}

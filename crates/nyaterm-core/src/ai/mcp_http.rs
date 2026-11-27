//! Local HTTP MCP configuration. Runtime credentials never belong in settings.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum McpTransport {
    #[default]
    Stdio,
    StreamableHttp,
    Unknown(String),
}
impl McpTransport {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable_http",
            Self::Unknown(value) => value,
        }
    }
}
impl From<String> for McpTransport {
    fn from(value: String) -> Self {
        match value.as_str() {
            "stdio" => Self::Stdio,
            "streamable_http" => Self::StreamableHttp,
            _ => Self::Unknown(value),
        }
    }
}
impl From<McpTransport> for String {
    fn from(value: McpTransport) -> Self {
        value.as_str().to_owned()
    }
}
pub const fn default_http_port() -> u16 {
    8002
}

pub fn normalize_origin(value: &str) -> Result<String, &'static str> {
    let url = url::Url::parse(value).map_err(|_| "Invalid browser origin")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains('*')
    {
        return Err("Use an exact HTTP or HTTPS origin without a path, credentials or wildcard");
    }
    Ok(url.origin().ascii_serialization())
}

pub fn validate_http_settings(port: u16, origins: &[String]) -> Result<Vec<String>, &'static str> {
    if port == 0 {
        return Err("HTTP port must be between 1 and 65535");
    }
    if origins.len() > 64 {
        return Err("Too many browser origins");
    }
    let mut normalized = origins
        .iter()
        .map(|origin| normalize_origin(origin))
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

/// This result contains a secret and is only for explicit copy/export actions.
pub fn client_config(client: &str, port: u16, token: &str) -> Result<String, &'static str> {
    let url = format!("http://127.0.0.1:{port}/mcp");
    let authorization = format!("Bearer {token}");
    if client == "codex" {
        return Ok(format!(
            "[mcp_servers.nyaterm]\nurl = {url:?}\nhttp_headers = {{ Authorization = {authorization:?} }}\n"
        ));
    }
    let server = match client {
        "claude" => {
            serde_json::json!({"type":"http", "url":url,"headers":{"Authorization":authorization}})
        }
        "cursor" | "generic" => {
            serde_json::json!({"url":url,"headers":{"Authorization":authorization}})
        }
        _ => return Err("Unsupported MCP client"),
    };
    serde_json::to_string_pretty(&if client == "generic" {
        server
    } else {
        serde_json::json!({"mcpServers":{"nyaterm":server}})
    })
    .map_err(|_| "Cannot format MCP configuration")
}

#[cfg(test)]
mod tests {
    use super::{McpTransport, normalize_origin, validate_http_settings};
    use crate::ai::ExternalMcpSettings;
    #[test]
    fn legacy_settings_and_future_fields_survive_round_trips() {
        let mut value: serde_json::Value =
            serde_json::from_str(r#"{"enabled":true,"future":{"a":1}}"#).unwrap();
        let settings: ExternalMcpSettings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(settings.transport, McpTransport::Stdio);
        assert_eq!(settings.http_port, 8002);
        assert_eq!(
            serde_json::to_value(settings).unwrap()["future"],
            value["future"]
        );
        value["transport"] = "future_transport".into();
        let settings: ExternalMcpSettings = serde_json::from_value(value).unwrap();
        assert!(matches!(settings.transport, McpTransport::Unknown(_)));
        assert_eq!(
            serde_json::to_value(settings).unwrap()["transport"],
            "future_transport"
        );
    }
    #[test]
    fn origin_validation_rejects_ambiguous_or_unsafe_entries() {
        for value in [
            "null",
            "*",
            "https://*.example.com",
            "https://a.example/path",
            "https://u:p@a.example",
            "file:///tmp/x",
            "https://a.example?x",
        ] {
            assert!(normalize_origin(value).is_err(), "{value}");
        }
        assert_eq!(
            normalize_origin("https://EXAMPLE.com:443/").unwrap(),
            "https://example.com"
        );
        assert!(validate_http_settings(0, &[]).is_err());
    }
    #[test]
    fn supported_settings_fixtures_keep_stdio_and_http_settings_round_trip() {
        for fixture in [
            include_str!("../../tests/fixtures/ai/settings_v3_tauri.json"),
            include_str!("../../tests/fixtures/ai/settings_v4_tauri.json"),
            include_str!("../../tests/fixtures/ai/settings_v5_tauri.json"),
            include_str!("../../tests/fixtures/ai/settings_v6_tauri.json"),
        ] {
            let mut settings: crate::ai::AiSettings = serde_json::from_str(fixture).unwrap();
            assert_eq!(settings.external_mcp.transport, McpTransport::Stdio);
            assert_eq!(settings.external_mcp.http_port, 8002);
            settings.external_mcp.transport = McpTransport::StreamableHttp;
            settings.external_mcp.http_port = 9002;
            settings.external_mcp.http_allowed_origins = vec!["https://client.example".into()];
            let encoded = serde_json::to_value(&settings).unwrap();
            assert!(encoded["external_mcp"].get("token").is_none());
            let decoded: crate::ai::AiSettings = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded.external_mcp, settings.external_mcp);
        }
    }
    #[test]
    fn client_configuration_uses_each_clients_native_format() {
        let config = super::client_config("codex", 8002, "fixture").unwrap();
        let value: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(
            value["mcp_servers"]["nyaterm"]["http_headers"]["Authorization"].as_str(),
            Some("Bearer fixture")
        );
        for client in ["claude", "cursor", "generic"] {
            let value: serde_json::Value =
                serde_json::from_str(&super::client_config(client, 8002, "fixture").unwrap())
                    .unwrap();
            let server = if client == "generic" {
                &value
            } else {
                &value["mcpServers"]["nyaterm"]
            };
            assert_eq!(server["url"], "http://127.0.0.1:8002/mcp");
            assert_eq!(server["headers"]["Authorization"], "Bearer fixture");
        }
    }
}

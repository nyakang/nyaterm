//! Client-specific configuration exports. Real credentials are supplied only for copying.
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ExternalMcpSettings, mcp_http::McpTransport};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum McpClient {
    #[default]
    Codex,
    Claude,
    Cursor,
    VsCode,
    Windsurf,
    OpenCode,
    CherryStudio,
    Generic,
    Unknown(String),
}

impl McpClient {
    pub const SUPPORTED: [Self; 8] = [
        Self::Codex,
        Self::Claude,
        Self::Cursor,
        Self::VsCode,
        Self::Windsurf,
        Self::OpenCode,
        Self::CherryStudio,
        Self::Generic,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::VsCode => "vscode",
            Self::Windsurf => "windsurf",
            Self::OpenCode => "opencode",
            Self::CherryStudio => "cherry",
            Self::Generic => "generic",
            Self::Unknown(value) => value,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code",
            Self::Windsurf => "Windsurf",
            Self::OpenCode => "OpenCode",
            Self::CherryStudio => "Cherry Studio",
            Self::Generic => "HTTP (JSON)",
            Self::Unknown(_) => "—",
        }
    }

    pub fn supports(&self, transport: &McpTransport) -> bool {
        !matches!(self, Self::Unknown(_))
            && match transport {
                McpTransport::Stdio => !matches!(self, Self::Generic),
                McpTransport::StreamableHttp => true,
                McpTransport::Unknown(_) => false,
            }
    }
}

impl From<String> for McpClient {
    fn from(value: String) -> Self {
        Self::SUPPORTED
            .into_iter()
            .find(|client| client.as_str() == value)
            .unwrap_or(Self::Unknown(value))
    }
}
impl From<McpClient> for String {
    fn from(value: McpClient) -> Self {
        value.as_str().to_owned()
    }
}

// Intentionally no Debug: the HTTP input can carry a live bearer credential.
pub enum ConfigTransport<'a> {
    Stdio { command: &'a str },
    Http { port: u16, token: &'a str },
}

pub fn client_config(
    client: &McpClient,
    transport: ConfigTransport<'_>,
) -> Result<String, &'static str> {
    let mut server = match transport {
        ConfigTransport::Stdio { command } => {
            if !client.supports(&McpTransport::Stdio) {
                return Err("Unsupported MCP client");
            }
            match client {
                McpClient::OpenCode => json!({"type":"local", "command":[command], "enabled":true}),
                McpClient::VsCode => json!({"type":"stdio", "command":command,"args":[]}),
                McpClient::CherryStudio => {
                    json!({"type":"stdio", "name":"nyaterm", "command":command,"args":[]})
                }
                _ => json!({"command":command,"args":[]}),
            }
        }
        ConfigTransport::Http { port, token } => {
            if !client.supports(&McpTransport::StreamableHttp) {
                return Err("Unsupported MCP client");
            }
            let url = format!("http://127.0.0.1:{port}/mcp");
            let headers = json!({"Authorization":format!("Bearer {token}")});
            match client {
                McpClient::Codex => json!({"url":url,"http_headers":headers}),
                McpClient::Claude | McpClient::VsCode => {
                    json!({"type":"http","url":url,"headers":headers})
                }
                McpClient::Windsurf => json!({"serverUrl":url,"headers":headers}),
                McpClient::OpenCode => {
                    json!({"type":"remote","url":url,"headers":headers,"enabled":true,"oauth":false})
                }
                McpClient::CherryStudio => {
                    json!({"type":"streamableHttp","name":"nyaterm","baseUrl":url,"headers":headers})
                }
                _ => json!({"url":url,"headers":headers}),
            }
        }
    };
    if *client == McpClient::Codex {
        return toml::to_string_pretty(&json!({"mcp_servers":{"nyaterm":server}}))
            .map_err(|_| "Cannot format MCP configuration");
    }
    let root = match client {
        McpClient::VsCode => "servers",
        McpClient::OpenCode => "mcp",
        _ => "mcpServers",
    };
    if *client != McpClient::Generic {
        server = json!({root:{"nyaterm":server}});
    }
    serde_json::to_string_pretty(&server).map_err(|_| "Cannot format MCP configuration")
}

impl ExternalMcpSettings {
    pub fn selected_client(&self) -> &McpClient {
        match self.transport {
            McpTransport::StreamableHttp => &self.http_client,
            _ => &self.stdio_client,
        }
    }

    /// Presentation preferences must not rotate credentials or interrupt jobs.
    pub fn runtime_eq(&self, other: &Self) -> bool {
        self.transport == other.transport
            && self.http_port == other.http_port
            && self.http_allowed_origins == other.http_allowed_origins
            && self.enabled == other.enabled
            && self.permission_mode == other.permission_mode
            && self.session_scope == other.session_scope
            && self.terminal_display_mode == other.terminal_display_mode
            && self.terminal_execution_mode == other.terminal_execution_mode
            && self.extra == other.extra
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigTransport, McpClient, client_config};
    use crate::ai::{ExternalMcpSettings, mcp_http::McpTransport};
    use serde_json::{Value, json};

    #[test]
    fn every_client_export_round_trips_paths_and_http_credentials() {
        for client in McpClient::SUPPORTED {
            for path in [
                r#"C:\Users\猫 name\"quoted"\nyaterm-mcp.exe"#,
                "/tmp/猫 name/nyaterm-mcp",
            ] {
                let config = client_config(&client, ConfigTransport::Stdio { command: path });
                if client == McpClient::Generic {
                    assert!(config.is_err());
                    continue;
                }
                let config = config.unwrap();
                if client == McpClient::Codex {
                    let doc: toml::Value = toml::from_str(&config).unwrap();
                    assert_eq!(
                        doc["mcp_servers"]["nyaterm"]["command"].as_str(),
                        Some(path)
                    );
                } else {
                    let doc: Value = serde_json::from_str(&config).unwrap();
                    let root = match client {
                        McpClient::VsCode => "servers",
                        McpClient::OpenCode => "mcp",
                        _ => "mcpServers",
                    };
                    let command = &doc[root]["nyaterm"]["command"];
                    assert_eq!(
                        if client == McpClient::OpenCode {
                            &command[0]
                        } else {
                            command
                        },
                        path
                    );
                }
            }
            let config = client_config(
                &client,
                ConfigTransport::Http {
                    port: 8123,
                    token: "fixture\"猫\\token",
                },
            )
            .unwrap();
            if client == McpClient::Codex {
                let doc: toml::Value = toml::from_str(&config).unwrap();
                assert_eq!(
                    doc["mcp_servers"]["nyaterm"]["http_headers"]["Authorization"].as_str(),
                    Some("Bearer fixture\"猫\\token")
                );
            } else {
                let doc: Value = serde_json::from_str(&config).unwrap();
                let server = match client {
                    McpClient::Generic => &doc,
                    McpClient::VsCode => &doc["servers"]["nyaterm"],
                    McpClient::OpenCode => &doc["mcp"]["nyaterm"],
                    _ => &doc["mcpServers"]["nyaterm"],
                };
                let url_key = match client {
                    McpClient::Windsurf => "serverUrl",
                    McpClient::CherryStudio => "baseUrl",
                    _ => "url",
                };
                assert_eq!(server[url_key], "http://127.0.0.1:8123/mcp");
                assert_eq!(
                    server["headers"]["Authorization"],
                    "Bearer fixture\"猫\\token"
                );
            }
        }
    }

    #[test]
    fn old_and_future_preferences_round_trip_without_runtime_changes() {
        let old: ExternalMcpSettings =
            serde_json::from_value(json!({"enabled":true,"future":42})).unwrap();
        assert_eq!(old.selected_client(), &McpClient::Codex);
        let mut next = old.clone();
        next.stdio_client = McpClient::VsCode;
        next.http_client = McpClient::Unknown("future_client".into());
        assert!(next.runtime_eq(&old));
        let value = serde_json::to_value(&next).unwrap();
        assert_eq!(value["future"], 42);
        assert_eq!(value["http_client"], "future_client");
        assert_eq!(
            serde_json::from_value::<ExternalMcpSettings>(value).unwrap(),
            next
        );
        assert!(!next.http_client.supports(&McpTransport::StreamableHttp));
        next.enabled = false;
        assert!(!next.runtime_eq(&old));
    }
}

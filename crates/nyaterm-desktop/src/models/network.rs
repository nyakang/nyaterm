#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkTab {
    Tunnels,
    Proxies,
}

impl NetworkTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tunnels => "Tunnels",
            Self::Proxies => "Proxies",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkGroupEditorState {
    pub(crate) tab: NetworkTab,
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkTunnelEditorField {
    Name,
    ListenPort,
    TargetHost,
    TargetPort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkTunnelEditorState {
    pub(crate) id: Option<String>,
    pub(crate) is_open: bool,
    pub(crate) name: String,
    pub(crate) tunnel_type: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) listen_port: String,
    pub(crate) target_host: String,
    pub(crate) target_port: String,
    pub(crate) auto_open: bool,
    pub(crate) bind_localhost: bool,
    pub(crate) group_id: Option<String>,
    pub(crate) focused_field: NetworkTunnelEditorField,
    pub(crate) error: Option<String>,
}

impl NetworkTunnelEditorState {
    pub(crate) fn is_dynamic(&self) -> bool {
        self.tunnel_type == "dynamic"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkProxyEditorField {
    Name,
    Host,
    Port,
    Command,
    Username,
    Password,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NetworkProxyEditorState {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) protocol: String,
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) command: String,
    pub(crate) username: String,
    pub(crate) password: nyaterm_core::SecretString,
    pub(crate) existing_password: Option<nyaterm_core::SecretString>,
    pub(crate) password_id: Option<String>,
    pub(crate) group_id: Option<String>,
    pub(crate) focused_field: NetworkProxyEditorField,
    pub(crate) error: Option<String>,
}

impl std::fmt::Debug for NetworkProxyEditorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkProxyEditorState")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("command", &"<redacted>")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("existing_password", &"<redacted>")
            .field("password_id", &self.password_id)
            .field("group_id", &self.group_id)
            .field("focused_field", &self.focused_field)
            .field("error", &self.error)
            .finish()
    }
}

impl NetworkProxyEditorState {
    pub(crate) fn is_proxy_command(&self) -> bool {
        self.protocol == "proxycommand"
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkProxyEditorField, NetworkProxyEditorState};

    #[test]
    fn proxy_editor_debug_output_redacts_password_and_command() {
        let secret = "nya-proxy-secret-never-log";
        let state = NetworkProxyEditorState {
            id: None,
            name: "Test".to_string(),
            protocol: "proxycommand".to_string(),
            host: "localhost".to_string(),
            port: "22".to_string(),
            command: secret.to_string(),
            username: "tester".to_string(),
            password: secret.to_string().into(),
            existing_password: Some(secret.to_string().into()),
            password_id: None,
            group_id: None,
            focused_field: NetworkProxyEditorField::Name,
            error: None,
        };
        let output = format!("{state:?}");

        assert!(!output.contains(secret));
        assert!(output.contains("<redacted>"));
    }
}

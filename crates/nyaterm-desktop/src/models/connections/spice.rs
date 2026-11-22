use nyaterm_core::spice::settings::{SpiceEndpoint, SpiceSettings};

/// Optional ports remain text while editing, so an empty value really disables
/// that transport and invalid input is not silently replaced by a previous value.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SpiceEditorDraft {
    pub(crate) tcp_port: String,
    pub(crate) tls_port: String,
    pub(crate) proxy: String,
    pub(crate) ca_pem: String,
    pub(crate) certificate_subject: String,
    pub(crate) reconnect_attempts: String,
    pub(crate) settings: SpiceSettings,
    endpoint_extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for SpiceEditorDraft {
    fn default() -> Self {
        Self {
            tcp_port: String::new(),
            tls_port: "5901".into(),
            proxy: String::new(),
            ca_pem: String::new(),
            certificate_subject: String::new(),
            reconnect_attempts: "5".into(),
            settings: SpiceSettings::default(),
            endpoint_extra: Default::default(),
        }
    }
}

impl SpiceEditorDraft {
    pub(crate) fn from_saved(endpoint: &SpiceEndpoint, settings: SpiceSettings) -> Self {
        Self {
            endpoint_extra: endpoint.extra.clone(),
            tcp_port: endpoint.port.map(|p| p.to_string()).unwrap_or_default(),
            tls_port: endpoint.tls_port.map(|p| p.to_string()).unwrap_or_default(),
            proxy: endpoint
                .proxy
                .as_ref()
                .map(|p| p.expose_secret().to_owned())
                .unwrap_or_default(),
            ca_pem: settings.security.ca_pem.clone().unwrap_or_default(),
            certificate_subject: settings
                .security
                .certificate_subject
                .clone()
                .unwrap_or_default(),
            reconnect_attempts: settings.reconnect_attempts.to_string(),
            settings,
        }
    }

    pub(crate) fn build(&self, host: &str) -> Result<(SpiceEndpoint, SpiceSettings), &'static str> {
        fn port(value: &str) -> Result<Option<u16>, &'static str> {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .map(Some)
                .ok_or("ports")
        }
        fn optional(value: &str) -> Option<String> {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        }
        let endpoint = SpiceEndpoint {
            extra: self.endpoint_extra.clone(),
            host: host.trim().into(),
            port: port(&self.tcp_port)?,
            tls_port: port(&self.tls_port)?,
            proxy: optional(&self.proxy).map(Into::into),
        };
        endpoint
            .validate_persistent_identity()
            .map_err(|_| "identity")?;
        let mut settings = self.settings.clone();
        settings.security.ca_pem = optional(&self.ca_pem);
        settings.security.certificate_subject = optional(&self.certificate_subject);
        settings.reconnect_attempts = self
            .reconnect_attempts
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 20)
            .ok_or("reconnect")?;
        settings
            .security
            .validate(&endpoint)
            .map_err(|_| "security")?;
        Ok((endpoint, settings))
    }
}

#[cfg(test)]
mod tests {
    use super::SpiceEditorDraft;
    use nyaterm_core::spice::settings::{SpiceEndpoint, SpiceSettings};

    #[test]
    fn spice_editor_keeps_optional_ports_and_unknown_security_preferences() {
        let endpoint = SpiceEndpoint {
            extra: std::collections::BTreeMap::from([(
                "future_endpoint".into(),
                serde_json::json!(true),
            )]),
            host: "vm.example".into(),
            port: Some(5900),
            tls_port: Some(5902),
            proxy: None,
        };
        let mut settings = SpiceSettings::default();
        settings
            .extra
            .insert("future".into(), serde_json::json!({"v": 4}));
        settings
            .security
            .extra
            .insert("future_security".into(), serde_json::json!(true));
        let mut draft = SpiceEditorDraft::from_saved(&endpoint, settings.clone());
        assert_eq!(
            draft.build("vm.example").unwrap(),
            (endpoint.clone(), settings.clone())
        );
        draft.tcp_port.clear();
        let (saved, updated) = draft.build("vm.example").unwrap();
        assert_eq!(saved.port, None);
        assert_eq!(saved.tls_port, Some(5902));
        assert_eq!(updated, settings);
        draft.tls_port.clear();
        assert!(draft.build("vm.example").is_err());
    }

    #[test]
    fn spice_editor_rejects_ticket_credentials_invalid_ports_and_implicit_tls_downgrade() {
        let mut draft = SpiceEditorDraft::default();
        assert_eq!(
            draft.build("pvespiceproxy:ticket:101:pve").unwrap_err(),
            "identity"
        );
        draft.proxy = "http://user:ticket@proxy.example".into();
        assert_eq!(draft.build("vm.example").unwrap_err(), "identity");
        draft.proxy.clear();
        draft.tls_port = "invalid".into();
        assert_eq!(draft.build("vm.example").unwrap_err(), "ports");
        draft.tls_port.clear();
        draft.tcp_port = "5900".into();
        assert_eq!(draft.build("vm.example").unwrap_err(), "security");
        draft.settings.security.require_tls = false;
        assert!(draft.build("vm.example").is_ok());
        draft.reconnect_attempts = "-1".into();
        assert_eq!(draft.build("vm.example").unwrap_err(), "reconnect");
    }
}

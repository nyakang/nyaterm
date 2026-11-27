//! Persistent Mosh settings. Session keys are deliberately absent.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoshPrediction {
    #[default]
    Adaptive,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoshSettings {
    pub server_path: String,
    pub udp_port_start: u16,
    pub udp_port_end: u16,
    pub udp_host: String,
    pub prediction: MoshPrediction,
}

impl Default for MoshSettings {
    fn default() -> Self {
        Self {
            server_path: "mosh-server".into(),
            udp_port_start: 60000,
            udp_port_end: 61000,
            udp_host: String::new(),
            prediction: MoshPrediction::Adaptive,
        }
    }
}

impl MoshSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.server_path.trim().is_empty()
            || self.server_path.len() > 4096
            || self.server_path.chars().any(char::is_control)
        {
            return Err("Invalid mosh-server executable path");
        }
        if self.udp_port_start == 0 || self.udp_port_end < self.udp_port_start {
            return Err("Invalid Mosh UDP port range");
        }
        if self.udp_host.len() > 1024
            || self.udp_host.chars().any(char::is_whitespace)
            || self.udp_host.chars().any(char::is_control)
        {
            return Err("Invalid Mosh UDP host");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MoshSettings;
    use crate::models::{ConnectionType, SavedConnection};

    #[test]
    fn mosh_defaults_and_unknown_fields_survive_saved_connection_round_trip() {
        let input = serde_json::json!({"id":"m", "name":"host", "type":"mosh",
            "host":"example.test", "mosh":{"future_setting":42}, "future":true});
        let connection: SavedConnection = serde_json::from_value(input).unwrap();
        let ConnectionType::Mosh { port, mosh, .. } = &connection.config else {
            panic!()
        };
        assert_eq!(*port, 22);
        assert_eq!(*mosh, MoshSettings::default());
        let output = serde_json::to_value(&connection).unwrap();
        assert_eq!(output["type"], "mosh");
        assert_eq!(output["mosh"]["future_setting"], 42);
        assert_eq!(output["future"], true);
        assert_eq!(connection, serde_json::from_value(output).unwrap());
        assert!(matches!(
            connection.config.ssh_bootstrap_config(),
            ConnectionType::Ssh { .. }
        ));
    }

    #[test]
    fn rejects_reversed_ports_and_control_characters() {
        let mut settings = MoshSettings {
            udp_port_end: 59999,
            ..MoshSettings::default()
        };
        assert!(settings.validate().is_err());
        settings = MoshSettings::default();
        settings.server_path.push('\n');
        assert!(settings.validate().is_err());
    }
    #[test]
    fn sync_preserves_mosh_settings_and_local_unknown_fields() {
        let original = serde_json::json!({"id":"m", "name":"Mosh", "type":"mosh", "host":"test",
            "mosh":{"server_path":"/opt/mosh-server", "udp_port_start":60001,"udp_port_end":60005,"prediction":"never","future":42}});
        let projected = crate::sync_schema::project_record("connection", &original).unwrap();
        assert_eq!(projected["mosh"]["server_path"], "/opt/mosh-server");
        let restored =
            crate::sync_schema::apply_record("connection", &original, &projected).unwrap();
        assert_eq!(restored["mosh"]["future"], 42);
        let connection: SavedConnection = serde_json::from_value(restored).unwrap();
        let ConnectionType::Mosh { mosh, .. } = connection.config else {
            panic!()
        };
        assert_eq!(mosh.udp_port_start, 60001);
        assert_eq!(mosh.prediction, super::MoshPrediction::Never);
    }

    #[test]
    fn temporary_link_uses_ssh_port_and_rejects_inline_passwords() {
        let parsed =
            crate::temporary_connection::parse_temporary_mosh_link("mosh://alice@[::1]:2222")
                .unwrap();
        assert_eq!(
            (parsed.username.as_str(), parsed.host.as_str(), parsed.port),
            ("alice", "::1", 2222)
        );
        assert!(
            crate::temporary_connection::parse_temporary_mosh_link("mosh://alice:secret@host")
                .is_err()
        );
    }
    #[test]
    fn batch_updates_mosh_but_skips_ssh_terminal_only_fields() {
        use crate::models::connection_batch::{
            BatchField, ConnectionBatchEdit, apply_connection_batch_edit,
        };
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id":"m", "name":"Mosh", "type":"mosh", "host":"host"
        }))
        .unwrap();
        let mut edit = ConnectionBatchEdit::default();
        edit.mosh.udp_port_start = BatchField::Set(60001);
        edit.username = BatchField::Set("alice".into());
        edit.agent_forwarding_external = BatchField::Set(true);
        edit.x11_forwarding = BatchField::Set(true);
        edit.encoding = BatchField::Set("GBK".into());
        let report = apply_connection_batch_edit(&mut connection, &edit);
        assert_eq!(report.applied_fields, 2);
        assert_eq!(report.skipped_fields, 3);
        let ConnectionType::Mosh { username, mosh, .. } = connection.config else {
            panic!()
        };
        assert_eq!(username, "alice");
        assert_eq!(mosh.udp_port_start, 60001);
    }
}

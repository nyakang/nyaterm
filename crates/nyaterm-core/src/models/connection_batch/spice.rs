//! Independent SPICE batch leaves; never replace a whole settings object.
use crate::models::ConnectionType;
use crate::models::connection_batch::{
    BatchField, ConnectionBatchApplyReport, ConnectionBatchSkipReason, flag_value,
};
use crate::spice::settings::{SpiceClipboardMode, SpiceEndpoint, SpiceSecuritySettings};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpiceBatchEdit {
    pub tcp_port: BatchField<u16>,
    pub tls_port: BatchField<u16>,
    pub proxy: BatchField<String>,
    pub ca_pem: BatchField<String>,
    pub certificate_subject: BatchField<String>,
    pub require_tls: BatchField<bool>,
    pub dynamic_resolution: BatchField<bool>,
    pub fit_window: BatchField<bool>,
    pub view_only: BatchField<bool>,
    pub clipboard_to_guest: BatchField<bool>,
    pub clipboard_from_guest: BatchField<bool>,
    pub audio_playback: BatchField<bool>,
    pub file_transfer: BatchField<bool>,
    pub reconnect_attempts: BatchField<u32>,
}

impl std::fmt::Debug for SpiceBatchEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Draft text may contain an accidentally pasted ticket/private key.
        f.debug_struct("SpiceBatchEdit")
            .field("changed", &!self.is_empty())
            .finish_non_exhaustive()
    }
}

impl SpiceBatchEdit {
    fn touched(&self) -> [bool; 14] {
        [
            !self.tcp_port.is_unchanged(),
            !self.tls_port.is_unchanged(),
            !self.proxy.is_unchanged(),
            !self.ca_pem.is_unchanged(),
            !self.certificate_subject.is_unchanged(),
            !self.require_tls.is_unchanged(),
            !self.dynamic_resolution.is_unchanged(),
            !self.fit_window.is_unchanged(),
            !self.view_only.is_unchanged(),
            !self.clipboard_to_guest.is_unchanged(),
            !self.clipboard_from_guest.is_unchanged(),
            !self.audio_playback.is_unchanged(),
            !self.file_transfer.is_unchanged(),
            !self.reconnect_attempts.is_unchanged(),
        ]
    }

    pub fn is_empty(&self) -> bool {
        !self.touched().into_iter().any(|changed| changed)
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        for field in [&self.tcp_port, &self.tls_port] {
            if matches!(field, BatchField::Set(0)) {
                return Err("invalid SPICE port".into());
            }
        }
        if matches!(self.reconnect_attempts, BatchField::Set(value) if value > 20) {
            return Err("invalid SPICE reconnect attempts".into());
        }
        let endpoint = SpiceEndpoint {
            extra: Default::default(),
            host: "validation.example".into(),
            port: Some(5900),
            tls_port: Some(5901),
            proxy: match &self.proxy {
                BatchField::Set(value) => Some(value.clone().into()),
                _ => None,
            },
        };
        endpoint.validate().map_err(|_| "invalid SPICE proxy")?;
        endpoint.validate_persistent_identity()?;
        let security = SpiceSecuritySettings {
            ca_pem: match &self.ca_pem {
                BatchField::Set(value) => Some(value.clone()),
                _ => None,
            },
            certificate_subject: match &self.certificate_subject {
                BatchField::Set(value) => Some(value.clone()),
                _ => None,
            },
            ..Default::default()
        };
        security
            .validate(&endpoint)
            .map_err(|_| "invalid SPICE certificate settings")?;
        Ok(())
    }

    pub(super) fn apply(
        &self,
        config: &mut ConnectionType,
        report: &mut ConnectionBatchApplyReport,
    ) {
        let supported = matches!(config, ConnectionType::Spice { .. });
        for touched in self.touched() {
            report.record(
                touched,
                (!supported).then_some(ConnectionBatchSkipReason::UnsupportedProtocol),
            );
        }
        let ConnectionType::Spice {
            endpoint, settings, ..
        } = config
        else {
            return;
        };
        for (edit, target) in [
            (&self.tcp_port, &mut endpoint.port),
            (&self.tls_port, &mut endpoint.tls_port),
        ] {
            match edit {
                BatchField::Set(value) => *target = Some(*value),
                BatchField::Clear => *target = None,
                BatchField::Unchanged => {}
            }
        }
        match &self.proxy {
            BatchField::Set(value) => endpoint.proxy = Some(value.clone().into()),
            BatchField::Clear => endpoint.proxy = None,
            BatchField::Unchanged => {}
        }
        for (edit, target) in [
            (&self.ca_pem, &mut settings.security.ca_pem),
            (
                &self.certificate_subject,
                &mut settings.security.certificate_subject,
            ),
        ] {
            match edit {
                BatchField::Set(value) => *target = Some(value.clone()),
                BatchField::Clear => *target = None,
                BatchField::Unchanged => {}
            }
        }
        if !self.require_tls.is_unchanged() {
            settings.security.require_tls = flag_value(&self.require_tls, true);
        }
        if !self.dynamic_resolution.is_unchanged() {
            settings.dynamic_resolution = flag_value(&self.dynamic_resolution, true);
        }
        if !self.fit_window.is_unchanged() {
            settings.fit_window = flag_value(&self.fit_window, true);
        }
        if !self.view_only.is_unchanged() {
            settings.view_only = flag_value(&self.view_only, false);
        }
        if !self.audio_playback.is_unchanged() {
            settings.audio_playback = flag_value(&self.audio_playback, true);
        }
        if !self.file_transfer.is_unchanged() {
            settings.file_transfer = flag_value(&self.file_transfer, true);
        }
        if !self.clipboard_to_guest.is_unchanged() || !self.clipboard_from_guest.is_unchanged() {
            let send = if self.clipboard_to_guest.is_unchanged() {
                settings.clipboard.can_send()
            } else {
                flag_value(&self.clipboard_to_guest, true)
            };
            let receive = if self.clipboard_from_guest.is_unchanged() {
                settings.clipboard.can_receive()
            } else {
                flag_value(&self.clipboard_from_guest, true)
            };
            settings.clipboard = match (send, receive) {
                (false, false) => SpiceClipboardMode::Disabled,
                (true, false) => SpiceClipboardMode::ToGuest,
                (false, true) => SpiceClipboardMode::FromGuest,
                (true, true) => SpiceClipboardMode::Bidirectional,
            };
        }
        match self.reconnect_attempts {
            BatchField::Set(value) => settings.reconnect_attempts = value,
            BatchField::Clear => settings.reconnect_attempts = 5,
            BatchField::Unchanged => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::ConnectionType;
    use crate::models::connection_batch::spice::SpiceBatchEdit;
    use crate::models::connection_batch::{BatchField, ConnectionBatchApplyReport};
    use serde_json::json;

    #[test]
    fn spice_batch_edits_preserve_other_ports_directions_and_future_settings() {
        let mut config: ConnectionType = serde_json::from_value(json!({
            "type":"spice", "endpoint":{"host":"guest", "port":5900, "tls_port":5901},
            "settings":{"clipboard":"from_guest", "future":42, "security":{"future_tls":true}}
        }))
        .unwrap();
        let edit = SpiceBatchEdit {
            tcp_port: BatchField::Clear,
            clipboard_to_guest: BatchField::Set(true),
            audio_playback: BatchField::Set(false),
            ..Default::default()
        };
        edit.validate().unwrap();
        let mut report = ConnectionBatchApplyReport::default();
        edit.apply(&mut config, &mut report);
        let ConnectionType::Spice {
            endpoint, settings, ..
        } = config
        else {
            panic!("SPICE config")
        };
        assert_eq!(endpoint.port, None);
        assert_eq!(endpoint.tls_port, Some(5901));
        assert!(settings.clipboard.can_send() && settings.clipboard.can_receive());
        assert!(!settings.audio_playback);
        assert_eq!(settings.extra["future"], 42);
        assert_eq!(settings.security.extra["future_tls"], true);
        assert_eq!(report.skipped_fields, 0);
    }

    #[test]
    fn spice_batch_values_reject_invalid_ports_attempts_and_proxy_secrets() {
        for edit in [
            SpiceBatchEdit {
                tcp_port: BatchField::Set(0),
                ..Default::default()
            },
            SpiceBatchEdit {
                reconnect_attempts: BatchField::Set(21),
                ..Default::default()
            },
            SpiceBatchEdit {
                proxy: BatchField::Set("http://user:private@proxy".into()),
                ..Default::default()
            },
        ] {
            assert!(edit.validate().is_err());
            assert!(!format!("{edit:?}").contains("private"));
        }
    }
}

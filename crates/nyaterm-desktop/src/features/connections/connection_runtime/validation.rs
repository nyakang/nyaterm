use super::helpers::ConnectionEditorValidationError;
use crate::models::{
    ConnectionEditorField, ConnectionEditorFocusTarget, ConnectionEditorSection,
    ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab, ConnectionEditorState,
    ConnectionKindTab,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features::connections) struct ConnectionEditorValidationLocation {
    pub section: ConnectionEditorSection,
    pub target: ConnectionEditorFocusTarget,
    pub algorithm_tab: Option<ConnectionEditorSshAlgorithmTab>,
}

impl ConnectionEditorValidationError {
    pub(super) fn location(
        &self,
        editor: &ConnectionEditorState,
    ) -> ConnectionEditorValidationLocation {
        use nyaterm_transport::{SshAlgorithmListKind, SshAlgorithmValidationError};
        let location = |section, target| ConnectionEditorValidationLocation {
            section,
            target,
            algorithm_tab: None,
        };
        match self {
            Self::RdpRelayUnsupported => {
                return location(
                    ConnectionEditorSection::Network,
                    ConnectionEditorFocusTarget::Heading,
                );
            }
            Self::RdpWindowsAuthenticationRequired => {
                return location(
                    ConnectionEditorSection::Security,
                    ConnectionEditorFocusTarget::Select(
                        ConnectionEditorSelect::RdpCertificatePolicy,
                    ),
                );
            }
            Self::RdpClipboardUnsupported => {
                return location(
                    ConnectionEditorSection::Session,
                    ConnectionEditorFocusTarget::Select(ConnectionEditorSelect::RdpClipboardMode),
                );
            }
            Self::SshAgentEndpoint(_) => {
                let target = match editor.agent_endpoint {
                    nyaterm_core::SshAgentEndpoint::Environment { .. } => {
                        ConnectionEditorFocusTarget::Field(
                            ConnectionEditorField::AgentEnvironmentVariable,
                        )
                    }
                    nyaterm_core::SshAgentEndpoint::UnixSocket { .. } => {
                        ConnectionEditorFocusTarget::Field(ConnectionEditorField::AgentUnixSocket)
                    }
                    _ => ConnectionEditorFocusTarget::Select(
                        ConnectionEditorSelect::SshAgentEndpoint,
                    ),
                };
                return location(ConnectionEditorSection::Basic, target);
            }
            Self::SshAgentForwarding { endpoint_index, .. } => {
                let target = endpoint_index
                    .filter(|_| {
                        editor.agent_forwarding_config.enabled
                            && editor.agent_forwarding_config.sources.external_agent
                    })
                    .and_then(|index| {
                        editor
                            .agent_forwarding_config
                            .sources
                            .external_agent_endpoints
                            .get(index)
                            .and_then(|endpoint| {
                                let field = match endpoint {
                                    nyaterm_core::SshAgentEndpoint::Environment { .. } => {
                                        ConnectionEditorField::AgentForwardingEnvironmentVariable
                                    }
                                    nyaterm_core::SshAgentEndpoint::UnixSocket { .. } => {
                                        ConnectionEditorField::AgentForwardingSocketPath
                                    }
                                    _ => return None,
                                };
                                Some(ConnectionEditorFocusTarget::ForwardingEndpoint {
                                    index,
                                    field,
                                })
                            })
                    })
                    .unwrap_or(ConnectionEditorFocusTarget::Heading);
                return location(ConnectionEditorSection::Forwarding, target);
            }
            Self::SshAlgorithms(error) => {
                let (SshAlgorithmValidationError::EmptyList { kind }
                | SshAlgorithmValidationError::Unsupported { kind, .. }) = error;
                return ConnectionEditorValidationLocation {
                    section: ConnectionEditorSection::Algorithms,
                    target: ConnectionEditorFocusTarget::Heading,
                    algorithm_tab: Some(match kind {
                        SshAlgorithmListKind::KeyExchange => {
                            ConnectionEditorSshAlgorithmTab::KeyExchange
                        }
                        SshAlgorithmListKind::Cipher => ConnectionEditorSshAlgorithmTab::Ciphers,
                        SshAlgorithmListKind::Mac => ConnectionEditorSshAlgorithmTab::Macs,
                        SshAlgorithmListKind::HostKey => ConnectionEditorSshAlgorithmTab::HostKeys,
                    }),
                };
            }
            _ => {}
        }
        let field = match self {
            ConnectionEditorValidationError::HostRequired => ConnectionEditorField::Host,
            ConnectionEditorValidationError::PortInvalid => ConnectionEditorField::Port,
            ConnectionEditorValidationError::UsernameRequired
            | ConnectionEditorValidationError::MissingAccount => ConnectionEditorField::Username,
            ConnectionEditorValidationError::VncPasswordTooLong => ConnectionEditorField::Password,
            ConnectionEditorValidationError::ShellPathRequired => ConnectionEditorField::ShellPath,
            ConnectionEditorValidationError::SerialPortRequired => {
                ConnectionEditorField::SerialPort
            }
            ConnectionEditorValidationError::BaudRateInvalid => ConnectionEditorField::BaudRate,
            ConnectionEditorValidationError::VncUsernameTooLong => ConnectionEditorField::Username,
            ConnectionEditorValidationError::TelnetKeepAliveIntervalInvalid => {
                ConnectionEditorField::TelnetKeepAliveInterval
            }
            ConnectionEditorValidationError::RdpDisplayWidthInvalid => {
                ConnectionEditorField::RdpDisplayWidth
            }
            ConnectionEditorValidationError::RdpDisplayHeightInvalid => {
                ConnectionEditorField::RdpDisplayHeight
            }
            ConnectionEditorValidationError::RdpReconnectAttemptsInvalid => {
                ConnectionEditorField::RdpReconnectAttempts
            }
            ConnectionEditorValidationError::VncReconnectAttemptsInvalid => {
                ConnectionEditorField::VncReconnectAttempts
            }
            ConnectionEditorValidationError::PostLoginCommandRequired => {
                ConnectionEditorField::PostLoginCommand
            }
            ConnectionEditorValidationError::PostLoginDelayInvalid => {
                ConnectionEditorField::PostLoginDelay
            }
            ConnectionEditorValidationError::SftpShellDetectionTimeoutInvalid => {
                ConnectionEditorField::SftpShellDetectionTimeout
            }
            ConnectionEditorValidationError::RdpWindowsAuthenticationRequired
            | ConnectionEditorValidationError::RdpRelayUnsupported
            | ConnectionEditorValidationError::RdpClipboardUnsupported
            | ConnectionEditorValidationError::SshAgentEndpoint(_)
            | ConnectionEditorValidationError::SshAgentForwarding { .. }
            | ConnectionEditorValidationError::SshAlgorithms(_) => {
                unreachable!("specialized validation locations handled above")
            }
        };
        let target = match field {
            ConnectionEditorField::SerialPort => {
                ConnectionEditorFocusTarget::Select(ConnectionEditorSelect::SerialPort)
            }
            ConnectionEditorField::Username
                if matches!(
                    editor.kind,
                    ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                ) && editor.username_source
                    == nyaterm_core::ConnectionUsernameSource::Account
                    && editor.account_id.is_some() =>
            {
                ConnectionEditorFocusTarget::Select(ConnectionEditorSelect::Account)
            }
            _ if matches!(self, Self::MissingAccount) => ConnectionEditorFocusTarget::Select(
                if matches!(
                    editor.kind,
                    ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                ) {
                    ConnectionEditorSelect::Account
                } else {
                    ConnectionEditorSelect::SavedPassword
                },
            ),
            _ => ConnectionEditorFocusTarget::Field(field),
        };
        location(field.section(), target)
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionEditorValidationError;
    use crate::features::connections::connection_runtime::helpers::{
        build_saved_connection_from_editor, connection_editor_from_saved,
    };
    use crate::models::{
        ConnectionEditorField, ConnectionEditorFocusTarget, ConnectionEditorSection,
        ConnectionEditorSshAlgorithmTab,
    };
    use nyaterm_core::{SavedConnection, SshAgentEndpoint};
    use nyaterm_transport::{SshAlgorithmListKind, SshAlgorithmValidationError};

    fn editor() -> crate::models::ConnectionEditorState {
        let saved: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "navigation", "name": "Navigation", "type": "ssh", "host": "example.test", "username": "root"
        })).unwrap();
        connection_editor_from_saved(saved, false)
    }

    #[test]
    fn validation_routes_hidden_fields_to_their_configuration_pages() {
        for (error, section, field) in [
            (
                ConnectionEditorValidationError::HostRequired,
                ConnectionEditorSection::Basic,
                ConnectionEditorField::Host,
            ),
            (
                ConnectionEditorValidationError::UsernameRequired,
                ConnectionEditorSection::Basic,
                ConnectionEditorField::Username,
            ),
            (
                ConnectionEditorValidationError::VncUsernameTooLong,
                ConnectionEditorSection::Basic,
                ConnectionEditorField::Username,
            ),
            (
                ConnectionEditorValidationError::VncPasswordTooLong,
                ConnectionEditorSection::Basic,
                ConnectionEditorField::Password,
            ),
            (
                ConnectionEditorValidationError::PostLoginDelayInvalid,
                ConnectionEditorSection::PostLogin,
                ConnectionEditorField::PostLoginDelay,
            ),
            (
                ConnectionEditorValidationError::SftpShellDetectionTimeoutInvalid,
                ConnectionEditorSection::Sftp,
                ConnectionEditorField::SftpShellDetectionTimeout,
            ),
            (
                ConnectionEditorValidationError::RdpDisplayWidthInvalid,
                ConnectionEditorSection::Display,
                ConnectionEditorField::RdpDisplayWidth,
            ),
            (
                ConnectionEditorValidationError::VncReconnectAttemptsInvalid,
                ConnectionEditorSection::Session,
                ConnectionEditorField::VncReconnectAttempts,
            ),
            (
                ConnectionEditorValidationError::TelnetKeepAliveIntervalInvalid,
                ConnectionEditorSection::Compatibility,
                ConnectionEditorField::TelnetKeepAliveInterval,
            ),
        ] {
            let location = error.location(&editor());
            assert_eq!(location.section, section);
            assert_eq!(location.target, ConnectionEditorFocusTarget::Field(field));
        }
    }

    #[test]
    fn agent_validation_retains_authentication_or_forwarding_endpoint_origin() {
        let mut draft = editor();
        draft.auth_mode = "agent".into();
        draft.agent_endpoint = SshAgentEndpoint::UnixSocket {
            path: String::new(),
        };
        let error = build_saved_connection_from_editor(&draft).unwrap_err();
        assert_eq!(
            error.location(&draft).section,
            ConnectionEditorSection::Basic
        );
        assert_eq!(
            error.location(&draft).target,
            ConnectionEditorFocusTarget::Field(ConnectionEditorField::AgentUnixSocket)
        );
        draft.auth_mode = "password".into();
        draft.agent_forwarding_config.enabled = true;
        draft.agent_forwarding_config.sources.external_agent = true;
        draft
            .agent_forwarding_config
            .sources
            .external_agent_endpoints = vec![
            SshAgentEndpoint::Auto,
            SshAgentEndpoint::Environment {
                variable: String::new(),
            },
        ];
        let error = build_saved_connection_from_editor(&draft).unwrap_err();
        assert!(matches!(
            error,
            ConnectionEditorValidationError::SshAgentForwarding {
                endpoint_index: Some(1),
                ..
            }
        ));
        assert_eq!(
            error.location(&draft).section,
            ConnectionEditorSection::Forwarding
        );
        assert_eq!(
            error.location(&draft).target,
            ConnectionEditorFocusTarget::ForwardingEndpoint {
                index: 1,
                field: ConnectionEditorField::AgentForwardingEnvironmentVariable
            }
        );
        for (enabled, external_agent) in [(false, true), (true, false)] {
            draft.agent_forwarding_config.enabled = enabled;
            draft.agent_forwarding_config.sources.external_agent = external_agent;
            assert_eq!(
                error.location(&draft).target,
                ConnectionEditorFocusTarget::Heading
            );
        }
    }

    #[test]
    fn algorithm_validation_selects_the_corresponding_secondary_tab() {
        for (kind, expected) in [
            (
                SshAlgorithmListKind::KeyExchange,
                ConnectionEditorSshAlgorithmTab::KeyExchange,
            ),
            (
                SshAlgorithmListKind::Cipher,
                ConnectionEditorSshAlgorithmTab::Ciphers,
            ),
            (
                SshAlgorithmListKind::Mac,
                ConnectionEditorSshAlgorithmTab::Macs,
            ),
            (
                SshAlgorithmListKind::HostKey,
                ConnectionEditorSshAlgorithmTab::HostKeys,
            ),
        ] {
            let error = ConnectionEditorValidationError::SshAlgorithms(
                SshAlgorithmValidationError::EmptyList { kind },
            );
            let location = error.location(&editor());
            assert_eq!(location.section, ConnectionEditorSection::Algorithms);
            assert_eq!(location.algorithm_tab, Some(expected));
        }
    }

    #[test]
    fn navigating_every_protocol_category_preserves_the_saved_configuration() {
        for kind in ["ssh", "telnet", "rdp", "vnc", "serial", "local_terminal"] {
            let saved: SavedConnection = serde_json::from_value(serde_json::json!({
                "id": "navigation", "name": "Navigation", "type": kind, "host": "example.test",
                "username": "root", "shell_path": "bash", "port_name": "COM1"
            }))
            .unwrap();
            let mut draft = connection_editor_from_saved(saved, false);
            let before = build_saved_connection_from_editor(&draft).unwrap();
            for section in ConnectionEditorSection::for_kind(draft.kind) {
                assert!(draft.select_section(*section));
                let after = build_saved_connection_from_editor(&draft).unwrap();
                assert_eq!(before, after, "{kind}/{section:?}");
                let reopened = connection_editor_from_saved(after, false);
                assert_eq!(reopened.section, ConnectionEditorSection::Basic);
                assert_eq!(
                    build_saved_connection_from_editor(&reopened).unwrap(),
                    before
                );
            }
        }
    }
}

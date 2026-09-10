use gpui::Context;
use nyaterm_core::{
    ActivationAction, ActivationRequest, ConnectionType, ExternalConnectionRequest,
    parse_activation_request,
};

use super::NyaTermApp;
use crate::temporary_ssh_link::{TemporarySshLinkConfig, TemporaryTelnetLinkConfig};

impl NyaTermApp {
    pub(crate) fn handle_activation(&mut self, request: ActivationRequest, cx: &mut Context<Self>) {
        let actions = match parse_activation_request(&request) {
            Ok(actions) => actions,
            Err(_) => {
                // Do not include the rejected URL: it may contain credentials or
                // other user-controlled data despite the parser rejecting it.
                self.shell
                    .set_status("external activation link was rejected".to_string());
                cx.notify();
                return;
            }
        };
        for action in actions {
            self.apply_activation_action(action, cx);
        }
    }

    fn apply_activation_action(&mut self, action: ActivationAction, cx: &mut Context<Self>) {
        match action {
            ActivationAction::Activate => cx.notify(),
            ActivationAction::Connect(request) => self.activate_external_connection(request, cx),
        }
    }

    fn activate_external_connection(
        &mut self,
        request: ExternalConnectionRequest,
        cx: &mut Context<Self>,
    ) {
        let matches = self
            .connection_state
            .connections()
            .iter()
            .filter(|connection| external_request_matches_connection(&request, &connection.config))
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [connection] => {
                if connection
                    .post_login
                    .as_ref()
                    .is_some_and(|command| command.enabled && !command.command.trim().is_empty())
                {
                    let Some(window) = self.shell.main_window() else {
                        return;
                    };
                    let connection = connection.clone();
                    let app = cx.weak_entity();
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            let _ = app.update(cx, |app, cx| {
                                use nyaterm_ui::NyaDialogWindowExt as _;
                                if window.has_active_nya_dialog(cx) {
                                    return;
                                }
                                app.open_confirm_dialog(
                                    (
                                        rust_i18n::t!("dialog.postLoginExternalTitle").to_string(),
                                        rust_i18n::t!(
                                            "dialog.postLoginExternalMessage",
                                            name = connection.name.clone()
                                        )
                                        .to_string(),
                                        rust_i18n::t!("common.confirm").to_string(),
                                        true,
                                        move |app, _, cx| {
                                            app.continue_saved_connection_start(
                                                connection.clone(),
                                                Default::default(),
                                                cx,
                                            );
                                            true
                                        },
                                    ),
                                    window,
                                    cx,
                                );
                            });
                        });
                    });
                } else {
                    self.continue_saved_connection_start(
                        connection.clone(),
                        Default::default(),
                        cx,
                    );
                }
            }
            [] => match request {
                ExternalConnectionRequest::Ssh {
                    host,
                    port,
                    username,
                } => {
                    let username = username.unwrap_or_else(|| "root".to_string());
                    self.start_external_ssh_link(
                        TemporarySshLinkConfig {
                            name: format!("{username}@{host}:{port}"),
                            host,
                            port,
                            username,
                        },
                        cx,
                    );
                }
                ExternalConnectionRequest::Telnet { host, port } => {
                    self.start_external_telnet_link(
                        TemporaryTelnetLinkConfig {
                            name: format!("telnet://{host}:{port}"),
                            host,
                            port,
                        },
                        cx,
                    );
                }
            },
            _ => {
                // Never pick an arbitrary saved credential when the external
                // target is ambiguous. Adding a username disambiguates SSH.
                self.shell.set_status(
                    "external link matched multiple saved connections; connection was not opened"
                        .to_string(),
                );
                cx.notify();
            }
        }
    }
}

fn external_request_matches_connection(
    request: &ExternalConnectionRequest,
    connection: &ConnectionType,
) -> bool {
    match (request, connection) {
        (
            ExternalConnectionRequest::Ssh {
                host,
                port,
                username,
            },
            ConnectionType::Ssh {
                host: saved_host,
                port: saved_port,
                username: saved_username,
                ..
            },
        ) => {
            host.eq_ignore_ascii_case(saved_host)
                && port == saved_port
                && username
                    .as_ref()
                    .is_none_or(|username| username == saved_username)
        }
        (
            ExternalConnectionRequest::Telnet { host, port },
            ConnectionType::Telnet {
                host: saved_host,
                port: saved_port,
                ..
            },
        ) => host.eq_ignore_ascii_case(saved_host) && port == saved_port,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{ConnectionType, ExternalConnectionRequest, TelnetAutoLoginConfig};

    use super::external_request_matches_connection;

    fn ssh_connection(username: &str) -> ConnectionType {
        ConnectionType::Ssh {
            host: "example.COM".to_string(),
            port: 22,
            username: username.to_string(),
            backspace_mode: String::new(),
            ai_execution_profile: Default::default(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        }
    }

    #[test]
    fn explicit_ssh_username_disambiguates_saved_connections() {
        let request = ExternalConnectionRequest::Ssh {
            host: "EXAMPLE.com".to_string(),
            port: 22,
            username: Some("deploy".to_string()),
        };
        assert!(external_request_matches_connection(
            &request,
            &ssh_connection("deploy")
        ));
        assert!(!external_request_matches_connection(
            &request,
            &ssh_connection("root")
        ));
    }

    #[test]
    fn telnet_match_requires_protocol_host_and_port() {
        let request = ExternalConnectionRequest::Telnet {
            host: "example.com".to_string(),
            port: 23,
        };
        let connection = ConnectionType::Telnet {
            host: "EXAMPLE.COM".to_string(),
            port: 23,
            username: String::new(),
            ai_execution_profile: Default::default(),
            backspace_mode: String::new(),
            raw_tcp_cli: false,
            enter_mode: String::new(),
            local_echo: false,
            local_line_edit: false,
            force_character_at_a_time: false,
            send_naws: true,
            send_sga: true,
            auto_login: TelnetAutoLoginConfig::default(),
            encoding: String::new(),
        };
        assert!(external_request_matches_connection(&request, &connection));
    }
}

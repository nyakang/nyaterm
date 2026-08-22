use rust_i18n::t;

use std::sync::Arc;

use gpui::{Context, Window};
use nyaterm_core::AiExecutionProfile;
use nyaterm_transport::{
    SerialSessionConfig, SshSessionConfig, SshSessionProfile, TelnetSessionConfig,
};

use super::NativeHostKeyVerifier;
use crate::features::{NyaTermApp, session::SavedConnectionStartOptions};
use crate::models::SessionLaunchConfig;
use crate::temporary_ssh_link::{
    TemporaryLinkProtocol, TemporarySerialLinkConfig, TemporarySshLinkConfig,
    TemporaryTelnetLinkConfig, build_temporary_serial_link, parse_temporary_ssh_link,
    parse_temporary_telnet_link,
};

impl NyaTermApp {
    pub(in crate::features) fn open_temporary_ssh_link_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_connection_serial_ports();
        self.session.dialogs.open_temporary_ssh_link();
        self.forget_text_inputs("temporary-ssh.");
        self.shell.set_status("temporary link opened".to_string());
        self.open_form_dialog(
            (
                t!("temporarySsh.title").to_string(),
                480.,
                t!("temporarySsh.connect").to_string(),
                |app, _, cx| app.temporary_ssh_link_dialog_content(cx),
                |app, window, cx| app.submit_temporary_ssh_link_dialog(window, cx),
                |app, cx| app.close_temporary_ssh_link_dialog(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_temporary_ssh_link_dialog(&mut self, cx: &mut Context<Self>) {
        self.session.dialogs.close_temporary_ssh_link();
        self.forget_text_inputs("temporary-ssh.");
        self.shell
            .set_status("temporary link cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn submit_temporary_ssh_link_dialog(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.session.dialogs.temporary_link_protocol() {
            TemporaryLinkProtocol::Ssh => self.submit_temporary_ssh_link(cx),
            TemporaryLinkProtocol::Telnet => self.submit_temporary_telnet_link(cx),
            TemporaryLinkProtocol::Serial => self.submit_temporary_serial_link(cx),
        }
    }

    pub(in crate::features) fn apply_temporary_ssh_link(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.session.dialogs.apply_temporary_ssh_link(text);
        cx.notify();
    }

    pub(in crate::features) fn apply_temporary_serial_port_name(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.session.dialogs.apply_temporary_serial_port_name(text);
        cx.notify();
    }

    pub(in crate::features) fn apply_temporary_serial_baud_rate(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.session.dialogs.apply_temporary_serial_baud_rate(text);
        cx.notify();
    }

    pub(in crate::features) fn set_temporary_link_protocol(
        &mut self,
        protocol: TemporaryLinkProtocol,
        cx: &mut Context<Self>,
    ) {
        if self.session.dialogs.temporary_link_protocol() != protocol {
            self.forget_text_inputs("temporary-ssh.link");
        }
        if protocol == TemporaryLinkProtocol::Serial {
            self.refresh_connection_serial_ports();
        }
        self.session.dialogs.set_temporary_link_protocol(protocol);
        cx.notify();
    }

    fn submit_temporary_ssh_link(&mut self, cx: &mut Context<Self>) -> bool {
        let parsed = match parse_temporary_ssh_link(self.session.dialogs.temporary_ssh_link_draft())
        {
            Ok(parsed) => parsed,
            Err(error) => {
                self.session
                    .dialogs
                    .reject_temporary_ssh_link(error.locale_key());
                self.shell
                    .set_status("temporary SSH link is invalid".to_string());
                cx.notify();
                return false;
            }
        };
        let config = self.temporary_ssh_session_config(parsed.clone());
        self.close_temporary_link_draft();
        self.begin_background_ssh_start(
            parsed.name,
            config,
            None,
            AiExecutionProfile::Auto,
            SavedConnectionStartOptions::default(),
            cx,
        );
        true
    }

    fn submit_temporary_telnet_link(&mut self, cx: &mut Context<Self>) -> bool {
        let parsed =
            match parse_temporary_telnet_link(self.session.dialogs.temporary_ssh_link_draft()) {
                Ok(parsed) => parsed,
                Err(error) => {
                    self.session
                        .dialogs
                        .reject_temporary_ssh_link(error.locale_key());
                    self.shell
                        .set_status("temporary Telnet link is invalid".to_string());
                    cx.notify();
                    return false;
                }
            };
        let config = self.temporary_telnet_session_config(parsed.clone());
        self.close_temporary_link_draft();
        self.begin_background_session_start(
            parsed.name,
            SessionLaunchConfig::Telnet(config),
            None,
            AiExecutionProfile::SendOnly,
            SavedConnectionStartOptions::default(),
            cx,
        );
        true
    }

    fn submit_temporary_serial_link(&mut self, cx: &mut Context<Self>) -> bool {
        let parsed = match build_temporary_serial_link(
            self.session.dialogs.temporary_serial_port_name(),
            self.session.dialogs.temporary_serial_baud_rate(),
        ) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.session
                    .dialogs
                    .reject_temporary_ssh_link(error.locale_key());
                self.shell
                    .set_status("temporary serial link is invalid".to_string());
                cx.notify();
                return false;
            }
        };
        let config = self.temporary_serial_session_config(parsed.clone());
        self.close_temporary_link_draft();
        self.begin_background_session_start(
            parsed.name,
            SessionLaunchConfig::Serial(config),
            None,
            AiExecutionProfile::SendOnly,
            SavedConnectionStartOptions::default(),
            cx,
        );
        true
    }

    fn close_temporary_link_draft(&mut self) {
        self.session.dialogs.close_temporary_ssh_link();
        self.forget_text_inputs("temporary-ssh.");
    }

    fn temporary_ssh_session_config(&self, parsed: TemporarySshLinkConfig) -> SshSessionConfig {
        let keep_alive_interval_secs =
            if self.settings.summary().terminal_keep_alive_mode == "disabled" {
                0
            } else {
                self.settings.summary().terminal_keep_alive_interval
            };
        SshSessionConfig {
            name: parsed.name,
            host: parsed.host,
            port: parsed.port,
            username: parsed.username,
            password: None,
            key_auth: None,
            agent_auth: false,
            agent_endpoint: Default::default(),
            agent_forwarding: false,
            agent_forwarding_config: None,
            agent_stored_key_provider: None,
            otp_id: None,
            auto_fill_otp: false,
            proxy_jump: None,
            proxy: None,
            allow_none_auth: false,
            backspace_mode: "del".to_string(),
            profile: SshSessionProfile::Standard,
            term: "xterm-256color".to_string(),
            x11_forwarding: false,
            x11_display: String::new(),
            encoding: self.settings.summary().interaction_default_encoding.clone(),
            ssh_algorithms: None,
            sftp: nyaterm_transport::SftpSettings::default(),
            terminal_shell_integration: self.settings.summary().terminal_zebra_stripes_enabled,
            deferred_pty: false,
            keep_alive_interval_secs,
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            host_key_verifier: Some(Arc::new(NativeHostKeyVerifier {
                store: self.store_blocking_client(),
                policy: self.settings.summary().host_key_policy.clone(),
                prompt_broker: self.session.prompts.host_key_broker(),
            })),
            credential_provider: Some(self.session.prompts.credential_broker()),
            agent_prompt_provider: Some(self.session.prompts.agent_broker()),
            otp_provider: Some(self.session.prompts.otp_provider()),
        }
    }

    fn temporary_telnet_session_config(
        &self,
        parsed: TemporaryTelnetLinkConfig,
    ) -> TelnetSessionConfig {
        TelnetSessionConfig {
            name: parsed.name,
            host: parsed.host,
            port: parsed.port,
            ..TelnetSessionConfig::default()
        }
    }

    fn temporary_serial_session_config(
        &self,
        parsed: TemporarySerialLinkConfig,
    ) -> SerialSessionConfig {
        SerialSessionConfig {
            name: parsed.name,
            port_name: parsed.port_name,
            baud_rate: parsed.baud_rate,
            ..SerialSessionConfig::default()
        }
    }
}

use nyaterm_transport::SessionKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendCommandTarget {
    Current,
    AllSessions,
    Group(String),
    Sessions(SendCommandSelection),
}

/// Explicit targets stay fixed when the active tab changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SendCommandSelection {
    ids: Vec<String>,
}

pub(crate) fn is_send_command_session_kind(kind: SessionKind) -> bool {
    matches!(
        kind,
        SessionKind::Serial
            | SessionKind::LocalPty
            | SessionKind::Ssh
            | SessionKind::Telnet
            | SessionKind::RawTcp
            | SessionKind::AdbShell
    )
}

impl SendCommandSelection {
    pub(crate) fn ids(&self) -> &[String] {
        &self.ids
    }
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|selected| selected == id)
    }
    pub(crate) fn toggle(&mut self, id: String, kind: SessionKind) {
        if self.contains(&id) {
            self.ids.retain(|selected| selected != &id);
        } else if is_send_command_session_kind(kind) {
            self.ids.push(id);
        }
    }
}

pub(crate) use nyaterm_core::send_command::{
    SendCommandDataType, SendCommandLineEnding, SendCommandMode,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendCommandControlFocus {
    Count,
    Interval,
}

#[cfg(test)]
pub(crate) fn build_send_command_units_for(
    draft: &str,
    data_type: SendCommandDataType,
    mode: SendCommandMode,
    line_ending: SendCommandLineEnding,
    session_kind: Option<SessionKind>,
) -> Result<Vec<Vec<u8>>, String> {
    use nyaterm_core::send_command::{SendCommandPlan, TextInterpretation};
    let plan = match data_type {
        SendCommandDataType::Hex => SendCommandPlan::hex(draft, mode, std::time::Duration::ZERO)?,
        SendCommandDataType::Text => SendCommandPlan::text(
            draft,
            mode,
            target_line_ending(session_kind, line_ending),
            TextInterpretation::Plain,
            std::time::Duration::ZERO,
        ),
    };
    Ok(plan.map(|chunk| chunk.bytes).collect())
}

pub(crate) fn target_line_ending(
    session_kind: Option<SessionKind>,
    configured: SendCommandLineEnding,
) -> SendCommandLineEnding {
    if matches!(
        session_kind,
        Some(SessionKind::LocalPty | SessionKind::Ssh | SessionKind::Telnet) | None
    ) {
        SendCommandLineEnding::Cr
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::send_command::parse_send_command_hex;
    use nyaterm_transport::SessionKind;

    use super::{
        SendCommandDataType, SendCommandLineEnding, SendCommandMode, build_send_command_units_for,
    };

    #[test]
    fn explicit_selection_allows_mixed_protocols_and_removes_targets_independently() {
        use super::SendCommandSelection;
        let mut selection = SendCommandSelection::default();
        selection.toggle("ssh".into(), SessionKind::Ssh);
        selection.toggle("serial".into(), SessionKind::Serial);
        selection.toggle("local".into(), SessionKind::LocalPty);
        assert_eq!(selection.ids(), ["ssh", "serial", "local"]);
        selection.toggle("serial".into(), SessionKind::Serial);
        assert_eq!(selection.ids(), ["ssh", "local"]);
    }

    #[test]
    fn explicit_selection_rejects_non_terminal_sessions() {
        use super::SendCommandSelection;
        let mut selection = SendCommandSelection::default();
        for kind in [
            SessionKind::Rdp,
            SessionKind::Vnc,
            SessionKind::Spice,
            SessionKind::AdbFiles,
            SessionKind::AdbScreen,
            SessionKind::Ftp,
        ] {
            selection.toggle("not-a-terminal".into(), kind);
        }
        assert!(selection.ids().is_empty());
    }

    #[test]
    fn parses_hex_send_input_with_spacing() {
        assert_eq!(
            parse_send_command_hex("48 65 6c 6C 6F").unwrap(),
            b"Hello".to_vec()
        );
        assert!(parse_send_command_hex("48 6").is_err());
        assert!(parse_send_command_hex("48 ZZ").is_err());
    }

    #[test]
    fn builds_shell_line_units_with_terminal_submit_return() {
        let units = build_send_command_units_for(
            "pwd\nwhoami",
            SendCommandDataType::Text,
            SendCommandMode::Line,
            SendCommandLineEnding::Crlf,
            Some(SessionKind::Ssh),
        )
        .unwrap();
        assert_eq!(units, vec![b"pwd\r".to_vec(), b"whoami\r".to_vec()]);
    }

    #[test]
    fn builds_serial_line_units_with_configured_line_ending() {
        let units = build_send_command_units_for(
            "AT\nATI",
            SendCommandDataType::Text,
            SendCommandMode::Line,
            SendCommandLineEnding::Crlf,
            Some(SessionKind::Serial),
        )
        .unwrap();
        assert_eq!(units, vec![b"AT\r\n".to_vec(), b"ATI\r\n".to_vec()]);
    }

    #[test]
    fn builds_character_and_hex_byte_units() {
        let text_units = build_send_command_units_for(
            "a\n",
            SendCommandDataType::Text,
            SendCommandMode::Character,
            SendCommandLineEnding::Lf,
            Some(SessionKind::LocalPty),
        )
        .unwrap();
        assert_eq!(text_units, vec![b"a".to_vec(), b"\r".to_vec()]);

        let hex_units = build_send_command_units_for(
            "0A FF",
            SendCommandDataType::Hex,
            SendCommandMode::Byte,
            SendCommandLineEnding::None,
            Some(SessionKind::Serial),
        )
        .unwrap();
        assert_eq!(hex_units, vec![vec![0x0a], vec![0xff]]);
    }

    #[test]
    fn empty_text_has_no_send_units_and_unicode_counts_utf8_bytes() {
        let build = |draft| {
            build_send_command_units_for(
                draft,
                SendCommandDataType::Text,
                SendCommandMode::Character,
                SendCommandLineEnding::None,
                Some(SessionKind::Serial),
            )
            .unwrap()
        };
        assert!(build("").is_empty());
        assert_eq!(build("中a"), vec!["中".as_bytes().to_vec(), vec![b'a']]);
    }
}

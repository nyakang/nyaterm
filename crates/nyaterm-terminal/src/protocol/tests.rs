use super::TerminalProtocolState;

#[test]
fn terminal_protocol_state_encodes_sgr_mouse_report() {
    let protocol = TerminalProtocolState {
        mouse_reporting: true,
        mouse_sgr: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(
        protocol.encode_mouse_report(0, 1, 2, true, false, false, false, false),
        b"\x1b[<0;2;3M".to_vec()
    );
    assert_eq!(
        protocol.encode_mouse_report(0, 1, 2, false, false, false, false, false),
        b"\x1b[<0;2;3m".to_vec()
    );
}

#[test]
fn terminal_protocol_state_blocks_alternate_scroll_when_mouse_reporting() {
    let protocol = TerminalProtocolState {
        alternate_screen: true,
        alternate_scroll: true,
        mouse_reporting: true,
        application_cursor_keys: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(protocol.alternate_scroll_payload(1), None);
}

#[test]
fn terminal_protocol_state_emits_alternate_scroll_payload() {
    let protocol = TerminalProtocolState {
        alternate_screen: true,
        alternate_scroll: true,
        application_cursor_keys: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(
        protocol.alternate_scroll_payload(1),
        Some(b"\x1bOA".to_vec())
    );
}

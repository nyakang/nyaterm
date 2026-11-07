use crate::TelnetSessionConfig;

pub(super) const IAC: u8 = 255;
pub(super) const NOP: [u8; 2] = [IAC, 241];
pub(super) const WILL: u8 = 251;
const WONT: u8 = 252;
pub(super) const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;
const OPT_ECHO: u8 = 1;
pub(super) const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
const OPT_NAWS: u8 = 31;

pub(super) fn negotiate_response(
    command: u8,
    option: u8,
    send_naws: bool,
    send_sga: bool,
) -> Vec<u8> {
    match command {
        WILL => {
            if option == OPT_ECHO || (send_sga && option == OPT_SUPPRESS_GO_AHEAD) {
                vec![IAC, DO, option]
            } else {
                vec![IAC, DONT, option]
            }
        }
        DO => {
            if send_naws && option == OPT_NAWS {
                vec![IAC, WILL, option]
            } else {
                vec![IAC, WONT, option]
            }
        }
        WONT => vec![IAC, DONT, option],
        DONT => vec![IAC, WONT, option],
        _ => vec![],
    }
}

pub(super) fn maybe_build_naws(
    cols: u16,
    rows: u16,
    config: &TelnetSessionConfig,
) -> Option<Vec<u8>> {
    if config.raw_tcp || !config.send_naws {
        return None;
    }
    Some(vec![
        IAC,
        SB,
        OPT_NAWS,
        (cols >> 8) as u8,
        (cols & 0xff) as u8,
        (rows >> 8) as u8,
        (rows & 0xff) as u8,
        IAC,
        SE,
    ])
}

pub(super) fn unescape_iac_iac(data: &[u8]) -> Vec<u8> {
    let mut visible = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == IAC && index + 1 < data.len() && data[index + 1] == IAC {
            visible.push(IAC);
            index += 2;
        } else {
            visible.push(data[index]);
            index += 1;
        }
    }
    visible
}

#[derive(Default)]
enum DecodeState {
    #[default]
    Data,
    Command,
    Negotiate(u8),
    Subnegotiation,
    SubnegotiationCommand,
}

/// TCP and relay copies may split anywhere, including inside IAC commands.
/// Subnegotiation is discarded incrementally without buffering server data.
#[derive(Default)]
pub(super) struct TelnetCommandDecoder {
    state: DecodeState,
}

impl TelnetCommandDecoder {
    pub(super) fn decode(&mut self, data: &[u8], on_negotiate: &mut impl FnMut(u8, u8)) -> Vec<u8> {
        let mut visible = Vec::with_capacity(data.len());
        for &byte in data {
            self.state = match self.state {
                DecodeState::Data if byte == IAC => DecodeState::Command,
                DecodeState::Data => {
                    visible.push(byte);
                    DecodeState::Data
                }
                DecodeState::Command => match byte {
                    IAC => {
                        visible.push(IAC);
                        DecodeState::Data
                    }
                    WILL | WONT | DO | DONT => DecodeState::Negotiate(byte),
                    SB => DecodeState::Subnegotiation,
                    _ => DecodeState::Data,
                },
                DecodeState::Negotiate(command) => {
                    on_negotiate(command, byte);
                    DecodeState::Data
                }
                DecodeState::Subnegotiation if byte == IAC => DecodeState::SubnegotiationCommand,
                DecodeState::Subnegotiation => DecodeState::Subnegotiation,
                DecodeState::SubnegotiationCommand if byte == SE => DecodeState::Data,
                DecodeState::SubnegotiationCommand => DecodeState::Subnegotiation,
            };
        }
        visible
    }
}

/// Telnet protocol escaping without text-input normalization.
pub(super) fn escape_telnet_application_data(data: &[u8], raw_tcp: bool) -> Vec<u8> {
    if raw_tcp {
        return data.to_vec();
    }
    let mut escaped = Vec::with_capacity(data.len());
    for &byte in data {
        escaped.push(byte);
        if byte == IAC {
            escaped.push(IAC);
        }
    }
    escaped
}

#[cfg(test)]
mod raw_input_tests {
    use super::escape_telnet_application_data;

    #[test]
    fn binary_input_preserves_controls_and_only_escapes_telnet_iac() {
        let bytes = [0, 13, 10, 127, 255, 128];
        assert_eq!(
            escape_telnet_application_data(&bytes, false),
            [0, 13, 10, 127, 255, 255, 128]
        );
        assert_eq!(escape_telnet_application_data(&bytes, true), bytes);
    }
}

#[cfg(test)]
mod decoder_tests {
    use super::{DO, IAC, OPT_ECHO, OPT_NAWS, SB, SE, TelnetCommandDecoder, WILL};

    #[test]
    fn negotiation_escaped_data_and_subnegotiation_survive_every_tcp_split() {
        let wire = [
            b'a', IAC, WILL, OPT_ECHO, IAC, IAC, IAC, SB, OPT_NAWS, 0, IAC, IAC, 0, IAC, SE, IAC,
            DO, OPT_NAWS, b'b',
        ];
        for split in 0..=wire.len() {
            let mut decoder = TelnetCommandDecoder::default();
            let mut negotiations = Vec::new();
            let mut negotiated = |command, option| negotiations.push((command, option));
            let mut visible = decoder.decode(&wire[..split], &mut negotiated);
            visible.extend(decoder.decode(&wire[split..], &mut negotiated));
            assert_eq!(visible, [b'a', IAC, b'b']);
            assert_eq!(negotiations, [(WILL, OPT_ECHO), (DO, OPT_NAWS)]);
        }
        let mut decoder = TelnetCommandDecoder::default();
        let mut visible = Vec::new();
        let mut negotiations = Vec::new();
        for byte in wire {
            visible.extend(decoder.decode(&[byte], &mut |command, option| {
                negotiations.push((command, option))
            }));
        }
        assert_eq!(visible, [b'a', IAC, b'b']);
        assert_eq!(negotiations, [(WILL, OPT_ECHO), (DO, OPT_NAWS)]);
    }
}

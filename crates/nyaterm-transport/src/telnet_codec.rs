pub(super) const IAC: u8 = 255;
pub(super) const NOP: [u8; 2] = [IAC, 241];
pub(super) const WILL: u8 = 251;
pub(super) const WONT: u8 = 252;
pub(super) const DO: u8 = 253;
pub(super) const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;
const OPT_ECHO: u8 = 1;
pub(super) const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
pub(super) const OPT_NAWS: u8 = 31;

pub(super) fn negotiate_response(command: u8, option: u8, send_sga: bool) -> Vec<u8> {
    match command {
        WILL => {
            if option == OPT_ECHO || (send_sga && option == OPT_SUPPRESS_GO_AHEAD) {
                vec![IAC, DO, option]
            } else {
                vec![IAC, DONT, option]
            }
        }
        DO => {
            if send_sga && option == OPT_SUPPRESS_GO_AHEAD {
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

pub(super) fn naws_subnegotiation(cols: u16, rows: u16) -> Vec<u8> {
    let mut wire = Vec::with_capacity(13);
    wire.extend_from_slice(&[IAC, SB, OPT_NAWS]);
    // RFC 1073: escape only the binary dimensions, not the frame delimiters.
    for byte in cols.to_be_bytes().into_iter().chain(rows.to_be_bytes()) {
        wire.push(byte);
        if byte == IAC {
            wire.push(IAC);
        }
    }
    wire.extend_from_slice(&[IAC, SE]);
    wire
}

#[derive(Clone, Copy)]
enum NawsNegotiation {
    Disabled,
    Offered,
    Enabled,
}

/// Pure NAWS transitions. The transport adapter serializes each transition
/// with its control-message enqueue, including failed enqueue handling.
#[derive(Clone, Copy)]
pub(super) struct TelnetNawsState {
    negotiation: NawsNegotiation,
    cols: u16,
    rows: u16,
}

impl TelnetNawsState {
    pub(super) fn new(cols: u16, rows: u16) -> Self {
        Self {
            negotiation: NawsNegotiation::Disabled,
            cols,
            rows,
        }
    }

    pub(super) fn offer(&mut self) -> Vec<u8> {
        if !matches!(self.negotiation, NawsNegotiation::Disabled) {
            return Vec::new();
        }
        self.negotiation = NawsNegotiation::Offered;
        vec![IAC, WILL, OPT_NAWS]
    }

    pub(super) fn accept(&mut self) -> Vec<u8> {
        let mut response = match self.negotiation {
            NawsNegotiation::Disabled => vec![IAC, WILL, OPT_NAWS],
            NawsNegotiation::Offered => Vec::new(),
            // RFC 854 prohibits acknowledging a mode already in effect.
            NawsNegotiation::Enabled => return Vec::new(),
        };
        self.negotiation = NawsNegotiation::Enabled;
        response.extend(naws_subnegotiation(self.cols, self.rows));
        response
    }

    pub(super) fn reject(&mut self) -> Vec<u8> {
        let response = if matches!(self.negotiation, NawsNegotiation::Enabled) {
            vec![IAC, WONT, OPT_NAWS]
        } else {
            Vec::new()
        };
        self.negotiation = NawsNegotiation::Disabled;
        response
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.cols = cols;
        self.rows = rows;
        if matches!(self.negotiation, NawsNegotiation::Enabled) {
            naws_subnegotiation(cols, rows)
        } else {
            Vec::new()
        }
    }
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
mod naws_tests {
    use super::{TelnetNawsState, naws_subnegotiation};

    #[test]
    fn dimensions_use_network_order_and_escape_iac_only_inside_the_payload() {
        let cases: &[(u16, u16, &[u8])] = &[
            (80, 24, &[255, 250, 31, 0, 80, 0, 24, 255, 240]),
            (0, 0, &[255, 250, 31, 0, 0, 0, 0, 255, 240]),
            (255, 24, &[255, 250, 31, 0, 255, 255, 0, 24, 255, 240]),
            (80, 255, &[255, 250, 31, 0, 80, 0, 255, 255, 255, 240]),
            (256, 256, &[255, 250, 31, 1, 0, 1, 0, 255, 240]),
            (
                65280,
                65280,
                &[255, 250, 31, 255, 255, 0, 255, 255, 0, 255, 240],
            ),
            (
                255,
                255,
                &[255, 250, 31, 0, 255, 255, 0, 255, 255, 255, 240],
            ),
            (
                65535,
                65535,
                &[
                    255, 250, 31, 255, 255, 255, 255, 255, 255, 255, 255, 255, 240,
                ],
            ),
        ];
        for &(cols, rows, expected) in cases {
            assert_eq!(naws_subnegotiation(cols, rows), expected, "{cols}x{rows}");
        }
    }

    #[test]
    fn offer_acceptance_reports_cached_size_and_duplicate_acceptance_is_silent() {
        let mut state = TelnetNawsState::new(80, 24);
        assert!(state.resize(90, 30).is_empty());
        assert_eq!(state.offer(), [255, 251, 31]);
        assert!(state.offer().is_empty());
        assert!(state.resize(100, 40).is_empty());
        assert_eq!(state.accept(), [255, 250, 31, 0, 100, 0, 40, 255, 240]);
        assert!(state.accept().is_empty());
        assert_eq!(
            state.resize(120, 50),
            [255, 250, 31, 0, 120, 0, 50, 255, 240]
        );
    }

    #[test]
    fn refusal_and_disable_require_a_new_will_before_reacceptance() {
        for previously_enabled in [false, true] {
            let mut state = TelnetNawsState::new(80, 24);
            state.offer();
            if previously_enabled {
                state.accept();
            }
            assert_eq!(
                state.reject(),
                if previously_enabled {
                    vec![255, 252, 31]
                } else {
                    vec![]
                }
            );
            assert!(state.reject().is_empty());
            assert!(state.resize(100, 40).is_empty());
            assert_eq!(
                state.accept(),
                [255, 251, 31, 255, 250, 31, 0, 100, 0, 40, 255, 240]
            );
        }
    }
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

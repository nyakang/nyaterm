//! TFTP wire format (RFC 1350) and option negotiation (RFC 2347/2348/2349).

use std::fmt;

pub const OP_RRQ: u16 = 1;
pub const OP_WRQ: u16 = 2;
pub const OP_DATA: u16 = 3;
pub const OP_ACK: u16 = 4;
pub const OP_ERROR: u16 = 5;
pub const OP_OACK: u16 = 6;

pub const ERR_NOT_DEFINED: u16 = 0;
pub const ERR_FILE_NOT_FOUND: u16 = 1;
pub const ERR_ACCESS_VIOLATION: u16 = 2;
pub const ERR_DISK_FULL: u16 = 3;
pub const ERR_ILLEGAL_OPERATION: u16 = 4;
pub const ERR_UNKNOWN_TID: u16 = 5;
pub const ERR_FILE_EXISTS: u16 = 6;

pub const DEFAULT_BLOCK_SIZE: u16 = 512;
pub const MIN_BLOCK_SIZE: u16 = 8;
pub const MAX_BLOCK_SIZE: u16 = 65464;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Octet,
    NetAscii,
    Mail,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "octet" => Some(Self::Octet),
            "netascii" => Some(Self::NetAscii),
            // Mail mode is obsolete and explicitly rejected by the server.
            "mail" => Some(Self::Mail),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub write: bool,
    pub file: String,
    pub mode: Mode,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    TooShort,
    Malformed,
    UnsupportedOpcode(u16),
    InvalidMode(String),
}

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => f.write_str("packet too short"),
            Self::Malformed => f.write_str("malformed packet"),
            Self::UnsupportedOpcode(opcode) => write!(f, "unsupported opcode {opcode}"),
            Self::InvalidMode(mode) => write!(f, "unsupported mode {mode}"),
        }
    }
}

impl std::error::Error for PacketError {}

pub fn opcode(buf: &[u8]) -> Option<u16> {
    (buf.len() >= 2).then(|| u16::from_be_bytes([buf[0], buf[1]]))
}

pub fn parse_request(buf: &[u8]) -> Result<Request, PacketError> {
    let opcode = opcode(buf).ok_or(PacketError::TooShort)?;
    let write = match opcode {
        OP_RRQ => false,
        OP_WRQ => true,
        other => return Err(PacketError::UnsupportedOpcode(other)),
    };
    let mut fields = buf[2..].split(|byte| *byte == 0);
    let file = fields
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| !value.is_empty())
        .ok_or(PacketError::Malformed)?;
    let mode = fields
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .ok_or(PacketError::Malformed)?;
    let mode = Mode::parse(mode).ok_or_else(|| PacketError::InvalidMode(mode.to_string()))?;
    let mut options = Vec::new();
    while let Some(name) = fields.next() {
        if name.is_empty() {
            break;
        }
        let Some(value) = fields.next() else {
            return Err(PacketError::Malformed);
        };
        let (Ok(name), Ok(value)) = (std::str::from_utf8(name), std::str::from_utf8(value)) else {
            return Err(PacketError::Malformed);
        };
        options.push((name.to_ascii_lowercase(), value.to_string()));
    }
    Ok(Request {
        write,
        file: file.to_string(),
        mode,
        options,
    })
}

pub fn write_data(block: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&OP_DATA.to_be_bytes());
    out.extend_from_slice(&block.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn parse_data(buf: &[u8]) -> Result<(u16, &[u8]), PacketError> {
    if buf.len() < 4 || opcode(buf) != Some(OP_DATA) {
        return Err(PacketError::Malformed);
    }
    Ok((u16::from_be_bytes([buf[2], buf[3]]), &buf[4..]))
}

pub fn write_ack(block: u16) -> [u8; 4] {
    let mut out = [0u8; 4];
    out[..2].copy_from_slice(&OP_ACK.to_be_bytes());
    out[2..].copy_from_slice(&block.to_be_bytes());
    out
}

pub fn parse_ack(buf: &[u8]) -> Result<u16, PacketError> {
    if buf.len() != 4 || opcode(buf) != Some(OP_ACK) {
        return Err(PacketError::Malformed);
    }
    Ok(u16::from_be_bytes([buf[2], buf[3]]))
}

pub fn write_error(code: u16, message: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + message.len());
    out.extend_from_slice(&OP_ERROR.to_be_bytes());
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(message.as_bytes());
    out.push(0);
    out
}

pub fn parse_error(buf: &[u8]) -> Result<(u16, String), PacketError> {
    if buf.len() < 4 || opcode(buf) != Some(OP_ERROR) {
        return Err(PacketError::Malformed);
    }
    let code = u16::from_be_bytes([buf[2], buf[3]]);
    let message = String::from_utf8_lossy(&buf[4..])
        .trim_end_matches('\0')
        .to_string();
    Ok((code, message))
}

pub fn write_oack(options: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&OP_OACK.to_be_bytes());
    for (name, value) in options {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    out
}

/// Options we agree to apply for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    pub block_size: u16,
    pub timeout_secs: u8,
    /// `Some` for octet-mode reads (server-side file size) and for writes when
    /// the client declared a size.
    pub tsize: Option<u64>,
    /// The subset of requested options we accepted, ready for an OACK.
    pub accepted: Vec<(String, String)>,
}

impl Negotiated {
    pub fn has_options(&self) -> bool {
        !self.accepted.is_empty()
    }
}

/// Accept only options the client actually asked for, clamping out-of-range
/// values instead of failing the transfer.
pub fn negotiate(request: &Request, file_size: Option<u64>) -> Negotiated {
    let mut negotiated = Negotiated {
        block_size: DEFAULT_BLOCK_SIZE,
        timeout_secs: 0,
        tsize: None,
        accepted: Vec::new(),
    };
    for (name, value) in &request.options {
        match name.as_str() {
            "blksize" => {
                let Ok(requested) = value.parse::<u32>() else {
                    continue;
                };
                if requested < u32::from(MIN_BLOCK_SIZE) {
                    continue;
                }
                let clamped = requested.min(u32::from(MAX_BLOCK_SIZE)) as u16;
                negotiated.block_size = clamped;
                negotiated
                    .accepted
                    .push((name.clone(), clamped.to_string()));
            }
            "timeout" => {
                let Ok(requested) = value.parse::<u8>() else {
                    continue;
                };
                if requested == 0 {
                    continue;
                }
                negotiated.timeout_secs = requested;
                negotiated
                    .accepted
                    .push((name.clone(), requested.to_string()));
            }
            "tsize" => {
                if request.mode != Mode::Octet {
                    continue;
                }
                let value = match (request.write, file_size) {
                    (false, Some(size)) => size,
                    (true, _) => value.parse::<u64>().unwrap_or(0),
                    (false, None) => continue,
                };
                negotiated.tsize = Some(value);
                negotiated.accepted.push((name.clone(), value.to_string()));
            }
            _ => {}
        }
    }
    negotiated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_with_options() {
        let mut request = OP_RRQ.to_be_bytes().to_vec();
        request.extend_from_slice(b"firmware.bin");
        request.push(0);
        request.extend_from_slice(b"octet");
        request.push(0);
        request.extend_from_slice(b"blksize");
        request.push(0);
        request.extend_from_slice(b"1468");
        request.push(0);
        request.extend_from_slice(b"tsize");
        request.push(0);
        request.extend_from_slice(b"0");
        request.push(0);

        let request = parse_request(&request).unwrap();
        assert!(!request.write);
        assert_eq!(request.file, "firmware.bin");
        assert_eq!(request.mode, Mode::Octet);
        assert_eq!(
            request.options,
            vec![
                ("blksize".to_string(), "1468".to_string()),
                ("tsize".to_string(), "0".to_string())
            ]
        );
    }

    #[test]
    fn rejects_unknown_mode_and_truncated_options() {
        let mut buf = OP_RRQ.to_be_bytes().to_vec();
        buf.extend_from_slice(b"a\0binary\0");
        assert!(matches!(
            parse_request(&buf),
            Err(PacketError::InvalidMode(_))
        ));

        let mut buf = OP_RRQ.to_be_bytes().to_vec();
        buf.extend_from_slice(b"a\0octet\0blksize");
        assert_eq!(parse_request(&buf), Err(PacketError::Malformed));
    }

    #[test]
    fn data_ack_and_error_round_trip() {
        let packet = write_data(7, b"hello");
        assert_eq!(parse_data(&packet).unwrap(), (7, &b"hello"[..]));
        assert_eq!(parse_ack(&write_ack(9)).unwrap(), 9);

        let error = write_error(ERR_ACCESS_VIOLATION, "denied");
        assert_eq!(
            parse_error(&error).unwrap(),
            (ERR_ACCESS_VIOLATION, "denied".to_string())
        );
    }

    #[test]
    fn oack_encodes_requested_options() {
        let options = vec![("blksize".to_string(), "1024".to_string())];
        let packet = write_oack(&options);
        assert_eq!(&packet[..2], &OP_OACK.to_be_bytes());
        let mut expected = b"blksize".to_vec();
        expected.push(0);
        expected.extend_from_slice(b"1024");
        expected.push(0);
        assert!(packet.ends_with(&expected));
    }

    #[test]
    fn negotiation_clamps_and_filters() {
        let request = Request {
            write: false,
            file: "a".into(),
            mode: Mode::Octet,
            options: vec![
                ("blksize".into(), "4".into()),
                ("timeout".into(), "5".into()),
                ("tsize".into(), "0".into()),
                ("windowsize".into(), "8".into()),
            ],
        };
        let negotiated = negotiate(&request, Some(2048));
        // 4 is below the minimum, so the default stays and no OACK entry is sent.
        assert_eq!(negotiated.block_size, DEFAULT_BLOCK_SIZE);
        assert_eq!(negotiated.timeout_secs, 5);
        assert_eq!(negotiated.tsize, Some(2048));
        assert_eq!(
            negotiated.accepted,
            vec![
                ("timeout".to_string(), "5".to_string()),
                ("tsize".to_string(), "2048".to_string())
            ]
        );
    }

    #[test]
    fn tsize_is_ignored_for_netascii() {
        let request = Request {
            write: true,
            file: "a".into(),
            mode: Mode::NetAscii,
            options: vec![("tsize".into(), "99".into())],
        };
        assert_eq!(negotiate(&request, None).tsize, None);
    }
}

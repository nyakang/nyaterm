//! Hand-rolled protobuf codecs matching upstream mobile-shell/mosh field numbers.
//!
//! Width/height are proto2 `int32` encoded as varints (not zigzag), matching mosh-go.

use crate::error::{Error, Result};

const WIRE_VARINT: u64 = 0;
const WIRE_BYTES: u64 = 2;

/// Outer transport wrapper (`TransportBuffers.Instruction`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransportInstruction {
    pub protocol_version: u32,
    pub old_num: u64,
    pub new_num: u64,
    pub ack_num: u64,
    pub throwaway_num: u64,
    pub diff: Vec<u8>,
    pub chaff: Vec<u8>,
}

/// One host-side instruction (`HostBuffers.Instruction` extensions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInstruction {
    pub hoststring: Vec<u8>,
    pub width: i32,
    pub height: i32,
    /// -1 means absent.
    pub echo_ack_num: i64,
}

/// One client-side instruction (`ClientBuffers.Instruction` extensions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserInstruction {
    pub keys: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

impl TransportInstruction {
    /// Encode like mosh-go: always emit old/new/ack/throwaway fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        if self.protocol_version != 0 {
            append_tag_varint(&mut b, 1, self.protocol_version as u64);
        }
        append_tag_varint(&mut b, 2, self.old_num);
        append_tag_varint(&mut b, 3, self.new_num);
        append_tag_varint(&mut b, 4, self.ack_num);
        append_tag_varint(&mut b, 5, self.throwaway_num);
        if !self.diff.is_empty() {
            append_tag_bytes(&mut b, 6, &self.diff);
        }
        if !self.chaff.is_empty() {
            append_tag_bytes(&mut b, 7, &self.chaff);
        }
        b
    }

    pub fn decode(mut data: &[u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !data.is_empty() {
            let (field, wtype, n) = decode_tag(data)?;
            data = &data[n..];
            match (field, wtype) {
                (1, WIRE_VARINT) => {
                    let (v, n) = decode_varint(data)?;
                    msg.protocol_version = v as u32;
                    data = &data[n..];
                }
                (2, WIRE_VARINT) => {
                    let (v, n) = decode_varint(data)?;
                    msg.old_num = v;
                    data = &data[n..];
                }
                (3, WIRE_VARINT) => {
                    let (v, n) = decode_varint(data)?;
                    msg.new_num = v;
                    data = &data[n..];
                }
                (4, WIRE_VARINT) => {
                    let (v, n) = decode_varint(data)?;
                    msg.ack_num = v;
                    data = &data[n..];
                }
                (5, WIRE_VARINT) => {
                    let (v, n) = decode_varint(data)?;
                    msg.throwaway_num = v;
                    data = &data[n..];
                }
                (6, WIRE_BYTES) => {
                    let (v, n) = decode_bytes(data)?;
                    msg.diff = v;
                    data = &data[n..];
                }
                (7, WIRE_BYTES) => {
                    let (v, n) = decode_bytes(data)?;
                    msg.chaff = v;
                    data = &data[n..];
                }
                (_, _) => {
                    let n = skip_field(data, wtype)?;
                    data = &data[n..];
                }
            }
        }
        Ok(msg)
    }
}

impl UserInstruction {
    pub fn keystroke(keys: impl Into<Vec<u8>>) -> Self {
        Self {
            keys: keys.into(),
            ..Default::default()
        }
    }

    pub fn resize(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            ..Default::default()
        }
    }

    fn encode_one(&self) -> Vec<u8> {
        let mut b = Vec::new();
        if !self.keys.is_empty() {
            // field 2 → Keystroke { field 4: keys }
            let mut keystroke = Vec::new();
            append_tag_bytes(&mut keystroke, 4, &self.keys);
            append_tag_bytes(&mut b, 2, &keystroke);
        }
        if self.width > 0 || self.height > 0 {
            // field 3 → ResizeMessage { field 5: width, field 6: height } as int32 varints
            let mut resize = Vec::new();
            append_tag_varint(&mut resize, 5, self.width as u32 as u64);
            append_tag_varint(&mut resize, 6, self.height as u32 as u64);
            append_tag_bytes(&mut b, 3, &resize);
        }
        b
    }

    /// Encode as a `UserMessage` (repeated Instruction field 1).
    pub fn encode_message(instructions: &[UserInstruction]) -> Vec<u8> {
        let mut outer = Vec::new();
        for inst in instructions {
            append_tag_bytes(&mut outer, 1, &inst.encode_one());
        }
        outer
    }

    fn decode_one(mut data: &[u8]) -> Result<Self> {
        let mut msg = Self::default();
        while !data.is_empty() {
            let (field, wtype, n) = decode_tag(data)?;
            data = &data[n..];
            match (field, wtype) {
                (2, WIRE_BYTES) => {
                    // Keystroke { field 4: keys }
                    let (inner, n) = decode_bytes(data)?;
                    data = &data[n..];
                    let mut rest = inner.as_slice();
                    while !rest.is_empty() {
                        let (f, wt, nn) = decode_tag(rest)?;
                        rest = &rest[nn..];
                        if f == 4 && wt == WIRE_BYTES {
                            let (v, nn) = decode_bytes(rest)?;
                            msg.keys.extend_from_slice(&v);
                            rest = &rest[nn..];
                        } else {
                            let nn = skip_field(rest, wt)?;
                            rest = &rest[nn..];
                        }
                    }
                }
                (3, WIRE_BYTES) => {
                    let (inner, n) = decode_bytes(data)?;
                    data = &data[n..];
                    let mut rest = inner.as_slice();
                    while !rest.is_empty() {
                        let (f, wt, nn) = decode_tag(rest)?;
                        rest = &rest[nn..];
                        match (f, wt) {
                            (5, WIRE_VARINT) => {
                                let (v, nn) = decode_varint(rest)?;
                                msg.width = v as i32;
                                rest = &rest[nn..];
                            }
                            (6, WIRE_VARINT) => {
                                let (v, nn) = decode_varint(rest)?;
                                msg.height = v as i32;
                                rest = &rest[nn..];
                            }
                            _ => {
                                let nn = skip_field(rest, wt)?;
                                rest = &rest[nn..];
                            }
                        }
                    }
                }
                _ => {
                    let n = skip_field(data, wtype)?;
                    data = &data[n..];
                }
            }
        }
        Ok(msg)
    }

    /// Decode a `UserMessage` (repeated Instruction field 1).
    pub fn decode_message(mut data: &[u8]) -> Result<Vec<UserInstruction>> {
        let mut out = Vec::new();
        while !data.is_empty() {
            let (field, wtype, n) = decode_tag(data)?;
            data = &data[n..];
            if field == 1 && wtype == WIRE_BYTES {
                let (inner, n) = decode_bytes(data)?;
                data = &data[n..];
                out.push(Self::decode_one(&inner)?);
            } else {
                let n = skip_field(data, wtype)?;
                data = &data[n..];
            }
        }
        Ok(out)
    }
}

impl HostInstruction {
    fn decode_one(mut data: &[u8]) -> Result<Self> {
        let mut msg = Self {
            echo_ack_num: -1,
            ..Default::default()
        };
        while !data.is_empty() {
            let (field, wtype, n) = decode_tag(data)?;
            data = &data[n..];
            match (field, wtype) {
                (2, WIRE_BYTES) => {
                    // HostBytes { field 4: hoststring }
                    let (inner, n) = decode_bytes(data)?;
                    data = &data[n..];
                    let mut rest = inner.as_slice();
                    while !rest.is_empty() {
                        let (f, wt, nn) = decode_tag(rest)?;
                        rest = &rest[nn..];
                        if f == 4 && wt == WIRE_BYTES {
                            let (v, nn) = decode_bytes(rest)?;
                            msg.hoststring = v;
                            rest = &rest[nn..];
                        } else {
                            let nn = skip_field(rest, wt)?;
                            rest = &rest[nn..];
                        }
                    }
                }
                (3, WIRE_BYTES) => {
                    let (inner, n) = decode_bytes(data)?;
                    data = &data[n..];
                    let mut rest = inner.as_slice();
                    while !rest.is_empty() {
                        let (f, wt, nn) = decode_tag(rest)?;
                        rest = &rest[nn..];
                        match (f, wt) {
                            (5, WIRE_VARINT) => {
                                let (v, nn) = decode_varint(rest)?;
                                msg.width = v as i32;
                                rest = &rest[nn..];
                            }
                            (6, WIRE_VARINT) => {
                                let (v, nn) = decode_varint(rest)?;
                                msg.height = v as i32;
                                rest = &rest[nn..];
                            }
                            _ => {
                                let nn = skip_field(rest, wt)?;
                                rest = &rest[nn..];
                            }
                        }
                    }
                }
                (7, WIRE_BYTES) => {
                    let (inner, n) = decode_bytes(data)?;
                    data = &data[n..];
                    let mut rest = inner.as_slice();
                    while !rest.is_empty() {
                        let (f, wt, nn) = decode_tag(rest)?;
                        rest = &rest[nn..];
                        if f == 8 && wt == WIRE_VARINT {
                            let (v, nn) = decode_varint(rest)?;
                            msg.echo_ack_num = v as i64;
                            rest = &rest[nn..];
                        } else {
                            let nn = skip_field(rest, wt)?;
                            rest = &rest[nn..];
                        }
                    }
                }
                _ => {
                    let n = skip_field(data, wtype)?;
                    data = &data[n..];
                }
            }
        }
        Ok(msg)
    }

    /// Decode a `HostMessage` (repeated Instruction field 1).
    pub fn decode_message(mut data: &[u8]) -> Result<Vec<HostInstruction>> {
        let mut out = Vec::new();
        while !data.is_empty() {
            let (field, wtype, n) = decode_tag(data)?;
            data = &data[n..];
            if field == 1 && wtype == WIRE_BYTES {
                let (inner, n) = decode_bytes(data)?;
                data = &data[n..];
                out.push(Self::decode_one(&inner)?);
            } else {
                let n = skip_field(data, wtype)?;
                data = &data[n..];
            }
        }
        Ok(out)
    }

    /// Encode HostMessage for tests / local servers.
    pub fn encode_message(instructions: &[HostInstruction]) -> Vec<u8> {
        let mut outer = Vec::new();
        for hi in instructions {
            let mut inner = Vec::new();
            if !hi.hoststring.is_empty() {
                let mut hb = Vec::new();
                append_tag_bytes(&mut hb, 4, &hi.hoststring);
                append_tag_bytes(&mut inner, 2, &hb);
            }
            if hi.width > 0 || hi.height > 0 {
                let mut r = Vec::new();
                append_tag_varint(&mut r, 5, hi.width as u32 as u64);
                append_tag_varint(&mut r, 6, hi.height as u32 as u64);
                append_tag_bytes(&mut inner, 3, &r);
            }
            if hi.echo_ack_num >= 0 {
                let mut e = Vec::new();
                append_tag_varint(&mut e, 8, hi.echo_ack_num as u64);
                append_tag_bytes(&mut inner, 7, &e);
            }
            append_tag_bytes(&mut outer, 1, &inner);
        }
        outer
    }
}

fn append_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn append_tag(buf: &mut Vec<u8>, field: u64, wtype: u64) {
    append_varint(buf, (field << 3) | wtype);
}

fn append_tag_varint(buf: &mut Vec<u8>, field: u64, v: u64) {
    append_tag(buf, field, WIRE_VARINT);
    append_varint(buf, v);
}

fn append_tag_bytes(buf: &mut Vec<u8>, field: u64, data: &[u8]) {
    append_tag(buf, field, WIRE_BYTES);
    append_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn decode_varint(data: &[u8]) -> Result<(u64, usize)> {
    let mut v = 0u64;
    for (i, &c) in data.iter().enumerate() {
        if i >= 10 {
            return Err(Error::Protobuf("varint too long".into()));
        }
        v |= u64::from(c & 0x7f) << (7 * i);
        if c < 0x80 {
            return Ok((v, i + 1));
        }
    }
    Err(Error::Protobuf("truncated varint".into()))
}

fn decode_tag(data: &[u8]) -> Result<(u64, u64, usize)> {
    let (v, n) = decode_varint(data)?;
    Ok((v >> 3, v & 7, n))
}

fn decode_bytes(data: &[u8]) -> Result<(Vec<u8>, usize)> {
    let (len, n) = decode_varint(data)?;
    let len = len as usize;
    if data.len() < n + len {
        return Err(Error::Protobuf("truncated bytes".into()));
    }
    Ok((data[n..n + len].to_vec(), n + len))
}

fn skip_field(data: &[u8], wtype: u64) -> Result<usize> {
    match wtype {
        WIRE_VARINT => {
            let (_, n) = decode_varint(data)?;
            Ok(n)
        }
        WIRE_BYTES => {
            let (len, n) = decode_varint(data)?;
            let total = n + len as usize;
            if data.len() < total {
                return Err(Error::Protobuf("truncated skip".into()));
            }
            Ok(total)
        }
        5 => Ok(4),
        1 => Ok(8),
        _ => Err(Error::Protobuf(format!("unknown wire type {wtype}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::{HostInstruction, TransportInstruction, UserInstruction};

    // Cases adapted from unixshells/mosh-go pb_test.go

    #[test]
    fn transport_roundtrip_always_emits_nums() {
        let msg = TransportInstruction {
            protocol_version: 2,
            old_num: 0,
            new_num: 1,
            ack_num: 0,
            throwaway_num: 0,
            diff: b"hello from server".to_vec(),
            chaff: vec![],
        };
        let encoded = msg.encode();
        // old_num=0 is still present as a field (mosh-go always encodes).
        assert!(encoded.len() > 10);
        let decoded = TransportInstruction::decode(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn transport_instruction_full_roundtrip() {
        let ti = TransportInstruction {
            protocol_version: 2,
            old_num: 10,
            new_num: 11,
            ack_num: 9,
            throwaway_num: 8,
            diff: b"hello".to_vec(),
            chaff: vec![0xde, 0xad],
        };
        let got = TransportInstruction::decode(&ti.encode()).unwrap();
        assert_eq!(got, ti);
    }

    #[test]
    fn transport_instruction_empty() {
        let ti = TransportInstruction::default();
        let got = TransportInstruction::decode(&ti.encode()).unwrap();
        assert_eq!(got.old_num, 0);
        assert_eq!(got.new_num, 0);
        assert!(got.diff.is_empty());
    }

    #[test]
    fn transport_instruction_large_values() {
        let ti = TransportInstruction {
            old_num: (1u64 << 63) - 1,
            new_num: (1u64 << 63) - 1,
            ack_num: (1u64 << 63) - 1,
            throwaway_num: (1u64 << 63) - 1,
            ..Default::default()
        };
        let got = TransportInstruction::decode(&ti.encode()).unwrap();
        assert_eq!(got.old_num, ti.old_num);
        assert_eq!(got.new_num, ti.new_num);
        assert_eq!(got.ack_num, ti.ack_num);
        assert_eq!(got.throwaway_num, ti.throwaway_num);
    }

    #[test]
    fn transport_malformed_truncated_varint() {
        let data = vec![0x80u8; 20];
        assert!(TransportInstruction::decode(&data).is_err());
    }

    #[test]
    fn transport_malformed_length_exceeds_buffer() {
        // Field 6 (diff bytes): tag (6<<3)|2 = 0x32, claim huge length.
        let mut data = vec![0x32];
        // varint 1<<30
        let mut v = 1u64 << 30;
        while v >= 0x80 {
            data.push((v as u8) | 0x80);
            v >>= 7;
        }
        data.push(v as u8);
        data.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        assert!(TransportInstruction::decode(&data).is_err());
    }

    #[test]
    fn transport_empty_input_ok() {
        let got = TransportInstruction::decode(&[]).unwrap();
        assert_eq!(got, TransportInstruction::default());
    }

    #[test]
    fn transport_unknown_field_skipped() {
        // field 99 wire type 0 (varint) value 1, then valid field 3 new_num=5
        // tag for field 99 wire 0: (99<<3)|0 = 792 = 0x318 → varint 0x98 0x06
        let mut data = vec![0x98, 0x06, 0x01];
        // field 3 new_num: tag (3<<3)|0 = 0x18, value 5
        data.extend_from_slice(&[0x18, 0x05]);
        let got = TransportInstruction::decode(&data).unwrap();
        assert_eq!(got.new_num, 5);
    }

    #[test]
    fn user_keystroke_and_resize() {
        let msg = UserInstruction::encode_message(&[
            UserInstruction::keystroke(b"ls\n"),
            UserInstruction::resize(80, 24),
        ]);
        assert!(msg.windows(3).any(|w| w == b"ls\n"));
        assert!(msg.contains(&80));
        assert!(msg.contains(&24));
    }

    #[test]
    fn user_message_roundtrip() {
        let instrs = vec![
            UserInstruction::keystroke(b"ls -la\n"),
            UserInstruction::resize(120, 40),
            UserInstruction {
                keys: b"a".to_vec(),
                width: 80,
                height: 24,
            },
        ];
        let enc = UserInstruction::encode_message(&instrs);
        let got = UserInstruction::decode_message(&enc).unwrap();
        assert_eq!(got.len(), instrs.len());
        for (a, b) in got.iter().zip(instrs.iter()) {
            assert_eq!(a.keys, b.keys);
            assert_eq!(a.width, b.width);
            assert_eq!(a.height, b.height);
        }
    }

    #[test]
    fn user_message_empty() {
        let enc = UserInstruction::encode_message(&[]);
        let got = UserInstruction::decode_message(&enc).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn host_message_roundtrip() {
        let instrs = vec![
            HostInstruction {
                hoststring: b"\x1b[H\x1b[2J".to_vec(),
                echo_ack_num: -1,
                ..Default::default()
            },
            HostInstruction {
                width: 80,
                height: 24,
                echo_ack_num: -1,
                ..Default::default()
            },
            HostInstruction {
                echo_ack_num: 42,
                ..Default::default()
            },
            HostInstruction {
                hoststring: b"hello".to_vec(),
                width: 132,
                height: 43,
                echo_ack_num: 7,
            },
        ];
        let enc = HostInstruction::encode_message(&instrs);
        let got = HostInstruction::decode_message(&enc).unwrap();
        assert_eq!(got.len(), instrs.len());
        for (a, b) in got.iter().zip(instrs.iter()) {
            assert_eq!(a.hoststring, b.hoststring);
            assert_eq!(a.width, b.width);
            assert_eq!(a.height, b.height);
            assert_eq!(a.echo_ack_num, b.echo_ack_num);
        }
    }

    #[test]
    fn host_message_empty() {
        let enc = HostInstruction::encode_message(&[]);
        let got = HostInstruction::decode_message(&enc).unwrap();
        assert!(got.is_empty());
    }
}

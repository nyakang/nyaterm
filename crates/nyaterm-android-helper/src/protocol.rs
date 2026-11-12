//! scrcpy 5.0.1 server protocol. This module is never linked by the application.
use nyaterm_android::ipc::{Control, MAX_TEXT_BYTES};
use std::io::{self, Read};

pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;
pub const CONFIG_FLAG: u64 = 1 << 62;
pub const KEY_FLAG: u64 = 1 << 61;

pub enum VideoPacket {
    Session {
        width: u32,
        height: u32,
    },
    Media {
        config: bool,
        pts: u64,
        bytes: Vec<u8>,
    },
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
pub fn read_video(reader: &mut impl Read) -> io::Result<VideoPacket> {
    read_video_reusing(reader, &mut Vec::new())
}

pub fn read_video_reusing(
    reader: &mut impl Read,
    scratch: &mut Vec<u8>,
) -> io::Result<VideoPacket> {
    let mut header = [0; 12];
    reader.read_exact(&mut header)?;
    if header[0] & 0x80 != 0 {
        let width = u32::from_be_bytes(header[4..8].try_into().unwrap());
        let height = u32::from_be_bytes(header[8..12].try_into().unwrap());
        if !nyaterm_android::ipc::valid_capture(width, height) {
            return Err(invalid("screen dimensions exceed limit"));
        }
        return Ok(VideoPacket::Session { width, height });
    }
    let flags = u64::from_be_bytes(header[..8].try_into().unwrap());
    let length = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_MEDIA_BYTES {
        return Err(invalid("media packet length"));
    }
    let mut bytes = std::mem::take(scratch);
    bytes.resize(length, 0);
    reader.read_exact(&mut bytes)?;
    Ok(VideoPacket::Media {
        config: flags & CONFIG_FLAG != 0,
        pts: flags & (KEY_FLAG - 1),
        bytes,
    })
}

fn position(bytes: &mut Vec<u8>, x: u32, y: u32, width: u32, height: u32) -> io::Result<()> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 || x >= width || y >= height {
        return Err(invalid("input position"));
    }
    bytes.extend_from_slice(&x.to_be_bytes());
    bytes.extend_from_slice(&y.to_be_bytes());
    bytes.extend_from_slice(&(width as u16).to_be_bytes());
    bytes.extend_from_slice(&(height as u16).to_be_bytes());
    Ok(())
}

fn text(bytes: &mut Vec<u8>, value: &str) -> io::Result<()> {
    if value.len() > MAX_TEXT_BYTES || value.contains('\0') {
        return Err(invalid("input text limit"));
    }
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

pub fn encode(control: &Control) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match control {
        Control::Key {
            action,
            keycode,
            repeat,
            metastate,
        } => {
            if *action > 1 {
                return Err(invalid("key action"));
            }
            bytes.extend_from_slice(&[0, *action]);
            for value in [keycode, repeat, metastate] {
                bytes.extend_from_slice(&value.to_be_bytes());
            }
        }
        Control::Touch {
            action,
            x,
            y,
            width,
            height,
            ..
        } => {
            if ![0, 1, 2, 3].contains(action) {
                return Err(invalid("touch action"));
            }
            bytes.extend_from_slice(&[2, *action]);
            bytes.extend_from_slice(&u64::MAX.to_be_bytes()); // scrcpy POINTER_ID_MOUSE
            position(&mut bytes, *x, *y, *width, *height)?;
            bytes.extend_from_slice(
                &(if *action == 1 || *action == 3 {
                    0u16
                } else {
                    u16::MAX
                })
                .to_be_bytes(),
            );
            bytes.extend_from_slice(&(if *action <= 1 { 1u32 } else { 0u32 }).to_be_bytes());
            bytes.extend_from_slice(
                &(if *action == 1 || *action == 3 {
                    0u32
                } else {
                    1u32
                })
                .to_be_bytes(),
            );
        }
        Control::Scroll {
            x,
            y,
            width,
            height,
            horizontal,
            vertical,
            ..
        } => {
            if !horizontal.is_finite() || !vertical.is_finite() {
                return Err(invalid("scroll value"));
            }
            bytes.push(3);
            position(&mut bytes, *x, *y, *width, *height)?;
            for value in [horizontal, vertical] {
                let value =
                    (value.clamp(-16.0, 16.0) / 16.0 * 32768.0).clamp(-32768.0, 32767.0) as i16;
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            bytes.extend_from_slice(&0u32.to_be_bytes());
        }
        Control::Text { text: value } => {
            if !value.is_ascii() {
                return Err(invalid("use explicit paste for Unicode"));
            }
            bytes.push(1);
            text(&mut bytes, value)?;
        }
        Control::Paste {
            text: value,
            sequence,
        } => {
            bytes.push(9);
            bytes.extend_from_slice(&sequence.to_be_bytes());
            bytes.push(1);
            text(&mut bytes, value)?;
        }
        Control::Rotate => bytes.push(11),
        _ => return Err(invalid("not a device input")),
    }
    Ok(bytes)
}

pub fn read_device_message(reader: &mut impl Read) -> io::Result<Option<Control>> {
    let mut tag = [0; 1];
    reader.read_exact(&mut tag)?;
    match tag[0] {
        0 => {
            let mut header = [0; 4];
            reader.read_exact(&mut header)?;
            let length = u32::from_be_bytes(header) as usize;
            if length > MAX_TEXT_BYTES {
                return Err(invalid("clipboard length"));
            }
            let mut ignored = vec![0; length];
            reader.read_exact(&mut ignored)?;
            Ok(None) // Autosync is disabled; never copy unsolicited clipboard data to the host.
        }
        1 => {
            let mut header = [0; 8];
            reader.read_exact(&mut header)?;
            Ok(Some(Control::ClipboardAck {
                sequence: u64::from_be_bytes(header),
            }))
        }
        _ => Err(invalid("unexpected device message")),
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::{MAX_MEDIA_BYTES, VideoPacket, encode, read_video};
    use nyaterm_android::ipc::Control;
    #[test]
    fn key_and_clipboard_match_scrcpy_501_wire_layout() {
        assert_eq!(
            encode(&Control::Key {
                action: 0,
                keycode: 3,
                repeat: 0,
                metastate: 0
            })
            .unwrap(),
            [0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode(&Control::Paste {
                text: "中".into(),
                sequence: 1
            })
            .unwrap(),
            [9, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 3, 0xe4, 0xb8, 0xad]
        );
        assert!(encode(&Control::Text { text: "中".into() }).is_err());
    }
    #[test]
    fn session_header_is_not_an_encoded_frame_and_lengths_are_bounded() {
        let mut bytes = vec![0x80, 0, 0, 0];
        bytes.extend_from_slice(&1080u32.to_be_bytes());
        bytes.extend_from_slice(&2400u32.to_be_bytes());
        assert!(matches!(
            read_video(&mut bytes.as_slice()).unwrap(),
            VideoPacket::Session {
                width: 1080,
                height: 2400
            }
        ));
        let mut bytes = vec![0; 8];
        bytes.extend_from_slice(&(MAX_MEDIA_BYTES as u32 + 1).to_be_bytes());
        assert!(read_video(&mut bytes.as_slice()).is_err());
    }
    #[test]
    fn touch_and_scroll_follow_official_control_message_reader_fixtures() {
        let touch = encode(&Control::Touch {
            action: 0,
            x: 100,
            y: 200,
            width: 1080,
            height: 2400,
            generation: 1,
        })
        .unwrap();
        let mut expected = vec![2, 0];
        expected.extend(u64::MAX.to_be_bytes());
        expected.extend(100u32.to_be_bytes());
        expected.extend(200u32.to_be_bytes());
        expected.extend(1080u16.to_be_bytes());
        expected.extend(2400u16.to_be_bytes());
        expected.extend(u16::MAX.to_be_bytes());
        expected.extend(1u32.to_be_bytes());
        expected.extend(1u32.to_be_bytes());
        assert_eq!(touch, expected);
        let scroll = encode(&Control::Scroll {
            x: 100,
            y: 200,
            width: 1080,
            height: 2400,
            horizontal: 1.0,
            vertical: -1.0,
            generation: 1,
        })
        .unwrap();
        assert_eq!(&scroll[13..17], &[0x08, 0x00, 0xf8, 0x00]);
        assert_eq!(scroll.len(), 21);
    }
}

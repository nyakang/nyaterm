//! The application parses only this bounded, versioned helper contract.
use crate::adb::client::DeviceBinding;
use nyaterm_core::models::android::AndroidScreenSettings;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 3;
pub const MAX_CAPTURE_EDGE: u32 = 2560;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CONTROL_BYTES: usize = 128 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Hello {
        protocol: u32,
        version: String,
    },
    Connect {
        binding: DeviceBinding,
        endpoint: std::net::SocketAddr,
        settings: AndroidScreenSettings,
    },
    Ready {
        width: u32,
        height: u32,
        generation: u64,
    },
    Key {
        action: u8,
        keycode: u32,
        repeat: u32,
        metastate: u32,
    },
    Touch {
        action: u8,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        generation: u64,
    },
    Scroll {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        horizontal: f32,
        vertical: f32,
        generation: u64,
    },
    Text {
        text: String,
    },
    Paste {
        text: String,
        sequence: u64,
    },
    ClipboardAck {
        sequence: u64,
    },
    RefreshPresentation,
    Presentation {
        visible: bool,
    },
    InputRejected {
        operation: InputOperation,
        code: InputRejection,
    },
    Rotate,
    ReleaseInputs,
    FrameAck {
        sequence: u64,
    },
    Ping {
        sequence: u64,
    },
    Pong {
        sequence: u64,
    },
    Disconnect,
    Disconnected,
    Fatal {
        code: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputOperation {
    Key,
    Touch,
    Scroll,
    Text,
    Paste,
    Rotate,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum InputRejection {
    #[error("Android text exceeds input limit")]
    TextTooLong,
    #[error("Android text contains NUL")]
    Nul,
    #[error("Android input exceeds IPC limit")]
    PacketTooLong,
    #[error("Android input is invalid")]
    InvalidInput,
}
impl InputRejection {
    pub fn code(self) -> &'static str {
        match self {
            Self::TextTooLong => "TextTooLong",
            Self::Nul => "InputNul",
            Self::PacketTooLong => "InputPacketTooLong",
            Self::InvalidInput => "InvalidInput",
        }
    }
}
impl Control {
    pub fn input_operation(&self) -> Option<InputOperation> {
        match self {
            Self::Key { .. } => Some(InputOperation::Key),
            Self::Touch { .. } => Some(InputOperation::Touch),
            Self::Scroll { .. } => Some(InputOperation::Scroll),
            Self::Text { .. } => Some(InputOperation::Text),
            Self::Paste { .. } => Some(InputOperation::Paste),
            Self::Rotate => Some(InputOperation::Rotate),
            _ => None,
        }
    }
    pub fn validate_input(&self) -> Result<(), InputRejection> {
        match self {
            Self::Text { text } | Self::Paste { text, .. } => {
                if text.len() > MAX_TEXT_BYTES {
                    return Err(InputRejection::TextTooLong);
                }
                if text.contains('\0') {
                    return Err(InputRejection::Nul);
                }
            }
            Self::Key { action, .. } if *action > 1 => return Err(InputRejection::InvalidInput),
            Self::Touch { action, .. } if *action > 3 => return Err(InputRejection::InvalidInput),
            Self::Scroll {
                horizontal,
                vertical,
                ..
            } if !horizontal.is_finite() || !vertical.is_finite() => {
                return Err(InputRejection::InvalidInput);
            }
            _ => {}
        }
        if let Self::Touch {
            x,
            y,
            width,
            height,
            ..
        }
        | Self::Scroll {
            x,
            y,
            width,
            height,
            ..
        } = self
            && (*width == 0
                || *height == 0
                || *width > 4096
                || *height > 4096
                || x >= width
                || y >= height)
        {
            return Err(InputRejection::InvalidInput);
        }
        if self.input_operation().is_some()
            && serde_json::to_vec(self)
                .map_err(|_| InputRejection::InvalidInput)?
                .len()
                > MAX_CONTROL_BYTES
        {
            return Err(InputRejection::PacketTooLong);
        }
        Ok(())
    }
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Text and clipboard values, including opaque future fields, are never diagnostic data.
        f.write_str("AndroidControl { .. }")
    }
}

#[derive(Clone)]
pub struct Frame {
    pub sequence: u64,
    pub binding_generation: u64,
    pub generation: u64,
    pub pts: u64,
    pub capture_width: u32,
    pub capture_height: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn validate(&self) -> io::Result<()> {
        let length = (self.stride as usize).checked_mul(self.height as usize);
        if self.sequence == 0
            || self.binding_generation == 0
            || self.generation == 0
            || self.width == 0
            || self.height == 0
            || self.width > self.capture_width
            || self.height > self.capture_height
            || !valid_capture(self.capture_width, self.capture_height)
            || self.stride != self.width.checked_mul(4).unwrap_or(0)
            || length != Some(self.pixels.len())
            || self.pixels.len() > MAX_FRAME_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Android frame",
            ));
        }
        Ok(())
    }
}

pub enum Packet {
    Control(Control),
    Frame(Frame),
}

pub fn write_packet(writer: &mut impl Write, packet: &Packet) -> io::Result<()> {
    match packet {
        Packet::Control(control) => {
            let data = serde_json::to_vec(control).map_err(io::Error::other)?;
            if data.len() > MAX_CONTROL_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "control length",
                ));
            }
            writer.write_all(&[1])?;
            writer.write_all(&(data.len() as u32).to_le_bytes())?;
            writer.write_all(&data)?;
        }
        Packet::Frame(frame) => {
            frame.validate()?;
            writer.write_all(&[2])?;
            writer.write_all(&(52 + frame.pixels.len() as u32).to_le_bytes())?;
            for value in [
                frame.sequence,
                frame.binding_generation,
                frame.generation,
                frame.pts,
            ] {
                writer.write_all(&value.to_le_bytes())?;
            }
            for value in [
                frame.width,
                frame.height,
                frame.stride,
                frame.capture_width,
                frame.capture_height,
            ] {
                writer.write_all(&value.to_le_bytes())?;
            }
            writer.write_all(&frame.pixels)?;
        }
    }
    writer.flush()
}

pub fn read_packet(reader: &mut impl Read) -> io::Result<Option<Packet>> {
    read_packet_reusing(reader, &mut Vec::new())
}

/// The returned frame owns the recycled allocation. Control packets leave it available.
pub fn read_packet_reusing(
    reader: &mut impl Read,
    pixels: &mut Vec<u8>,
) -> io::Result<Option<Packet>> {
    let mut kind = [0; 1];
    if reader.read(&mut kind)? == 0 {
        return Ok(None);
    }
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if kind[0] == 1 {
        if length > MAX_CONTROL_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "control length"));
        }
        let mut data = vec![0; length];
        reader.read_exact(&mut data)?;
        return serde_json::from_slice(&data)
            .map(|control| Some(Packet::Control(control)))
            .map_err(io::Error::other);
    }
    if kind[0] != 2 || !(52..=MAX_FRAME_BYTES + 52).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Android IPC packet length",
        ));
    }
    let mut header = [0; 52];
    reader.read_exact(&mut header)?;
    let get64 = |offset| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
    let get32 = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    // Validate geometry before allocating pixels.
    let width = get32(32);
    let height = get32(36);
    let stride = get32(40);
    let capture_width = get32(44);
    let capture_height = get32(48);
    if get64(0) == 0
        || get64(8) == 0
        || get64(16) == 0
        || width == 0
        || height == 0
        || width > capture_width
        || height > capture_height
        || !valid_capture(capture_width, capture_height)
        || width.checked_mul(4) != Some(stride)
        || (stride as usize).checked_mul(height as usize) != Some(length - 52)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Android IPC frame geometry",
        ));
    }
    pixels.resize(length - 52, 0);
    reader.read_exact(pixels)?;
    Ok(Some(Packet::Frame(Frame {
        sequence: get64(0),
        binding_generation: get64(8),
        generation: get64(16),
        pts: get64(24),
        capture_width,
        capture_height,
        width,
        height,
        stride,
        pixels: std::mem::take(pixels),
    })))
}

pub fn valid_capture(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width <= MAX_CAPTURE_EDGE && height <= MAX_CAPTURE_EDGE
}

/// Keep the capture aspect while bounding the pipe/GPU allocation to 16 MiB.
pub fn presentation_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if !valid_capture(width, height) {
        return None;
    }
    let pixels = u64::from(width) * u64::from(height);
    let limit = (MAX_FRAME_BYTES / 4) as u64;
    if pixels <= limit {
        return Some((width, height));
    }
    let scale = (limit as f64 / pixels as f64).sqrt();
    Some((
        (width as f64 * scale).floor() as u32,
        (height as f64 * scale).floor() as u32,
    ))
}

#[cfg(test)]
mod tests {
    use crate::ipc::{Control, Frame, MAX_FRAME_BYTES, Packet, read_packet, write_packet};
    #[test]
    fn frames_round_trip_and_malformed_geometry_is_rejected_before_allocation() {
        let frame = Frame {
            sequence: 1,
            binding_generation: 2,
            generation: 3,
            pts: 4,
            capture_width: 2,
            capture_height: 1,
            width: 2,
            height: 1,
            stride: 8,
            pixels: vec![7; 8],
        };
        let mut data = Vec::new();
        write_packet(&mut data, &Packet::Frame(frame)).unwrap();
        let Some(Packet::Frame(decoded)) = read_packet(&mut data.as_slice()).unwrap() else {
            panic!("frame missing")
        };
        assert_eq!(decoded.pixels, vec![7; 8]);
        data[37..41].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_packet(&mut data.as_slice()).is_err());
        let mut header = vec![2];
        header.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 53).to_le_bytes());
        assert!(read_packet(&mut header.as_slice()).is_err());
    }
    #[test]
    fn maximum_capture_is_scaled_to_bounded_bgra_without_losing_input_geometry() {
        use crate::ipc::{MAX_FRAME_BYTES, presentation_size, valid_capture};
        for (w, h) in [(2560, 1920), (2560, 2560), (1920, 2560), (1280, 720)] {
            let (pw, ph) = presentation_size(w, h).unwrap();
            assert!(u64::from(pw) * u64::from(ph) * 4 <= MAX_FRAME_BYTES as u64);
            assert!((pw as f64 / ph as f64 - w as f64 / h as f64).abs() < 0.002);
            let frame = Frame {
                sequence: 1,
                binding_generation: 2,
                generation: 3,
                pts: 0,
                capture_width: w,
                capture_height: h,
                width: pw,
                height: ph,
                stride: pw * 4,
                pixels: vec![0; pw as usize * ph as usize * 4],
            };
            frame.validate().unwrap();
        }
        for (w, h) in [(0, 2560), (2561, 10), (10, u32::MAX)] {
            assert!(!valid_capture(w, h));
            assert!(presentation_size(w, h).is_none());
        }
    }
    #[test]
    fn recycled_reader_preserves_allocation_through_controls_and_rejects_invalid_capture() {
        use crate::ipc::read_packet_reusing;
        let frame = Frame {
            sequence: 1,
            binding_generation: 1,
            generation: 1,
            pts: 0,
            capture_width: 2,
            capture_height: 2,
            width: 1,
            height: 1,
            stride: 4,
            pixels: vec![9; 4],
        };
        let mut data = Vec::new();
        write_packet(&mut data, &Packet::Control(Control::RefreshPresentation)).unwrap();
        write_packet(&mut data, &Packet::Frame(frame)).unwrap();
        let mut reader = data.as_slice();
        let mut pixels = Vec::with_capacity(32);
        let pointer = pixels.as_ptr();
        assert!(matches!(
            read_packet_reusing(&mut reader, &mut pixels).unwrap(),
            Some(Packet::Control(Control::RefreshPresentation))
        ));
        assert_eq!(pixels.as_ptr(), pointer);
        let Some(Packet::Frame(frame)) = read_packet_reusing(&mut reader, &mut pixels).unwrap()
        else {
            panic!("frame missing");
        };
        assert_eq!(frame.pixels.as_ptr(), pointer);
        assert_eq!((frame.capture_width, frame.width), (2, 1));
        let mut bytes = Vec::new();
        write_packet(&mut bytes, &Packet::Frame(frame)).unwrap();
        for offset in [5, 13, 21] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 8].fill(0);
            assert!(read_packet(&mut invalid.as_slice()).is_err());
        }
        bytes[49..53].copy_from_slice(&0u32.to_le_bytes());
        assert!(read_packet(&mut bytes.as_slice()).is_err());
    }
    #[test]
    fn clipboard_debug_is_redacted() {
        let control = Control::Paste {
            text: "private clipboard".into(),
            sequence: 1,
        };
        assert!(!format!("{control:?}").contains("private"));
    }
    #[test]
    fn input_limits_count_utf8_bytes_and_serialized_escaping() {
        use crate::ipc::{InputRejection, MAX_TEXT_BYTES};
        assert!(
            Control::Text {
                text: "a".repeat(MAX_TEXT_BYTES)
            }
            .validate_input()
            .is_ok()
        );
        assert!(matches!(
            Control::Text {
                text: "中".repeat(MAX_TEXT_BYTES / 3 + 1)
            }
            .validate_input(),
            Err(InputRejection::TextTooLong)
        ));
        assert!(matches!(
            Control::Paste {
                text: "private\0value".into(),
                sequence: 1
            }
            .validate_input(),
            Err(InputRejection::Nul)
        ));
        assert!(matches!(
            Control::Text {
                text: "\u{1}".repeat(MAX_TEXT_BYTES)
            }
            .validate_input(),
            Err(InputRejection::PacketTooLong)
        ));
        assert!(
            Control::Scroll {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
                horizontal: f32::NAN,
                vertical: 0.,
                generation: 1
            }
            .validate_input()
            .is_err()
        );
    }
}

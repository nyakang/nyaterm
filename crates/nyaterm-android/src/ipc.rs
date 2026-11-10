//! The application parses only this bounded, versioned helper contract.
use crate::adb::client::DeviceBinding;
use nyaterm_core::models::android::AndroidScreenSettings;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
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
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn validate(&self) -> io::Result<()> {
        let length = (self.stride as usize).checked_mul(self.height as usize);
        if self.width == 0
            || self.height == 0
            || self.width > 4096
            || self.height > 4096
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
            writer.write_all(&(44 + frame.pixels.len() as u32).to_le_bytes())?;
            for value in [
                frame.sequence,
                frame.binding_generation,
                frame.generation,
                frame.pts,
            ] {
                writer.write_all(&value.to_le_bytes())?;
            }
            for value in [frame.width, frame.height, frame.stride] {
                writer.write_all(&value.to_le_bytes())?;
            }
            writer.write_all(&frame.pixels)?;
        }
    }
    writer.flush()
}

pub fn read_packet(reader: &mut impl Read) -> io::Result<Option<Packet>> {
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
    if kind[0] != 2 || !(44..=MAX_FRAME_BYTES + 44).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Android IPC packet length",
        ));
    }
    let mut header = [0; 44];
    reader.read_exact(&mut header)?;
    let get64 = |offset| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
    let get32 = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    // Validate geometry before allocating pixels.
    let width = get32(32);
    let height = get32(36);
    let stride = get32(40);
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width.checked_mul(4) != Some(stride)
        || (stride as usize).checked_mul(height as usize) != Some(length - 44)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Android IPC frame geometry",
        ));
    }
    let mut pixels = vec![0; length - 44];
    reader.read_exact(&mut pixels)?;
    Ok(Some(Packet::Frame(Frame {
        sequence: get64(0),
        binding_generation: get64(8),
        generation: get64(16),
        pts: get64(24),
        width,
        height,
        stride,
        pixels,
    })))
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
        header.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 45).to_le_bytes());
        assert!(read_packet(&mut header.as_slice()).is_err());
    }
    #[test]
    fn clipboard_debug_is_redacted() {
        let control = Control::Paste {
            text: "private clipboard".into(),
            sequence: 1,
        };
        assert!(!format!("{control:?}").contains("private"));
    }
}

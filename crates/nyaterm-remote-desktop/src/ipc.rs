use std::io::{self, Read, Write};

use crate::{
    CursorPosition, CursorShape, CursorVisibility, PixelFormat, RdpControlMessage, RdpFrameEvent,
    RemoteCursorEvent, RemotePoint, VncControlMessage,
};

pub const HEADER_LEN: usize = 5;
pub const CONTROL_PAYLOAD_LIMIT: usize = 1024 * 1024;
pub const FRAME_PAYLOAD_LIMIT: usize = 160 * 1024 * 1024;
pub const CURSOR_PAYLOAD_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Control = 1,
    Frame = 2,
    Cursor = 3,
}

impl TryFrom<u8> for PacketType {
    type Error = io::Error;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Control),
            2 => Ok(Self::Frame),
            3 => Ok(Self::Cursor),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown helper IPC packet type",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub packet_type: PacketType,
    pub payload: crate::pixel_memory::PixelBuffer,
    body: crate::pixel_memory::PixelBuffer,
}

impl Packet {
    fn payload_len(&self) -> io::Result<usize> {
        self.payload
            .len()
            .checked_add(self.body.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "packet length overflow"))
    }
}

fn payload_limit(packet_type: PacketType) -> usize {
    match packet_type {
        PacketType::Control => CONTROL_PAYLOAD_LIMIT,
        PacketType::Frame => FRAME_PAYLOAD_LIMIT,
        PacketType::Cursor => CURSOR_PAYLOAD_LIMIT,
    }
}

pub fn read_packet(reader: &mut impl Read) -> io::Result<Option<Packet>> {
    read_packet_inner(reader, None)
}

pub(crate) fn read_packet_budgeted(reader: &mut impl Read) -> io::Result<Option<Packet>> {
    read_packet_inner(reader, Some(crate::pixel_memory::PixelBudget::shared()))
}

pub(crate) fn read_packet_inner(
    reader: &mut impl Read,
    budget: Option<&std::sync::Arc<crate::pixel_memory::PixelBudget>>,
) -> io::Result<Option<Packet>> {
    let mut header = [0u8; HEADER_LEN];
    let mut read = 0;
    while read < header.len() {
        match reader.read(&mut header[read..])? {
            0 if read == 0 => return Ok(None),
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated helper IPC header",
                ));
            }
            count => read += count,
        }
    }
    let packet_type = PacketType::try_from(header[0])?;
    let length = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
    if length > payload_limit(packet_type) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "helper IPC payload exceeds limit",
        ));
    }
    let lease = if packet_type != PacketType::Control {
        budget
            .map(|budget| budget.reserve(length).map(std::sync::Arc::new))
            .transpose()?
    } else {
        None
    };
    let mut payload = Vec::new();
    payload.try_reserve_exact(length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "failed to allocate helper IPC payload",
        )
    })?;
    payload.resize(length, 0);
    reader.read_exact(&mut payload)?;
    Ok(Some(Packet {
        packet_type,
        payload: crate::pixel_memory::PixelBuffer::with_lease(payload, lease),
        body: Default::default(),
    }))
}

/// Frame a packet without flushing.
///
/// A single VNC framebuffer update can carry up to `max_rectangles_per_update`
/// rectangles, so flushing per packet would turn one update into a thousand
/// write syscalls. Helpers batch into a `BufWriter` and flush once the outbound
/// queue drains; use [`write_packet`] when the packet must leave immediately.
pub fn write_packet_into(writer: &mut impl Write, packet: &Packet) -> io::Result<()> {
    let payload_len = packet.payload_len()?;
    if payload_len > payload_limit(packet.packet_type) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper IPC payload exceeds limit",
        ));
    }
    let length = u32::try_from(payload_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper IPC payload is too large",
        )
    })?;
    writer.write_all(&[packet.packet_type as u8])?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&packet.payload)?;
    writer.write_all(&packet.body)
}

pub fn write_packet(writer: &mut impl Write, packet: &Packet) -> io::Result<()> {
    write_packet_into(writer, packet)?;
    writer.flush()
}

#[derive(Debug, Default)]
pub struct PacketReader {
    buffer: Vec<u8>,
}

impl PacketReader {
    pub fn push(&mut self, bytes: &[u8]) -> io::Result<Vec<Packet>> {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut offset = 0;
        while self.buffer.len().saturating_sub(offset) >= HEADER_LEN {
            let packet_type = PacketType::try_from(self.buffer[offset])?;
            let length = u32::from_le_bytes(self.buffer[offset + 1..offset + 5].try_into().unwrap())
                as usize;
            if length > payload_limit(packet_type) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "helper IPC payload exceeds limit",
                ));
            }
            let packet_len = HEADER_LEN.checked_add(length).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "helper IPC length overflow")
            })?;
            if self.buffer.len() - offset < packet_len {
                break;
            }
            packets.push(Packet {
                packet_type,
                payload: self.buffer[offset + HEADER_LEN..offset + packet_len]
                    .to_vec()
                    .into(),
                body: Default::default(),
            });
            offset += packet_len;
        }
        if offset != 0 {
            self.buffer.drain(..offset);
        }
        Ok(packets)
    }
}

pub fn encode_control(message: &RdpControlMessage) -> io::Result<Packet> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    if payload.len() > CONTROL_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RDP control payload exceeds limit",
        ));
    }
    Ok(Packet {
        packet_type: PacketType::Control,
        payload: payload.into(),
        body: Default::default(),
    })
}

pub fn decode_control(packet: &Packet) -> io::Result<RdpControlMessage> {
    if packet.packet_type != PacketType::Control {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "packet is not an RDP control packet",
        ));
    }
    serde_json::from_slice(&packet.payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Encode a VNC control message.
///
/// VNC reuses [`PacketType::Control`] rather than claiming its own tag: a helper
/// only ever speaks one protocol, so the payload is never ambiguous. Keeping the
/// two codecs separate instead of making them generic leaves every existing
/// `decode_control` call site untouched.
pub fn encode_vnc_control(message: &VncControlMessage) -> io::Result<Packet> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    if payload.len() > CONTROL_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VNC control payload exceeds limit",
        ));
    }
    Ok(Packet {
        packet_type: PacketType::Control,
        payload: payload.into(),
        body: Default::default(),
    })
}

pub fn decode_vnc_control(packet: &Packet) -> io::Result<VncControlMessage> {
    if packet.packet_type != PacketType::Control {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "packet is not a control packet",
        ));
    }
    serde_json::from_slice(&packet.payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn put_session(payload: &mut Vec<u8>, session_id: &str) -> io::Result<()> {
    let length = u16::try_from(session_id.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper IPC session id is too long",
        )
    })?;
    payload.extend_from_slice(&length.to_le_bytes());
    payload.extend_from_slice(session_id.as_bytes());
    Ok(())
}

fn take<const N: usize>(payload: &[u8], offset: &mut usize) -> io::Result<[u8; N]> {
    let end = offset.checked_add(N).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "helper IPC packet offset overflow",
        )
    })?;
    let bytes = payload.get(*offset..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::UnexpectedEof, "truncated helper IPC packet")
    })?;
    *offset = end;
    Ok(bytes.try_into().unwrap())
}

fn take_session(payload: &[u8], offset: &mut usize) -> io::Result<String> {
    let length = u16::from_le_bytes(take::<2>(payload, offset)?) as usize;
    let end = offset.checked_add(length).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "helper IPC session id length overflow",
        )
    })?;
    let bytes = payload.get(*offset..end).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated helper IPC session id",
        )
    })?;
    *offset = end;
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "helper IPC session id is not UTF-8",
        )
    })
}

fn put_u32(payload: &mut Vec<u8>, value: u32) {
    payload.extend_from_slice(&value.to_le_bytes());
}
fn take_u32(payload: &[u8], offset: &mut usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(take::<4>(payload, offset)?))
}

pub fn encode_frame_packet(session_id: &str, event: &RdpFrameEvent) -> io::Result<Packet> {
    let RdpFrameEvent::Bitmap {
        epoch,
        full,
        x,
        y,
        width,
        height,
        stride,
        format,
        pixels,
    } = event
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "event is not a bitmap",
        ));
    };
    let mut payload = Vec::with_capacity(40 + session_id.len() + pixels.len());
    put_session(&mut payload, session_id)?;
    payload.extend_from_slice(&epoch.to_le_bytes());
    payload.push(u8::from(*full));
    payload.push(match format {
        PixelFormat::Bgra8 => 1,
        PixelFormat::Rgba8 => 2,
    });
    for value in [*x, *y, *width, *height, *stride] {
        put_u32(&mut payload, value);
    }
    put_u32(
        &mut payload,
        u32::try_from(pixels.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame is too large"))?,
    );
    payload.extend_from_slice(pixels);
    if payload.len() > FRAME_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame exceeds limit",
        ));
    }
    Ok(Packet {
        packet_type: PacketType::Frame,
        payload: payload.into(),
        body: Default::default(),
    })
}

/// Encode a frame without cloning its pixel buffer.
///
/// The packet writer emits `payload` and `body` contiguously, preserving the
/// existing wire format while allowing helper decoders to transfer ownership of
/// server-controlled image buffers directly to the stdout writer.
pub fn encode_frame_packet_owned(session_id: &str, event: RdpFrameEvent) -> io::Result<Packet> {
    let RdpFrameEvent::Bitmap {
        epoch,
        full,
        x,
        y,
        width,
        height,
        stride,
        format,
        pixels,
    } = event
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "event is not a bitmap",
        ));
    };
    let mut payload = Vec::with_capacity(40 + session_id.len());
    put_session(&mut payload, session_id)?;
    payload.extend_from_slice(&epoch.to_le_bytes());
    payload.push(u8::from(full));
    payload.push(match format {
        PixelFormat::Bgra8 => 1,
        PixelFormat::Rgba8 => 2,
    });
    for value in [x, y, width, height, stride] {
        put_u32(&mut payload, value);
    }
    put_u32(
        &mut payload,
        u32::try_from(pixels.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame is too large"))?,
    );
    if payload.len().saturating_add(pixels.len()) > FRAME_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame exceeds limit",
        ));
    }
    Ok(Packet {
        packet_type: PacketType::Frame,
        payload: payload.into(),
        body: pixels,
    })
}

#[derive(Clone, Copy)]
struct FrameHeader {
    epoch: u64,
    full: bool,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
}

fn decode_frame_header(packet: &Packet) -> io::Result<(String, FrameHeader, usize)> {
    if packet.packet_type != PacketType::Frame {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "packet is not a frame",
        ));
    }
    let mut offset = 0;
    let session_id = take_session(&packet.payload, &mut offset)?;
    let epoch = u64::from_le_bytes(take::<8>(&packet.payload, &mut offset)?);
    let full = take::<1>(&packet.payload, &mut offset)?[0] != 0;
    let format = match take::<1>(&packet.payload, &mut offset)?[0] {
        1 => PixelFormat::Bgra8,
        2 => PixelFormat::Rgba8,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown pixel format",
            ));
        }
    };
    let x = take_u32(&packet.payload, &mut offset)?;
    let y = take_u32(&packet.payload, &mut offset)?;
    let width = take_u32(&packet.payload, &mut offset)?;
    let height = take_u32(&packet.payload, &mut offset)?;
    let stride = take_u32(&packet.payload, &mut offset)?;
    let data_len = take_u32(&packet.payload, &mut offset)? as usize;
    let required = usize::try_from(height.saturating_sub(1))
        .ok()
        .and_then(|rows| rows.checked_mul(stride as usize))
        .and_then(|base| base.checked_add(width as usize * 4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame dimensions overflow"))?;
    let inline_data_len = packet.payload.len().saturating_sub(offset);
    let actual_data_len = if packet.body.is_empty() {
        inline_data_len
    } else if inline_data_len == 0 {
        packet.body.len()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame pixels are split across inline and owned payloads",
        ));
    };
    if width == 0
        || height == 0
        || stride < width.saturating_mul(4)
        || data_len != required
        || actual_data_len != data_len
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame dimensions or payload length",
        ));
    }
    Ok((
        session_id,
        FrameHeader {
            epoch,
            full,
            x,
            y,
            width,
            height,
            stride,
            format,
        },
        offset,
    ))
}

pub fn decode_frame_packet(packet: &Packet) -> io::Result<(String, RdpFrameEvent)> {
    // Immutable pixel ownership is shared, including the original budget lease.
    decode_frame_packet_owned(packet.clone())
}

pub fn decode_frame_packet_owned(packet: Packet) -> io::Result<(String, RdpFrameEvent)> {
    let (session_id, header, offset) = decode_frame_header(&packet)?;
    let pixels = if packet.body.is_empty() {
        packet.payload.skip(offset)
    } else {
        packet.body
    };
    Ok((
        session_id,
        RdpFrameEvent::Bitmap {
            epoch: header.epoch,
            full: header.full,
            x: header.x,
            y: header.y,
            width: header.width,
            height: header.height,
            stride: header.stride,
            format: header.format,
            pixels,
        },
    ))
}

pub fn encode_cursor_packet(session_id: &str, event: &RemoteCursorEvent) -> io::Result<Packet> {
    let pixel_bytes = match event {
        RemoteCursorEvent::Shape(shape) => shape.pixels.len(),
        RemoteCursorEvent::Position(_) | RemoteCursorEvent::Visibility(_) => 0,
    };
    let mut payload = Vec::with_capacity(36 + session_id.len() + pixel_bytes);
    put_session(&mut payload, session_id)?;
    match event {
        RemoteCursorEvent::Shape(shape) => {
            payload.push(1);
            payload.extend_from_slice(&shape.shape_id.to_le_bytes());
            for value in [shape.width, shape.height, shape.hotspot.x, shape.hotspot.y] {
                put_u32(&mut payload, value);
            }
            put_u32(
                &mut payload,
                u32::try_from(shape.pixels.len()).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "cursor is too large")
                })?,
            );
            payload.extend_from_slice(&shape.pixels);
        }
        RemoteCursorEvent::Position(position) => {
            payload.push(2);
            put_u32(&mut payload, position.x);
            put_u32(&mut payload, position.y);
        }
        RemoteCursorEvent::Visibility(visibility) => {
            payload.push(3);
            payload.push(u8::from(visibility.visible));
        }
    }
    if payload.len() > CURSOR_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor exceeds limit",
        ));
    }
    Ok(Packet {
        packet_type: PacketType::Cursor,
        payload: payload.into(),
        body: Default::default(),
    })
}

fn decode_cursor_payload(packet: &Packet) -> io::Result<(String, RemoteCursorEvent, usize)> {
    if packet.packet_type != PacketType::Cursor {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "packet is not a cursor",
        ));
    }
    let mut offset = 0;
    let session_id = take_session(&packet.payload, &mut offset)?;
    let kind = take::<1>(&packet.payload, &mut offset)?[0];
    let event = match kind {
        1 => {
            let shape_id = u64::from_le_bytes(take::<8>(&packet.payload, &mut offset)?);
            let width = take_u32(&packet.payload, &mut offset)?;
            let height = take_u32(&packet.payload, &mut offset)?;
            let hotspot = RemotePoint {
                x: take_u32(&packet.payload, &mut offset)?,
                y: take_u32(&packet.payload, &mut offset)?,
            };
            let data_len = take_u32(&packet.payload, &mut offset)? as usize;
            let expected = usize::try_from(width)
                .ok()
                .and_then(|width| {
                    usize::try_from(height)
                        .ok()
                        .and_then(|height| width.checked_mul(height))
                })
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "cursor dimensions overflow")
                })?;
            if data_len != expected || packet.payload.len().saturating_sub(offset) != data_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid cursor payload length",
                ));
            }
            RemoteCursorEvent::Shape(CursorShape {
                shape_id,
                width,
                height,
                hotspot,
                pixels: Default::default(),
            })
        }
        2 => RemoteCursorEvent::Position(CursorPosition {
            x: take_u32(&packet.payload, &mut offset)?,
            y: take_u32(&packet.payload, &mut offset)?,
        }),
        3 => RemoteCursorEvent::Visibility(CursorVisibility {
            visible: take::<1>(&packet.payload, &mut offset)?[0] != 0,
        }),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown cursor event kind",
            ));
        }
    };
    if !matches!(event, RemoteCursorEvent::Shape(_)) && offset != packet.payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing cursor event payload",
        ));
    }
    Ok((session_id, event, offset))
}

pub fn decode_cursor_packet(packet: &Packet) -> io::Result<(String, RemoteCursorEvent)> {
    let (session_id, mut event, offset) = decode_cursor_payload(packet)?;
    if let RemoteCursorEvent::Shape(shape) = &mut event {
        shape.pixels = packet.payload.clone().skip(offset);
    }
    Ok((session_id, event))
}

pub fn decode_cursor_packet_owned(packet: Packet) -> io::Result<(String, RemoteCursorEvent)> {
    if !packet.body.is_empty() {
        return decode_cursor_packet(&packet);
    }
    let (session_id, mut event, offset) = decode_cursor_payload(&packet)?;
    if let RemoteCursorEvent::Shape(shape) = &mut event {
        shape.pixels = packet.payload.skip(offset);
    }
    Ok((session_id, event))
}

#[cfg(test)]
mod tests {
    use super::{
        PacketReader, decode_control, decode_cursor_packet_owned, decode_frame_packet_owned,
        decode_vnc_control, encode_control, encode_cursor_packet, encode_frame_packet,
        encode_vnc_control, read_packet, write_packet, write_packet_into,
    };
    use crate::{
        CursorPosition, CursorShape, CursorVisibility, PROTOCOL_VERSION, PixelFormat,
        RdpControlMessage, RdpFrameEvent, RdpServerCapabilities, RemoteCursorEvent, RemotePoint,
        VncControlMessage, VncInputEvent, VncServerCapabilities, VncSessionState,
    };
    use std::io::{Cursor, Write};

    /// Records how many times the writer was flushed.
    #[derive(Default)]
    struct CountingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn round_trips_partial_control_and_binary_frame_packets() {
        let control = encode_control(&RdpControlMessage::ClientHello {
            version: PROTOCOL_VERSION,
        })
        .unwrap();
        let mut encoded = Vec::new();
        write_packet(&mut encoded, &control).unwrap();
        let mut incremental = PacketReader::default();
        assert!(incremental.push(&encoded[..2]).unwrap().is_empty());
        let packets = incremental.push(&encoded[2..]).unwrap();
        assert!(matches!(
            decode_control(&packets[0]).unwrap(),
            RdpControlMessage::ClientHello {
                version: PROTOCOL_VERSION
            }
        ));

        let frame = RdpFrameEvent::Bitmap {
            epoch: 7,
            full: true,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Bgra8,
            pixels: vec![9; 16].into(),
        };
        let packet = encode_frame_packet("session", &frame).unwrap();
        let mut encoded = Vec::new();
        write_packet(&mut encoded, &packet).unwrap();
        let decoded = read_packet(&mut Cursor::new(encoded)).unwrap().unwrap();
        assert_eq!(
            decode_frame_packet_owned(decoded).unwrap(),
            ("session".to_string(), frame)
        );
    }

    #[test]
    fn round_trips_rdp_static_capabilities_and_secure_attention() {
        let hello = encode_control(&RdpControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: RdpServerCapabilities {
                committed_unicode_text: true,
                secure_attention: true,
            },
        })
        .unwrap();
        assert!(matches!(
            decode_control(&hello).unwrap(),
            RdpControlMessage::ServerHello {
                capabilities: RdpServerCapabilities {
                    committed_unicode_text: true,
                    secure_attention: true,
                },
                ..
            }
        ));

        let sas = encode_control(&RdpControlMessage::SecureAttention {
            session_id: "rdp".to_string(),
        })
        .unwrap();
        assert!(matches!(
            decode_control(&sas).unwrap(),
            RdpControlMessage::SecureAttention { ref session_id } if session_id == "rdp"
        ));
        assert!(decode_vnc_control(&sas).is_err());
    }

    #[test]
    fn cursor_shape_position_and_visibility_are_independent_packets() {
        let events = [
            RemoteCursorEvent::Shape(CursorShape {
                shape_id: 9,
                width: 2,
                height: 2,
                hotspot: RemotePoint { x: 1, y: 0 },
                pixels: vec![7; 16].into(),
            }),
            RemoteCursorEvent::Position(CursorPosition { x: 640, y: 360 }),
            RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
        ];
        for event in events {
            let packet = encode_cursor_packet("rdp", &event).expect("cursor packet");
            assert_eq!(
                decode_cursor_packet_owned(packet).expect("cursor round trip"),
                ("rdp".to_string(), event)
            );
        }
    }

    #[test]
    fn rejects_unknown_packet_type_and_oversize_payload() {
        assert!(read_packet(&mut Cursor::new(vec![99, 0, 0, 0, 0])).is_err());
        let length = (super::CONTROL_PAYLOAD_LIMIT as u32 + 1).to_le_bytes();
        let mut bytes = vec![1];
        bytes.extend_from_slice(&length);
        assert!(read_packet(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn round_trips_vnc_control_across_a_split_read() {
        let messages = [
            VncControlMessage::ClientHello {
                version: PROTOCOL_VERSION,
            },
            VncControlMessage::ServerHello {
                version: PROTOCOL_VERSION,
                capabilities: VncServerCapabilities {
                    committed_unicode_keysyms: true,
                },
            },
            VncControlMessage::DesktopReset {
                session_id: "vnc".to_string(),
                epoch: 4,
                width: 1024,
                height: 768,
            },
            VncControlMessage::State {
                session_id: "vnc".to_string(),
                state: VncSessionState::Reconnecting,
                message: Some("retrying".to_string()),
            },
            VncControlMessage::Input {
                session_id: "vnc".to_string(),
                events: vec![
                    VncInputEvent::Key {
                        keysym: 0xFF0D,
                        pressed: true,
                    },
                    VncInputEvent::Text {
                        text: "你好".to_string(),
                    },
                ],
            },
            VncControlMessage::RequestFullFrame {
                session_id: "vnc".to_string(),
            },
        ];
        let mut encoded = Vec::new();
        for message in &messages {
            write_packet(&mut encoded, &encode_vnc_control(message).unwrap()).unwrap();
        }

        // Feed one byte at a time: the reader must reassemble every message.
        let mut reader = PacketReader::default();
        let mut decoded = Vec::new();
        for byte in &encoded {
            for packet in reader.push(&[*byte]).unwrap() {
                decoded.push(decode_vnc_control(&packet).unwrap());
            }
        }
        assert_eq!(decoded.len(), messages.len());
        assert!(matches!(
            decoded[1],
            VncControlMessage::ServerHello {
                capabilities: VncServerCapabilities {
                    committed_unicode_keysyms: true
                },
                ..
            }
        ));
        assert!(matches!(
            decoded[2],
            VncControlMessage::DesktopReset {
                epoch: 4,
                width: 1024,
                height: 768,
                ..
            }
        ));
        assert!(matches!(
            &decoded[3],
            VncControlMessage::State {
                state: VncSessionState::Reconnecting,
                ..
            }
        ));
    }

    #[test]
    fn protocol_specific_control_payloads_are_rejected_by_the_other_decoder() {
        // Both vocabularies share PacketType::Control, and structurally identical
        // variants (Disconnect, RequestFullFrame) do decode either way. That is
        // harmless because a helper process only ever speaks one protocol. What
        // must not pass silently is a payload only one side can represent.
        let vnc_only_state = encode_vnc_control(&VncControlMessage::State {
            session_id: "vnc".to_string(),
            state: VncSessionState::Authenticating,
            message: None,
        })
        .unwrap();
        assert!(decode_control(&vnc_only_state).is_err());

        // RdpControlMessage::Clipboard carries a generation counter; VNC's does not.
        let vnc_clipboard = encode_vnc_control(&VncControlMessage::Clipboard {
            session_id: "vnc".to_string(),
            text: "hello".to_string(),
        })
        .unwrap();
        assert!(decode_control(&vnc_clipboard).is_err());

        let rdp_only = encode_control(&RdpControlMessage::Resize {
            session_id: "rdp".to_string(),
            metrics: crate::RdpDisplayMetrics {
                width: 800,
                height: 600,
                desktop_scale_factor: 150,
                physical_size_mm: None,
            },
        })
        .unwrap();
        assert!(decode_vnc_control(&rdp_only).is_err());
    }

    #[test]
    fn only_write_packet_flushes() {
        let packet = encode_vnc_control(&VncControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: VncServerCapabilities {
                committed_unicode_keysyms: true,
            },
        })
        .unwrap();

        let mut buffered = CountingWriter::default();
        write_packet_into(&mut buffered, &packet).unwrap();
        write_packet_into(&mut buffered, &packet).unwrap();
        assert_eq!(buffered.flushes, 0);

        let mut immediate = CountingWriter::default();
        write_packet(&mut immediate, &packet).unwrap();
        assert_eq!(immediate.flushes, 1);
        assert_eq!(immediate.bytes.len() * 2, buffered.bytes.len());
    }
}

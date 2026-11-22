use std::io;

use crate::spice::protocol::SpiceControl;
use crate::{
    CONTROL_PAYLOAD_LIMIT, Packet, PacketType, RemoteCursorEvent, RemoteFrameEvent,
    decode_cursor_packet_owned, decode_frame_packet_owned, encode_cursor_packet,
    encode_frame_packet_owned,
};

pub fn encode_control(message: &SpiceControl) -> io::Result<Packet> {
    message
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let payload = serde_json::to_vec(message)
        .map_err(|_| io::Error::other("cannot encode SPICE control message"))?;
    Packet::control(payload)
}

pub fn decode_control(packet: &Packet) -> io::Result<SpiceControl> {
    if packet.packet_type != PacketType::Control || packet.payload.len() > CONTROL_PAYLOAD_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SPICE control packet",
        ));
    }
    // serde's diagnostic can quote an unknown enum's value. Never return it:
    // a malformed clipboard/credential field must not reach application logs.
    let message: SpiceControl = serde_json::from_slice(&packet.payload).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid SPICE control document")
    })?;
    message
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(message)
}

pub fn encode_frame(channel: u32, frame: RemoteFrameEvent) -> io::Result<Packet> {
    validate_channel(channel)?;
    encode_frame_packet_owned(&channel.to_string(), frame)
}

pub fn decode_frame(packet: Packet) -> io::Result<(u32, RemoteFrameEvent)> {
    let (identity, frame) = decode_frame_packet_owned(packet)?;
    Ok((decode_channel(&identity)?, frame))
}

pub fn encode_cursor(channel: u32, cursor: &RemoteCursorEvent) -> io::Result<Packet> {
    validate_channel(channel)?;
    encode_cursor_packet(&channel.to_string(), cursor)
}

pub fn decode_cursor(packet: Packet) -> io::Result<(u32, RemoteCursorEvent)> {
    let (identity, cursor) = decode_cursor_packet_owned(packet)?;
    Ok((decode_channel(&identity)?, cursor))
}

fn validate_channel(channel: u32) -> io::Result<()> {
    if channel > 255 {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SPICE display channel",
        ))
    } else {
        Ok(())
    }
}

fn decode_channel(identity: &str) -> io::Result<u32> {
    let channel = identity
        .parse::<u32>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid SPICE display channel"))?;
    validate_channel(channel)?;
    Ok(channel)
}

#[cfg(test)]
mod tests {
    use crate::pixel_memory::PixelBuffer;
    use crate::spice::ipc::{decode_control, decode_frame, encode_control, encode_frame};
    use crate::spice::protocol::{
        MAX_SPICE_INPUT_BATCH, SpiceControl, SpiceCredentialLifetime, SpiceInput,
        SpiceSessionConfig,
    };
    use crate::{Packet, PixelFormat, RemoteFrameEvent};
    use nyaterm_core::spice::settings::{SpiceEndpoint, SpiceSettings};

    #[test]
    fn control_round_trip_checks_limits_and_redacts_decode_errors() {
        let message = SpiceControl::Clipboard {
            text: "private clipboard".into(),
        };
        assert_eq!(
            decode_control(&encode_control(&message).unwrap()).unwrap(),
            message
        );
        assert!(!format!("{message:?}").contains("private clipboard"));
        let malformed = Packet::control(br#"{"kind":"private password"}"#.to_vec()).unwrap();
        assert!(
            !decode_control(&malformed)
                .unwrap_err()
                .to_string()
                .contains("private password")
        );
        assert!(
            encode_control(&SpiceControl::Input {
                events: vec![SpiceInput::ReleaseAll; MAX_SPICE_INPUT_BATCH + 1]
            })
            .is_err()
        );
    }

    #[test]
    fn display_frame_round_trip_transfers_pixel_ownership_without_copying() {
        let pixels = PixelBuffer::from(vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let original = pixels.as_ptr();
        let frame = RemoteFrameEvent::Bitmap {
            epoch: 4,
            full: false,
            x: 10,
            y: 20,
            width: 2,
            height: 1,
            stride: 8,
            format: PixelFormat::Bgra8,
            pixels,
        };
        let (channel, frame) = decode_frame(encode_frame(2, frame).unwrap()).unwrap();
        assert_eq!(channel, 2);
        let RemoteFrameEvent::Bitmap {
            pixels,
            epoch,
            x,
            y,
            ..
        } = frame
        else {
            panic!("expected bitmap");
        };
        assert_eq!(pixels.as_ptr(), original);
        assert_eq!((epoch, x, y), (4, 10, 20));
    }

    #[test]
    fn temporary_tickets_cannot_enable_reconnect_through_settings() {
        let mut config = SpiceSessionConfig {
            session_id: "fixture".into(),
            endpoint: SpiceEndpoint {
                extra: Default::default(),
                host: "vm".into(),
                port: None,
                tls_port: Some(5901),
                proxy: None,
            },
            username: "".into(),
            password: "ticket".into(),
            settings: SpiceSettings::default(),
            credential_lifetime: SpiceCredentialLifetime::Temporary,
            tcp_relay: None,
            tls_relay: None,
        };
        config.settings.reconnect_attempts = 100;
        assert!(!config.can_reconnect());
        config.credential_lifetime = SpiceCredentialLifetime::Saved;
        assert!(config.can_reconnect());
    }

    #[test]
    fn invalid_display_ids_and_framebuffer_sizes_are_rejected() {
        assert!(
            encode_control(&SpiceControl::SurfaceReset {
                channel: 0,
                epoch: 1,
                width: u32::MAX,
                height: 1
            })
            .is_err()
        );
        assert!(encode_control(&SpiceControl::SurfaceRemoved { channel: 256 }).is_err());
        assert!(
            encode_control(&SpiceControl::Input {
                events: vec![SpiceInput::Relative {
                    dx: i32::MIN,
                    dy: 0
                }]
            })
            .is_err()
        );
    }
}

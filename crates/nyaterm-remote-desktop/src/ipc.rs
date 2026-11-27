//! Compatibility facade for the independently versioned native contract.
pub use nyaterm_native_protocol::ipc::{
    CONTROL_PAYLOAD_LIMIT, CURSOR_PAYLOAD_LIMIT, FRAME_PAYLOAD_LIMIT, HEADER_LEN, Packet,
    PacketReader, PacketType, decode_control, decode_cursor_packet, decode_cursor_packet_owned,
    decode_frame_packet, decode_frame_packet_owned, decode_vnc_control, encode_control,
    encode_cursor_packet, encode_frame_packet, encode_frame_packet_owned, encode_vnc_control,
    read_packet, read_packet_budgeted, write_packet, write_packet_into,
};

#[cfg(test)]
pub use nyaterm_native_protocol::ipc::read_packet_inner;

//! Android helper wire contract.
pub use nyaterm_native_protocol::android::ipc::{
    ColorMatrix, Control, Frame, FrameColor, InputOperation, InputRejection, MAX_CAPTURE_EDGE,
    MAX_CONTROL_BYTES, MAX_FRAME_BYTES, MAX_TEXT_BYTES, PROTOCOL_VERSION, Packet, PixelFormat,
    TouchPoint, TouchPointer, presentation_size, read_packet, read_packet_reusing, valid_capture,
    write_packet,
};

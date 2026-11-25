//! Compatibility facade for the independently versioned native contract.
pub use nyaterm_native_protocol::spice::ipc::{
    decode_control, decode_cursor, decode_frame, encode_control, encode_cursor, encode_frame,
};

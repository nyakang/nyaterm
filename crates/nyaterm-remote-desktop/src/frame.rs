//! Compatibility facade for the independently versioned native contract.
pub use nyaterm_native_protocol::frame::{
    DirtyRect, Framebuffer, FramebufferError, FramebufferLimits, RDP_FRAMEBUFFER_LIMITS,
    VNC_FRAMEBUFFER_LIMITS, frame_batch_len, merge_dirty_rects, validate_framebuffer_dimensions,
};

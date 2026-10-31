//! Remote desktop adapters grouped by lifecycle, events, input, framebuffer, trust, resize and clipboard.
//! Protocol managers and feature state retain their existing single owners.

mod clipboard;
mod errors;
mod events;
mod framebuffer;
mod input;
mod lifecycle;
mod resize;
mod start;
#[cfg(test)]
mod tests;
mod trust;
#[cfg(test)]
use super::state::resize::RESIZE_DEBOUNCE;
pub(super) use errors::format_remote_desktop_error;
#[cfg(test)]
use events::MAINTENANCE_INTERVAL;
#[cfg(test)]
use events::remote_desktop_drain_delay;
#[cfg(test)]
use events::remote_desktop_periodic_delay;
#[cfg(test)]
use events::remote_state_clears_input;
#[cfg(test)]
use events::vnc_capabilities_for_state;
#[cfg(test)]
use input::POINTER_MOVE_INTERVAL;
#[cfg(test)]
use input::defer_rdp_pointer_move;
#[cfg(test)]
use input::rdp_key_modifiers;
#[cfg(test)]
use input::record_remote_cursor_position_if_sent;
#[cfg(test)]
use input::remote_committed_text_supported;
#[cfg(test)]
use input::remote_cursor_repaint_interval;
#[cfg(test)]
use input::remote_modifier_transitions;
#[cfg(test)]
use input::secure_attention_available;
#[cfg(test)]
use input::vnc_input_allowed;
#[cfg(test)]
use input::vnc_keysym_for_key;
#[cfg(test)]
use lifecycle::cancel_rdp_reconnect_schedule;
#[cfg(test)]
use lifecycle::clear_rdp_reconnect_after_frame;
#[cfg(test)]
use lifecycle::inline_remote_desktop_password;
#[cfg(test)]
use lifecycle::rdp_error_is_retryable;
#[cfg(test)]
use lifecycle::rdp_reconnect_delay;
#[cfg(test)]
use lifecycle::remote_desktop_authentication_disabled;
#[cfg(test)]
use lifecycle::remote_desktop_password_id;
#[cfg(test)]
use lifecycle::vnc_auth_failure_accepts_password_prompt;
#[cfg(test)]
use lifecycle::vnc_retry_requires_password;
#[cfg(windows)]
pub(super) use resize::fit_window_display_metrics;

use std::ffi::{c_char, c_int};
use std::ptr;

use nyaterm_core::secret::SecretString;
use nyaterm_remote_desktop::spice::display::SpiceMonitorRequest;
use nyaterm_remote_desktop::spice::protocol::{
    MAX_SPICE_CLIPBOARD_BYTES, SpiceCapabilities, SpiceControl, SpiceErrorKind, SpiceMouseMode,
};

use crate::native::abi::Object;
use crate::native::client::{Callbacks, NativeClient, NativeEvent, callback};

// spice-protocol vd_agent.h capability indexes, not flag masks.
const CAP_MONITORS: u32 = 1;
const CAP_CLIPBOARD_BY_DEMAND: u32 = 5;
const CAP_GUEST_CRLF: u32 = 9;
const CAP_FILE_TRANSFER_DISABLED: u32 = 13;
const CLIPBOARD_SELECTION: u32 = 0;
const CLIPBOARD_UTF8: u32 = 1;

#[derive(Default)]
pub(super) struct AgentState {
    pub capabilities: SpiceCapabilities,
    offered_text: SecretString,
    local_offer: bool,
    receive: ClipboardReceive,
    guest_crlf: bool,
}

#[derive(Default)]
struct ClipboardReceive {
    pending: bool,
    current: bool,
    again: bool,
}

impl ClipboardReceive {
    fn offer(&mut self) -> bool {
        self.current = true;
        if self.pending {
            self.again = true;
            false
        } else {
            self.pending = true;
            true
        }
    }

    fn response(&mut self) -> (bool, bool) {
        if !self.pending {
            return (false, false);
        }
        self.pending = self.again && self.current;
        let repeat = self.pending;
        self.again = false;
        (self.current && !repeat, repeat)
    }

    fn release(&mut self) {
        // Keep track of an outstanding reply so it cannot satisfy a later
        // owner's request. Replies are serialized by the SPICE agent channel.
        self.current = false;
        self.again = false;
    }
}

pub(super) fn attach_signals(cb: &Callbacks, channel: Object) {
    attach!(cb, channel, c"main-agent-update", agent_update);
    attach!(cb, channel, c"main-mouse-update", agent_update);
    attach!(
        cb,
        channel,
        c"main-clipboard-selection-grab",
        clipboard_grab
    );
    attach!(
        cb,
        channel,
        c"main-clipboard-selection-request",
        clipboard_request
    );
    attach!(cb, channel, c"main-clipboard-selection", clipboard_data);
    attach!(
        cb,
        channel,
        c"main-clipboard-selection-release",
        clipboard_release
    );
    // Bound the native agent accumulator as well as the Rust IPC payload.
    unsafe {
        (cb.api().g_object_set)(
            channel,
            c"max-clipboard".as_ptr(),
            MAX_SPICE_CLIPBOARD_BYTES as c_int,
            ptr::null::<c_char>(),
        );
    }
}

impl NativeClient<'_> {
    pub fn set_clipboard(&self, text: SecretString) -> Result<(), SpiceErrorKind> {
        if text.len() > MAX_SPICE_CLIPBOARD_BYTES || text.contains('\0') {
            return Err(SpiceErrorKind::Clipboard);
        }
        let main = {
            let mut state = self.callbacks.state.borrow_mut();
            if state.config.settings.view_only || !state.config.settings.clipboard.can_send() {
                return Err(SpiceErrorKind::Clipboard);
            }
            if !state.agent.capabilities.clipboard || state.main.is_null() {
                return Err(SpiceErrorKind::AgentUnavailable);
            }
            state.agent.offered_text = text;
            state.agent.local_offer = true;
            state.agent.receive.release();
            state.main
        };
        unsafe {
            (self
                .callbacks
                .api()
                .spice_main_channel_clipboard_selection_grab)(
                main,
                CLIPBOARD_SELECTION,
                &CLIPBOARD_UTF8,
                1,
            );
        }
        Ok(())
    }

    pub fn resize(&self, monitors: &[SpiceMonitorRequest]) -> Result<(), SpiceErrorKind> {
        SpiceControl::Resize {
            monitors: monitors.to_vec(),
        }
        .validate()
        .map_err(|_| SpiceErrorKind::Configuration)?;
        let main = {
            let state = self.callbacks.state.borrow();
            if state.config.settings.view_only || !state.config.settings.dynamic_resolution {
                return Err(SpiceErrorKind::Configuration);
            }
            if state.main.is_null() || !state.agent.capabilities.monitor_config {
                return Err(SpiceErrorKind::AgentUnavailable);
            }
            state.main
        };
        // Submit the topology atomically, rather than one resize message per monitor.
        unsafe {
            for monitor in monitors {
                let id = monitor.id.agent_id().ok_or(SpiceErrorKind::Configuration)?;
                (self.callbacks.api().spice_main_channel_update_display)(
                    main,
                    id,
                    monitor.x,
                    monitor.y,
                    monitor.width as i32,
                    monitor.height as i32,
                    0,
                );
                (self
                    .callbacks
                    .api()
                    .spice_main_channel_update_display_enabled)(
                    main,
                    id,
                    c_int::from(monitor.enabled),
                    0,
                );
            }
            if (self.callbacks.api().spice_main_channel_send_monitor_config)(main) == 0 {
                return Err(SpiceErrorKind::AgentUnavailable);
            }
        }
        Ok(())
    }
}

pub(super) fn publish_capabilities(cb: &Callbacks, channel: Object) {
    let agent_connected = cb.integer(channel, c"agent-connected") != 0;
    let has = |cap| {
        agent_connected
            && unsafe { (cb.api().spice_main_channel_agent_test_capability)(channel, cap) != 0 }
    };
    let mouse_mode = if cb.integer(channel, c"mouse-mode") == 2 {
        SpiceMouseMode::Absolute
    } else {
        SpiceMouseMode::Relative
    };
    let capabilities = SpiceCapabilities {
        agent_connected,
        mouse_mode,
        clipboard: has(CAP_CLIPBOARD_BY_DEMAND),
        monitor_config: has(CAP_MONITORS),
        file_transfer: agent_connected && !has(CAP_FILE_TRANSFER_DISABLED),
        audio_playback: cb.audio.ready(),
    };
    let guest_crlf = has(CAP_GUEST_CRLF);
    let changed = {
        let mut state = cb.state.borrow_mut();
        state.agent.guest_crlf = guest_crlf;
        if !agent_connected {
            state.agent.offered_text = SecretString::default();
            state.agent.local_offer = false;
            state.agent.receive = ClipboardReceive::default();
        }
        let changed = state.agent.capabilities != capabilities;
        state.agent.capabilities = capabilities.clone();
        changed
    };
    if changed {
        cb.emit(NativeEvent::Control(SpiceControl::Capabilities {
            capabilities,
        }));
    }
}

unsafe extern "C" fn agent_update(channel: Object, data: Object) {
    callback(data, |cb| publish_capabilities(cb, channel));
}

unsafe extern "C" fn clipboard_grab(
    channel: Object,
    selection: u32,
    types: *const u32,
    count: u32,
    data: Object,
) -> c_int {
    let mut accepted = 0;
    callback(data, |cb| {
        if selection != CLIPBOARD_SELECTION || count > 64 || count == 0 || types.is_null() {
            return;
        }
        {
            let mut state = cb.state.borrow_mut();
            state.agent.offered_text = SecretString::default();
            state.agent.local_offer = false;
        }
        let receive = {
            let state = cb.state.borrow();
            state.config.settings.clipboard.can_receive() && state.agent.capabilities.clipboard
        };
        if !receive {
            return;
        }
        // The signal's array is valid for this callback only.
        if unsafe { std::slice::from_raw_parts(types, count as usize) }.contains(&CLIPBOARD_UTF8) {
            let request = cb.state.borrow_mut().agent.receive.offer();
            if request {
                unsafe {
                    (cb.api().spice_main_channel_clipboard_selection_request)(
                        channel,
                        selection,
                        CLIPBOARD_UTF8,
                    );
                }
            }
            accepted = 1;
        } else {
            cb.state.borrow_mut().agent.receive.release();
        }
    });
    accepted
}

unsafe extern "C" fn clipboard_request(
    channel: Object,
    selection: u32,
    format: u32,
    data: Object,
) -> c_int {
    let mut accepted = 0;
    callback(data, |cb| {
        if selection != CLIPBOARD_SELECTION || format != CLIPBOARD_UTF8 {
            return;
        }
        let text = {
            let state = cb.state.borrow();
            if !state.agent.local_offer
                || !state.agent.capabilities.clipboard
                || !state.config.settings.clipboard.can_send()
                || state.config.settings.view_only
            {
                return;
            }
            normalize_line_endings(
                state.agent.offered_text.expose_secret(),
                state.agent.guest_crlf,
            )
        };
        let Ok(text) = text else {
            cb.error(SpiceErrorKind::Clipboard, false);
            return;
        };
        unsafe {
            (cb.api().spice_main_channel_clipboard_selection_notify)(
                channel,
                selection,
                format,
                text.expose_secret().as_ptr(),
                text.len(),
            );
        }
        accepted = 1;
    });
    accepted
}

unsafe extern "C" fn clipboard_data(
    channel: Object,
    selection: u32,
    format: u32,
    bytes: *const u8,
    size: u32,
    data: Object,
) {
    callback(data, |cb| {
        if selection != CLIPBOARD_SELECTION {
            return;
        }
        let (allowed, repeat) = {
            let mut state = cb.state.borrow_mut();
            let (deliver, repeat) = state.agent.receive.response();
            (
                deliver && state.config.settings.clipboard.can_receive(),
                repeat,
            )
        };
        if repeat {
            unsafe {
                (cb.api().spice_main_channel_clipboard_selection_request)(
                    channel,
                    selection,
                    CLIPBOARD_UTF8,
                );
            }
        }
        if !allowed || selection != CLIPBOARD_SELECTION || format != CLIPBOARD_UTF8 {
            return;
        }
        if size as usize > MAX_SPICE_CLIPBOARD_BYTES || (size != 0 && bytes.is_null()) {
            cb.error(SpiceErrorKind::Clipboard, false);
            return;
        }
        let bytes = if size == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(bytes, size as usize) }
        };
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        let Ok(text) = std::str::from_utf8(bytes) else {
            cb.error(SpiceErrorKind::Clipboard, false);
            return;
        };
        if text.contains('\0') {
            cb.error(SpiceErrorKind::Clipboard, false);
            return;
        }
        cb.emit(NativeEvent::Control(SpiceControl::ClipboardOffer {
            text: text.replace("\r\n", "\n").into(),
        }));
    });
}

unsafe extern "C" fn clipboard_release(_channel: Object, selection: u32, data: Object) {
    callback(data, |cb| {
        if selection == CLIPBOARD_SELECTION {
            cb.state.borrow_mut().agent.receive.release();
        }
    });
}

fn normalize_line_endings(text: &str, crlf: bool) -> Result<SecretString, SpiceErrorKind> {
    let text = zeroize::Zeroizing::new(text.replace("\r\n", "\n"));
    let result = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    let result = SecretString::from(result);
    if result.len() > MAX_SPICE_CLIPBOARD_BYTES {
        return Err(SpiceErrorKind::Clipboard);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::native::client::agent::{ClipboardReceive, normalize_line_endings};
    use nyaterm_remote_desktop::spice::protocol::MAX_SPICE_CLIPBOARD_BYTES;

    #[test]
    fn clipboard_conversion_does_not_double_crlf_and_bounds_expansion() {
        assert_eq!(
            normalize_line_endings("a\r\nb\nc", true).unwrap(),
            "a\r\nb\r\nc"
        );
        assert_eq!(normalize_line_endings("a\r\nb", false).unwrap(), "a\nb");
        assert!(normalize_line_endings(&"\n".repeat(MAX_SPICE_CLIPBOARD_BYTES), true).is_err());
    }

    #[test]
    fn rapid_regrab_and_release_never_publish_a_previous_owners_reply() {
        let mut receive = ClipboardReceive::default();
        assert!(receive.offer());
        assert!(!receive.offer());
        assert_eq!(receive.response(), (false, true));
        assert_eq!(receive.response(), (true, false));
        assert!(receive.offer());
        receive.release();
        assert_eq!(receive.response(), (false, false));
        assert_eq!(receive.response(), (false, false));
        assert!(receive.offer());
        receive.release();
        assert!(!receive.offer());
        assert_eq!(receive.response(), (false, true));
        assert_eq!(receive.response(), (true, false));
    }
}

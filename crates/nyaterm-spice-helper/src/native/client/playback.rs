use std::ffi::{c_char, c_int};
use std::ptr;

use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceErrorKind};

use crate::native::abi::Object;
use crate::native::client::{Callbacks, NativeEvent, callback};

pub(super) fn attach_signals(cb: &Callbacks, channel: Object) {
    attach!(cb, channel, c"playback-start", start);
    attach!(cb, channel, c"playback-data", pcm);
    attach!(cb, channel, c"playback-stop", stop);
    attach!(cb, channel, c"playback-get-delay", delay);
    attach!(cb, channel, c"notify::volume", volume);
    attach!(cb, channel, c"notify::mute", volume);
    attach!(cb, channel, c"notify::min-latency", volume);
}

pub(super) fn poll(cb: &Callbacks) {
    if cb.audio.take_failure() {
        cb.error(SpiceErrorKind::Audio, false);
    }
    let ready = cb.audio.ready();
    let changed = {
        let mut state = cb.state.borrow_mut();
        if state.agent.capabilities.audio_playback == ready {
            None
        } else {
            state.agent.capabilities.audio_playback = ready;
            Some(state.agent.capabilities.clone())
        }
    };
    if let Some(capabilities) = changed {
        cb.emit(NativeEvent::Control(SpiceControl::Capabilities {
            capabilities,
        }));
    }
}

unsafe extern "C" fn start(
    channel: Object,
    format: c_int,
    channels: c_int,
    rate: c_int,
    data: Object,
) {
    callback(data, |cb| {
        if format != 1 || !cb.audio.start(channels, rate) {
            cb.audio.stop();
            cb.error(SpiceErrorKind::Audio, false);
        } else {
            update_volume(cb, channel);
        }
    });
}

unsafe extern "C" fn pcm(_channel: Object, bytes: Object, size: c_int, data: Object) {
    callback(data, |cb| {
        if !(0..=192_000 * 4).contains(&size) || (size > 0 && bytes.is_null()) {
            cb.error(SpiceErrorKind::Audio, false);
            return;
        }
        // The decoded native PCM is borrowed only during this signal.
        let bytes = if size == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size as usize) }
        };
        if !cb.audio.push(bytes) {
            cb.error(SpiceErrorKind::Audio, false);
        }
    });
}

unsafe extern "C" fn stop(_channel: Object, data: Object) {
    callback(data, |cb| cb.audio.stop());
}

unsafe extern "C" fn delay(channel: Object, data: Object) {
    callback(data, |cb| unsafe {
        (cb.api().spice_playback_channel_set_delay)(channel, cb.audio.delay_ms());
    });
}

unsafe extern "C" fn volume(channel: Object, _property: Object, data: Object) {
    callback(data, |cb| update_volume(cb, channel));
}

fn update_volume(cb: &Callbacks, channel: Object) {
    cb.audio
        .set_latency_ms(cb.integer(channel, c"min-latency").max(0) as u32);
    let mut count = 0u32;
    let mut values: *const u16 = ptr::null();
    let mut mute = 0i32;
    // GObject's pointer property is borrowed from the channel; it is not a
    // boxed allocation. Copy values before returning to the native main loop.
    unsafe {
        (cb.api().g_object_get)(
            channel,
            c"nchannels".as_ptr(),
            &mut count as *mut u32,
            c"volume".as_ptr(),
            &mut values as *mut *const u16,
            c"mute".as_ptr(),
            &mut mute as *mut i32,
            ptr::null::<c_char>(),
        );
    }
    let mut volume = [1.0; 2];
    if !values.is_null() && count > 0 {
        volume[0] = f32::from(unsafe { *values }) / 65535.0;
        volume[1] = if count > 1 {
            f32::from(unsafe { *values.add(1) }) / 65535.0
        } else {
            volume[0]
        };
    }
    cb.audio.set_server_volume(volume, mute != 0);
}

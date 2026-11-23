use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ffi::{CStr, c_char, c_int, c_ulong};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use nyaterm_core::spice::settings::SpiceChannel;
use nyaterm_remote_desktop::spice::display::{SpiceDisplayId, SpiceMonitor, validate_monitors};
use nyaterm_remote_desktop::spice::protocol::{
    SpiceControl, SpiceErrorKind, SpiceInput, SpiceSessionConfig, SpiceSessionState,
};
use nyaterm_remote_desktop::{
    CursorPosition, CursorShape, CursorVisibility, DirtyRect, RemoteCursorEvent, RemoteFrameEvent,
    RemotePoint, RemotePointerButton, RemoteWheelAxis,
};

use crate::frame::{DamageTracker, FRAME_CHUNK_BYTES, SurfaceFormat, SurfaceLayout};
use crate::native::abi::{Api, Array, Callback, DisplayMonitor, Object};
use crate::native::{NativeError, NativeRuntime, NativeSession};

const MAX_NATIVE_CHANNELS: usize = 64;
const MAX_NATIVE_EVENTS: usize = 128;
const MAX_SURFACE_BYTES: usize = 512 * 1024 * 1024;

pub enum NativeEvent {
    Control(SpiceControl),
    Frame {
        channel: u32,
        event: RemoteFrameEvent,
    },
    Cursor {
        channel: u32,
        event: RemoteCursorEvent,
    },
}

struct Surface {
    layout: SurfaceLayout,
    /// Owned by spice-gtk until primary-destroy; only touched on the GLib thread.
    data: *const u8,
    damage: DamageTracker,
    epoch: u64,
}

struct State {
    config: SpiceSessionConfig,
    channels: BTreeMap<usize, (i32, u32)>,
    handlers: Vec<(Object, c_ulong)>,
    surfaces: BTreeMap<u32, Surface>,
    monitors: BTreeMap<u32, Vec<SpiceMonitor>>,
    events: VecDeque<NativeEvent>,
    main: Object,
    inputs: Object,
    epoch: u64,
    cursor_id: u64,
    pressed_keys: HashSet<u16>,
    buttons: u32,
    visible: bool,
    next_surface: u32,
    agent: agent::AgentState,
}

struct Callbacks {
    // The enclosing NativeClient borrows the runtime for longer than this box.
    api: *const Api,
    state: RefCell<State>,
    failed: Cell<bool>,
    audio: crate::audio::AudioOutput,
}

impl Callbacks {
    fn api(&self) -> &Api {
        // SAFETY: NativeClient's lifetime and drop order pin this API.
        unsafe { &*self.api }
    }

    fn emit(&self, event: NativeEvent) {
        let mut state = self.state.borrow_mut();
        if let NativeEvent::Cursor {
            channel,
            event: cursor,
        } = &event
        {
            state.events.retain(|queued| !matches!(queued,
                NativeEvent::Cursor { channel: old_channel, event: old }
                if old_channel == channel && std::mem::discriminant(old) == std::mem::discriminant(cursor)));
        }
        if state.events.len() >= MAX_NATIVE_EVENTS {
            self.failed.set(true);
        } else {
            state.events.push_back(event);
        }
    }

    fn error(&self, kind: SpiceErrorKind, fatal: bool) {
        if fatal {
            self.failed.set(true);
        }
        self.emit(NativeEvent::Control(SpiceControl::Error {
            error: kind,
            fatal,
        }));
    }

    fn identity(&self, channel: Object) -> Option<(i32, u32)> {
        self.state
            .borrow()
            .channels
            .get(&(channel as usize))
            .copied()
    }

    fn integer(&self, object: Object, name: &CStr) -> i32 {
        let mut value: c_int = 0;
        // SAFETY: callers use documented int/boolean channel properties.
        unsafe {
            (self.api().g_object_get)(
                object,
                name.as_ptr(),
                &mut value as *mut c_int,
                ptr::null::<c_char>(),
            );
        }
        value
    }

    fn attach(&self, object: Object, name: &CStr, callback: Callback) {
        // SAFETY: each callback below matches the corresponding public signal;
        // userdata stays boxed until every handler is disconnected in Drop.
        let id = unsafe {
            (self.api().g_signal_connect_data)(
                object,
                name.as_ptr(),
                callback,
                (self as *const Self).cast_mut().cast(),
                None,
                0,
            )
        };
        if id == 0 {
            self.failed.set(true);
        }
        self.state.borrow_mut().handlers.push((object, id));
    }
}

macro_rules! attach {
    ($callbacks:expr, $object:expr, $name:expr, $function:ident) => {{
        // GCallback intentionally erases a signal's C signature.
        let callback = unsafe {
            std::mem::transmute::<*const (), unsafe extern "C" fn()>($function as *const ())
        };
        $callbacks.attach($object, $name, Some(callback));
    }};
}

mod agent;
mod playback;
mod transfer;

/// A native connection, its signal userdata and all borrowed decoder surfaces.
/// Polling and commands must stay on the owning GLib thread.
pub struct NativeClient<'runtime> {
    session: NativeSession<'runtime>,
    callbacks: Box<Callbacks>,
    transfers: transfer::Transfers,
    _relay: Option<crate::relay::RelayProxy>,
}

impl<'runtime> NativeClient<'runtime> {
    pub fn connect(
        runtime: &'runtime NativeRuntime,
        config: SpiceSessionConfig,
    ) -> Result<Self, NativeError> {
        config
            .validate()
            .map_err(|_| NativeError::InvalidField("session"))?;
        let relay = if config.tcp_relay.is_some() || config.tls_relay.is_some() {
            Some(
                crate::relay::RelayProxy::start(&config)
                    .map_err(|_| NativeError::InvalidField("relay configuration"))?,
            )
        } else {
            None
        };
        let mut endpoint = config.endpoint.clone();
        if let Some(relay) = &relay {
            endpoint.proxy = Some(relay.url.clone());
        }
        let session = NativeSession::new(runtime)?;
        session.configure(
            &endpoint,
            &config.settings,
            &config.username,
            &config.password,
        )?;
        let callbacks = Box::new(Callbacks {
            api: &runtime.api,
            state: RefCell::new(State {
                config,
                channels: BTreeMap::new(),
                handlers: Vec::new(),
                surfaces: BTreeMap::new(),
                monitors: BTreeMap::new(),
                events: VecDeque::new(),
                main: ptr::null_mut(),
                inputs: ptr::null_mut(),
                epoch: 0,
                cursor_id: 0,
                pressed_keys: HashSet::new(),
                buttons: 0,
                visible: true,
                next_surface: 0,
                agent: agent::AgentState::default(),
            }),
            failed: Cell::new(false),
            audio: crate::audio::AudioOutput::new(),
        });
        attach!(callbacks, session.object, c"channel-new", channel_new);
        attach!(
            callbacks,
            session.object,
            c"channel-destroy",
            channel_destroy
        );
        let client = Self {
            session,
            callbacks,
            transfers: transfer::Transfers::new(),
            _relay: relay,
        };
        // SAFETY: callback userdata is stable; no state borrow crosses reentry.
        if unsafe { (runtime.api.spice_session_connect)(client.session.object) } == 0 {
            client.callbacks.error(SpiceErrorKind::Connection, true);
        }
        Ok(client)
    }

    pub fn failed(&self) -> bool {
        self.callbacks.failed.get()
    }

    /// Drain native sources with a bounded work budget, then return at most one
    /// chunk. The caller controls pipe backpressure by deciding when to poll.
    pub fn poll(&mut self) -> Option<NativeEvent> {
        self.poll_with_frames(true)
    }

    /// Even when the IPC writer is backpressured, native control and audio
    /// sources must continue to run. Only surface copying is suppressed.
    pub fn poll_with_frames(&mut self, allow_frames: bool) -> Option<NativeEvent> {
        playback::poll(&self.callbacks);
        for _ in 0..32 {
            if unsafe {
                (self.session.runtime.api.g_main_context_iteration)(self.session.runtime.context, 0)
            } == 0
            {
                break;
            }
            if self.callbacks.failed.get() {
                break;
            }
        }
        if let Some(event) = self.callbacks.state.borrow_mut().events.pop_front() {
            return Some(event);
        }
        if let Some(progress) = self.transfers.next_event(self.callbacks.api()) {
            return Some(NativeEvent::Control(progress));
        }
        if self.failed() {
            return None;
        }
        let mut state = self.callbacks.state.borrow_mut();
        if !state.visible || !allow_frames {
            return None;
        }
        let next_surface = state.next_surface;
        let channels: Vec<_> = state
            .surfaces
            .range(next_surface..)
            .chain(state.surfaces.range(..next_surface))
            .map(|(&channel, _)| channel)
            .collect();
        for channel in channels {
            state.next_surface = (channel + 1) % 256;
            let Some(surface) = state.surfaces.get_mut(&channel) else {
                continue;
            };
            let rect = match surface.damage.take_chunk(FRAME_CHUNK_BYTES) {
                Ok(Some(rect)) => rect,
                Ok(None) => continue,
                Err(_) => {
                    self.callbacks.failed.set(true);
                    return None;
                }
            };
            let length = match surface.layout.source_bytes() {
                Ok(length) => length,
                Err(_) => {
                    self.callbacks.failed.set(true);
                    return None;
                }
            };
            // SAFETY: primary-create validated dimensions, pointer and stride.
            // libspice owns this surface until primary-destroy. No GLib call
            // or yield occurs between reading the pointer and copying pixels.
            let source = unsafe {
                let start = if surface.layout.stride < 0 {
                    surface.data.offset(
                        (surface.layout.height as isize - 1) * surface.layout.stride as isize,
                    )
                } else {
                    surface.data
                };
                std::slice::from_raw_parts(start, length)
            };
            match surface.layout.bitmap(source, rect, surface.epoch) {
                Ok(event) => return Some(NativeEvent::Frame { channel, event }),
                Err(_) => {
                    self.callbacks.failed.set(true);
                    return None;
                }
            }
        }
        None
    }

    pub fn set_visible(&self, visible: bool) {
        let mut state = self.callbacks.state.borrow_mut();
        if visible && !state.visible {
            for surface in state.surfaces.values_mut() {
                surface.damage.mark_full();
            }
        }
        state.visible = visible;
    }

    pub fn set_muted(&self, muted: bool) {
        self.callbacks.audio.set_muted(muted);
    }

    pub fn refresh(&self) {
        for surface in self.callbacks.state.borrow_mut().surfaces.values_mut() {
            surface.damage.mark_full();
        }
    }

    pub fn send_input(&self, events: &[SpiceInput]) {
        for event in events {
            if !event.validate() {
                self.callbacks.error(SpiceErrorKind::Protocol, true);
                return;
            }
            if matches!(event, SpiceInput::ReleaseAll) {
                self.release_inputs();
                continue;
            }
            let (inputs, view_only) = {
                let state = self.callbacks.state.borrow();
                (state.inputs, state.config.settings.view_only)
            };
            if inputs.is_null() || view_only {
                continue;
            }
            let api = self.callbacks.api();
            // SAFETY: channel pointers are owned until channel-destroy on this
            // same thread. Event values are bounded by the shared contract.
            unsafe {
                match event {
                    SpiceInput::Key { scan_code, pressed } => {
                        if *pressed {
                            self.callbacks
                                .state
                                .borrow_mut()
                                .pressed_keys
                                .insert(*scan_code);
                            (api.spice_inputs_channel_key_press)(inputs, u32::from(*scan_code));
                        } else {
                            self.callbacks
                                .state
                                .borrow_mut()
                                .pressed_keys
                                .remove(scan_code);
                            (api.spice_inputs_channel_key_release)(inputs, u32::from(*scan_code));
                        }
                    }
                    SpiceInput::KeyLocks {
                        caps_lock,
                        num_lock,
                        scroll_lock,
                    } => {
                        (api.spice_inputs_channel_set_key_locks)(
                            inputs,
                            u32::from(*scroll_lock)
                                | (u32::from(*num_lock) << 1)
                                | (u32::from(*caps_lock) << 2),
                        );
                    }
                    SpiceInput::Absolute { display, position } => {
                        let buttons = self.callbacks.state.borrow().buttons;
                        (api.spice_inputs_channel_position)(
                            inputs,
                            position.x as i32,
                            position.y as i32,
                            display.agent_id().unwrap_or(0),
                            buttons as i32,
                        );
                    }
                    SpiceInput::Relative { dx, dy } => {
                        let buttons = self.callbacks.state.borrow().buttons;
                        (api.spice_inputs_channel_motion)(inputs, *dx, *dy, buttons as i32);
                    }
                    SpiceInput::Button { button, pressed } => {
                        let number = button_number(*button);
                        let buttons = {
                            let mut state = self.callbacks.state.borrow_mut();
                            if *pressed {
                                state.buttons |= 1 << (number - 1);
                            } else {
                                state.buttons &= !(1 << (number - 1));
                            }
                            state.buttons
                        };
                        if *pressed {
                            (api.spice_inputs_channel_button_press)(inputs, number, buttons as i32);
                        } else {
                            (api.spice_inputs_channel_button_release)(
                                inputs,
                                number,
                                buttons as i32,
                            );
                        }
                    }
                    SpiceInput::Wheel { axis, steps } => {
                        let button = match (axis, *steps > 0) {
                            (RemoteWheelAxis::Vertical, true) => 4,
                            (RemoteWheelAxis::Vertical, false) => 5,
                            // SPICE buttons 6/7 are navigation, not horizontal
                            // wheel buttons. Never turn a wheel into Back/Forward.
                            (RemoteWheelAxis::Horizontal, _) => continue,
                        };
                        let buttons = self.callbacks.state.borrow().buttons as i32;
                        for _ in 0..steps.unsigned_abs() {
                            (api.spice_inputs_channel_button_press)(inputs, button, buttons);
                            (api.spice_inputs_channel_button_release)(inputs, button, buttons);
                        }
                    }
                    SpiceInput::ReleaseAll => {}
                }
            }
        }
    }

    pub fn release_inputs(&self) {
        let (inputs, keys, buttons) = {
            let mut state = self.callbacks.state.borrow_mut();
            (
                state.inputs,
                std::mem::take(&mut state.pressed_keys),
                std::mem::take(&mut state.buttons),
            )
        };
        if inputs.is_null() {
            return;
        }
        unsafe {
            for key in keys {
                (self.callbacks.api().spice_inputs_channel_key_release)(inputs, u32::from(key));
            }
            for button in 1..=7 {
                if buttons & (1 << (button - 1)) != 0 {
                    (self.callbacks.api().spice_inputs_channel_button_release)(inputs, button, 0);
                }
            }
        }
    }
}

impl Drop for NativeClient<'_> {
    fn drop(&mut self) {
        self.release_inputs();
        self.transfers.shutdown(self.callbacks.api());
        // Disconnect every signal before freeing userdata. Hold a native ref
        // to each channel so pointer validity does not depend on session state.
        let (handlers, channels) = {
            let mut state = self.callbacks.state.borrow_mut();
            (
                std::mem::take(&mut state.handlers),
                std::mem::take(&mut state.channels),
            )
        };
        unsafe {
            for (object, id) in handlers {
                (self.callbacks.api().g_signal_handler_disconnect)(object, id);
            }
            for pointer in channels.keys() {
                (self.callbacks.api().g_object_unref)(*pointer as Object);
            }
        }
    }
}

fn button_number(button: RemotePointerButton) -> i32 {
    match button {
        RemotePointerButton::Left => 1,
        RemotePointerButton::Middle => 2,
        RemotePointerButton::Right => 3,
        RemotePointerButton::X1 => 6,
        RemotePointerButton::X2 => 7,
    }
}

fn callback(data: Object, action: impl FnOnce(&Callbacks)) {
    // SAFETY: only registered signals call this, with the pinned callback box.
    let callbacks = unsafe { &*data.cast::<Callbacks>() };
    if catch_unwind(AssertUnwindSafe(|| action(callbacks))).is_err() {
        callbacks.failed.set(true);
    }
}

unsafe extern "C" fn channel_new(_session: Object, channel: Object, data: Object) {
    callback(data, |cb| {
        let kind = cb.integer(channel, c"channel-type");
        let id = cb.integer(channel, c"channel-id");
        if !(0..=255).contains(&id) || cb.state.borrow().channels.len() >= MAX_NATIVE_CHANNELS {
            cb.error(SpiceErrorKind::ResourceLimit, true);
            return;
        }
        let feature = match kind {
            1 => Some(SpiceChannel::Main),
            2 => Some(SpiceChannel::Display),
            3 => Some(SpiceChannel::Inputs),
            4 => Some(SpiceChannel::Cursor),
            5 => Some(SpiceChannel::Playback),
            _ => None,
        };
        let permitted = {
            let state = cb.state.borrow();
            feature.is_some_and(|f| !state.config.settings.disabled_channels.contains(&f))
                && (kind != 5 || (state.config.settings.audio_playback && id == 0))
        };
        if !permitted {
            return;
        }
        unsafe {
            (cb.api().g_object_ref)(channel);
        }
        cb.state
            .borrow_mut()
            .channels
            .insert(channel as usize, (kind, id as u32));
        attach!(cb, channel, c"channel-event", channel_event);
        match kind {
            1 => {
                cb.state.borrow_mut().main = channel;
                agent::attach_signals(cb, channel);
            }
            2 => {
                attach!(cb, channel, c"display-primary-create", primary_create);
                attach!(cb, channel, c"display-primary-destroy", primary_destroy);
                attach!(cb, channel, c"display-invalidate", invalidate);
                attach!(cb, channel, c"notify::monitors", monitors_changed);
            }
            3 => cb.state.borrow_mut().inputs = channel,
            4 => {
                attach!(cb, channel, c"cursor-set", cursor_set);
                attach!(cb, channel, c"cursor-move", cursor_move);
                attach!(cb, channel, c"cursor-hide", cursor_hide);
                attach!(cb, channel, c"cursor-reset", cursor_hide);
            }
            5 => playback::attach_signals(cb, channel),
            _ => {}
        }
        if kind != 1 {
            unsafe {
                (cb.api().spice_channel_connect)(channel);
            }
        }
    });
}

unsafe extern "C" fn channel_destroy(_session: Object, channel: Object, data: Object) {
    callback(data, |cb| {
        let (identity, handlers) = {
            let mut state = cb.state.borrow_mut();
            let identity = state.channels.remove(&(channel as usize));
            let mut removed = Vec::new();
            state.handlers.retain(|(object, id)| {
                if *object == channel {
                    removed.push(*id);
                    false
                } else {
                    true
                }
            });
            if state.inputs == channel {
                state.inputs = ptr::null_mut();
                state.pressed_keys.clear();
                state.buttons = 0;
            }
            if state.main == channel {
                state.main = ptr::null_mut();
            }
            (identity, removed)
        };
        if let Some((kind, id)) = identity {
            if kind == 2 {
                remove_surface(cb, id);
            }
            if kind == 5 {
                cb.audio.stop();
            }
            unsafe {
                for handler in handlers {
                    (cb.api().g_signal_handler_disconnect)(channel, handler);
                }
                (cb.api().g_object_unref)(channel);
            }
        }
    });
}

unsafe extern "C" fn channel_event(channel: Object, event: c_int, data: Object) {
    callback(data, |cb| {
        let kind = cb.identity(channel).map(|v| v.0);
        match event {
            10 if kind == Some(1) => cb.emit(NativeEvent::Control(SpiceControl::State {
                state: SpiceSessionState::Connected,
            })),
            11 if kind == Some(1) => cb.emit(NativeEvent::Control(SpiceControl::State {
                state: SpiceSessionState::Migrating,
            })),
            12 if kind == Some(1) => cb.emit(NativeEvent::Control(SpiceControl::State {
                state: SpiceSessionState::Disconnected,
            })),
            20..=24 => {
                let error = match event {
                    21 => SpiceErrorKind::Certificate,
                    23 => SpiceErrorKind::Authentication,
                    22 => SpiceErrorKind::Protocol,
                    _ => SpiceErrorKind::Connection,
                };
                cb.error(error, kind == Some(1));
            }
            _ => {}
        }
    });
}

unsafe extern "C" fn primary_create(
    channel: Object,
    format: c_int,
    width: c_int,
    height: c_int,
    stride: c_int,
    _shmid: c_int,
    pixels: Object,
    data: Object,
) {
    callback(data, |cb| {
        let Some((_, id)) = cb.identity(channel) else {
            return;
        };
        let Ok(format) = SurfaceFormat::from_spice(format) else {
            cb.error(SpiceErrorKind::Protocol, true);
            return;
        };
        let layout = SurfaceLayout {
            width: width as u32,
            height: height as u32,
            stride,
            format,
        };
        let Ok(bytes) = layout.source_bytes() else {
            cb.error(SpiceErrorKind::ResourceLimit, true);
            return;
        };
        if pixels.is_null() {
            cb.error(SpiceErrorKind::Protocol, true);
            return;
        }
        let used = cb
            .state
            .borrow()
            .surfaces
            .iter()
            .filter(|(key, _)| **key != id)
            .map(|(_, s)| s.layout.source_bytes().unwrap_or(MAX_SURFACE_BYTES))
            .sum::<usize>();
        if used.saturating_add(bytes) > MAX_SURFACE_BYTES {
            cb.error(SpiceErrorKind::ResourceLimit, true);
            return;
        }
        let Ok(mut damage) = DamageTracker::new(layout.width, layout.height) else {
            return;
        };
        damage.mark_full();
        let epoch = {
            let mut state = cb.state.borrow_mut();
            state.epoch = state.epoch.saturating_add(1);
            let epoch = state.epoch;
            state.surfaces.insert(
                id,
                Surface {
                    layout,
                    data: pixels.cast(),
                    damage,
                    epoch,
                },
            );
            epoch
        };
        cb.emit(NativeEvent::Control(SpiceControl::SurfaceReset {
            channel: id,
            epoch,
            width: layout.width,
            height: layout.height,
        }));
        update_monitors(cb, channel);
    });
}

unsafe extern "C" fn primary_destroy(channel: Object, data: Object) {
    callback(data, |cb| {
        if let Some((_, id)) = cb.identity(channel) {
            remove_surface(cb, id);
        }
    });
}

fn remove_surface(cb: &Callbacks, id: u32) {
    let removed = {
        let mut state = cb.state.borrow_mut();
        state.monitors.remove(&id);
        state.surfaces.remove(&id).is_some()
    };
    if removed {
        cb.emit(NativeEvent::Control(SpiceControl::SurfaceRemoved {
            channel: id,
        }));
    }
}

unsafe extern "C" fn invalidate(
    channel: Object,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    data: Object,
) {
    callback(data, |cb| {
        let Some((_, id)) = cb.identity(channel) else {
            return;
        };
        let invalid = {
            let mut state = cb.state.borrow_mut();
            state.surfaces.get_mut(&id).is_some_and(|surface| {
                surface
                    .damage
                    .mark(DirtyRect {
                        x: x as u32,
                        y: y as u32,
                        width: width as u32,
                        height: height as u32,
                    })
                    .is_err()
            })
        };
        if invalid {
            cb.error(SpiceErrorKind::Protocol, true);
        }
    });
}

unsafe extern "C" fn monitors_changed(channel: Object, _property: Object, data: Object) {
    callback(data, |cb| update_monitors(cb, channel));
}

fn update_monitors(cb: &Callbacks, channel: Object) {
    let Some((_, id)) = cb.identity(channel) else {
        return;
    };
    let mut array: *mut Array = ptr::null_mut();
    unsafe {
        (cb.api().g_object_get)(
            channel,
            c"monitors".as_ptr(),
            &mut array as *mut *mut Array,
            ptr::null::<c_char>(),
        );
    }
    let mut monitors = Vec::new();
    if !array.is_null() {
        // SAFETY: the property is a referenced GArray of SpiceDisplayMonitorConfig.
        // Bound count before forming a slice, then release that property reference.
        unsafe {
            if (*array).len <= 16 && ((*array).len == 0 || !(*array).data.is_null()) {
                if (*array).len > 0 {
                    for m in std::slice::from_raw_parts(
                        (*array).data.cast::<DisplayMonitor>(),
                        (*array).len as usize,
                    ) {
                        if m.width == 0 || m.height == 0 {
                            continue;
                        }
                        monitors.push(SpiceMonitor {
                            id: SpiceDisplayId {
                                channel: id,
                                monitor: m.id,
                            },
                            surface: m.surface_id,
                            x: m.x,
                            y: m.y,
                            width: m.width,
                            height: m.height,
                        });
                    }
                }
            } else {
                cb.failed.set(true);
            }
            (cb.api().g_array_unref)(array);
        }
    }
    if monitors.is_empty()
        && let Some(surface) = cb.state.borrow().surfaces.get(&id)
    {
        monitors.push(SpiceMonitor {
            id: SpiceDisplayId {
                channel: id,
                monitor: 0,
            },
            surface: 0,
            x: 0,
            y: 0,
            width: surface.layout.width,
            height: surface.layout.height,
        });
    }
    cb.state.borrow_mut().monitors.insert(id, monitors);
    let monitors: Vec<_> = cb
        .state
        .borrow()
        .monitors
        .values()
        .flatten()
        .cloned()
        .collect();
    if validate_monitors(&monitors).is_err() {
        cb.error(SpiceErrorKind::Protocol, true);
        return;
    }
    cb.emit(NativeEvent::Control(SpiceControl::Monitors { monitors }));
}

unsafe extern "C" fn cursor_set(
    channel: Object,
    width: c_int,
    height: c_int,
    hot_x: c_int,
    hot_y: c_int,
    pixels: Object,
    data: Object,
) {
    callback(data, |cb| {
        let Some((_, id)) = cb.identity(channel) else {
            return;
        };
        // The public signal uses NULL to request the client's default cursor.
        if pixels.is_null() {
            cb.emit(NativeEvent::Cursor {
                channel: id,
                event: RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
            });
            return;
        }
        if width <= 0
            || height <= 0
            || width > 512
            || height > 512
            || hot_x < 0
            || hot_y < 0
            || hot_x >= width
            || hot_y >= height
        {
            cb.error(SpiceErrorKind::Protocol, true);
            return;
        }
        let shape_id = {
            let mut state = cb.state.borrow_mut();
            state.cursor_id = state.cursor_id.saturating_add(1);
            state.cursor_id
        };
        let length = width as usize * height as usize * 4;
        // SAFETY: cursor-set supplies width*height*4 RGBA bytes for the callback.
        let mut pixels =
            unsafe { std::slice::from_raw_parts(pixels.cast::<u8>(), length) }.to_vec();
        // spice-gtk's signal uses RGBA; NyaTerm's cursor texture contract is BGRA.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        cb.emit(NativeEvent::Cursor {
            channel: id,
            event: RemoteCursorEvent::Shape(CursorShape {
                shape_id,
                width: width as u32,
                height: height as u32,
                hotspot: RemotePoint {
                    x: hot_x as u32,
                    y: hot_y as u32,
                },
                pixels: pixels.into(),
            }),
        });
        cb.emit(NativeEvent::Cursor {
            channel: id,
            event: RemoteCursorEvent::Visibility(CursorVisibility { visible: true }),
        });
    });
}

unsafe extern "C" fn cursor_move(channel: Object, x: c_int, y: c_int, data: Object) {
    callback(data, |cb| {
        if let Some((_, id)) = cb.identity(channel) {
            cb.emit(NativeEvent::Cursor {
                channel: id,
                event: RemoteCursorEvent::Position(CursorPosition {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                }),
            });
        }
    });
}

unsafe extern "C" fn cursor_hide(channel: Object, data: Object) {
    callback(data, |cb| {
        if let Some((_, id)) = cb.identity(channel) {
            cb.emit(NativeEvent::Cursor {
                channel: id,
                event: RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
            });
        }
    });
}

//! Staged owner-thread controller. Preparing/committing an operation is pure;
//! only `NativeRdpOperation::run` calls the system control.
use std::{
    cell::Cell,
    collections::VecDeque,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use nyaterm_remote_desktop::activex::{
    NativeRdpConnectionEvent, NativeRdpDisconnectInfo, NativeRdpRect, NativeRdpSessionDetails,
};
use nyaterm_remote_desktop::{RdpDisplayMode, RdpError, RdpErrorKind, RdpSessionConfig};

use crate::display::DisplayUpdates;
use crate::lifecycle::{NativeRdpLifecycle, NativeRdpPhase};
use crate::native::NativeOperationLease;
use crate::{HostStatus, NativeRdpError, NativeRdpEventKind, NativeRdpHost, Viewport};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct Presentation {
    rect: NativeRdpRect,
    visible: bool,
    local_input: bool,
    restore_focus: bool,
    closing: bool,
}

pub struct NativeRdpControl {
    generation: u64,
    cancelled: Rc<Cell<bool>>,
    operating: Rc<Cell<bool>>,
    runtime: Option<Runtime>,
    desired: Presentation,
    focus_requested: Cell<bool>,
    events: VecDeque<NativeRdpConnectionEvent>,
    details: NativeRdpSessionDetails,
    details_changed: bool,
}

struct Runtime {
    parent: usize,
    generation: u64,
    config: Option<RdpSessionConfig>,
    host: Option<NativeRdpHost>,
    lifecycle: NativeRdpLifecycle,
    display: DisplayUpdates,
    fit: bool,
    dynamic_unsupported: bool,
    local_input: bool,
    events: VecDeque<NativeRdpConnectionEvent>,
    details: NativeRdpSessionDetails,
    last_native_status: Option<(HostStatus, NativeRdpPhase, bool, bool)>,
}

pub struct NativeRdpOperation {
    runtime: Runtime,
    presentation: Presentation,
    focus: bool,
    cancelled: Rc<Cell<bool>>,
    lease: NativeOperationLease,
}

pub struct NativeRdpCompleted {
    runtime: Runtime,
    // Drop the runtime (retiring its host) before releasing the operation lease.
    lease: NativeOperationLease,
}

impl NativeRdpControl {
    /// Records an initialization request; does not call Win32 or COM.
    pub fn create(
        parent: isize,
        rect: NativeRdpRect,
        config: &RdpSessionConfig,
    ) -> Result<Self, String> {
        let mut config = config.clone();
        config.normalize_backend_settings();
        nyaterm_remote_desktop::activex::validate_start(&config, rect)
            .map_err(|error| error.to_string())?;
        if parent == 0 {
            return Err("Invalid native RDP parent window".into());
        }
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let fit = config.display.mode == RdpDisplayMode::FitWindow;
        Ok(Self {
            generation,
            cancelled: Rc::new(Cell::new(false)),
            operating: Rc::new(Cell::new(false)),
            runtime: Some(Runtime {
                parent: parent as usize,
                generation,
                config: Some(config.clone()),
                host: None,
                lifecycle: NativeRdpLifecycle::new(generation),
                display: DisplayUpdates::new(generation),
                fit,
                dynamic_unsupported: false,
                local_input: false,
                events: VecDeque::new(),
                details: NativeRdpSessionDetails::default(),
                last_native_status: None,
            }),
            desired: Presentation {
                rect,
                visible: false,
                local_input: false,
                restore_focus: false,
                closing: false,
            },
            focus_requested: Cell::new(false),
            events: VecDeque::new(),
            details: NativeRdpSessionDetails::default(),
            details_changed: false,
        })
    }

    pub fn set_bounds(&mut self, rect: NativeRdpRect) {
        if rect.validate().is_ok() {
            self.desired.rect = rect;
        }
    }
    pub fn set_visible(&mut self, visible: bool) {
        self.desired.visible = visible;
    }
    pub fn set_local_input(&mut self, blocked: bool, restore_focus: bool) {
        self.desired.local_input = blocked;
        self.desired.restore_focus = restore_focus;
    }
    pub fn focus(&self) {
        self.focus_requested.set(true);
    }
    pub fn session_details(&self) -> NativeRdpSessionDetails {
        self.details.clone()
    }
    pub fn take_details_changed(&mut self) -> bool {
        std::mem::take(&mut self.details_changed)
    }
    pub fn poll_connection(&mut self) -> Option<NativeRdpConnectionEvent> {
        self.events.pop_front()
    }

    pub fn take_operation(&mut self) -> Option<NativeRdpOperation> {
        Some(NativeRdpOperation {
            runtime: self.runtime.take()?,
            presentation: self.desired,
            focus: self.focus_requested.replace(false),
            cancelled: self.cancelled.clone(),
            lease: NativeOperationLease::new(self.operating.clone()),
        })
    }

    /// Returns false for a result from an old/cancelled generation. Dropping the
    /// result retires its host on the creating thread without touching COM here.
    pub fn commit(&mut self, mut result: NativeRdpCompleted) -> bool {
        if result.runtime.generation != self.generation
            || self.cancelled.get()
            || self.runtime.is_some()
        {
            return false;
        }
        self.events.append(&mut result.runtime.events);
        self.details_changed |= self.details != result.runtime.details;
        self.details = result.runtime.details.clone();
        self.runtime = Some(result.runtime);
        drop(result.lease);
        true
    }

    pub fn request_shutdown(&mut self) {
        self.desired.closing = true;
        self.desired.visible = false;
        if self.runtime.is_none() {
            // An extracted operation can be running inside COM's message pump.
            self.cancelled.set(true);
        }
    }
    pub fn is_closed(&self) -> bool {
        !self.operating.get()
            && (self
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.lifecycle.phase() == NativeRdpPhase::Closed)
                || (self.desired.closing && self.runtime.is_none()))
    }
}

impl Drop for NativeRdpControl {
    fn drop(&mut self) {
        self.cancelled.set(true);
    }
}

impl NativeRdpOperation {
    /// Run on the creating UI/STA thread, after releasing every GPUI borrow.
    pub fn run(mut self) -> NativeRdpCompleted {
        if self.cancelled.get() {
            self.runtime.config = None;
            self.runtime.host = None;
            self.runtime.lifecycle.closed();
        } else if let Err(error) =
            self.runtime
                .drive(self.presentation, self.focus, &self.cancelled)
        {
            self.runtime
                .events
                .push_back(NativeRdpConnectionEvent::Failed(RdpError::new(
                    RdpErrorKind::Session,
                    error.to_string(),
                )));
            self.runtime.host = None;
            self.runtime.config = None;
            self.runtime.lifecycle.closed();
        }
        NativeRdpCompleted {
            runtime: self.runtime,
            lease: self.lease,
        }
    }
}

impl Runtime {
    fn drive(
        &mut self,
        presentation: Presentation,
        focus: bool,
        cancelled: &Cell<bool>,
    ) -> Result<(), NativeRdpError> {
        let now = Instant::now();
        if presentation.closing {
            self.lifecycle.begin_close(now);
            self.config = None;
        }
        if let Some(config) = self.config.take() {
            match unsafe { NativeRdpHost::create(self.parent, self.generation, &config) } {
                Ok(host) => self.host = Some(host),
                Err(error) => {
                    self.events
                        .push_back(NativeRdpConnectionEvent::Failed(RdpError::new(
                            RdpErrorKind::Unsupported,
                            error.to_string(),
                        )));
                    self.lifecycle.closed();
                    return Ok(());
                }
            }
            if cancelled.get() {
                self.host = None;
                self.lifecycle.closed();
                return Ok(());
            }
            if let Some(host) = self.host.as_mut() {
                let capabilities = host.capabilities()?;
                self.details.version = Some(capabilities.version);
                host.smart_sizing(!self.fit)?;
                // Size the control before Connect can pump a system dialog.
                // It remains hidden until the transport can show a logon surface.
                let rect = presentation.rect;
                host.viewport(
                    Viewport {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                        scale: rect.scale_percent,
                    },
                    false,
                )?;
                tracing::info!(
                    generation = self.generation,
                    "Windows RDP Connect requested"
                );
                host.start()?;
                tracing::info!(generation = self.generation, "Windows RDP Connect returned");
            }
        }
        if cancelled.get() {
            self.host = None;
            self.lifecycle.closed();
            return Ok(());
        }
        let Some(host) = self.host.as_mut() else {
            if presentation.closing {
                self.lifecycle.closed();
            }
            return Ok(());
        };
        let mut focus_surface = false;
        for _ in 0..256 {
            let Some(event) = host.poll()? else {
                break;
            };
            // Only typed event names and numeric codes; never connection data.
            tracing::info!(generation = self.generation, event = ?event.kind, "Windows RDP control event");
            if !self.lifecycle.apply(event) {
                continue;
            }
            match event.kind {
                NativeRdpEventKind::Connected => {
                    focus_surface = true;
                }
                NativeRdpEventKind::LoginComplete | NativeRdpEventKind::Reconnected => {
                    self.display.login_complete(now);
                    focus_surface = true;
                    self.events.push_back(NativeRdpConnectionEvent::Connected);
                }
                NativeRdpEventKind::Reconnecting => {
                    self.display.reconnecting();
                    self.events
                        .push_back(NativeRdpConnectionEvent::Reconnecting);
                }
                NativeRdpEventKind::Disconnected { basic, extended } => {
                    self.events
                        .push_back(NativeRdpConnectionEvent::Disconnected(
                            NativeRdpDisconnectInfo {
                                basic_reason: Some(basic),
                                extended_reason: Some(extended),
                                description: None,
                            },
                        ));
                }
                NativeRdpEventKind::Fatal { code } => {
                    self.events
                        .push_back(NativeRdpConnectionEvent::Failed(RdpError::new(
                            RdpErrorKind::Session,
                            format!("Windows RDP control failed (0x{:08X})", code as u32),
                        )));
                }
                _ => {}
            }
        }
        if presentation.closing {
            let wait = host.request_close()?;
            if !wait || self.lifecycle.may_destroy(now) {
                let host = self.host.take().expect("live close host");
                if let Err((host, _error)) = host.try_destroy() {
                    drop(host);
                }
                self.lifecycle.closed();
            }
            return Ok(());
        }
        let rect = presentation.rect;
        if self.fit && !self.dynamic_unsupported {
            self.display.observe(
                rect.width as u32,
                rect.height as u32,
                rect.scale_percent,
                now,
            );
            if let Some(request) = self.display.take_due(now) {
                let result = host.resize(request.width, request.height, request.scale);
                if self.display.complete(request, result.is_ok(), now) {
                    self.dynamic_unsupported = result.as_ref().is_err_and(|error| {
                        matches!(error.hresult as u32, 0x80004001 | 0x80004002 | 0x80020003)
                    });
                    if self.dynamic_unsupported {
                        host.smart_sizing(true)?;
                    }
                    self.details.dynamic_resize_available = Some(false);
                    self.details.status_text = Some("Dynamic resize failed after four attempts; retrying this size has stopped.".into());
                } else if result.is_ok() {
                    host.smart_sizing(false)?;
                    self.details.dynamic_resize_available = Some(true);
                    self.details.status_text = None;
                }
            }
        }
        let status = host.status()?;
        let visible = presentation.visible
            && self.lifecycle.can_present(status.drawing_ready != 0)
            && !cancelled.get();
        let native_status = (
            status,
            self.lifecycle.phase(),
            presentation.visible,
            visible,
        );
        if self.last_native_status != Some(native_status) {
            tracing::info!(
                generation = self.generation,
                connected = status.connected,
                drawing_ready = status.drawing_ready,
                surface_width = status.surface_width,
                surface_height = status.surface_height,
                surface_visible = status.surface_visible,
                requested_visible = presentation.visible,
                allowed_visible = visible,
                phase = ?self.lifecycle.phase(),
                "Windows RDP control status"
            );
            self.last_native_status = Some(native_status);
        }
        self.details.desktop_width = Some(status.width);
        self.details.desktop_height = Some(status.height);
        self.details.color_depth = Some(status.color_depth);
        self.details.cipher_strength = Some(status.cipher_strength);
        host.viewport(
            Viewport {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                scale: rect.scale_percent,
            },
            visible,
        )?;
        if presentation.local_input && !self.local_input {
            host.focus(false)?;
        }
        if visible
            && !presentation.local_input
            && (focus || focus_surface || (self.local_input && presentation.restore_focus))
        {
            host.focus(true)?;
        }
        self.local_input = presentation.local_input;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::NativeRdpControl;
    use nyaterm_remote_desktop::{RdpSessionConfig, activex::NativeRdpRect};

    fn control() -> NativeRdpControl {
        let config: RdpSessionConfig = serde_json::from_value(serde_json::json!({
            "backend":"windows", "windows_authentication":"system", "name":"fixture", "host":"example.invalid",
            "port":3389,"username":"","domain":"","password":null,"use_nla":true,"certificate_policy":"strict",
            "display":{"mode":"fit_window","width":800,"height":600,"color_depth":32},
            "clipboard":{"mode":"disabled"},"reconnect":{"enabled":false,"max_attempts":0}
        })).unwrap();
        NativeRdpControl::create(
            1,
            NativeRdpRect {
                width: 800,
                height: 600,
                scale_percent: 100,
                ..Default::default()
            },
            &config,
        )
        .unwrap()
    }

    #[test]
    fn cancelled_initialization_does_not_create_a_control_or_connect() {
        let mut control = control();
        let operation = control.take_operation().unwrap();
        drop(control);
        let completed = operation.run();
        assert!(completed.runtime.host.is_none());
        assert!(completed.runtime.config.is_none());
    }

    #[test]
    fn close_before_initialization_is_idempotent_and_performs_no_native_work() {
        let mut control = control();
        control.request_shutdown();
        control.request_shutdown();
        let completed = control.take_operation().unwrap().run();
        assert!(completed.runtime.host.is_none());
        assert!(control.commit(completed));
        assert!(control.is_closed());
    }

    #[test]
    fn shutdown_during_an_extracted_operation_waits_and_cancels_initialization() {
        let mut control = control();
        let baseline = crate::retired_count();
        let operation = control.take_operation().unwrap();
        assert_eq!(crate::retired_count(), baseline + 1);
        control.request_shutdown();
        assert!(!control.is_closed());
        let result = operation.run();
        assert!(result.runtime.host.is_none());
        assert!(!control.is_closed());
        assert!(!control.commit(result));
        assert!(control.is_closed());
        assert_eq!(crate::retired_count(), baseline);
    }

    #[test]
    fn old_generation_cannot_commit_into_a_replacement() {
        let mut old = control();
        let operation = old.take_operation().unwrap();
        drop(old);
        let completed = operation.run();
        let mut replacement = control();
        let pending = replacement.take_operation().unwrap();
        assert!(!replacement.commit(completed));
        drop(replacement);
        drop(pending.run());
    }
}

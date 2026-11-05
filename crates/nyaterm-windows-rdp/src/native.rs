use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Instant;

use nyaterm_remote_desktop::{RdpDisplayConfig, RdpSessionConfig};
use zeroize::Zeroizing;

use crate::cleanup::{Cleanup, CleanupAction};
use crate::{NativeRdpError, NativeRdpEvent, NativeRdpEventKind};

#[repr(C)]
struct Text {
    data: *const u16,
    len: u32,
}

impl Text {
    fn new(value: &[u16]) -> Self {
        Self {
            data: value.as_ptr(),
            len: value.len() as u32,
        }
    }
}

#[repr(C)]
struct Config {
    size: u32,
    version: u32,
    parent: usize,
    generation: u64,
    server: Text,
    username: Text,
    domain: Text,
    password: Text,
    port: u32,
    width: u32,
    height: u32,
    scale: u32,
    color_depth: u32,
    nla: u32,
    clipboard: u32,
    admin: u32,
    reconnect: u32,
    max_attempts: u32,
    performance: u32,
}

/// Physical client coordinates, already intersected with the visible panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct HostStatus {
    size: u32,
    pub connected: u32,
    pub drawing_ready: u32,
    pub callbacks: u32,
    pub width: u32,
    pub height: u32,
    pub color_depth: u32,
    pub cipher_strength: u32,
    pub surface_width: u32,
    pub surface_height: u32,
    pub surface_visible: u32,
}

#[derive(Default)]
#[repr(C)]
struct Event {
    generation: u64,
    kind: u32,
    code: i32,
    extended: u32,
}

#[repr(C)]
struct Capabilities {
    size: u32,
    flags: u32,
    version: [u16; 128],
}

/// Local control capabilities; server support is negotiated separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRdpCapabilities {
    pub display_update_interface: bool,
    pub initial_desktop_scale: bool,
    pub version: String,
}

unsafe extern "C" {
    fn nya_rdp_create(config: *const Config, owner: *mut *mut c_void, stage: *mut u32) -> i32;
    fn nya_rdp_start(owner: *mut c_void) -> i32;
    fn nya_rdp_viewport(owner: *mut c_void, rect: *const Viewport, visible: u32) -> i32;
    fn nya_rdp_resize(owner: *mut c_void, width: u32, height: u32, scale: u32) -> i32;
    fn nya_rdp_smart_sizing(owner: *mut c_void, enabled: u32) -> i32;
    fn nya_rdp_focus(owner: *mut c_void, child: u32) -> i32;
    fn nya_rdp_poll(owner: *mut c_void, event: *mut Event) -> i32;
    fn nya_rdp_status(owner: *mut c_void, status: *mut HostStatus) -> i32;
    fn nya_rdp_capabilities(owner: *mut c_void, capabilities: *mut Capabilities) -> i32;
    fn nya_rdp_request_close(owner: *mut c_void, wait: *mut u32) -> i32;
    fn nya_rdp_destroy(owner: *mut *mut c_void) -> i32;
}

fn result(operation: &'static str, hresult: i32) -> Result<(), NativeRdpError> {
    if hresult < 0 {
        Err(NativeRdpError { operation, hresult })
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum HostCreateError {
    Configuration(nyaterm_remote_desktop::RdpError),
    Native(NativeRdpError),
}

impl std::fmt::Display for HostCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(f),
            Self::Native(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for HostCreateError {}

/// Owns the system control on its creating STA. The `Rc` marker makes both
/// `Send` and `Sync` unavailable. Neither COM interfaces nor HWNDs escape.
pub struct NativeRdpHost {
    owner: NonNull<c_void>,
    last_viewport: Option<(Viewport, bool)>,
    close_wait: Option<bool>,
    close_started: Option<Instant>,
    _thread: PhantomData<Rc<()>>,
}

impl NativeRdpHost {
    pub fn capabilities(&mut self) -> Result<NativeRdpCapabilities, NativeRdpError> {
        let mut value = Capabilities {
            size: size_of::<Capabilities>() as u32,
            flags: 0,
            version: [0; 128],
        };
        result("capabilities", unsafe {
            nya_rdp_capabilities(self.owner.as_ptr(), &mut value)
        })?;
        let count = value
            .version
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(value.version.len());
        Ok(NativeRdpCapabilities {
            display_update_interface: value.flags & 1 != 0,
            initial_desktop_scale: value.flags & 2 != 0,
            version: String::from_utf16_lossy(&value.version[..count]),
        })
    }
    /// # Safety
    ///
    /// The parent must remain alive on this thread until this host and any
    /// retired hosts have been cleaned up. Call outside GPUI entity/App borrows:
    /// creation and every subsequent COM operation may pump window messages.
    pub unsafe fn create(
        parent: usize,
        generation: u64,
        config: &RdpSessionConfig,
    ) -> Result<Self, HostCreateError> {
        config
            .backend
            .validate_config(config)
            .map_err(HostCreateError::Configuration)?;
        if config.backend != nyaterm_remote_desktop::RdpBackend::Windows {
            return Err(HostCreateError::Native(NativeRdpError {
                operation: "backend",
                hresult: 0x80070057u32 as i32,
            }));
        }
        let server: Vec<u16> = config.host.encode_utf16().collect();
        let username: Vec<u16> = config.username.encode_utf16().collect();
        let domain: Vec<u16> = config.domain.encode_utf16().collect();
        let password = Zeroizing::new(
            config
                .password
                .as_ref()
                .map(|p| p.expose_secret())
                .unwrap_or("")
                .encode_utf16()
                .collect::<Vec<_>>(),
        );
        let ffi = Config {
            size: size_of::<Config>() as u32,
            version: 1,
            parent,
            generation,
            server: Text::new(&server),
            username: Text::new(&username),
            domain: Text::new(&domain),
            password: Text::new(&password),
            port: config.port.into(),
            width: config.display.width,
            height: config.display.height,
            scale: config.display.desktop_scale_factor.max(100),
            color_depth: config.display.color_depth.into(),
            nla: config.use_nla.into(),
            clipboard: 1,
            admin: config.administrative_session.into(),
            reconnect: config.reconnect.enabled.into(),
            max_attempts: config.reconnect.max_attempts,
            performance: performance_flags(&config.display),
        };
        let mut owner = std::ptr::null_mut();
        let mut stage = 0;
        let status = unsafe { nya_rdp_create(&ffi, &mut owner, &mut stage) };
        let operation = match stage {
            1 => "initialize UI STA",
            2 => "initialize ATL host",
            3 => "load system Client9",
            4 => "require IMsRdpClient9",
            5 => "attach system control",
            6 => "subscribe system events",
            7 => "apply authentication and resource policy",
            8 => "require IPersistStreamInit",
            9 => "IPersistStreamInit::InitNew",
            10 => "enable parent child clipping",
            100 => "Server",
            101 => "UserName",
            102 => "Domain",
            103 => "DesktopWidth",
            104 => "DesktopHeight",
            105 => "ColorDepth",
            106 => "AdvancedSettings9",
            107 => "RDPPort",
            108 => "EnableCredSspSupport",
            109 => "AuthenticationLevel",
            110 => "ConnectToAdministerServer",
            111 => "RedirectClipboard",
            112 => "RedirectDrives",
            113 => "RedirectPrinters",
            114 => "RedirectSmartCards",
            115 => "RedirectPorts",
            116 => "RedirectDevices",
            117 => "RedirectPOSDevices",
            118 => "IMsRdpClientNonScriptable3",
            119 => "RedirectDynamicDrives",
            120 => "RedirectDynamicDevices",
            121 => "AudioCaptureRedirectionMode",
            122 => "EnableAutoReconnect",
            123 => "MaxReconnectAttempts",
            124 => "PerformanceFlags",
            125 => "SmartSizing",
            130 => "SecuredSettings3",
            131 => "KeyboardHookMode",
            132 => "AudioRedirectionMode",
            133 => "IMsTscNonScriptable",
            134 => "ClearTextPassword",
            135 => "UIParentWindowHandle",
            _ => "validate native configuration",
        };
        if let Err(error) = result(operation, status) {
            if let Some(owner) = NonNull::new(owner) {
                retire(owner);
            }
            return Err(HostCreateError::Native(error));
        }
        let owner = NonNull::new(owner).ok_or(HostCreateError::Native(NativeRdpError {
            operation: "create returned no host",
            hresult: 0x80004003u32 as i32,
        }))?;
        Ok(Self {
            owner,
            last_viewport: None,
            close_wait: None,
            close_started: None,
            _thread: PhantomData,
        })
    }

    pub fn start(&mut self) -> Result<(), NativeRdpError> {
        result("connect", unsafe { nya_rdp_start(self.owner.as_ptr()) })
    }

    pub fn viewport(&mut self, rect: Viewport, visible: bool) -> Result<(), NativeRdpError> {
        if self.last_viewport == Some((rect, visible)) {
            return Ok(());
        }
        tracing::trace!(target: "nyaterm_windows_rdp::presentation", x = rect.x, y = rect.y, width = rect.width, height = rect.height, scale = rect.scale, visible, "applying native viewport");
        result("viewport", unsafe {
            nya_rdp_viewport(self.owner.as_ptr(), &rect, visible.into())
        })?;
        self.last_viewport = Some((rect, visible));
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32, scale: u32) -> Result<(), NativeRdpError> {
        tracing::trace!(target: "nyaterm_windows_rdp::presentation", width, height, scale, "updating remote display");
        result("resize", unsafe {
            nya_rdp_resize(self.owner.as_ptr(), width, height, scale)
        })
    }

    pub fn smart_sizing(&mut self, enabled: bool) -> Result<(), NativeRdpError> {
        result("SmartSizing", unsafe {
            nya_rdp_smart_sizing(self.owner.as_ptr(), enabled.into())
        })
    }

    pub fn focus(&mut self, remote: bool) -> Result<(), NativeRdpError> {
        tracing::trace!(target: "nyaterm_windows_rdp::presentation", remote, "requesting native focus");
        result("focus", unsafe {
            nya_rdp_focus(self.owner.as_ptr(), remote.into())
        })
    }

    pub fn poll(&mut self) -> Result<Option<NativeRdpEvent>, NativeRdpError> {
        let mut event = Event::default();
        let status = unsafe { nya_rdp_poll(self.owner.as_ptr(), &mut event) };
        result("poll", status)?;
        if status == 1 {
            return Ok(None);
        }
        let kind = match event.kind {
            1 => NativeRdpEventKind::Connected,
            2 => NativeRdpEventKind::LoginComplete,
            3 => NativeRdpEventKind::Disconnected {
                basic: event.code as u32,
                extended: event.extended,
            },
            4 => NativeRdpEventKind::Fatal { code: event.code },
            5 => NativeRdpEventKind::Reconnecting,
            6 => NativeRdpEventKind::CloseConfirmed,
            7 => NativeRdpEventKind::FocusReleased,
            8 => NativeRdpEventKind::Reconnected,
            9 => NativeRdpEventKind::Connecting,
            10 => NativeRdpEventKind::AuthenticationWarningDisplayed,
            11 => NativeRdpEventKind::AuthenticationWarningDismissed,
            12 => NativeRdpEventKind::LogonError { code: event.code },
            _ => {
                return Err(NativeRdpError {
                    operation: "unknown event",
                    hresult: 0x8007000Du32 as i32,
                });
            }
        };
        Ok(Some(NativeRdpEvent {
            generation: event.generation,
            kind,
        }))
    }

    pub fn status(&mut self) -> Result<HostStatus, NativeRdpError> {
        let mut status = HostStatus {
            size: size_of::<HostStatus>() as u32,
            ..HostStatus::default()
        };
        result("status", unsafe {
            nya_rdp_status(self.owner.as_ptr(), &mut status)
        })?;
        Ok(status)
    }

    /// True means wait for CloseConfirmed or the graceful-close budget.
    pub fn request_close(&mut self) -> Result<bool, NativeRdpError> {
        if let Some(wait) = self.close_wait {
            return Ok(wait);
        }
        self.close_started.get_or_insert_with(Instant::now);
        let mut wait = 0;
        result("request close", unsafe {
            nya_rdp_request_close(self.owner.as_ptr(), &mut wait)
        })?;
        self.close_wait = Some(wait != 0);
        Ok(wait != 0)
    }

    pub fn try_destroy(self) -> Result<(), (Self, NativeRdpError)> {
        let mut owner = self.owner.as_ptr();
        match result("destroy", unsafe { nya_rdp_destroy(&mut owner) }) {
            Ok(()) => {
                std::mem::forget(self);
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
}

impl Drop for NativeRdpHost {
    fn drop(&mut self) {
        // Entity release is not a safe place to pump COM. Defer all work until
        // the foreground maintenance tick has released its GPUI borrows.
        retire_with_close(self.owner, self.close_started, self.close_wait);
    }
}

struct Retired {
    owner: NonNull<c_void>,
    cleanup: Cleanup,
}

thread_local! { static RETIRED: RefCell<Vec<Retired>> = const { RefCell::new(Vec::new()) }; }
thread_local! { static CLEANING: Cell<usize> = const { Cell::new(0) }; }
thread_local! { static OPERATING: Cell<usize> = const { Cell::new(0) }; }

/// Keeps nested shutdown from releasing a parent while an extracted operation
/// still owns its host, including after its Entity/control has been dropped.
pub(crate) struct NativeOperationLease(Rc<Cell<bool>>);

impl NativeOperationLease {
    pub(crate) fn new(active: Rc<Cell<bool>>) -> Self {
        active.set(true);
        OPERATING.with(|count| count.set(count.get() + 1));
        Self(active)
    }
}

impl Drop for NativeOperationLease {
    fn drop(&mut self) {
        self.0.set(false);
        OPERATING.with(|count| count.set(count.get() - 1));
    }
}

fn retire(owner: NonNull<c_void>) {
    retire_with_close(owner, None, None);
}

fn retire_with_close(owner: NonNull<c_void>, started: Option<Instant>, wait: Option<bool>) {
    let mut cleanup = Cleanup::new(started.unwrap_or_else(Instant::now));
    if let Some(wait) = wait {
        cleanup.requested(wait);
    }
    RETIRED.with(|queue| queue.borrow_mut().push(Retired { owner, cleanup }));
}

/// Must run on the creating UI thread, outside GPUI borrows, also before exit.
/// A synchronously blocked COM call cannot be interrupted by this deadline.
pub fn maintain_retired(now: Instant) -> usize {
    let pending = RETIRED.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
    // COM may pump a nested shutdown task. Include stack-owned retirements in
    // the count so that task cannot destroy the parent window prematurely.
    struct Cleaning(usize);
    impl Drop for Cleaning {
        fn drop(&mut self) {
            CLEANING.with(|count| count.set(count.get() - self.0));
        }
    }
    CLEANING.with(|count| count.set(count.get() + pending.len()));
    let _cleaning = Cleaning(pending.len());
    let mut retry = Vec::new();
    let mut abandoned = 0;
    for mut host in pending {
        match host.cleanup.next(now) {
            CleanupAction::RequestClose => {
                let mut wait = 0;
                let status = unsafe { nya_rdp_request_close(host.owner.as_ptr(), &mut wait) };
                host.cleanup.requested(status < 0 || wait != 0);
            }
            CleanupAction::Wait => {}
            CleanupAction::Destroy => {
                let mut owner = host.owner.as_ptr();
                let status = unsafe { nya_rdp_destroy(&mut owner) };
                if status >= 0 {
                    continue;
                }
                host.cleanup.destroyed(false, now);
            }
            CleanupAction::Complete => continue,
            CleanupAction::Retain => {
                // Deliberately retain the allocation and callback context.
                abandoned += 1;
                continue;
            }
        }
        // Callbacks only queue. A late CloseConfirmed can be consumed here
        // without borrowing any GPUI object or executing a user callback.
        let mut event = Event::default();
        for _ in 0..256 {
            if unsafe { nya_rdp_poll(host.owner.as_ptr(), &mut event) } != 0 {
                break;
            }
            if event.kind == 6 {
                host.cleanup.confirmed();
            }
        }
        retry.push(host);
    }
    RETIRED.with(|queue| queue.borrow_mut().extend(retry));
    abandoned
}

pub fn retired_count() -> usize {
    RETIRED.with(|queue| queue.borrow().len())
        + CLEANING.with(Cell::get)
        + OPERATING.with(Cell::get)
}

fn performance_flags(display: &RdpDisplayConfig) -> u32 {
    u32::from(!display.display_wallpaper)
        | (u32::from(display.disable_full_window_drag) << 1)
        | (u32::from(display.disable_menu_animations) << 2)
        | (u32::from(!display.display_themes) << 3)
        | (u32::from(display.disable_cursor_shadow) << 5)
        | (u32::from(display.disable_cursor_blinking) << 6)
        | (u32::from(display.enable_font_smoothing) << 7)
        | (u32::from(display.enable_desktop_composition) << 8)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use nyaterm_remote_desktop::RdpSessionConfig;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GWL_STYLE, GetWindowLongPtrW, MSG,
        PM_REMOVE, PeekMessageW, TranslateMessage, WINDOW_EX_STYLE, WS_CLIPCHILDREN,
        WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    use crate::NativeRdpEventKind;
    use crate::native::{NativeRdpHost, Viewport, maintain_retired};

    unsafe extern "C" {
        fn nya_rdp_verify_connection_policy(owner: *mut std::ffi::c_void) -> i32;
    }

    // Match the application's single UI STA: independent test apartments must
    // not race system-control initialization against another apartment's teardown.
    // Hold this through the STA thread's join, including its OleUninitialize.
    static SYSTEM_CONTROL_TEST: Mutex<()> = Mutex::new(());

    struct Parent(HWND);

    impl Drop for Parent {
        fn drop(&mut self) {
            for _ in 0..3 {
                maintain_retired(Instant::now());
            }
            unsafe { DestroyWindow(self.0).unwrap() };
        }
    }

    #[test]
    fn system_control_initializes_with_clipboard_enabled_and_other_resources_disabled() {
        let _system_control = SYSTEM_CONTROL_TEST.lock().unwrap();
        // A dedicated thread supplies a fresh apartment; no GPUI or remote server.
        std::thread::spawn(|| unsafe {
            let parent = Parent(
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("NyaTerm offline test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    800,
                    600,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            );
            let mut hosts = Vec::new();
            for nla in [false, true] {
                let config: RdpSessionConfig = serde_json::from_value(serde_json::json!({
                    "backend":"windows", "windows_authentication":"system",
                    "name":"offline", "host":"example.invalid", "port":3389,
                    "username":"", "domain":"", "password":"offline-fixture",
                    "use_nla":nla, "administrative_session":nla, "certificate_policy":"strict",
                    "display":{"mode":"fixed","width":800,"height":600,"color_depth":32},
                    "clipboard":{"mode":"disabled"},
                    "reconnect":{"enabled":nla,"max_attempts":5}
                }))
                .unwrap();
                let mut host = NativeRdpHost::create(parent.0.0 as usize, 1, &config).unwrap();
                assert_ne!(
                    GetWindowLongPtrW(parent.0, GWL_STYLE) & WS_CLIPCHILDREN.0 as isize,
                    0
                );
                assert_eq!(nya_rdp_verify_connection_policy(host.owner.as_ptr()), 0);
                host.viewport(
                    Viewport {
                        x: 0,
                        y: 0,
                        width: 800,
                        height: 600,
                        scale: 100,
                    },
                    false,
                )
                .unwrap();
                let status = host.status().unwrap();
                assert_ne!(status.drawing_ready, 0);
                assert_eq!((status.surface_width, status.surface_height), (800, 600));
                assert_eq!(status.surface_visible, 0);
                hosts.push(host);
            }
            // Multiple application sessions share the UI STA. Releasing one
            // control must leave the other control and its apartment usable.
            while let Some(host) = hosts.pop() {
                assert_eq!(host.try_destroy().err().map(|(_, error)| error), None);
                for remaining in &mut hosts {
                    assert_eq!(
                        nya_rdp_verify_connection_policy(remaining.owner.as_ptr()),
                        0
                    );
                    assert_ne!(remaining.status().unwrap().drawing_ready, 0);
                }
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn system_control_delivers_connecting_and_disconnect_events_for_loopback_peer() {
        let _system_control = SYSTEM_CONTROL_TEST.lock().unwrap();
        // The peer only accepts and closes TCP: no credentials, certificate, or
        // remote server. This exercises the actual COM connection point and STA pump.
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        drop(stream);
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("loopback accept failed: {error}"),
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let events = std::thread::spawn(move || unsafe {
            let parent = Parent(
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("NyaTerm event test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    800,
                    600,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            );
            let config: RdpSessionConfig = serde_json::from_value(serde_json::json!({
                "backend":"windows", "windows_authentication":"system",
                "name":"loopback", "host":"127.0.0.1", "port":port,
                "username":"", "domain":"", "password":null,
                "use_nla":false, "certificate_policy":"strict",
                "display":{"mode":"fixed","width":800,"height":600,"color_depth":32},
                "clipboard":{"mode":"disabled"},
                "reconnect":{"enabled":false,"max_attempts":0}
            }))
            .unwrap();
            let mut host = NativeRdpHost::create(parent.0.0 as usize, 1, &config).unwrap();
            host.start().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut events = Vec::new();
            while Instant::now() < deadline {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                while let Some(event) = host.poll().unwrap() {
                    events.push(event.kind);
                }
                if events
                    .iter()
                    .any(|event| matches!(event, NativeRdpEventKind::Disconnected { .. }))
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(host.try_destroy().err().map(|(_, error)| error), None);
            events
        })
        .join()
        .unwrap();
        assert!(
            peer.join().unwrap(),
            "Connect never reached the loopback peer"
        );
        assert!(
            events.contains(&NativeRdpEventKind::Connecting),
            "events: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, NativeRdpEventKind::Disconnected { .. })),
            "events: {events:?}"
        );
        assert!(!events.contains(&NativeRdpEventKind::LoginComplete));
    }
}

use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::activex::{
    NativeRdpConnectionEvent, NativeRdpDisconnectInfo, NativeRdpRect, NativeRdpSessionDetails,
};
use nyaterm_remote_desktop::{RdpDisplayConfig, RdpError, RdpErrorKind, RdpSessionConfig};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{
    DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Ole::DISPID_PROPERTYPUT;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetCursorPos, GetWindowRect, GetWindowThreadProcessId,
    IsWindow, MoveWindow, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, WINDOW_EX_STYLE, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};
use windows::core::{BSTR, GUID, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, s, w};

use crate::display::DisplayUpdates;
use crate::events::{ControlEvent, EventSubscription};

#[windows::core::interface("c1e6743a-41c1-4a74-832a-0dd06c1c7a0e")]
unsafe trait IMsTscNonScriptable: IUnknown {
    fn set_clear_text_password(&self, password: *const u16) -> windows::core::HRESULT;
}

// This named-property interface is not exposed through IDispatch. Configure
// initial DPI before Connect so the server negotiates the intended desktop scale.
#[windows::core::interface("302d8188-0052-4807-806a-362b628f9ac5")]
unsafe trait IMsRdpExtendedSettings: IUnknown {
    fn put_property(&self, name: *const u16, value: *mut VARIANT) -> windows::core::HRESULT;
}

const MSTSC_PROG_ID: &str = "MsTscAx.MsTscAx";
static ATL_READY: OnceLock<bool> = OnceLock::new();

pub struct NativeRdpControl {
    parent: HWND,
    host: HWND,
    surface: HWND,
    control: IDispatch,
    events: EventSubscription,
    rect: NativeRdpRect,
    display_updates: DisplayUpdates,
    started_at: Option<Instant>,
    connected: bool,
    terminal: bool,
    desired_visible: bool,
    reconnect_deadline: Option<Instant>,
}

impl NativeRdpControl {
    pub fn create(
        parent: u64,
        parent_pid: u32,
        rect: NativeRdpRect,
        config: &RdpSessionConfig,
    ) -> Result<Self, String> {
        nyaterm_remote_desktop::activex::validate_start(config, rect).map_err(|e| e.to_string())?;
        let parent = HWND(parent as usize as *mut c_void);
        let mut owner_pid = 0;
        unsafe {
            GetWindowThreadProcessId(parent, Some(&mut owner_pid));
        }
        if !unsafe { IsWindow(Some(parent)).as_bool() } || owner_pid != parent_pid {
            return Err("Invalid ActiveX parent process or window".into());
        }
        let context = unsafe { windows::Win32::UI::HiDpi::GetWindowDpiAwarenessContext(parent) };
        if unsafe { windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(context) }.is_invalid()
        {
            return Err("Cannot match the parent DPI awareness".into());
        }
        ensure_atl()?;
        let host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                Some(parent),
                None,
                None,
                None,
            )
        }
        .map_err(|_| "Cannot create ActiveX child host".to_string())?;
        let result = (|| {
            let name = HSTRING::from(MSTSC_PROG_ID);
            let surface = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("AtlAxWin"),
                    PCWSTR(name.as_ptr()),
                    WS_CHILD
                        | WS_CLIPCHILDREN
                        | WS_CLIPSIBLINGS
                        | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE,
                    0,
                    0,
                    rect.width,
                    rect.height,
                    Some(host),
                    None,
                    None,
                    None,
                )
            }
            .map_err(|_| "Cannot create the Windows RDP control".to_string())?;
            let control = unsafe { control_from_window(surface) }?;
            let events = EventSubscription::new(&control)?;
            let native = Self {
                parent,
                host,
                surface,
                control,
                events,
                rect,
                display_updates: DisplayUpdates::new(&config.display),
                started_at: None,
                connected: false,
                terminal: false,
                desired_visible: false,
                reconnect_deadline: None,
            };
            native.configure(config)?;
            Ok(native)
        })();
        if result.is_err() {
            unsafe {
                let _ = DestroyWindow(host);
            }
        }
        result
    }

    fn configure(&self, config: &RdpSessionConfig) -> Result<(), String> {
        put_bstr(&self.control, "Server", &config.host)?;
        put_bstr(&self.control, "UserName", &config.username)?;
        put_bstr(&self.control, "Domain", &config.domain)?;
        put_i32(&self.control, "DesktopWidth", config.display.width as i32)?;
        put_i32(&self.control, "DesktopHeight", config.display.height as i32)?;
        put_i32(
            &self.control,
            "ColorDepth",
            config.display.color_depth as i32,
        )?;
        let advanced = get_dispatch(&self.control, "AdvancedSettings2")?;
        put_i32(&advanced, "RDPPort", config.port as i32)?;
        put_bool(&advanced, "EnableCredSspSupport", config.use_nla)?;
        put_bool(&advanced, "SmartSizing", true)?;
        if config.display.desktop_scale_factor != 0 {
            put_extended_property(
                &self.control,
                "DesktopScaleFactor",
                config.display.desktop_scale_factor,
            )?;
            put_extended_property(&self.control, "DeviceScaleFactor", 100)?;
        }
        // Windows system authentication is explicit and fails closed.
        put_i32(&advanced, "AuthenticationLevel", 1)?;
        put_bool(
            &advanced,
            "ConnectToServerConsole",
            config.administrative_session,
        )?;
        put_i32(
            &advanced,
            "PerformanceFlags",
            performance_flags(&config.display) as i32,
        )?;
        put_bool(
            &advanced,
            "RedirectClipboard",
            config.clipboard.mode == nyaterm_remote_desktop::RdpClipboardMode::TextAndFiles,
        )?;
        put_bool(
            &advanced,
            "EnableAutoReconnect",
            config.reconnect.enabled && config.reconnect.max_attempts > 0,
        )?;
        put_i32(
            &advanced,
            "MaxReconnectAttempts",
            config.reconnect.max_attempts as i32,
        )?;
        if let Some(password) = &config.password {
            let non_scriptable: IMsTscNonScriptable = self
                .control
                .cast()
                .map_err(|_| "ActiveX saved credentials are unavailable".to_string())?;
            let password = BSTR::from(password.expose_secret());
            unsafe { non_scriptable.set_clear_text_password(password.as_ptr()) }
                .ok()
                .map_err(|_| "Cannot set ActiveX credentials".to_string())?;
        }
        Ok(())
    }

    pub fn window_handle(&self) -> u64 {
        self.host.0 as usize as u64
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.started_at.is_some() || self.connected || self.terminal {
            return Err("ActiveX session already started".into());
        }
        self.started_at = Some(Instant::now());
        self.invoke("Connect")
    }

    fn invoke(&self, name: &str) -> Result<(), String> {
        unsafe {
            self.control.Invoke(
                get_disp_id(&self.control, name)?,
                &GUID::zeroed(),
                0,
                DISPATCH_METHOD,
                &DISPPARAMS::default(),
                None,
                None,
                None,
            )
        }
        .map_err(|_| format!("ActiveX {name} failed"))
    }

    pub fn viewport(&mut self, rect: NativeRdpRect, visible: bool) -> Result<(), String> {
        rect.validate().map_err(|e| e.to_string())?;
        if self.rect != rect {
            self.display_updates.viewport(rect, Instant::now());
            self.rect = rect;
            unsafe {
                MoveWindow(self.host, rect.x, rect.y, rect.width, rect.height, true)
                    .map_err(|_| "Cannot move ActiveX host".to_string())?;
                MoveWindow(self.surface, 0, 0, rect.width, rect.height, true)
                    .map_err(|_| "Cannot resize ActiveX control".to_string())?;
            }
        }
        self.desired_visible = visible;
        unsafe {
            let _ = ShowWindow(
                self.host,
                if visible && !self.terminal {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_HIDE
                },
            );
        }
        Ok(())
    }

    pub fn focus(&self) {
        if self.desired_visible && !self.terminal {
            unsafe {
                let _ = SetFocus(Some(self.surface));
            }
        }
    }

    pub fn pointer_inside(&self) -> bool {
        if !self.desired_visible {
            return false;
        }
        let mut point = POINT::default();
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe {
            GetCursorPos(&mut point).is_ok()
                && GetWindowRect(self.host, &mut rect).is_ok()
                && point.x >= rect.left
                && point.x < rect.right
                && point.y >= rect.top
                && point.y < rect.bottom
        }
    }

    pub fn details(&self) -> NativeRdpSessionDetails {
        NativeRdpSessionDetails {
            version: get_text(&self.control, "Version").ok(),
            cipher_strength: get_u32(&self.control, "CipherStrength").ok(),
            status_text: None,
            dynamic_resize_available: self.display_updates.available(),
            desktop_width: get_u32(&self.control, "DesktopWidth").ok(),
            desktop_height: get_u32(&self.control, "DesktopHeight").ok(),
            color_depth: get_u32(&self.control, "ColorDepth").ok(),
        }
    }

    fn disconnect_info(&self, reason: u32) -> NativeRdpDisconnectInfo {
        NativeRdpDisconnectInfo {
            basic_reason: Some(reason),
            extended_reason: get_u32(&self.control, "ExtendedDisconnectReason").ok(),
            description: None,
        }
    }

    pub fn tick(&mut self) -> Result<Vec<NativeRdpConnectionEvent>, String> {
        let mut output = Vec::new();
        for event in self.events.drain() {
            match event {
                ControlEvent::Connected if !self.terminal && !self.connected => {
                    self.connected = true;
                    self.started_at = None;
                    self.reconnect_deadline = None;
                    output.push(NativeRdpConnectionEvent::Connected);
                }
                ControlEvent::Disconnected(reason) if !self.terminal => {
                    self.terminal = true;
                    self.connected = false;
                    output.push(NativeRdpConnectionEvent::Disconnected(
                        self.disconnect_info(reason),
                    ));
                }
                ControlEvent::Fatal(code) if !self.terminal => {
                    self.terminal = true;
                    output.push(NativeRdpConnectionEvent::Failed(RdpError::new(
                        RdpErrorKind::Session,
                        format!("ActiveX fatal error {code}"),
                    )));
                }
                ControlEvent::Reconnecting if !self.terminal => {
                    self.connected = false;
                    self.reconnect_deadline
                        .get_or_insert(Instant::now() + Duration::from_secs(120));
                    output.push(NativeRdpConnectionEvent::Reconnecting);
                }
                _ => {}
            }
        }
        if !unsafe { IsWindow(Some(self.parent)).as_bool() } {
            return Err("ActiveX parent window was closed".into());
        }
        if !self.terminal
            && (self
                .started_at
                .is_some_and(|at| at.elapsed() > Duration::from_secs(30))
                || self
                    .reconnect_deadline
                    .is_some_and(|at| Instant::now() >= at))
        {
            self.terminal = true;
            output.push(NativeRdpConnectionEvent::Failed(RdpError::new(
                RdpErrorKind::Timeout,
                "ActiveX connection timed out",
            )));
        }
        // Connected is a short with three states, never a boolean. Events
        // report failures (including the first attempt); this read only checks
        // the control contract and never treats `2` (connecting) as connected.
        match get_u32(&self.control, "Connected")? {
            0..=2 => {}
            _ => return Err("Invalid ActiveX connection state".into()),
        }
        if self.terminal {
            unsafe {
                let _ = ShowWindow(self.host, SW_HIDE);
            }
        } else if self.connected {
            self.display_updates
                .apply_due(Instant::now(), |parameters| {
                    update_session_display_settings(
                        &self.control,
                        parameters.width,
                        parameters.height,
                        parameters.scale,
                    )
                });
        }
        Ok(output)
    }
}

impl Drop for NativeRdpControl {
    fn drop(&mut self) {
        // Unsubscribe before releasing the window/control; all work stays on STA.
        self.events.disconnect();
        unsafe {
            let _ = DestroyWindow(self.host);
        }
    }
}

fn ensure_atl() -> Result<(), String> {
    if *ATL_READY.get_or_init(|| {
        let Ok(module) = (unsafe { LoadLibraryW(w!("atl.dll")) }) else {
            return false;
        };
        let Some(init) = (unsafe { GetProcAddress(module, s!("AtlAxWinInit")) }) else {
            return false;
        };
        let init: unsafe extern "system" fn() -> i32 = unsafe { std::mem::transmute(init) };
        unsafe { init() != 0 }
    }) {
        Ok(())
    } else {
        Err("Windows ActiveX hosting (atl.dll) is unavailable".to_string())
    }
}

unsafe fn control_from_window(hwnd: HWND) -> Result<IDispatch, String> {
    let module = unsafe { LoadLibraryW(w!("atl.dll")) }
        .map_err(|error| format!("cannot load atl.dll: {error}"))?;
    let get_control = unsafe { GetProcAddress(module, s!("AtlAxGetControl")) }
        .ok_or_else(|| "AtlAxGetControl is unavailable".to_string())?;
    let get_control: unsafe extern "system" fn(HWND, *mut *mut c_void) -> i32 =
        unsafe { std::mem::transmute(get_control) };
    let mut unknown = std::ptr::null_mut();
    let result = unsafe { get_control(hwnd, &mut unknown) };
    if result < 0 || unknown.is_null() {
        return Err(format!(
            "cannot acquire the Windows RDP control (0x{result:08X})"
        ));
    }
    let unknown = unsafe { IUnknown::from_raw(unknown) };
    unknown
        .cast::<IDispatch>()
        .map_err(|error| format!("the Windows RDP control has no automation interface: {error}"))
}

fn performance_flags(display: &RdpDisplayConfig) -> u32 {
    let mut flags = 0;
    if !display.display_wallpaper {
        flags |= 0x0000_0001;
    }
    if display.disable_full_window_drag {
        flags |= 0x0000_0002;
    }
    if display.disable_menu_animations {
        flags |= 0x0000_0004;
    }
    if !display.display_themes {
        flags |= 0x0000_0008;
    }
    if display.disable_cursor_shadow {
        flags |= 0x0000_0020;
    }
    if display.disable_cursor_blinking {
        flags |= 0x0000_0040;
    }
    if display.enable_font_smoothing {
        flags |= 0x0000_0080;
    }
    if display.enable_desktop_composition {
        flags |= 0x0000_0100;
    }
    flags
}

fn update_session_display_settings(
    control: &IDispatch,
    width: u32,
    height: u32,
    scale: u32,
) -> Result<(), String> {
    let dispid = get_disp_id(control, "UpdateSessionDisplaySettings")?;
    let mut arguments = display_resize_arguments(width, height, scale);
    let params = DISPPARAMS {
        rgvarg: arguments.as_mut_ptr(),
        rgdispidNamedArgs: std::ptr::null_mut(),
        cArgs: arguments.len() as u32,
        cNamedArgs: 0,
    };
    unsafe {
        control
            .Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_METHOD,
                &params,
                None,
                None,
                None,
            )
            .map_err(|error| error.to_string())
    }
}

fn get_disp_id(control: &IDispatch, name: &str) -> Result<i32, String> {
    let wide = HSTRING::from(name);
    let names = [PCWSTR(wide.as_ptr())];
    let mut dispid = 0;
    unsafe {
        control
            .GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0, &mut dispid)
            .map_err(|error| format!("RDP control has no property {name}: {error}"))?;
    }
    Ok(dispid)
}

fn get_property(control: &IDispatch, name: &str) -> Result<VARIANT, String> {
    let dispid = get_disp_id(control, name)?;
    let params = DISPPARAMS::default();
    let mut result = VARIANT::default();
    unsafe {
        control
            .Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_PROPERTYGET,
                &params,
                Some(&mut result),
                None,
                None,
            )
            .map_err(|error| format!("cannot read RDP property {name}: {error}"))?;
    }
    Ok(result)
}

fn get_dispatch(control: &IDispatch, name: &str) -> Result<IDispatch, String> {
    let value = get_property(control, name)?;
    IDispatch::try_from(&value)
        .map_err(|error| format!("RDP property {name} is not an automation object: {error}"))
}

fn get_u32(control: &IDispatch, name: &str) -> Result<u32, String> {
    let value = get_property(control, name)?;
    i32::try_from(&value)
        .map(|value| value as u32)
        .map_err(|error| format!("RDP property {name} is not a number: {error}"))
}

fn get_text(control: &IDispatch, name: &str) -> Result<String, String> {
    let value = get_property(control, name)?;
    BSTR::try_from(&value)
        .map(|value| value.to_string())
        .map_err(|error| format!("RDP property {name} is not text: {error}"))
}

fn put_bstr(control: &IDispatch, name: &str, value: &str) -> Result<(), String> {
    put_property(control, name, VARIANT::from(BSTR::from(value)))
}

fn put_i32(control: &IDispatch, name: &str, value: i32) -> Result<(), String> {
    put_property(control, name, VARIANT::from(value))
}

fn put_bool(control: &IDispatch, name: &str, value: bool) -> Result<(), String> {
    put_property(control, name, VARIANT::from(value))
}

fn put_property(control: &IDispatch, name: &str, value: VARIANT) -> Result<(), String> {
    let dispid = get_disp_id(control, name)?;
    put_variant(control, dispid, value)
}

fn put_variant(control: &IDispatch, dispid: i32, value: VARIANT) -> Result<(), String> {
    let mut arguments = [value];
    let mut names = [DISPID_PROPERTYPUT];
    let params = DISPPARAMS {
        rgvarg: arguments.as_mut_ptr(),
        rgdispidNamedArgs: names.as_mut_ptr(),
        cArgs: 1,
        cNamedArgs: 1,
    };
    unsafe {
        control
            .Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_PROPERTYPUT,
                &params,
                None,
                None,
                None,
            )
            .map_err(|error| format!("cannot set RDP property {dispid}: {error}"))
    }
}

fn put_extended_property(control: &IDispatch, property: &str, value: u32) -> Result<(), String> {
    let settings: IMsRdpExtendedSettings = control
        .cast()
        .map_err(|error| format!("the Windows RDP control has no extended settings: {error}"))?;
    let name = BSTR::from(property);
    let mut value = VARIANT::from(value);
    unsafe { settings.put_property(name.as_ptr(), &mut value) }
        .ok()
        .map_err(|error| format!("cannot set RDP property {property}: {error}"))
}

fn display_resize_arguments(width: u32, height: u32, scale: u32) -> [VARIANT; 7] {
    // Automation arguments are reversed. Keep the negotiated desktop scale;
    // device scale 100 avoids applying the local DPI twice.
    [
        VARIANT::from(100u32),
        VARIANT::from(if scale == 0 { 100 } else { scale }),
        VARIANT::from(0u32),
        VARIANT::from(0u32),
        VARIANT::from(0u32),
        VARIANT::from(height),
        VARIANT::from(width),
    ]
}

#[cfg(test)]
mod tests {
    use super::display_resize_arguments;

    #[test]
    fn display_resizes_preserve_remote_dpi_and_dimensions() {
        let values =
            display_resize_arguments(1600, 900, 150).map(|value| u32::try_from(&value).unwrap());
        assert_eq!(values, [100, 150, 0, 0, 0, 900, 1600]);
        let unset = display_resize_arguments(1600, 900, 0);
        assert_eq!(u32::try_from(&unset[1]).unwrap(), 100);
    }
}

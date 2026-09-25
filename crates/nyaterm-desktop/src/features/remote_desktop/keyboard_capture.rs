use std::sync::Arc;

use nyaterm_remote_desktop::{RdpSessionManager, VncSessionManager};

#[cfg(target_os = "windows")]
mod platform {
    use std::collections::HashSet;
    use std::ptr::null_mut;
    use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
    use std::thread;

    use nyaterm_remote_desktop::{
        RdpInputEvent, RdpSessionManager, VncInputEvent, VncSessionManager,
    };
    use windows_sys::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, VK_LWIN, VK_RWIN,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW,
        GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, MSG,
        PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
        WM_SYSKEYUP,
    };

    static CAPTURE: OnceLock<Arc<CaptureState>> = OnceLock::new();
    static HOOK_THREAD: OnceLock<Mutex<Option<HookThread>>> = OnceLock::new();

    struct HookThread {
        thread_id: u32,
        worker: thread::JoinHandle<()>,
    }

    struct CaptureState {
        rdp_manager: Mutex<Weak<RdpSessionManager>>,
        vnc_manager: Mutex<Weak<VncSessionManager>>,
        target: Mutex<Option<CaptureTarget>>,
        win_key_down: Mutex<bool>,
        captured_keys: Mutex<HashSet<(u16, bool)>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CaptureTarget {
        session_id: String,
        is_vnc: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CapturedKeyEvent {
        scan_code: u16,
        extended: bool,
        vnc_keysym: Option<u32>,
        pressed: bool,
    }

    pub(super) fn set_keyboard_capture(
        rdp_manager: Arc<RdpSessionManager>,
        vnc_manager: Arc<VncSessionManager>,
        target: Option<(String, bool)>,
    ) {
        let target = target.map(|(session_id, is_vnc)| CaptureTarget { session_id, is_vnc });
        let state = CAPTURE.get_or_init(|| {
            Arc::new(CaptureState {
                rdp_manager: Mutex::new(Weak::new()),
                vnc_manager: Mutex::new(Weak::new()),
                target: Mutex::new(None),
                win_key_down: Mutex::new(false),
                captured_keys: Mutex::new(HashSet::new()),
            })
        });
        if let Ok(mut current_manager) = state.rdp_manager.lock() {
            *current_manager = Arc::downgrade(&rdp_manager);
        }
        if let Ok(mut current_manager) = state.vnc_manager.lock() {
            *current_manager = Arc::downgrade(&vnc_manager);
        }
        let previous = state.target.lock().ok().and_then(|mut current| {
            let previous = current.clone();
            *current = target.clone();
            previous
        });
        if previous != target {
            let had_pressed_keys = reset_pressed_state(state);
            if had_pressed_keys && let Some(previous) = previous {
                if previous.is_vnc {
                    let _ = vnc_manager
                        .send_input(&previous.session_id, vec![VncInputEvent::ReleaseAllInputs]);
                } else {
                    let _ = rdp_manager
                        .send_input(&previous.session_id, vec![RdpInputEvent::ReleaseAllInputs]);
                }
            }
        }
        if target.is_some() {
            ensure_hook_thread();
        } else {
            stop_hook_thread();
        }
    }

    pub(super) fn shutdown_keyboard_capture() {
        if let Some(state) = CAPTURE.get() {
            reset_pressed_state(state);
            if let Ok(mut target) = state.target.lock() {
                *target = None;
            }
        }
        stop_hook_thread();
    }

    fn ensure_hook_thread() {
        let runtime = HOOK_THREAD.get_or_init(|| Mutex::new(None));
        let Ok(mut runtime) = runtime.lock() else {
            return;
        };
        if runtime.is_some() {
            return;
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = match thread::Builder::new()
            .name("rdp-keyboard-capture".to_string())
            .spawn(move || hook_thread(ready_tx))
        {
            Ok(worker) => worker,
            Err(error) => {
                tracing::warn!(%error, "failed to start RDP keyboard capture thread");
                return;
            }
        };
        match ready_rx.recv() {
            Ok(Some(thread_id)) => {
                *runtime = Some(HookThread { thread_id, worker });
            }
            Ok(None) | Err(_) => {
                let _ = worker.join();
            }
        }
    }

    fn stop_hook_thread() {
        let Some(runtime) = HOOK_THREAD.get() else {
            return;
        };
        let hook = runtime.lock().ok().and_then(|mut runtime| runtime.take());
        let Some(hook) = hook else {
            return;
        };
        let posted = unsafe { PostThreadMessageW(hook.thread_id, WM_QUIT, 0, 0) };
        if posted == 0 {
            tracing::warn!("failed to post shutdown to RDP keyboard capture thread");
        }
        if hook.worker.join().is_err() {
            tracing::warn!("RDP keyboard capture thread panicked during shutdown");
        }
    }

    fn reset_pressed_state(state: &CaptureState) -> bool {
        let Ok(mut win_key_down) = state.win_key_down.lock() else {
            return false;
        };
        let Ok(mut captured_keys) = state.captured_keys.lock() else {
            return false;
        };
        let had_pressed_keys = *win_key_down || !captured_keys.is_empty();
        *win_key_down = false;
        captured_keys.clear();
        had_pressed_keys
    }

    fn hook_thread(ready_tx: mpsc::SyncSender<Option<u32>>) {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                null_mut::<std::ffi::c_void>() as HINSTANCE,
                0,
            )
        };
        if hook.is_null() {
            tracing::warn!("failed to install RDP keyboard capture hook");
            let _ = ready_tx.send(None);
            return;
        }
        let mut message = MSG::default();
        unsafe {
            PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        }
        let _ = ready_tx.send(Some(unsafe { GetCurrentThreadId() }));
        while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0
            && foreground_window_belongs_to_us()
            && let Some(state) = CAPTURE.get()
            && let Some(raw) = unsafe { (lparam as *const KBDLLHOOKSTRUCT).as_ref() }
            && let Some(event) =
                captured_key_event(wparam as u32, raw.vkCode, raw.scanCode, raw.flags)
            && update_capture_state(state, &event)
            && let Some(destination) = capture_target(state)
        {
            if destination.target.is_vnc {
                if let (Some(manager), Some(keysym)) = (destination.vnc_manager, event.vnc_keysym) {
                    let _ = manager.send_input(
                        &destination.target.session_id,
                        vec![VncInputEvent::Key {
                            keysym,
                            pressed: event.pressed,
                        }],
                    );
                }
            } else {
                let input = if event.pressed {
                    RdpInputEvent::KeyDown {
                        scan_code: event.scan_code,
                        extended: event.extended,
                        repeat: false,
                    }
                } else {
                    RdpInputEvent::KeyUp {
                        scan_code: event.scan_code,
                        extended: event.extended,
                        repeat: false,
                    }
                };
                if let Some(manager) = destination.rdp_manager {
                    let _ = manager.send_input(&destination.target.session_id, vec![input]);
                }
            }
            return 1;
        }
        unsafe {
            CallNextHookEx(
                null_mut::<std::ffi::c_void>() as HHOOK,
                code,
                wparam,
                lparam,
            )
        }
    }

    fn foreground_window_belongs_to_us() -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return false;
        }
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(foreground, &mut process_id) };
        process_id == unsafe { GetCurrentProcessId() }
    }

    fn captured_key_event(
        message: u32,
        vk_code: u32,
        raw_scan_code: u32,
        flags: u32,
    ) -> Option<CapturedKeyEvent> {
        let pressed = match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => return None,
        } && flags & LLKHF_UP == 0;
        let mapped = unsafe { MapVirtualKeyW(vk_code, MAPVK_VK_TO_VSC_EX) };
        let scan_code = if raw_scan_code == 0 {
            mapped
        } else {
            raw_scan_code
        } & 0xff;
        if scan_code == 0 {
            return None;
        }
        Some(CapturedKeyEvent {
            scan_code: scan_code as u16,
            extended: flags & LLKHF_EXTENDED != 0 || matches!(vk_code as u16, VK_LWIN | VK_RWIN),
            vnc_keysym: vnc_keysym_for_virtual_key(vk_code),
            pressed,
        })
    }

    fn vnc_keysym_for_virtual_key(vk_code: u32) -> Option<u32> {
        match vk_code as u16 {
            VK_LWIN => Some(0xffeb),
            VK_RWIN => Some(0xffec),
            0x08 => Some(0xff08),
            0x09 => Some(0xff09),
            0x0d => Some(0xff0d),
            0x1b => Some(0xff1b),
            0x20 => Some(u32::from(' ')),
            0x21 => Some(0xff55),
            0x22 => Some(0xff56),
            0x23 => Some(0xff57),
            0x24 => Some(0xff50),
            0x25 => Some(0xff51),
            0x26 => Some(0xff52),
            0x27 => Some(0xff53),
            0x28 => Some(0xff54),
            0x2d => Some(0xff63),
            0x2e => Some(0xffff),
            0x30..=0x39 => Some(vk_code),
            0x41..=0x5a => Some(vk_code + 0x20),
            0x70..=0x87 => Some(0xffbe + vk_code - 0x70),
            _ => None,
        }
    }

    fn update_capture_state(state: &CaptureState, event: &CapturedKeyEvent) -> bool {
        let Ok(mut win_key_down) = state.win_key_down.lock() else {
            return false;
        };
        let Ok(mut captured_keys) = state.captured_keys.lock() else {
            return false;
        };
        let is_win_key = event.extended && matches!(event.scan_code, 0x5b | 0x5c);
        if is_win_key {
            *win_key_down = event.pressed;
            return true;
        }
        let key = (event.scan_code, event.extended);
        if event.pressed && *win_key_down {
            captured_keys.insert(key);
            return true;
        }
        !event.pressed && captured_keys.remove(&key)
    }

    struct CaptureDestination {
        target: CaptureTarget,
        rdp_manager: Option<Arc<RdpSessionManager>>,
        vnc_manager: Option<Arc<VncSessionManager>>,
    }

    fn capture_target(state: &CaptureState) -> Option<CaptureDestination> {
        Some(CaptureDestination {
            target: state.target.lock().ok()?.clone()?,
            rdp_manager: state.rdp_manager.lock().ok()?.upgrade(),
            vnc_manager: state.vnc_manager.lock().ok()?.upgrade(),
        })
    }

    #[cfg(test)]
    mod tests {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LWIN;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            LLKHF_EXTENDED, LLKHF_UP, WM_KEYDOWN, WM_KEYUP,
        };

        use super::{CapturedKeyEvent, captured_key_event};

        #[test]
        fn windows_key_and_combo_scan_codes_are_preserved() {
            assert_eq!(
                captured_key_event(WM_KEYDOWN, u32::from(VK_LWIN), 0x5b, LLKHF_EXTENDED),
                Some(CapturedKeyEvent {
                    scan_code: 0x5b,
                    extended: true,
                    vnc_keysym: Some(0xffeb),
                    pressed: true,
                })
            );
            assert_eq!(
                captured_key_event(WM_KEYUP, u32::from(b'R'), 0x13, LLKHF_UP),
                Some(CapturedKeyEvent {
                    scan_code: 0x13,
                    extended: false,
                    vnc_keysym: Some(u32::from('r')),
                    pressed: false,
                })
            );
        }
    }
}

pub(super) fn set_keyboard_capture(
    rdp_manager: Arc<RdpSessionManager>,
    vnc_manager: Arc<VncSessionManager>,
    target: Option<(String, bool)>,
) {
    #[cfg(target_os = "windows")]
    platform::set_keyboard_capture(rdp_manager, vnc_manager, target);
    #[cfg(not(target_os = "windows"))]
    let _ = (rdp_manager, vnc_manager, target);
}

pub(super) fn shutdown_keyboard_capture() {
    #[cfg(target_os = "windows")]
    platform::shutdown_keyboard_capture();
}

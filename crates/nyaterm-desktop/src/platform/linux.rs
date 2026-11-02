use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::errors::ConnectionError;
use x11rb::protocol::xproto::{ConnectionExt, EventMask, UNMAP_NOTIFY_EVENT, UnmapNotifyEvent};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

pub(crate) fn set_window_class(
    window: &(impl HasDisplayHandle + HasWindowHandle),
    instance: &str,
    class: &str,
) {
    // Wayland keeps the lowercase desktop ID. X11's class matches the desktop
    // entry's StartupWMClass while its instance retains that same desktop ID.
    with_x11_window(window, |connection, window, _| {
        let value = format!("{instance}\0{class}\0");
        connection
            .change_property8(
                x11rb::protocol::xproto::PropMode::REPLACE,
                window,
                x11rb::protocol::xproto::AtomEnum::WM_CLASS,
                x11rb::protocol::xproto::AtomEnum::STRING,
                value.as_bytes(),
            )?
            .ignore_error();
        connection.flush()
    });
}

pub(crate) fn hide_to_tray(window: &(impl HasDisplayHandle + HasWindowHandle)) -> bool {
    with_x11_window(window, withdraw_window)
}

pub(crate) fn restore_from_tray(window: &(impl HasDisplayHandle + HasWindowHandle)) {
    // _NET_ACTIVE_WINDOW alone cannot restore a withdrawn window. Map it first,
    // on GPUI's connection, so its subsequent activation request stays ordered.
    with_x11_window(window, |connection, window, _| {
        connection.map_window(window)?.ignore_error();
        connection.flush()
    });
}

fn with_x11_window(
    window: &(impl HasDisplayHandle + HasWindowHandle),
    operation: impl FnOnce(&XCBConnection, u32, u32) -> Result<(), ConnectionError>,
) -> bool {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else {
        return false;
    };
    let (RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(handle)) =
        (display.as_raw(), handle.as_raw())
    else {
        // Native Wayland has no general window-withdrawal request. Preserve the
        // ordinary minimize fallback there, regardless of the desktop's name.
        return false;
    };
    let Some(pointer) = display.connection else {
        return false;
    };
    // GPUI owns the connection. This non-owning wrapper lives only while the
    // borrowed handles do; it never reads events or waits for server replies.
    let Ok(connection) =
        (unsafe { XCBConnection::from_raw_xcb_connection(pointer.as_ptr(), false) })
    else {
        return false;
    };
    let Some(screen) = usize::try_from(display.screen)
        .ok()
        .and_then(|screen| connection.setup().roots.get(screen))
    else {
        return false;
    };
    operation(&connection, handle.window.get(), screen.root).is_ok()
}

fn withdraw_window(
    connection: &XCBConnection,
    window: u32,
    root: u32,
) -> Result<(), ConnectionError> {
    connection.unmap_window(window)?.ignore_error();
    // ICCCM withdrawal also needs a synthetic UnmapNotify on the root. In
    // particular, an already-iconified window emits no real unmap event.
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            UnmapNotifyEvent {
                response_type: UNMAP_NOTIFY_EVENT,
                sequence: 0,
                event: root,
                window,
                from_configure: false,
            },
        )?
        .ignore_error();
    connection.flush()
}

#[cfg(test)]
mod tests {
    use super::{hide_to_tray, restore_from_tray, set_window_class};
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
        XcbDisplayHandle, XcbWindowHandle,
    };
    use std::num::NonZeroU32;
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, MapState,
        WindowClass,
    };
    use x11rb::xcb_ffi::XCBConnection;

    struct NativeWindow<'a> {
        connection: &'a XCBConnection,
        screen: usize,
        id: u32,
    }

    impl HasDisplayHandle for NativeWindow<'_> {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            let handle = XcbDisplayHandle::new(
                NonNull::new(self.connection.get_raw_xcb_connection()),
                self.screen as i32,
            );
            // The connection is borrowed for the entire native window lifetime.
            Ok(unsafe { DisplayHandle::borrow_raw(handle.into()) })
        }
    }

    impl HasWindowHandle for NativeWindow<'_> {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let handle = XcbWindowHandle::new(NonZeroU32::new(self.id).unwrap());
            // This window is destroyed only after its borrowed handle expires.
            Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
        }
    }

    impl Drop for NativeWindow<'_> {
        fn drop(&mut self) {
            self.connection
                .destroy_window(self.id)
                .unwrap()
                .check()
                .unwrap();
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "window manager did not settle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "requires an X11 display"]
    fn startup_class_matches_desktop_entry_without_changing_the_instance() {
        let (connection, screen_index) = XCBConnection::connect(None).unwrap();
        let screen = &connection.setup().roots[screen_index];
        let window = NativeWindow {
            connection: &connection,
            screen: screen_index,
            id: connection.generate_id().unwrap(),
        };
        connection
            .create_window(
                screen.root_depth,
                window.id,
                screen.root,
                0,
                0,
                32,
                32,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        for (instance, class) in [
            ("nyaterm", "NyaTerm"),
            ("nyaterm-preview", "NyaTerm Preview"),
        ] {
            set_window_class(&window, instance, class);
            let property = connection
                .get_property(
                    false,
                    window.id,
                    AtomEnum::WM_CLASS,
                    AtomEnum::STRING,
                    0,
                    1024,
                )
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(property.value, format!("{instance}\0{class}\0").as_bytes());
        }
    }

    #[test]
    #[ignore = "requires an X11 display with an EWMH window manager"]
    fn hiding_and_restoring_normal_and_iconified_windows_preserves_the_native_window() {
        let (connection, screen_index) = XCBConnection::connect(None).unwrap();
        let screen = &connection.setup().roots[screen_index];
        let window = NativeWindow {
            connection: &connection,
            screen: screen_index,
            id: connection.generate_id().unwrap(),
        };
        connection
            .create_window(
                screen.root_depth,
                window.id,
                screen.root,
                50,
                50,
                320,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        let clients_atom = connection
            .intern_atom(false, b"_NET_CLIENT_LIST")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let change_state_atom = connection
            .intern_atom(false, b"WM_CHANGE_STATE")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let is_managed = || {
            connection
                .get_property(
                    false,
                    screen.root,
                    clients_atom,
                    AtomEnum::WINDOW,
                    0,
                    u32::MAX,
                )
                .unwrap()
                .reply()
                .unwrap()
                .value32()
                .unwrap()
                .any(|id| id == window.id)
        };
        let is_viewable = || {
            connection
                .get_window_attributes(window.id)
                .unwrap()
                .reply()
                .unwrap()
                .map_state
                == MapState::VIEWABLE
        };

        restore_from_tray(&window);
        wait_until(|| is_viewable() && is_managed());
        for iconified in [false, true, false] {
            if iconified {
                connection
                    .send_event(
                        false,
                        screen.root,
                        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                        ClientMessageEvent::new(32, window.id, change_state_atom, [3, 0, 0, 0, 0]),
                    )
                    .unwrap()
                    .check()
                    .unwrap();
                wait_until(|| !is_viewable());
                assert!(
                    is_managed(),
                    "iconified windows still appear in the taskbar"
                );
            }
            assert!(hide_to_tray(&window));
            wait_until(|| !is_viewable() && !is_managed());
            restore_from_tray(&window);
            wait_until(|| is_viewable() && is_managed());
            let geometry = connection.get_geometry(window.id).unwrap().reply().unwrap();
            assert_eq!((geometry.width, geometry.height), (320, 240));
        }
        // The adapter's temporary wrappers must not disconnect GPUI's connection.
        connection.get_input_focus().unwrap().reply().unwrap();
    }
}

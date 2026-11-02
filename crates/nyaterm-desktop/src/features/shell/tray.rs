pub(crate) mod model;

use crate::features::NyaTermApp;
use crate::features::sync::CloudSyncLiveState;
use crate::models::NavItem;
use gpui::{Context, Window};
#[cfg(target_os = "linux")]
use std::time::Duration;
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

use model::{
    TrayAction, TrayEntry, TraySession, TraySnapshot, TraySyncState, TrayWindowSnapshot,
    escape_menu_text,
};

pub(crate) struct SystemTray {
    #[cfg(not(target_os = "linux"))]
    icon: TrayIcon,
    #[cfg(target_os = "linux")]
    updates: Option<std::sync::mpsc::SyncSender<TraySnapshot>>,
    #[cfg(target_os = "linux")]
    installed: std::sync::mpsc::Receiver<Result<TraySnapshot, String>>,
    #[cfg(target_os = "linux")]
    update_pending: bool,
    #[cfg(target_os = "linux")]
    worker: Option<std::thread::JoinHandle<()>>,
    // Only successfully installed menus are cached, so a failed rebuild is retried.
    snapshot: Option<TraySnapshot>,
    refresh_required: bool,
}

impl SystemTray {
    pub(crate) fn new(
        snapshot: TraySnapshot,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let icon = build_tray(&snapshot, pixels, width, height)?;
            Ok(Self {
                icon,
                snapshot: Some(snapshot),
                refresh_required: false,
            })
        }
        #[cfg(target_os = "linux")]
        {
            linux_tray(snapshot, pixels, width, height)
        }
    }

    pub(crate) fn available(&self) -> bool {
        #[cfg(target_os = "linux")]
        if self
            .worker
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
        {
            return false;
        }
        self.snapshot.is_some()
    }

    pub(crate) fn update(&mut self, snapshot: TraySnapshot) {
        #[cfg(target_os = "linux")]
        {
            while let Ok(result) = self.installed.try_recv() {
                self.update_pending = false;
                match result {
                    Ok(installed) => {
                        self.snapshot = Some(installed);
                        self.refresh_required = false;
                    }
                    Err(_) => tracing::warn!("could not install tray menu"),
                }
            }
            if self.update_pending || !self.available() {
                return;
            }
        }
        if self.snapshot.as_ref() == Some(&snapshot) && !self.refresh_required {
            return;
        }
        #[cfg(not(target_os = "linux"))]
        match menu(&snapshot) {
            Ok(menu) => {
                self.icon.set_menu(Some(Box::new(menu)));
                self.snapshot = Some(snapshot);
                self.refresh_required = false;
            }
            Err(_) => tracing::warn!("could not install tray menu"),
        }
        #[cfg(target_os = "linux")]
        if let Some(updates) = &self.updates {
            self.update_pending = updates.try_send(snapshot).is_ok();
        }
    }

    pub(crate) fn refresh_checkmark(&mut self) {
        // Native check items toggle themselves before emitting their event.
        // Reinstall the authoritative snapshot even if saving is rejected.
        self.refresh_required = true;
    }
}

fn native_entry(entry: &TrayEntry) -> Result<Box<dyn IsMenuItem>, String> {
    Ok(match entry {
        TrayEntry::Action {
            action,
            label,
            enabled,
        } => Box::new(MenuItem::with_id(
            action.menu_id(),
            escape_menu_text(label),
            *enabled,
            None,
        )),
        TrayEntry::Check {
            action,
            label,
            enabled,
            checked,
        } => Box::new(CheckMenuItem::with_id(
            action.menu_id(),
            escape_menu_text(label),
            *enabled,
            *checked,
            None,
        )),
        TrayEntry::Info { id, label } => Box::new(MenuItem::with_id(
            format!("nyaterm::tray::info::{id}"),
            escape_menu_text(label),
            false,
            None,
        )),
        TrayEntry::Separator => Box::new(PredefinedMenuItem::separator()),
        TrayEntry::Submenu { id, label, entries } => {
            let submenu = Submenu::with_id(
                format!("nyaterm::tray::submenu::{id}"),
                escape_menu_text(label),
                true,
            );
            for entry in entries {
                submenu
                    .append(native_entry(entry)?.as_ref())
                    .map_err(|error| error.to_string())?;
            }
            Box::new(submenu)
        }
    })
}

fn menu(snapshot: &TraySnapshot) -> Result<Menu, String> {
    let menu = Menu::new();
    for entry in &snapshot.entries {
        menu.append(native_entry(entry)?.as_ref())
            .map_err(|error| error.to_string())?;
    }
    Ok(menu)
}

fn build_tray(
    snapshot: &TraySnapshot,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<TrayIcon, String> {
    let icon =
        tray_icon::Icon::from_rgba(pixels, width, height).map_err(|error| error.to_string())?;
    let builder = TrayIconBuilder::new()
        .with_id(nyaterm_core::app_identity::AppFlavor::current().desktop_id())
        .with_tooltip(nyaterm_core::app_identity::AppFlavor::current().display_name())
        .with_icon(icon)
        .with_menu(Box::new(menu(snapshot)?))
        .with_menu_on_left_click(false);
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);
    builder.build().map_err(|error| error.to_string())
}

impl NyaTermApp {
    pub(crate) fn tray_window_snapshot(&self) -> TrayWindowSnapshot {
        let sync_state = if self.cloud_sync.job_running() {
            TraySyncState::Running
        } else if self.cloud_sync.conflict().is_some() {
            TraySyncState::Conflict
        } else {
            match self.cloud_sync.live_state() {
                CloudSyncLiveState::Running => TraySyncState::Running,
                CloudSyncLiveState::Success => TraySyncState::Success,
                CloudSyncLiveState::Failed => TraySyncState::Failed,
                CloudSyncLiveState::Idle => match self
                    .cloud_sync
                    .history()
                    .first()
                    .map(|entry| entry.status.as_str())
                {
                    Some("success") => TraySyncState::Success,
                    Some("failed") => TraySyncState::Failed,
                    Some("conflict") => TraySyncState::Conflict,
                    _ => TraySyncState::Idle,
                },
            }
        };
        TrayWindowSnapshot {
            sessions: self
                .session
                .ordered_sessions()
                .into_iter()
                .map(|session| TraySession {
                    name: self.session.display_name_by_info(&session),
                    id: session.id,
                    kind: session.kind,
                })
                .collect(),
            sync_state,
            sync_enabled: self.cloud_sync.settings().enabled,
            minimize_to_tray: self.settings.summary().minimize_to_tray,
            lock_available: self.settings.summary().enable_startup_lock
                || self.settings.summary().enable_idle_lock,
        }
    }

    pub(crate) fn owns_tray_session(&self, id: &str) -> bool {
        self.session.has_session(id)
    }

    pub(crate) fn tray_screen_locked(&self) -> bool {
        self.security.screen_locked()
    }

    pub(crate) fn handle_tray_action(
        &mut self,
        action: TrayAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A native menu can outlive the state it described. Enforce the lock again here.
        if self.security.screen_locked() && action.requires_unlock() {
            show_window(window, cx);
            return;
        }
        if action.shows_window() {
            show_window(window, cx);
        }
        match action {
            TrayAction::Show => {}
            TrayAction::Hide => {
                if !self.minimize_to_system_tray(window, cx) {
                    self.notify_operation(
                        "tray-unavailable",
                        nyaterm_ui::notification::NyaNotificationKind::Warning,
                        rust_i18n::t!("tray.unavailable").to_string(),
                        cx,
                    );
                    window.minimize_window();
                }
            }
            TrayAction::NewConnection => self.open_connection_editor(None, None, false, window, cx),
            TrayAction::SyncPush => self.prompt_provider_cloud_sync_push(window, cx),
            TrayAction::SyncPull => self.prompt_provider_cloud_sync_pull(window, cx),
            TrayAction::ActiveSessions => {
                self.ensure_panel_open(NavItem::ActiveSessions, cx);
                self.persist_ui_layout();
            }
            TrayAction::SyncHistory => {
                self.ensure_panel_open(NavItem::SyncBackupHistory, cx);
                self.persist_ui_layout();
                self.queue_cloud_sync_history_refresh(None, cx);
            }
            TrayAction::Settings => self.open_page(NavItem::Settings, cx),
            TrayAction::MinimizeMode => self.toggle_minimize_to_tray_from_tray(cx),
            TrayAction::Lock => self.lock_app(window, cx),
            TrayAction::Update => self.open_update_dialog(window, cx),
            TrayAction::FocusSession(id) => {
                if self.session.has_session(&id) {
                    self.activate_session_id(&id, cx);
                }
            }
            // Process-level actions are handled by DesktopController.
            TrayAction::NewWindow | TrayAction::Quit => {}
        }
        cx.notify();
    }

    pub(in crate::features) fn minimize_to_system_tray(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .desktop_controller
            .as_ref()
            .and_then(gpui::WeakEntity::upgrade)
            .is_some_and(|controller| controller.read(cx).tray_available())
        {
            return false;
        }
        #[cfg(windows)]
        {
            if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
                && let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw()
            {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                        handle.hwnd.get() as _,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                    );
                }
                return true;
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = window;
            // Preserve macOS application-hide semantics: all workspace windows
            // hide together and are restored when the application is activated.
            cx.hide();
            true
        }
        #[cfg(target_os = "linux")]
        {
            crate::platform::linux::hide_to_tray(window)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (window, cx);
            false
        }
    }
}

pub(crate) fn show_window(window: &mut Window, cx: &mut gpui::App) {
    #[cfg(target_os = "linux")]
    crate::platform::linux::restore_from_tray(window);

    #[cfg(windows)]
    if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
        && let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe { show_native_window(handle.hwnd.get() as _) };
        // Native activation must finish before a tray action opens a dialog.
        // GPUI's queued activation could otherwise run after the dialog opens.
        return;
    }
    cx.activate(true);
    window.activate_window();
}

#[cfg(windows)]
unsafe fn show_native_window(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };

    // SW_RESTORE also unmaximizes; only minimized windows need restoring.
    unsafe {
        let command = if IsIconic(hwnd) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(hwnd, command);
        // SW_SHOW leaves an already-visible window inactive. The tray menu's
        // hidden HWND may still be active, and GPUI uses GetActiveWindow as the
        // owner of a new dialog. Select the real owner synchronously.
        SetForegroundWindow(hwnd);
        SetActiveWindow(hwnd);
        SetFocus(hwnd);
    }
}

#[cfg(target_os = "linux")]
fn linux_tray(
    snapshot: TraySnapshot,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<SystemTray, String> {
    let (updates, rx) = std::sync::mpsc::sync_channel::<TraySnapshot>(1);
    let (installed_tx, installed) = std::sync::mpsc::channel();
    let worker = crate::thread_owner::spawn_joinable("nyaterm-tray", move || {
        let initialized = gtk::init()
            .map_err(|error| error.to_string())
            .and_then(|_| build_tray(&snapshot, pixels, width, height));
        let icon = match initialized {
            Ok(icon) => icon,
            Err(error) => {
                let _ = installed_tx.send(Err(error));
                return;
            }
        };
        let _ = installed_tx.send(Ok(snapshot));
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(snapshot) => {
                    let result = menu(&snapshot).map(|menu| {
                        icon.set_menu(Some(Box::new(menu)));
                        snapshot
                    });
                    let _ = installed_tx.send(result);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    })
    .map_err(|error| error.to_string())?;
    Ok(SystemTray {
        updates: Some(updates),
        installed,
        update_pending: true,
        worker: Some(worker),
        snapshot: None,
        refresh_required: false,
    })
}

#[cfg(target_os = "linux")]
impl Drop for SystemTray {
    fn drop(&mut self) {
        self.updates.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::uuid;

    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{NavItem, PanelOpenMode, PanelSide};

    #[cfg(windows)]
    #[test]
    fn showing_native_window_preserves_size_and_activates_before_opening_dialogs() {
        use super::show_native_window;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, SetActiveWindow};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, IsIconic, IsWindowVisible, IsZoomed, SW_HIDE,
            SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, ShowWindow, WS_OVERLAPPEDWINDOW,
        };

        struct NativeWindow(windows_sys::Win32::Foundation::HWND);
        impl Drop for NativeWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
            }
        }

        unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let create_window = || {
                NativeWindow(CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    320,
                    240,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ))
            };
            let window = create_window();
            let tray_menu_owner = create_window();
            assert!(!window.0.is_null(), "create native test window");
            assert!(
                !tray_menu_owner.0.is_null(),
                "create hidden tray menu owner"
            );

            ShowWindow(window.0, SW_MAXIMIZE);
            assert_ne!(IsZoomed(window.0), 0);
            for state in [None, Some(SW_HIDE), Some(SW_MINIMIZE)] {
                if let Some(state) = state {
                    ShowWindow(window.0, state);
                }
                SetActiveWindow(tray_menu_owner.0);
                assert_eq!(GetActiveWindow(), tray_menu_owner.0);
                show_native_window(window.0);
                assert_eq!(
                    GetActiveWindow(),
                    window.0,
                    "dialog must resolve the main window as its owner immediately"
                );
                assert_ne!(IsWindowVisible(window.0), 0);
                assert_eq!(IsIconic(window.0), 0);
                assert_ne!(IsZoomed(window.0), 0, "preserve maximized state");
            }

            ShowWindow(window.0, SW_RESTORE);
            for state in [None, Some(SW_HIDE), Some(SW_MINIMIZE)] {
                if let Some(state) = state {
                    ShowWindow(window.0, state);
                }
                SetActiveWindow(tray_menu_owner.0);
                assert_eq!(GetActiveWindow(), tray_menu_owner.0);
                show_native_window(window.0);
                assert_eq!(
                    GetActiveWindow(),
                    window.0,
                    "dialog must resolve the main window as its owner immediately"
                );
                assert_ne!(IsWindowVisible(window.0), 0);
                assert_eq!(IsIconic(window.0), 0);
                assert_eq!(IsZoomed(window.0), 0, "preserve normal state");
            }
        }
    }

    #[test]
    fn opening_tray_panels_repeatedly_keeps_them_visible_in_each_layout() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = std::env::temp_dir().join(format!("nyaterm-tray-panels-{}", uuid()));
        let app = app_with_visible_local_session(&mut cx, &root, "session");
        cx.update_entity(&app, |app, cx| {
            for multi_open in [false, true] {
                app.shell.panels.multi_open = multi_open;
                for item in [NavItem::ActiveSessions, NavItem::SyncBackupHistory] {
                    app.ensure_panel_open(item, cx);
                    app.ensure_panel_open(item, cx);
                    assert!(
                        app.current_left_panel() == Some(item)
                            || app.current_right_panel() == Some(item)
                    );
                }
            }
            app.shell.panels.set_open_mode(PanelOpenMode::Floating);
            for item in [NavItem::ActiveSessions, NavItem::SyncBackupHistory] {
                let side = app.panel_side_for_item(item).unwrap_or(PanelSide::Right);
                app.ensure_panel_open(item, cx);
                app.ensure_panel_open(item, cx);
                assert_eq!(app.shell.floating_panel(side), Some(item));
            }
        });
    }

    #[cfg(windows)]
    #[test]
    fn native_menu_contains_checked_items_submenus_and_disabled_status_rows() {
        use super::menu;
        use super::model::{TraySession, TraySnapshot, TraySyncState};
        use nyaterm_transport::SessionKind;

        let snapshot = TraySnapshot::new(
            vec![TraySession {
                id: "id&1".into(),
                name: "R&D".into(),
                kind: SessionKind::Rdp,
            }],
            TraySyncState::Running,
            true,
            true,
            false,
        );
        let menu = menu(&snapshot).expect("native Windows menu");
        let items = menu.items();
        assert!(
            items[9]
                .as_check_menuitem()
                .expect("native check item")
                .is_checked()
        );
        let sessions = items[5].as_submenu().expect("sessions submenu").items();
        let session = sessions[0].as_menuitem().expect("session shortcut");
        assert_eq!(session.text(), "R&&D · RDP");
        assert!(session.id().as_ref().ends_with("session:id&1"));
        let sync = items[6].as_submenu().expect("sync submenu").items();
        assert!(!sync[0].as_menuitem().expect("status row").is_enabled());
        assert!(!sync[2].as_menuitem().expect("push").is_enabled());
        assert!(!sync[3].as_menuitem().expect("pull").is_enabled());
    }
}

use crate::features::NyaTermApp;
use gpui::{AppContext as _, Context, Window};
use std::time::Duration;
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Clone, PartialEq, Eq)]
struct TraySnapshot {
    entries: Vec<(String, String)>,
}

pub(in crate::features) struct SystemTray {
    #[cfg(not(target_os = "linux"))]
    icon: TrayIcon,
    #[cfg(target_os = "linux")]
    updates: Option<std::sync::mpsc::Sender<TraySnapshot>>,
    #[cfg(target_os = "linux")]
    worker: Option<std::thread::JoinHandle<()>>,
    snapshot: TraySnapshot,
}

fn menu(snapshot: &TraySnapshot) -> Result<Menu, String> {
    let menu = Menu::new();
    for (id, label) in &snapshot.entries {
        menu.append(&MenuItem::with_id(id, label, true, None))
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
    TrayIconBuilder::new()
        .with_id("nyaterm")
        .with_tooltip("NyaTerm")
        .with_icon(icon)
        .with_menu(Box::new(menu(snapshot)?))
        .build()
        .map_err(|error| error.to_string())
}

impl NyaTermApp {
    fn tray_snapshot(&self) -> TraySnapshot {
        let mut entries = vec![
            ("show".into(), rust_i18n::t!("tray.show").to_string()),
            (
                "new".into(),
                rust_i18n::t!("tray.newConnection").to_string(),
            ),
            ("push".into(), rust_i18n::t!("tray.push").to_string()),
            ("pull".into(), rust_i18n::t!("tray.pull").to_string()),
            (
                "minimize-mode".into(),
                format!(
                    "{} {}",
                    if self.settings.summary().minimize_to_tray {
                        "✓"
                    } else {
                        "○"
                    },
                    rust_i18n::t!("tray.minimizeToTray")
                ),
            ),
            ("lock".into(), rust_i18n::t!("tray.lock").to_string()),
            ("update".into(), rust_i18n::t!("tray.update").to_string()),
        ];
        for session in self.session.ordered_sessions().into_iter().take(8) {
            entries.push((format!("session:{}", session.id), session.name));
        }
        entries.push(("quit".into(), rust_i18n::t!("tray.quit").to_string()));
        TraySnapshot { entries }
    }

    pub(crate) fn start_system_tray(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_spawn(async {
                    image::load_from_memory(include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../nyaterm-app/resources/icons/32x32.png"
                    )))
                    .map(|image| {
                        let image = image.to_rgba8();
                        (image.width(), image.height(), image.into_raw())
                    })
                })
                .await;
            let Ok((width, height, pixels)) = image else {
                return;
            };
            let initialized = this
                .update(cx, |app, _| {
                    let snapshot = app.tray_snapshot();
                    #[cfg(not(target_os = "linux"))]
                    let tray = build_tray(&snapshot, pixels, width, height)
                        .map(|icon| SystemTray { icon, snapshot });
                    #[cfg(target_os = "linux")]
                    let tray = linux_tray(snapshot, pixels, width, height);
                    app.shell.system_tray = tray.ok();
                    app.shell.system_tray.is_some()
                })
                .unwrap_or(false);
            if !initialized {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this
                    .update(cx, |app, cx| {
                        let snapshot = app.tray_snapshot();
                        if let Some(tray) = app.shell.system_tray.as_mut()
                            && tray.snapshot != snapshot
                        {
                            #[cfg(not(target_os = "linux"))]
                            if let Ok(menu) = menu(&snapshot) {
                                tray.icon.set_menu(Some(Box::new(menu)));
                            }
                            #[cfg(target_os = "linux")]
                            if let Some(updates) = &tray.updates {
                                let _ = updates.send(snapshot.clone());
                            }
                            tray.snapshot = snapshot;
                        }
                        while let Ok(event) = MenuEvent::receiver().try_recv() {
                            app.handle_tray_action(event.id.as_ref().to_owned(), cx);
                        }
                        while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
                            if matches!(event, tray_icon::TrayIconEvent::DoubleClick { .. }) {
                                app.handle_tray_action("show".into(), cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_tray_action(&mut self, action: String, cx: &mut Context<Self>) {
        let Some(window) = self.shell.main_window() else {
            return;
        };
        let app = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    show_window(window, cx);
                    match action.as_str() {
                        "show" => {}
                        "new" => app.open_connection_editor(None, None, false, window, cx),
                        "push" => app.prompt_provider_cloud_sync_push(window, cx),
                        "pull" => app.prompt_provider_cloud_sync_pull(window, cx),
                        "minimize-mode" => app.toggle_minimize_to_tray(cx),
                        "lock" => app.lock_app(window, cx),
                        "update" => app.open_update_dialog(window, cx),
                        "quit" => app.handle_window_close_request(window, cx),
                        _ => {
                            if let Some(id) = action.strip_prefix("session:")
                                && app.session.has_session(id)
                            {
                                app.activate_session_id(id, cx);
                            }
                        }
                    }
                    cx.notify();
                });
            });
        });
    }

    pub(in crate::features) fn minimize_to_system_tray(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.shell.system_tray.is_none() {
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
            cx.hide();
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (window, cx);
            false
        }
    }
}

fn show_window(window: &mut Window, cx: &mut gpui::App) {
    #[cfg(windows)]
    if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
        && let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                handle.hwnd.get() as _,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE,
            );
        }
    }
    cx.activate(true);
    window.activate_window();
}

#[cfg(target_os = "linux")]
fn linux_tray(
    snapshot: TraySnapshot,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<SystemTray, String> {
    let (updates, rx) = std::sync::mpsc::channel::<TraySnapshot>();
    let initial = snapshot.clone();
    let worker = crate::thread_owner::spawn_joinable("nyaterm-tray", move || {
        if gtk::init().is_err() {
            return;
        }
        let Ok(icon) = build_tray(&initial, pixels, width, height) else {
            return;
        };
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(snapshot) => {
                    if let Ok(menu) = menu(&snapshot) {
                        icon.set_menu(Some(Box::new(menu)));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    })
    .map_err(|error| error.to_string())?;
    Ok(SystemTray {
        updates: Some(updates),
        worker: Some(worker),
        snapshot,
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

use super::DesktopController;
use crate::features::shell::tray::SystemTray;
use crate::features::shell::tray::model::TrayAction;
use crate::features::shell::tray::model::TraySnapshot;
use crate::features::shell::tray::model::TraySyncState;
use crate::features::shell::tray::show_window as show_tray_window;
use gpui::AppContext as _;
use gpui::Context;
use nyaterm_core::OpenWorkspaceRequest;
use nyaterm_core::WorkspaceId;
use std::time::Duration;

pub(super) fn tray_click_shows_window(event: &tray_icon::TrayIconEvent) -> bool {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        }
    )
}

impl DesktopController {
    pub(super) fn start_tray(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_spawn(async {
                    #[cfg(target_os = "macos")]
                    let icon_bytes = include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../nyaterm-app/resources/icons/tray-template.png"
                    ));
                    #[cfg(not(target_os = "macos"))]
                    let icon_bytes = include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../nyaterm-app/resources/icons/32x32.png"
                    ));
                    image::load_from_memory(icon_bytes).map(|image| {
                        let image = image.to_rgba8();
                        (image.width(), image.height(), image.into_raw())
                    })
                })
                .await;
            let Ok((width, height, pixels)) = image else {
                return;
            };
            let initialized = this
                .update(cx, |controller, cx| {
                    let snapshot = controller.tray_snapshot(cx);
                    controller.tray = SystemTray::new(snapshot, pixels, width, height).ok();
                    controller.tray.is_some()
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
                    .update(cx, |controller, cx| controller.poll_tray(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn tray_apps(
        &self,
        cx: &gpui::App,
    ) -> Vec<(WorkspaceId, gpui::Entity<crate::features::NyaTermApp>)> {
        let mut apps: Vec<_> = self
            .windows
            .iter()
            .filter_map(|(id, entry)| {
                if self.closing_workspaces.contains(id) {
                    return None;
                }
                entry
                    .shell
                    .upgrade()
                    .and_then(|shell| shell.read(cx).app.clone())
                    .map(|app| (*id, app))
            })
            .collect();
        apps.sort_by_key(|(id, _)| {
            (
                self.device_windows
                    .window_order
                    .iter()
                    .position(|candidate| candidate == id)
                    .unwrap_or(usize::MAX),
                id.0,
            )
        });
        apps
    }

    pub(super) fn recent_tray_workspace(&self, cx: &gpui::App) -> Option<WorkspaceId> {
        let apps = self.tray_apps(cx);
        self.most_recent_workspace_id
            .filter(|id| apps.iter().any(|(candidate, _)| candidate == id))
            .or_else(|| apps.first().map(|(id, _)| *id))
    }

    pub(super) fn tray_action_workspace(
        &self,
        action: &TrayAction,
        cx: &gpui::App,
    ) -> Option<WorkspaceId> {
        match action {
            TrayAction::FocusSession(session_id) => self
                .tray_apps(cx)
                .iter()
                .find(|(_, app)| app.read(cx).owns_tray_session(session_id))
                .map(|(id, _)| *id),
            _ => self.recent_tray_workspace(cx),
        }
    }

    pub(super) fn tray_snapshot(&self, cx: &gpui::App) -> TraySnapshot {
        let apps = self.tray_apps(cx);
        let Some(target_id) = self.recent_tray_workspace(cx) else {
            return TraySnapshot::empty();
        };
        let mut snapshots: Vec<_> = apps
            .iter()
            .map(|(id, app)| (*id, app.read(cx).tray_window_snapshot()))
            .collect();
        let target = &snapshots
            .iter()
            .find(|(id, _)| *id == target_id)
            .expect("ready tray target")
            .1;
        let (enabled, minimize, lock_available) = self
            .process_state
            .as_ref()
            .map(|state| {
                let snapshot = state.read(cx).snapshot();
                (
                    snapshot.cloud_sync_settings.enabled,
                    snapshot.settings.minimize_to_tray,
                    snapshot.settings.enable_startup_lock || snapshot.settings.enable_idle_lock,
                )
            })
            .unwrap_or((
                target.sync_enabled,
                target.minimize_to_tray,
                target.lock_available,
            ));
        let sync = TraySyncState::aggregate(
            enabled,
            self.auto_sync.running(),
            snapshots.iter().map(|(_, snapshot)| snapshot.sync_state),
            target.sync_state,
        );
        let locked = self.screen_locked
            || apps
                .iter()
                .any(|(_, app)| app.read(cx).tray_screen_locked());
        let sessions = snapshots
            .iter_mut()
            .flat_map(|(_, snapshot)| std::mem::take(&mut snapshot.sessions))
            .collect();
        TraySnapshot::new(sessions, sync, minimize, lock_available, locked)
    }

    pub(super) fn poll_tray(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.tray_snapshot(cx);
        if let Some(tray) = self.tray.as_mut() {
            tray.update(snapshot);
        }
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if let Some(action) = TrayAction::from_menu_id(event.id.as_ref()) {
                if action == TrayAction::MinimizeMode
                    && let Some(tray) = self.tray.as_mut()
                {
                    tray.refresh_checkmark();
                }
                self.handle_tray_action(action, cx);
            }
        }
        while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if tray_click_shows_window(&event) {
                self.handle_tray_action(TrayAction::Show, cx);
            }
        }
    }

    pub(super) fn show_recent_tray_window(&mut self, cx: &mut Context<Self>) {
        let id = self
            .most_recent_workspace_id
            .filter(|id| self.windows.contains_key(id))
            .or_else(|| {
                self.device_windows
                    .window_order
                    .iter()
                    .copied()
                    .find(|id| self.windows.contains_key(id))
            });
        if let Some(entry) = id.and_then(|id| self.windows.get(&id)) {
            let _ = entry
                .handle
                .update(cx, |_, window, cx| show_tray_window(window, cx));
        } else if let Err(error) = self.open_workspace(OpenWorkspaceRequest::default(), cx) {
            tracing::error!(%error, "could not show NyaTerm window");
        }
    }

    pub(super) fn handle_tray_action(&mut self, action: TrayAction, cx: &mut Context<Self>) {
        if self.process_quitting {
            return;
        }
        let locked = self.screen_locked
            || self
                .tray_apps(cx)
                .iter()
                .any(|(_, app)| app.read(cx).tray_screen_locked());
        if locked && action.requires_unlock() {
            self.show_recent_tray_window(cx);
            return;
        }
        match action {
            TrayAction::Show => self.show_recent_tray_window(cx),
            TrayAction::NewWindow => {
                if let Err(error) = self.open_workspace(OpenWorkspaceRequest::default(), cx) {
                    tracing::error!(%error, "could not open NyaTerm window");
                }
            }
            TrayAction::Quit => self.request_quit(cx),
            _ => {
                let controller = cx.weak_entity();
                // Resolve ownership after deferring, then release the controller before
                // entering a feature that may call back into it (settings, lock, sync).
                cx.defer(move |cx| {
                    let Some(controller) = controller.upgrade() else {
                        return;
                    };
                    let (target, action) = {
                        let state = controller.read(cx);
                        if state.process_quitting {
                            return;
                        }
                        let apps = state.tray_apps(cx);
                        let locked = state.screen_locked
                            || apps
                                .iter()
                                .any(|(_, app)| app.read(cx).tray_screen_locked());
                        let action = if locked && action.requires_unlock() {
                            TrayAction::Show
                        } else {
                            action
                        };
                        if matches!(
                            action,
                            TrayAction::SyncPush | TrayAction::SyncPull | TrayAction::Lock
                        ) && !state.tray_snapshot(cx).action_enabled(&action)
                        {
                            return;
                        }
                        let id = state.tray_action_workspace(&action, cx);
                        let target = id.and_then(|id| {
                            let entry = state.windows.get(&id)?;
                            let app = apps
                                .iter()
                                .find(|(candidate, _)| *candidate == id)?
                                .1
                                .clone();
                            Some((entry.handle, app))
                        });
                        (target, action)
                    };
                    if let Some((handle, app)) = target {
                        let _ = handle.update(cx, |_, window, cx| {
                            app.update(cx, |app, cx| app.handle_tray_action(action, window, cx))
                        });
                    }
                });
            }
        }
    }

    pub fn tray_available(&self) -> bool {
        self.tray.as_ref().is_some_and(SystemTray::available)
    }
}

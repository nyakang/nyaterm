use super::{AndroidView, DeviceLease, ScreenState};
use crate::features::file_manager::{FileManagerSeed, FileManagerWorkspace, PaneEndpoint, PaneId};
use crate::features::{
    NyaTermApp, runtime_jobs::await_blocking_job, session::SavedConnectionStartOptions,
};
use crate::models::session::AndroidSessionConfig;
use crate::models::{
    NavItem, SessionLaunchConfig, SessionRuntimeMetadata, WorkspaceSplitDirection,
};
use gpui::{AppContext, Context, Window};
use nyaterm_android::adb::client::AdbClient;
use nyaterm_android::screen::ScreenHandle;
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection, uuid};
use nyaterm_files::adb::AdbFileService;
use nyaterm_files::sftp::RemoteFilePath;
use std::sync::Arc;

impl NyaTermApp {
    pub(in crate::features) fn start_android_connection(
        &mut self,
        connection: SavedConnection,
        mut options: SavedConnectionStartOptions,
        kind: Option<AndroidViewKind>,
        cx: &mut Context<Self>,
    ) {
        self.ensure_android_clock(cx);
        let ConnectionType::Adb {
            device,
            directory,
            screen,
        } = connection.config.clone()
        else {
            return;
        };
        let key = connection.id.clone();
        if options.reconnect_session_id.is_none()
            && let Some(lease) = self.android.devices.get(&key)
            && lease.files.control.check_cancelled().is_ok()
        {
            let config = lease.config.clone();
            if let Some(kind) = kind {
                self.open_android_view(config, kind, options, cx);
            } else {
                let files = self.open_android_view(
                    config.clone(),
                    AndroidViewKind::Files,
                    SavedConnectionStartOptions::default(),
                    cx,
                );
                if options.workspace_split.is_none() {
                    options.workspace_split =
                        files.map(|id| (WorkspaceSplitDirection::Horizontal, id));
                }
                self.open_android_view(config, AndroidViewKind::Shell, options, cx);
            }
            return;
        }
        if self.android.pending.contains_key(&key) {
            return;
        }
        self.android.generation = self.android.generation.wrapping_add(1);
        let generation = self.android.generation;
        self.android.pending.insert(key.clone(), generation);
        let custom = self.settings.summary().android_adb_path.clone();
        let tracker = self.android.tracker.clone();
        let name = connection.name.clone();
        let task_key = key.clone();
        let task = self.blocking_jobs.submit_task("android-bind", move |_| {
            let client =
                AdbClient::local((!custom.is_empty()).then(|| std::path::Path::new(&custom)))
                    .map_err(|e| e.to_string())?;
            tracker.ensure(&client).map_err(|e| e.to_string())?;
            let binding = client
                .bind(&device, generation)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(AndroidSessionConfig {
                name,
                device_key: task_key,
                device,
                directory,
                screen,
                binding,
                client,
                cols: 80,
                rows: 24,
            })
        });
        self.shell
            .set_status(format!("Connecting Android · {}", connection.name));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|v| v);
            let _ = this.update(cx, |app, cx| {
                if app.android.pending.get(&key) != Some(&generation) {
                    return;
                }
                app.android.pending.remove(&key);
                match result {
                    Ok(config) => {
                        if let Some(old) = app.android.devices.remove(&key) {
                            old.files.cancel();
                        }
                        let existing: Vec<String> = app.android.views.iter().filter(|(_,view)| view.device_key == key).map(|(id,_)| id.clone()).collect();
                        for id in existing {
                            app.android.stop_screen(&id);
                            let _ = app.session.manager().close_detached(&id);
                            if let Some(metadata) = app.session.metadata_mut(&id) {
                                metadata.disconnected = true;
                                match &mut metadata.launch_config {
                                    SessionLaunchConfig::AdbShell(current) | SessionLaunchConfig::AdbFiles(current) | SessionLaunchConfig::AdbScreen(current) => *current = config.clone(),
                                    _ => {},
                                }
                            }
                            if let Some(workspace) = app.file_manager.workspace(&format!("sftp-{id}")) {
                                workspace.update(cx, |workspace,cx| {
                                    workspace.left.generation = workspace.left.generation.wrapping_add(1);
                                    workspace.left.loading = false; workspace.left.disconnected = true;
                                    cx.notify();
                                });
                            }
                        }
                        app.android.devices.insert(
                            key.clone(),
                            DeviceLease {
                                files: Arc::new(AdbFileService::new(config.binding.clone())),
                                owners: Arc::new(()),
                                config: config.clone(),
                                observed: app.android.tracker.snapshot().devices.iter().any(|device| device.serial == config.binding.serial && device.status == nyaterm_android::adb::client::DeviceStatus::Online),
                            },
                        );
                        // Old cleanup belongs to this device only; retry against its new transport.
                        let tickets = app.android.cleanup.clone();
                        let binding = config.binding.clone();
                        let _ = app.blocking_jobs.submit_detached(
                            "android-deferred-cleanup",
                            move |_| {
                                for ticket in tickets {
                                    let cleanup =
                                        ticket.lock().unwrap_or_else(|p| p.into_inner()).take();
                                    if let Some(mut cleanup) = cleanup {
                                        if cleanup.binding.serial != binding.serial {
                                            *ticket.lock().unwrap_or_else(|p| p.into_inner()) =
                                                Some(cleanup);
                                            continue;
                                        }
                                        cleanup.binding = binding.clone();
                                        if cleanup.retry().is_err() {
                                            *ticket.lock().unwrap_or_else(|p| p.into_inner()) =
                                                Some(cleanup);
                                        }
                                    }
                                }
                            },
                        );
                        if let Some(kind) = kind {
                            app.open_android_view(config, kind, options, cx);
                        } else {
                            let files = app.open_android_view(
                                config.clone(),
                                AndroidViewKind::Files,
                                SavedConnectionStartOptions::default(),
                                cx,
                            );
                            if options.workspace_split.is_none() {
                                options.workspace_split =
                                    files.map(|id| (WorkspaceSplitDirection::Horizontal, id));
                            }
                            app.open_android_view(config, AndroidViewKind::Shell, options, cx);
                        }
                    }
                    Err(error) => app.shell.set_status(error),
                }
                cx.notify();
            });
            let _ = NyaTermApp::update_in_main_window(&this, cx, |app, window, cx| {
                app.pump_startup_restore_queue_if_ready(window, cx);
            });
        })
        .detach();
    }
    pub(in crate::features) fn open_android_view(
        &mut self,
        config: AndroidSessionConfig,
        kind: AndroidViewKind,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if kind != AndroidViewKind::Shell
            && self.session.restore_is_complete()
            && options.reconnect_session_id.is_none()
            && let Some(id) = self.android.views.iter().find_map(|(id, v)| {
                (v.device_key == config.device_key && v.kind == kind).then(|| id.clone())
            })
        {
            self.activate_session_id(&id, cx);
            return Some(id);
        }
        if kind == AndroidViewKind::Shell {
            self.begin_background_session_start(
                format!("{} · Shell", config.name),
                SessionLaunchConfig::AdbShell(config.clone()),
                Some(config.device_key),
                AiExecutionProfile::SendOnly,
                options,
                cx,
            );
            return None;
        }
        let id = options.reconnect_session_id.clone().unwrap_or_else(uuid);
        let launch = match kind {
            AndroidViewKind::Files => SessionLaunchConfig::AdbFiles(config.clone()),
            AndroidViewKind::Screen => SessionLaunchConfig::AdbScreen(config.clone()),
            AndroidViewKind::Shell => unreachable!(),
        };
        let metadata = SessionRuntimeMetadata {
            ssh_config: None,
            ssh_multiplex_key: None,
            source_connection_id: Some(config.device_key.clone()),
            ai_execution_profile: AiExecutionProfile::Disabled,
            launch_config: launch,
            disconnected: false,
        };
        self.android.views.insert(
            id.clone(),
            AndroidView {
                device_key: config.device_key.clone(),
                kind,
            },
        );
        if options.reconnect_session_id.is_some() {
            self.register_session_for_reconnect(&id, metadata);
        } else {
            self.register_session_for_start(
                &id,
                metadata,
                options.tab_placement,
                options.insert_index,
            );
        }
        if let Some(name) = options.custom_name.clone() {
            self.session.set_custom_name(id.clone(), name);
        }
        if let Some(color) = options.tab_color {
            self.session.set_tab_color(&id, Some(color));
        }
        if options.locked {
            self.session.set_tab_locked(&id, true);
        }
        if kind == AndroidViewKind::Files {
            let manager_id = format!("sftp-{id}");
            let app = cx.entity();
            let key = config.device_key.clone();
            let seed = FileManagerSeed {
                left: PaneEndpoint::Adb(key),
                left_path: Some(RemoteFilePath::new(config.directory.clone())),
                right: PaneEndpoint::Local,
                right_path: None,
            };
            if self.file_manager.workspace(&manager_id).is_none() {
                let workspace =
                    cx.new(|cx| FileManagerWorkspace::new(app, manager_id.clone(), seed, cx));
                workspace.update(cx, |w, _| w.single_pane = true);
                self.file_manager.insert_android(id.clone(), workspace);
            }
            self.refresh_file_manager_pane(manager_id, PaneId::Left, cx);
        } else {
            self.begin_android_screen(id.clone(), config, cx);
        }
        self.activate_session_id(&id, cx);
        self.shell.select_nav(NavItem::Workspace);
        self.apply_workspace_split_for_duplicate(cx, options.workspace_split, &id);
        self.persist_open_tabs();
        cx.notify();
        Some(id)
    }
    fn begin_android_screen(
        &mut self,
        id: String,
        config: AndroidSessionConfig,
        cx: &mut Context<Self>,
    ) {
        self.android.close_view(&id);
        self.android.views.insert(
            id.clone(),
            AndroidView {
                device_key: config.device_key.clone(),
                kind: AndroidViewKind::Screen,
            },
        );
        self.android.generation = self.android.generation.wrapping_add(1);
        let attempt = self.android.generation;
        self.android.screens.insert(
            id.clone(),
            ScreenState {
                attempt,
                physical_keys: Default::default(),
                handle: None,
                frame: None,
                texture: None,
                texture_size: None,
                bounds: None,
                bounds_generation: 0,
                generation: 0,
                dimensions: (0, 0),
                drag: false,
                error: None,
                last_render: std::time::Instant::now(),
                zoom: 1.0,
            },
        );
        let generation = config.binding.generation;
        let task = self
            .blocking_jobs
            .submit_task("android-screen-start", move |_| {
                ScreenHandle::start(config.client, config.binding, config.screen)
                    .map_err(|e| e.to_string())
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|v| v);
            let _ = this.update(cx, |app, cx| {
                let valid = app
                    .android
                    .screens
                    .get(&id)
                    .is_some_and(|screen| screen.attempt == attempt)
                    && app
                        .android
                        .views
                        .get(&id)
                        .and_then(|v| app.android.devices.get(&v.device_key))
                        .is_some_and(|d| d.config.binding.generation == generation);
                if !valid {
                    return;
                }
                if let Some(screen) = app.android.screens.get_mut(&id) {
                    match result {
                        Ok(handle) => screen.handle = Some(handle),
                        Err(error) => screen.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(in crate::features) fn android_open_from_session(
        &mut self,
        id: &str,
        kind: AndroidViewKind,
        cx: &mut Context<Self>,
    ) {
        if let Some(config) = self.android_config_for_session(id) {
            if self
                .android
                .file_service(&config.device_key)
                .is_ok_and(|files| files.control.check_cancelled().is_ok())
            {
                self.open_android_view(config, kind, SavedConnectionStartOptions::default(), cx);
            } else if let Some(connection) = self
                .connection_state
                .connections()
                .iter()
                .find(|connection| connection.id == config.device_key)
                .cloned()
            {
                let existing = self.android.views.iter().find_map(|(id, view)| {
                    (kind != AndroidViewKind::Shell
                        && view.device_key == config.device_key
                        && view.kind == kind)
                        .then(|| id.clone())
                });
                self.start_android_connection(
                    connection,
                    SavedConnectionStartOptions {
                        reconnect_session_id: existing,
                        ..Default::default()
                    },
                    Some(kind),
                    cx,
                );
            }
        }
    }
    pub(in crate::features) fn android_config_for_session(
        &self,
        id: &str,
    ) -> Option<AndroidSessionConfig> {
        match &self.session.metadata(id)?.launch_config {
            SessionLaunchConfig::AdbShell(c)
            | SessionLaunchConfig::AdbFiles(c)
            | SessionLaunchConfig::AdbScreen(c) => self
                .android
                .devices
                .get(&c.device_key)
                .map(|d| d.config.clone()),
            _ => None,
        }
    }
    pub(in crate::features) fn focus_android_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.android.focus_out.is_none() {
            self.android.focus_out =
                Some(
                    cx.on_focus_out(&self.android.focus, window, |app, _, _, _| {
                        let ids: Vec<String> = app.android.screens.keys().cloned().collect();
                        for id in ids {
                            app.android.release_inputs(&id);
                        }
                    }),
                );
        }
        window.focus(&self.android.focus, cx);
    }
    pub(in crate::features) fn close_android_device(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self
            .android
            .views
            .get(session_id)
            .map(|v| v.device_key.clone())
        else {
            return;
        };
        self.android.pending.remove(&key);
        if let Some(device) = self.android.devices.remove(&key) {
            device.files.cancel();
        }
        let ids: Vec<String> = self
            .android
            .views
            .iter()
            .filter(|(_, v)| v.device_key == key)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.session.manager().close_detached(&id);
            self.remove_session_state(&id, cx);
        }
        self.prune_workspace_split();
        self.rebuild_session_tab_owners();
        self.reconcile_terminal_windows();
        if let Some(next) = self.session.next_live_session() {
            self.activate_session_id(&next, cx);
        }
        self.persist_open_tabs();
        self.persist_workspace_pane_layout();
        cx.notify();
    }
    pub(in crate::features) fn prepare_android_tracker(&mut self, cx: &mut Context<Self>) {
        self.ensure_android_clock(cx);
        let custom = self.settings.summary().android_adb_path.clone();
        let tracker = self.android.tracker.clone();
        let task = self
            .blocking_jobs
            .submit_task("android-discovery", move |_| {
                let client =
                    AdbClient::local((!custom.is_empty()).then(|| std::path::Path::new(&custom)))
                        .map_err(|e| e.to_string())?;
                tracker.ensure(&client).map_err(|e| e.to_string())
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|v| v);
            let _ = this.update(cx, |app, cx| {
                if let Err(error) = result {
                    app.shell.set_status(error);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn ensure_android_clock(&mut self, cx: &mut Context<Self>) {
        if self.android.clock.is_some() {
            return;
        }
        self.android.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                if this
                    .update(cx, |app, cx| {
                        let snapshot = app.android.tracker.snapshot();
                        if snapshot.revision == 0
                            || snapshot.revision == app.android.tracker_revision
                        {
                            return;
                        }
                        app.android.tracker_revision = snapshot.revision;
                        let mut missing = Vec::new();
                        for (key, lease) in &mut app.android.devices {
                            let device = snapshot
                                .devices
                                .iter()
                                .find(|d| d.serial == lease.config.binding.serial);
                            if device.is_some_and(|d| {
                                d.status == nyaterm_android::adb::client::DeviceStatus::Online
                            }) {
                                lease.observed = true;
                            } else if lease.observed {
                                missing.push(key.clone());
                                lease.observed = false;
                            }
                        }
                        for key in missing {
                            if let Some(lease) = app.android.devices.get(&key) {
                                lease.files.cancel();
                            }
                            let views: Vec<String> = app
                                .android
                                .views
                                .iter()
                                .filter(|(_, v)| v.device_key == key)
                                .map(|(id, _)| id.clone())
                                .collect();
                            for id in views {
                                app.android.stop_screen(&id);
                                let _ = app.session.manager().close_detached(&id);
                                app.mark_session_disconnected(&id, cx);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
}

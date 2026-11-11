use super::labels::AndroidError;
use super::{AndroidView, DeviceLease, ScreenState};
use crate::features::file_manager::{FileManagerSeed, FileManagerWorkspace, PaneEndpoint, PaneId};
use crate::features::{
    NyaTermApp, runtime_jobs::await_blocking_job, session::SavedConnectionStartOptions,
};
use crate::models::session::AndroidSessionConfig;
use crate::models::{NavItem, SessionLaunchConfig, SessionRuntimeMetadata};
use gpui::{AppContext, Context, Window};
use nyaterm_android::adb::client::AdbClient;
use nyaterm_android::screen::ScreenHandle;
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection, uuid};
use nyaterm_files::adb::AdbFileService;
use nyaterm_files::sftp::RemoteFilePath;
use rust_i18n::t;
use std::sync::Arc;

impl NyaTermApp {
    pub(super) fn report_android_error(
        &mut self,
        operation: &'static str,
        request_id: &str,
        error: &AndroidError,
        cx: &mut Context<Self>,
    ) {
        self.report_android_failure(operation, request_id, error.code(), error.message(), cx);
    }
    pub(super) fn report_android_failure(
        &mut self,
        operation: &'static str,
        request_id: &str,
        code: &'static str,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if code == "Cancelled" {
            return;
        }
        tracing::warn!(
            operation,
            backend = "Adb",
            request_id,
            error_code = code,
            "Android operation failed"
        );
        self.shell.set_status(message.clone());
        self.notify_background_operation(
            "android-operation",
            nyaterm_ui::notification::NyaNotificationKind::Error,
            message,
            cx,
        );
    }
    pub(in crate::features) fn start_android_connection(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        kind: Option<AndroidViewKind>,
        cx: &mut Context<Self>,
    ) {
        let kind = kind.unwrap_or(AndroidViewKind::Shell);
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
            && let Some(lease) = self.android.devices.get_mut(&key)
            && lease.config.device == device
            && lease.files.control.check_cancelled().is_ok()
            && lease.files.runtime.is_valid()
        {
            lease.config.name = connection.name.clone();
            lease.config.directory = directory;
            lease.config.screen = screen;
            let config = lease.config.clone();
            self.open_android_view(config, kind, options, cx);
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
                AdbClient::local((!custom.is_empty()).then(|| std::path::Path::new(&custom)))?;
            tracker.ensure(&client)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let snapshot = loop {
                let snapshot = tracker.snapshot();
                if snapshot.revision > 0 {
                    break snapshot;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(nyaterm_android::error::AdbError::Timeout);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            };
            let binding = client.bind(&device, generation)?;
            let snapshot = if snapshot.devices.iter().any(|entry| {
                entry.serial == binding.serial
                    && entry.status == nyaterm_android::adb::client::DeviceStatus::Online
                    && entry.transport_id == binding.transport_id
            }) {
                snapshot
            } else {
                // Explicit TCP connect may create the online mapping. Wait for that
                // new binding's tracker epoch rather than treating it as an old task.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let current = tracker.snapshot();
                    if current.server_generation != snapshot.server_generation {
                        return Err(nyaterm_android::error::AdbError::Offline);
                    }
                    if current.devices.iter().any(|entry| {
                        entry.serial == binding.serial
                            && entry.status == nyaterm_android::adb::client::DeviceStatus::Online
                            && entry.transport_id == binding.transport_id
                    }) {
                        break current;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(nyaterm_android::error::AdbError::Timeout);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            };
            let files = Arc::new(AdbFileService::new(binding.clone()));
            tracker.register_runtime_since(&files.runtime, &snapshot)?;
            Ok::<_, nyaterm_android::error::AdbError>((
                AndroidSessionConfig {
                    name,
                    device_key: task_key,
                    device,
                    directory,
                    screen,
                    binding,
                    client,
                    cols: 80,
                    rows: 24,
                },
                files,
            ))
        });
        self.shell
            .set_status(t!("android.connecting", name = connection.name).into_owned());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.map_err(|_| AndroidError::JobFailed).and_then(|v| v.map_err(AndroidError::Backend));
            let _ = this.update(cx, |app, cx| {
                if app.android.pending.get(&key) != Some(&generation) {
                    return;
                }
                app.android.pending.remove(&key);
                if let Err(error) = &result { app.report_android_error("bind", &generation.to_string(), error, cx); }
                match result {
                    Ok((config, files)) => {
                        if !files.runtime.is_valid() {
                            app.report_android_error("bind", &generation.to_string(), &AndroidError::Backend(nyaterm_android::error::AdbError::Offline), cx);
                            return;
                        }
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
                                files,
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
                                    let mut pending = ticket.lock().unwrap_or_else(|p| p.into_inner());
                                    let cleanup = pending.take();
                                    if let Some(mut cleanup) = cleanup {
                                        if cleanup.binding.serial != binding.serial {
                                            *pending = Some(cleanup);
                                            continue;
                                        }
                                        cleanup.binding = binding.clone();
                                        if cleanup.retry().is_err() {
                                            *pending = Some(cleanup);
                                        }
                                    }
                                }
                            },
                        );
                        app.open_android_view(config, kind, options, cx);
                    }
                    Err(error) => app.shell.set_status(error.message()),
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
                t!("android.shellTitle", name = config.name).into_owned(),
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
                left_path: Some(RemoteFilePath::new(if config.directory.is_empty() {
                    nyaterm_core::models::android::default_android_directory()
                } else {
                    config.directory.clone()
                })),
                right: PaneEndpoint::Local,
                right_path: None,
            };
            if self.file_manager.workspace(&manager_id).is_none() {
                let workspace =
                    cx.new(|cx| FileManagerWorkspace::new(app, manager_id.clone(), seed, cx));
                self.file_manager.insert_android(id.clone(), workspace);
            }
            self.refresh_file_manager_pane(manager_id.clone(), PaneId::Left, cx);
            self.refresh_file_manager_pane(manager_id, PaneId::Right, cx);
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
        let runtime = self
            .android
            .file_service(&config.device_key)
            .map(|files| files.runtime.clone())
            .unwrap_or_else(|_| {
                let runtime = Arc::new(nyaterm_android::adb::runtime::DeviceRuntime::new(
                    config.binding.clone(),
                ));
                runtime.invalidate();
                runtime
            });
        let start_control = runtime.control.child();
        let ticket = Arc::new(nyaterm_android::screen::CleanupStatus::default());
        self.android.cleanup.push(ticket.clone());
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
                start_control: start_control.clone(),
                visible: true,
                error_reported: false,
                last_input_notice: None,
                zoom: 1.0,
            },
        );
        let generation = config.binding.generation;
        let task = self
            .blocking_jobs
            .submit_task("android-screen-start", move |_| {
                ScreenHandle::start_controlled(
                    config.client,
                    config.binding,
                    config.screen,
                    start_control,
                    ticket,
                    runtime,
                )
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task)
                .await
                .map_err(|_| AndroidError::JobFailed)
                .and_then(|v| v.map_err(AndroidError::Backend));
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
                if let Err(error) = &result {
                    app.report_android_error("screen-start", &attempt.to_string(), error, cx);
                }
                if let Some(screen) = app.android.screens.get_mut(&id) {
                    match result {
                        Ok(handle) => {
                            if handle
                                .send(nyaterm_android::ipc::Control::Presentation {
                                    visible: screen.visible,
                                })
                                .is_err()
                            {
                                screen.error = Some(AndroidError::Disconnected);
                            }
                            screen.handle = Some(handle);
                        }
                        Err(error) => {
                            screen.error = Some(error);
                            screen.error_reported = true;
                        }
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
    pub(in crate::features) fn prepare_android_tracker(&mut self, cx: &mut Context<Self>) {
        self.ensure_android_clock(cx);
        let custom = self.settings.summary().android_adb_path.clone();
        let tracker = self.android.tracker.clone();
        let task = self
            .blocking_jobs
            .submit_task("android-discovery", move |_| {
                let client =
                    AdbClient::local((!custom.is_empty()).then(|| std::path::Path::new(&custom)))?;
                tracker.ensure(&client)
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task)
                .await
                .map_err(|_| AndroidError::JobFailed)
                .and_then(|v| v.map_err(AndroidError::Backend));
            let _ = this.update(cx, |app, cx| {
                if let Err(error) = result {
                    app.shell.set_status(error.message());
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
                        app.android.collect_unused_devices();
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
                                    && d.transport_id == lease.config.binding.transport_id
                                    && lease.files.runtime.is_valid()
                            }) {
                                lease.observed = true;
                            } else if lease.observed || !lease.files.runtime.is_valid() {
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

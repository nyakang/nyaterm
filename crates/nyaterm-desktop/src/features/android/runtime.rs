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

/// Sessions that need a live adb server, not just a device listing.
fn android_session(kind: nyaterm_transport::SessionKind) -> bool {
    matches!(
        kind,
        nyaterm_transport::SessionKind::AdbShell
            | nyaterm_transport::SessionKind::AdbFiles
            | nyaterm_transport::SessionKind::AdbScreen
    )
}

impl NyaTermApp {
    pub(in crate::features) fn android_screen_button_available(&self, id: &str) -> bool {
        matches!(
            self.session
                .metadata(id)
                .map(|metadata| &metadata.launch_config),
            Some(SessionLaunchConfig::AdbShell(_))
        ) && self
            .tab_action_policy_for(id, &self.tab_root_for_session(id))
            .is_some_and(|policy| policy.availability.android_screen)
    }

    pub(in crate::features) fn open_android_screen_from_terminal(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.android_screen_button_available(id) {
            self.android_open_from_session(id, AndroidViewKind::Screen, cx);
        }
    }

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
        if !self.session.start_request_can_start(&options) {
            return;
        }
        if let Some(request_id) = self.android.pending.get(&connection.id)
            && self
                .session
                .start_pending_entries()
                .any(|(id, _)| id == request_id)
        {
            return;
        }
        let mut options = self.prepare_session_start_options(options);
        options.android_view = kind;
        options.request_id = Some(
            self.session
                .begin_saved_start_preparation(&connection, &options),
        );
        let pending_key = connection.id.clone();
        self.android
            .pending
            .insert(pending_key.clone(), options.request_id.clone().unwrap());
        self.shell.show_workspace();
        self.ensure_pending_session_status_clock(cx);
        cx.notify();
        let request_options = options.clone();
        self.ensure_native_component(
            nyaterm_core::plugins::native::NativeComponentId::Android,
            move |app| app.session.start_request_can_start(&request_options),
            move |app, result, cx| {
                if app.android.pending.get(&pending_key) == options.request_id.as_ref() {
                    app.android.pending.remove(&pending_key);
                }
                if !app.session.start_request_can_start(&options) {
                    return;
                }
                match result {
                    Ok(component) => app.start_android_connection_with_component(
                        connection, options, kind, component, cx,
                    ),
                    Err(error) => {
                        app.fail_start_preparation(&options, error.message, cx);
                        cx.notify();
                    }
                }
            },
            cx,
        );
    }

    fn start_android_connection_with_component(
        &mut self,
        connection: SavedConnection,
        options: SavedConnectionStartOptions,
        kind: Option<AndroidViewKind>,
        component: nyaterm_core::plugins::native::NativeLaunch,
        cx: &mut Context<Self>,
    ) {
        let kind = kind.unwrap_or(AndroidViewKind::Shell);
        if !self.session.start_request_can_start(&options) {
            return;
        }
        // A session needs the server before any tab or view exists; the
        // session itself then keeps the lease.
        self.android.sync_discovery(true);
        self.ensure_android_clock(cx);
        self.android_ensure_tracker(cx);
        let ConnectionType::Adb {
            device,
            directory,
            screen,
        } = connection.config.clone()
        else {
            return;
        };
        let key = connection.id.clone();
        if let Some(lease) = self.android.devices.get_mut(&key)
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
        if let Some(request_id) = self.android.pending.get(&key)
            && self
                .session
                .start_pending_entries()
                .any(|(id, _)| id == request_id)
        {
            return;
        }
        let mut options = self.prepare_session_start_options(options);
        options.android_view = Some(kind);
        if options.request_id.is_none() {
            options.request_id = Some(
                self.session
                    .begin_saved_start_preparation(&connection, &options),
            );
        }
        let attempt = self
            .session
            .start_attempt(options.request_id.as_ref().unwrap());
        self.shell.clear_last_connect_failure();
        self.shell.show_workspace();
        self.ensure_pending_session_status_clock(cx);
        let generation = super::next_generation();
        self.android
            .pending
            .insert(key.clone(), options.request_id.clone().unwrap());
        let tracker = self.android.tracker.clone();
        let name = connection.name.clone();
        let task_key = key.clone();
        let binding_lease = tracker.lease();
        let cancellation = attempt.clone();
        let ticket = tracker
            .discovery_control()
            .with_cancellation(move || cancellation.is_cancelled());
        let task = self.blocking_jobs.submit_task("android-bind", move |_| {
            let check_cancelled = || {
                if attempt.is_cancelled() {
                    Err(nyaterm_android::error::AdbError::Cancelled)
                } else {
                    Ok(())
                }
            };
            check_cancelled()?;
            let client = AdbClient::local(Some(
                component
                    .root
                    .join("adb")
                    .join(if cfg!(windows) { "adb.exe" } else { "adb" })
                    .as_path(),
            ))?
            .with_component(component)
            .with_server_manager(tracker.server_manager());
            tracker.ensure_with_control(&client, &ticket)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let snapshot = loop {
                check_cancelled()?;
                let snapshot = tracker.snapshot();
                if snapshot.revision > 0 {
                    break snapshot;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(nyaterm_android::error::AdbError::Timeout);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            };
            let binding = client.bind_with_control(&device, generation, &ticket)?;
            check_cancelled()?;
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
                    check_cancelled()?;
                    let current = tracker.snapshot();
                    if current.tracker_generation != snapshot.tracker_generation
                        || current.server_generation != snapshot.server_generation
                    {
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
            let files =
                Arc::new(AdbFileService::new(binding.clone()).with_discovery_lease(binding_lease));
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
                if app.android.pending.get(&key) != options.request_id.as_ref() {
                    return;
                }
                app.android.pending.remove(&key);
                if !app.session.start_request_can_start(&options) {
                    return;
                }
                match result {
                    Ok((config, files)) => {
                        if !files.runtime.is_valid() {
                            let error = AndroidError::Backend(nyaterm_android::error::AdbError::Offline);
                            app.fail_start_preparation(&options, error.message(), cx);
                            app.shell.set_status(error.message());
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
                                    SessionLaunchConfig::AdbShell(current) | SessionLaunchConfig::AdbFiles(current) | SessionLaunchConfig::AdbScreen(current) => {
                                        *current = config.clone();
                                        current.client.component = None;
                                    },
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
                        let cleanup_client = config.client.clone();
                        let cleanup_lease = app.android.tracker.lease();
                        let _ = app.blocking_jobs.submit_detached(
                            "android-deferred-cleanup",
                            move |_| {
                                let _cleanup_lease = cleanup_lease;
                                for ticket in tickets {
                                    let mut pending = ticket.lock().unwrap_or_else(|p| p.into_inner());
                                    let cleanup = pending.take();
                                    if let Some(mut cleanup) = cleanup {
                                        if cleanup.binding.serial != binding.serial {
                                            *pending = Some(cleanup);
                                            continue;
                                        }
                                        cleanup.binding = binding.clone();
                                        cleanup.client = cleanup_client.clone();
                                        if cleanup.retry().is_err() {
                                            cleanup.client.component = None;
                                            *pending = Some(cleanup);
                                        }
                                    }
                                }
                            },
                        );
                        app.open_android_view(config, kind, options, cx);
                    }
                    Err(error) => {
                        app.fail_start_preparation(&options, error.message(), cx);
                        app.shell.set_status(error.message());
                    }
                }
                cx.notify();
            });
            let _ = NyaTermApp::update_in_main_window(&this, cx, |app, window, cx| {
                app.pump_startup_restore_queue_if_ready(window, cx);
            });
        })
        .detach();
    }
    /// New views requested from an existing session still go through registry
    /// authorization; the source session's retained launch is not permission
    /// to launch a disabled or uninstalled component again.
    pub(in crate::features) fn request_android_view(
        &mut self,
        mut config: AndroidSessionConfig,
        kind: AndroidViewKind,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let generation = config.binding.generation;
        let key = config.device_key.clone();
        let discovery = self.android.tracker.lease();
        self.ensure_native_component(
            nyaterm_core::plugins::native::NativeComponentId::Android,
            move |app| {
                app.android.devices.get(&key).is_some_and(|device| {
                    device.config.binding.generation == generation
                        && device.files.runtime.is_valid()
                })
            },
            move |app, result, cx| {
                let _discovery = discovery;
                if !app
                    .android
                    .devices
                    .get(&config.device_key)
                    .is_some_and(|device| {
                        device.config.binding.generation == generation
                            && device.files.runtime.is_valid()
                    })
                {
                    return;
                }
                match result {
                    Ok(component) => {
                        config.client.executable = component
                            .root
                            .join("adb")
                            .join(if cfg!(windows) { "adb.exe" } else { "adb" });
                        config.client.component = Some(component);
                        app.open_android_view(config, kind, options, cx);
                    }
                    Err(error) => {
                        app.shell.set_status(error.message);
                        cx.notify();
                    }
                }
            },
            cx,
        );
    }

    pub(in crate::features) fn open_android_view(
        &mut self,
        config: AndroidSessionConfig,
        kind: AndroidViewKind,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let options = if options.request_id.is_some() {
            self.prepare_session_start_options(options)
        } else {
            options
        };
        if kind != AndroidViewKind::Shell
            && let Some(request_id) = options.request_id.as_ref()
        {
            // These views register directly instead of submitting a shell worker.
            self.session.complete_start_preparation(request_id);
        }
        if kind != AndroidViewKind::Shell
            && self.session.restore_is_complete()
            && options.reconnect_session_id.is_none()
            && let Some(id) = self.android.views.iter().find_map(|(id, v)| {
                (v.device_key == config.device_key && v.kind == kind).then(|| id.clone())
            })
        {
            self.activate_session_id(&id, cx);
            self.settle_session_start_tab_placements_if_idle();
            return Some(id);
        }
        if kind == AndroidViewKind::Shell {
            self.begin_background_session_start(
                config.name.clone(),
                SessionLaunchConfig::AdbShell(config.clone()),
                Some(config.device_key),
                AiExecutionProfile::SendOnly,
                options,
                cx,
            );
            return None;
        }
        let id = options.reconnect_session_id.clone().unwrap_or_else(uuid);
        let mut config = config;
        if kind == AndroidViewKind::Screen
            && let Some(screen) = self.android.screens.get(&id)
        {
            config.screen = screen.settings;
        }
        let launch = match kind {
            AndroidViewKind::Files => SessionLaunchConfig::AdbFiles(config.clone()),
            AndroidViewKind::Screen => SessionLaunchConfig::AdbScreen(config.clone()),
            AndroidViewKind::Shell => unreachable!(),
        };
        let metadata = SessionRuntimeMetadata {
            confirmed_ssh_runtime_mode: None,
            pending_ssh_startup_command: None,
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
        self.settle_session_start_tab_placements_if_idle();
        self.persist_open_tabs();
        cx.notify();
        Some(id)
    }
    pub(super) fn begin_android_screen(
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
        let attempt = super::next_generation();
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
            ScreenState::new(cx.focus_handle(), attempt, start_control.clone()),
        );
        if let Some(screen) = self.android.screens.get_mut(&id) {
            screen.settings = config.screen;
        }
        let generation = config.binding.generation;
        let discovery = self.android.tracker.lease();
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
                    discovery,
                )
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task)
                .await
                .map_err(|_| AndroidError::JobFailed)
                .and_then(|v| v.map_err(AndroidError::Backend));
            let _ = this.update(cx, |app, cx| {
                let valid = app.android.screens.get(&id).is_some_and(|screen| {
                    screen.attempt == attempt && !screen.start_control.is_cancelled()
                }) && app
                    .android
                    .views
                    .get(&id)
                    .and_then(|v| app.android.devices.get(&v.device_key))
                    .is_some_and(|d| {
                        d.config.binding.generation == generation && d.files.runtime.is_valid()
                    });
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
                self.request_android_view(config, kind, SavedConnectionStartOptions::default(), cx);
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
            | SessionLaunchConfig::AdbScreen(c) => {
                self.android.devices.get(&c.device_key).map(|d| {
                    let mut config = d.config.clone();
                    if let Some(screen) = self.android.screens.get(id) {
                        config.screen = screen.settings;
                    }
                    config
                })
            }
            _ => None,
        }
    }
    pub(in crate::features) fn focus_android_screen(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(screen) = self.android.screens.get_mut(id) else {
            return;
        };
        if screen.focus_out.is_none() {
            let target = id.to_owned();
            screen.focus_out = Some(cx.on_focus_out(&screen.focus, window, move |app, _, _, _| {
                app.android.release_inputs(&target);
            }));
        }
        window.focus(&screen.focus, cx);
    }
    /// Whether some surface in this window needs device discovery.
    pub(in crate::features) fn android_discovery_needed(&self) -> bool {
        // A start that is still binding its device has no view or session yet
        // but is talking to the server right now; counting it keeps the tick
        // from releasing the lease in the middle of that bind.
        !self.android.pending.is_empty()
            || self
                .session
                .start_pending_entries()
                .any(|(_, pending)| android_session(pending.kind))
            || self
                .connection_state
                .active_editor_draft()
                .is_some_and(|editor| editor.kind == crate::models::ConnectionKindTab::Adb)
            || self.android.views.keys().any(|id| {
                self.session
                    .metadata(id)
                    .is_some_and(|metadata| !metadata.disconnected)
            })
            || self.android.devices.values().any(|lease| {
                lease.files.runtime.is_valid()
                    && Arc::strong_count(&lease.files) > Arc::strong_count(&lease.owners)
            })
            || self.session.ordered_sessions().iter().any(|session| {
                android_session(session.kind)
                    && self
                        .session
                        .metadata(&session.id)
                        .is_none_or(|metadata| !metadata.disconnected)
            })
    }

    pub(in crate::features) fn sync_android_discovery(&mut self) {
        self.collect_android_devices();
        let needed = self.android_discovery_needed();
        self.android.sync_discovery(needed);
    }

    pub(in crate::features) fn prepare_android_tracker(&mut self, cx: &mut Context<Self>) {
        // Some callers arrive before the draft is visible to
        // `active_editor_draft`, so the lease is taken here unconditionally;
        // the editor/view/session exits give it back.
        self.android.sync_discovery(true);
        self.ensure_android_clock(cx);
        self.android_ensure_tracker(cx);
    }

    /// Whether something needs discovery but no tracker is serving it.
    ///
    /// Split out so tests can assert the restart decision without an `adb`.
    pub(in crate::features) fn android_needs_tracker(&self) -> bool {
        self.android.holds_discovery()
            && self.android.tracker_start_pending.is_none()
            && !self.android.tracker.is_active()
            && (self.android.tracker_error.is_none() || self.android.tracker_retry_after.is_some())
            && self
                .android
                .tracker_retry_after
                .is_none_or(|at| std::time::Instant::now() >= at)
    }

    pub(in crate::features) fn android_ensure_tracker(&mut self, cx: &mut Context<Self>) {
        if !self.android_needs_tracker() {
            return;
        }
        let attempt = super::next_generation();
        let ticket = self.android.tracker.discovery_control();
        self.android.tracker_start_pending = Some((attempt, ticket.clone()));
        let valid_ticket = ticket.clone();
        self.ensure_native_component(
            nyaterm_core::plugins::native::NativeComponentId::Android,
            move |_| !valid_ticket.is_cancelled(),
            move |app, result, cx| {
                if app
                    .android
                    .tracker_start_pending
                    .as_ref()
                    .is_none_or(|(current, _)| *current != attempt)
                {
                    return;
                }
                if ticket.is_cancelled() {
                    app.android.tracker_start_pending = None;
                    return;
                }
                match result {
                    Ok(component) => app.start_android_tracker(component, ticket, attempt, cx),
                    Err(error) => {
                        app.android.tracker_start_pending = None;
                        app.android.tracker_retry_after = None;
                        app.android.tracker_error = Some(error.message.clone());
                        app.shell.set_status(error.message);
                        cx.notify();
                    }
                }
            },
            cx,
        );
    }

    fn start_android_tracker(
        &mut self,
        component: nyaterm_core::plugins::native::NativeLaunch,
        ticket: nyaterm_android::operation::AdbOperationControl,
        attempt: u64,
        cx: &mut Context<Self>,
    ) {
        let job_ticket = ticket.clone();
        let tracker = self.android.tracker.clone();
        let task = self
            .blocking_jobs
            .submit_task("android-discovery", move |_| {
                job_ticket.check()?;
                let client = AdbClient::local(Some(
                    component
                        .root
                        .join("adb")
                        .join(if cfg!(windows) { "adb.exe" } else { "adb" })
                        .as_path(),
                ))?
                .with_component(component)
                .with_server_manager(tracker.server_manager());
                tracker.ensure_with_control(&client, &job_ticket)
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task)
                .await
                .map_err(|_| AndroidError::JobFailed)
                .and_then(|v| v.map_err(AndroidError::Backend));
            let _ = this.update(cx, |app, cx| {
                if app
                    .android
                    .tracker_start_pending
                    .as_ref()
                    .is_none_or(|(current, _)| *current != attempt)
                {
                    return;
                }
                app.android.tracker_start_pending = None;
                if ticket.is_cancelled() {
                    return;
                }
                match result {
                    Ok(()) => {
                        app.android.tracker_retry_after = None;
                        app.android.tracker_error = None;
                    }
                    Err(error) => {
                        // Back off so a broken adb cannot turn the 200 ms tick
                        // into a retry storm; report only the first failure of
                        // a streak instead of rewriting the status every 2 s.
                        app.android.tracker_error = Some(error.message());
                        let first = app.android.tracker_retry_after.is_none();
                        app.android.tracker_retry_after =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                        if first {
                            app.shell.set_status(error.message());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn collect_android_devices(&mut self) {
        let mut pending: std::collections::HashSet<String> = self
            .session
            .start_pending_entries()
            .filter_map(|(_, p)| match &p.launch_config {
                Some(SessionLaunchConfig::AdbShell(config)) => Some(config.device_key.clone()),
                _ => None,
            })
            .collect();
        pending.extend(
            self.android
                .views
                .iter()
                .filter(|(id, _)| {
                    self.session
                        .metadata(id)
                        .is_some_and(|metadata| !metadata.disconnected)
                })
                .map(|(_, view)| view.device_key.clone()),
        );
        self.android.collect_unused_devices(&pending);
    }
    pub(in crate::features) fn ensure_android_clock(&mut self, cx: &mut Context<Self>) {
        if self.android.clock.is_some() {
            return;
        }
        self.android.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                if !this
                    .update(cx, |app, cx| {
                        app.sync_android_discovery();
                        if !app.android.holds_discovery()
                            && app.android.tracker_start_pending.is_none()
                        {
                            app.android.clock = None;
                            return false;
                        }
                        // Convergent restart: a consumer that took a lease
                        // without going through the connection editor gets
                        // its tracker from here.
                        app.android_ensure_tracker(cx);
                        let snapshot = match app
                            .android
                            .tracker
                            .snapshot_if_changed(app.android.tracker_revision)
                        {
                            Some(snapshot) => snapshot,
                            None if app
                                .android
                                .devices
                                .values()
                                .all(|lease| lease.files.runtime.is_valid()) =>
                            {
                                return true;
                            }
                            None => app.android.tracker.snapshot(),
                        };
                        let mut changed = (snapshot.tracker_generation, snapshot.revision)
                            != app.android.tracker_revision;
                        app.android.tracker_revision =
                            (snapshot.tracker_generation, snapshot.revision);
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
                                .filter(|(id, v)| {
                                    v.device_key == key
                                        && app
                                            .session
                                            .metadata(id)
                                            .is_none_or(|metadata| !metadata.disconnected)
                                })
                                .map(|(id, _)| id.clone())
                                .collect();
                            for id in views {
                                changed = true;
                                app.android.stop_screen(&id);
                                let _ = app.session.manager().close_detached(&id);
                                app.mark_session_disconnected(&id, cx);
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                        true
                    })
                    .is_ok_and(|running| running)
                {
                    break;
                }
            }
        }));
    }
}

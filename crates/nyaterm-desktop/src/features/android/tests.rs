use crate::features::android::{AndroidView, DeviceLease};
use crate::features::test_support::app_with_visible_local_session;
use crate::models::session::AndroidSessionConfig;
use crate::test_support::{TestAppCleanup, TestConfigDir};
use nyaterm_android::adb::client::{AdbClient, DeviceBinding};
use nyaterm_core::models::android::{AdbDeviceSelector, AndroidScreenSettings, AndroidViewKind};
use nyaterm_files::adb::AdbFileService;
use std::sync::Arc;

#[test]
fn adb_shell_uses_connection_name_and_preserves_custom_label() {
    use crate::features::test_support::app_with_visible_android_session;

    let root = TestConfigDir::new("nyaterm-adb-label");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "shell", "/sdcard/");
    app.update(&mut cx, |app, _| {
        let name = app.android_config_for_session("shell").unwrap().name;
        assert_eq!(app.session.session_info("shell").unwrap().name, name);
        assert_eq!(app.session.display_name("shell"), Some(name.clone()));
        app.session
            .set_custom_name("shell".into(), "My device".into());
        assert_eq!(
            app.session.display_name("shell").as_deref(),
            Some("My device")
        );
        assert_eq!(app.session.session_info("shell").unwrap().name, name);
    });
}

#[test]
fn adb_screen_button_requires_its_own_live_shell_and_device() {
    use crate::features::test_support::app_with_visible_android_session;

    let root = TestConfigDir::new("nyaterm-adb-screen-button");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "shell", "/sdcard/");
    app.update(&mut cx, |app, _| {
        assert!(app.android_screen_button_available("shell"));
        assert!(!app.android_screen_button_available("missing"));
        app.android.file_service("android-device").unwrap().cancel();
        assert!(!app.android_screen_button_available("shell"));
    });
}

#[test]
fn closing_android_views_keeps_other_channels_and_submitted_file_service_alive() {
    let root = TestConfigDir::new("nyaterm-android-view-lifecycle");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    app.update(&mut cx, |app, _| {
        let binding = DeviceBinding {
            server: "127.0.0.1:5037".parse().unwrap(),
            serial: "synthetic".into(),
            transport_id: Some(7),
            generation: 1,
        };
        let files = Arc::new(AdbFileService::new(binding.clone()));
        let config = AndroidSessionConfig {
            name: "Android".into(),
            device_key: "device".into(),
            device: AdbDeviceSelector::Usb {
                serial: "synthetic".into(),
            },
            directory: "/sdcard/".into(),
            screen: AndroidScreenSettings::default(),
            binding,
            client: AdbClient {
                executable: "unused".into(),
                server: "127.0.0.1:5037".parse().unwrap(),
            },
            cols: 80,
            rows: 24,
        };
        app.android.devices.insert(
            "device".into(),
            DeviceLease {
                config,
                files: files.clone(),
                observed: true,
                owners: Arc::new(()),
            },
        );
        for (id, kind) in [
            ("files", AndroidViewKind::Files),
            ("shell", AndroidViewKind::Shell),
            ("screen", AndroidViewKind::Screen),
        ] {
            app.android.views.insert(
                id.into(),
                AndroidView {
                    device_key: "device".into(),
                    kind,
                },
            );
        }
        app.android.close_view("screen");
        app.android.close_view("files");
        assert_eq!(app.android.kind("shell"), Some(AndroidViewKind::Shell));
        assert!(files.control.check_cancelled().is_ok());
        app.android.shutdown();
        assert!(files.control.check_cancelled().is_err());
    });
}

#[test]
fn ordinary_android_connection_reuses_binding_and_latest_settings_with_requested_split() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    use crate::models::{SessionLaunchConfig, WorkspaceSplitDirection};
    for split in [
        None,
        Some((WorkspaceSplitDirection::Vertical, "fixture".into())),
    ] {
        let root = TestConfigDir::new("nyaterm-android-shell-only");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
        app.update(&mut cx, |app, cx| {
            let connection = serde_json::from_value(serde_json::json!({
                "id":"android-device", "name":"Android fixture", "type":"adb", "device":{"kind":"usb", "serial":"synthetic"}, "directory":"/sdcard/updated"
            })).unwrap();
            app.start_android_connection(connection, SavedConnectionStartOptions { workspace_split: split.clone(), ..Default::default() }, None, cx);
            assert!(app.android.views.values().all(|view| view.kind == AndroidViewKind::Shell));
            assert_eq!(app.file_manager.all_workspaces().count(), 0);
            let pending: Vec<_> = app.session.start_pending_entries().collect();
            assert_eq!(pending.len(), 1);
            assert!(matches!(pending[0].1.launch_config, Some(SessionLaunchConfig::AdbShell(_))));
            let Some(SessionLaunchConfig::AdbShell(config)) = &pending[0].1.launch_config else { unreachable!() };
            assert_eq!(config.directory, "/sdcard/updated");
            assert_eq!(app.android_config_for_session("fixture").unwrap().directory, "/sdcard/updated");
            assert_eq!(pending[0].1.workspace_split, split);
        });
    }
}

#[test]
fn restored_android_files_initializes_both_panes_and_standard_manager_reuses_existing_tab() {
    use crate::features::file_manager::{PaneEndpoint, PaneId};
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    use nyaterm_files::sftp::RemoteFilePath;
    let root = TestConfigDir::new("nyaterm-android-files-entry");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/Download");
    app.update(&mut cx, |app, cx| {
        let config = app.android_config_for_session("fixture").unwrap();
        let id = app
            .open_android_view(
                config,
                AndroidViewKind::Files,
                SavedConnectionStartOptions::default(),
                cx,
            )
            .unwrap();
        let embedded = app.file_manager.workspace(&format!("sftp-{id}")).unwrap();
        assert!(embedded.read(cx).pane(PaneId::Left).generation > 0);
        assert!(embedded.read(cx).pane(PaneId::Right).generation > 0);
        app.session.select_active_session("fixture");
        app.reset_transfer_browser_for_active_session();
        app.transfer
            .begin_browser_directory_load("/sdcard/Download/subdir".into());
        app.open_file_manager_tab(None, cx);
        let manager_id = app.file_manager.active_id().unwrap().to_string();
        let manager = app.file_manager.workspace(&manager_id).unwrap();
        assert_eq!(
            manager.read(cx).pane(PaneId::Right).endpoint,
            PaneEndpoint::Adb("android-device".into())
        );
        assert_eq!(
            manager.read(cx).pane(PaneId::Right).path.display_path,
            "/sdcard/Download/subdir"
        );
        assert!(manager.read(cx).pane(PaneId::Left).generation > 0);
        assert!(manager.read(cx).pane(PaneId::Right).generation > 0);
        manager.update(cx, |manager, _| {
            manager.pane_mut(PaneId::Right).path = RemoteFilePath::new("/sdcard/keep")
        });
        app.transfer
            .begin_browser_directory_load("/sdcard/other".into());
        app.open_file_manager_tab(None, cx);
        assert_eq!(app.file_manager.workspaces().count(), 1);
        assert_eq!(
            manager.read(cx).pane(PaneId::Right).path.display_path,
            "/sdcard/keep"
        );
    });
}

#[test]
fn adb_screen_button_uses_runtime_policy_and_focuses_screen_for_the_target_device() {
    use crate::features::test_support::{
        app_with_visible_android_session, session_focus_test_window,
    };
    use crate::models::SessionLaunchConfig;
    let root = TestConfigDir::new("nyaterm-android-screen-menu");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "shell", "/sdcard/");
    app.update(&mut cx, |app, _| {
        app.session.mark_restore_complete();
        let mut metadata = app.session.metadata("shell").unwrap().clone();
        let config = app.android_config_for_session("shell").unwrap();
        metadata.launch_config = SessionLaunchConfig::AdbScreen(config.clone());
        app.session
            .register_session_metadata("screen", metadata.clone());
        app.android.views.insert(
            "screen".into(),
            AndroidView {
                device_key: config.device_key,
                kind: AndroidViewKind::Screen,
            },
        );
        if let SessionLaunchConfig::AdbScreen(config) = &mut metadata.launch_config {
            config.device_key = "other-device".into();
        }
        app.session
            .register_session_metadata("other-screen", metadata);
        app.android.views.insert(
            "other-screen".into(),
            AndroidView {
                device_key: "other-device".into(),
                kind: AndroidViewKind::Screen,
            },
        );
    });
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.android_screen_button_available("shell"));
            app.android
                .pending
                .insert("android-device".into(), "pending".into());
            assert!(
                !app.tab_action_policy_for("shell", "shell")
                    .unwrap()
                    .availability
                    .android_screen
            );
            app.open_android_screen_from_terminal("shell", cx);
            assert_eq!(app.session.active_id(), Some("shell"));
            app.android.pending.clear();
            app.session.metadata_mut("shell").unwrap().disconnected = true;
            assert!(!app.android_screen_button_available("shell"));
            app.session.metadata_mut("shell").unwrap().disconnected = false;
            app.session.select_active_session("other-screen");
            app.open_android_screen_from_terminal("shell", cx);
            assert_eq!(app.session.active_id(), Some("screen"));
            assert_eq!(app.android.views.len(), 3);
            assert!(
                app.android.screens.is_empty(),
                "existing view must be focused without starting another helper"
            );
        })
    });
}

#[test]
fn changing_saved_device_selector_requires_rebind_without_reusing_the_old_shell_channel() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-selector-rebind");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        // Binding must stop before network discovery; this is a different synthetic device.
        let mut settings = app.settings.summary().clone();
        settings.android_adb_path = root.path().join("missing-adb").to_string_lossy().into_owned();
        app.settings.replace_summary(settings);
        let connection = serde_json::from_value(serde_json::json!({
            "id":"android-device", "name":"Changed device", "type":"adb", "device":{"kind":"usb", "serial":"other-synthetic"}
        })).unwrap();
        app.start_android_connection(connection, SavedConnectionStartOptions::default(), None, cx);
        assert!(app.android.pending.contains_key("android-device"));
        assert_eq!(app.session.start_pending_entries().count(), 1);
        assert_eq!(app.android_config_for_session("fixture").unwrap().binding.serial, "synthetic");
    });
}

#[test]
fn closing_a_starting_screen_cancels_only_its_startup_and_unused_leases_are_released() {
    use crate::features::android::ScreenState;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-start-cancel");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        let service = app.android.file_service("android-device").unwrap();
        let control = service.runtime.control.child();
        app.android.screens.insert(
            "fixture".into(),
            ScreenState::new(cx.focus_handle(), 1, control.clone()),
        );
        app.android.close_view("fixture");
        assert!(control.is_cancelled());
        assert!(service.runtime.is_valid());
        app.android.collect_unused_devices(&Default::default());
        assert!(
            app.android.devices.contains_key("android-device"),
            "submitted service keeps the lease alive"
        );
        let runtime = Arc::downgrade(&service.runtime);
        drop(service);
        app.android.collect_unused_devices(&Default::default());
        assert!(!app.android.devices.contains_key("android-device"));
        assert!(
            runtime.upgrade().is_none(),
            "weak registry must not permanently own the device"
        );
    });
}

#[test]
fn shared_window_leases_do_not_keep_each_other_alive_after_all_views_close() {
    use crate::features::android::AndroidFeatureState;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-shared-lease");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, _| {
        let lease = &app.android.devices["android-device"];
        let weak = Arc::downgrade(&lease.files);
        let mut other = AndroidFeatureState::new(app.android.tracker.clone());
        other.devices.insert(
            "android-device".into(),
            DeviceLease {
                config: lease.config.clone(),
                files: lease.files.clone(),
                observed: lease.observed,
                owners: lease.owners.clone(),
            },
        );
        app.android.close_view("fixture");
        app.android.collect_unused_devices(&Default::default());
        assert!(app.android.devices.is_empty());
        other.collect_unused_devices(&Default::default());
        assert!(other.devices.is_empty());
        assert!(weak.upgrade().is_none());
    });
}

#[test]
fn pending_first_shell_keeps_the_only_device_until_cancelled() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-pending-shell");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        let config = app.android_config_for_session("fixture").unwrap();
        app.android.close_view("fixture");
        app.open_android_view(
            config,
            AndroidViewKind::Shell,
            SavedConnectionStartOptions::default(),
            cx,
        );
        assert!(app.android.views.is_empty());
        for _ in 0..10 {
            app.collect_android_devices();
        }
        assert!(app.android.devices.contains_key("android-device"));
        let request = app
            .session
            .start_pending_entries()
            .next()
            .unwrap()
            .0
            .clone();
        app.close_pending_session_start(request, cx);
        app.collect_android_devices();
        assert!(app.android.devices.is_empty());
    });
}

#[test]
fn unchanged_tracker_revision_does_not_hide_a_binding_invalidated_by_another_window() {
    use crate::features::test_support::app_with_visible_android_session;
    use std::time::Duration;
    let root = TestConfigDir::new("nyaterm-android-shared-invalidation");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        app.ensure_android_clock(cx);
        app.android.file_service("android-device").unwrap().cancel();
        assert!(!app.session.metadata("fixture").unwrap().disconnected);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
    app.update(&mut cx, |app, _| {
        assert!(app.session.metadata("fixture").unwrap().disconnected);
    });
}

#[test]
fn reconnecting_one_shell_reuses_valid_binding_without_stopping_sibling_views() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-channel-reconnect");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app,cx| {
        let service = app.android.file_service("android-device").unwrap();
        for (id,kind) in [("sibling",AndroidViewKind::Shell),("screen",AndroidViewKind::Screen),("files",AndroidViewKind::Files)] {
            app.android.views.insert(id.into(),AndroidView {device_key:"android-device".into(),kind});
        }
        let connection = serde_json::from_value(serde_json::json!({"id":"android-device","name":"Android","type":"adb","device":{"kind":"usb","serial":"synthetic"}})).unwrap();
        app.start_android_connection(connection, SavedConnectionStartOptions { reconnect_session_id:Some("fixture".into()), ..Default::default() }, None, cx);
        assert!(app.android.pending.is_empty());
        assert_eq!(app.session.start_pending_entries().count(),1);
        assert!(Arc::ptr_eq(&service, &app.android.file_service("android-device").unwrap()));
        assert!(service.runtime.is_valid());
        assert_eq!(app.android.views.len(),4);
        assert!(!app.session.metadata("fixture").unwrap().disconnected);
    });
}

#[test]
fn android_transfer_conflict_preserves_both_leases_and_allows_source_rollback() {
    use crate::features::android::AndroidFeatureState;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-transfer-conflict");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        let lease = &app.android.devices["android-device"];
        let source = Arc::clone(&lease.files);
        let config = lease.config.clone();
        let mut target = AndroidFeatureState::new(app.android.tracker.clone());
        let files = Arc::new(AdbFileService::new(config.binding.clone()));
        target.devices.insert(
            "android-device".into(),
            DeviceLease {
                config,
                files: files.clone(),
                observed: true,
                owners: Arc::new(()),
            },
        );
        let bundle = app
            .android
            .detach_tab(&["fixture".into()], &mut app.file_manager);
        assert!(!target.can_attach_tab(&bundle));
        assert!(source.runtime.is_valid() && files.runtime.is_valid());
        assert!(app.android.can_attach_tab(&bundle));
        app.android
            .attach_tab(bundle, &mut app.file_manager, cx.entity(), cx);
        assert!(Arc::ptr_eq(
            &source,
            &app.android.devices["android-device"].files
        ));
        assert!(Arc::ptr_eq(&files, &target.devices["android-device"].files));
        assert!(app.android.views.contains_key("fixture"));
        let bundle = app
            .android
            .detach_tab(&["fixture".into()], &mut app.file_manager);
        files.cancel();
        assert!(target.can_attach_tab(&bundle));
        target.attach_tab(bundle, &mut app.file_manager, cx.entity(), cx);
        assert!(target.views.contains_key("fixture"));
        assert!(Arc::ptr_eq(
            &source,
            &target.devices["android-device"].files
        ));
    });
}

#[test]
fn android_split_screens_have_distinct_focus_and_release_only_the_blurred_screen() {
    use crate::features::android::ScreenState;
    use crate::features::test_support::app_with_visible_android_session;
    use gpui::{IntoElement as _, ParentElement as _, Styled as _};
    struct Host(gpui::Entity<crate::features::NyaTermApp>);
    impl gpui::Render for Host {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
                .size_full()
                .flex()
                .child(
                    self.0
                        .update(cx, |app, cx| app.android_screen_content("first".into(), cx))
                        .into_any_element(),
                )
                .child(
                    self.0
                        .update(cx, |app, cx| {
                            app.android_screen_content("second".into(), cx)
                        })
                        .into_any_element(),
                )
        }
    }
    let root = TestConfigDir::new("nyaterm-android-screen-focus");
    let mut cx = gpui::TestAppContext::single();
    cx.update(nyaterm_ui::init_components);
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        for id in ["first", "second"] {
            let mut screen = ScreenState::new(
                cx.focus_handle(),
                super::next_generation(),
                Default::default(),
            );
            screen.physical_keys.insert(29);
            app.android.screens.insert(id.into(), screen);
        }
        assert!(app.android.screens["first"].focus != app.android.screens["second"].focus);
    });
    let host_app = app.clone();
    let (_, window) = cx.add_window_view(move |_, _| Host(host_app));
    window.update(|window, cx| {
        window.activate_window();
        let _ = window.draw(cx);
    });
    window.update(|window, cx| {
        app.update(cx, |app, cx| app.focus_android_screen("first", window, cx));
        let _ = window.draw(cx);
    });
    window.run_until_parked();
    window.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.focus_android_screen("second", window, cx);
            assert!(app.android.screens["second"].focus.is_focused(window));
        });
        let _ = window.draw(cx);
    });
    window.run_until_parked();
    window.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("synthetic paste".into()));
        app.update(cx, |app, _| {
            app.android
                .screens
                .get_mut("second")
                .unwrap()
                .physical_keys
                .extend([113, 117]);
        });
    });
    window.simulate_keystrokes("ctrl-v");
    window.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("v").unwrap(),
    });
    window.run_until_parked();
    window.update(|_, cx| {
        app.update(cx, |app, _| {
            assert!(app.android.screens["first"].physical_keys.is_empty());
            assert!(app.android.screens["second"].physical_keys.contains(&29));
            assert!(!app.android.screens["second"].physical_keys.contains(&113));
            assert!(!app.android.screens["second"].physical_keys.contains(&117));
            assert!(!app.android.screens["second"].paste_shortcut);
            app.android.release_all_inputs();
            assert!(app.android.screens["second"].physical_keys.is_empty());
        })
    });
}

fn missing_adb_connection() -> nyaterm_core::SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": "missing-android", "name": "Android", "type": "adb",
        "device": {"kind": "usb", "serial": "missing-device"}
    }))
    .unwrap()
}

fn wait_for_android_bind(
    app: &gpui::Entity<crate::features::NyaTermApp>,
    cx: &mut gpui::TestAppContext,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if app.update(cx, |app, _| app.android.pending.is_empty()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "ADB bind did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn android_bind_failure_stays_in_its_workspace_tab_and_retries_the_same_view() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::session_focus_test_window;
    for kind in [
        AndroidViewKind::Shell,
        AndroidViewKind::Files,
        AndroidViewKind::Screen,
    ] {
        let root = TestConfigDir::new("nyaterm-android-bind-workspace");
        let mut cx = gpui::TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let (request, placement) = app.update(&mut cx, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.android_adb_path = root
                .path()
                .join("missing-adb")
                .to_string_lossy()
                .into_owned();
            app.settings.replace_summary(settings);
            let connection = missing_adb_connection();
            app.connection_state
                .replace_loaded(vec![connection.clone()], Vec::new());
            app.continue_saved_connection_start(
                connection,
                SavedConnectionStartOptions {
                    android_view: Some(kind),
                    custom_name: Some("My Android".into()),
                    tab_color: Some(0x123456),
                    locked: true,
                    ..Default::default()
                },
                cx,
            );
            assert_eq!(app.session_strip_tabs(&None).len(), 2);
            let (request, pending) = app.session.start_pending_entries().next().unwrap();
            assert!(pending.preparing);
            assert_eq!(pending.android_view_kind(), Some(kind));
            (
                request.clone(),
                pending.tab_placement.unwrap().request_sequence,
            )
        });
        wait_for_android_bind(&app, &mut cx);
        app.update(&mut cx, |app, _| {
            assert!(!app.session.start_has_pending());
            assert_eq!(
                app.session.active_failed_request_id(),
                Some(request.as_str())
            );
            let failed = app.session.start_active_failed().unwrap();
            assert_eq!(
                failed.error,
                super::labels::AndroidError::Backend(
                    nyaterm_android::error::AdbError::ExecutableMissing
                )
                .message()
            );
            assert_eq!(failed.pending.custom_name.as_deref(), Some("My Android"));
            assert_eq!(app.session_strip_tabs(&None).len(), 2);
            assert!(app.android.views.is_empty());
        });
        let window = session_focus_test_window(&mut cx, &app);
        window.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.retry_failed_session_start(request.clone(), window, cx);
                assert!(!app.session.start_has_failed());
                let (next_request, pending) = app.session.start_pending_entries().next().unwrap();
                assert_ne!(next_request, &request);
                assert_eq!(pending.android_view_kind(), Some(kind));
                assert_eq!(pending.tab_placement.unwrap().request_sequence, placement);
                assert_eq!(pending.custom_name.as_deref(), Some("My Android"));
                assert_eq!(pending.tab_color, Some(0x123456));
                assert!(pending.locked);
                assert_eq!(app.session_strip_tabs(&None).len(), 2);
            })
        });
        wait_for_android_bind(&app, &mut cx);
    }
}

#[test]
fn closing_android_bind_allows_a_new_attempt_and_ignores_the_old_result() {
    use crate::features::session::SavedConnectionStartOptions;
    let root = TestConfigDir::new("nyaterm-android-bind-cancel");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
    let next_request = app.update(&mut cx, |app, cx| {
        let mut settings = app.settings.summary().clone();
        settings.android_adb_path = root
            .path()
            .join("missing-adb")
            .to_string_lossy()
            .into_owned();
        app.settings.replace_summary(settings);
        app.start_android_connection(missing_adb_connection(), Default::default(), None, cx);
        let request = app
            .session
            .start_pending_entries()
            .next()
            .unwrap()
            .0
            .clone();
        let attempt = app.session.start_attempt(&request);
        app.close_pending_session_start(request.clone(), cx);
        assert!(attempt.is_cancelled());
        assert_eq!(app.session_strip_tabs(&None).len(), 1);
        app.start_android_connection(
            missing_adb_connection(),
            SavedConnectionStartOptions::default(),
            None,
            cx,
        );
        let next_request = app
            .session
            .start_pending_entries()
            .next()
            .unwrap()
            .0
            .clone();
        assert_ne!(next_request, request);
        next_request
    });
    wait_for_android_bind(&app, &mut cx);
    app.update(&mut cx, |app, _| {
        assert_eq!(app.session.start_failed_entries().count(), 1);
        assert_eq!(
            app.session.active_failed_request_id(),
            Some(next_request.as_str())
        );
        assert_eq!(app.session_strip_tabs(&None).len(), 2);
        assert!(app.android.devices.is_empty());
    });
}

#[test]
fn android_files_handoff_consumes_preparation_when_reusing_a_valid_device() {
    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_android_session;
    let root = TestConfigDir::new("nyaterm-android-files-preparation");
    let mut cx = gpui::TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_android_session(&mut cx, root.path(), "fixture", "/sdcard/");
    app.update(&mut cx, |app, cx| {
        let config = app.android_config_for_session("fixture").unwrap();
        let connection = serde_json::from_value(serde_json::json!({
            "id":"android-device", "name":"Android", "type":"adb",
            "device":{"kind":"usb", "serial":"synthetic"}
        }))
        .unwrap();
        let mut options = app.prepare_session_start_options(SavedConnectionStartOptions {
            android_view: Some(AndroidViewKind::Files),
            ..Default::default()
        });
        options.request_id = Some(
            app.session
                .begin_saved_start_preparation(&connection, &options),
        );
        let id = app
            .open_android_view(config, AndroidViewKind::Files, options, cx)
            .unwrap();
        assert!(!app.session.start_has_pending());
        assert_eq!(app.android.kind(&id), Some(AndroidViewKind::Files));
        assert_eq!(app.session.active_id(), Some(id.as_str()));
    });
}

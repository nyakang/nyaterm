use crate::features::android::{AndroidView, DeviceLease};
use crate::features::test_support::app_with_visible_local_session;
use crate::models::session::AndroidSessionConfig;
use crate::test_support::{TestAppCleanup, TestConfigDir};
use nyaterm_android::adb::client::{AdbClient, DeviceBinding};
use nyaterm_core::models::android::{AdbDeviceSelector, AndroidScreenSettings, AndroidViewKind};
use nyaterm_files::adb::AdbFileService;
use std::sync::Arc;

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
fn android_screen_menu_uses_runtime_policy_and_focuses_screen_for_the_target_device() {
    use crate::features::shell::tab_mouse::TabActionTarget;
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
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let target = TabActionTarget::Session("shell".into());
            assert!(app.tab_target_action_enabled(&target, "android_screen"));
            app.android.pending.insert("android-device".into(), 2);
            assert!(
                !app.tab_action_policy_for("shell", "shell")
                    .unwrap()
                    .availability
                    .android_screen
            );
            app.run_tab_target_action(target.clone(), "android_screen", window, cx);
            assert_eq!(app.session.active_id(), Some("shell"));
            app.android.pending.clear();
            app.session.metadata_mut("shell").unwrap().disconnected = true;
            assert!(!app.tab_target_action_enabled(&target, "android_screen"));
            app.session.metadata_mut("shell").unwrap().disconnected = false;
            app.run_tab_target_action(target, "android_screen", window, cx);
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
        assert_eq!(app.session.start_pending_entries().count(), 0);
        assert_eq!(app.android_config_for_session("fixture").unwrap().binding.serial, "synthetic");
    });
}

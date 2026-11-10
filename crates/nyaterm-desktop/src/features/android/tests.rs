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

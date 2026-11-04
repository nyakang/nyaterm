use nyaterm_core::{
    CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions, OAuthDriveSyncSettings,
};
use nyaterm_store::StoreDomain;

use super::with_remote;
use crate::http::cloud_sync::DriveFixture;
use crate::http::cloud_sync::metrics::with_test_drive_http;
use crate::test_support::{TestConfigDir, blocking_test_store};

#[test]
fn opendal_drive_factory_persists_rotation_in_store_and_rejects_superseded_accounts() {
    for provider in ["google_drive", "onedrive", "aliyun_drive"] {
        for superseded in [false, true] {
            let fixture = DriveFixture::new();
            let dir = TestConfigDir::new("nyaterm-opendal-store-tokens");
            let store = blocking_test_store(dir.path());
            let oauth = OAuthDriveSyncSettings {
                access_token: Some("expired".into()),
                refresh_token: Some("refresh".into()),
                client_id: Some("client".into()),
                client_secret: Some("secret".into()),
                ..Default::default()
            };
            let mut settings = CloudSyncSettings {
                provider: provider.into(),
                google_drive: oauth.clone(),
                onedrive: oauth.clone(),
                ..Default::default()
            };
            settings.aliyun_drive.access_token = oauth.access_token.clone();
            settings.aliyun_drive.refresh_token = oauth.refresh_token.clone();
            settings.aliyun_drive.client_id = oauth.client_id.clone();
            settings.aliyun_drive.client_secret = oauth.client_secret.clone();
            let saved = settings.clone();
            store
                .request_fn(StoreDomain::CloudSync, move |store| {
                    store.save_cloud_sync_settings(saved)
                })
                .unwrap();
            let options = LocalCloudSyncOptions {
                config_dir: dir.path().into(),
                portable_key_path: None,
                remote_dir: dir.path().into(),
                remote_root: "space".into(),
                device_id: "test-device".into(),
                device_name: None,
                app_version: "test".into(),
                master_password: "password".into(),
                enabled: true,
            };
            let result = with_test_drive_http(&fixture.server.endpoint, || {
                with_remote(
                    &store,
                    &settings,
                    &options,
                    "connection_test",
                    |remote, _| {
                        if superseded {
                            let mut changed = settings.clone();
                            changed.google_drive.client_id = Some("other-client".into());
                            changed.onedrive.client_id = Some("other-client".into());
                            changed.aliyun_drive.client_id = Some("other-client".into());
                            store
                                .request_fn(StoreDomain::CloudSync, move |store| {
                                    store.save_cloud_sync_settings(changed)
                                })
                                .unwrap();
                        }
                        remote.create_dir("space")
                    },
                )
            });
            if superseded {
                assert!(matches!(result, Err(CloudSyncError::CredentialsChanged)));
                assert!(!fixture.server.requests.lock().unwrap().iter().any(|r| {
                    r.headers
                        .get("authorization")
                        .is_some_and(|h| h == "Bearer refreshed")
                }));
            } else {
                result.unwrap();
                let loaded = store
                    .request_fn(StoreDomain::CloudSync, |store| {
                        store.load_cloud_sync_settings()
                    })
                    .unwrap();
                let (access, refresh) = match provider {
                    "google_drive" => (
                        &loaded.google_drive.access_token,
                        &loaded.google_drive.refresh_token,
                    ),
                    "onedrive" => (
                        &loaded.onedrive.access_token,
                        &loaded.onedrive.refresh_token,
                    ),
                    _ => (
                        &loaded.aliyun_drive.access_token,
                        &loaded.aliyun_drive.refresh_token,
                    ),
                };
                assert_eq!(access.as_deref(), Some("refreshed"));
                assert_eq!(refresh.as_deref(), Some("rotated"));
            }
        }
    }
}

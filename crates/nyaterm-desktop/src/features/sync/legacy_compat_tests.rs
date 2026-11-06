//! Legacy encrypted snapshots remain interoperable across storage adapters.
use crate::http::cloud_sync::{
    CloudStorageFixture, NativeS3Remote, NativeWebdavRemote, OpenDalRemote,
};
use nyaterm_core::CloudSyncRemote;

#[test]
fn encrypted_v3_snapshots_round_trip_between_native_and_opendal_on_both_services() {
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use nyaterm_core::cloud_sync::{pull_snapshot_with_remote, push_snapshot_with_remote};
    use nyaterm_core::{CloudSyncState, LocalCloudSyncOptions};
    use nyaterm_store::StoreDomain;
    for webdav in [true, false] {
        let server = CloudStorageFixture::storage();
        let a = TestConfigDir::new("nyaterm-opendal-v3-a");
        let b = TestConfigDir::new("nyaterm-opendal-v3-b");
        let sa = blocking_test_store(a.path());
        let sb = blocking_test_store(b.path());
        let options = |dir: &TestConfigDir| LocalCloudSyncOptions {
            config_dir: dir.path().into(),
            portable_key_path: None,
            remote_dir: dir.join("unused"),
            remote_root: "space".into(),
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: None,
            app_version: "test".into(),
            master_password: "fixture-password".into(),
            enabled: true,
        };
        let oa = options(&a);
        let ob = options(&b);
        let native: Box<dyn CloudSyncRemote> = if webdav {
            Box::new(NativeWebdavRemote::new(&server.webdav()).unwrap())
        } else {
            Box::new(NativeS3Remote::new(&server.s3()).unwrap())
        };
        let remote = if webdav {
            OpenDalRemote::webdav(&server.webdav(), Default::default())
        } else {
            OpenDalRemote::s3(&server.s3(), Default::default())
        }
        .unwrap();
        sa.request_fn(StoreDomain::CloudSync, |store| {
            store.save_master_password(Some("fixture-password"))
        })
        .unwrap();
        sb.request_fn(StoreDomain::CloudSync, |store| {
            store.save_master_password(Some("fixture-password"))
        })
        .unwrap();
        let first =
            push_snapshot_with_remote(&sa, &oa, native.as_ref(), &CloudSyncState::default(), true)
                .unwrap();
        let pulled =
            pull_snapshot_with_remote(&sb, &ob, &remote, &CloudSyncState::default(), true).unwrap();
        assert_eq!(pulled.pointer, first.pointer);
        let pointer = pulled.pointer.clone().unwrap();
        let legacy_path = format!(
            "space/{}",
            nyaterm_core::cloud_sync::legacy_sync_snapshot_file(&pointer.revision_id)
        );
        let preserved = native.read_if_exists(&legacy_path).unwrap().unwrap();
        let second = push_snapshot_with_remote(&sb, &ob, &remote, &pulled.state, true).unwrap();
        pull_snapshot_with_remote(&sa, &oa, native.as_ref(), &first.state, true).unwrap();
        assert_eq!(
            native.read_if_exists(&legacy_path).unwrap().unwrap(),
            preserved
        );
        assert!(second.pointer.is_some());
    }
}

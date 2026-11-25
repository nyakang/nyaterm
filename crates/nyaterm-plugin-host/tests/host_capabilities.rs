use nyaterm_core::plugins::extension::{
    Authorization, Capability, Grant, HostRequest, Scope, TaskEvent,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_plugin_host::authority::Authority;
use nyaterm_plugin_host::host_io::LocalHost;
use nyaterm_plugin_host::tasks::TaskBroker;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn authority(capabilities: Vec<Capability>, grants: Vec<Grant>) -> Authority {
    Authority::new(
        capabilities,
        Authorization {
            plugin_id: "fixture".into(),
            source: "reviewed-digest".into(),
            grants,
        },
    )
}

fn wait(broker: &mut TaskBroker, id: u64) -> TaskEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = broker.poll(id).unwrap();
        if !matches!(event, TaskEvent::Pending) {
            return event;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn private_storage_round_trips_without_external_file_grants() {
    let root = TestTempDir::new("nyaterm-extension-storage");
    let auth = authority(vec![Capability::Storage], vec![]);
    let mut broker = TaskBroker::new(auth, Arc::new(LocalHost::new(root.path().into()).unwrap()));
    let id = broker
        .submit(HostRequest::StorageWrite {
            key: "setting".into(),
            bytes: b"value".to_vec(),
        })
        .unwrap();
    assert!(matches!(wait(&mut broker, id), TaskEvent::Completed { .. }));
    let id = broker
        .submit(HostRequest::StorageRead {
            key: "setting".into(),
        })
        .unwrap();
    assert!(
        matches!(wait(&mut broker, id), TaskEvent::Completed { bytes, .. } if bytes == b"value")
    );
}

#[test]
fn local_file_access_is_scoped_and_rejects_parent_traversal() {
    let root = TestTempDir::new("nyaterm-extension-files");
    std::fs::create_dir_all(root.path()).unwrap();
    std::fs::write(root.join("file"), b"0123456789").unwrap();
    let directory = root.path().to_string_lossy().into_owned();
    let auth = authority(
        vec![Capability::LocalFilesRead],
        vec![Grant {
            capability: Capability::LocalFilesRead,
            scope: Scope::Directory {
                path: directory.clone(),
            },
        }],
    );
    let mut broker = TaskBroker::new(
        auth,
        Arc::new(LocalHost::new(root.join("private")).unwrap()),
    );
    let request = |path: &str| HostRequest::LocalRead {
        root: directory.clone(),
        path: path.into(),
        offset: 3,
        length: 3,
    };
    let id = broker.submit(request("file")).unwrap();
    assert!(matches!(wait(&mut broker, id), TaskEvent::Completed { bytes, .. } if bytes == b"345"));
    let id = broker.submit(request("../outside")).unwrap();
    assert!(matches!(wait(&mut broker, id), TaskEvent::Failed { .. }));
    assert!(
        broker
            .submit(HostRequest::LocalRead {
                root: root.join("other").to_string_lossy().into_owned(),
                path: "file".into(),
                offset: 0,
                length: 1
            })
            .is_err()
    );
}

#[test]
fn revocation_blocks_completed_results_and_future_tasks() {
    let root = TestTempDir::new("nyaterm-extension-revoke");
    let auth = authority(vec![Capability::Storage], vec![]);
    let mut broker = TaskBroker::new(
        auth.clone(),
        Arc::new(LocalHost::new(root.path().into()).unwrap()),
    );
    let id = broker
        .submit(HostRequest::StorageWrite {
            key: "value".into(),
            bytes: vec![],
        })
        .unwrap();
    auth.revoke();
    assert!(matches!(broker.poll(id).unwrap(), TaskEvent::Cancelled));
    assert!(
        broker
            .submit(HostRequest::StorageRead {
                key: "value".into()
            })
            .is_err()
    );
}

#[test]
fn completed_results_count_toward_admission_until_consumed() {
    let root = TestTempDir::new("nyaterm-extension-admission");
    let auth = authority(vec![Capability::Storage], vec![]);
    let mut broker = TaskBroker::new(auth, Arc::new(LocalHost::new(root.path().into()).unwrap()));
    let mut ids = Vec::new();
    for n in 0..16 {
        ids.push(
            broker
                .submit(HostRequest::StorageWrite {
                    key: format!("key-{n}"),
                    bytes: vec![],
                })
                .unwrap(),
        );
    }
    assert!(
        broker
            .submit(HostRequest::StorageRead {
                key: "key-0".into()
            })
            .is_err()
    );
    for id in ids {
        assert!(matches!(wait(&mut broker, id), TaskEvent::Completed { .. }));
    }
    assert!(
        broker
            .submit(HostRequest::StorageRead {
                key: "key-0".into()
            })
            .is_ok()
    );
}

#[test]
fn signed_archive_digest_and_trust_key_must_both_match() {
    use nyaterm_core::plugins::catalog::{Artifact, Release};
    use sha2::{Digest, Sha256};
    let bytes = include_bytes!("fixtures/native/component.json");
    let mut release = Release {
        version: "1.0.0".into(),
        host_version: ">=2.0.8".into(),
        api_version: "2.0.0".into(),
        capabilities: vec![],
        artifact: Artifact {
            url: "https://example.com/test.zip".into(),
            sha256: hex::encode(Sha256::digest(bytes)),
            size: bytes.len() as u64,
            signature: include_str!("fixtures/native/component.minisig").into(),
        },
    };
    let key = include_str!("fixtures/native/public-key.txt").trim();
    assert!(nyaterm_plugin_host::catalog::verify_archive(bytes, &release, key).is_ok());
    assert!(nyaterm_plugin_host::catalog::verify_archive(bytes, &release, "invalid").is_err());
    release.artifact.sha256 = "0".repeat(64);
    assert!(nyaterm_plugin_host::catalog::verify_archive(bytes, &release, key).is_err());
}

use std::sync::{Arc, Mutex};

use nyaterm_core::cloud_sync::credentials::DriveProvider;
use nyaterm_core::cloud_sync::io::CloudObjectRead;
use nyaterm_core::{CloudSyncError, CloudSyncRemote, OAuthDriveSyncSettings};

use super::OpenDalDriveRemote;
use crate::CloudTokenSink;

use crate::test_support::DriveFixture;

pub(crate) fn remote(fixture: &DriveFixture, provider: DriveProvider) -> OpenDalDriveRemote {
    configured(fixture, provider, "access", Arc::new(|_| Ok(())))
}
fn configured(
    fixture: &DriveFixture,
    provider: DriveProvider,
    token: &str,
    sink: CloudTokenSink,
) -> OpenDalDriveRemote {
    let settings = OAuthDriveSyncSettings {
        root: "apps/nyaterm".into(),
        access_token: Some(token.into()),
        refresh_token: Some("refresh".into()),
        client_id: Some("client".into()),
        client_secret: Some("secret".into()),
    };
    let mut remote =
        OpenDalDriveRemote::new(provider, &settings, "default", sink, Default::default()).unwrap();
    remote.session.transport.test_endpoint = Some(fixture.server.endpoint.clone());
    remote
}
const PROVIDERS: [DriveProvider; 3] = [
    DriveProvider::Google,
    DriveProvider::OneDrive,
    DriveProvider::Aliyun,
];

#[test]
fn drive_services_cover_remote_methods_complete_pagination_and_safe_publication() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        remote.create_dir("space/sync/snapshots").unwrap();
        assert!(remote.read_if_exists("space/absent").unwrap().is_none());
        for index in 0..5 {
            remote
                .write(
                    &format!("space/sync/snapshots/{index}.enc"),
                    format!("encrypted-{index}").as_bytes(),
                )
                .unwrap();
        }
        assert_eq!(remote.list_files("space/sync/snapshots").unwrap().len(), 5);
        assert_eq!(
            remote.list_metadata("space/sync/snapshots").unwrap().len(),
            5
        );
        let conditional = remote
            .read_conditional("space/sync/snapshots/0.enc", None)
            .unwrap();
        let CloudObjectRead::Modified { bytes, etag } = conditional else {
            panic!("expected content");
        };
        assert_eq!(bytes, b"encrypted-0");
        if provider == DriveProvider::OneDrive {
            assert!(etag.is_some());
        }
        if let Some(etag) = etag {
            assert!(matches!(
                remote
                    .read_conditional("space/sync/snapshots/0.enc", Some(&etag))
                    .unwrap(),
                CloudObjectRead::NotModified
            ));
        }
        let path = "space/sync/v4/writers/a/1-hash.bin";
        remote
            .write_sync_file(path, b"complete-publication")
            .unwrap();
        remote
            .write_sync_file(path, b"complete-publication")
            .unwrap();
        assert!(remote.write_sync_file(path, b"other").is_err());
        assert_eq!(
            remote.read_if_exists(path).unwrap().unwrap(),
            b"complete-publication"
        );
        assert_eq!(
            remote.list_sync_files("space/sync/v4/writers/a").unwrap(),
            vec![path]
        );
        assert!(remote.delete("space/sync/v4/writers/a").is_err());
        remote.delete("space/sync/snapshots/0.enc").unwrap();
        assert!(
            remote
                .read_if_exists("space/sync/snapshots/0.enc")
                .unwrap()
                .is_none()
        );
        assert!(remote.session.transport.drive.is_some());
        let requests = fixture.server.requests.lock().unwrap();
        remote.session.transport.record(|stats| {
            assert_eq!(stats.requests, requests.len() as u64);
            assert_eq!(
                stats.uploaded_bytes,
                requests.iter().map(|r| r.body.len() as u64).sum::<u64>()
            );
            assert!(stats.downloaded_bytes > 0);
        });
    }
}

#[test]
fn drive_refresh_persists_before_use_and_preserves_rotation_and_omitted_refresh() {
    for provider in PROVIDERS {
        for rotated in [true, false] {
            let fixture = DriveFixture::new();
            fixture.state.lock().unwrap().rotated_refresh = rotated;
            let observed = Arc::new(Mutex::new(Vec::new()));
            let updates = observed.clone();
            let requests = fixture.server.requests.clone();
            let sink: CloudTokenSink = Arc::new(move |update| {
                assert!(!requests.lock().unwrap().iter().any(|r| {
                    r.headers
                        .get("authorization")
                        .is_some_and(|h| h == "Bearer refreshed")
                }));
                updates.lock().unwrap().push(update);
                Ok(())
            });
            let remote = configured(&fixture, provider, "expired", sink);
            remote.create_dir("space").unwrap();
            let updates = observed.lock().unwrap();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].provider, provider);
            assert_eq!(updates[0].expected_access_token.as_deref(), Some("expired"));
            assert_eq!(
                updates[0].expected_refresh_token.as_deref(),
                Some("refresh")
            );
            assert_eq!(
                updates[0].refresh_token.as_deref(),
                rotated.then_some("rotated")
            );
        }
    }
}

#[test]
fn drive_failed_or_superseded_token_persistence_stops_before_authorized_replay() {
    for provider in PROVIDERS {
        for superseded in [false, true] {
            let fixture = DriveFixture::new();
            let sink: CloudTokenSink = Arc::new(move |_| {
                Err(if superseded {
                    CloudSyncError::CredentialsChanged
                } else {
                    CloudSyncError::CredentialPersistence
                })
            });
            let remote = configured(&fixture, provider, "expired", sink);
            let error = remote.create_dir("space").unwrap_err();
            assert!(matches!(
                (&error, superseded),
                (CloudSyncError::CredentialsChanged, true)
                    | (CloudSyncError::CredentialPersistence, false)
            ));
            assert!(!fixture.server.requests.lock().unwrap().iter().any(|r| {
                r.headers
                    .get("authorization")
                    .is_some_and(|h| h == "Bearer refreshed")
            }));
            assert_eq!(fixture.state.lock().unwrap().nodes.len(), 1);
        }
    }
}

#[test]
fn drive_publication_recovers_lost_move_reply_without_replacing_existing_objects() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        remote.create_dir("space").unwrap();
        fixture.state.lock().unwrap().lose_move_reply = true;
        assert!(
            remote
                .write_sync_file("space/pending.bin", b"pending")
                .is_err()
        );
        remote
            .write_sync_file("space/pending.bin", b"pending")
            .unwrap();
        let state = fixture.state.lock().unwrap();
        assert_eq!(
            state
                .nodes
                .values()
                .filter(|n| n.name == "pending.bin" && n.complete && n.bytes == b"pending")
                .count(),
            1
        );
    }
}

#[test]
fn drive_listings_reject_cycles_and_google_rejects_duplicates_and_incomplete_search() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        remote.create_dir("space").unwrap();
        fixture.state.lock().unwrap().cycle = true;
        assert!(remote.list_files("space").is_err());
    }
    let fixture = DriveFixture::new();
    let remote = remote(&fixture, DriveProvider::Google);
    remote.create_dir("space").unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.insert("root", "apps", false, Vec::new());
        state.insert("root", "apps", false, Vec::new());
    }
    assert!(remote.list_files("space").is_err());
    fixture.state.lock().unwrap().incomplete = true;
    assert!(remote.read_if_exists("space/object").is_err());
}

#[test]
fn aliyun_refuses_destructive_overwrites_and_keeps_old_bytes() {
    let fixture = DriveFixture::new();
    let remote = remote(&fixture, DriveProvider::Aliyun);
    remote.write("space/object", b"old-encrypted-data").unwrap();
    assert!(remote.write("space/object", b"new-encrypted-data").is_err());
    assert_eq!(
        remote.read_if_exists("space/object").unwrap().unwrap(),
        b"old-encrypted-data"
    );
    assert!(
        !fixture
            .server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.target.contains("/delete"))
    );
}

#[test]
fn drive_upload_boundaries_include_empty_four_mib_and_sixty_four_mib() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        for size in [0, 4 * 1024 * 1024, 4 * 1024 * 1024 + 1, 64 * 1024 * 1024] {
            let bytes = vec![b'x'; size];
            let path = format!("space/object-{size}");
            remote.write_sync_file(&path, &bytes).unwrap();
            assert_eq!(remote.read_if_exists(&path).unwrap().unwrap(), bytes);
            fixture.server.requests.lock().unwrap().clear();
        }
    }
}

#[test]
fn onedrive_final_accepted_fragment_is_not_a_completed_publication() {
    let fixture = DriveFixture::new();
    let remote = remote(&fixture, DriveProvider::OneDrive);
    fixture.state.lock().unwrap().final_chunk_status = 202;
    assert!(
        remote
            .write_sync_file("space/incomplete", &vec![b'x'; 4 * 1024 * 1024 + 1])
            .is_err()
    );
    assert!(remote.read_if_exists("space/incomplete").unwrap().is_none());
}

#[test]
fn native_and_opendal_drive_services_read_each_others_existing_formats() {
    use crate::metrics::with_test_drive_http;
    use crate::{NativeAliyunDriveRemote, NativeGoogleDriveRemote, NativeOneDriveRemote};
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        with_test_drive_http(&fixture.server.endpoint, || {
            let settings = OAuthDriveSyncSettings {
                root: "apps/nyaterm".into(),
                access_token: Some("access".into()),
                ..Default::default()
            };
            let native: Box<dyn CloudSyncRemote> = match provider {
                DriveProvider::Google => Box::new(NativeGoogleDriveRemote::new(&settings).unwrap()),
                DriveProvider::OneDrive => Box::new(NativeOneDriveRemote::new(&settings).unwrap()),
                DriveProvider::Aliyun => Box::new(
                    NativeAliyunDriveRemote::new(&nyaterm_core::AliyunDriveSyncSettings {
                        root: settings.root.clone(),
                        access_token: settings.access_token.clone(),
                        ..Default::default()
                    })
                    .unwrap(),
                ),
            };
            let remote = remote(&fixture, provider);
            native
                .write(
                    "space/sync/snapshots/native.enc",
                    b"native-encrypted-fixture",
                )
                .unwrap();
            assert_eq!(
                remote
                    .read_if_exists("space/sync/snapshots/native.enc")
                    .unwrap()
                    .unwrap(),
                b"native-encrypted-fixture"
            );
            remote
                .write_sync_file(
                    "space/sync/snapshots/opendal.enc",
                    b"opendal-encrypted-fixture",
                )
                .unwrap();
            assert_eq!(
                native
                    .read_if_exists("space/sync/snapshots/opendal.enc")
                    .unwrap()
                    .unwrap(),
                b"opendal-encrypted-fixture"
            );
            let mut old = native.list_files("space/sync/snapshots").unwrap();
            old.sort();
            let mut new = remote.list_files("space/sync/snapshots").unwrap();
            new.sort();
            assert_eq!(old, new);
        });
    }
}

#[test]
fn drive_auth_failures_permissions_and_rate_limits_keep_typed_redacted_errors() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        fixture.state.lock().unwrap().refresh_status = 400;
        let remote = configured(
            &fixture,
            provider,
            "expired",
            Arc::new(|_| panic!("failed refresh persisted")),
        );
        let error = remote.create_dir("space").unwrap_err();
        assert!(matches!(
            error,
            CloudSyncError::Authentication { status: 400, .. }
        ));
        assert!(!error.to_string().contains("must never escape"));
        for status in [403, 429] {
            let fixture = DriveFixture::new();
            fixture.state.lock().unwrap().api_status = Some(status);
            let remote = configured(
                &fixture,
                provider,
                "access",
                Arc::new(|_| panic!("permission/rate-limit refreshed")),
            );
            let error = remote.list_files("space").unwrap_err();
            assert!(
                matches!(error, CloudSyncError::Http { status: actual, retry_after_seconds:Some(17), .. } if actual == status)
            );
            assert!(!error.to_string().contains("secret body"));
            assert_eq!(fixture.server.requests.lock().unwrap().len(), 1);
        }
    }
}

#[test]
fn drive_missing_access_token_refreshes_once_before_any_api_request() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = configured(
            &fixture,
            provider,
            "",
            Arc::new(|update| {
                assert_eq!(update.expected_access_token.as_deref(), Some(""));
                Ok(())
            }),
        );
        remote.create_dir("space").unwrap();
        let requests = fixture.server.requests.lock().unwrap();
        assert!(!requests[0].headers.contains_key("authorization"));
        assert!(requests.iter().skip(1).all(|r| {
            r.headers
                .get("authorization")
                .is_some_and(|h| h == "Bearer refreshed")
        }));
    }
}

#[test]
fn drive_failed_upload_is_not_replayed_or_published_and_explicit_recovery_is_safe() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        remote.create_dir("space").unwrap();
        fixture.state.lock().unwrap().mutation_failure = Some(503);
        assert!(
            remote
                .write_sync_file("space/failed.bin", b"complete")
                .is_err()
        );
        assert!(remote.read_if_exists("space/failed.bin").unwrap().is_none());
        let requests = fixture.server.requests.lock().unwrap();
        let uploads = requests
            .iter()
            .filter(|r| {
                r.target.contains("upload/drive/")
                    || r.target.contains("uploads.example/")
                    || r.target.contains(":/content")
            })
            .count();
        assert_eq!(uploads, 1);
        drop(requests);
        remote
            .write_sync_file("space/failed.bin", b"complete")
            .unwrap();
        assert_eq!(
            remote.read_if_exists("space/failed.bin").unwrap().unwrap(),
            b"complete"
        );
    }
}

#[test]
fn drive_transport_rejects_untrusted_bearer_targets_and_strips_auth_on_download_redirects() {
    use crate::opendal::drive_http::request;
    use opendal::HttpTransport;
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        let remote = remote(&fixture, provider);
        for authenticated in [true, false] {
            let error = remote.run("read", move |s| async move {
                let mut req = request("GET", "https://attacker.example/steal", None)?;
                if !authenticated {
                    req.headers_mut().remove("authorization");
                }
                s.transport.fetch(req).await.map(|_| ())
            });
            assert!(error.is_err());
        }
        assert!(fixture.server.requests.lock().unwrap().is_empty());
    }
    let fixture = DriveFixture::new();
    let remote = remote(&fixture, DriveProvider::OneDrive);
    remote.write("space/object", b"download").unwrap();
    fixture.state.lock().unwrap().redirect_downloads = true;
    assert_eq!(
        remote.read_if_exists("space/object").unwrap().unwrap(),
        b"download"
    );
    assert!(
        fixture
            .server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.target.starts_with("/downloads.example/")
                && !r.headers.contains_key("authorization"))
    );
}

#[test]
fn onedrive_listing_rejects_cross_origin_next_link_before_sending_bearer() {
    let fixture = DriveFixture::new();
    let remote = remote(&fixture, DriveProvider::OneDrive);
    remote.create_dir("space").unwrap();
    fixture.state.lock().unwrap().bad_next_link = true;
    assert!(remote.list_files("space").is_err());
    assert!(
        !fixture
            .server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.target.contains("attacker.example"))
    );
}

#[test]
fn signed_upload_on_api_origin_does_not_inherit_bearer_credentials() {
    use crate::test_support::http::{Reply, Server};
    use opendal::{Buffer, HttpTransport};
    use serde_json::json;
    let fixture = DriveFixture::new();
    let mut remote = remote(&fixture, DriveProvider::OneDrive);
    let server = Server::new(|request| {
        if request.target.ends_with("/issue-session") {
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("Bearer access")
            );
            Reply::new(200, serde_json::to_vec(&json!({"uploadUrl":"https://graph.microsoft.com/v1.0/signed-upload?sig=private"})).unwrap())
        } else {
            assert!(request.target.contains("/signed-upload?sig=private"));
            assert!(!request.headers.contains_key("authorization"));
            Reply::new(200, Vec::new())
        }
    });
    remote.session.transport.test_endpoint = Some(server.endpoint.clone());
    remote
        .run("write", |s| async move {
            s.api(
                "GET",
                "https://graph.microsoft.com/v1.0/issue-session",
                None,
            )
            .await?;
            let req =
                http::Request::put("https://graph.microsoft.com/v1.0/signed-upload?sig=private")
                    .body(Buffer::from("payload"))
                    .unwrap();
            s.transport.fetch(req).await.map(|_| ())
        })
        .unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[test]
fn token_installation_has_no_cancellation_point_after_successful_persistence() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    let fixture = DriveFixture::new();
    let saved = Arc::new(AtomicUsize::new(0));
    let observed = saved.clone();
    let remote = configured(
        &fixture,
        DriveProvider::OneDrive,
        "expired",
        Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            // Make the surrounding async deadline expire inside the synchronous
            // persistence/install critical section. The refreshed token must survive.
            std::thread::sleep(Duration::from_millis(1200));
            Ok(())
        }),
    );
    remote
        .run("read", |s| async move {
            let _ = tokio::time::timeout(Duration::from_millis(1000), s.lookup_full("")).await;
            Ok(())
        })
        .unwrap();
    assert_eq!(saved.load(Ordering::SeqCst), 1);
    remote.create_dir("space").unwrap();
    assert_eq!(saved.load(Ordering::SeqCst), 1);
}

#[test]
fn drive_refresh_service_failure_is_not_replayed_as_a_read_retry() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        fixture.state.lock().unwrap().refresh_status = 503;
        let remote = configured(
            &fixture,
            provider,
            "expired",
            Arc::new(|_| panic!("failed refresh persisted")),
        );
        let error = remote.read_if_exists("space/object").unwrap_err();
        assert!(matches!(error, CloudSyncError::Http { status: 503, .. }));
        assert_eq!(fixture.server.requests.lock().unwrap().len(), 2);
    }
}

#[test]
fn oauth_rate_limits_remain_rate_limits_instead_of_authentication_failures() {
    for provider in PROVIDERS {
        let fixture = DriveFixture::new();
        fixture.state.lock().unwrap().refresh_status = 429;
        let remote = configured(
            &fixture,
            provider,
            "expired",
            Arc::new(|_| panic!("rate limited refresh persisted")),
        );
        assert!(matches!(
            remote.read_if_exists("space/object"),
            Err(CloudSyncError::Http { status: 429, .. })
        ));
        assert_eq!(fixture.server.requests.lock().unwrap().len(), 2);
    }
}

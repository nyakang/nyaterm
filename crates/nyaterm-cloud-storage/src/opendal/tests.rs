use nyaterm_core::CloudSyncRemote;

use super::{OpenDalRemote, endpoint, path};
use crate::{NativeS3Remote, NativeWebdavRemote};
use nyaterm_core::cloud_sync::io::{CloudObjectRead, CloudSyncIoStats};
use nyaterm_core::cloud_sync::metadata_cache::{CachedMetadataRemote, CloudMetadataCache};
use std::sync::{Arc, Mutex};

use crate::test_support::http::{Reply, Server};

#[test]
fn rejects_unsafe_configuration_and_paths_without_echoing_secrets() {
    assert!(
        endpoint(
            "https://user:password@example.org/dav?token=secret",
            "webdav"
        )
        .is_err()
    );
    for value in [
        "sync/../escape",
        "sync/./escape",
        "sync\\escape",
        "sync//escape",
        "sync/\0",
    ] {
        assert!(path(value).is_err());
    }
    assert_eq!(path(" /sync/v4/ ").unwrap(), "sync/v4");
}

fn assert_remote_contract(_: &impl CloudSyncRemote) {}

#[test]
fn adapter_keeps_the_existing_sync_remote_facade() {
    let remote = OpenDalRemote::webdav(
        &nyaterm_core::WebdavSyncSettings {
            endpoint: "https://example.org/dav".into(),
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    assert_remote_contract(&remote);
}

#[test]
fn webdav_native_and_opendal_share_paths_bytes_listings_and_safe_publication() {
    let server = Server::storage();
    let settings = server.webdav();
    let native = NativeWebdavRemote::new(&settings).unwrap();
    let remote = OpenDalRemote::webdav(&settings, Default::default()).unwrap();
    let object = "nyaterm/sync/snapshots/old.redb.enc";
    native.write(object, b"native-encrypted-fixture").unwrap();
    assert_eq!(
        remote.read_if_exists(object).unwrap().unwrap(),
        b"native-encrypted-fixture"
    );
    remote.write(object, b"opendal-encrypted-fixture").unwrap();
    assert_eq!(
        native.read_if_exists(object).unwrap().unwrap(),
        b"opendal-encrypted-fixture"
    );
    assert_eq!(
        native.list_files("nyaterm/sync/snapshots").unwrap(),
        remote.list_files("nyaterm/sync/snapshots").unwrap()
    );
    let publication = "nyaterm/sync/v4/writers/a/1-digest.bin";
    remote
        .write_sync_file(publication, b"complete-publication")
        .unwrap();
    remote
        .write_sync_file(publication, b"complete-publication")
        .unwrap();
    assert!(remote.write_sync_file(publication, b"different").is_err());
    assert_eq!(
        native.read_if_exists(publication).unwrap().unwrap(),
        b"complete-publication"
    );
    assert_eq!(
        native.list_sync_files("nyaterm/sync/v4/writers/a").unwrap(),
        vec![publication]
    );
    assert_eq!(
        remote.list_sync_files("nyaterm/sync/v4/writers/a").unwrap(),
        vec![publication]
    );
    remote.delete(object).unwrap();
    assert!(native.read_if_exists(object).unwrap().is_none());
    assert!(server.requests.lock().unwrap().iter().all(|request| {
        request.target.starts_with("/dav/apps/nyaterm/")
            || request.method == "MKCOL"
            || request.method == "PROPFIND"
    }));
}

#[test]
fn s3_native_and_opendal_preserve_root_pagination_metadata_and_session_credentials() {
    let server = Server::storage();
    let settings = server.s3();
    let native = NativeS3Remote::new(&settings).unwrap();
    let remote = OpenDalRemote::s3(&settings, Default::default()).unwrap();
    for name in [
        "a.redb.enc",
        "b.redb.enc",
        "c.redb.enc",
        "d.redb.enc",
        "e.redb.enc",
    ] {
        native
            .write(&format!("space/sync/snapshots/{name}"), name.as_bytes())
            .unwrap();
    }
    native
        .write("space/sync/snapshots-other/unrelated", b"unrelated")
        .unwrap();
    let old = native.list_metadata("space/sync/snapshots/").unwrap();
    let new = remote.list_metadata("space/sync/snapshots/").unwrap();
    assert!(old == new);
    assert_eq!(new.len(), 5);
    for object in &new {
        assert_eq!(
            remote.read_if_exists(&object.path).unwrap(),
            native.read_if_exists(&object.path).unwrap()
        );
    }
    let publication = "space/sync/v4/writers/a/1-digest.bin";
    remote
        .write_sync_file(publication, b"immutable-publication")
        .unwrap();
    assert_eq!(
        native.read_if_exists(publication).unwrap().unwrap(),
        b"immutable-publication"
    );
    assert!(remote.write_sync_file(publication, b"different").is_err());
    remote.delete(publication).unwrap();
    remote.delete(publication).unwrap();
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().all(|request| {
        request.headers.get("authorization").is_some_and(|value| {
            value.starts_with("AWS4-HMAC-SHA256") && value.contains("/us-east-1/s3/aws4_request")
        })
    }));
    assert!(requests.iter().all(|request| {
        request
            .headers
            .get("x-amz-security-token")
            .is_some_and(|value| value == "fixture-session")
    }));
    assert!(requests.iter().any(|request| {
        request.method == "PUT"
            && request
                .headers
                .get("if-none-match")
                .is_some_and(|value| value == "*")
    }));
}

#[test]
fn conditional_cache_survives_backend_rebuilds_without_head_races_and_counts_actual_io() {
    let server = Server::storage();
    let settings = server.s3();
    let native = NativeS3Remote::new(&settings).unwrap();
    let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
    let cache = Mutex::new(CloudMetadataCache::default());
    let path = "space/sync/latest.redb";
    native.write(path, b"old").unwrap();
    for _ in 0..2 {
        let source = OpenDalRemote::s3(&settings, stats.clone()).unwrap();
        let cached = CachedMetadataRemote::new(&source, &cache, "space");
        assert_eq!(cached.read_if_exists(path).unwrap().unwrap(), b"old");
    }
    native.write(path, b"new").unwrap();
    let source = OpenDalRemote::s3(&settings, stats.clone()).unwrap();
    let cached = CachedMetadataRemote::new(&source, &cache, "space");
    assert_eq!(cached.read_if_exists(path).unwrap().unwrap(), b"new");
    native.delete(path).unwrap();
    assert!(cached.read_if_exists(path).unwrap().is_none());
    let stats = stats.lock().unwrap();
    assert_eq!(stats.requests, 4);
    assert_eq!(stats.downloaded_bytes, 6);
    assert_eq!(stats.not_modified, 1);
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.method == "HEAD")
    );
}

#[test]
fn conditional_fallback_only_retries_a_read_rejected_by_a_compatible_service() {
    for status in [400, 405, 501] {
        let server = Server::new(move |request| {
            if request.headers.contains_key("if-none-match") {
                Reply::new(status, "")
            } else {
                Reply::new(200, "body").header("ETag", "\"new\"")
            }
        });
        let remote = OpenDalRemote::s3(&server.s3(), Default::default()).unwrap();
        match remote
            .read_conditional("space/latest", Some("\"old\""))
            .unwrap()
        {
            CloudObjectRead::Modified { bytes, etag } => {
                assert_eq!(bytes, b"body");
                assert_eq!(etag.as_deref(), Some("\"new\""));
            }
            _ => panic!("expected fallback body"),
        }
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }
}

#[test]
fn webdav_digest_and_redirects_preserve_methods_headers_and_upload_bodies() {
    use super::transport::Transport;
    use opendal::{Buffer, HttpTransport};
    for status in [301, 302, 307, 308] {
        let server = Server::new(move |request| {
            if !request
                .headers
                .get("authorization")
                .is_some_and(|value| value.starts_with("Digest "))
            {
                return Reply::new(401, "challenge").header(
                    "WWW-Authenticate",
                    "Digest realm=\"fixture\", nonce=\"fixture\", qop=\"auth\"",
                );
            }
            if request.target == "/dav/old" {
                Reply::new(status, "redirect").header("Location", "/dav/new")
            } else {
                Reply::new(200, "")
            }
        });
        let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
        let transport = Transport::new(stats.clone(), Some(&server.webdav())).unwrap();
        let worker = super::worker::Worker::new().unwrap();
        let url = format!("{}/dav/old", server.endpoint);
        worker
            .run(async move {
                transport
                    .fetch(
                        http::Request::builder()
                            .method("PUT")
                            .uri(url)
                            .header("Content-Type", "application/octet-stream")
                            .body(Buffer::from("upload"))
                            .unwrap(),
                    )
                    .await
                    .map_err(|error| super::error::map_error("webdav", "write", error))?;
                Ok(())
            })
            .unwrap();
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|request| request.method == "PUT" && request.body == b"upload")
        );
        assert!(requests[2].headers["authorization"].contains("uri=\"/dav/new\""));
        assert_eq!(stats.lock().unwrap().requests, 3);
        assert_eq!(stats.lock().unwrap().uploaded_bytes, 18);
    }
}

#[test]
fn cross_origin_redirect_and_303_mutation_never_replay_credentials_or_bodies() {
    use opendal::{Buffer, HttpTransport};
    for status in [301, 302, 303, 307, 308] {
        let target = Server::new(|_| panic!("credentials escaped origin"));
        let location = format!("{}/stolen", target.endpoint);
        let server = Server::new(move |_| Reply::new(status, "").header("Location", &location));
        let transport =
            super::transport::Transport::new(Default::default(), Some(&server.webdav())).unwrap();
        let worker = super::worker::Worker::new().unwrap();
        let url = format!("{}/dav/old", server.endpoint);
        assert!(
            worker
                .run(async move {
                    transport
                        .fetch(
                            http::Request::builder()
                                .method("PUT")
                                .uri(url)
                                .body(Buffer::from("upload"))
                                .unwrap(),
                        )
                        .await
                        .map_err(|error| super::error::map_error("webdav", "write", error))?;
                    Ok(())
                })
                .is_err()
        );
        assert!(target.requests.lock().unwrap().is_empty());
    }
}

#[test]
fn webdav_cross_origin_download_redirect_is_followed_without_credentials() {
    let downloader = Server::new(|_| Reply::new(200, "remote-object"));
    let location = format!("{}/download", downloader.endpoint);
    let source = Server::new(move |_| Reply::new(302, "").header("Location", &location));
    let remote = OpenDalRemote::webdav(&source.webdav(), Default::default()).unwrap();
    assert_eq!(
        remote
            .read_if_exists("sync/latest.redb")
            .unwrap()
            .as_deref(),
        Some(b"remote-object".as_slice())
    );
    let received = downloader.requests.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert!(!received[0].headers.contains_key("authorization"));
}

#[test]
fn both_webdav_adapters_isolate_cross_origin_challenges_and_restore_only_origin_credentials() {
    for native in [false, true] {
        for digest in [false, true] {
            for challenge in [false, true] {
                let origin = Arc::new(Mutex::new(String::new()));
                let origin_url = origin.clone();
                let third = Server::new(move |_| {
                    Reply::new(307, "").header(
                        "Location",
                        &format!("{}/returned", origin_url.lock().unwrap()),
                    )
                });
                let third_url = third.endpoint.clone();
                let target = Server::new(move |_| {
                    if challenge {
                        Reply::new(401, "").header(
                            "WWW-Authenticate",
                            "Digest realm=\"foreign\", nonce=\"foreign\", qop=\"auth\"",
                        )
                    } else {
                        Reply::new(302, "").header("Location", &format!("{third_url}/next"))
                    }
                });
                let location = format!("{}/foreign", target.endpoint);
                let source = Server::new(move |request| {
                    if digest
                        && !request
                            .headers
                            .get("authorization")
                            .is_some_and(|value| value.starts_with("Digest "))
                    {
                        return Reply::new(401, "").header(
                            "WWW-Authenticate",
                            "Digest realm=\"origin\", nonce=\"origin\", qop=\"auth\"",
                        );
                    }
                    if request.target == "/returned" {
                        Reply::new(200, "object")
                    } else {
                        Reply::new(302, "").header("Location", &location)
                    }
                });
                *origin.lock().unwrap() = source.endpoint.clone();
                let remote: Box<dyn CloudSyncRemote> = if native {
                    Box::new(NativeWebdavRemote::new(&source.webdav()).unwrap())
                } else {
                    Box::new(OpenDalRemote::webdav(&source.webdav(), Default::default()).unwrap())
                };
                let result = remote.read_if_exists("sync/latest.redb");
                if challenge {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result.unwrap().as_deref(), Some(b"object".as_slice()));
                }
                let foreign = target.requests.lock().unwrap();
                assert_eq!(
                    foreign.len(),
                    1,
                    "a foreign 401 must not trigger an authentication retry"
                );
                assert!(!foreign[0].headers.contains_key("authorization"));
                assert!(foreign[0].body.is_empty());
                let onward = third.requests.lock().unwrap();
                assert_eq!(onward.len(), usize::from(!challenge));
                assert!(
                    onward
                        .iter()
                        .all(|r| !r.headers.contains_key("authorization"))
                );
                let origin = source.requests.lock().unwrap();
                assert_eq!(
                    origin.len(),
                    1 + usize::from(digest) + usize::from(!challenge)
                );
                if !challenge {
                    let auth = &origin.last().unwrap().headers["authorization"];
                    assert!(auth.starts_with(if digest { "Digest " } else { "Basic " }));
                    if digest {
                        assert!(auth.contains("uri=\"/returned\""));
                    }
                }
            }
        }
    }
}

#[test]
fn webdav_head_redirects_strip_credentials_and_mutations_never_leave_the_origin() {
    use opendal::{Buffer, HttpTransport};
    for method in ["HEAD", "PUT", "DELETE", "MKCOL", "MOVE", "PROPFIND"] {
        let target = Server::new(|_| Reply::new(200, ""));
        let location = format!("{}/object", target.endpoint);
        let source = Server::new(move |_| Reply::new(307, "").header("Location", &location));
        let transport =
            super::transport::Transport::new(Default::default(), Some(&source.webdav())).unwrap();
        let url = format!("{}/original", source.endpoint);
        super::worker::Worker::new()
            .unwrap()
            .run(async move {
                let result = transport
                    .fetch(
                        http::Request::builder()
                            .method(method)
                            .uri(url)
                            .body(Buffer::new())
                            .unwrap(),
                    )
                    .await;
                if method == "HEAD" {
                    assert!(result.unwrap().status().is_success());
                }
                // Raw transport returns a controlled 307 error for unsupported replay.
                else {
                    assert!(result.is_err());
                }
                Ok(())
            })
            .unwrap();
        let requests = target.requests.lock().unwrap();
        assert_eq!(requests.len(), usize::from(method == "HEAD"));
        assert!(
            requests
                .iter()
                .all(|r| r.method == "HEAD" && !r.headers.contains_key("authorization"))
        );
    }
}

#[test]
fn errors_keep_status_retry_after_and_redact_server_data_without_replaying_mutations() {
    use nyaterm_core::CloudSyncError;
    for status in [401, 403, 409, 429, 503] {
        let server = Server::new(move |_| {
            Reply::new(status, "PRIVATE-TOKEN PRIVATE-BODY").header("Retry-After", "120")
        });
        let remote = OpenDalRemote::s3(&server.s3(), Default::default()).unwrap();
        let error = remote.write("space/object", b"PRIVATE-UPLOAD").unwrap_err();
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("PRIVATE"));
        assert!(!diagnostic.contains("fixture-secret"));
        if status != 401 {
            assert_eq!(
                error.retry_after(),
                Some(std::time::Duration::from_secs(120))
            );
        }
        assert!(matches!(
            error,
            CloudSyncError::Http { .. } | CloudSyncError::Authentication { .. }
        ));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn malformed_listings_and_repeated_s3_tokens_fail_closed() {
    for body in [
        "<html>login</html>",
        "<ListBucketResult/>",
        "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>",
        "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>same</NextContinuationToken></ListBucketResult>",
    ] {
        let server = Server::new(move |_| Reply::new(200, body));
        let remote = OpenDalRemote::s3(&server.s3(), Default::default()).unwrap();
        assert!(remote.list_metadata("space/sync").is_err());
        assert!(server.requests.lock().unwrap().len() <= 2);
    }
    let server = Server::new(|_| Reply::new(200, "<html>login</html>"));
    let remote = OpenDalRemote::webdav(&server.webdav(), Default::default()).unwrap();
    assert!(remote.list_sync_files("space/sync").is_err());
}

#[test]
fn interrupted_response_is_an_error_and_counts_received_bytes() {
    let server = Server::new(|_| {
        let mut reply = Reply::new(200, "partial");
        reply.advertised_length = Some(100);
        reply
    });
    let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
    let remote = OpenDalRemote::s3(&server.s3(), stats.clone()).unwrap();
    let error = remote.read_if_exists("space/snapshot").unwrap_err();
    assert_eq!(error.classification(), "connection_reset");
    let stats = stats.lock().unwrap();
    assert!(stats.downloaded_bytes >= 7);
    assert!(stats.requests <= 3);
}

#[tokio::test]
async fn synchronous_facade_uses_an_owned_worker_even_inside_an_existing_runtime() {
    for webdav in [true, false] {
        let server = Server::storage();
        let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
        let remote = if webdav {
            OpenDalRemote::webdav(&server.webdav(), stats.clone())
        } else {
            OpenDalRemote::s3(&server.s3(), stats.clone())
        }
        .unwrap();
        remote.write("space/object.bin", b"body").unwrap();
        assert_eq!(
            remote.read_if_exists("space/object.bin").unwrap().unwrap(),
            b"body"
        );
        assert_eq!(
            remote.list_sync_files("space").unwrap(),
            vec!["space/object.bin"]
        );
        assert_eq!(
            stats.lock().unwrap().requests as usize,
            server.requests.lock().unwrap().len()
        );
        // Compatibility clients are also destroyed on their owning worker.
        drop(remote);
    }
}

#[test]
fn https_validates_certificate_chains_and_accepts_an_explicit_test_ca() {
    use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
    use opendal::{Buffer, HttpTransport};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;
    let key = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let certificate = key.cert.der().clone();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "https://localhost:{}/object",
        listener.local_addr().unwrap().port()
    );
    let server = crate::thread::spawn_joinable("cloud-tls-fixture", move || {
        for _ in 0..2 {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("TLS fixture: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let connection = rustls::ServerConnection::new(Arc::new(config.clone())).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut headers = Vec::new();
            loop {
                let mut byte = [0];
                if tls.read_exact(&mut byte).is_err() {
                    break;
                }
                headers.push(byte[0]);
                if headers.ends_with(b"\r\n\r\n") {
                    tls.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody",
                    )
                    .unwrap();
                    tls.flush().unwrap();
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                    break;
                }
            }
        }
    })
    .unwrap();
    let transport =
        super::transport::Transport::with_timeout(Default::default(), None, Duration::from_secs(2))
            .unwrap();
    let worker = super::worker::Worker::new().unwrap();
    let url = endpoint.clone();
    let untrusted = transport.clone();
    let error = worker
        .run(async move {
            untrusted
                .fetch(http::Request::get(url).body(Buffer::new()).unwrap())
                .await
                .map(|_| ())
                .map_err(|error| super::error::map_error("webdav", "read", error))
        })
        .unwrap_err();
    assert!(matches!(
        error,
        nyaterm_core::CloudSyncError::Storage {
            kind: CloudStorageErrorKind::Tls,
            ..
        }
    ));
    let trusted =
        transport.with_test_root(zed_reqwest::Certificate::from_der(&certificate).unwrap());
    worker
        .run(async move {
            let response = trusted
                .fetch(http::Request::get(endpoint).body(Buffer::new()).unwrap())
                .await
                .map_err(|error| super::error::map_error("webdav", "read", error))?;
            assert_eq!(response.status().as_u16(), 200);
            Ok(())
        })
        .unwrap();
    server.join().unwrap();
}

#[test]
fn timeout_and_connection_failures_have_typed_redacted_diagnostics() {
    use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
    use opendal::{Buffer, HttpTransport};
    use std::time::Duration;
    let server = Server::new(|_| {
        std::thread::sleep(Duration::from_millis(150));
        Reply::new(200, "late")
    });
    let transport = super::transport::Transport::with_timeout(
        Default::default(),
        None,
        Duration::from_millis(30),
    )
    .unwrap();
    let endpoint = format!("{}/private", server.endpoint);
    let worker = super::worker::Worker::new().unwrap();
    let error = worker
        .run(async move {
            transport
                .fetch(http::Request::get(endpoint).body(Buffer::new()).unwrap())
                .await
                .map(|_| ())
                .map_err(|error| super::error::map_error("s3", "read", error))
        })
        .unwrap_err();
    assert!(matches!(
        error,
        nyaterm_core::CloudSyncError::Storage {
            kind: CloudStorageErrorKind::Timeout,
            ..
        }
    ));
    assert!(!format!("{error:?}").contains("private"));
}

#[test]
fn s3_default_custom_and_virtual_host_urls_match_native_signing_paths() {
    use opendal::{Buffer, HttpBody, HttpTransport, HttpTransporter, OperationContext};
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<String>>>);
    impl HttpTransport for Capture {
        async fn fetch(
            &self,
            request: http::Request<Buffer>,
        ) -> opendal::Result<http::Response<HttpBody>> {
            self.0.lock().unwrap().push(request.uri().to_string());
            Err(super::error::Failure::Http {
                status: 404,
                retry_after: None,
            }
            .into_opendal())
        }
    }
    for endpoint in [
        "",
        "https://storage.example.test",
        "https://storage.example.test/base",
    ] {
        for virtual_host in [true, false] {
            let settings = nyaterm_core::S3SyncSettings {
                endpoint: endpoint.into(),
                region: "ap-northeast-1".into(),
                bucket: "fixture".into(),
                root: "apps/nyaterm".into(),
                virtual_host_style: virtual_host,
                access_key_id: Some("fixture-key".into()),
                secret_access_key: Some("fixture-secret".into()),
                ..Default::default()
            };
            let name = "space/Ŀ¼ with space/%name";
            let expected = nyaterm_core::build_s3_signed_request(
                &settings,
                nyaterm_core::S3HttpMethod::Get,
                name,
                &nyaterm_core::s3_payload_sha256(&[]),
                std::time::SystemTime::now(),
            )
            .unwrap()
            .url;
            let requests = Arc::new(Mutex::new(Vec::new()));
            let mut remote = OpenDalRemote::s3(&settings, Default::default()).unwrap();
            remote.operator = remote.operator.with_context(
                OperationContext::new()
                    .with_http_transport(HttpTransporter::new(Capture(requests.clone()))),
            );
            assert!(remote.read_if_exists(name).unwrap().is_none());
            assert_eq!(*requests.lock().unwrap(), vec![expected]);
        }
    }
}

#[test]
fn lost_upload_acknowledgement_never_replays_mutations_or_deletes_legacy_data() {
    use std::collections::BTreeMap;
    for webdav in [true, false] {
        let objects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
        let stored = objects.clone();
        let server = Server::new(move |request| {
            let mut files = stored.lock().unwrap();
            match request.method.as_str() {
                "MKCOL" => Reply::new(201, ""),
                "GET" if request.target.ends_with("/old.redb.enc") => {
                    Reply::new(200, "legacy-encrypted")
                }
                "GET" => files
                    .get(&request.target)
                    .map_or_else(|| Reply::new(404, ""), |body| Reply::new(200, body.clone())),
                "PUT" => {
                    files.insert(request.target.clone(), request.body.clone());
                    let mut reply = Reply::new(200, "");
                    reply.advertised_length = Some(32);
                    reply
                }
                _ => panic!("ambiguous publication must not MOVE, DELETE or retry"),
            }
        });
        let remote = if webdav {
            OpenDalRemote::webdav(&server.webdav(), Default::default())
        } else {
            OpenDalRemote::s3(&server.s3(), Default::default())
        }
        .unwrap();
        let name = "space/sync/v4/writers/a/1-digest.bin";
        assert!(
            remote
                .write_sync_file(name, b"complete-encrypted-publication")
                .is_err()
        );
        assert_eq!(
            remote
                .read_if_exists("space/old.redb.enc")
                .unwrap()
                .unwrap(),
            b"legacy-encrypted"
        );
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == "PUT")
                .count(),
            1
        );
        assert_eq!(objects.lock().unwrap().len(), 1);
        if webdav {
            assert!(remote.read_if_exists(name).unwrap().is_none());
            assert!(
                objects
                    .lock()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .contains("/.sync-v4-staging/")
            );
        } else {
            assert_eq!(
                remote.read_if_exists(name).unwrap().unwrap(),
                b"complete-encrypted-publication"
            );
            // Explicit subsequent recovery can recognize a completed immutable object.
            remote
                .write_sync_file(name, b"complete-encrypted-publication")
                .unwrap();
        }
    }
}

#[test]
fn s3_listing_never_forwards_session_credentials_across_redirects() {
    let target = Server::new(|_| Reply::new(200, "must not receive session credentials"));
    let location = format!("{}/fixture", target.endpoint);
    let source = Server::new(move |_| Reply::new(301, "").header("Location", &location));
    let remote = OpenDalRemote::s3(&source.s3(), Default::default()).unwrap();
    assert!(matches!(
        remote.list_metadata("space/"),
        Err(nyaterm_core::CloudSyncError::Http { status: 301, .. })
    ));
    assert!(target.requests.lock().unwrap().is_empty());
}

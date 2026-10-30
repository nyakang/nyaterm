use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::NativeS3Remote;
use nyaterm_core::cloud_sync::io::{CloudObjectRead, CloudSyncIoStats};
use nyaterm_core::cloud_sync::metadata_cache::{CachedMetadataRemote, CloudMetadataCache};
use nyaterm_core::{CloudSyncError, CloudSyncRemote, S3SyncSettings};

struct Reply {
    status: u16,
    headers: &'static str,
    body: &'static str,
    conditional: bool,
}

fn server(replies: Vec<Reply>) -> (S3SyncSettings, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let settings = S3SyncSettings {
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        bucket: "fixture".into(),
        region: "us-east-1".into(),
        access_key_id: Some("fixture-key".into()),
        secret_access_key: Some("fixture-secret".into()),
        ..Default::default()
    };
    let handle = std::thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert_eq!(request.contains("if-none-match:"), reply.conditional);
            if reply.conditional {
                assert!(request.contains("if-none-match: \"v1\""));
            }
            let len = request
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            stream.read_exact(&mut vec![0; len]).unwrap();
            write!(
                stream,
                "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                reply.status,
                reply.body.len(),
                reply.headers,
                reply.body
            )
            .unwrap();
        }
    });
    (settings, handle)
}

#[test]
fn metadata_survives_adapter_rebuilds_and_handles_304_changes_and_deletion() {
    let (settings, server) = server(vec![
        Reply {
            status: 200,
            headers: "ETag: \"v1\"\r\n",
            body: "old",
            conditional: false,
        },
        Reply {
            status: 304,
            headers: "ETag: \"v1\"\r\n",
            body: "",
            conditional: true,
        },
        Reply {
            status: 200,
            headers: "ETag: \"v2\"\r\n",
            body: "new",
            conditional: true,
        },
        // The request now uses v2, so exercise the adapter directly below instead.
    ]);
    let cache = Mutex::new(CloudMetadataCache::default());
    let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
    for expected in [b"old", b"old", b"new"] {
        let source = NativeS3Remote::new_with_stats(&settings, stats.clone()).unwrap();
        let remote = CachedMetadataRemote::new(&source, &cache, "root");
        assert_eq!(
            remote
                .read_if_exists("root/sync/latest.redb")
                .unwrap()
                .unwrap(),
            expected
        );
    }
    server.join().unwrap();
    let stats = *stats.lock().unwrap();
    assert_eq!(stats.requests, 3);
    assert_eq!(stats.not_modified, 1);
    assert_eq!(stats.downloaded_bytes, 6);
    let (settings, server) = self::server(vec![Reply {
        status: 404,
        headers: "",
        body: "missing",
        conditional: false,
    }]);
    assert!(matches!(
        NativeS3Remote::new(&settings)
            .unwrap()
            .read_conditional("missing", None)
            .unwrap(),
        CloudObjectRead::Missing
    ));
    server.join().unwrap();
}

#[test]
fn unsupported_conditionals_fall_back_but_authentication_and_rate_limits_do_not() {
    for status in [400, 405, 501] {
        let (settings, server) = server(vec![
            Reply {
                status,
                headers: "",
                body: "unsupported",
                conditional: true,
            },
            Reply {
                status: 200,
                headers: "",
                body: "body",
                conditional: false,
            },
        ]);
        let result = NativeS3Remote::new(&settings)
            .unwrap()
            .read_conditional("file", Some("\"v1\""));
        assert!(matches!(
            result,
            Ok(CloudObjectRead::Modified { etag: None, .. })
        ));
        server.join().unwrap();
    }
    for status in [401, 403, 429, 503] {
        let (settings, server) = server(vec![Reply {
            status,
            headers: "Retry-After: 123\r\n",
            body: "PRIVATE-ECHO",
            conditional: true,
        }]);
        let error = NativeS3Remote::new(&settings)
            .unwrap()
            .read_conditional("file", Some("\"v1\""))
            .err()
            .unwrap();
        assert!(matches!(error,CloudSyncError::Http {status:s,..} if s==status));
        assert!(!format!("{error} {error:?}").contains("PRIVATE"));
        assert_eq!(
            error.retry_after(),
            Some(std::time::Duration::from_secs(123))
        );
        server.join().unwrap();
    }
}

#[test]
fn list_pagination_preserves_opaque_etags_sizes_and_provider_root() {
    let (mut settings, server) = server(vec![
        Reply {
            status: 200,
            headers: "",
            conditional: false,
            body: r#"<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>provider/root/sync/heads/a.json</Key><ETag>&quot;opaque-7&quot;</ETag><Size>57</Size></Contents><NextContinuationToken>next&amp;page</NextContinuationToken></ListBucketResult>"#,
        },
        Reply {
            status: 200,
            headers: "",
            conditional: false,
            body: r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>provider/root/sync/heads/b.json</Key><Size>123</Size></Contents></ListBucketResult>"#,
        },
    ]);
    settings.root = "provider".into();
    let entries = NativeS3Remote::new(&settings)
        .unwrap()
        .list_metadata("root/sync/heads/")
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "root/sync/heads/a.json");
    assert_eq!(entries[0].etag.as_deref(), Some("\"opaque-7\""));
    assert_eq!(entries[0].size, Some(57));
    assert!(entries[1].etag.is_none());
    server.join().unwrap();
}

#[test]
fn successful_put_invalidates_metadata_before_readback() {
    let (settings, server) = server(vec![
        Reply {
            status: 200,
            headers: "ETag: \"v1\"\r\n",
            body: "old",
            conditional: false,
        },
        Reply {
            status: 200,
            headers: "",
            body: "",
            conditional: false,
        },
        Reply {
            status: 200,
            headers: "ETag: \"v2\"\r\n",
            body: "new",
            conditional: false,
        },
    ]);
    let source = NativeS3Remote::new(&settings).unwrap();
    let cache = Mutex::new(CloudMetadataCache::default());
    let remote = CachedMetadataRemote::new(&source, &cache, "root");
    let path = "root/sync/latest.redb";
    remote.read_if_exists(path).unwrap();
    remote.write(path, b"new").unwrap();
    assert_eq!(remote.read_if_exists(path).unwrap().unwrap(), b"new");
    server.join().unwrap();
}

#[test]
fn v4_listing_keeps_every_page_and_rejects_repeated_tokens_or_error_documents() {
    let page_one = r#"<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>space/sync/v4/registry/a.bin</Key></Contents><NextContinuationToken>next</NextContinuationToken></ListBucketResult>"#;
    let page_two = r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>space/sync/v4/registry/b.bin</Key></Contents></ListBucketResult>"#;
    let (settings, worker) = server(vec![
        Reply {
            status: 200,
            headers: "",
            body: page_one,
            conditional: false,
        },
        Reply {
            status: 200,
            headers: "",
            body: page_two,
            conditional: false,
        },
    ]);
    let entries = NativeS3Remote::new(&settings)
        .unwrap()
        .list_sync_files("space/sync/v4/registry")
        .unwrap();
    assert_eq!(
        entries,
        vec![
            "space/sync/v4/registry/a.bin",
            "space/sync/v4/registry/b.bin"
        ]
    );
    worker.join().unwrap();
    let (settings, worker) = server(vec![
        Reply {
            status: 200,
            headers: "",
            body: page_one,
            conditional: false,
        },
        Reply {
            status: 200,
            headers: "",
            body: page_one,
            conditional: false,
        },
    ]);
    assert!(
        NativeS3Remote::new(&settings)
            .unwrap()
            .list_sync_files("space/sync/v4/registry")
            .is_err()
    );
    worker.join().unwrap();
    for body in [
        "<Error><Code>AccessDenied</Code></Error>",
        "<ListBucketResult><IsTruncated>false</IsTruncated>",
        "<ListBucketResult></ListBucketResult>",
        "<ListBucketResult><IsTruncated>unknown</IsTruncated></ListBucketResult>",
    ] {
        assert!(super::parse_s3_list_page(body).is_err());
    }
}

#[test]
fn listing_rejects_outside_prefix_objects_and_duplicate_objects_across_pages() {
    for body in [
        r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>other/space/object.bin</Key></Contents></ListBucketResult>"#,
        r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>provider-copy/space/object.bin</Key></Contents></ListBucketResult>"#,
    ] {
        let (mut settings, worker) = server(vec![Reply {
            status: 200,
            headers: "",
            body,
            conditional: false,
        }]);
        settings.root = "provider".into();
        let result = NativeS3Remote::new(&settings)
            .unwrap()
            .list_metadata("space/");
        worker.join().unwrap();
        assert!(result.is_err());
    }
    let (settings, worker) = server(vec![
        Reply {
            status: 200,
            headers: "",
            body: r#"<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>space/a.bin</Key></Contents><NextContinuationToken>next</NextContinuationToken></ListBucketResult>"#,
            conditional: false,
        },
        Reply {
            status: 200,
            headers: "",
            body: r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>space/a.bin</Key></Contents></ListBucketResult>"#,
            conditional: false,
        },
    ]);
    let result = NativeS3Remote::new(&settings)
        .unwrap()
        .list_metadata("space/");
    worker.join().unwrap();
    assert!(result.is_err());
}

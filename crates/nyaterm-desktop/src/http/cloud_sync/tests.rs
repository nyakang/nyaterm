use std::sync::Mutex;

use nyaterm_core::WebdavSyncSettings;
use zed_reqwest::StatusCode;

use super::aliyun::{
    AliyunDriveType, aliyun_drive_error_code, aliyun_drive_remote_error,
    parse_aliyun_drive_list_page,
};
use super::google_drive::{google_drive_multipart_body, parse_google_drive_list_page};
use super::helpers::{
    build_digest_authorization, form_urlencoded, parse_digest_challenge, percent_encode_path,
};
use super::onedrive::{onedrive_item_path, parse_onedrive_list_page};
use super::s3::parse_s3_list_page;
use super::webdav::parse_webdav_file_names;
use super::{NativeAliyunDriveRemote, NativeOneDriveRemote, NativeWebdavRemote};

#[test]
fn webdav_url_joins_endpoint_root_and_sync_path() {
    let remote = NativeWebdavRemote::new(&WebdavSyncSettings {
        endpoint: "https://dav.example.com/remote.php/webdav/".to_string(),
        root: "/apps/nyaterm/".to_string(),
        username: String::new(),
        password: None,
    })
    .expect("remote");

    assert_eq!(
        remote.url_for("/nyaterm/sync/latest.redb"),
        "https://dav.example.com/remote.php/webdav/apps/nyaterm/nyaterm/sync/latest.redb"
    );
}

#[test]
fn webdav_digest_authorization_matches_rfc_example() {
    let header = build_digest_authorization(
        r#"realm="testrealm@host.com", qop="auth", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        "Mufasa",
        "Circle Of Life",
        "GET",
        "/dir/index.html",
        "0a4f113b",
        "00000001",
    )
    .expect("digest header");

    assert!(header.contains("Digest username=\"Mufasa\""));
    assert!(header.contains("qop=auth"));
    assert!(header.contains("response=\"6629fae49393a05397450978507c4ef1\""));
}

#[test]
fn webdav_digest_parser_handles_quoted_commas() {
    let parsed = parse_digest_challenge(
        r#"realm="Nya,Term", nonce="abc", algorithm=MD5, qop="auth,auth-int""#,
    );

    assert_eq!(parsed.get("realm").map(String::as_str), Some("Nya,Term"));
    assert_eq!(parsed.get("nonce").map(String::as_str), Some("abc"));
    assert_eq!(parsed.get("qop").map(String::as_str), Some("auth,auth-int"));
}

#[test]
fn webdav_propfind_parser_uses_local_names_and_decodes_file_names() {
    let body = r#"<?xml version="1.0"?>
        <d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/nyaterm/sync/snapshots/</d:href></d:response>
          <d:response><d:href>/dav/nyaterm/sync/snapshots/rev%20one.redb.enc</d:href></d:response>
          <d:response><d:href>/dav/nyaterm/sync/snapshots/ignored.txt</d:href></d:response>
          <response xmlns="DAV:"><href>/dav/nyaterm/sync/snapshots/rev-two.redb.enc</href></response>
        </d:multistatus>"#;

    assert_eq!(
        parse_webdav_file_names(body).expect("parse PROPFIND"),
        vec!["rev one.redb.enc", "rev-two.redb.enc"]
    );
}

#[test]
fn webdav_propfind_parser_rejects_invalid_percent_encoding() {
    let body = r#"<multistatus xmlns="DAV:"><response><href>/snapshots/bad%ZZ.redb.enc</href></response></multistatus>"#;

    assert!(parse_webdav_file_names(body).is_err());
}

#[test]
fn webdav_listing_errors_are_not_interpreted_as_an_empty_space() {
    for body in [
        "<html><body>Authentication required</body></html>",
        "<multistatus><response><href>/missing.redb.enc</href><status>HTTP/1.1 403 Forbidden</status></response></multistatus>",
        "<multistatus><response><href>/missing.redb.enc</href>",
    ] {
        assert!(parse_webdav_file_names(body).is_err());
    }
    assert!(
        parse_webdav_file_names("<multistatus/>")
            .unwrap()
            .is_empty()
    );
    assert_eq!(parse_webdav_file_names("<multistatus><response><href>/a&amp;b.redb.enc</href><status>HTTP/1.1 200 OK</status></response></multistatus>").unwrap(), vec!["a&b.redb.enc"]);
}

#[test]
fn s3_list_parser_reads_keys_and_continuation_token() {
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
          <IsTruncated>true</IsTruncated>
          <Contents><Key>root/sync/snapshots/r1.redb.enc</Key></Contents>
          <Contents><Key>root/sync/snapshots/r2.redb.enc</Key></Contents>
          <NextContinuationToken>token+/=</NextContinuationToken>
        </ListBucketResult>"#;
    let page = parse_s3_list_page(body).expect("parse S3 list");

    assert_eq!(
        page.objects.into_iter().map(|e| e.path).collect::<Vec<_>>(),
        vec![
            "root/sync/snapshots/r1.redb.enc",
            "root/sync/snapshots/r2.redb.enc"
        ]
    );
    assert!(page.truncated);
    assert_eq!(page.next_token.as_deref(), Some("token+/="));
}

#[test]
fn google_drive_multipart_body_contains_metadata_and_media() {
    let (boundary, body) =
        google_drive_multipart_body("root", "latest.redb", b"payload").expect("multipart");
    let text = String::from_utf8(body).expect("utf8 multipart");

    assert!(text.contains(&format!("--{boundary}\r\n")));
    assert!(text.contains(r#""name":"latest.redb""#));
    assert!(text.contains(r#""parents":["root"]"#));
    assert!(text.contains("Content-Type: application/octet-stream\r\n\r\npayload"));
    assert!(text.ends_with(&format!("--{boundary}--\r\n")));
}

#[test]
fn drive_list_page_parsers_keep_files_and_pagination_tokens() {
    let google = parse_google_drive_list_page(
        r#"{"files":[{"name":"r1.redb.enc","mimeType":"application/octet-stream"},{"name":"folder","mimeType":"application/vnd.google-apps.folder"}],"nextPageToken":"google-next"}"#,
    )
    .expect("Google Drive page");
    assert_eq!(google.names, vec!["r1.redb.enc"]);
    assert_eq!(google.next_page_token.as_deref(), Some("google-next"));

    let onedrive = parse_onedrive_list_page(
        r#"{"value":[{"name":"r2.redb.enc","file":{}},{"name":"folder","folder":{}}],"@odata.nextLink":"https://graph.example/next?$skiptoken=abc"}"#,
    )
    .expect("OneDrive page");
    assert_eq!(onedrive.names, vec!["r2.redb.enc"]);
    assert_eq!(
        onedrive.next_link.as_deref(),
        Some("https://graph.example/next?$skiptoken=abc")
    );

    let aliyun = parse_aliyun_drive_list_page(&serde_json::json!({
        "items": [
            {"name": "r3.redb.enc", "type": "file"},
            {"name": "folder", "type": "folder"}
        ],
        "next_marker": "aliyun-next"
    }))
    .expect("Aliyun Drive page");
    assert_eq!(aliyun.names, vec!["r3.redb.enc"]);
    assert_eq!(aliyun.next_marker, "aliyun-next");
}

#[test]
fn drive_list_parsers_reject_malformed_or_partial_pages_instead_of_empty_spaces() {
    for body in [
        "{}",
        "{\"files\":null}",
        r#"{"files":[],"nextPageToken":false}"#,
        r#"{"files":[],"incompleteSearch":true}"#,
        r#"{"files":[{"name":"missing-type"}]}"#,
        r#"{"files":[{"name":"../outside","mimeType":"application/octet-stream"}]}"#,
    ] {
        assert!(parse_google_drive_list_page(body).is_err());
    }
    for body in [
        "{}",
        r#"{"value":[],"@odata.nextLink":null}"#,
        r#"{"value":[{"name":"missing-kind"}]}"#,
        r#"{"value":[{"name":"object","file":null}]}"#,
    ] {
        assert!(parse_onedrive_list_page(body).is_err());
    }
    for value in [
        serde_json::json!({}),
        serde_json::json!({"items":[],"next_marker":false}),
        serde_json::json!({"items":[{"type":"file"}]}),
        serde_json::json!({"items":[{"type":"unknown","name":"object"}]}),
    ] {
        assert!(parse_aliyun_drive_list_page(&value).is_err());
    }
}

#[test]
fn form_urlencoded_uses_oauth_form_rules() {
    assert_eq!(
        form_urlencoded(&[("client id", "abc+123"), ("secret", "a/b?c")]),
        "client+id=abc%2B123&secret=a%2Fb%3Fc"
    );
}

#[test]
fn onedrive_item_path_joins_root_and_child_segments() {
    assert_eq!(
        onedrive_item_path("/Nya Term/", "/sync/latest.redb"),
        "Nya Term/sync/latest.redb"
    );
    assert_eq!(
        onedrive_item_path("", "sync/latest.redb"),
        "sync/latest.redb"
    );
}

#[test]
fn percent_encode_path_preserves_separators_and_encodes_segments() {
    assert_eq!(
        percent_encode_path("Nya Term/sync/latest redb/猫"),
        "Nya%20Term/sync/latest%20redb/%E7%8C%AB"
    );
}

#[test]
fn onedrive_urls_use_graph_path_addressing_templates() {
    let remote = NativeOneDriveRemote {
        token_sink: std::sync::Arc::new(|_| Ok(())),
        client: zed_reqwest::blocking::Client::builder()
            .build()
            .expect("client"),
        root: "Nya Term".to_string(),
        access_token: Mutex::new("token".to_string().into()),
        refresh_token: Mutex::new(nyaterm_core::SecretString::default()),
        client_id: None,
        client_secret: None,
    };

    assert_eq!(
        remote.children_url("Nya Term/sync"),
        "https://graph.microsoft.com/v1.0/me/drive/root:/Nya%20Term/sync:/children"
    );
    assert_eq!(
        remote.content_url("sync/latest redb").expect("content url"),
        "https://graph.microsoft.com/v1.0/me/drive/root:/Nya%20Term/sync/latest%20redb:/content"
    );
}

#[test]
fn aliyun_drive_type_matches_legacy_values() {
    assert_eq!(
        AliyunDriveType::parse("").expect("default"),
        AliyunDriveType::Default
    );
    assert_eq!(
        AliyunDriveType::parse("resource").expect("resource"),
        AliyunDriveType::Resource
    );
    assert_eq!(
        AliyunDriveType::parse("backup").expect("backup"),
        AliyunDriveType::Backup
    );
    assert!(AliyunDriveType::parse("archive").is_err());
}

#[test]
fn aliyun_drive_item_path_uses_rooted_absolute_path() {
    let remote = NativeAliyunDriveRemote {
        token_sink: std::sync::Arc::new(|_| Ok(())),
        client: zed_reqwest::blocking::Client::builder()
            .build()
            .expect("client"),
        root: "Nya Term".to_string(),
        drive_type: AliyunDriveType::Resource,
        access_token: Mutex::new("token".to_string().into()),
        refresh_token: Mutex::new(nyaterm_core::SecretString::default()),
        client_id: None,
        client_secret: None,
        drive_id: Mutex::new(None),
    };

    assert_eq!(
        remote.item_path("sync/latest redb"),
        "/Nya Term/sync/latest redb"
    );
    assert_eq!(remote.item_path(""), "/Nya Term");
}

#[test]
fn aliyun_drive_error_helpers_do_not_expose_response_body() {
    let body = r#"{"code":"NotFound.File","message":"file missing"}"#;

    assert_eq!(
        aliyun_drive_error_code(body).as_deref(),
        Some("NotFound.File")
    );
    assert!(
        aliyun_drive_remote_error(StatusCode::BAD_REQUEST, body, "lookup", None)
            .to_string()
            .contains("HTTP 400")
    );
}

#[test]
fn cloud_sync_http_errors_never_include_query_tokens_or_echoed_response_bodies() {
    use nyaterm_core::{SnippetHttpClient, SnippetHttpMethod, SnippetHttpRequest};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    let client = zed_reqwest::blocking::Client::new();
    let raw = client
        .get("invalid-scheme://example.test/?token=PRIVATE-QUERY")
        .send()
        .unwrap_err();
    let error = super::helpers::safe_http_error("test", raw);
    assert!(!format!("{error} {error:?}").contains("PRIVATE"));
    for status in [401, 429, 503] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 8192];
            let _ = stream.read(&mut request).unwrap();
            let body = "PRIVATE-ECHOED-CREDENTIAL";
            write!(stream, "HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nRetry-After: 1900\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let error = super::NativeSnippetHttpClient::new()
            .unwrap()
            .send(SnippetHttpRequest {
                method: SnippetHttpMethod::Get,
                url: format!("http://{address}/?signature=PRIVATE-SIGNATURE"),
                headers: BTreeMap::new(),
                query: BTreeMap::new(),
                json_body: None,
            })
            .unwrap_err();
        server.join().unwrap();
        assert!(!format!("{error} {error:?}").contains("PRIVATE"));
        assert_eq!(
            error.retry_after(),
            Some(std::time::Duration::from_secs(1900))
        );
        assert!(
            matches!(error, nyaterm_core::CloudSyncError::Http { status: actual, .. } if actual == status)
        );
    }
    // Malformed XML/JSON can include arbitrary server-controlled tag/field text.
    for error in [
        parse_webdav_file_names("<PRIVATE-XML></different>").unwrap_err(),
        parse_s3_list_page("<PRIVATE-XML></different>")
            .err()
            .unwrap(),
        super::helpers::safe_json::<serde_json::Value>("PRIVATE-JSON", "test").unwrap_err(),
        aliyun_drive_remote_error(StatusCode::UNAUTHORIZED, "PRIVATE-TOKEN", "lookup", None),
    ] {
        assert!(!format!("{error} {error:?}").contains("PRIVATE"));
    }
}

#[test]
fn cloud_sync_retry_after_accepts_seconds_and_future_http_dates() {
    use zed_reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("321"));
    assert_eq!(super::helpers::retry_after_seconds(&headers), Some(321));
    let date =
        httpdate::fmt_http_date(std::time::SystemTime::now() + std::time::Duration::from_secs(600));
    headers.insert(RETRY_AFTER, HeaderValue::from_str(&date).unwrap());
    assert!(matches!(
        super::helpers::retry_after_seconds(&headers),
        Some(598..=600)
    ));
    headers.insert(RETRY_AFTER, HeaderValue::from_static("invalid"));
    assert_eq!(super::helpers::retry_after_seconds(&headers), None);
}

#[test]
fn cloud_sync_token_save_is_required_before_install_and_survives_later_cancellation() {
    use nyaterm_core::cloud_sync::credentials::{CloudCredentialUpdate, DriveProvider};
    use nyaterm_core::{CloudSyncError, SecretString};
    use std::sync::Arc;
    let access = Arc::new(Mutex::new(SecretString::from("old-access")));
    let refresh = Arc::new(Mutex::new(SecretString::from("old-refresh")));
    let update = CloudCredentialUpdate {
        provider: DriveProvider::Google,
        client_id: None,
        client_secret: None,
        expected_access_token: Some("old-access".into()),
        expected_refresh_token: Some("old-refresh".into()),
        access_token: "new-access".into(),
        refresh_token: Some("new-refresh".into()),
    };
    let fail: super::CloudTokenSink = Arc::new(|_| Err(CloudSyncError::CredentialPersistence));
    assert!(matches!(
        super::helpers::install_refreshed_tokens(&access, &refresh, &fail, update.clone()),
        Err(CloudSyncError::CredentialPersistence)
    ));
    assert_eq!(access.lock().unwrap().expose_secret(), "old-access");
    assert_eq!(refresh.lock().unwrap().expose_secret(), "old-refresh");
    let old_access = access.clone();
    let old_refresh = refresh.clone();
    let saved = Arc::new(Mutex::new(None));
    let persisted = saved.clone();
    let sink: super::CloudTokenSink = Arc::new(move |update| {
        assert_eq!(old_access.lock().unwrap().expose_secret(), "old-access");
        assert_eq!(old_refresh.lock().unwrap().expose_secret(), "old-refresh");
        *persisted.lock().unwrap() = Some(update);
        Ok(())
    });
    super::helpers::install_refreshed_tokens(&access, &refresh, &sink, update).unwrap();
    let control = nyaterm_core::cloud_sync::control::CloudSyncOperationControl::default();
    control.request_cancel();
    assert!(control.checkpoint().is_err());
    assert_eq!(access.lock().unwrap().expose_secret(), "new-access");
    assert_eq!(
        saved
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("new-refresh")
    );
    let access_only = CloudCredentialUpdate {
        provider: DriveProvider::Google,
        client_id: None,
        client_secret: None,
        expected_access_token: Some("new-access".into()),
        expected_refresh_token: Some("new-refresh".into()),
        access_token: "next-access".into(),
        refresh_token: None,
    };
    let sink: super::CloudTokenSink = Arc::new(|_| Ok(()));
    super::helpers::install_refreshed_tokens(&access, &refresh, &sink, access_only).unwrap();
    assert_eq!(access.lock().unwrap().expose_secret(), "next-access");
    assert_eq!(refresh.lock().unwrap().expose_secret(), "new-refresh");
}

#[test]
fn cloud_sync_oauth_revocation_is_typed_without_exposing_error_descriptions() {
    let error = super::helpers::oauth_failure(
        "Google Drive",
        400,
        r#"{"error":"invalid_grant","error_description":"PRIVATE-REFRESH-TOKEN"}"#,
        None,
    );
    assert!(matches!(
        error,
        nyaterm_core::CloudSyncError::Authentication { status: 400, .. }
    ));
    assert_eq!(error.classification(), "authentication");
    assert!(!format!("{error} {error:?}").contains("PRIVATE"));
}

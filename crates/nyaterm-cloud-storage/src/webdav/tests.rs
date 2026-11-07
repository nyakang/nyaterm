use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use nyaterm_core::{CloudSyncError, CloudSyncRemote, WebdavSyncSettings};
use zed_reqwest::{Method, StatusCode};

use super::{NativeWebdavRemote, redirect_target};
use crate::helpers::{build_digest_authorization, parse_digest_challenge};

struct Request {
    head: String,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// One request the fixture answers, and the reply it sends.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: &'static str,
}

fn reply(status: u16) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: "",
    }
}

fn listing(body: &'static str) -> Reply {
    Reply {
        status: 207,
        headers: vec![("Content-Type", "application/xml; charset=utf-8".into())],
        body,
    }
}

fn redirect(location: &str) -> Reply {
    Reply {
        status: 301,
        headers: vec![("Location", location.to_string())],
        body: "<html><body>Moved</body></html>",
    }
}

impl Reply {
    /// Reads the fixture's request to completion, so the client is never blocked
    /// writing a body the fixture did not consume.
    fn answer(&self, stream: &mut std::net::TcpStream) -> Request {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 16_384, "request headers are too large");
        }
        let head = String::from_utf8(request.clone()).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        if length > 0 {
            stream.read_exact(&mut body).unwrap();
        }
        let mut response = format!("HTTP/1.1 {} Fixture\r\n", self.status);
        for (name, value) in &self.headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        response.push_str("Connection: close\r\n\r\n");
        response.push_str(self.body);
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
        Request { head, body }
    }
}

fn scripted_server(
    requests: Vec<(&'static str, Reply)>,
) -> (NativeWebdavRemote, std::thread::JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let remote = NativeWebdavRemote::new(&WebdavSyncSettings {
        endpoint: format!("http://{}/dav", listener.local_addr().unwrap()),
        root: "apps/nyaterm".into(),
        ..Default::default()
    })
    .unwrap();
    let handle = std::thread::spawn(move || {
        let mut received = Vec::new();
        for (expected, reply) in requests {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request {expected}");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            };
            let request = reply.answer(&mut stream);
            assert_eq!(request.head.lines().next().unwrap(), expected);
            received.push(request);
        }
        received
    });
    (remote, handle)
}

const EMPTY_LISTING: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/apps/nyaterm/sync/</D:href>
    <D:propstat>
      <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
    <D:propstat>
      <D:prop><D:getcontentlength/></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

#[test]
fn creates_the_configured_root_before_protocol_subdirectories() {
    let (remote, server) = scripted_server(vec![
        ("MKCOL /dav/apps/ HTTP/1.1", reply(201)),
        ("MKCOL /dav/apps/nyaterm/ HTTP/1.1", reply(201)),
        ("MKCOL /dav/apps/nyaterm/sync/ HTTP/1.1", reply(201)),
        ("MKCOL /dav/apps/nyaterm/sync/v4/ HTTP/1.1", reply(201)),
    ]);
    let result = remote.create_dir("sync/v4");
    server.join().unwrap();
    result.unwrap();
}

#[test]
fn existing_parent_directories_do_not_block_creation() {
    let (remote, server) = scripted_server(vec![
        ("MKCOL /dav/apps/ HTTP/1.1", reply(405)),
        ("MKCOL /dav/apps/nyaterm/ HTTP/1.1", reply(405)),
        ("MKCOL /dav/apps/nyaterm/sync/ HTTP/1.1", reply(201)),
    ]);
    let result = remote.create_dir("sync");
    server.join().unwrap();
    result.unwrap();
}

#[test]
fn a_parent_conflict_stops_creation_instead_of_reporting_success() {
    let (remote, server) = scripted_server(vec![("MKCOL /dav/apps/ HTTP/1.1", reply(409))]);
    let result = remote.create_dir("sync/v4");
    server.join().unwrap();
    assert!(matches!(
        result,
        Err(CloudSyncError::Http { status: 409, .. })
    ));
}

#[test]
fn propfind_names_the_collection_with_its_trailing_slash() {
    let (remote, server) = scripted_server(vec![(
        "PROPFIND /dav/apps/nyaterm/sync/ HTTP/1.1",
        listing(EMPTY_LISTING),
    )]);
    let result = remote.list_sync_files("sync");
    server.join().unwrap();
    result.unwrap();
}

#[test]
fn a_collection_redirect_is_retried_with_the_original_method_and_body() {
    // Servers that answer a slashless collection URL with a redirect must still
    // receive this request as a PROPFIND; following the redirect as a GET would
    // read the HTML stub the redirect points at.
    let (remote, server) = scripted_server(vec![
        (
            "PROPFIND /dav/apps/nyaterm/sync HTTP/1.1",
            redirect("/dav/apps/nyaterm/sync/"),
        ),
        (
            "PROPFIND /dav/apps/nyaterm/sync/ HTTP/1.1",
            listing(EMPTY_LISTING),
        ),
    ]);
    let url = format!("{}/apps/nyaterm/sync", remote.endpoint);
    let result = remote.send_with_headers(
        zed_reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
        &url,
        Some(b"<propfind/>".to_vec()),
        &[("Depth", "1"), ("Content-Type", "application/xml")],
    );
    for request in server.join().unwrap() {
        assert_eq!(request.body, b"<propfind/>");
        assert_eq!(request.header("Depth"), Some("1"));
        assert_eq!(request.header("Content-Type"), Some("application/xml"));
    }
    assert_eq!(result.unwrap().status().as_u16(), 207);
}

#[test]
fn cross_origin_read_redirects_never_forward_basic_or_digest_credentials() {
    for digest in [false, true] {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let body = "remote-object";
        let location = format!("http://{}/download", target.local_addr().unwrap());
        let target_handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match target.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing redirected download");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("redirect target accept failed: {error}"),
                }
            };
            Reply {
                status: 200,
                headers: Vec::new(),
                body,
            }
            .answer(&mut stream)
        });
        let mut script = Vec::new();
        if digest {
            script.push((
                "GET /dav/apps/nyaterm/sync/latest.redb HTTP/1.1",
                unauthorized(),
            ));
        }
        script.push((
            "GET /dav/apps/nyaterm/sync/latest.redb HTTP/1.1",
            redirect(&location),
        ));
        let (remote, server) = scripted_server(script);
        let remote = with_credentials(remote);
        let bytes = remote.read_if_exists("sync/latest.redb").unwrap();
        server.join().unwrap();
        let request = target_handle.join().unwrap();
        assert_eq!(request.header("authorization"), None);
        assert_eq!(bytes.as_deref(), Some(body.as_bytes()));
    }
}

#[test]
fn read_redirects_may_cross_origins_but_never_downgrade_or_carry_userinfo() {
    for (current, location, allowed) in [
        (
            "https://dav.example/sync",
            "https://other.example/other",
            true,
        ),
        (
            "http://dav.example/sync",
            "http://other.example/other",
            true,
        ),
        (
            "https://dav.example/sync",
            "http://other.example/other",
            false,
        ),
        (
            "https://dav.example/sync",
            "https://user@other.example/other",
            false,
        ),
        (
            "https://dav.example/sync",
            "https://other.example/other#fragment",
            false,
        ),
    ] {
        let target =
            redirect_target(StatusCode::FOUND, Some(location), current, &Method::GET).unwrap();
        assert_eq!(target.is_some(), allowed, "{current} -> {location}");
    }
}

#[test]
fn native_head_redirects_strip_authentication_and_writes_never_leave_origin() {
    use crate::test_support::http::{Reply as HttpReply, Server};
    for method in ["HEAD", "PUT", "DELETE", "MKCOL", "MOVE", "PROPFIND"] {
        let target = Server::new(|_| HttpReply::new(200, ""));
        let location = format!("{}/object", target.endpoint);
        let source = Server::new(move |_| HttpReply::new(307, "").header("Location", &location));
        let remote = NativeWebdavRemote::new(&source.webdav()).unwrap();
        let response = remote
            .send(
                Method::from_bytes(method.as_bytes()).unwrap(),
                &format!("{}/original", source.endpoint),
                None,
            )
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if method == "HEAD" { 200 } else { 307 }
        );
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
fn redirects_require_the_same_origin_or_a_default_port_tls_upgrade() {
    for (current, location, allowed) in [
        ("https://dav.example/sync", "/other", true),
        (
            "https://dav.example:443/sync",
            "https://dav.example/other",
            true,
        ),
        ("https://dav.example:8443/sync", "/other", true),
        (
            "https://dav.example/sync",
            "https://dav.example:8443/other",
            false,
        ),
        (
            "http://dav.example/sync",
            "http://dav.example:8080/other",
            false,
        ),
        ("http://dav.example/sync", "https://dav.example/other", true),
        (
            "http://dav.example:80/sync",
            "https://dav.example:443/other",
            true,
        ),
        (
            "http://dav.example:8080/sync",
            "https://dav.example/other",
            false,
        ),
        (
            "http://dav.example/sync",
            "https://dav.example:8443/other",
            false,
        ),
        (
            "http://dav.example:8080/sync",
            "https://dav.example:8080/other",
            false,
        ),
        (
            "https://dav.example/sync",
            "http://dav.example/other",
            false,
        ),
        (
            "https://dav.example/sync",
            "https://other.example/other",
            false,
        ),
        (
            "https://dav.example/sync",
            "https://user@dav.example/other",
            false,
        ),
        ("https://dav.example/sync", "/other#fragment", false),
        ("https://dav.example/sync", "ftp://dav.example/other", false),
    ] {
        let target =
            redirect_target(StatusCode::FOUND, Some(location), current, &Method::PUT).unwrap();
        assert_eq!(target.is_some(), allowed, "{current} -> {location}");
    }
}

const DIGEST_CHALLENGE: &str = r#"Digest realm="fixture", nonce="fixture", qop="auth""#;

fn unauthorized() -> Reply {
    Reply {
        headers: vec![("WWW-Authenticate", DIGEST_CHALLENGE.into())],
        ..reply(401)
    }
}

fn with_credentials(mut remote: NativeWebdavRemote) -> NativeWebdavRemote {
    remote.username = "fixture".into();
    remote.password = Some("fixture".into());
    remote
}

#[test]
fn cross_port_redirects_never_forward_basic_or_digest_credentials_or_uploads() {
    for digest in [false, true] {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let mut moved = redirect(&format!("http://{}/other", target.local_addr().unwrap()));
        moved.status = 302;
        let mut script = Vec::new();
        if digest {
            script.push(("PUT /dav/object HTTP/1.1", unauthorized()));
        }
        script.push(("PUT /dav/object HTTP/1.1", moved));
        let (remote, server) = scripted_server(script);
        let remote = with_credentials(remote);
        let result = remote.send(
            Method::PUT,
            &format!("{}/object", remote.endpoint),
            Some(b"upload".to_vec()),
        );
        let requests = server.join().unwrap();
        let last = requests.last().unwrap();
        let authorization = last.header("Authorization").unwrap();
        assert!(authorization.starts_with(if digest { "Digest " } else { "Basic " }));
        assert_eq!(last.body, b"upload");
        assert_eq!(result.unwrap().status(), StatusCode::FOUND);
        assert!(
            matches!(target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

#[test]
fn see_other_never_replays_webdav_mutations() {
    for name in ["PUT", "MKCOL", "PROPFIND", "DELETE"] {
        let method = Method::from_bytes(name.as_bytes()).unwrap();
        let error = redirect_target(
            StatusCode::SEE_OTHER,
            Some("/upload-result"),
            "https://dav.example/object",
            &method,
        )
        .unwrap_err();
        assert!(
            matches!(error, CloudSyncError::Remote(detail) if detail.contains("303") && detail.contains(name))
        );
    }

    let mut moved = redirect("/upload-result");
    moved.status = 303;
    let (remote, server) = scripted_server(vec![("PUT /dav/apps/nyaterm/object HTTP/1.1", moved)]);
    let result = remote.write("object", b"upload");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, b"upload");
    assert!(matches!(result, Err(CloudSyncError::Remote(detail)) if detail.contains("303")));
}

#[test]
fn see_other_can_follow_get_and_head() {
    for method in [Method::GET, Method::HEAD] {
        assert_eq!(
            redirect_target(
                StatusCode::SEE_OTHER,
                Some("/result"),
                "https://dav.example/object",
                &method
            )
            .unwrap()
            .as_deref(),
            Some("https://dav.example/result")
        );
    }
    let mut moved = redirect("/result");
    moved.status = 303;
    let (remote, server) = scripted_server(vec![
        ("GET /dav/apps/nyaterm/object HTTP/1.1", moved),
        ("GET /result HTTP/1.1", reply(200)),
    ]);
    let result = remote.read_if_exists("object");
    server.join().unwrap();
    assert_eq!(result.unwrap(), Some(Vec::new()));
}

#[test]
fn digest_redirects_resign_the_final_path_and_query_and_increment_nonce_count() {
    let mut moved = redirect("/final/object?revision=2");
    moved.status = 302;
    let (remote, server) = scripted_server(vec![
        (
            "PUT /dav/object HTTP/1.1",
            redirect("/authenticated/object?revision=1"),
        ),
        (
            "PUT /authenticated/object?revision=1 HTTP/1.1",
            unauthorized(),
        ),
        ("PUT /authenticated/object?revision=1 HTTP/1.1", moved),
        ("PUT /final/object?revision=2 HTTP/1.1", reply(201)),
    ]);
    let remote = with_credentials(remote);
    let result = remote.send(
        Method::PUT,
        &format!("{}/object", remote.endpoint),
        Some(b"upload".to_vec()),
    );
    let requests = server.join().unwrap();
    assert_eq!(result.unwrap().status(), StatusCode::CREATED);
    for request in &requests {
        assert_eq!(request.body, b"upload");
    }
    let mut cnonce = None;
    for (request, uri, nc) in [
        (&requests[2], "/authenticated/object?revision=1", "00000001"),
        (&requests[3], "/final/object?revision=2", "00000002"),
    ] {
        let auth = request.header("Authorization").unwrap();
        let params = parse_digest_challenge(auth.strip_prefix("Digest ").unwrap());
        let current_cnonce = params.get("cnonce").unwrap();
        assert_eq!(
            cnonce.get_or_insert_with(|| current_cnonce.clone()),
            current_cnonce
        );
        let expected = build_digest_authorization(
            DIGEST_CHALLENGE.strip_prefix("Digest ").unwrap(),
            "fixture",
            "fixture",
            "PUT",
            uri,
            current_cnonce,
            nc,
        )
        .unwrap();
        assert_eq!(auth, expected);
    }
}

#[test]
fn invalid_digest_credentials_stop_after_one_authentication_retry() {
    let (remote, server) = scripted_server(vec![
        ("PUT /dav/object HTTP/1.1", unauthorized()),
        ("PUT /dav/object HTTP/1.1", unauthorized()),
    ]);
    let remote = with_credentials(remote);
    let result = remote.send(
        Method::PUT,
        &format!("{}/object", remote.endpoint),
        Some(b"upload".to_vec()),
    );
    assert_eq!(server.join().unwrap().len(), 2);
    assert_eq!(result.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn four_redirect_hops_allow_the_final_request_with_method_and_body_intact() {
    for status in [301, 302, 307, 308] {
        let mut moved = redirect("/dav/object");
        moved.status = status;
        let (remote, server) = scripted_server(vec![
            ("PUT /dav/object HTTP/1.1", redirect("/one")),
            ("PUT /one HTTP/1.1", redirect("/two")),
            ("PUT /two HTTP/1.1", redirect("/three")),
            ("PUT /three HTTP/1.1", moved),
            ("PUT /dav/object HTTP/1.1", reply(201)),
        ]);
        let result = remote.send(
            Method::PUT,
            &format!("{}/object", remote.endpoint),
            Some(b"upload".to_vec()),
        );
        for request in server.join().unwrap() {
            assert_eq!(request.body, b"upload");
        }
        assert_eq!(result.unwrap().status(), StatusCode::CREATED);
    }
}

#[test]
fn a_fifth_redirect_is_rejected_without_resetting_the_budget_after_digest() {
    for digest in [false, true] {
        let mut script = vec![("PUT /dav/object HTTP/1.1", redirect("/one"))];
        if digest {
            script.push(("PUT /one HTTP/1.1", unauthorized()));
        }
        script.extend([
            ("PUT /one HTTP/1.1", redirect("/two")),
            ("PUT /two HTTP/1.1", redirect("/three")),
            ("PUT /three HTTP/1.1", redirect("/four")),
            ("PUT /four HTTP/1.1", redirect("/five")),
        ]);
        let (remote, server) = scripted_server(script);
        let remote = with_credentials(remote);
        let result = remote.send(
            Method::PUT,
            &format!("{}/object", remote.endpoint),
            Some(b"upload".to_vec()),
        );
        assert_eq!(server.join().unwrap().len(), if digest { 6 } else { 5 });
        assert!(
            matches!(result, Err(CloudSyncError::Remote(detail)) if detail.contains("redirect chain is too long"))
        );
    }
}

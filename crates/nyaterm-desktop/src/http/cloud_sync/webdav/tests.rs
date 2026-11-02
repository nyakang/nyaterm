use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use nyaterm_core::{CloudSyncError, CloudSyncRemote, WebdavSyncSettings};

use super::NativeWebdavRemote;

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
    fn answer(&self, stream: &mut std::net::TcpStream) -> (String, String) {
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
        let line = head.lines().next().unwrap_or_default().to_string();
        let mut response = format!("HTTP/1.1 {} Fixture\r\n", self.status);
        for (name, value) in &self.headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        response.push_str("Connection: close\r\n\r\n");
        response.push_str(self.body);
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
        (line, String::from_utf8_lossy(&body).to_string())
    }
}

fn scripted_server(
    requests: Vec<(&'static str, Reply)>,
) -> (NativeWebdavRemote, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let remote = NativeWebdavRemote::new(&WebdavSyncSettings {
        endpoint: format!("http://{}/dav", listener.local_addr().unwrap()),
        root: "apps/nyaterm".into(),
        ..Default::default()
    })
    .unwrap();
    let handle = std::thread::spawn(move || {
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
            let (line, _body) = reply.answer(&mut stream);
            assert_eq!(line, expected);
        }
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
    let result = remote.send(
        zed_reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
        &url,
        Some(b"<propfind/>".to_vec()),
    );
    server.join().unwrap();
    assert_eq!(result.unwrap().status().as_u16(), 207);
}

#[test]
fn a_redirect_to_another_host_is_not_followed_with_credentials() {
    let (remote, server) = scripted_server(vec![(
        "GET /dav/apps/nyaterm/sync/latest.redb HTTP/1.1",
        redirect("http://other.example/dav/apps/nyaterm/sync/latest.redb"),
    )]);
    let result = remote.read_if_exists("sync/latest.redb");
    server.join().unwrap();
    assert!(matches!(
        result,
        Err(CloudSyncError::Http { status: 301, .. })
    ));
}

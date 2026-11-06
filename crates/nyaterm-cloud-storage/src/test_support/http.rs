//! Small stateful HTTP fixture shared by native/OpenDAL compatibility tests.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use nyaterm_core::{S3SyncSettings, WebdavSyncSettings};
use sha2::{Digest, Sha256};

pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    fn read(stream: &mut TcpStream) -> std::io::Result<Self> {
        // Winsock accept inherits the listener's nonblocking mode.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            head.push(byte[0]);
            assert!(head.len() < 64 * 1024);
        }
        let head = String::from_utf8(head).unwrap();
        let mut lines = head.lines();
        let mut first = lines.next().unwrap().split_whitespace();
        let method = first.next().unwrap().into();
        let target = first.next().unwrap().into();
        let headers: BTreeMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().into()))
            .collect();
        let len = headers
            .get("content-length")
            .map(|v| v.parse().unwrap())
            .unwrap_or(0);
        let mut body = vec![0; len];
        stream.read_exact(&mut body)?;
        Ok(Self {
            method,
            target,
            headers,
            body,
        })
    }
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub advertised_length: Option<usize>,
}

impl Reply {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            advertised_length: None,
        }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    fn send(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.status,
            self.advertised_length.unwrap_or(self.body.len())
        )?;
        for (name, value) in &self.headers {
            write!(stream, "{name}: {value}\r\n")?;
        }
        stream.write_all(b"\r\n")?;
        stream.write_all(&self.body)
    }
}

pub struct Server {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn new(mut respond: impl FnMut(&Request) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let thread = crate::thread::spawn_joinable("cloud-fixture", move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let Ok(request) = Request::read(&mut stream) else {
                            continue;
                        };
                        let reply = respond(&request);
                        received.lock().unwrap().push(request);
                        let _ = reply.send(&mut stream);
                        // Windows can reset a just-closed socket before the
                        // peer has consumed its response. Half-close and drain
                        // to make fixture failures represent protocol behavior.
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        let _ = std::io::copy(&mut stream, &mut std::io::sink());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
        })
        .unwrap();
        Self {
            endpoint,
            stop,
            thread: Some(thread),
            requests,
        }
    }

    pub fn storage() -> Self {
        let mut files = BTreeMap::<String, Vec<u8>>::new();
        let mut directories = BTreeSet::from(["/".to_owned(), "/dav/".to_owned()]);
        Self::new(move |request| {
            let url = url::Url::parse(&format!("http://fixture{}", request.target)).unwrap();
            let path = url.path().to_owned();
            let query: BTreeMap<_, _> = url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if query.get("list-type").is_some_and(|value| value == "2") {
                let prefix = query.get("prefix").unwrap();
                let after = query.get("continuation-token");
                let objects: Vec<_> = files
                    .iter()
                    .filter_map(|(key, value)| {
                        key.strip_prefix("/fixture/").map(|key| (key, value))
                    })
                    .filter(|(key, _)| {
                        key.starts_with(prefix) && after.is_none_or(|after| *key > after.as_str())
                    })
                    .collect();
                let truncated = objects.len() > 2;
                let page: Vec<_> = objects.into_iter().take(2).collect();
                let mut xml = format!(
                    "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>{truncated}</IsTruncated>"
                );
                for (key, body) in &page {
                    xml.push_str(&format!("<Contents><Key>{key}</Key><Size>{}</Size><ETag>{}</ETag><LastModified>2026-11-04T00:00:00Z</LastModified></Contents>", body.len(), etag(body)));
                }
                if truncated {
                    xml.push_str(&format!(
                        "<NextContinuationToken>{}</NextContinuationToken>",
                        page.last().unwrap().0
                    ));
                }
                xml.push_str("</ListBucketResult>");
                return Reply::new(200, xml);
            }
            match request.method.as_str() {
                "MKCOL" => {
                    if directories.contains(&path) {
                        return Reply::new(405, "");
                    }
                    if !directories.contains(parent(&path)) {
                        return Reply::new(409, "");
                    }
                    directories.insert(path);
                    Reply::new(201, "")
                }
                "PROPFIND" => {
                    if !directories.contains(&path) {
                        return Reply::new(404, "");
                    }
                    let mut xml = String::from("<D:multistatus xmlns:D=\"DAV:\">");
                    xml.push_str(&dav_entry(&path, None));
                    if request
                        .headers
                        .get("depth")
                        .is_some_and(|value| value == "1")
                    {
                        for directory in &directories {
                            if directory != &path && parent(directory) == path {
                                xml.push_str(&dav_entry(directory, None));
                            }
                        }
                        for (file, body) in &files {
                            if parent(file) == path {
                                xml.push_str(&dav_entry(file, Some(body)));
                            }
                        }
                    }
                    xml.push_str("</D:multistatus>");
                    Reply::new(207, xml)
                }
                "PUT" => {
                    if path.starts_with("/dav/") && !directories.contains(parent(&path)) {
                        return Reply::new(409, "");
                    }
                    if request
                        .headers
                        .get("if-none-match")
                        .is_some_and(|value| value == "*")
                        && files.contains_key(&path)
                    {
                        return Reply::new(412, "");
                    }
                    files.insert(path, request.body.clone());
                    Reply::new(200, "")
                }
                "GET" => match files.get(&path) {
                    None => Reply::new(404, ""),
                    Some(body) if request.headers.get("if-none-match") == Some(&etag(body)) => {
                        Reply::new(304, "")
                    }
                    Some(body) => Reply::new(200, body.clone()).header("ETag", &etag(body)),
                },
                "DELETE" => {
                    files.remove(&path);
                    Reply::new(204, "")
                }
                "MOVE" => {
                    assert_eq!(
                        request.headers.get("overwrite").map(String::as_str),
                        Some("F")
                    );
                    let target = url::Url::parse(request.headers.get("destination").unwrap())
                        .unwrap()
                        .path()
                        .to_owned();
                    if files.contains_key(&target) {
                        return Reply::new(412, "");
                    }
                    match files.remove(&path) {
                        Some(body) => {
                            files.insert(target, body);
                            Reply::new(201, "")
                        }
                        None => Reply::new(404, ""),
                    }
                }
                other => panic!("unexpected fixture method {other}"),
            }
        })
    }

    pub fn webdav(&self) -> WebdavSyncSettings {
        WebdavSyncSettings {
            endpoint: format!("{}/dav", self.endpoint),
            root: "apps/nyaterm".into(),
            username: "fixture".into(),
            password: Some("fixture-password".into()),
        }
    }

    pub fn s3(&self) -> S3SyncSettings {
        S3SyncSettings {
            endpoint: self.endpoint.clone(),
            bucket: "fixture".into(),
            region: "us-east-1".into(),
            root: "apps/nyaterm".into(),
            access_key_id: Some("fixture-key".into()),
            secret_access_key: Some("fixture-secret".into()),
            session_token: Some("fixture-session".into()),
            ..Default::default()
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn parent(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    path.rfind('/').map_or("/", |index| &path[..=index])
}

fn etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(body)))
}

fn dav_entry(path: &str, body: Option<&[u8]>) -> String {
    let props = if let Some(body) = body {
        format!(
            "<D:resourcetype/><D:getcontentlength>{}</D:getcontentlength><D:getetag>{}</D:getetag>",
            body.len(),
            etag(body)
        )
    } else {
        "<D:resourcetype><D:collection/></D:resourcetype>".into()
    };
    format!(
        "<D:response><D:href>{path}</D:href><D:propstat><D:prop>{props}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
    )
}

//! Counts application bodies at the HTTP boundary, including authentication retries.

use std::cell::RefCell;
use std::io::Read;
use std::sync::{Arc, Mutex};

use nyaterm_core::CloudSyncError;
use nyaterm_core::cloud_sync::io::CloudSyncIoStats;

thread_local! {
    static CURRENT: RefCell<Option<Arc<Mutex<CloudSyncIoStats>>>> = const { RefCell::new(None) };
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static DRIVE_ENDPOINT: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn test_drive_endpoint() -> Option<String> {
    DRIVE_ENDPOINT.with_borrow(Clone::clone)
}

#[cfg(any(test, feature = "test-support"))]
pub fn with_test_drive_http<T>(endpoint: &str, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DRIVE_ENDPOINT.with_borrow_mut(|current| *current = self.0.take());
        }
    }
    let _restore = Restore(DRIVE_ENDPOINT.with_borrow_mut(|v| v.replace(endpoint.into())));
    operation()
}

/// Join this thread's counters, or install a new root scope.
///
/// Only the caller receiving `true` must call [`end_scope`], after nested
/// operations have finished. Adapters bind the same counters to their workers.
pub fn begin_scope() -> (Arc<Mutex<CloudSyncIoStats>>, bool) {
    CURRENT.with_borrow_mut(|current| match current {
        Some(stats) => (stats.clone(), false),
        None => {
            let stats = Arc::new(Mutex::new(CloudSyncIoStats::default()));
            *current = Some(stats.clone());
            (stats, true)
        }
    })
}

/// Clear a root scope installed by [`begin_scope`] on the calling thread.
pub fn end_scope() {
    CURRENT.with_borrow_mut(|current| *current = None);
}

/// Bind compatibility requests on a worker to the operation's existing counters.
pub(super) fn with_stats<T>(
    stats: Arc<Mutex<CloudSyncIoStats>>,
    operation: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<Arc<Mutex<CloudSyncIoStats>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with_borrow_mut(|current| *current = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with_borrow_mut(|current| current.replace(stats)));
    operation()
}

fn record(update: impl FnOnce(&mut CloudSyncIoStats)) {
    CURRENT.with_borrow(|current| {
        if let Some(stats) = current {
            update(
                &mut stats
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    });
}

pub(super) trait CountedRequest {
    fn send_counted(self) -> Result<zed_reqwest::blocking::Response, zed_reqwest::Error>;
}

impl CountedRequest for zed_reqwest::blocking::RequestBuilder {
    fn send_counted(self) -> Result<zed_reqwest::blocking::Response, zed_reqwest::Error> {
        let (client, request) = self.build_split();
        let request = request?;
        #[cfg(any(test, feature = "test-support"))]
        let request = {
            let mut request = request;
            if let Some(endpoint) = test_drive_endpoint() {
                let url = request.url();
                let rewritten = format!(
                    "{endpoint}/{}{}{}",
                    url.host_str().unwrap_or_default(),
                    url.path(),
                    url.query().map(|q| format!("?{q}")).unwrap_or_default()
                );
                *request.url_mut() = url::Url::parse(&rewritten).expect("fixture URL");
            }
            request
        };
        // Cloud adapters submit buffered JSON, form and object bodies. Count each
        // submitted attempt, including a failed send; never inspect/log its data.
        let upload = request
            .body()
            .and_then(|body| body.as_bytes())
            .map_or(0, <[u8]>::len);
        record(|stats| {
            stats.requests += 1;
            stats.uploaded_bytes += upload as u64;
        });
        let response = client.execute(request)?;
        if response.status() == zed_reqwest::StatusCode::NOT_MODIFIED {
            record(|stats| stats.not_modified += 1);
        }
        Ok(response)
    }
}

pub(super) trait CountedResponse {
    fn bytes_counted(self) -> Result<Vec<u8>, CloudSyncError>;
    fn text_counted(self) -> Result<String, CloudSyncError>;
}

impl CountedResponse for zed_reqwest::blocking::Response {
    fn bytes_counted(mut self) -> Result<Vec<u8>, CloudSyncError> {
        let mut body = Vec::new();
        let mut buffer = [0; 8_192];
        loop {
            let count = self.read(&mut buffer).map_err(|error| {
                CloudSyncError::Io(std::io::Error::new(
                    error.kind(),
                    "cloud response body could not be read",
                ))
            })?;
            if count == 0 {
                break;
            }
            record(|stats| stats.downloaded_bytes += count as u64);
            body.extend_from_slice(&buffer[..count]);
        }
        Ok(body)
    }

    fn text_counted(self) -> Result<String, CloudSyncError> {
        // Cloud APIs and WebDAV XML use UTF-8. Match reqwest's UTF-8 lossy
        // conversion, including its removal of a leading UTF-8 BOM.
        let body = self.bytes_counted()?;
        let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&body);
        Ok(String::from_utf8_lossy(body).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{CountedRequest, CountedResponse};
    use crate::NativeWebdavRemote;
    use nyaterm_core::{CloudSyncRemote, WebdavSyncSettings};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn cloud_sync_http_metrics_include_digest_retries_and_partial_failed_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                if index == 0 {
                    write!(stream, "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"fixture\", nonce=\"fixture\", qop=\"auth\"\r\nContent-Length: 3\r\nConnection: close\r\n\r\nold").unwrap();
                } else if index == 1 {
                    assert!(request.contains("authorization: digest "));
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata"
                    )
                    .unwrap();
                } else {
                    stream.read_exact(&mut [0; 6]).unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 503 Failure\r\nContent-Length: 32768\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                    stream.write_all(&[b'x'; 16384]).unwrap();
                }
            }
        });
        struct Scope;
        impl Drop for Scope {
            fn drop(&mut self) {
                super::end_scope();
            }
        }
        let _scope = Scope;
        let (_, root) = super::begin_scope();
        assert!(root);
        let (stats, root) = super::begin_scope();
        assert!(!root);
        let remote = NativeWebdavRemote::new(&WebdavSyncSettings {
            endpoint: url.clone(),
            root: String::new(),
            username: "fixture".into(),
            password: Some("fixture".into()),
        })
        .unwrap();
        assert_eq!(remote.read_if_exists("object").unwrap().unwrap(), b"data");
        let response = zed_reqwest::blocking::Client::new()
            .post(&url)
            .body("upload")
            .send_counted()
            .unwrap();
        let error = response.bytes_counted().unwrap_err();
        assert!(!format!("{error} {error:?}").contains(&url));
        server.join().unwrap();
        let stats = *stats.lock().unwrap();
        assert_eq!(stats.requests, 3);
        assert_eq!(stats.uploaded_bytes, 6);
        assert!(stats.downloaded_bytes >= 8192 + 7);
        assert!(stats.downloaded_bytes <= 16384 + 7);
    }
}

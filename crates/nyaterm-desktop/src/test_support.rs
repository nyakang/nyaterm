use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nyaterm_store::{FlushBarrier, StoreBlockingClient, StoreConfig, StoreRuntime};

pub(crate) use nyaterm_core::test_support::TestTempDir as TestConfigDir;

/// Runs GPUI's test cleanup when a plain Rust test leaves its context's scope.
///
/// `add_window_view` and `add_empty_window` retain visual contexts until `quit`.
/// Dropping the original `TestAppContext` alone leaves those contexts, windows,
/// entities and their workers alive. Unlike `#[gpui::test]`, `#[test]` does not
/// call `quit` for us. Keep this guard in the owning scope, including when the
/// context variable is shadowed by a borrowed visual context.
#[must_use = "keep the cleanup guard alive until the test context is no longer used"]
pub(crate) struct TestAppCleanup {
    cx: gpui::TestAppContext,
}

impl TestAppCleanup {
    pub(crate) fn new(cx: &gpui::TestAppContext) -> Self {
        Self { cx: cx.clone() }
    }
}

impl Drop for TestAppCleanup {
    fn drop(&mut self) {
        self.cx.quit();
    }
}

/// Opens an isolated test store and waits until its redb worker is ready.
///
/// Waiting on the barrier is part of the cleanup contract: without it, a very
/// short test can remove its untouched directory before the worker starts, and
/// the late worker then recreates `config/nyaterm.redb` after cleanup.
pub(crate) fn blocking_test_store(root: &Path) -> StoreBlockingClient {
    let runtime = StoreRuntime::spawn(StoreConfig {
        config_dir: root.join("config"),
        portable_key_path: None,
    })
    .expect("spawn test store");
    let store = runtime.blocking_client();
    store
        .request(0, FlushBarrier)
        .expect("receive test store barrier")
        .outcome
        .expect("initialize test store");
    store
}

pub(crate) fn spawn_webdav_service_unavailable_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock WebDAV listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let server = std::thread::spawn(move || {
        // Sync jobs queue behind the process-wide lock during parallel test runs.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "WebDAV request was not sent");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        let read_deadline = Instant::now() + Duration::from_secs(15);
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    assert!(
                        Instant::now() < read_deadline,
                        "WebDAV request headers timed out"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("mock request headers: {error}"),
            };
            assert!(count > 0, "incomplete request");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16 * 1024, "oversized mock request");
        }
        assert!(request.starts_with(b"MKCOL "));
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("mock response");
    });
    (endpoint, server)
}

/// Strictly exercises the metadata reads used by a connection probe.
pub(crate) fn spawn_webdav_healthy_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock WebDAV listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let server = std::thread::spawn(move || {
        for method in ["PROPFIND", "GET"] {
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing WebDAV {method}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).expect("blocking stream");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).expect("request headers");
                request.push(byte[0]);
                assert!(request.len() < 16 * 1024, "oversized mock headers");
            }
            let head = String::from_utf8(request).expect("HTTP headers");
            let mut line = head
                .lines()
                .next()
                .expect("request line")
                .split_whitespace();
            assert_eq!(line.next(), Some(method));
            let path = line.next().expect("request path");
            let header = |name: &str| {
                head.lines()
                    .filter_map(|line| line.split_once(':'))
                    .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then(|| value.trim()))
            };
            let length: usize = header("Content-Length").unwrap_or("0").parse().unwrap();
            assert!(length < 16 * 1024, "oversized mock body");
            let mut body = vec![0; length];
            stream.read_exact(&mut body).expect("request body");
            if method == "PROPFIND" {
                assert!(path.ends_with("/sync/heads/"), "unexpected listing path");
                assert_eq!(header("Depth"), Some("1"));
                assert!(body.windows(8).any(|part| part == b"propfind"));
                let body = "<D:multistatus xmlns:D=\"DAV:\"/>";
                write!(stream, "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            } else {
                assert!(
                    path.ends_with("/sync/latest.redb"),
                    "unexpected pointer path"
                );
                assert!(body.is_empty());
                stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    });
    (endpoint, server)
}

#[test]
fn test_config_dir_removes_its_tree_on_drop() {
    let path = {
        let dir = TestConfigDir::new("nyaterm-test-cleanup");
        std::fs::create_dir_all(dir.path().join("config")).expect("create test tree");
        dir.path().to_path_buf()
    };

    assert!(!path.exists(), "temporary test tree was not removed");
}

#[test]
fn synchronized_test_store_cannot_recreate_its_tree_after_cleanup() {
    let path = {
        let dir = TestConfigDir::new("nyaterm-test-store-cleanup");
        let path = dir.path().to_path_buf();
        let store = blocking_test_store(dir.path());
        assert!(path.join("config/nyaterm.redb").is_file());
        drop(store);
        path
    };

    // This delay would expose the old race: an unsynchronized worker could
    // start after the guard returned and recreate the database behind it.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !path.exists(),
        "the storage worker recreated its temporary tree after cleanup"
    );
}

#[cfg(test)]
mod gpui_cleanup_tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use gpui::{Context, Entity, IntoElement, Render, TestAppContext, Window, div};

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    struct WindowProbe {
        released: Rc<Cell<usize>>,
    }

    impl Render for WindowProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    impl Drop for WindowProbe {
        fn drop(&mut self) {
            self.released.set(self.released.get() + 1);
        }
    }

    fn assert_visual_cleanup(panic_in_test: bool) {
        let released = Rc::new(Cell::new(0));
        let quit_called = Rc::new(Cell::new(false));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = TestAppCleanup::new(&cx);
            let quit_called = Rc::clone(&quit_called);
            cx.on_quit(move || quit_called.set(true));
            cx.add_empty_window();
            for _ in 0..2 {
                let released = Rc::clone(&released);
                let (_, cx) = cx.add_window_view(move |_, _| WindowProbe { released });
                cx.run_until_parked();
            }
            assert_eq!(released.get(), 0);
            if panic_in_test {
                panic!("exercise GPUI cleanup on unwind");
            }
        }));
        assert_eq!(result.is_err(), panic_in_test);
        assert!(quit_called.get(), "GPUI quit callbacks were not run");
        assert_eq!(released.get(), 2, "window entities survived test cleanup");
    }

    #[test]
    fn visual_contexts_release_window_entities_at_scope_exit() {
        assert_visual_cleanup(false);
    }

    #[test]
    fn visual_contexts_release_window_entities_during_panic_unwind() {
        assert_visual_cleanup(true);
    }

    struct AppWindow {
        _app: Entity<NyaTermApp>,
    }

    impl Render for AppWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn repeated_visual_tests_release_apps_and_their_worker_owned_resources() {
        for _ in 0..16 {
            let root = TestConfigDir::new("nyaterm-visual-test-cleanup");
            let weak = {
                let mut cx = TestAppContext::single();
                let _test_app_cleanup = TestAppCleanup::new(&cx);
                let app = app_with_visible_local_session(&mut cx, root.path(), "session");
                let weak = app.downgrade();
                cx.add_window_view(move |_, _| AppWindow { _app: app });
                weak
            };
            assert!(
                weak.upgrade().is_none(),
                "test window retained its NyaTermApp"
            );
            assert!(
                !root.path().exists(),
                "worker-owned app resources survived cleanup"
            );
        }
    }
}

//! Named joinable workers owned and reaped by each storage adapter.
use std::io;
use std::thread::{self, JoinHandle};

pub(crate) fn spawn_joinable<T: Send + 'static>(
    name: impl Into<String>,
    run: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    thread::Builder::new().name(name.into()).spawn(run)
}

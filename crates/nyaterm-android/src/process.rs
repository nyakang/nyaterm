//! Bounded child reaping, shared by startup failure and session shutdown.
use std::io;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// Call only on a background worker. Always wait after forced termination.
pub fn reap(child: &mut Child, grace: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + grace;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            return child.wait();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

//! Best-effort local Rust panic events, independent of GPUI and tracing.
//!
//! Messages and thread names are intentionally saved verbatim. Reports are not
//! included in diagnostic exports and do not establish that the process crashed.

use std::backtrace::Backtrace;
use std::cmp::Reverse;
use std::fmt::{self, Write as _};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_REPORTS_PER_PROCESS: usize = 20;
const MAX_RETAINED_REPORTS: usize = 30;
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TRUNCATION_MARKER: &str = "\n[report truncated]\n";

/// Install once during startup, before other application threads are started.
/// Directory creation failure leaves the previous hook untouched.
pub fn install(crash_dir: PathBuf) -> io::Result<()> {
    fs::create_dir_all(&crash_dir)?;
    let previous_hook = panic::take_hook();
    let attempts = AtomicUsize::new(0);
    panic::set_hook(Box::new(move |info| {
        let sequence = attempts.try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < MAX_REPORTS_PER_PROCESS).then(|| count + 1)
        });
        if let Ok(sequence) = sequence {
            let _ = write_panic_report(&crash_dir, info, sequence + 1);
        }
        previous_hook(info);
    }));
    Ok(())
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn write_panic_report(
    crash_dir: &Path,
    info: &PanicHookInfo<'_>,
    sequence: usize,
) -> io::Result<()> {
    let timestamp = timestamp_ms();
    let pid = std::process::id();
    let path = crash_dir.join(format!("panic-{timestamp}-{pid}-{sequence}.log"));
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BoundedWriter::new(file);
    let _ = write_report_body(&mut writer, info, timestamp, pid);
    writer.finish()
}

fn write_report_body<W: Write>(
    writer: &mut BoundedWriter<W>,
    info: &PanicHookInfo<'_>,
    timestamp: u128,
    pid: u32,
) -> fmt::Result {
    writeln!(writer, "NyaTerm Panic Report\nFormat-Version: 1")?;
    writeln!(
        writer,
        "Event: rust_panic\nTimestamp-UTC-Unix-Ms: {timestamp}"
    )?;
    writeln!(writer, "Version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(
        writer,
        "OS: {}\nArch: {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    let thread = std::thread::current();
    writeln!(writer, "PID: {pid}\nThread-ID: {:?}", thread.id())?;
    // Put the source location before any unbounded user-controlled strings.
    if let Some(location) = info.location() {
        writeln!(
            writer,
            "Location: {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        )?;
    } else {
        writeln!(writer, "Location: <unknown>")?;
    }
    writeln!(
        writer,
        "Thread-Name: {}",
        thread.name().unwrap_or("<unnamed>")
    )?;
    let message = if let Some(message) = info.payload().downcast_ref::<&str>() {
        message
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.as_str()
    } else {
        "<non-string panic payload>"
    };
    writeln!(writer, "\nMessage:\n{message}\n\nBacktrace:")?;
    // Basic information has already reached the file before symbol resolution.
    write!(writer, "{}", Backtrace::force_capture())
}

/// Reserves space for a truncation marker and never builds a full report string.
/// fmt::Error only stops formatting; the original I/O error is returned by finish.
struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
    truncated: bool,
    error: Option<io::Error>,
}

impl<W: Write> BoundedWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            remaining: MAX_REPORT_BYTES - TRUNCATION_MARKER.len(),
            truncated: false,
            error: None,
        }
    }

    fn finish(mut self) -> io::Result<()> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.truncated {
            self.inner.write_all(TRUNCATION_MARKER.as_bytes())?;
        }
        self.inner.flush()
    }
}

impl<W: Write> fmt::Write for BoundedWriter<W> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.error.is_some() || self.truncated {
            return Err(fmt::Error);
        }
        let mut end = value.len().min(self.remaining);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        if let Err(error) = self.inner.write_all(&value.as_bytes()[..end]) {
            self.error = Some(error);
            return Err(fmt::Error);
        }
        self.remaining -= end;
        if end < value.len() {
            self.truncated = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}

/// Run only in the instance owner, outside the panic hook and GPUI callbacks.
pub fn cleanup_old_reports(crash_dir: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(crash_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let now = timestamp_ms();
    let mut retained = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(key) = name.to_str().and_then(report_key) else {
            continue;
        };
        if now.saturating_sub(key.0) > RETENTION.as_millis() {
            fs::remove_file(entry.path())?;
        } else {
            retained.push((key, entry.path()));
        }
    }
    retained.sort_unstable_by_key(|entry| Reverse(entry.0));
    for (_, path) in retained.into_iter().skip(MAX_RETAINED_REPORTS) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn report_key(name: &str) -> Option<(u128, u32, usize)> {
    let mut parts = name
        .strip_prefix("panic-")?
        .strip_suffix(".log")?
        .split('-');
    let timestamp = parts.next()?;
    let pid = parts.next()?;
    let sequence = parts.next()?;
    if parts.next().is_some()
        || [timestamp, pid, sequence]
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let key = (
        timestamp.parse().ok()?,
        pid.parse().ok()?,
        sequence.parse().ok()?,
    );
    (key.1 > 0 && key.2 > 0).then_some(key)
}

#[cfg(test)]
mod tests {
    use crate::crash_report::{
        BoundedWriter, MAX_REPORT_BYTES, TRUNCATION_MARKER, cleanup_old_reports, report_key,
        timestamp_ms,
    };
    use nyaterm_core::test_support::TestTempDir;
    use std::fmt::Write as _;
    use std::fs;
    use std::io::{self, Write};

    #[test]
    fn truncation_keeps_utf8_valid_and_includes_marker_in_budget() {
        for prefix in 0..4 {
            let mut bytes = Vec::new();
            let mut writer = BoundedWriter::new(&mut bytes);
            writer.write_str(&"a".repeat(prefix)).unwrap();
            assert!(writer.write_str(&"猫🐈".repeat(MAX_REPORT_BYTES)).is_err());
            writer.finish().unwrap();
            assert!(bytes.len() <= MAX_REPORT_BYTES);
            assert!(
                std::str::from_utf8(&bytes)
                    .unwrap()
                    .ends_with(TRUNCATION_MARKER)
            );
        }
    }

    #[test]
    fn write_failure_preserves_partial_report_and_original_error() {
        struct FailingWriter(Vec<u8>);
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let count = bytes.len().min(8 - self.0.len());
                if count == 0 {
                    return Err(io::Error::new(io::ErrorKind::StorageFull, "test disk full"));
                }
                self.0.extend_from_slice(&bytes[..count]);
                Ok(count)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut sink = FailingWriter(Vec::new());
        let mut writer = BoundedWriter::new(&mut sink);
        assert!(writer.write_str("a partial panic report").is_err());
        assert_eq!(
            writer.finish().unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(sink.0, b"a partia");
    }

    #[test]
    fn cleanup_preserves_newest_reports_and_ignores_unrelated_entries() {
        let root = TestTempDir::new("nyaterm-panic-retention");
        fs::create_dir_all(root.path()).unwrap();
        let now = timestamp_ms();
        for sequence in 1..=35 {
            fs::write(
                root.join(format!("panic-{now}-1-{sequence}.log")),
                b"recent",
            )
            .unwrap();
        }
        let old = root.join("panic-0-1-1.log");
        fs::write(&old, b"old").unwrap();
        let unrelated = root.join("panic-0-not-a-report.log");
        fs::write(&unrelated, b"unrelated").unwrap();
        let directory = root.join("panic-0-2-1.log");
        fs::create_dir(&directory).unwrap();
        cleanup_old_reports(root.path()).unwrap();
        assert!(!old.exists());
        assert!(unrelated.is_file());
        assert!(directory.is_dir());
        for sequence in 1..=35 {
            assert_eq!(
                root.join(format!("panic-{now}-1-{sequence}.log")).exists(),
                sequence > 5
            );
        }
        cleanup_old_reports(&root.join("missing")).unwrap();
    }

    #[test]
    fn cleanup_only_accepts_owned_report_names() {
        assert_eq!(report_key("panic-123-45-6.log"), Some((123, 45, 6)));
        for name in [
            "panic-1-2-0.log",
            "panic-1-0-1.log",
            "panic-+1-2-3.log",
            "panic-1-2-3-4.log",
            "panic-1-2-3.log.bak",
            "panic-1--3.log",
        ] {
            assert!(report_key(name).is_none(), "{name}");
        }
    }

    #[test]
    fn cleanup_does_not_follow_or_remove_report_symlinks() {
        let root = TestTempDir::new("nyaterm-panic-symlink");
        let target = TestTempDir::new("nyaterm-panic-symlink-target");
        fs::create_dir_all(root.path()).unwrap();
        fs::create_dir_all(target.path()).unwrap();
        let original = target.join("original.log");
        fs::write(&original, b"keep this file").unwrap();
        let link = root.join("panic-0-1-1.log");
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(&original, &link);
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&original, &link);
        if let Err(error) = result {
            // Windows can require Developer Mode or an elevated symlink privilege.
            if error.kind() == io::ErrorKind::PermissionDenied {
                eprintln!("symlink test unavailable: {error}");
                return;
            }
            panic!("create test symlink: {error}");
        }
        cleanup_old_reports(root.path()).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(original).unwrap(), b"keep this file");
    }
}

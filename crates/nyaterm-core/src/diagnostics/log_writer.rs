//! Size-sharded, locally dated diagnostics log files.
//!
//! One file per local day keeps the historical `<prefix>.<YYYY-MM-DD>.jsonl`
//! contract. Once a file reaches the configured byte budget the writer moves to
//! `<prefix>.<YYYY-MM-DD>_<NNN>.jsonl` (three-digit shard index starting at
//! `001`, zero padded). Lexicographic sorting therefore stays chronological:
//! the base file sorts first, later shards follow in write order, and the
//! zero-padded local date keeps day boundaries ordered.
//!
//! Files whose name does not parse as `<prefix>.<date>[_<shard>].jsonl` (for
//! example `<prefix>-cloud-sync.jsonl`) are never pruned by this module.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use time::{Date, Duration, Month, OffsetDateTime};

use super::{LOG_FILE_PREFIX, LOG_FILE_SUFFIX, normalize_retention_days};

/// Default per-file budget shown on fresh installs and when the setting is unset.
pub const DEFAULT_LOG_SHARD_BYTES: u64 = 10 * 1024 * 1024;
/// Lower bound accepted for a log shard; tiny values would shard on every event.
pub const MIN_LOG_SHARD_BYTES: u64 = 1024 * 1024;
/// Upper bound accepted for a log shard; still well above the 10 MiB default.
pub const MAX_LOG_SHARD_BYTES: u64 = 100 * 1024 * 1024;

const MEBIBYTE: u64 = 1024 * 1024;

/// Process-wide shard budget read by the writer on every line. Settings update
/// this atomically so a change applies to the running process without restart.
static LOG_SHARD_BYTES: AtomicU64 = AtomicU64::new(DEFAULT_LOG_SHARD_BYTES);
/// Process-wide retention window used when the writer prunes on startup or when
/// it crosses a local-day boundary.
static LOG_RETENTION_DAYS: AtomicU32 = AtomicU32::new(super::DEFAULT_RETENTION_DAYS);
/// Set once persisted settings reach the log layer. Pruning waits for this so a
/// startup default can never delete files the user's longer retention keeps.
static LOG_SETTINGS_LOADED: AtomicBool = AtomicBool::new(false);

/// Clamp a requested shard budget into the supported range.
///
/// `0` (unset or corrupted) maps to [`DEFAULT_LOG_SHARD_BYTES`]; values below
/// [`MIN_LOG_SHARD_BYTES`] clamp up, values above [`MAX_LOG_SHARD_BYTES`] clamp
/// down, and the result rounds up to a whole MiB so the UI and file sizes agree.
pub fn normalize_log_shard_bytes(bytes: u64) -> u64 {
    if bytes == 0 {
        return DEFAULT_LOG_SHARD_BYTES;
    }
    bytes
        .clamp(MIN_LOG_SHARD_BYTES, MAX_LOG_SHARD_BYTES)
        .div_ceil(MEBIBYTE)
        * MEBIBYTE
}

pub fn log_shard_bytes() -> u64 {
    LOG_SHARD_BYTES.load(Ordering::Relaxed)
}

/// Store the effective shard budget and return it after normalization.
pub fn set_log_shard_bytes(bytes: u64) -> u64 {
    let normalized = normalize_log_shard_bytes(bytes);
    LOG_SHARD_BYTES.store(normalized, Ordering::Relaxed);
    normalized
}

pub fn log_retention_days() -> u32 {
    LOG_RETENTION_DAYS.load(Ordering::Relaxed)
}

/// Store the effective retention window and return it after normalization.
pub fn set_log_retention_days(days: u32) -> u32 {
    let normalized = normalize_retention_days(days);
    LOG_RETENTION_DAYS.store(normalized, Ordering::Relaxed);
    normalized
}

/// Apply persisted diagnostics settings to the live log layer.
pub fn apply_log_runtime_settings(shard_bytes: u64, retention_days: u32) {
    set_log_shard_bytes(shard_bytes);
    set_log_retention_days(retention_days);
    LOG_SETTINGS_LOADED.store(true, Ordering::Release);
}

fn log_settings_loaded() -> bool {
    LOG_SETTINGS_LOADED.load(Ordering::Acquire)
}

/// Local wall clock with a UTC fallback on platforms that refuse to guess.
pub fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Delete diagnostics log files older than `retention_days` local days.
///
/// Only files whose name parses as a diagnostics day/shard file are considered,
/// so cloud-sync history and unrelated JSONL files survive. Returns the number
/// of removed files. Failures are returned for a deferred retry by the writer;
/// pruning errors never prevent logging.
fn prune_log_files_at(log_dir: &Path, retention_days: u32, today: Date) -> io::Result<usize> {
    let cutoff = today - Duration::days(i64::from(normalize_retention_days(retention_days)));
    let entries = match fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    let mut failure = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failure = Some(error);
                continue;
            }
        };
        let Some((file_date, _)) = entry.file_name().to_str().and_then(parse_log_file_name) else {
            continue;
        };
        if file_date >= cutoff {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(removed), Err)
}

/// File name for a given local date and shard index (`0` is the base file).
///
/// Shards use `_NNN` rather than `.NNN` because `.` sorts before the base
/// file's `.jsonl` extension; `_` sorts after it, so plain lexicographic sorting
/// yields base then `_001`, `_002`, ... and zero-padded dates keep days ordered.
pub fn log_file_name(date: Date, shard: u32) -> String {
    let date = format_date(date);
    if shard == 0 {
        format!("{LOG_FILE_PREFIX}.{date}.{LOG_FILE_SUFFIX}")
    } else {
        format!("{LOG_FILE_PREFIX}.{date}_{shard:03}.{LOG_FILE_SUFFIX}")
    }
}

/// Parse a diagnostics log file name into its local date and shard index.
pub fn parse_log_file_name(name: &str) -> Option<(Date, u32)> {
    let rest = name.strip_prefix(LOG_FILE_PREFIX)?;
    let rest = rest.strip_prefix('.')?;
    let rest = rest.strip_suffix(&format!(".{LOG_FILE_SUFFIX}"))?;
    let (date, shard) = match rest.split_once('_') {
        Some((date, shard)) => (date, shard.parse::<u32>().ok()?),
        None => (rest, 0),
    };
    Some((parse_date(date)?, shard))
}

fn format_date(date: Date) -> String {
    let (year, month, day) = date.to_calendar_date();
    format!("{year:04}-{:02}-{day:02}", u8::from(month))
}

fn parse_date(raw: &str) -> Option<Date> {
    let bytes = raw.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = raw[0..4].parse::<i32>().ok()?;
    let month = Month::try_from(raw[5..7].parse::<u8>().ok()?).ok()?;
    let day = raw[8..10].parse::<u8>().ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

struct CurrentShard {
    file: File,
    date: Date,
    len: u64,
}

/// Append-only JSONL sink that shards by byte budget and rotates by local date.
///
/// Lines are buffered until a newline arrives so a rotation can never split one
/// JSONL record across two files. Callers hand this to a non-blocking appender;
/// file IO therefore never runs on the caller's thread.
pub struct RotatingLogWriter {
    dir: PathBuf,
    pending: Vec<u8>,
    current: Option<CurrentShard>,
    prune_due: bool,
    prune_retry_at: Option<OffsetDateTime>,
    #[cfg(test)]
    test_limits: Option<TestLimits>,
}

#[cfg(test)]
struct TestLimits {
    shard_bytes: u64,
    retention_days: u32,
    clock: Box<dyn Fn() -> OffsetDateTime + Send>,
    ready: Box<dyn Fn() -> bool + Send>,
}

impl RotatingLogWriter {
    pub fn new(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        // Retention pruning is deferred until persisted settings reach the log
        // layer, so the startup default cannot delete a longer-retained history.
        Ok(Self {
            dir,
            pending: Vec::new(),
            current: None,
            prune_due: true,
            prune_retry_at: None,
            #[cfg(test)]
            test_limits: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_limits(
        dir: impl AsRef<Path>,
        shard_bytes: u64,
        retention_days: u32,
        clock: impl Fn() -> OffsetDateTime + Send + 'static,
    ) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let writer = Self {
            dir,
            pending: Vec::new(),
            current: None,
            prune_due: true,
            prune_retry_at: None,
            test_limits: Some(TestLimits {
                shard_bytes,
                retention_days,
                clock: Box::new(clock),
                ready: Box::new(|| true),
            }),
        };
        Ok(writer)
    }

    fn settings_ready(&self) -> bool {
        #[cfg(test)]
        if let Some(limits) = self.test_limits.as_ref() {
            return (limits.ready)();
        }
        log_settings_loaded()
    }

    fn shard_limit(&self) -> u64 {
        #[cfg(test)]
        if let Some(limits) = self.test_limits.as_ref() {
            return limits.shard_bytes;
        }
        log_shard_bytes()
    }

    fn retention_days(&self) -> u32 {
        #[cfg(test)]
        if let Some(limits) = self.test_limits.as_ref() {
            return limits.retention_days;
        }
        log_retention_days()
    }

    fn now(&self) -> OffsetDateTime {
        #[cfg(test)]
        if let Some(limits) = self.test_limits.as_ref() {
            return (limits.clock)();
        }
        local_now()
    }

    fn commit(&mut self, chunk: &[u8]) -> io::Result<()> {
        let now = self.now();
        let today = now.date();
        // Acquire the settings publication before reading the effective limits.
        let settings_ready = self.settings_ready();
        let limit = self.shard_limit();
        let day_changed = self
            .current
            .as_ref()
            .is_some_and(|shard| shard.date != today);
        if day_changed {
            self.current = None;
            self.prune_due = true;
        }
        if self.prune_due && settings_ready && self.prune_retry_at.is_none_or(|retry| now >= retry)
        {
            match prune_log_files_at(&self.dir, self.retention_days(), today) {
                Ok(_) => {
                    self.prune_due = false;
                    self.prune_retry_at = None;
                }
                Err(_) => self.prune_retry_at = Some(now + Duration::seconds(60)),
            }
        }
        if self
            .current
            .as_ref()
            .is_some_and(|shard| shard.len >= limit)
        {
            // Budget reached; the next shard index is selected on open.
            self.current = None;
        }
        let Some(current) = self.current.as_mut() else {
            let mut shard = open_shard(&self.dir, today, limit)?;
            shard.file.write_all(chunk)?;
            shard.len += chunk.len() as u64;
            self.current = Some(shard);
            return Ok(());
        };
        current.file.write_all(chunk)?;
        current.len += chunk.len() as u64;
        Ok(())
    }
}

impl Write for RotatingLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        let Some(newline) = self.pending.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(buf.len());
        };
        let remainder = self.pending.split_off(newline + 1);
        let ready = std::mem::replace(&mut self.pending, remainder);
        for record in ready.split_inclusive(|byte| *byte == b'\n') {
            self.commit(record)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let ready = std::mem::take(&mut self.pending);
            self.commit(&ready)?;
        }
        match self.current.as_mut() {
            Some(shard) => shard.file.flush(),
            None => Ok(()),
        }
    }
}

fn open_shard(dir: &Path, date: Date, limit: u64) -> io::Result<CurrentShard> {
    let mut highest: Option<(u32, u64)> = None;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Some((file_date, shard)) = entry.file_name().to_str().and_then(parse_log_file_name)
            else {
                continue;
            };
            if file_date != date {
                continue;
            }
            let len = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if highest.is_none_or(|(index, _)| shard >= index) {
                highest = Some((shard, len));
            }
        }
    }
    let shard = match highest {
        // Continue the newest file until it reaches the budget, then start the
        // next index. This keeps "append after restart" behaviour when the day
        // file is still small.
        Some((index, len)) if len >= limit => index.saturating_add(1),
        Some((index, _)) => index,
        None => 0,
    };
    let path = dir.join(log_file_name(date, shard));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    Ok(CurrentShard { file, date, len })
}

#[cfg(test)]
mod tests;

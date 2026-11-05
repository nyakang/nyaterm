use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use time::{Date, Month, OffsetDateTime, UtcOffset};

use super::{
    DEFAULT_LOG_SHARD_BYTES, MAX_LOG_SHARD_BYTES, MIN_LOG_SHARD_BYTES, RotatingLogWriter,
    log_file_name, normalize_log_shard_bytes, parse_log_file_name, prune_log_files_at,
};

const SHARD_BUDGET: u64 = 64;

fn local_day(day: u8) -> OffsetDateTime {
    local_time(2026, Month::November, day)
}

fn local_time(year: i32, month: Month, day: u8) -> OffsetDateTime {
    let date = Date::from_calendar_date(year, month, day).expect("valid date");
    date.with_hms(12, 0, 0)
        .expect("valid time")
        .assume_offset(UtcOffset::from_hms(8, 0, 0).expect("valid offset"))
}

fn temp_dir(name: &str) -> crate::test_support::TestTempDir {
    crate::test_support::TestTempDir::new(&format!("nyaterm-log-writer-{name}"))
}

fn writer(dir: &Path, clock: Arc<Mutex<OffsetDateTime>>) -> RotatingLogWriter {
    writer_with_budget(dir, clock, SHARD_BUDGET)
}

fn writer_with_budget(
    dir: &Path,
    clock: Arc<Mutex<OffsetDateTime>>,
    budget: u64,
) -> RotatingLogWriter {
    RotatingLogWriter::with_test_limits(dir, budget, 7, move || *clock.lock().expect("clock lock"))
        .expect("writer")
}

fn written_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    files
}

#[test]
fn shard_file_names_stay_chronological_and_parse_stably() {
    let day = Date::from_calendar_date(2026, Month::November, 4).expect("date");
    let next_day = Date::from_calendar_date(2026, Month::November, 5).expect("date");
    let base = log_file_name(day, 0);
    let names: Vec<String> = [(day, 0), (day, 1), (day, 2), (next_day, 0), (next_day, 1)]
        .into_iter()
        .map(|(date, shard)| log_file_name(date, shard))
        .collect();
    assert_eq!(base, "nyaterm-diagnostics.2026-11-04.jsonl");
    assert_eq!(names[1], "nyaterm-diagnostics.2026-11-04_001.jsonl");
    assert_eq!(names[2], "nyaterm-diagnostics.2026-11-04_002.jsonl");
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, names);
    for (name, (date, shard)) in
        names
            .iter()
            .zip([(day, 0), (day, 1), (day, 2), (next_day, 0), (next_day, 1)])
    {
        assert_eq!(parse_log_file_name(name), Some((date, shard)));
    }
    assert_eq!(
        parse_log_file_name("nyaterm-diagnostics-cloud-sync.jsonl"),
        None
    );
    assert_eq!(parse_log_file_name("nyaterm-diagnostics.notes.jsonl"), None);
}

#[test]
fn writer_shards_at_the_byte_budget_and_never_splits_a_line() {
    let dir = temp_dir("size-shards");
    let clock = Arc::new(Mutex::new(local_day(4)));
    let mut writer = writer(&dir, clock);

    let mut expected = String::new();
    for index in 0..6 {
        let line = format!("{{\"event\":\"line-{index}\"}}\n");
        expected.push_str(&line);
        writer.write_all(line.as_bytes()).expect("write line");
    }
    writer.flush().expect("flush");
    drop(writer);

    let files = written_files(dir.path());
    assert_eq!(
        files.len(),
        2,
        "64-byte budget holds three 20-byte lines per file"
    );
    let mut actual = String::new();
    for (index, path) in files.iter().enumerate() {
        let content = std::fs::read_to_string(path).expect("read shard");
        assert!(content.ends_with('\n'), "shard must end on a line boundary");
        // A budget is checked before the next line, so one record may overshoot.
        assert!(
            content.len() <= SHARD_BUDGET as usize + 32,
            "shard grew past one record beyond the budget: {} bytes",
            content.len()
        );
        if index == 0 {
            assert!(
                "nyaterm-diagnostics.2026-11-04.jsonl"
                    == path.file_name().unwrap().to_str().unwrap()
            );
        }
        actual.push_str(&content);
    }
    assert_eq!(actual, expected, "each JSONL line must stay in one file");
    let mut names: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(ToOwned::to_owned))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "nyaterm-diagnostics.2026-11-04.jsonl".to_string(),
            "nyaterm-diagnostics.2026-11-04_001.jsonl".to_string(),
        ]
    );
}

#[test]
fn writer_continues_with_a_new_shard_after_restarting_on_a_full_file() {
    let dir = temp_dir("restart");
    let day = Date::from_calendar_date(2026, Month::November, 4).expect("date");
    std::fs::create_dir_all(dir.path()).expect("dir");
    std::fs::write(
        dir.path().join(log_file_name(day, 0)),
        "01234567890123456789012345678901234567890123456789012345678901234567890123456789\n",
    )
    .expect("seed full shard");

    let clock = Arc::new(Mutex::new(local_day(4)));
    let mut writer = writer(&dir, clock);
    writer
        .write_all(b"{\"event\":\"restart\"}\n")
        .expect("write");
    writer.flush().expect("flush");

    let files = written_files(dir.path());
    assert_eq!(files.len(), 2);
    assert_eq!(
        files[1].file_name().and_then(|value| value.to_str()),
        Some("nyaterm-diagnostics.2026-11-04_001.jsonl"),
        "a full day file must be followed by shard 001"
    );
    let shard = std::fs::read_to_string(&files[1]).expect("read shard");
    assert!(shard.contains("restart"));
}

#[test]
fn writer_rotates_on_local_midnight_and_prunes_expired_days() {
    let dir = temp_dir("local-midnight");
    let clock = Arc::new(Mutex::new(local_day(4)));
    let mut writer = writer(&dir, Arc::clone(&clock));
    writer
        .write_all(b"{\"event\":\"day-four\"}\n")
        .expect("write");
    writer.flush().expect("flush");

    std::fs::write(
        dir.path().join("nyaterm-diagnostics.2020-01-01.jsonl"),
        b"{\"event\":\"expired\"}\n",
    )
    .expect("write expired log");
    std::fs::write(
        dir.path().join("nyaterm-diagnostics-cloud-sync.jsonl"),
        b"{\"domain\":\"cloud_sync.history\"}\n",
    )
    .expect("write cloud sync history");

    *clock.lock().expect("clock lock") = local_day(5);
    writer
        .write_all(b"{\"event\":\"day-five\"}\n")
        .expect("write");
    writer.flush().expect("flush");
    drop(writer);

    let names: Vec<String> = written_files(dir.path())
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(ToOwned::to_owned))
        .collect();
    assert!(
        names.contains(&"nyaterm-diagnostics.2026-11-05.jsonl".to_string()),
        "the new local day must start its own file: {names:?}"
    );
    assert!(
        !dir.path()
            .join("nyaterm-diagnostics.2020-01-01.jsonl")
            .exists(),
        "files outside the retention window are pruned"
    );
    assert!(
        dir.path()
            .join("nyaterm-diagnostics-cloud-sync.jsonl")
            .exists(),
        "cloud-sync history is not a diagnostics shard and must survive pruning"
    );
}

#[test]
fn prune_keeps_files_inside_the_retention_window() {
    let dir = temp_dir("prune-window");
    std::fs::create_dir_all(dir.path()).expect("dir");
    for name in [
        "nyaterm-diagnostics.2026-10-01.jsonl",
        "nyaterm-diagnostics.2026-10-01_002.jsonl",
        "nyaterm-diagnostics.2026-10-30.jsonl",
        "nyaterm-diagnostics.2026-10-30_001.jsonl",
        "nyaterm-diagnostics.notes.jsonl",
    ] {
        std::fs::write(dir.path().join(name), b"{}\n").expect("write");
    }
    let today = Date::from_calendar_date(2026, Month::November, 4).expect("date");
    let removed = prune_log_files_at(dir.path(), 7, today).expect("prune");
    assert_eq!(removed, 2, "both 2026-10-01 shards expire");
    assert!(
        dir.path()
            .join("nyaterm-diagnostics.2026-10-30.jsonl")
            .exists()
    );
    assert!(
        dir.path()
            .join("nyaterm-diagnostics.2026-10-30_001.jsonl")
            .exists()
    );
    assert!(dir.path().join("nyaterm-diagnostics.notes.jsonl").exists());
}

#[test]
fn live_shard_budget_reconfiguration_applies_without_restart() {
    let dir = temp_dir("live-budget");
    let line = format!("{}\n", "x".repeat(600 * 1024));
    let mut writer = writer_with_budget(
        dir.path(),
        Arc::new(Mutex::new(local_day(4))),
        DEFAULT_LOG_SHARD_BYTES,
    );
    writer.write_all(line.as_bytes()).expect("first line");
    writer.flush().expect("flush");
    writer.test_limits.as_mut().unwrap().shard_bytes = MIN_LOG_SHARD_BYTES;
    writer.write_all(line.as_bytes()).expect("second line");
    writer.write_all(line.as_bytes()).expect("third line");
    writer.flush().expect("flush");
    drop(writer);

    assert_eq!(
        written_files(dir.path()).len(),
        2,
        "the open writer must honour the new 1 MiB budget on the next line"
    );
}

#[test]
fn shard_size_normalization_clamps_extremes_and_keeps_the_default() {
    assert_eq!(normalize_log_shard_bytes(0), DEFAULT_LOG_SHARD_BYTES);
    assert_eq!(normalize_log_shard_bytes(1024), MIN_LOG_SHARD_BYTES);
    assert_eq!(
        normalize_log_shard_bytes(MAX_LOG_SHARD_BYTES + 1024),
        MAX_LOG_SHARD_BYTES
    );
    assert_eq!(
        normalize_log_shard_bytes(DEFAULT_LOG_SHARD_BYTES),
        DEFAULT_LOG_SHARD_BYTES
    );
    assert_eq!(
        normalize_log_shard_bytes(DEFAULT_LOG_SHARD_BYTES + 100),
        DEFAULT_LOG_SHARD_BYTES + 1024 * 1024,
        "values round up to whole MiB"
    );
}

#[test]
fn delayed_settings_prune_even_with_an_open_startup_shard() {
    let dir = temp_dir("delayed-settings");
    std::fs::create_dir_all(dir.path()).unwrap();
    let expired = dir.path().join("nyaterm-diagnostics.2020-01-01.jsonl");
    std::fs::write(&expired, b"{}\n").unwrap();
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut sink = writer(&dir, Arc::new(Mutex::new(local_day(4))));
    let published = Arc::clone(&ready);
    sink.test_limits.as_mut().unwrap().ready =
        Box::new(move || published.load(std::sync::atomic::Ordering::Acquire));
    sink.write_all(b"startup\n").unwrap();
    assert!(expired.exists());
    assert!(sink.current.is_some());
    ready.store(true, std::sync::atomic::Ordering::Release);
    sink.write_all(b"configured\n").unwrap();
    assert!(!expired.exists());
    assert!(!sink.prune_due);
}

#[test]
fn failed_cleanup_retries_after_sixty_seconds_without_blocking_writes() {
    let dir = temp_dir("prune-retry");
    std::fs::create_dir_all(dir.path()).unwrap();
    // An expired-name directory deterministically fails remove_file on all OSes.
    let expired = dir.path().join("nyaterm-diagnostics.2020-01-01.jsonl");
    std::fs::create_dir(&expired).unwrap();
    let clock = Arc::new(Mutex::new(local_day(4)));
    let mut sink = writer(&dir, Arc::clone(&clock));
    sink.write_all(b"first\n").unwrap();
    assert!(sink.prune_due);
    let retry = sink.prune_retry_at;
    std::fs::remove_dir(&expired).unwrap();
    std::fs::write(&expired, b"{}\n").unwrap();
    *clock.lock().unwrap() += time::Duration::seconds(59);
    sink.write_all(b"second\n").unwrap();
    assert!(expired.exists());
    assert_eq!(sink.prune_retry_at, retry);
    *clock.lock().unwrap() += time::Duration::seconds(1);
    sink.write_all(b"third\n").unwrap();
    assert!(!expired.exists());
    assert!(!sink.prune_due);
}

#[test]
fn batched_and_split_writes_produce_identical_shards_and_keep_partial_tail() {
    let batch_dir = temp_dir("batch");
    let split_dir = temp_dir("split");
    let data = (0..100)
        .map(|index| format!("{{\"event\":{index}}}\n"))
        .collect::<String>();
    let mut batch = writer(&batch_dir, Arc::new(Mutex::new(local_day(4))));
    batch.write_all(data.as_bytes()).unwrap();
    batch.write_all(b"unfinished").unwrap();
    assert_eq!(batch.pending, b"unfinished");
    let mut split = writer(&split_dir, Arc::new(Mutex::new(local_day(4))));
    for part in data.as_bytes().chunks(7) {
        split.write_all(part).unwrap();
    }
    let contents = |dir: &Path| {
        written_files(dir)
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>()
    };
    let shards = contents(&batch_dir);
    assert!(shards.len() > 10);
    assert!(
        shards
            .iter()
            .all(|bytes| bytes.len() <= SHARD_BUDGET as usize + 20)
    );
    assert_eq!(shards, contents(&split_dir));
    assert_eq!(shards.concat(), data.as_bytes());
}

#[test]
fn restart_appends_to_the_unfilled_shard_and_oversized_records_stay_whole() {
    let dir = temp_dir("append-oversized");
    let clock = Arc::new(Mutex::new(local_day(4)));
    {
        let mut sink = writer(&dir, Arc::clone(&clock));
        sink.write_all(b"first\n").unwrap();
        sink.flush().unwrap();
    }
    let mut sink = writer(&dir, clock);
    let oversized = format!("{}\n", "x".repeat(200));
    sink.write_all(oversized.as_bytes()).unwrap();
    sink.write_all(b"next\n").unwrap();
    sink.flush().unwrap();
    let files = written_files(&dir);
    assert_eq!(files.len(), 2);
    assert_eq!(
        std::fs::read_to_string(&files[0]).unwrap(),
        format!("first\n{oversized}")
    );
    assert_eq!(std::fs::read(&files[1]).unwrap(), b"next\n");
}

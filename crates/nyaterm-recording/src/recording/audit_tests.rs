use super::{
    ExistingFileBehavior, RecordingContext, RecordingManager, RecordingMode, RecordingProfile,
    RecordingRotationPolicy, TerminalHistorySearchRequest, append_numbered_suffix,
    input_recording_path, lock_recover,
};
use nyaterm_core::test_support::TestTempDir;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use time::OffsetDateTime;

fn profile(root: &Path, mode: RecordingMode) -> (RecordingContext, RecordingProfile) {
    (
        RecordingContext {
            session_id: "audit".into(),
            session_name: "audit".into(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "terminal".into(),
            host: None,
            port: None,
            username: None,
            started_at: OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc()),
        },
        RecordingProfile {
            mode,
            base_path: root.into(),
            path_template: "audit.log".into(),
            include_timestamps: false,
            include_io_labels: false,
            include_session_metadata: false,
            rotation: RecordingRotationPolicy::Session,
            existing_file_behavior: ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: false,
        },
    )
}

#[test]
fn default_capture_never_stores_no_echo_user_input() {
    let dir = TestTempDir::new("nyaterm-recording-privacy");
    let manager = RecordingManager::new();
    let path = dir.path().join("audit.log");
    manager.write_input("audit", b"synthetic-before-recording\r");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_input("audit", b"synthetic-no-echo-input\r");
    manager.write_raw_input("audit", b"synthetic-raw-input");
    manager.stop("audit").unwrap();
    assert!(fs::read(path).unwrap().is_empty());
    assert_eq!(
        lock_recover(&manager.sessions)["audit"].input_buffer.len(),
        0
    );
}

#[test]
fn execution_boundaries_preserve_identical_output_and_separate_unterminated_lines() {
    let dir = TestTempDir::new("nyaterm-recording-execution");
    for include_input in [false, true] {
        let manager = RecordingManager::new();
        let (context, mut settings) = profile(dir.path(), RecordingMode::Transcript);
        settings.include_input = include_input;
        settings.include_io_labels = true;
        let path = dir.join(format!("{include_input}.log"));
        manager
            .start_with_profile("audit", context, settings, Some(path.clone()))
            .unwrap();
        manager.write_output("audit", "before>");
        manager.execution_boundary("audit", Some("same\nsecond"));
        manager.write_execution_output("audit", "same\nsecond");
        manager.execution_boundary("audit", None);
        manager.write_output("audit", "after>");
        manager.stop("audit").unwrap();
        let text = fs::read_to_string(path).unwrap();
        let expected = if include_input {
            "[OUTPUT] before>\n[INPUT] same\n[INPUT] second\n[OUTPUT] same\n[OUTPUT] second\n[OUTPUT] after>\n"
        } else {
            "[OUTPUT] before>\n[OUTPUT] same\n[OUTPUT] second\n[OUTPUT] after>\n"
        };
        assert_eq!(text, expected);
    }
}

#[test]
fn background_execution_preserves_manual_input_echo_and_partial_control_strings() {
    let dir = TestTempDir::new("nyaterm-recording-interleaved-execution");
    let manager = RecordingManager::new();
    let (context, mut settings) = profile(dir.path(), RecordingMode::Transcript);
    settings.include_input = true;
    settings.include_io_labels = true;
    let path = dir.join("interleaved.log");
    manager
        .start_with_profile("audit", context, settings, Some(path.clone()))
        .unwrap();
    manager.write_input("audit", b"man");
    manager.execution_boundary("audit", Some("background"));
    manager.write_execution_output("audit", "man\n");
    manager.write_output("audit", "man\x1b]hidden-title");
    manager.execution_boundary("audit", None);
    manager.write_output("audit", "\x07");
    manager.write_input("audit", b"ual\r");
    manager.write_output("audit", "manual\r\nmanual-result\n");
    manager.stop("audit").unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "[INPUT] background\n[OUTPUT] man\n[INPUT] manual\n[OUTPUT] manual-result\n"
    );
}

#[test]
fn split_escape_strings_are_removed_from_transcript() {
    let dir = TestTempDir::new("nyaterm-recording-split-escape");
    let manager = RecordingManager::new();
    let path = dir.path().join("audit.log");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    for part in [
        "\x1b[",
        "31mRED\x1b[0",
        "m\x1b]title",
        "-hidden\x1b",
        "\\\n",
    ] {
        manager.write_output("audit", part);
    }
    manager.stop("audit").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "RED\n");
}

#[test]
fn unterminated_lines_and_input_stay_within_aggregate_budget() {
    let dir = TestTempDir::new("nyaterm-recording-budget");
    let manager = RecordingManager::new();
    manager.set_memory_limit(1024);
    manager.set_history_include_input(true);
    let path = dir.path().join("audit.log");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    for _ in 0..64 {
        manager.write_output("audit", &"红".repeat(4096));
        manager.write_input("audit", &vec![b'x'; 4096]);
        let sessions = lock_recover(&manager.sessions);
        let state = &sessions["audit"];
        assert!(state.record_bytes + state.buffered_bytes() <= 1024);
    }
    manager.stop("audit").unwrap();
    assert!(fs::metadata(path).unwrap().len() > 64 * 4096 * 3);
    let search = manager
        .search_history(TerminalHistorySearchRequest {
            session_id: "audit".into(),
            query: "红".into(),
            case_sensitive: true,
            regex: false,
            whole_word: false,
            limit: None,
            context_before: None,
            context_after: None,
            max_lines: None,
        })
        .unwrap();
    assert!(search.truncated);
}

#[test]
fn repeated_start_and_export_cannot_damage_active_file_or_aliases() {
    let dir = TestTempDir::new("nyaterm-recording-active-file");
    let manager = RecordingManager::new();
    let path = dir.path().join("audit.log");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_output("audit", "before\n");
    manager.flush_recordings();
    let original = fs::read(&path).unwrap();
    assert!(
        manager
            .start("audit", path.to_str().unwrap(), false, false)
            .is_err()
    );
    assert!(
        manager
            .save_transcript("audit", path.to_str().unwrap(), false, false)
            .is_err()
    );
    let alias = dir.path().join("alias.log");
    fs::hard_link(&path, &alias).unwrap();
    assert!(
        manager
            .save_transcript("audit", alias.to_str().unwrap(), false, false)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    manager.stop("audit").unwrap();
}

#[test]
fn raw_output_is_byte_exact_and_user_input_has_a_separate_file() {
    let dir = TestTempDir::new("nyaterm-recording-raw-byte-exact");
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Raw);
    profile.include_input = true;
    let path = manager
        .start_with_profile("audit", context, profile, None)
        .unwrap();
    let bytes = b"\xb2\xe2\xff\x00\x1b[31m";
    manager.write_raw_output("audit", bytes);
    manager.write_output("audit", "local diagnostic must not enter Raw");
    manager.write_raw_input("audit", b"\x1b[200~\xb2\xe2\x1b[201~");
    let input = manager.status("audit").unwrap().input_file_path.unwrap();
    assert!(
        manager
            .save_transcript("audit", input.to_str().unwrap(), false, false)
            .is_err()
    );
    manager.stop("audit").unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(fs::read(input).unwrap(), b"\x1b[200~\xb2\xe2\x1b[201~");
}

#[test]
fn shutdown_finishes_partial_output_and_search_does_not_commit_it() {
    let dir = TestTempDir::new("nyaterm-recording-final-tail");
    let manager = RecordingManager::new();
    let path = dir.path().join("audit.log");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_output("audit", "partial");
    manager
        .save_transcript(
            "audit",
            dir.path().join("export.log").to_str().unwrap(),
            false,
            false,
        )
        .unwrap();
    // The partial line stays in the pending output editor (still editable, so
    // a later backspace could retract it) instead of `output_buffer`; the
    // transcript export must not commit it either way.
    assert_eq!(lock_recover(&manager.sessions)["audit"].output_buffer, "");
    assert_eq!(
        lock_recover(&manager.sessions)["audit"]
            .output_edit
            .pending(),
        "partial"
    );
    manager.write_output("audit", " completion");
    assert!(manager.finish_all().is_empty());
    assert_eq!(fs::read_to_string(path).unwrap(), "partial completion\n");
}

#[test]
fn atomic_export_failure_preserves_destination_and_unique_export_preserves_existing_file() {
    let dir = TestTempDir::new("nyaterm-recording-atomic-export");
    let manager = RecordingManager::new();
    manager.write_output("audit", "export\n");
    let path = dir.path().join("export.log");
    fs::create_dir_all(dir.path()).unwrap();
    fs::write(&path, b"existing").unwrap();
    let unique = manager
        .save_transcript_unique("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    assert_ne!(Path::new(&unique), path);
    assert_eq!(fs::read(&path).unwrap(), b"existing");
    let folder = dir.path().join("folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("keep"), b"existing").unwrap();
    assert!(
        manager
            .save_transcript("audit", folder.to_str().unwrap(), false, false)
            .is_err()
    );
    assert_eq!(fs::read(folder.join("keep")).unwrap(), b"existing");
}

#[test]
fn explicit_paths_rotate_with_input_and_append_counts_existing_bytes() {
    let dir = TestTempDir::new("nyaterm-recording-explicit-rotation");
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Raw);
    profile.include_input = true;
    profile.rotation = RecordingRotationPolicy::Size { max_bytes: 8 };
    profile.existing_file_behavior = ExistingFileBehavior::Append;
    let path = dir.path().join("chosen.bin");
    fs::create_dir_all(dir.path()).unwrap();
    fs::write(&path, b"existing").unwrap();
    manager
        .start_with_profile("audit", context, profile, Some(path.clone()))
        .unwrap();
    manager.write_raw_output("audit", b"next");
    manager.write_raw_input("audit", b"typed");
    let status = manager.status("audit").unwrap();
    assert_eq!(
        status.file_path,
        Some(append_numbered_suffix(path.clone(), 2))
    );
    assert_eq!(
        status.input_file_path,
        Some(input_recording_path(status.file_path.as_ref().unwrap()))
    );
    manager.stop("audit").unwrap();
    assert_eq!(fs::read(path).unwrap(), b"existing");
}

#[test]
fn daily_rotation_keeps_original_start_time_and_never_truncates_earlier_segment() {
    let dir = TestTempDir::new("nyaterm-recording-daily-rotation");
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Raw);
    let started_at = context.started_at;
    profile.rotation = RecordingRotationPolicy::Daily;
    let initial = manager
        .start_with_profile("audit", context, profile, None)
        .unwrap();
    manager.write_raw_output("audit", b"before");
    lock_recover(&manager.sessions)
        .get_mut("audit")
        .unwrap()
        .recording
        .as_mut()
        .unwrap()
        .daily_key = "previous-day".into();
    manager.write_raw_output("audit", b"after");
    assert_eq!(
        manager.status("audit").unwrap().started_at,
        Some(started_at)
    );
    assert_ne!(
        manager.status("audit").unwrap().file_path,
        Some(initial.clone().into())
    );
    manager.stop("audit").unwrap();
    assert_eq!(fs::read(initial).unwrap(), b"before");
}

#[test]
#[ignore = "manual throughput benchmark; run with --ignored --nocapture"]
fn recording_stream_processing_scales_without_unbounded_retained_text() {
    for multiline in [false, true] {
        for size in [64 * 1024, 256 * 1024, 1024 * 1024] {
            let manager = RecordingManager::new();
            manager.set_memory_limit(4096);
            let chunk = if multiline {
                "x\n".repeat(2048)
            } else {
                "x".repeat(4096)
            };
            let start = Instant::now();
            for _ in 0..size / chunk.len() {
                manager.write_output("audit", &chunk);
            }
            {
                let sessions = lock_recover(&manager.sessions);
                let state = &sessions["audit"];
                assert!(state.record_bytes + state.buffered_bytes() <= 4096);
            }
            eprintln!(
                "multiline={multiline} bytes={size} elapsed={:?}",
                start.elapsed()
            );
        }
    }
}

#[test]
fn companion_open_failure_does_not_truncate_existing_output() {
    let dir = TestTempDir::new("nyaterm-recording-companion-failure");
    fs::create_dir_all(dir.path()).unwrap();
    let path = dir.join("audit.log");
    fs::write(&path, b"existing").unwrap();
    fs::create_dir(input_recording_path(&path)).unwrap();
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Raw);
    profile.include_input = true;
    assert!(
        manager
            .start_with_profile("audit", context, profile, Some(path.clone()))
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"existing");
}

#[test]
fn rotation_refuses_alias_of_another_active_session_without_truncating_it() {
    let dir = TestTempDir::new("nyaterm-recording-rotation-alias");
    let manager = RecordingManager::new();
    let (context, mut config) = profile(dir.path(), RecordingMode::Raw);
    config.rotation = RecordingRotationPolicy::Size { max_bytes: 4 };
    let path = dir.join("chosen.log");
    manager
        .start_with_profile("audit", context, config, Some(path.clone()))
        .unwrap();
    let other = dir.join("other.log");
    manager
        .start("other", other.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_output("other", "retained\n");
    manager.flush_recordings();
    fs::hard_link(&other, append_numbered_suffix(path.clone(), 1)).unwrap();
    manager.write_raw_output("audit", b"first");
    manager.write_raw_output("audit", b"next");
    let completion = manager.stop_complete("audit").unwrap();
    assert_eq!(completion.file_path, path);
    assert!(completion.error.is_some());
    manager.stop("other").unwrap();
    assert_eq!(fs::read_to_string(other).unwrap(), "retained\n");
}

#[test]
fn failed_final_flush_retains_output_and_input_paths_in_completion() {
    let dir = TestTempDir::new("nyaterm-recording-final-flush-failure");
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Raw);
    profile.include_input = true;
    let path = manager
        .start_with_profile("audit", context, profile, None)
        .unwrap();
    let input = input_recording_path(Path::new(&path));
    {
        let mut sessions = lock_recover(&manager.sessions);
        let file = sessions
            .get_mut("audit")
            .unwrap()
            .recording
            .as_mut()
            .unwrap();
        file.writer = std::io::BufWriter::new(fs::File::open(&path).unwrap());
        file.writer.write_all(b"synthetic-write-failure").unwrap();
    }
    let completion = manager.stop_complete("audit").unwrap();
    assert_eq!(completion.file_path, Path::new(&path));
    assert_eq!(completion.input_file_path, Some(input));
    assert!(completion.error.is_some());
    assert!(!manager.is_recording("audit"));
}

#[test]
fn queue_discontinuity_separates_partial_lines_and_resets_escape_state() {
    let dir = TestTempDir::new("nyaterm-recording-gap");
    let manager = RecordingManager::new();
    let path = dir.join("gap.log");
    manager
        .start("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_output("audit", "before\x1b]partial");
    manager.report_dropped("audit", 8);
    manager.write_output("audit", "after\n");
    assert!(manager.stop_complete("audit").unwrap().error.is_some());
    assert_eq!(fs::read_to_string(path).unwrap(), "before\nafter\n");
}

#[test]
fn recording_profile_input_snapshot_is_independent_of_later_history_setting_changes() {
    let dir = TestTempDir::new("nyaterm-recording-profile-snapshot");
    let manager = RecordingManager::new();
    let (context, mut profile) = profile(dir.path(), RecordingMode::Transcript);
    profile.include_input = true;
    profile.include_io_labels = true;
    let path = manager
        .start_with_profile(
            "enabled",
            context.clone(),
            profile.clone(),
            Some(dir.join("enabled.log")),
        )
        .unwrap();
    manager.set_history_include_input(false);
    manager.write_input("enabled", b"opted-in\r");
    manager.stop("enabled").unwrap();
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .contains("[INPUT] opted-in")
    );
    profile.include_input = false;
    let path = manager
        .start_with_profile("disabled", context, profile, Some(dir.join("disabled.log")))
        .unwrap();
    manager.set_history_include_input(true);
    manager.write_input("disabled", b"must-be-excluded\r");
    manager.stop("disabled").unwrap();
    assert!(fs::read(path).unwrap().is_empty());
}

#[test]
fn transcript_preserves_empty_output_lines_and_finishes_each_rotation_segment() {
    let dir = TestTempDir::new("nyaterm-recording-empty-lines-footers");
    let manager = RecordingManager::new();
    let path = dir.join("lines.log");
    manager
        .start("lines", path.to_str().unwrap(), false, false)
        .unwrap();
    manager.write_output("lines", "first\n\nlast\n");
    manager.stop("lines").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "first\n\nlast\n");
    let (context, mut profile) = profile(dir.path(), RecordingMode::Transcript);
    profile.rotation = RecordingRotationPolicy::Size { max_bytes: 1024 };
    profile.include_session_metadata = true;
    let path = manager
        .start_with_profile("audit", context, profile, None)
        .unwrap();
    manager.write_output("audit", &format!("{}\n", "a".repeat(900)));
    manager.write_output("audit", "tail\n");
    let completion = manager.stop_complete("audit").unwrap();
    assert!(completion.error.is_none());
    assert_ne!(completion.file_path, Path::new(&path));
    assert!(fs::read_to_string(path).unwrap().contains("Rotated"));
    assert!(
        fs::read_to_string(completion.file_path)
            .unwrap()
            .contains("Stopped")
    );
}

#[test]
fn input_submission_separates_prompt_and_preserves_output_without_echo_evidence() {
    let dir = TestTempDir::new("nyaterm-recording-no-echo");
    for output in [
        "fixture\n",
        "echo fixture\n",
        "result: echo fixture\n",
        "other\necho fixture\n",
        "\necho fixture\n",
    ] {
        let manager = RecordingManager::new();
        manager.set_history_include_input(true);
        manager.write_output("audit", "before>");
        manager.write_input("audit", b"echo fixture\r");
        for chunk in output.as_bytes().chunks(2) {
            manager.write_output("audit", std::str::from_utf8(chunk).unwrap());
        }
        let path = dir.join("no-echo.log");
        manager
            .save_transcript("audit", path.to_str().unwrap(), true, false)
            .unwrap();
        let expected = format!(
            "[OUTPUT] before>\n[INPUT] echo fixture\n{}",
            output
                .lines()
                .map(|line| format!("[OUTPUT] {line}\n"))
                .collect::<String>()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), expected, "{output:?}");
    }
}

#[test]
fn observed_edited_echo_stays_editable_across_submission_and_split_crlf() {
    let dir = TestTempDir::new("nyaterm-recording-observed-echo");
    let manager = RecordingManager::new();
    manager.set_history_include_input(true);
    manager.write_output("audit", "PS> ");
    manager.write_input("audit", b"pingng");
    manager.write_output("audit", "pingng");
    manager.write_input("audit", b"\x08\x08 qq.com\r");
    for chunk in [
        "\x08 ",
        "\x08\x08",
        " \x08 qq",
        ".com\r",
        "\n",
        "result: ping qq.com\n",
        "ping qq.com\n",
    ] {
        manager.write_output("audit", chunk);
    }
    let path = dir.join("edited.log");
    manager
        .save_transcript("audit", path.to_str().unwrap(), true, false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "[INPUT] ping qq.com\n[OUTPUT] PS> \n[OUTPUT] result: ping qq.com\n[OUTPUT] ping qq.com\n"
    );
}

#[test]
fn submitted_echo_expectation_expires_on_the_first_unmatched_row() {
    let dir = TestTempDir::new("nyaterm-recording-echo-expiry");
    let manager = RecordingManager::new();
    manager.set_history_include_input(true);
    manager.write_input("audit", b"same");
    manager.write_output("audit", "same");
    manager.write_input("audit", b"\r");
    manager.write_output("audit", " output\nsame\nresult: same\n");
    let path = dir.join("expired.log");
    manager
        .save_transcript("audit", path.to_str().unwrap(), true, false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "[INPUT] same\n[OUTPUT] same output\n[OUTPUT] same\n[OUTPUT] result: same\n"
    );
}

#[test]
fn extreme_cursor_gaps_and_unicode_fragments_respect_memory_budget() {
    let dir = TestTempDir::new("nyaterm-recording-cursor-budget");
    for budget in [1, 16, 1024] {
        let manager = RecordingManager::new();
        manager.set_memory_limit(budget);
        manager.set_history_include_input(true);
        for _ in 0..8 {
            manager.write_output("audit", "界界\x1b[999999999CX\r\x1b[K");
            manager.write_execution_output("audit", "界界\x1b[999999999CX\n");
            let sessions = lock_recover(&manager.sessions);
            let state = &sessions["audit"];
            assert!(state.record_bytes + state.buffered_bytes() <= budget);
        }
        let path = dir.join(format!("{budget}.log"));
        manager
            .save_transcript("audit", path.to_str().unwrap(), false, false)
            .unwrap();
    }
}

#[test]
fn snapshots_keep_background_and_interactive_partial_rows_separate() {
    let dir = TestTempDir::new("nyaterm-recording-separate-rows");
    let manager = RecordingManager::new();
    manager.write_output("audit", "manual>");
    manager.write_execution_output("audit", "background");
    let path = dir.join("snapshot.log");
    manager
        .save_transcript("audit", path.to_str().unwrap(), true, false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "[OUTPUT] manual>\n[OUTPUT] background\n"
    );
}

#[test]
fn ordering_boundary_resets_cursor_even_on_an_empty_row() {
    let dir = TestTempDir::new("nyaterm-recording-empty-cursor");
    let manager = RecordingManager::new();
    manager.write_output("audit", "\x1b[999C");
    manager.execution_boundary("audit", None);
    manager.write_output("audit", "next\n");
    let path = dir.join("cursor.log");
    manager
        .save_transcript("audit", path.to_str().unwrap(), false, false)
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "next\n");
}

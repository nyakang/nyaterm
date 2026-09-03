use super::*;

#[test]
fn remote_same_endpoint_file_copy_command_quotes_paths() {
    let command = remote_same_endpoint_copy_command(
        "/tmp/source dir/it's $(safe).txt",
        "/tmp/target dir/out's file.txt",
        false,
    );

    assert!(command.contains("cp -a --"));
    assert!(command.contains("'/tmp/source dir/it'\\''s $(safe).txt'"));
    assert!(command.contains("'/tmp/target dir/out'\\''s file.txt'"));
}

#[test]
fn remote_same_endpoint_directory_copy_command_merges_existing_target() {
    let command = remote_same_endpoint_copy_command("/tmp/source dir", "/tmp/target dir", true);

    assert!(command.contains("if [ -d '/tmp/target dir' ]; then"));
    assert!(command.contains("cp -a -- '/tmp/source dir'/. '/tmp/target dir'/"));
    assert!(command.contains("else cp -a -- '/tmp/source dir' '/tmp/target dir'"));
}

#[test]
fn download_progress_allows_short_non_empty_reads_until_complete() {
    let remote_size = 256 * 1024;
    let request_kib = 256;
    let payload_bytes = 256 * 1024;
    let read_bytes = 64 * 1024;
    let mut offset = 0;
    let mut bytes_written = 0;

    for _ in 0..3 {
        let progress = classify_download_read_progress(
            "/remote/file.bin",
            "C:/local/file.bin",
            remote_size,
            offset,
            bytes_written,
            read_bytes as usize,
            request_kib,
            payload_bytes,
        )
        .expect("short non-empty read should continue");

        offset = match progress {
            DownloadReadProgress::Continue(next_offset) => next_offset,
            DownloadReadProgress::Complete => panic!("download completed too early"),
        };
        bytes_written += read_bytes;
    }

    let progress = classify_download_read_progress(
        "/remote/file.bin",
        "C:/local/file.bin",
        remote_size,
        offset,
        bytes_written,
        read_bytes as usize,
        request_kib,
        payload_bytes,
    )
    .expect("final short read should complete");
    assert_eq!(progress, DownloadReadProgress::Complete);
    bytes_written += read_bytes;
    assert!(
        ensure_download_complete(
            "/remote/file.bin",
            "C:/local/file.bin",
            remote_size,
            bytes_written,
            request_kib,
            payload_bytes,
        )
        .is_ok()
    );
}

#[test]
fn download_progress_rejects_empty_read_before_remote_size() {
    let error = classify_download_read_progress(
        "/remote/file.bin",
        "C:/local/file.bin",
        256 * 1024,
        64 * 1024,
        64 * 1024,
        0,
        256,
        256 * 1024,
    )
    .expect_err("empty read before remote size should fail");

    assert!(error.to_string().contains("Unexpected EOF"));
    assert!(
        error
            .to_string()
            .contains("expected 262144 bytes, got 65536 bytes")
    );
}

#[test]
fn download_completion_rejects_short_written_count() {
    let error = ensure_download_complete(
        "/remote/file.bin",
        "C:/local/file.bin",
        256 * 1024,
        192 * 1024,
        256,
        256 * 1024,
    )
    .expect_err("short written count should fail");

    assert!(error.to_string().contains("Unexpected EOF"));
    assert!(
        error
            .to_string()
            .contains("expected 262144 bytes, got 196608 bytes")
    );
}

#[test]
fn download_completion_accepts_empty_remote_file() {
    assert!(
        ensure_download_complete(
            "/remote/empty.txt",
            "C:/local/empty.txt",
            0,
            0,
            256,
            256 * 1024,
        )
        .is_ok()
    );
}

#[test]
fn directory_concurrency_uses_fast_default_without_server_limits() {
    let concurrency = sftp_directory_concurrency(None);

    assert_eq!(concurrency.session_pool_size, 2);
    assert_eq!(concurrency.small_file_concurrency, 16);
    assert_eq!(concurrency.large_file_concurrency, 2);
}

#[test]
fn directory_concurrency_respects_low_server_handle_limits() {
    let concurrency = sftp_directory_concurrency(Some(12));

    assert_eq!(concurrency.session_pool_size, 2);
    assert_eq!(concurrency.small_file_concurrency, 4);
    assert_eq!(concurrency.large_file_concurrency, 2);
}

#[test]
fn sftp_channel_open_retry_classifies_temporary_capacity_failures() {
    assert!(is_retryable_sftp_channel_open_error(
        &russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)
    ));
    assert!(is_retryable_sftp_channel_open_error(
        &russh::Error::ChannelOpenFailure(ChannelOpenFailure::ResourceShortage)
    ));
}

#[test]
fn sftp_channel_open_retry_rejects_policy_and_type_failures() {
    assert!(!is_retryable_sftp_channel_open_error(
        &russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited)
    ));
    assert!(!is_retryable_sftp_channel_open_error(
        &russh::Error::ChannelOpenFailure(ChannelOpenFailure::UnknownChannelType)
    ));
}

#[test]
fn write_text_permission_restore_preserves_posix_mode_bits() {
    assert_eq!(
        permissions_to_preserve_after_write(Some(0o100644)),
        Some(0o644)
    );
    assert_eq!(
        permissions_to_preserve_after_write(Some(0o100755)),
        Some(0o755)
    );
    assert_eq!(
        permissions_to_preserve_after_write(Some(0o104755)),
        Some(0o4755)
    );
}

#[test]
fn write_text_permission_restore_skips_missing_permission_attrs() {
    assert_eq!(permissions_to_preserve_after_write(None), None);
}

fn transfer_settings_with_request_kib(request_kib: u32) -> crate::config::TransferSettings {
    crate::config::TransferSettings {
        transfer_buffer_size: request_kib,
        ..crate::config::TransferSettings::default()
    }
}

#[test]
fn automatic_pipeline_config_keeps_existing_depths() {
    let cases = [(64, 16, 16), (128, 8, 16), (256, 4, 8)];

    for (request_kib, expected_download_depth, expected_concurrent_writes) in cases {
        let settings = transfer_settings_with_request_kib(request_kib);
        assert_eq!(
            sftp_pipeline_config(&settings, None),
            (
                request_kib as usize,
                expected_download_depth,
                expected_concurrent_writes,
            )
        );
    }
}

#[test]
fn manual_pipeline_depth_overrides_downloads_and_uploads() {
    for request_kib in [64, 128, 256] {
        let settings = transfer_settings_with_request_kib(request_kib);
        assert_eq!(
            sftp_pipeline_config(&settings, Some(32)),
            (request_kib as usize, 32, 32)
        );
    }
}

#[test]
fn manual_pipeline_depth_is_defensively_clamped() {
    let settings = transfer_settings_with_request_kib(128);
    assert_eq!(sftp_pipeline_config(&settings, Some(1)), (128, 4, 4));
    assert_eq!(sftp_pipeline_config(&settings, Some(128)), (128, 64, 64));
}

#[test]
fn directory_concurrency_keeps_at_least_one_worker() {
    let concurrency = sftp_directory_concurrency(Some(2));

    assert_eq!(concurrency.session_pool_size, 1);
    assert_eq!(concurrency.small_file_concurrency, 1);
    assert_eq!(concurrency.large_file_concurrency, 1);
}

#[test]
fn directory_pipeline_keeps_existing_depth_without_server_limit() {
    assert_eq!(sftp_directory_download_pipeline_cap(None, 2, 16, 16), 16);
}

#[test]
fn directory_pipeline_respects_server_handle_budget() {
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(128), 2, 16, 16),
        15
    );
}

#[test]
fn directory_pipeline_caps_manual_depth_to_server_handle_budget() {
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(128), 2, 16, 64),
        15
    );
}

#[test]
fn directory_pipeline_never_returns_zero() {
    assert_eq!(sftp_directory_download_pipeline_cap(Some(2), 2, 16, 16), 1);
}

#[test]
fn directory_pipeline_uses_actual_file_worker_count() {
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(128), 2, 3, 16),
        16
    );
}

#[test]
fn directory_pipeline_handles_single_session_pool() {
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(128), 1, 16, 16),
        7
    );
}

#[test]
fn directory_pipeline_rounds_up_non_even_workers_per_session() {
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(128), 3, 10, 16),
        16
    );
    assert_eq!(
        sftp_directory_download_pipeline_cap(Some(64), 3, 10, 16),
        14
    );
}

#[test]
fn directory_pipeline_keeps_requested_depth_below_budget() {
    assert_eq!(sftp_directory_download_pipeline_cap(Some(128), 2, 16, 4), 4);
}

#[test]
fn directory_progress_accumulates_chunk_deltas_without_completion_double_count() {
    let controller = create_directory_transfer_controller(
        Some("directory-progress-test".to_string()),
        "session-1",
        "folder".to_string(),
        "/remote/folder",
        "C:/local/folder",
        "download",
        2,
        1_000,
    );
    let completed_bytes = AtomicU64::new(0);

    assert_eq!(
        add_directory_transferred_bytes(&controller, &completed_bytes, 128, 1_000),
        128
    );
    assert_eq!(
        add_directory_transferred_bytes(&controller, &completed_bytes, 256, 1_000),
        384
    );

    controller.update_item_progress(1, 2);
    controller.update_progress(completed_bytes.load(Ordering::SeqCst), 1_000);

    let event = controller.build_event("progress", 0, None);
    assert_eq!(event.bytes_transferred, 384);
    assert_eq!(event.item_count_completed, Some(1));
    assert_eq!(event.item_count_total, Some(2));
}

#[test]
fn directory_worker_count_is_bounded_by_file_count() {
    let concurrency = sftp_directory_concurrency(None);

    assert_eq!(sftp_directory_file_concurrency(0, concurrency), 1);
    assert_eq!(sftp_directory_file_concurrency(3, concurrency), 3);
    assert_eq!(
        sftp_directory_file_concurrency(10_000, concurrency),
        concurrency.small_file_concurrency
    );
}

#[test]
fn directory_stall_watchdog_fires_only_while_running_without_progress() {
    let snapshot = DirectoryProgressSnapshot {
        bytes: 128,
        completed: 1,
    };

    assert!(directory_transfer_stalled(
        TransferControlState::Running,
        snapshot,
        snapshot,
        SFTP_DIRECTORY_STALL_TIMEOUT,
        2,
    ));
    assert!(!directory_transfer_stalled(
        TransferControlState::Paused,
        snapshot,
        snapshot,
        SFTP_DIRECTORY_STALL_TIMEOUT * 2,
        2,
    ));
    assert!(!directory_transfer_stalled(
        TransferControlState::Running,
        snapshot,
        DirectoryProgressSnapshot {
            bytes: 256,
            completed: 1,
        },
        SFTP_DIRECTORY_STALL_TIMEOUT,
        2,
    ));
    assert!(!directory_transfer_stalled(
        TransferControlState::Running,
        snapshot,
        snapshot,
        SFTP_DIRECTORY_STALL_TIMEOUT,
        1,
    ));
}

#[tokio::test]
async fn directory_worker_error_aborts_remaining_workers() {
    let mut join_set = tokio::task::JoinSet::new();
    join_set.spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        AppResult::Ok(())
    });
    let mut first_err = None;

    handle_directory_worker_result(
        "upload",
        Ok(Err(AppError::Channel("first failure".to_string()))),
        &mut first_err,
        &mut join_set,
    );

    assert!(
        first_err
            .as_ref()
            .is_some_and(|error| error.to_string().contains("first failure"))
    );
    let result = join_set
        .join_next()
        .await
        .expect("aborted worker should still be drained");
    assert!(result.expect_err("worker should be aborted").is_cancelled());
}

#[test]
fn recursive_remove_rejects_dangerous_targets() {
    assert!(!is_safe_recursive_remove_target(""));
    assert!(!is_safe_recursive_remove_target("/"));
    assert!(!is_safe_recursive_remove_target("."));
    assert!(!is_safe_recursive_remove_target(".."));
    assert!(!is_safe_recursive_remove_target("/tmp/../home"));
}

#[test]
fn recursive_remove_accepts_normal_remote_targets() {
    assert!(is_safe_recursive_remove_target("/tmp/uploads"));
    assert!(is_safe_recursive_remove_target("relative/uploads"));
    assert!(is_safe_recursive_remove_target("/home/user/data/"));
}

#[test]
fn raw_path_match_allows_fast_remove_without_raw_path() {
    let path_ref = RemotePathRef::new("/tmp/nyaterm", None).unwrap();

    assert!(raw_path_matches_display_path(&path_ref));
}

#[test]
fn raw_path_match_allows_fast_remove_when_bytes_match_display_path() {
    let token = raw_path_token(b"/tmp/nyaterm");
    let path_ref = RemotePathRef::new("/tmp/nyaterm", Some(&token)).unwrap();

    assert!(raw_path_matches_display_path(&path_ref));
}

#[test]
fn raw_path_match_rejects_fast_remove_when_bytes_do_not_match_display_path() {
    let token = raw_path_token(b"/tmp/raw-name");
    let path_ref = RemotePathRef::new("/tmp/display-name", Some(&token)).unwrap();

    assert!(!raw_path_matches_display_path(&path_ref));
}

#[test]
fn raw_path_match_does_not_override_unsafe_recursive_remove_targets() {
    for target in ["/", "..", "/tmp/../x"] {
        let token = raw_path_token(target.as_bytes());
        let path_ref = RemotePathRef::new(target, Some(&token)).unwrap();

        assert!(raw_path_matches_display_path(&path_ref));
        assert!(!is_safe_recursive_remove_target(path_ref.display_path()));
    }
}

#[test]
fn raw_child_path_is_joined_from_parent_bytes() {
    let parent = b"/remote/\x80parent".to_vec();
    let child = b"\x81child".to_vec();

    assert_eq!(
        join_remote_child_bytes(&parent, &child),
        b"/remote/\x80parent/\x81child"
    );
}

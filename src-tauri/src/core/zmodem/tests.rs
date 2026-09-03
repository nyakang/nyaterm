#[cfg(test)]
mod tests {
    use super::{
        ProgressThrottle, ZMODEM_FINISH_DRAIN_IDLE, ZMODEM_PROGRESS_BYTES,
        ZMODEM_PROGRESS_INTERVAL, ZmodemAction, ZmodemDetectResult, ZmodemDetector,
        ZmodemDirection, ZmodemDownloadOoDrain, ZmodemEvent, ZmodemTransfer,
        ZmodemUploadConflictMode, ZmodemUploadDrain, cancel_sequence, zmodem_mtime_from_metadata,
        zmodem_mtime_from_system_time,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, UNIX_EPOCH};
    use zmodem2::{Encoding, Frame, Header};

    fn detected_direction(result: ZmodemDetectResult) -> ZmodemDirection {
        match result {
            ZmodemDetectResult::Detected { direction, .. } => direction,
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    fn zhex_header(frame: Frame) -> Vec<u8> {
        let mut wire = Vec::new();
        Header::new(Encoding::ZHEX, frame, &[0; 4])
            .write(&mut wire)
            .expect("write zmodem header")
            .expect("complete zmodem header");
        wire
    }

    fn write_zrinit() -> Vec<u8> {
        zhex_header(Frame::ZRINIT)
    }

    fn write_zskip() -> Vec<u8> {
        zhex_header(Frame::ZSKIP)
    }

    fn write_zfin() -> Vec<u8> {
        zhex_header(Frame::ZFIN)
    }

    fn temp_upload_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "nyaterm-zmodem-{name}-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"zmodem upload test").expect("write temp upload file");
        path
    }

    fn cleanup_temp_files(paths: &[PathBuf]) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }

    fn has_complete_event(actions: &[ZmodemAction]) -> bool {
        actions.iter().any(|action| {
            matches!(
                action,
                ZmodemAction::EmitEvent(ZmodemEvent::Complete {
                    direction: ZmodemDirection::Upload,
                    ..
                })
            )
        })
    }

    fn failed_reason(actions: &[ZmodemAction]) -> Option<&str> {
        actions.iter().find_map(|action| match action {
            ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => Some(reason.as_str()),
            _ => None,
        })
    }

    fn has_cancel_sequence(actions: &[ZmodemAction]) -> bool {
        actions.iter().any(|action| match action {
            ZmodemAction::SendToRemote(data) => data == &cancel_sequence(),
            _ => false,
        })
    }

    #[test]
    fn detects_complete_zhex_download_header() {
        let mut detector = ZmodemDetector::new();
        let result = detector.feed(b"ready\r\n**\x18B00");

        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Download);
                assert_eq!(passthrough, b"ready\r\n");
                assert_eq!(initial_bytes, b"**\x18B00");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn zmodem_events_serialize_lowercase_directions() {
        let detected = serde_json::to_value(ZmodemEvent::Detected {
            direction: ZmodemDirection::Download,
        })
        .expect("detected event json");
        assert_eq!(
            detected,
            json!({
                "type": "detected",
                "direction": "download",
            })
        );

        let progress = serde_json::to_value(ZmodemEvent::Progress {
            file_name: "sample.bin".to_string(),
            bytes_transferred: 128,
            total_size: 256,
            local_path: None,
            direction: ZmodemDirection::Upload,
        })
        .expect("progress event json");
        assert_eq!(
            progress,
            json!({
                "type": "progress",
                "fileName": "sample.bin",
                "bytesTransferred": 128,
                "totalSize": 256,
                "localPath": null,
                "direction": "upload",
            })
        );

        let download_progress = serde_json::to_value(ZmodemEvent::Progress {
            file_name: "download.log".to_string(),
            bytes_transferred: 512,
            total_size: 512,
            local_path: Some("/tmp/download.log".to_string()),
            direction: ZmodemDirection::Download,
        })
        .expect("download progress event json");
        assert_eq!(
            download_progress,
            json!({
                "type": "progress",
                "fileName": "download.log",
                "bytesTransferred": 512,
                "totalSize": 512,
                "localPath": "/tmp/download.log",
                "direction": "download",
            })
        );

        let complete = serde_json::to_value(ZmodemEvent::Complete {
            direction: ZmodemDirection::Download,
            file_count: 1,
        })
        .expect("complete event json");
        assert_eq!(
            complete,
            json!({
                "type": "complete",
                "direction": "download",
                "fileCount": 1,
            })
        );
    }

    #[test]
    fn upload_zskip_fails_overwrite_transfer() {
        let path = temp_upload_file("overwrite-refused");
        let mut transfer = ZmodemTransfer::new(ZmodemDirection::Upload, &write_zrinit());
        let accept_actions =
            transfer.accept_upload(vec![path.clone()], ZmodemUploadConflictMode::Overwrite, false);

        assert!(failed_reason(&accept_actions).is_none());

        let actions = transfer.feed_incoming(&write_zskip());

        assert!(transfer.is_done());
        assert_eq!(transfer.file_count, 0);
        assert!(has_cancel_sequence(&actions));
        assert!(!has_complete_event(&actions));
        assert!(
            failed_reason(&actions)
                .expect("failed event")
                .contains("destination may be unwritable")
        );

        cleanup_temp_files(&[path]);
    }

    #[test]
    fn upload_zskip_fails_batch_without_counting_skipped_file() {
        let first = temp_upload_file("batch-refused-first");
        let second = temp_upload_file("batch-refused-second");
        let mut transfer = ZmodemTransfer::new(ZmodemDirection::Upload, &write_zrinit());
        let accept_actions = transfer.accept_upload(
            vec![first.clone(), second.clone()],
            ZmodemUploadConflictMode::Overwrite,
            false,
        );

        assert!(failed_reason(&accept_actions).is_none());

        let actions = transfer.feed_incoming(&write_zskip());

        assert!(transfer.is_done());
        assert_eq!(transfer.file_count, 0);
        assert!(has_cancel_sequence(&actions));
        assert!(!has_complete_event(&actions));
        assert!(failed_reason(&actions).is_some());

        cleanup_temp_files(&[first, second]);
    }

    #[test]
    fn upload_zskip_in_skip_mode_does_not_count_file_as_uploaded() {
        let path = temp_upload_file("intentional-skip");
        let mut transfer = ZmodemTransfer::new(ZmodemDirection::Upload, &write_zrinit());
        let accept_actions =
            transfer.accept_upload(vec![path.clone()], ZmodemUploadConflictMode::Skip, false);

        assert!(failed_reason(&accept_actions).is_none());

        let skip_actions = transfer.feed_incoming(&write_zskip());

        assert!(!transfer.is_done());
        assert_eq!(transfer.file_count, 0);
        assert!(failed_reason(&skip_actions).is_none());
        assert!(!has_complete_event(&skip_actions));

        let finish_actions = transfer.feed_incoming(&write_zfin());

        assert!(transfer.is_done());
        assert_eq!(transfer.file_count, 0);
        assert!(failed_reason(&finish_actions).is_none());
        assert!(has_complete_event(&finish_actions));

        cleanup_temp_files(&[path]);
    }

    #[test]
    fn zmodem_mtime_disabled_returns_zero() {
        let path = std::env::temp_dir().join(format!(
            "nyaterm-zmodem-mtime-disabled-{}.tmp",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).expect("create temp file");
        let metadata = file.metadata().expect("temp metadata");

        assert_eq!(zmodem_mtime_from_metadata(&metadata, false), 0);

        drop(file);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn zmodem_mtime_converts_unix_seconds() {
        let time = UNIX_EPOCH + Duration::from_secs(1_710_000_000);

        assert_eq!(zmodem_mtime_from_system_time(Ok(time)), 1_710_000_000);
    }

    #[test]
    fn zmodem_mtime_failure_returns_zero() {
        let error = std::io::Error::new(std::io::ErrorKind::Other, "modified failed");

        assert_eq!(zmodem_mtime_from_system_time(Err(error)), 0);
    }

    #[test]
    fn zmodem_mtime_pre_epoch_returns_zero() {
        let time = UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .expect("pre epoch system time");

        assert_eq!(zmodem_mtime_from_system_time(Ok(time)), 0);
    }

    #[test]
    fn zmodem_mtime_overflow_returns_zero() {
        let time = UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX) + 1);

        assert_eq!(zmodem_mtime_from_system_time(Ok(time)), 0);
    }

    #[test]
    fn detects_zbin_upload_header_across_chunks() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"prefix\r\n**\x18") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"prefix\r\n")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }

        let result = detector.feed(b"A\x01payload");
        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Upload);
                assert!(passthrough.is_empty());
                assert_eq!(initial_bytes, b"**\x18A\x01payload");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn detects_rz_zhex_upload_header_after_prompt_text() {
        let mut detector = ZmodemDetector::new();
        let result = detector.feed(b"\x18z waiting to receive.**\x18B01");

        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Upload);
                assert_eq!(passthrough, b"\x18z waiting to receive.");
                assert_eq!(initial_bytes, b"**\x18B01");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn reports_single_line_start_zpad_as_pending_startup_risk() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"*") {
            ZmodemDetectResult::NoMatch { passthrough } => assert!(passthrough.is_empty()),
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }
        assert!(detector.has_pending_prefix());
    }

    #[test]
    fn detects_rz_zhex_upload_header_split_after_prompt_text() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"\x18z waiting to receive.**") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"\x18z waiting to receive.")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }
        assert!(detector.has_pending_prefix());

        assert_eq!(
            detected_direction(detector.feed(b"\x18B01")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn detects_zhex_frame_type_split_after_first_hex_digit() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"**\x18B0") {
            ZmodemDetectResult::NoMatch { passthrough } => assert!(passthrough.is_empty()),
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }
        assert!(detector.has_pending_prefix());

        assert_eq!(
            detected_direction(detector.feed(b"1rest")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn passthroughs_interactive_asterisks_immediately() {
        let mut detector = ZmodemDetector::new();
        let chunks: [&[u8]; 4] = [b"docker", b"*", b"*", b"*"];
        let mut visible = Vec::new();

        for chunk in chunks {
            match detector.feed(chunk) {
                ZmodemDetectResult::NoMatch { passthrough } => {
                    visible.extend_from_slice(&passthrough);
                }
                ZmodemDetectResult::Detected { .. } => panic!("unexpected ZMODEM detection"),
            }
        }

        assert_eq!(visible, b"docker***");
    }

    #[test]
    fn waits_for_zmodem_header_after_zdle_even_inside_text() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"prefix**\x18") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"prefix")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected ZMODEM detection"),
        }

        assert_eq!(
            detected_direction(detector.feed(b"A\x01payload")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn progress_throttle_emits_first_time() {
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.should_emit_at(0, false, Instant::now()));
    }

    #[test]
    fn progress_throttle_respects_time_and_byte_thresholds() {
        let mut throttle = ProgressThrottle::new();
        let start = Instant::now();
        assert!(throttle.should_emit_at(0, false, start));
        assert!(!throttle.should_emit_at(1, false, start + Duration::from_millis(10)));
        assert!(throttle.should_emit_at(
            ZMODEM_PROGRESS_BYTES,
            false,
            start + Duration::from_millis(20)
        ));
        assert!(throttle.should_emit_at(
            ZMODEM_PROGRESS_BYTES + 1,
            false,
            start + ZMODEM_PROGRESS_INTERVAL + Duration::from_millis(30)
        ));
    }

    #[test]
    fn progress_throttle_force_emits_completion() {
        let mut throttle = ProgressThrottle::new();
        let start = Instant::now();
        assert!(throttle.should_emit_at(128, false, start));
        assert!(throttle.should_emit_at(129, true, start + Duration::from_millis(1)));
    }

    #[test]
    fn upload_drain_suppresses_residual_bytes_until_idle() {
        let mut drain = ZmodemUploadDrain::new();
        let start = Instant::now();

        assert!(!drain.should_suppress(start));

        drain.start(start);
        assert!(drain.should_suppress(start + ZMODEM_FINISH_DRAIN_IDLE / 2));
        assert!(drain.should_suppress(start + ZMODEM_FINISH_DRAIN_IDLE));
        assert!(!drain.should_suppress(start + ZMODEM_FINISH_DRAIN_IDLE * 3));
    }

    #[test]
    fn upload_drain_allows_prompt_text() {
        let mut drain = ZmodemUploadDrain::new();
        let start = Instant::now();

        drain.start(start);
        assert_eq!(
            drain.filter(
                b"\r\n[root@ubuntu ~]# ",
                start + ZMODEM_FINISH_DRAIN_IDLE / 2
            ),
            b"\r\n[root@ubuntu ~]# "
        );
        assert_eq!(drain.filter(b"next", start + Duration::from_millis(1)), b"next");
    }

    #[test]
    fn upload_drain_suppresses_binary_residue() {
        let mut drain = ZmodemUploadDrain::new();
        let start = Instant::now();

        drain.start(start);
        assert_eq!(
            drain.filter(b"\x18\x18\x00\xff", start + Duration::from_millis(1)),
            b""
        );
        assert!(drain.should_suppress(start + Duration::from_millis(2)));
    }

    #[test]
    fn download_oo_drain_strips_only_zmodem_trailer() {
        let mut drain = ZmodemDownloadOoDrain::new();
        let start = Instant::now();

        drain.start(start);
        assert_eq!(drain.filter(b"O", start + Duration::from_millis(1)), b"");
        assert_eq!(
            drain.filter(b"Oprompt", start + Duration::from_millis(2)),
            b"prompt"
        );
        assert_eq!(drain.filter(b"next", start + Duration::from_millis(3)), b"next");
    }

    #[test]
    fn download_oo_drain_expires_without_stripping_prompt() {
        let mut drain = ZmodemDownloadOoDrain::new();
        let start = Instant::now();

        drain.start(start);
        assert_eq!(
            drain.filter(b"Output", start + ZMODEM_FINISH_DRAIN_IDLE * 2),
            b"Output"
        );
    }
}

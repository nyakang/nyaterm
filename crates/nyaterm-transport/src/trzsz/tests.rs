use std::io::Read;
use std::time::{Duration, Instant};

use base64::Engine as _;

use super::{
    TRZSZ_MAX_DECODED_PAYLOAD_BYTES, TRZSZ_UPLOAD_DATA_CHUNK_SIZE, TrzszAction, TrzszConfig,
    TrzszDetectResult, TrzszDetector, TrzszDownloadEngine, TrzszDownloadError, TrzszDownloadEvent,
    TrzszMode, TrzszOutputEvent, TrzszProtocolFrame, TrzszProtocolPayload, TrzszProtocolStream,
    TrzszTransferEvent, TrzszTransferPhase, TrzszTransferState, TrzszTrigger, TrzszUploadEngine,
    TrzszUploadEntry, TrzszUploadError, TrzszUploadEvent, TrzszUploadPayload, TrzszUploadSource,
    build_trzsz_action_frame, build_trzsz_config_frame, build_trzsz_integer_frame,
    build_trzsz_string_frame, bytes_payload, encode_trzsz_string, parse_trzsz_action_frame,
    parse_trzsz_config_frame, parse_trzsz_json_frame, parse_trzsz_protocol_frame,
    trzsz_fail_response,
};

fn decode_trzsz_string(encoded: &[u8]) -> String {
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64");
    let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).expect("zlib");
    decoded
}

#[test]
fn modern_text_data_digits_are_not_a_binary_header_and_name_json_is_not_config() {
    let mut config = TrzszConfig::local_default(None, false);
    config.protocol = Some(4);
    let mut stream = TrzszProtocolStream::new();
    stream.filter_terminal_output(&build_trzsz_config_frame(&config, false));
    let name = build_trzsz_string_frame(
        "NAME",
        br#"{"path_id":0,"path_name":["file"],"size":3}"#,
        "\n",
    );
    let name = stream.filter_terminal_output(&name).frames.remove(0);
    assert!(parse_trzsz_config_frame(&name).is_none());
    let frame = stream
        .filter_terminal_output(b"#DATA:1234\n")
        .frames
        .remove(0);
    assert_eq!(frame.payload, TrzszProtocolPayload::Raw("1234".into()));
}

#[test]
fn old_directory_name_json_is_parsed_with_a_plain_name_ack() {
    for protocol in [0, 1, 2] {
        let mut engine = TrzszDownloadEngine::new(false);
        engine.set_directory_mode(true);
        engine.set_protocol_version(protocol);
        engine
            .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
            .unwrap();
        let frame = build_trzsz_string_frame(
            "NAME",
            br#"{"path_id":7,"path_name":["folder"],"is_dir":true}"#,
            "\n",
        );
        let step = engine
            .observe_frame(parse_trzsz_protocol_frame(&frame).unwrap())
            .unwrap();
        assert!(matches!(
            step.events[0],
            TrzszDownloadEvent::Directory { path_id: 7, .. }
        ));
        assert_eq!(
            bytes_payload(&parse_trzsz_protocol_frame(&step.responses[0]).unwrap()).unwrap(),
            b"folder"
        );
    }
}

#[test]
fn modern_independent_upload_roots_receive_distinct_path_ids() {
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![
            TrzszUploadEntry::from_bytes("a", Vec::new()),
            TrzszUploadEntry::from_bytes("b", Vec::new()),
        ],
    );
    engine.set_protocol_version(3);
    engine.begin().unwrap();
    let first = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:2\n").unwrap())
        .unwrap();
    let json =
        parse_trzsz_json_frame(&parse_trzsz_protocol_frame(&first.responses[0]).unwrap()).unwrap();
    assert_eq!(json["path_id"], 0);
    // Inspect the second NAME through the same helper used by every file.
    let second = engine.send_name(1, Vec::new()).unwrap();
    let json =
        parse_trzsz_json_frame(&parse_trzsz_protocol_frame(&second.responses[0]).unwrap()).unwrap();
    assert_eq!(json["path_id"], 1);

    let mut explicit = TrzszUploadEntry::from_bytes("explicit", Vec::new());
    explicit.source = Some(TrzszUploadSource {
        path_id: 0,
        path_name: vec!["explicit".into()],
        is_dir: false,
        size: 0,
        perm: None,
        archive: false,
    });
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![
            TrzszUploadEntry::from_bytes("synthetic", Vec::new()),
            explicit,
        ],
    );
    engine.set_protocol_version(3);
    let first = engine.send_name(0, Vec::new()).unwrap();
    let json =
        parse_trzsz_json_frame(&parse_trzsz_protocol_frame(&first.responses[0]).unwrap()).unwrap();
    assert_eq!(json["path_id"], 1);
    let second = engine.send_name(1, Vec::new()).unwrap();
    let json =
        parse_trzsz_json_frame(&parse_trzsz_protocol_frame(&second.responses[0]).unwrap()).unwrap();
    assert_eq!(json["path_id"], 0);
}

#[test]
fn large_modern_wire_frames_stream_with_bounded_buffers_and_ack_only_after_writes() {
    for binary in [false, true] {
        let mut config = TrzszConfig::local_default(None, false);
        config.protocol = Some(4);
        config.compress = Some(2);
        config.binary = binary;
        let data = vec![0xa7; 2 * 1024 * 1024 + 1];
        let wire = if binary {
            data.clone()
        } else {
            base64::engine::general_purpose::STANDARD
                .encode(&data)
                .into_bytes()
        };
        let mut engine = TrzszDownloadEngine::new(false);
        engine.configure(&config).unwrap();
        engine
            .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
            .unwrap();
        let name = build_trzsz_string_frame(
            "NAME",
            format!(
                r#"{{"path_id":0,"path_name":["large"],"size":{}}}"#,
                data.len()
            )
            .as_bytes(),
            "\n",
        );
        engine
            .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
            .unwrap();
        engine
            .observe_frame(TrzszProtocolFrame {
                frame_type: "SIZE".into(),
                payload: TrzszProtocolPayload::Integer(data.len() as i64),
            })
            .unwrap();
        let mut stream = TrzszProtocolStream::new();
        stream.filter_terminal_output(&build_trzsz_config_frame(&config, false));
        let mut frame = if binary {
            format!("#DATA:{}\n", wire.len()).into_bytes()
        } else {
            b"#DATA:".to_vec()
        };
        frame.extend(&wire);
        if !binary {
            frame.push(b'\n');
        }
        let mut written = 0;
        let mut acks = Vec::new();
        for chunk in frame.chunks(4093) {
            for frame in stream.filter_terminal_output(chunk).frames {
                let step = engine
                    .observe_frame_into(frame, |event| {
                        if let TrzszDownloadEvent::Data { bytes, .. } = event {
                            assert!(bytes.len() <= 256 * 1024);
                            written += bytes.len();
                        }
                        Ok(())
                    })
                    .unwrap();
                if !step.responses.is_empty() {
                    assert_eq!(written, data.len());
                }
                acks.extend(step.responses);
            }
            assert!(stream.pending_line.len() <= TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 8);
            assert!(stream.binary_data.len() <= TRZSZ_UPLOAD_DATA_CHUNK_SIZE);
        }
        assert_eq!(
            acks,
            vec![format!("#SUCC:{}/{}\n", wire.len(), data.len()).into_bytes()]
        );
        let finish = if binary {
            b"#DATA:0\n".as_slice()
        } else {
            b"#DATA:\n"
        };
        for frame in stream.filter_terminal_output(finish).frames {
            engine.observe_frame(frame).unwrap();
        }
        let md5 = build_trzsz_string_frame("MD5", &md5::compute(&data).0, "\n");
        engine
            .observe_frame(parse_trzsz_protocol_frame(&md5).unwrap())
            .unwrap();
        assert!(engine.is_completed());
    }
}

#[test]
fn fast_scan_predicate_skips_ordinary_output() {
    assert!(!TrzszDetector::output_may_contain_trigger(
        b"Last login: Wed Jul 15\n$ "
    ));
    assert!(TrzszDetector::new().is_idle());
}

#[test]
fn fast_scan_predicate_keeps_split_trigger_prefixes() {
    assert!(TrzszDetector::output_may_contain_trigger(b":"));
    assert!(TrzszDetector::output_may_contain_trigger(b"::TRZSZ"));
    assert!(TrzszDetector::output_may_contain_trigger(
        b"noise\n::TRZSZ:TRANSFER:"
    ));
}

#[test]
fn detects_complete_trigger_with_metadata() {
    let mut detector = TrzszDetector::new();

    let result = detector.feed(b"hello ::TRZSZ:TRANSFER:S:1.2.3:1700000000000:3456 tail");

    match result {
        TrzszDetectResult::Detected {
            trigger,
            passthrough,
            remaining,
        } => {
            assert_eq!(passthrough, b"hello ");
            assert_eq!(remaining, b" tail");
            assert_eq!(trigger.mode, TrzszMode::Send);
            assert_eq!(trigger.version, "1.2.3");
            assert_eq!(trigger.unique_id.as_deref(), Some("1700000000000"));
            assert!(!trigger.remote_is_windows);
            assert_eq!(trigger.tunnel_port, Some(3456));
            assert_eq!(trigger.raw, b"::TRZSZ:TRANSFER:S:1.2.3:1700000000000:3456");
        }
        TrzszDetectResult::NoMatch { .. } => panic!("expected trzsz trigger"),
    }
}

#[test]
fn holds_split_trigger_prefix_until_complete() {
    let mut detector = TrzszDetector::new();

    assert_eq!(
        detector.feed(b"before ::TRZ"),
        TrzszDetectResult::NoMatch {
            passthrough: b"before ".to_vec()
        }
    );
    assert_eq!(
        detector.feed(b"SZ:TRANSFER:R:0.1.0"),
        TrzszDetectResult::Detected {
            trigger: TrzszTrigger {
                mode: TrzszMode::Receive,
                version: "0.1.0".to_string(),
                unique_id: None,
                remote_is_windows: false,
                tunnel_port: None,
                raw: b"::TRZSZ:TRANSFER:R:0.1.0".to_vec(),
            },
            passthrough: Vec::new(),
            remaining: Vec::new(),
        }
    );
}

#[test]
fn isolated_colons_are_released_without_another_output_chunk() {
    for text in [b":".as_slice(), b"::".as_slice(), b"prompt:".as_slice()] {
        let mut detector = TrzszDetector::new();
        let now = Instant::now();
        let TrzszDetectResult::NoMatch { mut passthrough } = detector.feed_at(text, now) else {
            panic!("ordinary colon text must not trigger a transfer");
        };
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_millis(25))
                .is_empty()
        );
        passthrough.extend(detector.flush_pending_if_idle(now + Duration::from_millis(100)));
        assert_eq!(passthrough, text);
        assert!(detector.is_idle());
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_secs(2))
                .is_empty()
        );
    }
}

#[test]
fn repeated_colon_echoes_do_not_lag_two_characters_or_pollute_backspace() {
    let mut detector = TrzszDetector::new();
    let started_at = Instant::now();
    let mut displayed = Vec::new();
    for press in 0..7 {
        let now = started_at + Duration::from_millis(press * 300);
        let TrzszDetectResult::NoMatch { passthrough } = detector.feed_at(b":", now) else {
            panic!("colon echo must not trigger a transfer");
        };
        displayed.extend(passthrough);
        displayed.extend(detector.flush_pending_if_idle(now + Duration::from_millis(100)));
        assert_eq!(displayed, vec![b':'; press as usize + 1]);
    }
    assert_eq!(
        detector.feed(b"\x08\x1b[K"),
        TrzszDetectResult::NoMatch {
            passthrough: b"\x08\x1b[K".to_vec()
        }
    );
}

#[test]
fn backspace_before_idle_deadline_releases_colons_once_in_wire_order() {
    let mut detector = TrzszDetector::new();
    let now = Instant::now();
    detector.feed_at(b"::", now);
    assert_eq!(
        detector.feed_at(b"\x08\x1b[K", now + Duration::from_millis(20)),
        TrzszDetectResult::NoMatch {
            passthrough: b"::\x08\x1b[K".to_vec()
        }
    );
    assert!(
        detector
            .flush_pending_if_idle(now + Duration::from_secs(2))
            .is_empty()
    );
}

#[test]
fn idle_flush_preserves_trigger_split_at_every_prefix_boundary() {
    let marker = b"::TRZSZ:TRANSFER:S:1.2.3:1700000000000";
    for split in 1..=b"::TRZSZ:TRANSFER:".len() {
        let mut detector = TrzszDetector::new();
        let now = Instant::now();
        assert_eq!(
            detector.feed_at(&marker[..split], now),
            TrzszDetectResult::NoMatch {
                passthrough: Vec::new()
            }
        );
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_millis(25))
                .is_empty()
        );
        let TrzszDetectResult::Detected {
            trigger,
            passthrough,
            remaining,
        } = detector.feed_at(&marker[split..], now + Duration::from_millis(30))
        else {
            panic!("split marker must still be recognized at boundary {split}");
        };
        assert_eq!(trigger.raw, marker);
        assert!(passthrough.is_empty());
        assert!(remaining.is_empty());
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_secs(2))
                .is_empty()
        );
    }
}

#[test]
fn recognizable_trigger_prefix_gets_more_time_but_eventually_releases() {
    let mut detector = TrzszDetector::new();
    let now = Instant::now();
    detector.feed_at(b"::TRZ", now);
    assert!(
        detector
            .flush_pending_if_idle(now + Duration::from_millis(500))
            .is_empty()
    );
    assert!(matches!(
        detector.feed_at(b"SZ:TRANSFER:S:1.2.3", now + Duration::from_millis(600)),
        TrzszDetectResult::Detected { .. }
    ));
    detector.feed_at(b"::TRZ", now + Duration::from_secs(1));
    assert_eq!(
        detector.flush_pending_if_idle(now + Duration::from_secs(2)),
        b"::TRZ"
    );
    assert!(detector.is_idle());
}

#[test]
fn prefix_idle_deadline_tracks_the_latest_chunk_and_reset_discards_it() {
    let mut detector = TrzszDetector::new();
    let now = Instant::now();
    detector.feed_at(b":", now);
    detector.feed_at(b":", now + Duration::from_millis(40));
    assert!(
        detector
            .flush_pending_if_idle(now + Duration::from_millis(70))
            .is_empty()
    );
    assert_eq!(
        detector.flush_pending_if_idle(now + Duration::from_millis(100)),
        b"::"
    );
    detector.feed_at(b":", now);
    detector.reset();
    assert!(
        detector
            .flush_pending_if_idle(now + Duration::from_secs(2))
            .is_empty()
    );
}

#[test]
fn passes_plain_text_when_suffix_is_not_a_trigger() {
    let mut detector = TrzszDetector::new();

    assert_eq!(
        detector.feed(b"value: still text"),
        TrzszDetectResult::NoMatch {
            passthrough: b"value: still text".to_vec()
        }
    );
}

#[test]
fn leaves_incomplete_trigger_pending_after_noise() {
    let mut detector = TrzszDetector::new();

    assert_eq!(
        detector.feed(b"log ::TRZSZ:TRANSFER:D:1."),
        TrzszDetectResult::NoMatch {
            passthrough: b"log ".to_vec()
        }
    );
    assert_eq!(
        detector.feed(b"2.3:42"),
        TrzszDetectResult::Detected {
            trigger: TrzszTrigger {
                mode: TrzszMode::Directory,
                version: "1.2.3".to_string(),
                unique_id: Some("42".to_string()),
                remote_is_windows: false,
                tunnel_port: None,
                raw: b"::TRZSZ:TRANSFER:D:1.2.3:42".to_vec(),
            },
            passthrough: Vec::new(),
            remaining: Vec::new(),
        }
    );
}

#[test]
fn non_numeric_suffix_after_version_is_remaining_text() {
    let mut detector = TrzszDetector::new();

    assert_eq!(
        detector.feed(b"::TRZSZ:TRANSFER:S:1.2.3:abc"),
        TrzszDetectResult::Detected {
            trigger: TrzszTrigger {
                mode: TrzszMode::Send,
                version: "1.2.3".to_string(),
                unique_id: None,
                remote_is_windows: false,
                tunnel_port: None,
                raw: b"::TRZSZ:TRANSFER:S:1.2.3".to_vec(),
            },
            passthrough: Vec::new(),
            remaining: b":abc".to_vec(),
        }
    );
}

#[test]
fn waits_for_split_optional_numeric_field() {
    let mut detector = TrzszDetector::new();

    assert_eq!(
        detector.feed(b"::TRZSZ:TRANSFER:R:1.2.3:"),
        TrzszDetectResult::NoMatch {
            passthrough: Vec::new()
        }
    );
    assert_eq!(
        detector.feed(b"99"),
        TrzszDetectResult::Detected {
            trigger: TrzszTrigger {
                mode: TrzszMode::Receive,
                version: "1.2.3".to_string(),
                unique_id: Some("99".to_string()),
                remote_is_windows: false,
                tunnel_port: None,
                raw: b"::TRZSZ:TRANSFER:R:1.2.3:99".to_vec(),
            },
            passthrough: Vec::new(),
            remaining: Vec::new(),
        }
    );
}

#[test]
fn marks_official_windows_server_unique_ids() {
    let mut detector = TrzszDetector::new();

    match detector.feed(b"::TRZSZ:TRANSFER:S:1.2.3:1") {
        TrzszDetectResult::Detected { trigger, .. } => {
            assert_eq!(trigger.unique_id.as_deref(), Some("1"));
            assert!(trigger.remote_is_windows);
        }
        TrzszDetectResult::NoMatch { .. } => panic!("expected windows server trigger"),
    }

    match detector.feed(b"::TRZSZ:TRANSFER:R:1.2.3:1700000000010") {
        TrzszDetectResult::Detected { trigger, .. } => {
            assert_eq!(trigger.unique_id.as_deref(), Some("1700000000010"));
            assert!(trigger.remote_is_windows);
        }
        TrzszDetectResult::NoMatch { .. } => panic!("expected windows server trigger"),
    }

    match detector.feed(b"::TRZSZ:TRANSFER:D:1.2.3:1700000000000") {
        TrzszDetectResult::Detected { trigger, .. } => {
            assert_eq!(trigger.unique_id.as_deref(), Some("1700000000000"));
            assert!(!trigger.remote_is_windows);
        }
        TrzszDetectResult::NoMatch { .. } => panic!("expected non-windows trigger"),
    }
}

#[test]
fn repeated_tracked_unique_id_is_passthrough() {
    let mut detector = TrzszDetector::new();
    let marker = b"::TRZSZ:TRANSFER:S:1.2.3:1700000000999";

    match detector.feed(marker) {
        TrzszDetectResult::Detected { trigger, .. } => {
            assert_eq!(trigger.unique_id.as_deref(), Some("1700000000999"));
        }
        TrzszDetectResult::NoMatch { .. } => panic!("expected first trigger"),
    }

    assert_eq!(
        detector.feed(marker),
        TrzszDetectResult::NoMatch {
            passthrough: marker.to_vec()
        }
    );
}

#[test]
fn repeated_short_unique_id_can_trigger_again() {
    let mut detector = TrzszDetector::new();
    let marker = b"::TRZSZ:TRANSFER:R:1.2.3:42";

    assert!(matches!(
        detector.feed(marker),
        TrzszDetectResult::Detected { .. }
    ));
    assert!(matches!(
        detector.feed(marker),
        TrzszDetectResult::Detected { .. }
    ));
}

#[test]
fn repeated_non_windows_thirteen_digit_unique_id_can_trigger_again() {
    let mut detector = TrzszDetector::new();
    let marker = b"::TRZSZ:TRANSFER:D:1.2.3:1700000000000";

    assert!(matches!(
        detector.feed(marker),
        TrzszDetectResult::Detected { .. }
    ));
    let second = detector.feed(marker);
    if cfg!(windows) {
        assert_eq!(
            second,
            TrzszDetectResult::NoMatch {
                passthrough: marker.to_vec()
            }
        );
    } else {
        assert!(matches!(second, TrzszDetectResult::Detected { .. }));
    }
}

#[test]
fn reset_keeps_repeated_unique_id_guard() {
    let mut detector = TrzszDetector::new();
    let marker = b"::TRZSZ:TRANSFER:S:1.2.3:1700000000998";

    assert!(matches!(
        detector.feed(marker),
        TrzszDetectResult::Detected { .. }
    ));
    detector.reset();

    assert_eq!(
        detector.feed(marker),
        TrzszDetectResult::NoMatch {
            passthrough: marker.to_vec()
        }
    );
}

#[test]
fn reset_drops_pending_prefix() {
    let mut detector = TrzszDetector::new();

    let _ = detector.feed(b"::TRZ");
    detector.reset();

    assert_eq!(
        detector.feed(b"SZ:TRANSFER:S:1.2.3"),
        TrzszDetectResult::NoMatch {
            passthrough: b"SZ:TRANSFER:S:1.2.3".to_vec()
        }
    );
}

#[test]
fn filters_trigger_marker_from_terminal_output() {
    let mut detector = TrzszDetector::new();

    let output =
        detector.filter_terminal_output(b"before ::TRZSZ:TRANSFER:S:1.2.3:1700000000000 after");

    assert_eq!(output.passthrough, b"before  after");
    assert_eq!(output.triggers.len(), 1);
    assert_eq!(output.triggers[0].mode, TrzszMode::Send);
    assert_eq!(output.triggers[0].version, "1.2.3");
    assert_eq!(
        output.triggers[0].unique_id.as_deref(),
        Some("1700000000000")
    );
}

#[test]
fn filters_multiple_markers_and_keeps_tail_prefix_pending() {
    let mut detector = TrzszDetector::new();

    let output = detector
        .filter_terminal_output(b"a::TRZSZ:TRANSFER:S:1.2.3b::TRZSZ:TRANSFER:R:2.3.4c::TRZ");

    assert_eq!(output.passthrough, b"abc");
    assert_eq!(output.triggers.len(), 2);
    assert_eq!(output.triggers[0].mode, TrzszMode::Send);
    assert_eq!(output.triggers[1].mode, TrzszMode::Receive);

    let output = detector.filter_terminal_output(b"SZ:TRANSFER:D:3.4.5d");
    assert_eq!(output.passthrough, b"d");
    assert_eq!(output.triggers.len(), 1);
    assert_eq!(output.triggers[0].mode, TrzszMode::Directory);
    assert_eq!(output.triggers[0].version, "3.4.5");
}

#[test]
fn scans_passthrough_and_triggers_in_order() {
    let mut detector = TrzszDetector::new();

    let scan = detector.scan_terminal_output(b"#keep\n::TRZSZ:TRANSFER:S:1.2.3#ACT:bad\n");

    assert_eq!(
        scan.events,
        vec![
            TrzszOutputEvent::Passthrough(b"#keep\n".to_vec()),
            TrzszOutputEvent::Trigger(TrzszTrigger {
                mode: TrzszMode::Send,
                version: "1.2.3".to_string(),
                unique_id: None,
                remote_is_windows: false,
                tunnel_port: None,
                raw: b"::TRZSZ:TRANSFER:S:1.2.3".to_vec(),
            }),
            TrzszOutputEvent::Passthrough(b"#ACT:bad\n".to_vec()),
        ]
    );
}

#[test]
fn protocol_stream_filters_split_frame_and_keeps_plain_text() {
    let mut stream = TrzszProtocolStream::new();
    let action = r#"{"protocol":4,"binary":true}"#;
    let line = format!("#ACT:{}\n", encode_trzsz_string(action.as_bytes()));

    let first = stream.filter_terminal_output(&line.as_bytes()[..8]);
    assert!(first.passthrough.is_empty());
    assert!(first.frames.is_empty());

    let second = stream.filter_terminal_output(&line.as_bytes()[8..]);
    assert!(second.passthrough.is_empty());
    assert_eq!(second.frames.len(), 1);
    assert_eq!(second.frames[0].frame_type, "ACT");
    assert_eq!(
        parse_trzsz_json_frame(&second.frames[0]).unwrap()["protocol"],
        4
    );

    let plain = stream.filter_terminal_output(b"done\n");
    assert_eq!(plain.passthrough, b"done\n");
    assert!(plain.frames.is_empty());
}

#[test]
fn protocol_stream_consumes_binary_data_after_header() {
    let mut stream = TrzszProtocolStream::new();

    let output = stream.filter_terminal_output(b"#DATA:4\nabcd#SUCC:not-base64\n");

    assert_eq!(output.passthrough, Vec::<u8>::new());
    assert_eq!(output.consumed_binary_bytes, 4);
    assert_eq!(output.frames.len(), 3);
    assert_eq!(
        output.frames[0],
        TrzszProtocolFrame {
            frame_type: "DATA".to_string(),
            payload: TrzszProtocolPayload::Integer(4),
        }
    );
    assert_eq!(
        output.frames[1],
        TrzszProtocolFrame {
            frame_type: "DATA".to_string(),
            payload: TrzszProtocolPayload::EncodedBytes(b"abcd".to_vec()),
        }
    );
    assert_eq!(output.frames[2].frame_type, "SUCC");
}

#[test]
fn stale_trigger_text_with_protocol_markers_is_passthrough() {
    let mut detector = TrzszDetector::new();

    let data = b"::TRZSZ:TRANSFER:S:1.2.3:1700000000000 trailing terminal #CFG:payload";
    let output = detector.filter_terminal_output(data);

    assert_eq!(output.passthrough, data);
    assert!(output.triggers.is_empty());

    let data = b"::TRZSZ:TRANSFER:R:1.2.3:1700000000000 terminal Saved file.txt";
    let output = detector.filter_terminal_output(data);

    assert_eq!(output.passthrough, data);
    assert!(output.triggers.is_empty());
}

#[test]
fn builds_official_fail_response_line() {
    let response = trzsz_fail_response("trzsz unsupported", false);
    assert!(response.starts_with(b"#fail:"));
    assert!(response.ends_with(b"\n"));
    assert!(!response.ends_with(b"!\n"));
    let encoded = &response[b"#fail:".len()..response.len() - 1];
    assert_eq!(decode_trzsz_string(encoded), "trzsz unsupported");
}

#[test]
fn builds_windows_fail_response_line() {
    let response = trzsz_fail_response("trzsz unsupported", true);
    assert!(response.starts_with(b"#fail:"));
    assert!(response.ends_with(b"!\n"));
    let encoded = &response[b"#fail:".len()..response.len() - 2];
    assert_eq!(decode_trzsz_string(encoded), "trzsz unsupported");
}

#[test]
fn parses_encoded_action_frame_as_json() {
    let action = r#"{"lang":"go","version":"1.1.8","protocol":4,"binary":true}"#;
    let line = format!("#ACT:{}\n", encode_trzsz_string(action.as_bytes()));
    let frame = parse_trzsz_protocol_frame(line.as_bytes()).expect("frame");

    assert_eq!(frame.frame_type, "ACT");
    assert_eq!(
        frame.payload,
        TrzszProtocolPayload::EncodedBytes(action.as_bytes().to_vec())
    );
    let json = parse_trzsz_json_frame(&frame).expect("json");
    assert_eq!(json["lang"], "go");
    assert_eq!(json["protocol"], 4);
    assert_eq!(json["binary"], true);
}

#[test]
fn builds_and_parses_local_action_frame() {
    let action = TrzszAction::local_default(true);
    let frame_bytes = build_trzsz_action_frame(&action, true);

    assert!(frame_bytes.starts_with(b"#ACT:"));
    assert!(frame_bytes.ends_with(b"!\n"));
    let frame = parse_trzsz_protocol_frame(&frame_bytes).expect("action frame");
    let parsed = parse_trzsz_action_frame(&frame).expect("typed action");

    assert_eq!(parsed.lang, "rust");
    assert_eq!(parsed.protocol, Some(4));
    assert!(parsed.confirm);
    assert!(parsed.support_binary);
    assert!(parsed.support_directory);
    assert_eq!(parsed.newline.as_deref(), Some("!\n"));
}

#[test]
fn builds_and_parses_local_config_frame() {
    let action = TrzszAction::local_default(false);
    let config = TrzszConfig::local_default(Some(&action), true);
    let frame_bytes = build_trzsz_config_frame(&config, false);

    assert!(frame_bytes.starts_with(b"#CFG:"));
    assert!(frame_bytes.ends_with(b"\n"));
    assert!(!frame_bytes.ends_with(b"!\n"));
    let frame = parse_trzsz_protocol_frame(&frame_bytes).expect("config frame");
    let parsed = parse_trzsz_config_frame(&frame).expect("typed config");

    assert!(parsed.binary);
    assert!(parsed.directory);
    assert_eq!(parsed.protocol, Some(4));
    assert_eq!(parsed.timeout, Some(20));
    assert_eq!(parsed.max_buf_size, Some(10 * 1024 * 1024));
}

#[test]
fn builds_integer_and_string_protocol_frames() {
    let integer = build_trzsz_integer_frame("SUCC", 4096, "\n");
    assert_eq!(
        parse_trzsz_protocol_frame(&integer).expect("integer"),
        TrzszProtocolFrame {
            frame_type: "SUCC".to_string(),
            payload: TrzszProtocolPayload::Integer(4096),
        }
    );

    let string = build_trzsz_string_frame("SUCC", b"ok", "\n");
    assert_eq!(
        parse_trzsz_protocol_frame(&string).expect("string").payload,
        TrzszProtocolPayload::EncodedBytes(b"ok".to_vec())
    );
}

#[test]
fn parses_binary_data_header_as_integer() {
    let frame = parse_trzsz_protocol_frame(b"#DATA:4096\n").expect("frame");

    assert_eq!(
        frame,
        TrzszProtocolFrame {
            frame_type: "DATA".to_string(),
            payload: TrzszProtocolPayload::Integer(4096),
        }
    );
}

#[test]
fn parses_windows_fail_frame_and_decodes_message() {
    let response = trzsz_fail_response("trzsz unsupported", true);
    let frame = parse_trzsz_protocol_frame(&response).expect("frame");

    assert_eq!(frame.frame_type, "fail");
    assert_eq!(
        frame.payload,
        TrzszProtocolPayload::EncodedBytes(b"trzsz unsupported".to_vec())
    );
}

#[test]
fn rejects_non_protocol_lines_and_keeps_unknown_payload_raw() {
    assert!(parse_trzsz_protocol_frame(b"plain output\n").is_none());
    assert!(parse_trzsz_protocol_frame(b"#BAD\n").is_none());

    let frame = parse_trzsz_protocol_frame(b"#META:not-base64\n").expect("frame");
    assert_eq!(frame.frame_type, "META");
    assert_eq!(
        frame.payload,
        TrzszProtocolPayload::Raw("not-base64".to_string())
    );
}

#[test]
fn rejects_encoded_payloads_that_expand_past_limit() {
    let payload = vec![0; TRZSZ_MAX_DECODED_PAYLOAD_BYTES + 1];
    let line = format!("#META:{}\n", encode_trzsz_string(&payload));
    let frame = parse_trzsz_protocol_frame(line.as_bytes()).expect("frame");

    assert!(matches!(frame.payload, TrzszProtocolPayload::Raw(_)));
}

#[test]
fn transfer_state_tracks_negotiation_data_and_success() {
    let trigger = TrzszTrigger {
        mode: TrzszMode::Receive,
        version: "1.1.8".to_string(),
        unique_id: Some("1700000000000".to_string()),
        remote_is_windows: false,
        tunnel_port: None,
        raw: b"::TRZSZ:TRANSFER:R:1.1.8:1700000000000".to_vec(),
    };
    let mut state = TrzszTransferState::new();

    assert_eq!(
        state.observe_trigger(&trigger),
        TrzszTransferEvent::Started {
            mode: TrzszMode::Receive,
            remote_is_windows: false,
        }
    );
    assert_eq!(state.phase, TrzszTransferPhase::Triggered);

    let action = r#"{"lang":"go","version":"1.1.8","protocol":4,"binary":true}"#;
    let frame = parse_trzsz_protocol_frame(
        format!("#ACT:{}\n", encode_trzsz_string(action.as_bytes())).as_bytes(),
    )
    .expect("act");
    match state.observe_frame(frame) {
        TrzszTransferEvent::Action { action } => {
            assert_eq!(action.protocol, Some(4));
            assert!(state.action.as_ref().unwrap().support_binary);
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert_eq!(state.phase, TrzszTransferPhase::ActionNegotiated);

    let config = r#"{"lang":"go","binary":true,"bufsize":1048576}"#;
    let frame = parse_trzsz_protocol_frame(
        format!("#CFG:{}\n", encode_trzsz_string(config.as_bytes())).as_bytes(),
    )
    .expect("cfg");
    match state.observe_frame(frame) {
        TrzszTransferEvent::Config { config } => {
            assert_eq!(config.max_buf_size, Some(1048576));
            assert!(state.config.as_ref().unwrap().binary);
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert_eq!(state.phase, TrzszTransferPhase::Configured);

    let frame = parse_trzsz_protocol_frame(b"#NUM:1\n").expect("num");
    assert_eq!(
        state.observe_frame(frame),
        TrzszTransferEvent::Metadata {
            frame_type: "NUM".to_string(),
            payload: TrzszProtocolPayload::Integer(1),
        }
    );
    assert_eq!(state.phase, TrzszTransferPhase::Transferring);

    let frame = parse_trzsz_protocol_frame(b"#DATA:4096\n").expect("data");
    assert_eq!(
        state.observe_frame(frame),
        TrzszTransferEvent::Data {
            payload: TrzszProtocolPayload::Integer(4096),
        }
    );

    let frame =
        parse_trzsz_protocol_frame(format!("#SUCC:{}\n", encode_trzsz_string(b"ok")).as_bytes())
            .expect("succ");
    assert_eq!(
        state.observe_frame(frame),
        TrzszTransferEvent::Success {
            payload: TrzszProtocolPayload::EncodedBytes(b"ok".to_vec()),
        }
    );
    assert_eq!(state.phase, TrzszTransferPhase::Completed);
}

#[test]
fn transfer_state_tracks_failure_and_exit_messages() {
    let mut state = TrzszTransferState::new();

    let fail =
        parse_trzsz_protocol_frame(&trzsz_fail_response("permission denied", true)).expect("fail");
    assert_eq!(
        state.observe_frame(fail),
        TrzszTransferEvent::Failure {
            message: "permission denied".to_string(),
        }
    );
    assert_eq!(state.phase, TrzszTransferPhase::Failed);

    let exit = parse_trzsz_protocol_frame(
        format!("#EXIT:{}\n", encode_trzsz_string(b"user cancelled")).as_bytes(),
    )
    .expect("exit");
    assert_eq!(
        state.observe_frame(exit),
        TrzszTransferEvent::Exit {
            message: "user cancelled".to_string(),
        }
    );
    assert_eq!(state.phase, TrzszTransferPhase::Failed);
}

#[test]
fn download_engine_receives_binary_file_and_generates_acks() {
    let mut engine = TrzszDownloadEngine::new(false);
    let digest = md5::compute(b"hello").0.to_vec();

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").expect("num"))
        .expect("num step");
    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::FileCount { count: 1 }]
    );
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(1)
    );

    let name = build_trzsz_string_frame("NAME", b"hello.txt", "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&name).expect("name"))
        .expect("name step");
    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::FileName {
            name: "hello.txt".to_string()
        }]
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SIZE:5\n").expect("size"))
        .expect("size step");
    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::FileSize {
            name: "hello.txt".to_string(),
            size: 5,
        }]
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#DATA:5\n").expect("data header"))
        .expect("data header step");
    assert!(step.events.is_empty());
    assert!(step.responses.is_empty());

    let data = build_trzsz_string_frame("DATA", b"hello", "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&data).expect("data body"))
        .expect("data body step");
    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::Data {
            name: "hello.txt".to_string(),
            bytes: b"hello".to_vec(),
            received: 5,
            size: 5,
        }]
    );
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(5)
    );

    let md5 = build_trzsz_string_frame("MD5", &digest, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&md5).expect("md5"))
        .expect("md5 step");
    assert_eq!(
        step.events,
        vec![
            TrzszDownloadEvent::FileFinished {
                name: "hello.txt".to_string(),
                digest: digest.clone(),
            },
            TrzszDownloadEvent::Completed {
                names: vec!["hello.txt".to_string()]
            }
        ]
    );
    assert!(engine.is_completed());
}

#[test]
fn download_engine_requests_zero_size_for_modern_protocol() {
    let mut engine = TrzszDownloadEngine::new(false);
    engine.set_protocol_version(4);

    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
        .unwrap();
    let name = build_trzsz_string_frame(
        "NAME",
        br#"{"path_id":0,"path_name":["anaconda-ks.cfg"],"is_dir":false,"archive":false,"size":1088,"perm":null}"#,
        "\n",
    );
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
        .unwrap();

    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::FileName {
            name: "anaconda-ks.cfg".to_string()
        }]
    );
    let ack = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(
        bytes_payload(&ack).unwrap(),
        br#"{"name":"anaconda-ks.cfg","size":0}"#.to_vec()
    );
}

#[test]
fn download_engine_accepts_empty_file_without_data_frame() {
    let mut engine = TrzszDownloadEngine::new(true);
    let digest = md5::compute(b"").0.to_vec();

    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
        .unwrap();
    let name = build_trzsz_string_frame("NAME", b"empty.txt", "!\n");
    engine
        .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
        .unwrap();
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SIZE:0!\n").unwrap())
        .unwrap();

    let md5 = build_trzsz_string_frame("MD5", &digest, "!\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&md5).unwrap())
        .unwrap();

    assert_eq!(
        step.events.last(),
        Some(&TrzszDownloadEvent::Completed {
            names: vec!["empty.txt".to_string()]
        })
    );
    assert!(step.responses[0].ends_with(b"!\n"));
}

#[test]
fn download_engine_accepts_directory_entries_without_size() {
    let mut engine = TrzszDownloadEngine::new(false);
    engine.set_directory_mode(true);
    engine.set_protocol_version(4);

    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
        .unwrap();
    let name = build_trzsz_string_frame(
        "NAME",
        br#"{"path_id":7,"path_name":["logs","2026"],"is_dir":true}"#,
        "\n",
    );
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
        .unwrap();

    assert_eq!(
        step.events,
        vec![
            TrzszDownloadEvent::Directory {
                name: "2026".to_string(),
                path_id: 7,
                components: vec!["logs".to_string(), "2026".to_string()],
            },
            TrzszDownloadEvent::Completed {
                names: vec!["logs".to_string()]
            }
        ]
    );
    let ack = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(
        bytes_payload(&ack).unwrap(),
        br#"{"name":"logs","size":0}"#.to_vec()
    );
    assert!(engine.is_completed());
}

#[test]
fn download_engine_receives_directory_file_metadata() {
    let mut engine = TrzszDownloadEngine::new(false);
    engine.set_directory_mode(true);
    engine.set_protocol_version(4);
    let digest = md5::compute(b"abc").0.to_vec();

    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
        .unwrap();
    let name = build_trzsz_string_frame(
        "NAME",
        br#"{"path_id":3,"path_name":["project","src","main.rs"],"size":3}"#,
        "\n",
    );
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
        .unwrap();
    assert_eq!(
        step.events,
        vec![
            TrzszDownloadEvent::FilePath {
                name: "main.rs".to_string(),
                path_id: 3,
                components: vec![
                    "project".to_string(),
                    "src".to_string(),
                    "main.rs".to_string()
                ],
            },
            TrzszDownloadEvent::FileName {
                name: "main.rs".to_string()
            }
        ]
    );
    let ack = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(
        bytes_payload(&ack).unwrap(),
        br#"{"name":"project","size":0}"#.to_vec()
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SIZE:3\n").unwrap())
        .unwrap();
    assert_eq!(
        step.events,
        vec![TrzszDownloadEvent::FileSize {
            name: "main.rs".to_string(),
            size: 3,
        }]
    );
    let data = TrzszProtocolFrame {
        frame_type: "DATA".into(),
        payload: TrzszProtocolPayload::Raw("YWJj".into()),
    };
    engine.observe_frame(data).unwrap();
    engine
        .observe_frame(TrzszProtocolFrame {
            frame_type: "DATA".into(),
            payload: TrzszProtocolPayload::Raw(String::new()),
        })
        .unwrap();
    let md5 = build_trzsz_string_frame("MD5", &digest, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&md5).unwrap())
        .unwrap();
    assert_eq!(
        step.events.last(),
        Some(&TrzszDownloadEvent::Completed {
            names: vec!["project".to_string()]
        })
    );
}

#[test]
fn download_engine_rejects_binary_chunk_length_mismatch() {
    let mut engine = TrzszDownloadEngine::new(false);
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#NUM:1\n").unwrap())
        .unwrap();
    let name = build_trzsz_string_frame("NAME", b"bad.bin", "\n");
    engine
        .observe_frame(parse_trzsz_protocol_frame(&name).unwrap())
        .unwrap();
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SIZE:5\n").unwrap())
        .unwrap();
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#DATA:5\n").unwrap())
        .unwrap();

    let data = build_trzsz_string_frame("DATA", b"nope", "\n");
    let error = engine
        .observe_frame(parse_trzsz_protocol_frame(&data).unwrap())
        .expect_err("length mismatch");

    assert_eq!(
        error,
        TrzszDownloadError::DataLengthMismatch {
            expected: 5,
            actual: 4,
        }
    );
}

#[test]
fn upload_engine_sends_regular_file_after_acks() {
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![TrzszUploadEntry::from_bytes("hello.txt", b"hello".to_vec())],
    );
    let digest = md5::compute(b"hello").0.to_vec();

    let step = engine.begin().expect("begin");
    assert_eq!(step.events, vec![TrzszUploadEvent::Started { count: 1 }]);
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(1)
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:1\n").unwrap())
        .expect("num ack");
    let name = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(name.frame_type, "NAME");
    assert_eq!(bytes_payload(&name).unwrap(), b"hello.txt".to_vec());

    let remote_name = build_trzsz_string_frame("SUCC", b"hello.txt", "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_name).unwrap())
        .expect("name ack");
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::FileStarted {
            name: "hello.txt".to_string(),
            remote_name: "hello.txt".to_string(),
            size: 5,
        }]
    );
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(5)
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:5\n").unwrap())
        .expect("size ack");
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::Data {
            name: "hello.txt".to_string(),
            sent: 5,
            size: 5,
        }]
    );
    let data = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(data.frame_type, "DATA");
    assert_eq!(bytes_payload(&data).unwrap(), b"hello".to_vec());

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:5\n").unwrap())
        .expect("data ack");
    let md5_frame = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(md5_frame.frame_type, "MD5");
    assert_eq!(bytes_payload(&md5_frame).unwrap(), digest);

    let md5_ack = build_trzsz_string_frame("SUCC", &digest, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&md5_ack).unwrap())
        .expect("md5 ack");
    assert_eq!(
        step.events,
        vec![
            TrzszUploadEvent::FileFinished {
                name: "hello.txt".to_string(),
                digest,
            },
            TrzszUploadEvent::Completed {
                names: vec!["hello.txt".to_string()]
            }
        ]
    );
    assert!(engine.is_completed());
}

#[test]
fn upload_engine_sends_json_source_name_for_modern_protocol() {
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![TrzszUploadEntry::from_bytes("hello.txt", b"hello".to_vec())],
    );
    engine.set_protocol_version(4);

    engine.begin().expect("begin");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:1\n").unwrap())
        .expect("num ack");
    let name = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(name.frame_type, "NAME");
    let source: serde_json::Value =
        serde_json::from_slice(&bytes_payload(&name).unwrap()).expect("source json");
    assert_eq!(source["path_id"], 0);
    assert_eq!(source["path_name"], serde_json::json!(["hello.txt"]));
    assert_eq!(source["is_dir"], false);
    assert_eq!(source["size"], 5);

    let remote_name = build_trzsz_string_frame("SUCC", br#"{"name":"hello.txt","size":0}"#, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_name).unwrap())
        .expect("name ack");
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::FileStarted {
            name: "hello.txt".to_string(),
            remote_name: "hello.txt".to_string(),
            size: 5,
        }]
    );
}

#[test]
fn upload_engine_streams_file_data_in_chunks() {
    let mut payload = vec![b'a'; TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 13];
    payload[TRZSZ_UPLOAD_DATA_CHUNK_SIZE..].copy_from_slice(b"tail-payload!");
    let digest = md5::compute(&payload).0.to_vec();
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![TrzszUploadEntry::from_bytes("large.bin", payload)],
    );

    engine.begin().unwrap();
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:1\n").unwrap())
        .unwrap();
    let remote_name = build_trzsz_string_frame("SUCC", b"large.bin", "\n");
    engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_name).unwrap())
        .unwrap();

    let step = engine
        .observe_frame(
            parse_trzsz_protocol_frame(
                format!("#SUCC:{}\n", TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 13).as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
    let first = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(first.frame_type, "DATA");
    assert_eq!(
        bytes_payload(&first).unwrap().len(),
        TRZSZ_UPLOAD_DATA_CHUNK_SIZE
    );
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::Data {
            name: "large.bin".to_string(),
            sent: TRZSZ_UPLOAD_DATA_CHUNK_SIZE as i64,
            size: (TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 13) as i64,
        }]
    );

    let step = engine
        .observe_frame(
            parse_trzsz_protocol_frame(
                format!("#SUCC:{TRZSZ_UPLOAD_DATA_CHUNK_SIZE}\n").as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
    let second = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(bytes_payload(&second).unwrap(), b"tail-payload!".to_vec());
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::Data {
            name: "large.bin".to_string(),
            sent: (TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 13) as i64,
            size: (TRZSZ_UPLOAD_DATA_CHUNK_SIZE + 13) as i64,
        }]
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:13\n").unwrap())
        .unwrap();
    let md5_frame = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(md5_frame.frame_type, "MD5");
    assert_eq!(bytes_payload(&md5_frame).unwrap(), digest);
}

#[test]
fn upload_engine_handles_empty_file_and_windows_newlines() {
    let mut engine = TrzszUploadEngine::new(
        true,
        vec![TrzszUploadEntry::from_bytes("empty.txt", Vec::new())],
    );
    let digest = md5::compute(b"").0.to_vec();

    let step = engine.begin().unwrap();
    assert!(step.responses[0].ends_with(b"!\n"));
    engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:1!\n").unwrap())
        .unwrap();
    let remote_name = build_trzsz_string_frame("SUCC", b"empty.txt", "!\n");
    engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_name).unwrap())
        .unwrap();
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:0!\n").unwrap())
        .unwrap();

    let md5_frame = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(bytes_payload(&md5_frame).unwrap(), digest.clone());
    assert!(step.responses[0].ends_with(b"!\n"));

    let md5_ack = build_trzsz_string_frame("SUCC", &digest, "!\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&md5_ack).unwrap())
        .unwrap();
    assert_eq!(
        step.events.last(),
        Some(&TrzszUploadEvent::Completed {
            names: vec!["empty.txt".to_string()]
        })
    );
}

#[test]
fn upload_engine_rejects_mismatched_ack() {
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![TrzszUploadEntry::from_bytes("hello.txt", b"hello".to_vec())],
    );
    engine.begin().unwrap();
    let error = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:2\n").unwrap())
        .expect_err("mismatch");
    assert_eq!(
        error,
        TrzszUploadError::AckMismatch {
            expected: TrzszProtocolPayload::Integer(1),
            actual: TrzszProtocolPayload::Integer(2),
        }
    );
}

#[test]
fn upload_engine_sends_directory_entries_without_size() {
    let mut engine = TrzszUploadEngine::new(
        false,
        vec![
            TrzszUploadEntry {
                name: "folder".to_string(),
                size: 0,
                payload: TrzszUploadPayload::Memory(Vec::new()),
                source: Some(TrzszUploadSource {
                    archive: false,
                    path_id: 0,
                    path_name: vec!["folder".to_string()],
                    is_dir: true,
                    size: 0,
                    perm: Some(0o755),
                }),
            },
            {
                let mut entry = TrzszUploadEntry::from_bytes("note.txt", b"note".to_vec());
                entry.source = Some(TrzszUploadSource {
                    archive: false,
                    path_id: 0,
                    path_name: vec!["folder".to_string(), "note.txt".to_string()],
                    is_dir: false,
                    size: 4,
                    perm: Some(0o644),
                });
                entry
            },
        ],
    );
    engine.set_protocol_version(4);

    let step = engine.begin().expect("begin");
    assert_eq!(step.events, vec![TrzszUploadEvent::Started { count: 2 }]);
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(2)
    );

    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(b"#SUCC:2\n").unwrap())
        .expect("num ack");
    let dir_name = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    let source: serde_json::Value =
        serde_json::from_slice(&bytes_payload(&dir_name).unwrap()).unwrap();
    assert_eq!(source["path_id"], 0);
    assert_eq!(source["path_name"][0], "folder");
    assert_eq!(source["is_dir"], true);

    let remote_dir = build_trzsz_string_frame("SUCC", br#"{"name":"folder","size":0}"#, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_dir).unwrap())
        .expect("dir ack");
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::Directory {
            name: "folder".to_string(),
            remote_name: "folder".to_string(),
        }]
    );
    let file_name = parse_trzsz_protocol_frame(&step.responses[0]).unwrap();
    assert_eq!(file_name.frame_type, "NAME");
    let source: serde_json::Value =
        serde_json::from_slice(&bytes_payload(&file_name).unwrap()).unwrap();
    assert_eq!(source["path_name"][0], "folder");
    assert_eq!(source["path_name"][1], "note.txt");
    assert_eq!(source["is_dir"], false);

    let remote_file = build_trzsz_string_frame("SUCC", br#"{"name":"note.txt","size":0}"#, "\n");
    let step = engine
        .observe_frame(parse_trzsz_protocol_frame(&remote_file).unwrap())
        .expect("file ack");
    assert_eq!(
        step.events,
        vec![TrzszUploadEvent::FileStarted {
            name: "note.txt".to_string(),
            remote_name: "note.txt".to_string(),
            size: 4,
        }]
    );
    assert_eq!(
        parse_trzsz_protocol_frame(&step.responses[0])
            .unwrap()
            .payload,
        TrzszProtocolPayload::Integer(4)
    );
}

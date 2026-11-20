use super::{
    SendCommandLineEnding, SendCommandMode, SendCommandPlan, TextInterpretation,
    format_interval_ms, parse_interval_ms,
};

#[test]
fn cached_statistics_match_plain_delivery_without_allocating_send_units_in_views() {
    for text in [
        "",
        "one",
        "one\n",
        "\n\n",
        "\none\n\n",
        "中\r\n\tlast",
        "one\rtwo\n\nthree",
    ] {
        for mode in [
            SendCommandMode::Line,
            SendCommandMode::Character,
            SendCommandMode::Packet,
            SendCommandMode::Byte,
        ] {
            for ending in [
                SendCommandLineEnding::None,
                SendCommandLineEnding::Cr,
                SendCommandLineEnding::Lf,
                SendCommandLineEnding::Crlf,
            ] {
                let actual: Vec<_> = SendCommandPlan::text(
                    text,
                    mode,
                    ending,
                    TextInterpretation::Plain,
                    Duration::ZERO,
                )
                .collect();
                let stats = super::TextSendStatistics::from_text(text).for_delivery(mode, ending);
                assert_eq!(stats.units, actual.len(), "{text:?}, {mode:?}, {ending:?}");
                assert_eq!(
                    stats.bytes,
                    actual.iter().map(|chunk| chunk.bytes.len()).sum::<usize>()
                );
            }
        }
    }
}
use std::time::Duration;

#[test]
fn literal_segmentation_preserves_every_byte_and_does_not_add_a_submit() {
    for text in [
        "abc\ndef",
        "abc\rdef",
        "abc\r\ndef",
        "\r\n\n\r",
        "中\\p{{变量}}",
        "abc\n",
        "",
    ] {
        for mode in [SendCommandMode::Line, SendCommandMode::Character] {
            let chunks: Vec<_> = SendCommandPlan::text(
                text,
                mode,
                SendCommandLineEnding::Crlf,
                TextInterpretation::Literal,
                Duration::from_millis(15),
            )
            .collect();
            assert_eq!(
                chunks
                    .into_iter()
                    .flat_map(|chunk| chunk.bytes)
                    .collect::<Vec<_>>(),
                text.as_bytes()
            );
        }
    }
}

#[test]
fn line_delivery_uses_target_ending_without_a_phantom_trailing_line() {
    for (ending, expected) in [
        (SendCommandLineEnding::Cr, b"one\r\rthree\r".as_slice()),
        (
            SendCommandLineEnding::Crlf,
            b"one\r\n\r\nthree\r\n".as_slice(),
        ),
    ] {
        let chunks: Vec<_> = SendCommandPlan::text(
            "one\n\nthree\n",
            SendCommandMode::Line,
            ending,
            TextInterpretation::Plain,
            Duration::from_millis(250),
        )
        .collect();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].delay, Duration::ZERO);
        assert!(
            chunks[1..]
                .iter()
                .all(|chunk| chunk.delay == Duration::from_millis(250))
        );
        assert_eq!(
            chunks
                .into_iter()
                .flat_map(|chunk| chunk.bytes)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn script_pauses_are_additive_and_explicit_controls_are_not_rewritten() {
    let chunks: Vec<_> = SendCommandPlan::text(
        "one\\r\\p\ntwo\\p\\pnext",
        SendCommandMode::Line,
        SendCommandLineEnding::Crlf,
        TextInterpretation::Script,
        Duration::from_millis(200),
    )
    .collect();
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.bytes.as_slice())
            .collect::<Vec<_>>(),
        [b"one\r".as_slice(), b"two", b"next\r\n"]
    );
    assert_eq!(chunks[1].delay, Duration::from_millis(1200));
    assert_eq!(chunks[2].delay, Duration::from_millis(2200));
}

#[test]
fn character_delivery_keeps_unicode_and_does_not_submit_the_last_line() {
    let chunks: Vec<_> = SendCommandPlan::text(
        "中a\r\nb",
        SendCommandMode::Character,
        SendCommandLineEnding::Crlf,
        TextInterpretation::Plain,
        Duration::ZERO,
    )
    .collect();
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.bytes.as_slice())
            .collect::<Vec<_>>(),
        ["中".as_bytes(), b"a", b"\r\n", b"b"]
    );
}

#[test]
fn zero_interval_batches_are_bounded_and_keep_pause_barriers() {
    let text = "中".repeat(20_000);
    let mut plan = SendCommandPlan::text(
        &text,
        SendCommandMode::Character,
        SendCommandLineEnding::Cr,
        TextInterpretation::Literal,
        Duration::ZERO,
    );
    let mut sent = Vec::new();
    let mut units = 0;
    while let Some(batch) = plan.next_batch(16 * 1024) {
        assert!(batch.bytes.len() <= 16 * 1024);
        assert!(std::str::from_utf8(&batch.bytes).is_ok());
        sent.extend(batch.bytes);
        units += batch.units;
    }
    assert_eq!(sent, text.as_bytes());
    assert_eq!(units, 20_000);
    let mut plan = SendCommandPlan::text(
        "before\\pafter",
        SendCommandMode::Character,
        SendCommandLineEnding::Cr,
        TextInterpretation::Script,
        Duration::ZERO,
    );
    assert_eq!(plan.next_batch(16 * 1024).unwrap().bytes, b"before");
    let after = plan.next_batch(16 * 1024).unwrap();
    assert_eq!(after.bytes, b"after");
    assert_eq!(after.delay, Duration::from_secs(1));
}

#[test]
fn large_line_and_hex_batches_do_not_duplicate_or_drop_data() {
    let text = "字".repeat(10_000);
    let mut plan = SendCommandPlan::text(
        &text,
        SendCommandMode::Line,
        SendCommandLineEnding::Cr,
        TextInterpretation::Literal,
        Duration::from_millis(100),
    );
    let mut sent = Vec::new();
    let mut units = 0;
    while let Some(batch) = plan.next_batch(1024) {
        assert_eq!(batch.delay, Duration::ZERO);
        units += batch.units;
        sent.extend(batch.bytes);
    }
    assert_eq!(sent, text.as_bytes());
    assert_eq!(units, 1);
    let mut hex = SendCommandPlan::hex("00 FF 0A", SendCommandMode::Byte, Duration::ZERO).unwrap();
    assert_eq!(hex.next_batch(16).unwrap().bytes, [0, 255, 10]);
}

#[test]
fn interval_editor_round_trips_every_millisecond_without_rounding() {
    for value in 0..=60_000 {
        assert_eq!(parse_interval_ms(&format_interval_ms(value)), Some(value));
    }
    for value in ["NaN", "inf", "-1", "60.001", "0.0001", "1.2.3", "", "+1"] {
        assert_eq!(parse_interval_ms(value), None);
    }
}

#[test]
fn newline_normalization_does_not_turn_a_real_blank_line_into_script_layout() {
    let plan = SendCommandPlan::text(
        "one\n\\p\nnext",
        SendCommandMode::Line,
        SendCommandLineEnding::Cr,
        TextInterpretation::Script,
        Duration::ZERO,
    );
    assert_eq!(
        plan.flat_map(|chunk| chunk.bytes).collect::<Vec<_>>(),
        b"one\r\rnext\r"
    );
}

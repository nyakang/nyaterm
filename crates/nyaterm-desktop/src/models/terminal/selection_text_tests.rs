use futures::executor::block_on;
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal::selection::{TerminalBufferCellPos, TerminalSelection};

use super::{TerminalFramePipeline, TerminalSelectionTextError};
use crate::models::recording::RecordingWritePipeline;

#[test]
fn worker_extracts_a_range_larger_than_any_paint_snapshot() {
    let mut recording = RecordingWritePipeline::spawn(4096);
    let mut pipeline = TerminalFramePipeline::spawn(recording.writer());
    let output = (0..600)
        .map(|i| format!("line {i:04}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    pipeline.seed_session("s1", &output, "UTF-8", 1000);
    pipeline.flush_for_test();
    let frame = pipeline.take_session_for_transfer("s1").unwrap().unwrap();
    let history = frame.screen.history_state();
    pipeline
        .insert_sessions_from_transfer(vec![("s1".into(), frame)])
        .unwrap();
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(0, 0),
        TerminalBufferCellPos::new(499, 8),
    );
    let text = block_on(pipeline.request_selection_text("s1".into(), selection, history))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        text,
        (0..500)
            .map(|i| format!("line {i:04}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(
        block_on(pipeline.request_selection_text("missing".into(), selection, history)).unwrap(),
        Err(TerminalSelectionTextError::MissingSession)
    );
    pipeline.shutdown();
    recording.shutdown();
}

#[test]
fn worker_accepts_growth_but_rejects_lost_ranges_reset_reflow_and_screen_switch() {
    let mut recording = RecordingWritePipeline::spawn(4096);
    let mut pipeline = TerminalFramePipeline::spawn(recording.writer());
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(0, 0),
        TerminalBufferCellPos::new(0, 2),
    );
    for (id, change) in [("evict", 0), ("reset", 1), ("reflow", 2), ("alternate", 3)] {
        pipeline.seed_session(id, "abc\r\n", "UTF-8", 10);
        pipeline.flush_for_test();
        let mut frame = pipeline.take_session_for_transfer(id).unwrap().unwrap();
        let history = frame.screen.history_state();
        // Ordinary scrolling before the buffer fills keeps absolute row keys valid.
        frame.screen.advance(b"more\r\n");
        pipeline
            .insert_sessions_from_transfer(vec![(id.into(), frame)])
            .unwrap();
        assert_eq!(
            block_on(pipeline.request_selection_text(id.into(), selection, history))
                .unwrap()
                .unwrap()
                .as_deref(),
            Some("abc")
        );
        let mut frame = pipeline.take_session_for_transfer(id).unwrap().unwrap();
        match change {
            0 => frame.screen.advance("x\r\n".repeat(100).as_bytes()),
            1 => frame.screen.advance(b"\x1bc"),
            2 => frame.screen.resize(40, 24),
            _ => frame.screen.advance(b"\x1b[?1049h"),
        }
        pipeline
            .insert_sessions_from_transfer(vec![(id.into(), frame)])
            .unwrap();
        assert_eq!(
            block_on(pipeline.request_selection_text(id.into(), selection, history)).unwrap(),
            Err(if change == 0 {
                TerminalSelectionTextError::InvalidRange
            } else {
                TerminalSelectionTextError::StaleHistory
            })
        );
    }
    // A valid coordinate epoch still must not silently truncate an invalid range.
    let screen = TerminalScreen::default();
    pipeline.ensure_session("empty", "UTF-8", 100);
    let invalid = TerminalSelection::from_range(
        TerminalBufferCellPos::new(0, 0),
        TerminalBufferCellPos::new(1000, 0),
    );
    assert_eq!(
        block_on(pipeline.request_selection_text("empty".into(), invalid, screen.history_state()))
            .unwrap(),
        Err(TerminalSelectionTextError::InvalidRange)
    );
    pipeline.shutdown();
    recording.shutdown();
}

#[test]
fn worker_rebases_retained_ranges_and_clips_only_evicted_text() {
    let mut recording = RecordingWritePipeline::spawn(4096);
    let mut pipeline = TerminalFramePipeline::spawn(recording.writer());
    pipeline.seed_session("s1", "", "UTF-8", 8);
    pipeline.flush_for_test();
    let mut frame = pipeline.take_session_for_transfer("s1").unwrap().unwrap();
    frame.screen = TerminalScreen::new(16, 3);
    frame.screen.set_scrollback_limit(8);
    frame
        .screen
        .advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let history = frame.screen.history_state();
    let expected = frame
        .screen
        .viewport_snapshot(8)
        .line(3 - 1)
        .unwrap()
        .to_string();
    frame.screen.advance(b"11\r\n12\r\n");
    pipeline
        .insert_sessions_from_transfer(vec![("s1".into(), frame)])
        .unwrap();
    // Row 2 survives and becomes row 0; row 1 is actually lost.
    for selection in [
        TerminalSelection::from_range(
            TerminalBufferCellPos::new(1, 0),
            TerminalBufferCellPos::new(2, 15),
        ),
        TerminalSelection::from_range(
            TerminalBufferCellPos::new(2, 15),
            TerminalBufferCellPos::new(1, 0),
        ),
    ] {
        let result =
            block_on(pipeline.request_selection_text_frame("s1".into(), selection, history))
                .unwrap()
                .unwrap();
        assert_eq!(result.text.as_deref(), Some(expected.as_str()));
        assert_eq!(result.selection.ordered().0.line, 0);
        assert_eq!(result.selection.ordered().1.line, 0);
    }
    pipeline.shutdown();
    recording.shutdown();
}

#[test]
fn worker_scroll_request_anchors_its_source_offset_after_history_rotation() {
    let mut recording = RecordingWritePipeline::spawn(4096);
    let mut pipeline = TerminalFramePipeline::spawn(recording.writer());
    pipeline.seed_session("s1", "", "UTF-8", 8);
    pipeline.flush_for_test();
    let mut frame = pipeline.take_session_for_transfer("s1").unwrap().unwrap();
    frame.screen = TerminalScreen::new(16, 3);
    frame.screen.set_scrollback_limit(8);
    frame
        .screen
        .advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let source = frame.screen.history_state();
    let reading = frame
        .screen
        .viewport_snapshot(3)
        .line(0)
        .unwrap()
        .to_string();
    frame.screen.advance(b"11\r\n12\r\n");
    pipeline
        .insert_sessions_from_transfer(vec![("s1".into(), frame)])
        .unwrap();
    pipeline.request_anchored_snapshot(
        "s1".into(),
        3,
        source,
        false,
        nyaterm_core::ActionLinksMatcherSettings::default(),
        true,
    );
    pipeline.flush_for_test();
    let mut events = std::collections::VecDeque::new();
    pipeline.drain_events_into(&mut events, usize::MAX);
    let frame = events
        .into_iter()
        .find_map(|event| match event {
            crate::models::TerminalFrameEvent::Snapshot(frame) if frame.offset > 0 => Some(frame),
            _ => None,
        })
        .unwrap();
    assert_eq!(frame.offset, 5);
    let start = frame.snapshot.history.scrollback_len - frame.offset;
    let window_start =
        frame.snapshot.total_rows - frame.snapshot.display_offset - frame.snapshot.row_count();
    assert_eq!(
        frame.snapshot.line(start - window_start),
        Some(reading.as_str())
    );
    pipeline.shutdown();
    recording.shutdown();
}

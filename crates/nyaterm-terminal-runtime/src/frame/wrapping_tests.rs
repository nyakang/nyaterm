use super::{TerminalFrameEvent, TerminalFramePipeline, TerminalFrameSnapshotEvent};

fn snapshots(pipeline: &TerminalFramePipeline) -> Vec<TerminalFrameSnapshotEvent> {
    pipeline.finish_pending_output().unwrap();
    pipeline
        .take_events_for_transfer(&["s1".into()])
        .into_iter()
        .filter_map(|event| match event {
            TerminalFrameEvent::Snapshot(frame) => Some(frame),
            _ => None,
        })
        .collect()
}

#[test]
fn wrap_changes_publish_idle_frames_without_resizing_or_new_output() {
    let pipeline = TerminalFramePipeline::default();
    pipeline.ensure_session("s1", "UTF-8", 1000);
    pipeline.resize_session("s1", 12, 4);
    pipeline.submit_output(
        "s1",
        "一二三四五六七八九十abcdef\r\nhard break"
            .as_bytes()
            .to_vec(),
        "UTF-8",
        1000,
    );
    pipeline.request_snapshot("s1", 0, false, Default::default());
    let original = snapshots(&pipeline).pop().unwrap();
    let mut previous = pipeline.layout_generation();
    for enabled in [true, false, true, false] {
        let generation = pipeline.set_logical_line_snapshots(enabled);
        assert_ne!(generation, previous);
        let frames = snapshots(&pipeline);
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame.layout_generation, generation);
        assert_eq!(frame.snapshot.logical_lines.is_some(), enabled);
        assert_eq!(frame.snapshot.history, original.snapshot.history);
        assert_eq!(frame.snapshot.cursor, original.snapshot.cursor);
        assert_eq!(pipeline.set_logical_line_snapshots(enabled), generation);
        assert!(
            snapshots(&pipeline).is_empty(),
            "same preference is a no-op"
        );
        previous = generation;
    }
}

#[test]
fn hidden_and_interactive_sessions_keep_the_requested_layout_generation() {
    let pipeline = TerminalFramePipeline::default();
    pipeline.seed_session("s1", "long text", "UTF-8", 1000);
    pipeline.set_snapshot_priority(Vec::new());
    snapshots(&pipeline);
    let generation = pipeline.set_logical_line_snapshots(true);
    assert!(snapshots(&pipeline).is_empty());
    pipeline.request_snapshot("s1", 0, false, Default::default());
    let frame = snapshots(&pipeline).pop().unwrap();
    assert_eq!(frame.layout_generation, generation);
    assert!(frame.snapshot.logical_lines.is_some());
    pipeline.submit_output("s1", b"\x1b[?1049h".to_vec(), "UTF-8", 1000);
    pipeline.request_snapshot("s1", 0, false, Default::default());
    let frame = snapshots(&pipeline).pop().unwrap();
    assert_eq!(frame.layout_generation, generation);
    assert!(
        frame.snapshot.logical_lines.is_none(),
        "interactive grid fallback is not a stale policy"
    );
    pipeline.submit_output("s1", b"\x1b[?1049l".to_vec(), "UTF-8", 1000);
    pipeline.request_snapshot("s1", 0, false, Default::default());
    assert!(
        snapshots(&pipeline)
            .pop()
            .unwrap()
            .snapshot
            .logical_lines
            .is_some()
    );
}

#[test]
fn wrap_generations_survive_reconnect_and_are_replaced_on_transfer() {
    let source = TerminalFramePipeline::default();
    let first = source.set_logical_line_snapshots(true);
    source.seed_session("s1", "text", "UTF-8", 1000);
    assert!(source.rekey_session("s1", "reconnected", "UTF-8", 1000));
    source.request_snapshot("reconnected", 0, false, Default::default());
    source.finish_pending_output().unwrap();
    for event in source.take_events_for_transfer(&["reconnected".into()]) {
        if let TerminalFrameEvent::Snapshot(frame) = event {
            assert_eq!(frame.layout_generation, first);
        }
    }
    let session = source
        .take_session_for_transfer("reconnected")
        .unwrap()
        .unwrap();
    let target = TerminalFramePipeline::default();
    let second = target.set_logical_line_snapshots(true);
    assert_ne!(first, second);
    target
        .insert_sessions_from_transfer(vec![("s1".into(), session)])
        .unwrap();
    target.request_snapshot("s1", 0, false, Default::default());
    assert_eq!(snapshots(&target).pop().unwrap().layout_generation, second);
}

#[test]
fn priority_history_requests_cannot_overtake_a_wrap_change() {
    use super::frame_commands::{TerminalFrameCommand, push_terminal_frame_command};
    use std::collections::VecDeque;

    let mut commands = VecDeque::from([
        TerminalFrameCommand::Output {
            session_id: "s1".into(),
            data: b"before".to_vec(),
            encoding: "UTF-8".into(),
            scrollback_limit: 1000,
        },
        TerminalFrameCommand::SetLogicalLineSnapshots {
            enabled: true,
            generation: Default::default(),
        },
    ]);
    push_terminal_frame_command(
        &mut commands,
        TerminalFrameCommand::RequestSnapshot {
            session_id: "s1".into(),
            offset: 80,
            source_history: None,
            action_links_enabled: false,
            action_link_matchers: Default::default(),
            priority: true,
            purpose: super::TerminalFrameSnapshotPurpose::Paint,
        },
    );
    assert!(matches!(
        commands.pop_front(),
        Some(TerminalFrameCommand::Output { .. })
    ));
    assert!(matches!(
        commands.pop_front(),
        Some(TerminalFrameCommand::SetLogicalLineSnapshots { .. })
    ));
    assert!(matches!(
        commands.pop_front(),
        Some(TerminalFrameCommand::RequestSnapshot { .. })
    ));
}

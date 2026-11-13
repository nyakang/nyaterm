use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{FINISH_WAIT, Shared, ZmodemRuntime, strip_finish_trailer, strip_finish_trailer_with};
use crate::zmodem::{
    ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer, ZmodemTransferOptions,
};
use crate::{
    ManagedSession, SessionEvent, SessionInfo, SessionKind, SessionManager, SshChannelTransport,
    SshCommand, SshSessionConfig, ssh_mailbox,
};

fn output(data: &[u8]) -> SessionEvent {
    SessionEvent::Output {
        session_id: "test".into(),
        data: data.to_vec(),
    }
}

fn output_bytes(events: impl IntoIterator<Item = SessionEvent>) -> Vec<u8> {
    events
        .into_iter()
        .flat_map(|event| match event {
            SessionEvent::Output { data, .. } => data,
            _ => panic!("expected output"),
        })
        .collect()
}

fn finish_line_endings() -> Vec<Vec<u8>> {
    let mut endings = Vec::new();
    for lf in [b'\n', 0x8a] {
        for cr in [None, Some(b'\r'), Some(0x8d)] {
            for xon in [None, Some(0x11), Some(0x91)] {
                let mut ending = Vec::new();
                ending.extend(cr);
                ending.push(lf);
                ending.extend(xon);
                endings.push(ending);
            }
        }
    }
    endings
}

fn mock_session() -> (Arc<SessionManager>, ssh_mailbox::Receiver) {
    let manager = Arc::new(SessionManager::new());
    let (command_tx, receiver) = ssh_mailbox::channel();
    manager.sessions.lock().unwrap().insert(
        "test".into(),
        ManagedSession::Ssh(SshChannelTransport {
            shell_enabled: true,
            info: SessionInfo {
                id: "test".into(),
                name: "test".into(),
                kind: SessionKind::Ssh,
                working_dir: None,
                cols: 80,
                rows: 24,
            },
            attempt: SshSessionConfig::default().attempt,
            command_tx,
            tmux_tx: crate::tmux::mailbox::channel().0,
            tmux_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            backspace_as_bs: false,
            worker_thread: None,
        }),
    );
    (manager, receiver)
}

fn wait_done(runtime: &ZmodemRuntime) -> super::ZmodemRuntimeEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = runtime.try_recv_event()
            && event.done
        {
            return event;
        }
        assert!(Instant::now() < deadline, "runtime did not stop");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn picker_does_not_dequeue_input_and_cancel_bypasses_accumulated_data() {
    let (manager, _receiver) = mock_session();
    let runtime = ZmodemRuntime::spawn(
        manager.clone(),
        "test".into(),
        ZmodemTransfer::new(ZmodemDirection::Download, &[]),
    )
    .unwrap();
    manager.assign_session_event_consumer("test", runtime.consumer_id());
    runtime.activate(VecDeque::from([output(b"bootstrap")]));
    manager.event_queue.push(output(&vec![1; 8 * 1024 * 1024]));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(runtime.stats().input_bytes, 0);
    assert_eq!(runtime.stats().source_queue_peak_bytes, 8 * 1024 * 1024);
    runtime.cancel("picker cancelled");
    let done = wait_done(&runtime);
    assert!(done.actions.iter().any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) if reason == "picker cancelled")));
    assert!(
        matches!(&done.pending_events[0], SessionEvent::Output { data, .. } if data == b"bootstrap")
    );
    assert_eq!(
        manager.event_queue.drain(100).stats.drained_output_bytes,
        8 * 1024 * 1024
    );
}

#[test]
fn remote_exit_while_picker_is_open_terminates_without_removing_queued_output() {
    let (manager, _receiver) = mock_session();
    let runtime = ZmodemRuntime::spawn(
        manager.clone(),
        "test".into(),
        ZmodemTransfer::new(ZmodemDirection::Download, &[]),
    )
    .unwrap();
    runtime.activate(VecDeque::new());
    manager.event_queue.push(output(b"pending"));
    manager.event_queue.push(SessionEvent::Exited {
        session_id: "test".into(),
        reason: "EOF".into(),
    });
    assert!(
        wait_done(&runtime)
            .actions
            .iter()
            .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Failed { .. })))
    );
    assert_eq!(manager.event_queue.drain(100).events.len(), 2);
}

#[test]
fn cancel_interrupts_full_write_queue_without_waiting_for_ui_or_capacity() {
    let (manager, _receiver) = mock_session();
    manager
        .write_raw("test", &vec![0; 8 * 1024 * 1024])
        .unwrap();
    let runtime = ZmodemRuntime::spawn(
        manager.clone(),
        "test".into(),
        ZmodemTransfer::new(ZmodemDirection::Download, &[]),
    )
    .unwrap();
    runtime.activate(VecDeque::new());
    runtime.accept_download(tempfile::tempdir().unwrap().path().into());
    std::thread::sleep(Duration::from_millis(40));
    runtime.cancel("stop full writer");
    assert!(
        wait_done(&runtime)
            .actions
            .iter()
            .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Failed { .. })))
    );
}

#[test]
fn split_oo_is_removed_and_nonmatching_terminal_bytes_are_preserved() {
    for (first, second, expected) in [
        (b"O".as_slice(), b"Oprompt".as_slice(), b"prompt".as_slice()),
        (b"O", b"prompt", b"Oprompt"),
        (b"prompt", b"later", b"promptlater"),
    ] {
        let manager = Arc::new(SessionManager::new());
        let consumer = manager.register_event_consumer();
        manager.assign_session_event_consumer("test", consumer);
        let mut pending = VecDeque::from([output(first)]);
        manager.event_queue.push(output(second));
        strip_finish_trailer(
            &manager,
            "test",
            consumer,
            &Shared::default(),
            ZmodemDirection::Download,
            &mut pending,
        );
        pending.extend(manager.event_queue.drain(100).events);
        let bytes: Vec<u8> = pending
            .into_iter()
            .flat_map(|event| match event {
                SessionEvent::Output { data, .. } => data,
                _ => panic!("output"),
            })
            .collect();
        assert_eq!(bytes, expected);
    }
}

#[test]
fn finish_trailers_preserve_prompts_across_every_output_split() {
    for direction in [ZmodemDirection::Upload, ZmodemDirection::Download] {
        let mut endings = finish_line_endings();
        if direction == ZmodemDirection::Download {
            endings.extend([vec![], vec![0x11], vec![0x91]]);
            for ending in &mut endings {
                ending.extend_from_slice(b"OO");
            }
        }
        for ending in endings {
            for prompt in [
                b"OOprompt".as_slice(),
                "\x1b[32m用户@host\x1b[0m\r\n# ".as_bytes(),
            ] {
                let wire = [ending.as_slice(), prompt].concat();
                for split in 0..=wire.len() {
                    let manager = SessionManager::new();
                    let consumer = manager.register_event_consumer();
                    manager.assign_session_event_consumer("test", consumer);
                    let mut pending = VecDeque::from([
                        output(&wire[..split]),
                        output(b""),
                        output(&wire[split..]),
                    ]);
                    let consumed = strip_finish_trailer(
                        &manager,
                        "test",
                        consumer,
                        &Shared::default(),
                        direction,
                        &mut pending,
                    );
                    assert_eq!(consumed, ending, "{direction:?}, split {split}");
                    assert_eq!(
                        output_bytes(pending),
                        prompt,
                        "{direction:?}, split {split}"
                    );
                }
            }
        }
    }
}

#[test]
fn nonmatching_finish_candidates_are_replayed_without_losing_terminal_text() {
    for direction in [ZmodemDirection::Upload, ZmodemDirection::Download] {
        for bytes in [
            b"OOprompt".as_slice(),
            b"Oprompt",
            b"\rprompt",
            b"\r\x13prompt",
            b"\r\r\nprompt",
            b"\x8dprompt",
            b"\xcf\xcfprompt",
            b"\x13prompt",
            b" prompt",
            "中文\x1b[0m".as_bytes(),
        ] {
            // Bare OO is the download token, but must remain upload text.
            if direction == ZmodemDirection::Download && bytes.starts_with(b"OO") {
                continue;
            }
            for split in 0..=bytes.len() {
                let manager = SessionManager::new();
                let consumer = manager.register_event_consumer();
                let mut pending =
                    VecDeque::from([output(&bytes[..split]), output(&bytes[split..])]);
                let consumed = strip_finish_trailer(
                    &manager,
                    "test",
                    consumer,
                    &Shared::default(),
                    direction,
                    &mut pending,
                );
                assert!(consumed.is_empty(), "{direction:?}, {bytes:?}");
                assert_eq!(output_bytes(pending), bytes);
            }
        }
    }
}

#[test]
fn download_requires_complete_oo_before_consuming_any_trailer_bytes() {
    for ending in finish_line_endings() {
        for suffix in [b"prompt".as_slice(), b"Oprompt"] {
            let manager = SessionManager::new();
            let consumer = manager.register_event_consumer();
            let bytes = [ending.as_slice(), suffix].concat();
            let mut pending = VecDeque::from([output(&bytes)]);
            let consumed = strip_finish_trailer(
                &manager,
                "test",
                consumer,
                &Shared::default(),
                ZmodemDirection::Download,
                &mut pending,
            );
            assert!(consumed.is_empty());
            assert_eq!(output_bytes(pending), bytes);
        }
    }
}

#[test]
fn partial_finish_trailers_time_out_and_late_output_is_preserved() {
    for (direction, partial) in [
        (ZmodemDirection::Upload, b"\r".as_slice()),
        (ZmodemDirection::Download, b"\r\x8aO".as_slice()),
    ] {
        let manager = SessionManager::new();
        let consumer = manager.register_event_consumer();
        manager.assign_session_event_consumer("test", consumer);
        let mut pending = VecDeque::from([output(partial)]);
        let started = Instant::now();
        let consumed = strip_finish_trailer(
            &manager,
            "test",
            consumer,
            &Shared::default(),
            direction,
            &mut pending,
        );
        assert!(consumed.is_empty());
        assert!(started.elapsed() >= FINISH_WAIT);
        manager.event_queue.push(output(b"Olate prompt"));
        pending.extend(manager.event_queue.drain(100).events);
        assert_eq!(output_bytes(pending), [partial, b"Olate prompt"].concat());
    }
}

#[test]
fn complete_upload_newline_is_consumed_when_optional_xon_never_arrives() {
    let manager = SessionManager::new();
    let consumer = manager.register_event_consumer();
    manager.assign_session_event_consumer("test", consumer);
    let mut pending = VecDeque::from([output(b"\r\x8a")]);
    assert_eq!(
        strip_finish_trailer(
            &manager,
            "test",
            consumer,
            &Shared::default(),
            ZmodemDirection::Upload,
            &mut pending,
        ),
        b"\r\x8a"
    );
    assert!(pending.is_empty());
}

#[test]
fn finish_trailers_can_arrive_after_a_short_wait() {
    for direction in [ZmodemDirection::Upload, ZmodemDirection::Download] {
        let mut pending = VecDeque::from([output(b"\r")]);
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        let mut arrivals = [
            VecDeque::new(),
            VecDeque::from([output(b"\x8a")]),
            VecDeque::new(),
            VecDeque::from([output(b"\x91OOprompt")]),
        ]
        .into_iter();
        let consumed = strip_finish_trailer_with(
            "test",
            direction,
            &mut pending,
            || elapsed.get() < FINISH_WAIT,
            || {
                elapsed.set(elapsed.get() + Duration::from_millis(20));
                arrivals.next()
            },
        );
        assert_eq!(elapsed.get(), Duration::from_millis(80));
        assert_eq!(
            consumed,
            if direction == ZmodemDirection::Upload {
                b"\r\x8a\x91".as_slice()
            } else {
                b"\r\x8a\x91OO".as_slice()
            }
        );
        assert_eq!(
            output_bytes(pending),
            if direction == ZmodemDirection::Upload {
                b"OOprompt".as_slice()
            } else {
                b"prompt".as_slice()
            }
        );
    }
}

#[test]
fn finish_trailers_arriving_at_the_deadline_preserve_unmatched_output() {
    for direction in [ZmodemDirection::Upload, ZmodemDirection::Download] {
        let mut pending = VecDeque::from([output(b"\r")]);
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        let mut arrivals = [
            (Duration::from_millis(20), output(b"\x8a")),
            (FINISH_WAIT, output(b"\x91OOprompt")),
        ]
        .into_iter();
        let consumed = strip_finish_trailer_with(
            "test",
            direction,
            &mut pending,
            || elapsed.get() < FINISH_WAIT,
            || {
                arrivals.next().map(|(at, event)| {
                    elapsed.set(at);
                    VecDeque::from([event])
                })
            },
        );
        assert_eq!(elapsed.get(), FINISH_WAIT);
        assert_eq!(
            consumed,
            if direction == ZmodemDirection::Upload {
                b"\r\x8a".as_slice()
            } else {
                b"".as_slice()
            }
        );
        assert_eq!(
            output_bytes(pending),
            if direction == ZmodemDirection::Upload {
                b"\x91OOprompt".as_slice()
            } else {
                b"\r\x8a\x91OOprompt".as_slice()
            }
        );
    }
}

#[test]
fn metadata_interrupts_finish_matching_without_reordering_events() {
    for metadata in [
        SessionEvent::OutputBoundary {
            session_id: "test".into(),
            token: "fence".into(),
        },
        SessionEvent::Exited {
            session_id: "test".into(),
            reason: "EOF".into(),
        },
        SessionEvent::Error {
            session_id: "test".into(),
            message: "disconnected".into(),
        },
    ] {
        for direction in [ZmodemDirection::Upload, ZmodemDirection::Download] {
            let manager = SessionManager::new();
            let consumer = manager.register_event_consumer();
            let expected =
                VecDeque::from([output(b"\r"), metadata.clone(), output(b"\x8aOOprompt")]);
            let mut pending = expected.clone();
            assert!(
                strip_finish_trailer(
                    &manager,
                    "test",
                    consumer,
                    &Shared::default(),
                    direction,
                    &mut pending,
                )
                .is_empty()
            );
            assert_eq!(pending, expected);
        }
    }
}

#[test]
fn closing_or_cancelling_interrupts_a_partial_finish_wait() {
    use std::sync::atomic::Ordering;

    for close in [false, true] {
        let manager = SessionManager::new();
        let consumer = manager.register_event_consumer();
        manager.assign_session_event_consumer("test", consumer);
        let shared = Shared::default();
        let mut pending = VecDeque::from([output(b"\r\x8aO")]);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(20));
                if close {
                    shared.state.lock().unwrap().closed = true;
                } else {
                    shared.cancelled.store(true, Ordering::Release);
                }
            });
            let started = Instant::now();
            assert!(
                strip_finish_trailer(
                    &manager,
                    "test",
                    consumer,
                    &shared,
                    ZmodemDirection::Download,
                    &mut pending,
                )
                .is_empty()
            );
            assert!(started.elapsed() < FINISH_WAIT);
        });
        assert_eq!(output_bytes(pending), b"\r\x8aO");
    }
}

#[test]
fn disconnect_during_finish_wait_preserves_output_and_exit_order() {
    let manager = SessionManager::new();
    let consumer = manager.register_event_consumer();
    manager.assign_session_event_consumer("test", consumer);
    let mut pending = VecDeque::from([output(b"\r\x8aO")]);
    let exited = SessionEvent::Exited {
        session_id: "test".into(),
        reason: "EOF".into(),
    };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            manager.event_queue.push(exited.clone());
        });
        let started = Instant::now();
        assert!(
            strip_finish_trailer(
                &manager,
                "test",
                consumer,
                &Shared::default(),
                ZmodemDirection::Download,
                &mut pending,
            )
            .is_empty()
        );
        assert!(started.elapsed() < FINISH_WAIT);
    });
    pending.extend(manager.event_queue.drain(100).events);
    assert_eq!(pending, VecDeque::from([output(b"\r\x8aO"), exited]));
}

#[test]
fn background_transfer_finishes_with_ui_paused_then_restores_prompt_and_other_session() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("binary");
    let dst = root.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let payload: Vec<u8> = (0..700_003).map(|i| (i % 256) as u8).collect();
    std::fs::write(&src, &payload).unwrap();
    let (manager, mut receiver) = mock_session();
    let options = ZmodemTransferOptions::reliable_transport();
    for direction in [ZmodemDirection::Download, ZmodemDirection::Upload]
        .into_iter()
        .cycle()
        .take(4)
    {
        let peer_direction = match direction {
            ZmodemDirection::Download => ZmodemDirection::Upload,
            ZmodemDirection::Upload => ZmodemDirection::Download,
        };
        let mut peer = ZmodemTransfer::with_options(peer_direction, &[], options);
        let mut actions = match direction {
            ZmodemDirection::Download => peer.accept_upload(vec![src.clone()]),
            ZmodemDirection::Upload => peer.accept_download(dst.clone()),
        };
        let runtime = ZmodemRuntime::spawn(
            manager.clone(),
            "test".into(),
            ZmodemTransfer::with_options(direction, &[], options),
        )
        .unwrap();
        manager.assign_session_event_consumer("test", runtime.consumer_id());
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = recorded.clone();
        runtime.set_input_observer(Arc::new(move |_, bytes| {
            observer.lock().unwrap().extend_from_slice(bytes)
        }));
        runtime.activate(VecDeque::new());
        match direction {
            ZmodemDirection::Download => runtime.accept_download(dst.clone()),
            ZmodemDirection::Upload => runtime.accept_upload(vec![src.clone()]),
        }
        let mut expected_recording = Vec::new();
        manager.event_queue.push(SessionEvent::Output {
            session_id: "other".into(),
            data: b"unrelated".to_vec(),
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut wire = Vec::new();
            for action in actions {
                match action {
                    ZmodemAction::SendToRemote(bytes) => wire.extend(bytes),
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => panic!("{reason}"),
                    _ => {}
                }
            }
            // Real lrzsz uses LF|0x80 in its ZHEX trailer. The library's
            // default ASCII LF would miss both upload and download regressions.
            if let Some(start) = wire.windows(6).position(|bytes| bytes == b"**\x18B08") {
                assert_eq!(&wire[start + 18..start + 20], b"\r\n");
                wire[start + 19] = 0x8a;
            }
            expected_recording.extend_from_slice(&wire);
            if peer.is_done() {
                wire.extend_from_slice(b"prompt");
            }
            if !wire.is_empty() {
                manager.event_queue.push(output(&wire));
            }
            if peer.is_done() {
                break;
            }
            actions = Vec::new();
            while let Ok(SshCommand::Write(write)) = receiver.try_recv() {
                actions.extend(peer.feed_incoming(&write.bytes));
            }
            assert!(Instant::now() < deadline, "peer stalled");
            std::thread::yield_now();
        }
        // No UI event has been consumed during the whole protocol exchange.
        let done = wait_done(&runtime);
        assert!(
            done.actions
                .iter()
                .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. })))
        );
        assert_eq!(std::fs::read(dst.join("binary")).unwrap(), payload);
        let tail: Vec<u8> = done
            .pending_events
            .into_iter()
            .flat_map(|e| match e {
                SessionEvent::Output { data, .. } => data,
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(tail, b"prompt");
        assert_eq!(*recorded.lock().unwrap(), expected_recording);
        assert_eq!(runtime.stats().input_bytes, expected_recording.len() as u64);
        assert!(
            matches!(&manager.event_queue.drain(100).events[0], SessionEvent::Output { session_id, data } if session_id == "other" && data == b"unrelated")
        );
        assert!(runtime.stats().write_queue_peak_bytes <= 8 * 1024 * 1024);
    }
}

#[test]
fn cancellation_can_publish_a_terminal_result_with_all_lifecycle_slots_full() {
    let shared = super::Shared::default();
    for _ in 0..super::EVENT_CAPACITY {
        assert!(super::publish(
            &shared,
            super::ZmodemRuntimeEvent {
                actions: Vec::new(),
                done: false,
                pending_events: VecDeque::from([SessionEvent::CwdChanged {
                    session_id: "test".into(),
                    cwd: "/".into()
                }])
            }
        ));
    }
    shared
        .cancelled
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(super::publish(
        &shared,
        super::ZmodemRuntimeEvent {
            actions: vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                reason: "cancel".into()
            })],
            done: true,
            pending_events: VecDeque::new()
        }
    ));
    let state = shared.state.lock().unwrap();
    assert_eq!(state.events.len(), super::EVENT_CAPACITY);
    assert!(state.events.back().unwrap().done);
}

#[tokio::test]
async fn tmux_modem_capability_and_transfer_writers_reject_before_sending() {
    use crate::SessionError;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (manager, mut receiver) = mock_session();
    let (mut info, active) = {
        let sessions = manager.sessions.lock().unwrap();
        let ManagedSession::Ssh(host) = &sessions["test"] else {
            unreachable!()
        };
        (host.info.clone(), host.tmux_active.clone())
    };
    let (pane_tx, mut pane_rx) = crate::tmux::mailbox::channel();
    active.store(true, Ordering::Release);
    info.id = "pane".into();
    SessionManager::register_tmux_pane(
        &manager.sessions,
        &manager.event_queue,
        "test",
        info,
        "%0".into(),
        pane_tx,
    )
    .unwrap();
    for session in ["test", "pane"] {
        assert!(!manager.supports_modem_transfer(session));
        for error in [
            manager
                .write_raw_cancellable(session, b"binary", &AtomicBool::new(false))
                .unwrap_err(),
            manager
                .write_raw_cancellable_measured(session, b"binary", &AtomicBool::new(false))
                .unwrap_err(),
        ] {
            let SessionError::Write { source, .. } = error else {
                panic!("unexpected error")
            };
            assert_eq!(
                source.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::Unsupported
            );
        }
    }
    // Pane protocol/input bytes ride send-keys like any other pane input;
    // only the transfer writers above stay rejected.
    manager.write_raw("pane", b"binary").unwrap();
    assert!(matches!(
        pane_rx.recv().await.unwrap().command,
        SshCommand::TmuxPaneInput { pane, data } if pane == "%0" && data == b"binary"
    ));
    // The tmux-owned control channel still refuses raw writes on the host.
    let SessionError::Write { source, .. } = manager.write_raw("test", b"binary").unwrap_err()
    else {
        panic!("unexpected error")
    };
    assert_eq!(
        source.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Unsupported
    );
    assert!(receiver.try_recv().is_err());
    active.store(false, Ordering::Release);
    assert!(manager.supports_modem_transfer("test"));
    manager.write_raw("test", b"binary").unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), SshCommand::Write(write) if write.bytes == b"binary")
    );
}

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{ZmodemRuntime, strip_finish_oo};
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

fn mock_session() -> (Arc<SessionManager>, ssh_mailbox::Receiver) {
    let manager = Arc::new(SessionManager::new());
    let (command_tx, receiver) = ssh_mailbox::channel();
    manager.sessions.lock().unwrap().insert(
        "test".into(),
        ManagedSession::Ssh(SshChannelTransport {
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
            tmux_tx: tokio::sync::mpsc::unbounded_channel().0,
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
        strip_finish_oo(&manager, "test", consumer, &mut pending);
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
fn background_transfer_finishes_with_ui_paused_then_restores_prompt_and_other_session() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("binary");
    let dst = root.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let payload: Vec<u8> = (0..700_003).map(|i| (i % 256) as u8).collect();
    std::fs::write(&src, &payload).unwrap();
    let (manager, mut receiver) = mock_session();
    let options = ZmodemTransferOptions::reliable_transport();
    for _ in 0..2 {
        let mut peer = ZmodemTransfer::with_options(ZmodemDirection::Upload, &[], options);
        let mut actions = peer.accept_upload(vec![src.clone()]);
        let runtime = ZmodemRuntime::spawn(
            manager.clone(),
            "test".into(),
            ZmodemTransfer::with_options(ZmodemDirection::Download, &[], options),
        )
        .unwrap();
        manager.assign_session_event_consumer("test", runtime.consumer_id());
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = recorded.clone();
        runtime.set_input_observer(Arc::new(move |_, bytes| {
            observer.lock().unwrap().extend_from_slice(bytes)
        }));
        runtime.activate(VecDeque::new());
        runtime.accept_download(dst.clone());
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

use std::collections::VecDeque;
use std::io::{BufWriter, Write};

use super::{
    TransferState, ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
    ZmodemTransferOptions, validate_file_size,
};

#[test]
fn file_size_boundary_rejects_four_gib_and_larger_without_clamping() {
    assert_eq!(validate_file_size(u32::MAX as u64).unwrap(), u32::MAX);
    assert!(validate_file_size(u32::MAX as u64 + 1).is_err());
    assert!(validate_file_size(u64::MAX).is_err());
}

#[test]
fn missing_later_file_rejects_the_batch_before_any_file_is_announced() {
    let root = tempfile::tempdir().unwrap();
    let existing = root.path().join("existing");
    std::fs::write(&existing, b"data").unwrap();
    let mut sender = ZmodemTransfer::new(ZmodemDirection::Upload, &[]);
    let actions = sender.accept_upload(vec![existing, root.path().join("missing")]);
    assert!(sender.is_done());
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Failed { .. })))
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. })))
    );
    assert!(sender.feed_incoming(b"anything").is_empty());
}

#[test]
fn streaming_windows_round_trip_multiple_binary_files_with_fragmented_input() {
    for window in [10, 64, 256, 1024] {
        for nonstop in [false, true] {
            round_trip(window, nonstop, false);
        }
    }
}

#[test]
fn buffered_download_flush_failure_is_fatal_instead_of_successful() {
    round_trip(256, true, true);
}

#[test]
fn download_write_failure_is_fatal() {
    round_trip_fault(256, true, 5);
}

#[test]
fn shrinking_an_open_upload_file_is_fatal() {
    round_trip_fault(256, true, 2);
}

#[test]
fn later_file_is_revalidated_when_opened_after_preflight() {
    round_trip_fault(256, true, 3);
}

#[test]
fn corrupted_data_is_retransmitted_with_buffered_seek() {
    round_trip_fault(64, true, 4);
    round_trip_fault(
        ZmodemTransferOptions::reliable_transport().streaming_window,
        true,
        4,
    );
}

fn round_trip(window: usize, nonstop: bool, fail_flush: bool) {
    round_trip_fault(window, nonstop, u8::from(fail_flush));
}

fn round_trip_fault(window: usize, nonstop: bool, fault: u8) {
    let fail_flush = fault == 1;
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    std::fs::create_dir(&src).unwrap();
    std::fs::create_dir(&dst).unwrap();
    let payloads = [
        vec![],
        (0..if window >= 1024 {
            1_100_003
        } else if fault == 2 {
            600_003
        } else {
            80_003
        })
            .map(|i| (i % 256) as u8)
            .collect::<Vec<_>>(),
        b"last".to_vec(),
    ];
    let paths = payloads
        .iter()
        .enumerate()
        .map(|(index, data)| {
            let path = src.join(format!("file-{index}"));
            std::fs::write(&path, data).unwrap();
            path
        })
        .collect();
    let options = ZmodemTransferOptions {
        nonstop_receive: nonstop,
        streaming_window: window,
        buffered_files: true,
    };
    let mut transfers = [
        ZmodemTransfer::with_options(ZmodemDirection::Upload, &[], options),
        ZmodemTransfer::with_options(ZmodemDirection::Download, &[], options),
    ];
    let mut actions = [
        transfers[0].accept_upload(paths),
        transfers[1].accept_download(dst.clone()),
    ];
    let mut queues = [VecDeque::<u8>::new(), VecDeque::new()];
    let mut complete = [false; 2];
    let mut failures = Vec::new();
    let mut more = [false; 2];
    let mut replaced = false;
    let mut turns = 0;
    let mut injected = false;
    loop {
        turns += 1;
        assert!(turns < 200_000, "protocol stalled");
        for index in 0..2 {
            for action in std::mem::take(&mut actions[index]) {
                match action {
                    ZmodemAction::SendToRemote(mut bytes) => {
                        if fault == 4 && index == 0 && !injected && bytes.len() > 500 {
                            bytes[64] ^= 1;
                            injected = true;
                        }
                        queues[1 - index].extend(bytes);
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Complete { file_count, .. }) => {
                        assert_eq!(file_count, 3);
                        complete[index] = true;
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                        failures.push(reason)
                    }
                    _ => {}
                }
            }
            if (fail_flush || fault == 5)
                && !replaced
                && let TransferState::Receiving {
                    current_file: Some(file),
                    ..
                } = &mut transfers[index].state
                && (fault != 5 || file.name == "file-1")
            {
                // A read-only handle fails either immediately or at flush.
                let readonly = std::fs::File::open(src.join("file-0")).unwrap();
                file.file =
                    BufWriter::with_capacity(if fault == 5 { 0 } else { 256 * 1024 }, readonly);
                if fault != 5 {
                    file.file.write_all(b"buffered").unwrap();
                }
                replaced = true;
            }
            if !injected && index == 0 {
                if fault == 2
                    && let TransferState::Sending {
                        current_file: Some(file),
                        ..
                    } = &transfers[0].state
                    && file.name == "file-1"
                    && file.position > 0
                {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(src.join("file-1"))
                        .unwrap()
                        .set_len(0)
                        .unwrap();
                    injected = true;
                }
                if fault == 3 {
                    // The later file existed during preflight; opening it must
                    // still fail explicitly if it disappears before its turn.
                    std::fs::remove_file(src.join("file-2")).unwrap();
                    injected = true;
                }
            }
            let input: Vec<u8> = queues[index].iter().take(137).copied().collect();
            if !input.is_empty() || more[index] {
                let step = transfers[index].advance(&input);
                assert!(
                    step.actions
                        .iter()
                        .filter_map(|a| match a {
                            ZmodemAction::SendToRemote(bytes) => Some(bytes.len()),
                            _ => None,
                        })
                        .sum::<usize>()
                        <= 64 * 1024
                );
                queues[index].drain(..step.consumed);
                more[index] = step.more;
                actions[index] = step.actions;
                if transfers[index].is_done() {
                    queues[index].clear();
                }
            }
        }
        if !failures.is_empty() {
            break;
        }
        if complete == [true, true] {
            break;
        }
    }
    if fault == 2 || fault == 3 {
        assert!(injected);
        assert!(!failures.is_empty(), "fault must fail");
        assert!(!complete[0]);
    } else if fault == 5 {
        assert!(
            failures.iter().any(|reason| reason.contains("write")),
            "{failures:?}"
        );
        assert!(!complete[1]);
    } else if fail_flush {
        assert!(
            failures.iter().any(|reason| reason.contains("flush")),
            "{failures:?}"
        );
        assert!(!complete[1]);
    } else {
        assert!(failures.is_empty(), "{failures:?}");
        for (index, payload) in payloads.iter().enumerate() {
            assert_eq!(
                &std::fs::read(dst.join(format!("file-{index}"))).unwrap(),
                payload
            );
        }
    }
}

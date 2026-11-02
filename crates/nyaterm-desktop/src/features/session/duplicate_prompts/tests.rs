use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use nyaterm_transport::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathTransferOptions, SftpTransferDirection, SftpTransferOptions,
};

use super::{
    DuplicatePromptAction, SftpDuplicatePromptBroker, SftpDuplicatePromptState,
    UploadBatchDuplicateContext, UploadBatchDuplicateResolver,
};

fn request(name: &str) -> SftpDuplicateRequest {
    SftpDuplicateRequest {
        direction: SftpTransferDirection::Upload,
        source_path: format!("/local/{name}"),
        target_path: format!("/remote/{name}"),
        is_directory: false,
    }
}

fn activate(broker: &SftpDuplicatePromptBroker) -> SftpDuplicatePromptState {
    let prompt = broker.pop_pending().expect("queued conflict");
    SftpDuplicatePromptState {
        id: prompt.id,
        request: prompt.request,
        response_tx: prompt.response_tx,
        upload_batch: prompt.upload_batch,
    }
}

fn answer(rx: mpsc::Receiver<SftpDuplicateDecision>) -> SftpDuplicateDecision {
    rx.recv_timeout(Duration::from_secs(2))
        .expect("conflict released")
}

#[test]
fn ordinary_choices_only_resolve_the_current_upload() {
    for decision in [
        SftpDuplicateDecision::Overwrite,
        SftpDuplicateDecision::Skip,
        SftpDuplicateDecision::Rename,
    ] {
        let broker = SftpDuplicatePromptBroker::default();
        let batch = Arc::new(UploadBatchDuplicateContext::default());
        let first = broker
            .enqueue_batch_request(request("first"), Some(batch.clone()))
            .unwrap();
        let second = broker
            .enqueue_batch_request(request("second"), Some(batch))
            .unwrap();
        broker
            .resolve(activate(&broker), DuplicatePromptAction::Current(decision))
            .unwrap();
        assert_eq!(answer(first), decision);
        assert_eq!(second.try_recv(), Err(mpsc::TryRecvError::Empty));
        broker
            .resolve(
                activate(&broker),
                DuplicatePromptAction::Current(SftpDuplicateDecision::Skip),
            )
            .unwrap();
        assert_eq!(answer(second), SftpDuplicateDecision::Skip);
    }
}

#[test]
fn overwrite_batch_releases_queued_siblings_and_future_conflicts_but_isolates_other_operations() {
    let broker = SftpDuplicatePromptBroker::default();
    let batch = Arc::new(UploadBatchDuplicateContext::default());
    let other_batch = Arc::new(UploadBatchDuplicateContext::default());
    let first = broker
        .enqueue_batch_request(request("first"), Some(batch.clone()))
        .unwrap();
    let unrelated = broker
        .enqueue_batch_request(request("other"), Some(other_batch))
        .unwrap();
    let sibling = broker
        .enqueue_batch_request(request("sibling"), Some(batch.clone()))
        .unwrap();
    let ordinary = broker
        .enqueue_decision_request(request("ordinary"))
        .unwrap();
    broker
        .resolve(
            activate(&broker),
            DuplicatePromptAction::OverwriteUploadBatch,
        )
        .unwrap();
    assert_eq!(answer(first), SftpDuplicateDecision::Overwrite);
    assert_eq!(answer(sibling), SftpDuplicateDecision::Overwrite);
    assert_eq!(
        answer(
            broker
                .enqueue_batch_request(request("future"), Some(batch))
                .unwrap()
        ),
        SftpDuplicateDecision::Overwrite
    );
    assert_eq!(unrelated.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(ordinary.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert_eq!(activate(&broker).request.target_path, "/remote/other");
    assert_eq!(activate(&broker).request.target_path, "/remote/ordinary");
    assert!(!broker.has_pending());
    let new_operation = broker
        .enqueue_batch_request(request("first"), Some(Arc::default()))
        .unwrap();
    assert_eq!(new_operation.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert!(broker.has_pending());
}

#[test]
fn overwrite_batch_does_not_revisit_previously_skipped_items() {
    let broker = SftpDuplicatePromptBroker::default();
    let batch = Arc::new(UploadBatchDuplicateContext::default());
    let skipped = broker
        .enqueue_batch_request(request("skipped"), Some(batch.clone()))
        .unwrap();
    broker
        .resolve(
            activate(&broker),
            DuplicatePromptAction::Current(SftpDuplicateDecision::Skip),
        )
        .unwrap();
    let current = broker
        .enqueue_batch_request(request("current"), Some(batch))
        .unwrap();
    broker
        .resolve(
            activate(&broker),
            DuplicatePromptAction::OverwriteUploadBatch,
        )
        .unwrap();
    assert_eq!(answer(skipped), SftpDuplicateDecision::Skip);
    assert_eq!(answer(current), SftpDuplicateDecision::Overwrite);
}

#[test]
fn enqueue_racing_with_overwrite_batch_never_leaves_a_sibling_waiting() {
    for _ in 0..32 {
        let broker = Arc::new(SftpDuplicatePromptBroker::default());
        let batch = Arc::new(UploadBatchDuplicateContext::default());
        let current = broker
            .enqueue_batch_request(request("current"), Some(batch.clone()))
            .unwrap();
        let prompt = activate(&broker);
        let barrier = Arc::new(Barrier::new(2));
        let worker_broker = broker.clone();
        let worker_barrier = barrier.clone();
        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                worker_barrier.wait();
                answer(
                    worker_broker
                        .enqueue_batch_request(request("racing"), Some(batch))
                        .unwrap(),
                )
            });
            barrier.wait();
            broker
                .resolve(prompt, DuplicatePromptAction::OverwriteUploadBatch)
                .unwrap();
            assert_eq!(answer(current), SftpDuplicateDecision::Overwrite);
            assert_eq!(worker.join().unwrap(), SftpDuplicateDecision::Overwrite);
            assert!(!broker.has_pending());
        });
    }
}

#[test]
fn retry_options_keep_batch_choice_while_new_batches_and_non_uploads_still_ask() {
    let broker = Arc::new(SftpDuplicatePromptBroker::default());
    let resolver = Arc::new(UploadBatchDuplicateResolver::new(broker.clone()));
    let rx = broker
        .enqueue_batch_request(request("current"), Some(resolver.batch.clone()))
        .unwrap();
    broker
        .resolve(
            activate(&broker),
            DuplicatePromptAction::OverwriteUploadBatch,
        )
        .unwrap();
    assert_eq!(answer(rx), SftpDuplicateDecision::Overwrite);
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Ask,
        Some(resolver.clone()),
        SftpTransferOptions::default(),
    );
    let retry = options.with_transfer_options(SftpTransferOptions::default().with_max_retries(2));
    assert_eq!(
        retry
            .duplicate_resolver()
            .unwrap()
            .resolve_duplicate(&request("retry"))
            .unwrap(),
        SftpDuplicateDecision::Overwrite
    );
    let mut download = request("download");
    download.direction = SftpTransferDirection::Download;
    let (wake, mut wake_rx) = crate::models::event_wake::EventWake::new();
    wake.arm(crate::models::event_wake::ANY_INTEREST);
    broker.set_wake(wake);
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || resolver.resolve_duplicate(&download));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            use futures::StreamExt as _;
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), wake_rx.next())
                    .await
                    .unwrap(),
                Some(())
            );
        });
        let prompt = activate(&broker);
        assert!(prompt.upload_batch.is_none());
        broker
            .resolve(
                prompt,
                DuplicatePromptAction::Current(SftpDuplicateDecision::Skip),
            )
            .unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), SftpDuplicateDecision::Skip);
    });
}

#[test]
fn a_non_batch_prompt_rejects_an_invalid_all_action_and_releases_the_waiter() {
    let broker = SftpDuplicatePromptBroker::default();
    let rx = broker.enqueue_decision_request(request("single")).unwrap();
    assert!(
        broker
            .resolve(
                activate(&broker),
                DuplicatePromptAction::OverwriteUploadBatch
            )
            .is_err()
    );
    assert_eq!(answer(rx), SftpDuplicateDecision::Skip);
}

#[test]
fn identical_sftp_duplicate_prompts_receive_unique_ids() {
    let broker = SftpDuplicatePromptBroker::default();
    let request = SftpDuplicateRequest {
        direction: SftpTransferDirection::Upload,
        source_path: "/local/file.txt".to_string(),
        target_path: "/remote/file.txt".to_string(),
        is_directory: false,
    };
    let first_rx = broker
        .enqueue_decision_request(request.clone())
        .expect("enqueue first duplicate prompt");
    let second_rx = broker
        .enqueue_decision_request(request)
        .expect("enqueue second duplicate prompt");
    let first = broker.pop_pending().expect("first pending prompt");
    let second = broker.pop_pending().expect("second pending prompt");
    assert_ne!(first.id, second.id);
    first
        .response_tx
        .send(SftpDuplicateDecision::Skip)
        .expect("send first duplicate decision");
    second
        .response_tx
        .send(SftpDuplicateDecision::Overwrite)
        .expect("send second duplicate decision");
    assert_eq!(
        first_rx.recv_timeout(Duration::from_secs(1)),
        Ok(SftpDuplicateDecision::Skip)
    );
    assert_eq!(
        second_rx.recv_timeout(Duration::from_secs(1)),
        Ok(SftpDuplicateDecision::Overwrite)
    );
}

use super::Prefetch;
use crate::sftp::SftpTransferControl;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn bytes(offset: u64, length: usize) -> Vec<u8> {
    (offset..offset + length as u64)
        .map(|i| (i % 251) as u8)
        .collect()
}

#[tokio::test]
async fn three_reads_are_in_flight_before_any_response_and_out_of_order_data_is_correct() {
    let control = SftpTransferControl::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut prefetch = Prefetch::new(
        Box::new(move |offset, length| {
            let tx = tx.clone();
            Box::pin(async move {
                let (reply, response) = oneshot::channel();
                tx.send((offset, length, reply)).unwrap();
                response.await.map_err(anyhow::Error::from)
            })
        }),
        &control,
        Some(64 * 20),
        64,
        3,
    );
    let result = {
        let read = prefetch.read(0, 64);
        tokio::pin!(read);
        let mut requests = Vec::new();
        for _ in 0..3 {
            tokio::select! {
                result = &mut read => panic!("read completed without response: {result:?}"),
                request = rx.recv() => requests.push(request.unwrap()),
            }
        }
        assert!(rx.try_recv().is_err());
        for (offset, length, reply) in requests.into_iter().rev() {
            reply.send(bytes(offset, length)).unwrap();
        }
        read.await.unwrap()
    };
    assert_eq!(result, bytes(0, 64));
    assert!(prefetch.blocks.len() <= 6);
    assert!(prefetch.pending.len() <= 3);
}

#[tokio::test]
async fn window_stays_bounded_and_small_cross_block_and_seek_reads_are_correct() {
    for (block_size, concurrency) in [(8, 1), (64, 3), (256, 10)] {
        let control = SftpTransferControl::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let size = 10_003;
        let mut prefetch = Prefetch::new(
            Box::new(move |offset, length| {
                seen.lock().unwrap().push((offset, length));
                Box::pin(async move { Ok(bytes(offset, length)) })
            }),
            &control,
            Some(size),
            block_size,
            concurrency,
        );
        assert_eq!(prefetch.read(0, 2).await.unwrap(), bytes(0, 2));
        let before = calls.lock().unwrap().len();
        assert_eq!(prefetch.read(1, 2).await.unwrap(), bytes(1, 2));
        assert_eq!(calls.lock().unwrap().len(), before);
        for offset in [block_size as u64 - 2, 5_003, 5_002, 0, 9_998, 10_003] {
            let length = 64.min(size.saturating_sub(offset) as usize);
            assert_eq!(
                prefetch.read(offset, 64).await.unwrap(),
                bytes(offset, length)
            );
            assert!(prefetch.blocks.len() <= 2 * concurrency);
            assert!(prefetch.pending.len() <= concurrency);
            let allocated: usize = prefetch
                .blocks
                .iter()
                .map(|(offset, data)| {
                    data.as_ref()
                        .map_or(block_size, |result| result.as_ref().map_or(0, Vec::len))
                        .min(size.saturating_sub(*offset) as usize)
                })
                .sum();
            assert!(allocated <= 2 * concurrency * block_size);
        }
    }
}

#[tokio::test]
async fn speculative_error_is_deferred_until_its_range_is_consumed() {
    let control = SftpTransferControl::new();
    let mut prefetch = Prefetch::new(
        Box::new(|offset, length| {
            Box::pin(async move {
                anyhow::ensure!(offset != 64, "injected read failure");
                Ok(bytes(offset, length))
            })
        }),
        &control,
        Some(256),
        64,
        3,
    );
    assert_eq!(prefetch.read(0, 64).await.unwrap(), bytes(0, 64));
    assert!(prefetch.read(64, 1).await.is_err());
    // The error did not poison an unrelated cached range.
    assert_eq!(prefetch.read(128, 1).await.unwrap(), bytes(128, 1));
}

#[tokio::test]
async fn consumed_prefetch_failure_preserves_timeout_classification() {
    let control = SftpTransferControl::new();
    let mut prefetch = Prefetch::new(
        Box::new(|_, _| {
            Box::pin(async {
                let elapsed = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
                    .await
                    .unwrap_err();
                Err(anyhow::Error::new(elapsed).context("injected SFTP timeout"))
            })
        }),
        &control,
        Some(64),
        64,
        1,
    );
    let error = prefetch.read(0, 1).await.unwrap_err();
    assert!(crate::sftp::sftp_error_invalidates_compatibility_session(
        &error
    ));
}

#[tokio::test]
async fn unknown_size_uses_one_cached_block_and_reports_eof_without_speculation() {
    let control = SftpTransferControl::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let mut prefetch = Prefetch::new(
        Box::new(move |offset, length| {
            seen.lock().unwrap().push(offset);
            Box::pin(async move {
                Ok(bytes(
                    offset,
                    length.min(70_u64.saturating_sub(offset) as usize),
                ))
            })
        }),
        &control,
        None,
        64,
        10,
    );
    assert_eq!(prefetch.read(0, 2).await.unwrap(), bytes(0, 2));
    assert_eq!(*calls.lock().unwrap(), vec![0]);
    assert_eq!(prefetch.read(2, 64).await.unwrap(), bytes(2, 64));
    assert_eq!(*calls.lock().unwrap(), vec![0, 64]);
    assert_eq!(prefetch.read(66, 64).await.unwrap(), bytes(66, 4));
    assert!(prefetch.read(70, 1).await.unwrap().is_empty());
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(prefetch.slots, 1);
}

#[tokio::test]
async fn pause_blocks_cached_delivery_and_cancel_wakes_an_outstanding_read() {
    let control = SftpTransferControl::new();
    let mut prefetch = Prefetch::new(
        Box::new(|offset, length| Box::pin(async move { Ok(bytes(offset, length)) })),
        &control,
        Some(1024),
        64,
        3,
    );
    prefetch.read(0, 1).await.unwrap();
    control.pause();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), prefetch.read(1, 1))
            .await
            .is_err()
    );
    control.resume();
    assert_eq!(prefetch.read(1, 1).await.unwrap(), bytes(1, 1));
    let mut stalled = Prefetch::new(
        Box::new(|_, _| Box::pin(std::future::pending())),
        &control,
        Some(1024),
        64,
        3,
    );
    let cancel = async {
        tokio::task::yield_now().await;
        control.cancel();
    };
    let (result, _) = tokio::join!(stalled.read(0, 1), cancel);
    assert!(result.is_err());
}

#[tokio::test]
async fn seek_outside_window_and_consumer_drop_release_obsolete_read_futures() {
    struct ActiveRead(Arc<AtomicUsize>);
    impl Drop for ActiveRead {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let active = Arc::new(AtomicUsize::new(0));
    let count = active.clone();
    let control = SftpTransferControl::new();
    let mut prefetch = Prefetch::new(
        Box::new(move |offset, length| {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                let _guard = ActiveRead(count);
                if offset < 256 {
                    std::future::pending::<()>().await;
                }
                Ok(bytes(offset, length))
            })
        }),
        &control,
        Some(10_000),
        64,
        3,
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), prefetch.read(0, 1))
            .await
            .is_err()
    );
    assert_eq!(active.load(Ordering::SeqCst), 3);
    assert_eq!(prefetch.read(5_003, 1).await.unwrap(), bytes(5_003, 1));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), prefetch.read(0, 1))
            .await
            .is_err()
    );
    assert_eq!(active.load(Ordering::SeqCst), 3);
    drop(prefetch);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

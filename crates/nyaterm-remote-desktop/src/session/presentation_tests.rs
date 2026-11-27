use std::sync::{Arc, Mutex};

use super::{EventQueue, handle_control};
use crate::{PixelFormat, RdpControlMessage, RdpFrameEvent, RdpSessionState};

fn pipeline_queue(
    explicit_completion: bool,
) -> (Arc<EventQueue>, crate::presentation::PresentationWorker) {
    let queue = Arc::new(EventQueue::default());
    let worker = crate::presentation::PresentationWorker::spawn_with_limits(
        "rdp-presentation-test".into(),
        crate::presentation::RdpPresentationConfig::default(),
        crate::RDP_FRAMEBUFFER_LIMITS,
        queue.heartbeat.clone(),
        None,
        |error| panic!("{error}"),
    )
    .unwrap();
    queue.presentation.set(worker.client.clone()).ok().unwrap();
    queue.state.lock().unwrap().frame_update_complete = explicit_completion;
    (queue, worker)
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "RDP compositor made no progress"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn take_presentation(
    worker: &crate::presentation::PresentationWorker,
) -> crate::presentation::RdpPresentation {
    let mut presentation = None;
    wait_for(|| {
        presentation = worker.client.take();
        presentation.is_some()
    });
    presentation.unwrap()
}

fn apply_uploads(
    pixels: &mut [u8],
    width: u32,
    presentation: &crate::presentation::RdpPresentation,
) {
    for region in &presentation.regions {
        for row in 0..region.rect.height as usize {
            let source = row * region.rect.width as usize * 4;
            let target =
                ((region.rect.y as usize + row) * width as usize + region.rect.x as usize) * 4;
            pixels[target..target + region.rect.width as usize * 4]
                .copy_from_slice(&region.pixels[source..source + region.rect.width as usize * 4]);
        }
    }
}

fn pixel(epoch: u64, x: u32, y: u32, value: u8) -> RdpFrameEvent {
    RdpFrameEvent::Bitmap {
        epoch,
        full: false,
        x,
        y,
        width: 1,
        height: 1,
        stride: 4,
        format: PixelFormat::Bgra8,
        pixels: vec![value, value, value, 255].into(),
    }
}

#[test]
fn rdp_pipeline_composes_more_than_eight_rectangles_and_eight_mebibytes_without_ui_drain() {
    let (queue, mut worker) = pipeline_queue(true);
    queue.push_reset("s", 1, 2048, 2048);
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    // A 16 MiB submission split into 64 independently colored horizontal bands.
    for band in 0u32..64 {
        assert!(
            queue
                .receive_frame(RdpFrameEvent::Bitmap {
                    epoch: 1,
                    full: false,
                    x: 0,
                    y: band * 32,
                    width: 2048,
                    height: 32,
                    stride: 2048 * 4,
                    format: PixelFormat::Bgra8,
                    pixels: [band as u8, 2, 3, 255].repeat(2048 * 32).into(),
                })
                .unwrap()
        );
    }
    wait_for(|| worker.client.metrics().rectangles == 64);
    assert!(
        worker.client.take().is_none(),
        "a partial submission is never published"
    );
    assert!(
        queue.drain().frames.is_empty(),
        "pixels never enter the UI frame queue"
    );
    queue.complete_update(1, 1).unwrap();
    let presentation = take_presentation(&worker);
    let mut actual = vec![0; 2048 * 2048 * 4];
    apply_uploads(&mut actual, 2048, &presentation);
    for (y, row) in actual.as_chunks::<{ 2048 * 4 }>().0.iter().enumerate() {
        assert!(
            row.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [(y / 32) as u8, 2, 3, 255])
        );
    }
    assert_eq!(worker.client.metrics().completed_updates, 1);
    worker.shutdown();
}

#[test]
fn rdp_pipeline_replaces_mailbox_with_all_unacknowledged_damage_and_latest_overlaps() {
    let (queue, mut worker) = pipeline_queue(true);
    queue.push_reset("s", 1, 256, 256);
    queue.receive_frame(pixel(1, 0, 0, 1)).unwrap();
    queue.complete_update(1, 1).unwrap();
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    let first = take_presentation(&worker);
    let mut actual = vec![0; 256 * 256 * 4];
    apply_uploads(&mut actual, 256, &first);
    worker.client.acknowledge(first.id);
    queue.receive_frame(pixel(1, 1, 1, 10)).unwrap();
    queue.complete_update(1, 2).unwrap();
    wait_for(|| worker.client.metrics().publications >= 2);
    queue.receive_frame(pixel(1, 129, 129, 20)).unwrap();
    queue.receive_frame(pixel(1, 1, 1, 30)).unwrap();
    queue.complete_update(1, 3).unwrap();
    wait_for(|| worker.client.metrics().publications >= 3);
    assert!(worker.client.metrics().replaced_publications >= 1);
    let latest = take_presentation(&worker);
    apply_uploads(&mut actual, 256, &latest);
    assert_eq!(&actual[(256 + 1) * 4..(256 + 2) * 4], &[30, 30, 30, 255]);
    assert_eq!(
        &actual[(129 * 256 + 129) * 4..(129 * 256 + 130) * 4],
        &[20, 20, 20, 255]
    );
    assert_eq!(&actual[..4], &[1, 1, 1, 255]);
    worker.client.acknowledge(latest.id);
    worker.shutdown();
}

#[test]
fn rdp_pipeline_accepts_legacy_helper_bitmaps_without_completion_messages() {
    let (queue, mut worker) = pipeline_queue(false);
    // This extent is legal for RDP but exceeds VNC's maximum width.
    queue.push_reset("s", 4, 8192, 1);
    queue.receive_frame(pixel(4, 8191, 0, 7)).unwrap();
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    let presentation = take_presentation(&worker);
    assert_eq!((presentation.width, presentation.height), (8192, 1));
    assert_eq!(&presentation.regions[0].pixels[8191 * 4..], &[7, 7, 7, 255]);
    assert_eq!(worker.client.metrics().completed_updates, 1);
    assert!(
        queue.complete_update(4, 2).is_err(),
        "unnegotiated new messages are rejected"
    );
    worker.shutdown();
}

#[test]
fn rdp_pipeline_reset_rejects_stale_pixels_and_old_acknowledgments() {
    let (queue, mut worker) = pipeline_queue(true);
    queue.push_reset("s", 1, 1, 1);
    queue.receive_frame(pixel(1, 0, 0, 1)).unwrap();
    queue.complete_update(1, 1).unwrap();
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    let old = take_presentation(&worker);
    queue.push_reset("s", 2, 1, 1);
    worker.client.acknowledge(old.id);
    queue.receive_frame(pixel(1, 0, 0, 99)).unwrap();
    queue.complete_update(1, 99).unwrap();
    queue.receive_frame(pixel(2, 0, 0, 2)).unwrap();
    queue.complete_update(2, 1).unwrap();
    assert!(queue.complete_update(2, 1).is_err());
    let new = take_presentation(&worker);
    assert_eq!(new.id.epoch, 2);
    assert!(new.full);
    assert_eq!(new.regions[0].pixels.as_ref(), &[2, 2, 2, 255]);
    queue.close();
    assert!(!queue.receive_frame(pixel(2, 0, 0, 3)).unwrap());
    assert!(worker.client.take().is_none());
    worker.shutdown();
}

#[test]
fn rdp_pipeline_negotiates_completion_and_rejects_messages_without_support() {
    let (queue, mut worker) = pipeline_queue(false);
    let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
    let capabilities = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let mut hello_received = false;
    handle_control(
        "s",
        RdpControlMessage::ServerHello {
            version: crate::PROTOCOL_VERSION,
            capabilities: crate::RdpServerCapabilities {
                frame_update_complete: true,
                server_redirection: false,
                ..Default::default()
            },
        },
        &queue,
        &state,
        &capabilities,
        &pending,
        &mut hello_received,
    )
    .unwrap();
    queue.push_reset("s", 1, 1, 1);
    queue.receive_frame(pixel(1, 0, 0, 8)).unwrap();
    wait_for(|| worker.client.metrics().rectangles == 1);
    assert_eq!(worker.client.metrics().completed_updates, 0);
    handle_control(
        "s",
        RdpControlMessage::FrameUpdateComplete {
            session_id: "s".into(),
            epoch: 1,
            sequence: 1,
        },
        &queue,
        &state,
        &capabilities,
        &pending,
        &mut hello_received,
    )
    .unwrap();
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    let presentation = take_presentation(&worker);
    assert_eq!(&presentation.regions[0].pixels[..], &[8, 8, 8, 255]);
    worker.shutdown();
}

#[test]
fn rdp_pipeline_reconnect_discards_pending_submissions_before_same_size_reset() {
    let (queue, mut worker) = pipeline_queue(true);
    queue.push_reset("s", 1, 1, 1);
    queue.receive_frame(pixel(1, 0, 0, 1)).unwrap();
    queue.complete_update(1, 1).unwrap();
    worker
        .client
        .set_visible(true, crate::presentation::RdpPresentationConfig::default());
    wait_for(|| worker.client.metrics().publications == 1);
    let state = Arc::new(Mutex::new(RdpSessionState::Connected));
    handle_control(
        "s",
        RdpControlMessage::State {
            session_id: "s".into(),
            state: RdpSessionState::Reconnecting,
            message: None,
        },
        &queue,
        &state,
        &Arc::new(Mutex::new(None)),
        &Arc::new(Mutex::new(std::collections::HashMap::new())),
        &mut true,
    )
    .unwrap();
    assert!(worker.client.take().is_none());
    queue.receive_frame(pixel(1, 0, 0, 99)).unwrap();
    queue.complete_update(1, 2).unwrap();
    queue.push_reset("s", 2, 1, 1);
    queue.receive_frame(pixel(2, 0, 0, 2)).unwrap();
    queue.complete_update(2, 1).unwrap();
    let presentation = take_presentation(&worker);
    assert!(presentation.full);
    assert_eq!(presentation.id.epoch, 2);
    assert_eq!(&presentation.regions[0].pixels[..], &[2, 2, 2, 255]);
    worker.shutdown();
}

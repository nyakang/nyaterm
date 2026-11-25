use std::collections::VecDeque;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::frame::{FramebufferLimits, VNC_FRAMEBUFFER_LIMITS};
use crate::heartbeat::Heartbeat;
use crate::ipc::FRAME_PAYLOAD_LIMIT;
use crate::protocol::{QueueWaker, RemoteFrameEvent, VncError, VncErrorKind};

use super::compositor::Compositor;
use super::{VncPresentation, VncPresentationConfig, VncPresentationId, VncPresentationMetrics};

const INGRESS_ITEMS: usize = 64;

enum Command {
    Clear,
    Frame(RemoteFrameEvent),
    Complete { epoch: u64, sequence: u64 },
}

impl Command {
    fn bytes(&self) -> usize {
        match self {
            Self::Frame(RemoteFrameEvent::Bitmap { pixels, .. }) => pixels.len(),
            _ => 0,
        }
    }
}

struct Mailbox {
    commands: VecDeque<Command>,
    ingress_bytes: usize,
    epoch: Option<u64>,
    visible: bool,
    generation: u64,
    interval: Duration,
    acknowledged: Option<VncPresentationId>,
    taken: Option<VncPresentationId>,
    presentation: Option<VncPresentation>,
    metrics: VncPresentationMetrics,
    closed: bool,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    changed: Condvar,
    heartbeat: Arc<Heartbeat>,
}

#[derive(Clone)]
pub(crate) struct PresentationClient(Arc<Shared>);

impl PresentationClient {
    pub(crate) fn frame(&self, frame: RemoteFrameEvent) -> Result<bool, VncError> {
        self.submit(Command::Frame(frame))
    }

    pub(crate) fn complete(&self, epoch: u64, sequence: u64) -> Result<bool, VncError> {
        self.submit(Command::Complete { epoch, sequence })
    }

    fn submit(&self, command: Command) -> Result<bool, VncError> {
        let bytes = command.bytes();
        if bytes > FRAME_PAYLOAD_LIMIT {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "remote desktop ingress pixels exceed the byte limit",
            ));
        }
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        let blocked_at = Instant::now();
        let mut blocked = false;
        while !state.closed
            && (state.commands.len() >= INGRESS_ITEMS
                || state.ingress_bytes.saturating_add(bytes) > FRAME_PAYLOAD_LIMIT)
        {
            blocked = true;
            self.0.heartbeat.blocked(true);
            state = self
                .0
                .changed
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
            self.0.heartbeat.blocked(false);
        }
        if blocked {
            state.metrics.ingress_blocked_nanoseconds = state
                .metrics
                .ingress_blocked_nanoseconds
                .saturating_add(u64::try_from(blocked_at.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
        if state.closed {
            return Ok(false);
        }
        if let Command::Frame(RemoteFrameEvent::Reset { epoch, .. }) = &command {
            state.epoch = Some(*epoch);
            state.presentation = None;
            state.taken = None;
            state.acknowledged = None;
        }
        state.ingress_bytes += bytes;
        state.commands.push_back(command);
        state.metrics.ingress_items_high_water = state
            .metrics
            .ingress_items_high_water
            .max(state.commands.len());
        state.metrics.ingress_bytes_high_water = state
            .metrics
            .ingress_bytes_high_water
            .max(state.ingress_bytes);
        drop(state);
        self.0.changed.notify_all();
        Ok(true)
    }

    pub(crate) fn set_visible(&self, visible: bool, config: VncPresentationConfig) {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        if state.visible == visible && state.interval == config.frame_interval {
            return;
        }
        if state.visible != visible {
            state.visible = visible;
            state.generation = state.generation.saturating_add(1);
            state.presentation = None;
            state.taken = None;
            state.acknowledged = None;
        }
        state.interval = config.frame_interval.max(Duration::from_millis(1));
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        state.generation = state.generation.saturating_add(1);
        state.presentation = None;
        state.taken = None;
        state.acknowledged = None;
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn discard(&self) {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return;
        }
        state.epoch = None;
        state.generation = state.generation.saturating_add(1);
        state.presentation = None;
        state.taken = None;
        state.acknowledged = None;
        state.commands.clear();
        state.ingress_bytes = 0;
        state.commands.push_back(Command::Clear);
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn take(&self) -> Option<VncPresentation> {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        let presentation = state.presentation.take()?;
        if state.closed
            || !state.visible
            || state.epoch != Some(presentation.id.epoch)
            || state.generation != presentation.id.generation
        {
            return None;
        }
        state.taken = Some(presentation.id);
        Some(presentation)
    }

    pub(crate) fn acknowledge(&self, id: VncPresentationId) {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed || state.taken != Some(id) {
            return;
        }
        state.acknowledged = Some(id);
        state.taken = None;
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn metrics(&self) -> VncPresentationMetrics {
        self.0
            .mailbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .metrics
    }

    pub(crate) fn close(&self) {
        let mut state = self.0.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        state.commands.clear();
        state.ingress_bytes = 0;
        state.presentation = None;
        state.taken = None;
        state.acknowledged = None;
        drop(state);
        self.0.changed.notify_all();
    }
}

pub(crate) struct PresentationWorker {
    pub(crate) client: PresentationClient,
    thread: Option<JoinHandle<()>>,
}

impl PresentationWorker {
    pub(crate) fn spawn(
        name: String,
        config: VncPresentationConfig,
        heartbeat: Arc<Heartbeat>,
        wake: Option<QueueWaker>,
        failed: impl Fn(VncError) + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_limits(
            name,
            config,
            VNC_FRAMEBUFFER_LIMITS,
            heartbeat,
            wake,
            failed,
        )
    }

    pub(crate) fn spawn_with_limits(
        name: String,
        config: VncPresentationConfig,
        limits: FramebufferLimits,
        heartbeat: Arc<Heartbeat>,
        wake: Option<QueueWaker>,
        failed: impl Fn(VncError) + Send + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox {
                commands: VecDeque::new(),
                ingress_bytes: 0,
                epoch: None,
                visible: false,
                generation: 1,
                interval: config.frame_interval.max(Duration::from_millis(1)),
                acknowledged: None,
                taken: None,
                presentation: None,
                metrics: Default::default(),
                closed: false,
            }),
            changed: Condvar::new(),
            heartbeat,
        });
        let client = PresentationClient(Arc::clone(&shared));
        let worker_client = client.clone();
        let thread = nyaterm_core::runtime::worker::spawn(name, move || {
            let result = catch_unwind(AssertUnwindSafe(|| run(&shared, wake, limits)));
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some(VncError::new(
                    VncErrorKind::Internal,
                    "remote desktop presentation worker panicked",
                )),
            };
            if let Some(error) = error {
                worker_client.close();
                failed(error);
            }
        })?;
        Ok(Self {
            client,
            thread: Some(thread),
        })
    }

    pub(crate) fn shutdown(&mut self) {
        self.client.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for PresentationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    shared: &Shared,
    wake: Option<QueueWaker>,
    limits: FramebufferLimits,
) -> Result<(), VncError> {
    let mut compositor = Compositor::with_limits(limits);
    let mut published = None;
    let mut published_at: Option<Instant> = None;
    let mut observed_generation = 0;
    loop {
        let (command, visible, generation, interval, ack) = {
            let mut state = shared.mailbox.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if state.closed {
                    return Ok(());
                }
                let delay = published_at
                    .map(|time| state.interval.saturating_sub(time.elapsed()))
                    .unwrap_or_default();
                if !state.commands.is_empty()
                    || state.acknowledged.is_some()
                    || state.generation != observed_generation
                    || (state.visible
                        && compositor.ready()
                        && compositor.id() != published
                        && delay.is_zero())
                {
                    break;
                }
                if state.visible && compositor.ready() && compositor.id() != published {
                    state = shared
                        .changed
                        .wait_timeout(state, delay)
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                } else {
                    state = shared
                        .changed
                        .wait(state)
                        .unwrap_or_else(|p| p.into_inner());
                }
            }
            let command = state.commands.pop_front();
            state.ingress_bytes = state
                .ingress_bytes
                .saturating_sub(command.as_ref().map_or(0, Command::bytes));
            (
                command,
                state.visible,
                state.generation,
                state.interval,
                state.acknowledged.take(),
            )
        };
        shared.changed.notify_all();
        observed_generation = generation;
        compositor.set_generation(generation);
        if let Some(id) = ack {
            compositor.acknowledge(id);
        }
        if let Some(command) = command {
            let start = Instant::now();
            let (rectangles, completed_updates) = match command {
                Command::Clear => {
                    compositor = Compositor::with_limits(limits);
                    compositor.set_generation(generation);
                    published = None;
                    published_at = None;
                    (0, 0)
                }
                Command::Frame(RemoteFrameEvent::Reset {
                    epoch,
                    width,
                    height,
                }) => {
                    compositor.reset(epoch, width, height)?;
                    published = None;
                    published_at = None;
                    (0, 0)
                }
                Command::Frame(frame) => {
                    compositor.apply(&frame)?;
                    (1, 0)
                }
                Command::Complete { epoch, sequence } => {
                    compositor.complete(epoch, sequence)?;
                    (0, 1)
                }
            };
            let mut state = shared.mailbox.lock().unwrap_or_else(|p| p.into_inner());
            state.metrics.rectangles += rectangles;
            state.metrics.completed_updates += completed_updates;
            state.metrics.compose_nanoseconds = state
                .metrics
                .compose_nanoseconds
                .saturating_add(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
        if visible
            && compositor.ready()
            && compositor.id() != published
            && published_at.is_none_or(|time| time.elapsed() >= interval)
        {
            let presentation = compositor.prepare()?;
            let mut state = shared.mailbox.lock().unwrap_or_else(|p| p.into_inner());
            if !state.closed
                && state.visible
                && state.generation == presentation.id.generation
                && state.epoch == Some(presentation.id.epoch)
            {
                state.metrics.publications += 1;
                state.metrics.replaced_publications += u64::from(state.presentation.is_some());
                state.metrics.packed_bytes += presentation
                    .regions
                    .iter()
                    .map(|r| r.pixels.len() as u64)
                    .sum::<u64>();
                published = Some(presentation.id);
                published_at = Some(Instant::now());
                state.presentation = Some(presentation);
                drop(state);
                if let Some(wake) = &wake {
                    wake();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{INGRESS_ITEMS, PresentationWorker};
    use crate::heartbeat::Heartbeat;
    use crate::pixel_memory::{PixelBudget, PixelBuffer};
    use crate::presentation::VncPresentationConfig;
    use crate::protocol::{PixelFormat, RemoteFrameEvent, VncErrorKind};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn worker() -> PresentationWorker {
        PresentationWorker::spawn(
            "vnc-presentation-test".into(),
            VncPresentationConfig::default(),
            Arc::new(Heartbeat::default()),
            None,
            |error| panic!("{error}"),
        )
        .unwrap()
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "presentation worker made no progress"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn hidden_worker_composes_thousands_of_rectangles_without_a_ui_consumer_and_resumes_full() {
        let mut worker = worker();
        worker
            .client
            .frame(RemoteFrameEvent::Reset {
                epoch: 1,
                width: 256,
                height: 256,
            })
            .unwrap();
        let budget = PixelBudget::test(4);
        let lease = Arc::new(budget.reserve(4).unwrap());
        let pixels = PixelBuffer::with_lease(vec![3, 2, 1, 255], Some(lease));
        for index in 0..1000 {
            worker
                .client
                .frame(RemoteFrameEvent::Bitmap {
                    epoch: 1,
                    full: false,
                    x: index % 256,
                    y: index / 256,
                    width: 1,
                    height: 1,
                    stride: 4,
                    format: PixelFormat::Bgra8,
                    pixels: pixels.clone(),
                })
                .unwrap();
        }
        drop(pixels);
        worker.client.complete(1, 1).unwrap();
        wait_until(|| worker.client.metrics().completed_updates == 1);
        assert_eq!(
            budget.used(),
            0,
            "composed ingress releases its original lease"
        );
        assert_eq!(
            worker.client.metrics().publications,
            0,
            "hidden sessions never pack pixels"
        );
        worker
            .client
            .set_visible(true, VncPresentationConfig::default());
        let mut presentation = None;
        wait_until(|| {
            presentation = worker.client.take();
            presentation.is_some()
        });
        let presentation = presentation.unwrap();
        assert!(presentation.full);
        assert!(
            presentation.regions[0].pixels[..4000]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [3, 2, 1, 255])
        );
        worker.client.acknowledge(presentation.id);
        worker
            .client
            .set_visible(false, VncPresentationConfig::default());
        worker
            .client
            .set_visible(true, VncPresentationConfig::default());
        let mut resumed = None;
        wait_until(|| {
            resumed = worker.client.take();
            resumed.is_some()
        });
        let resumed = resumed.unwrap();
        assert!(resumed.full);
        assert!(resumed.id.generation > presentation.id.generation);
        worker.shutdown();
        assert!(worker.client.take().is_none());
        assert!(!worker.client.complete(1, 2).unwrap());
    }

    #[test]
    fn invalid_geometry_reports_a_typed_failure_and_closes_ingress() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        let mut worker = PresentationWorker::spawn(
            "vnc-presentation-invalid".into(),
            VncPresentationConfig::default(),
            Arc::new(Heartbeat::default()),
            None,
            move |error| sink.lock().unwrap().push(error),
        )
        .unwrap();
        worker
            .client
            .frame(RemoteFrameEvent::Reset {
                epoch: 1,
                width: 0,
                height: 1,
            })
            .unwrap();
        wait_until(|| !errors.lock().unwrap().is_empty());
        assert_eq!(errors.lock().unwrap()[0].kind, VncErrorKind::Protocol);
        assert!(!worker.client.complete(1, 1).unwrap());
        worker.shutdown();
    }

    #[test]
    fn shutdown_releases_queued_pixels_and_unblocks_a_full_ingress_producer() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let mut worker = PresentationWorker::spawn(
            "vnc-presentation-backpressure".into(),
            VncPresentationConfig::default(),
            Arc::new(Heartbeat::default()),
            Some(Arc::new(move || {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
            })),
            |error| panic!("{error}"),
        )
        .unwrap();
        worker
            .client
            .set_visible(true, VncPresentationConfig::default());
        worker
            .client
            .frame(RemoteFrameEvent::Reset {
                epoch: 1,
                width: 1,
                height: 1,
            })
            .unwrap();
        worker.client.complete(1, 1).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        // Hold the worker at publication so queue occupancy is deterministic.
        let budget = PixelBudget::test(4);
        let frame = RemoteFrameEvent::Bitmap {
            epoch: 1,
            full: false,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Bgra8,
            pixels: PixelBuffer::with_lease(
                vec![1, 2, 3, 255],
                Some(Arc::new(budget.reserve(4).unwrap())),
            ),
        };
        for _ in 0..INGRESS_ITEMS {
            worker.client.frame(frame.clone()).unwrap();
        }
        drop(frame);
        let client = worker.client.clone();
        let producer = std::thread::spawn(move || client.complete(1, 2));
        {
            let mailbox = worker.client.0.mailbox.lock().unwrap();
            assert_eq!(mailbox.commands.len(), INGRESS_ITEMS);
            assert_eq!(mailbox.ingress_bytes, INGRESS_ITEMS * 4);
        }
        worker.client.close();
        assert!(!producer.join().unwrap().unwrap());
        assert_eq!(budget.used(), 0);
        release_tx.send(()).unwrap();
        worker.shutdown();
    }
}

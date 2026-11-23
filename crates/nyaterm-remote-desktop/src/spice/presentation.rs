//! One background compositor per SPICE session, regardless of display count.
//! Unacknowledged damage is cumulative; replacing a mailbox image loses no delta.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::heartbeat::Heartbeat;
use crate::presentation::compositor::Compositor;
use crate::presentation::{RemotePresentation, RemotePresentationId};
use crate::spice::display::{SPICE_FRAMEBUFFER_LIMITS, SpiceDisplayId, SpiceMonitor};
use crate::spice::protocol::{SpiceControl, SpiceErrorKind};
use crate::{DirtyRect, QueueWaker, RemoteFrameEvent};

#[cfg(test)]
mod benchmarks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpicePresentationId {
    pub display: SpiceDisplayId,
    pub frame: RemotePresentationId,
}

#[derive(Clone, Debug)]
pub struct SpicePresentation {
    pub display: SpiceMonitor,
    pub frame: RemotePresentation,
}

impl SpicePresentation {
    pub fn id(&self) -> SpicePresentationId {
        SpicePresentationId {
            display: self.display.id,
            frame: self.frame.id,
        }
    }
}

pub(crate) enum Command {
    Reset {
        channel: u32,
        epoch: u64,
        width: u32,
        height: u32,
    },
    Remove(u32),
    Monitors(Vec<SpiceMonitor>),
    Frame(u32, RemoteFrameEvent),
}

impl Command {
    fn bytes(&self) -> usize {
        match self {
            Self::Frame(_, RemoteFrameEvent::Bitmap { pixels, .. }) => pixels.len(),
            _ => 0,
        }
    }
}

struct Mailbox {
    commands: VecDeque<Command>,
    bytes: usize,
    visible: bool,
    selected: Option<SpiceDisplayId>,
    interval: Duration,
    generation: u64,
    presentation: Option<SpicePresentation>,
    taken: Option<SpicePresentationId>,
    acknowledged: Option<SpicePresentationId>,
    closed: bool,
}

struct Shared {
    state: Mutex<Mailbox>,
    changed: Condvar,
    heartbeat: Arc<Heartbeat>,
}

#[derive(Clone)]
pub(crate) struct PresentationClient(Arc<Shared>);

impl PresentationClient {
    pub(crate) fn control(&self, control: &SpiceControl) -> Result<bool, SpiceErrorKind> {
        let command = match control {
            SpiceControl::SurfaceReset {
                channel,
                epoch,
                width,
                height,
            } => Command::Reset {
                channel: *channel,
                epoch: *epoch,
                width: *width,
                height: *height,
            },
            SpiceControl::SurfaceRemoved { channel } => Command::Remove(*channel),
            SpiceControl::Monitors { monitors } => Command::Monitors(monitors.clone()),
            _ => return Ok(true),
        };
        self.submit(command)
    }

    pub(crate) fn frame(
        &self,
        channel: u32,
        frame: RemoteFrameEvent,
    ) -> Result<bool, SpiceErrorKind> {
        self.submit(Command::Frame(channel, frame))
    }

    fn submit(&self, command: Command) -> Result<bool, SpiceErrorKind> {
        let bytes = command.bytes();
        if bytes > 4 * 1024 * 1024 {
            return Err(SpiceErrorKind::ResourceLimit);
        }
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        while !state.closed
            && (state.commands.len() >= 64 || state.bytes + bytes > 16 * 1024 * 1024)
        {
            self.0.heartbeat.blocked(true);
            state = self
                .0
                .changed
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
            self.0.heartbeat.blocked(false);
        }
        if state.closed {
            return Ok(false);
        }
        if matches!(
            command,
            Command::Reset { .. } | Command::Remove(_) | Command::Monitors(_)
        ) {
            state.generation = state.generation.saturating_add(1);
            state.presentation = None;
            state.taken = None;
            state.acknowledged = None;
        }
        state.bytes += bytes;
        state.commands.push_back(command);
        drop(state);
        self.0.changed.notify_all();
        Ok(true)
    }

    pub(crate) fn configure(
        &self,
        visible: bool,
        selected: Option<SpiceDisplayId>,
        interval: Duration,
    ) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.visible != visible || state.selected != selected {
            state.visible = visible;
            state.selected = selected;
            state.generation = state.generation.saturating_add(1);
            state.presentation = None;
            state.taken = None;
            state.acknowledged = None;
        }
        state.interval = interval.clamp(Duration::from_millis(8), Duration::from_secs(1));
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn take(&self) -> Option<SpicePresentation> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        let frame = state.presentation.take()?;
        state.taken = Some(frame.id());
        Some(frame)
    }

    pub(crate) fn acknowledge(&self, id: SpicePresentationId) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.taken == Some(id) && id.frame.generation == state.generation {
            state.acknowledged = Some(id);
            state.taken = None;
        }
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.generation = state.generation.saturating_add(1);
        state.presentation = None;
        state.taken = None;
        state.acknowledged = None;
        drop(state);
        self.0.changed.notify_all();
    }

    pub(crate) fn close(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        state.commands.clear();
        state.bytes = 0;
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
        heartbeat: Arc<Heartbeat>,
        wake: Option<QueueWaker>,
        failed: impl Fn(SpiceErrorKind) + Send + 'static,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(Mailbox {
                commands: VecDeque::new(),
                bytes: 0,
                visible: false,
                selected: None,
                interval: Duration::from_millis(16),
                generation: 1,
                presentation: None,
                taken: None,
                acknowledged: None,
                closed: false,
            }),
            changed: Condvar::new(),
            heartbeat,
        });
        let client = PresentationClient(shared.clone());
        let worker_client = client.clone();
        let thread =
            nyaterm_core::runtime::worker::spawn("spice-presentation".into(), move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&shared, wake)))
                        .unwrap_or(Err(SpiceErrorKind::HelperCrash));
                if let Err(error) = result {
                    worker_client.close();
                    failed(error);
                }
            })?;
        Ok(Self {
            client,
            thread: Some(thread),
        })
    }
}

impl Drop for PresentationWorker {
    fn drop(&mut self) {
        self.client.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Displays {
    surfaces: BTreeMap<u32, Compositor>,
    monitors: Vec<SpiceMonitor>,
    sequence: u64,
}

impl Displays {
    fn apply(&mut self, command: Command) -> Result<(), SpiceErrorKind> {
        match command {
            Command::Reset {
                channel,
                epoch,
                width,
                height,
            } => {
                self.surfaces.remove(&channel);
                let bytes: u64 = self
                    .surfaces
                    .values()
                    .filter_map(Compositor::bounds)
                    .map(|r| u64::from(r.width) * u64::from(r.height) * 4)
                    .sum();
                if self.surfaces.len() >= 64
                    || bytes + u64::from(width) * u64::from(height) * 4 > 512 * 1024 * 1024
                {
                    return Err(SpiceErrorKind::ResourceLimit);
                }
                let mut compositor = Compositor::with_limits(SPICE_FRAMEBUFFER_LIMITS);
                compositor.reset(epoch, width, height).map_err(map_error)?;
                // A reset publishes an opaque cleared surface even before damage.
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or(SpiceErrorKind::Protocol)?;
                compositor
                    .complete(epoch, self.sequence)
                    .map_err(map_error)?;
                self.surfaces.insert(channel, compositor);
            }
            Command::Remove(channel) => {
                self.surfaces.remove(&channel);
            }
            Command::Monitors(monitors) => {
                self.monitors = monitors;
            }
            Command::Frame(channel, frame) => {
                let surface = self
                    .surfaces
                    .get_mut(&channel)
                    .ok_or(SpiceErrorKind::Protocol)?;
                surface.apply(&frame).map_err(map_error)?;
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or(SpiceErrorKind::Protocol)?;
                if let Some(id) = surface.id() {
                    surface
                        .complete(id.epoch, self.sequence)
                        .map_err(map_error)?;
                }
            }
        }
        Ok(())
    }

    fn selected(&self, requested: Option<SpiceDisplayId>) -> Option<SpiceMonitor> {
        let valid = |m: &&SpiceMonitor| {
            m.surface == 0
                && self
                    .surfaces
                    .get(&m.id.channel)
                    .and_then(Compositor::bounds)
                    .is_some_and(|b| {
                        u64::from(m.x) + u64::from(m.width) <= u64::from(b.width)
                            && u64::from(m.y) + u64::from(m.height) <= u64::from(b.height)
                    })
        };
        if let Some(monitor) = self
            .monitors
            .iter()
            .filter(valid)
            .find(|m| Some(m.id) == requested)
            .or_else(|| self.monitors.iter().find(valid))
        {
            return Some(monitor.clone());
        }
        // No-agent boot consoles do not necessarily send monitor configuration.
        if !self.monitors.is_empty() {
            return None;
        }
        let (&channel, surface) = self.surfaces.iter().next()?;
        let bounds = surface.bounds()?;
        Some(SpiceMonitor {
            id: SpiceDisplayId {
                channel,
                monitor: 0,
            },
            surface: 0,
            x: 0,
            y: 0,
            width: bounds.width,
            height: bounds.height,
        })
    }
}

fn run(shared: &Shared, wake: Option<QueueWaker>) -> Result<(), SpiceErrorKind> {
    let mut displays = Displays::default();
    let mut published: Option<SpicePresentationId> = None;
    let mut last_publication: Option<Instant> = None;
    loop {
        let (command, visible, selected, generation, interval, ack) = {
            let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Ok(());
            }
            let command = state.commands.pop_front();
            if let Some(command) = &command {
                state.bytes -= command.bytes();
            }
            (
                command,
                state.visible,
                state.selected,
                state.generation,
                state.interval,
                state.acknowledged.take(),
            )
        };
        shared.changed.notify_all();
        if let Some(ack) = ack
            && let Some(surface) = displays.surfaces.get_mut(&ack.display.channel)
        {
            surface.acknowledge(ack.frame);
        }
        let had_command = command.is_some();
        if let Some(command) = command {
            displays.apply(command)?;
        }
        let mut due = None;
        if visible && let Some(display) = displays.selected(selected) {
            let surface = displays
                .surfaces
                .get_mut(&display.id.channel)
                .expect("selected surface exists");
            surface.set_generation(generation);
            let id = surface.id().map(|frame| SpicePresentationId {
                display: display.id,
                frame,
            });
            if surface.ready() && id != published {
                let delay = last_publication
                    .map(|t| interval.saturating_sub(t.elapsed()))
                    .unwrap_or_default();
                if delay.is_zero() {
                    let frame = surface
                        .prepare_crop(DirtyRect {
                            x: display.x,
                            y: display.y,
                            width: display.width,
                            height: display.height,
                        })
                        .map_err(map_error)?;
                    let output = SpicePresentation { display, frame };
                    let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                    // A selection/reset can arrive while pixels are being packed.
                    if state.generation == generation
                        && state.visible
                        && !state.closed
                        && !state.commands.iter().any(|c| {
                            matches!(
                                c,
                                Command::Reset { .. } | Command::Remove(_) | Command::Monitors(_)
                            )
                        })
                    {
                        published = Some(output.id());
                        state.presentation = Some(output);
                        last_publication = Some(Instant::now());
                        drop(state);
                        if let Some(wake) = &wake {
                            wake();
                        }
                    }
                } else {
                    due = Some(delay);
                }
            }
        }
        if had_command {
            continue;
        }
        let state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Ok(());
        }
        if !state.commands.is_empty()
            || state.acknowledged.is_some()
            || state.generation != generation
            || state.interval != interval
        {
            continue;
        }
        if let Some(delay) = due {
            drop(
                shared
                    .changed
                    .wait_timeout(state, delay)
                    .unwrap_or_else(|p| p.into_inner()),
            );
        } else {
            drop(
                shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner()),
            );
        }
    }
}

fn map_error(error: crate::VncError) -> SpiceErrorKind {
    match error.kind {
        crate::VncErrorKind::ResourceExhausted => SpiceErrorKind::ResourceLimit,
        _ => SpiceErrorKind::Protocol,
    }
}

#[cfg(test)]
mod tests {
    use crate::spice::display::{SpiceDisplayId, SpiceMonitor};
    use crate::spice::presentation::{Command, Displays, PresentationWorker};
    use crate::{PixelFormat, RemoteFrameEvent};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    fn monitor(id: u32, x: u32) -> SpiceMonitor {
        SpiceMonitor {
            id: SpiceDisplayId {
                channel: 0,
                monitor: id,
            },
            surface: 0,
            x,
            y: 0,
            width: 64,
            height: 2,
        }
    }

    fn bitmap(epoch: u64, x: u32, width: u32, value: u8) -> RemoteFrameEvent {
        RemoteFrameEvent::Bitmap {
            epoch,
            full: false,
            x,
            y: 0,
            width,
            height: 2,
            stride: width * 4,
            format: PixelFormat::Bgra8,
            pixels: vec![value; width as usize * 8].into(),
        }
    }

    #[test]
    fn monitor_crops_have_local_coordinates_and_keep_unacknowledged_changes() {
        let mut displays = Displays::default();
        displays
            .apply(Command::Reset {
                channel: 0,
                epoch: 1,
                width: 128,
                height: 2,
            })
            .unwrap();
        displays
            .apply(Command::Monitors(vec![monitor(0, 0), monitor(1, 64)]))
            .unwrap();
        displays
            .apply(Command::Frame(0, bitmap(1, 0, 64, 17)))
            .unwrap();
        displays
            .apply(Command::Frame(0, bitmap(1, 64, 64, 33)))
            .unwrap();
        let screen = displays.selected(Some(monitor(1, 64).id)).unwrap();
        let surface = displays.surfaces.get_mut(&0).unwrap();
        surface.set_generation(2);
        let crop = crate::DirtyRect {
            x: screen.x,
            y: screen.y,
            width: screen.width,
            height: screen.height,
        };
        let first = surface.prepare_crop(crop).unwrap();
        assert_eq!((first.width, first.height), (64, 2));
        assert_eq!(first.regions[0].rect.x, 0);
        assert!(first.regions[0].pixels.iter().all(|v| *v == 33));
        surface.acknowledge(first.id);
        displays
            .apply(Command::Frame(0, bitmap(1, 64, 1, 55)))
            .unwrap();
        let surface = displays.surfaces.get_mut(&0).unwrap();
        let dropped = surface.prepare_crop(crop).unwrap();
        // Replacing the unconsumed publication retains its dirty pixels.
        let replacement = surface.prepare_crop(crop).unwrap();
        assert_eq!(dropped.regions[0].pixels, replacement.regions[0].pixels);
        assert_eq!(&replacement.regions[0].pixels[..4], &[55; 4]);
        surface.set_generation(3);
        let left = surface
            .prepare_crop(crate::DirtyRect { x: 0, ..crop })
            .unwrap();
        assert!(left.full);
        assert!(left.regions[0].pixels.iter().all(|v| *v == 17));
    }

    #[test]
    fn no_agent_console_uses_surface_and_removal_never_leaves_a_stale_selection() {
        let mut displays = Displays::default();
        displays
            .apply(Command::Reset {
                channel: 0,
                epoch: 1,
                width: 32,
                height: 32,
            })
            .unwrap();
        assert_eq!(displays.selected(None).unwrap().width, 32);
        displays.apply(Command::Remove(0)).unwrap();
        assert!(displays.selected(None).is_none());
    }

    #[test]
    fn final_damage_is_published_on_timer_and_hidden_resume_requires_full_upload() {
        let (wake, recv) = mpsc::channel();
        let worker = PresentationWorker::spawn(
            Arc::default(),
            Some(Arc::new(move || {
                let _ = wake.send(());
            })),
            |error| panic!("{error:?}"),
        )
        .unwrap();
        worker
            .client
            .configure(true, None, Duration::from_millis(30));
        worker
            .client
            .submit(Command::Reset {
                channel: 0,
                epoch: 1,
                width: 128,
                height: 2,
            })
            .unwrap();
        recv.recv_timeout(Duration::from_secs(2)).unwrap();
        let first = worker.client.take().unwrap();
        worker.client.acknowledge(first.id());
        worker.client.frame(0, bitmap(1, 0, 1, 77)).unwrap();
        recv.recv_timeout(Duration::from_secs(2)).unwrap();
        let final_frame = worker.client.take().unwrap();
        assert_eq!(&final_frame.frame.regions[0].pixels[..4], &[77; 4]);
        worker.client.acknowledge(final_frame.id());
        worker
            .client
            .configure(false, None, Duration::from_millis(30));
        worker.client.frame(0, bitmap(1, 64, 1, 88)).unwrap();
        worker
            .client
            .configure(true, None, Duration::from_millis(30));
        recv.recv_timeout(Duration::from_secs(2)).unwrap();
        let resumed = worker.client.take().unwrap();
        assert!(resumed.frame.full);
        assert_eq!(&resumed.frame.regions[0].pixels[64 * 4..65 * 4], &[88; 4]);
    }
}

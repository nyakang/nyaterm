//! Process ownership and bounded ordered delivery; contains no decoder or GPUI.

mod queue;
mod reader;

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use nyaterm_core::runtime::reaper::Reaper;

use crate::heartbeat::Supervisor;
use crate::helper_process::{self, IpcWriter, StderrCapture};
use crate::spice::ipc::encode_control;
use crate::spice::protocol::{
    SPICE_PROTOCOL_VERSION, SpiceControl, SpiceErrorKind, SpiceInput, SpiceSessionConfig,
};
use crate::spice::session::queue::EventQueue;
use crate::{QueueWaker, RemoteCursorEvent, RemoteFrameEvent};

pub enum SpiceEvent {
    Control(SpiceControl),
    Frame {
        channel: u32,
        event: RemoteFrameEvent,
    },
    Cursor {
        channel: u32,
        event: RemoteCursorEvent,
    },
}

struct Record {
    _component: Option<nyaterm_core::plugins::native::NativeLaunch>,
    queue: Arc<EventQueue>,
    writer: Arc<IpcWriter>,
    child: Arc<Mutex<Option<Child>>>,
    supervisor: Option<Supervisor>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<StderrCapture>,
    presentation: Option<crate::spice::presentation::PresentationWorker>,
    // Last field: shutdown waits through destruction of every worker above.
    _lifetime: RecordLifetime,
}

#[derive(Default)]
struct LiveState {
    closed: bool,
    count: usize,
}

#[derive(Default)]
struct LiveRecords {
    state: Mutex<LiveState>,
    changed: Condvar,
}

struct RecordLifetime(Arc<LiveRecords>);

impl RecordLifetime {
    fn begin(live: &Arc<LiveRecords>) -> Result<Self, SpiceErrorKind> {
        let mut state = live.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err(SpiceErrorKind::Connection);
        }
        state.count += 1;
        Ok(Self(live.clone()))
    }
}

impl Drop for RecordLifetime {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.count -= 1;
        self.0.changed.notify_all();
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(presentation) = &self.presentation {
            presentation.client.close();
        }
        let _ = send_critical(&self.writer, SpiceControl::Disconnect);
        if let Some(mut supervisor) = self.supervisor.take() {
            supervisor.stop();
        }
        helper_process::cleanup_child(
            &mut self.child.lock().unwrap_or_else(|p| p.into_inner()),
            &mut self.reader,
        );
        if let Some(stderr) = self.stderr.take() {
            stderr.finish();
        }
    }
}

#[derive(Default)]
pub struct SpiceSessionManager {
    sessions: Mutex<HashMap<String, Record>>,
    reaper: Mutex<Option<Reaper<Record>>>,
    waker: Mutex<Option<QueueWaker>>,
    presentation_enabled: bool,
    live: Arc<LiveRecords>,
}

impl SpiceSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_presentation_pipeline() -> Self {
        let mut manager = Self::default();
        manager.presentation_enabled = true;
        manager
    }

    pub fn contains(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(id)
    }

    /// Blocking fallback for an abandoned start when the reaper is saturated.
    pub fn close(&self, id: &str) {
        let record = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        drop(record);
    }

    /// Install before creating sessions. The notifier should only wake a task.
    pub fn set_queue_waker(&self, waker: QueueWaker) {
        *self.waker.lock().unwrap_or_else(|p| p.into_inner()) = Some(waker);
    }

    /// Locate the SPICE helper beside the executable (or via
    /// `NYATERM_SPICE_HELPER`). Packaged builds install it as a native
    /// component instead.
    pub fn resolve_spice_helper_path() -> Result<std::path::PathBuf, SpiceErrorKind> {
        helper_process::resolve_helper("nyaterm-spice-helper", "NYATERM_SPICE_HELPER")
            .map_err(|_| SpiceErrorKind::RuntimeMissing)
    }

    /// Call on a background worker: process creation and rollback may block.
    /// Each call creates a new generation; reconnect policy belongs to desktop.
    pub fn create_session(&self, config: SpiceSessionConfig) -> Result<String, SpiceErrorKind> {
        let path = Self::resolve_spice_helper_path()?;
        self.create_session_with_helper(config, &path)
    }

    /// Explicit helper path supports packaged runtimes and integration fixtures.
    pub fn create_session_with_helper(
        &self,
        config: SpiceSessionConfig,
        path: &Path,
    ) -> Result<String, SpiceErrorKind> {
        self.create_session_with_component(config, path, None)
    }
    pub fn create_session_with_launch(
        &self,
        config: SpiceSessionConfig,
        component: nyaterm_core::plugins::native::NativeLaunch,
    ) -> Result<String, SpiceErrorKind> {
        let path = component.executable.clone();
        self.create_session_with_component(config, &path, Some(component))
    }
    fn create_session_with_component(
        &self,
        config: SpiceSessionConfig,
        path: &Path,
        component: Option<nyaterm_core::plugins::native::NativeLaunch>,
    ) -> Result<String, SpiceErrorKind> {
        config
            .validate()
            .map_err(|_| SpiceErrorKind::Configuration)?;
        let lifetime = RecordLifetime::begin(&self.live)?;
        self.ensure_reaper()?;
        let id = config.session_id.clone();
        if self
            .sessions
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?
            .contains_key(&id)
        {
            return Err(SpiceErrorKind::Configuration);
        }
        let helper_process::HelperProcess {
            mut child,
            stdin,
            stdout,
            stderr,
        } = helper_process::spawn_helper(path, "spice").map_err(|_| SpiceErrorKind::HelperCrash)?;
        let writer = match IpcWriter::spawn(stdin, "nyaterm-spice-writer".into()) {
            Ok(writer) => Arc::new(writer),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                stderr.finish();
                return Err(SpiceErrorKind::HelperCrash);
            }
        };
        let queue = Arc::new(EventQueue::new(
            self.waker.lock().ok().and_then(|w| w.clone()),
        ));
        let mut record = Record {
            _component: component,
            queue: queue.clone(),
            writer: writer.clone(),
            child: Arc::new(Mutex::new(Some(child))),
            supervisor: None,
            reader: None,
            stderr: Some(stderr),
            presentation: None,
            _lifetime: lifetime,
        };
        if self.presentation_enabled {
            let failed_queue = queue.clone();
            record.presentation = Some(
                crate::spice::presentation::PresentationWorker::spawn(
                    queue.heartbeat.clone(),
                    self.waker.lock().ok().and_then(|w| w.clone()),
                    move |error| failed_queue.fail(error),
                )
                .map_err(|_| SpiceErrorKind::HelperCrash)?,
            );
        }
        record.reader = Some(
            reader::spawn(
                stdout,
                queue.clone(),
                writer.clone(),
                config,
                record.presentation.as_ref().map(|p| p.client.clone()),
            )
            .map_err(|_| SpiceErrorKind::HelperCrash)?,
        );
        let timeout_queue = queue.clone();
        record.supervisor = Some(
            Supervisor::spawn(
                queue.heartbeat.clone(),
                record.child.clone(),
                move |sequence| {
                    let _ = send_critical(&writer, SpiceControl::Ping { sequence });
                },
                move || timeout_queue.fail(SpiceErrorKind::Timeout),
            )
            .map_err(|_| SpiceErrorKind::HelperCrash)?,
        );
        send_critical(
            &record.writer,
            SpiceControl::Hello {
                version: SPICE_PROTOCOL_VERSION,
            },
        )?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?;
        if sessions.contains_key(&id)
            || self
                .live
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .closed
        {
            drop(sessions);
            return Err(SpiceErrorKind::Configuration);
        }
        sessions.insert(id.clone(), record);
        Ok(id)
    }

    /// Nonblocking UI-side commands. Input overflow queues ReleaseAll before
    /// accepting later keys, using the shared helper mailbox recovery policy.
    pub fn send(&self, id: &str, command: SpiceControl) -> Result<(), SpiceErrorKind> {
        let writer = self
            .sessions
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?
            .get(id)
            .map(|r| r.writer.clone())
            .ok_or(SpiceErrorKind::Connection)?;
        let packet = encode_control(&command).map_err(|_| SpiceErrorKind::Configuration)?;
        let release = || {
            encode_control(&SpiceControl::Input {
                events: vec![SpiceInput::ReleaseAll],
            })
        };
        let result = match &command {
            SpiceControl::Input { events }
                if !events.is_empty()
                    && events
                        .iter()
                        .all(|event| matches!(event, SpiceInput::Absolute { .. })) =>
            {
                writer.send_latest_move(packet)
            }
            SpiceControl::Input { .. }
            | SpiceControl::Clipboard { .. }
            | SpiceControl::Resize { .. }
            | SpiceControl::SendFiles { .. } => {
                writer.send_reliable(packet, release().map_err(|_| SpiceErrorKind::Protocol)?)
            }
            SpiceControl::SetVisible { .. }
            | SpiceControl::SetMuted { .. }
            | SpiceControl::Refresh
            | SpiceControl::CancelTransfer { .. } => writer.send_critical(packet),
            _ => return Err(SpiceErrorKind::Protocol),
        };
        result.map_err(map_io)
    }

    pub fn drain(&self, id: &str, limit: usize) -> Vec<SpiceEvent> {
        let queue = self
            .sessions
            .lock()
            .ok()
            .and_then(|s| s.get(id).map(|r| r.queue.clone()));
        queue.map_or_else(Vec::new, |queue| queue.drain(limit))
    }

    fn presentation(&self, id: &str) -> Option<crate::spice::presentation::PresentationClient> {
        self.sessions
            .lock()
            .ok()?
            .get(id)?
            .presentation
            .as_ref()
            .map(|p| p.client.clone())
    }

    pub fn configure_presentation(
        &self,
        id: &str,
        visible: bool,
        display: Option<crate::spice::display::SpiceDisplayId>,
        interval: std::time::Duration,
    ) {
        if let Some(client) = self.presentation(id) {
            client.configure(visible, display, interval);
        }
    }

    pub fn take_presentation(
        &self,
        id: &str,
    ) -> Option<crate::spice::presentation::SpicePresentation> {
        self.presentation(id)?.take()
    }

    pub fn acknowledge_presentation(
        &self,
        id: &str,
        frame: crate::spice::presentation::SpicePresentationId,
    ) {
        if let Some(client) = self.presentation(id) {
            client.acknowledge(frame);
        }
    }

    pub fn invalidate_presentation(&self, id: &str) {
        if let Some(client) = self.presentation(id) {
            client.invalidate();
        }
    }

    #[cfg(feature = "test-support")]
    pub fn test_live_record_count(&self) -> usize {
        self.live
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .count
    }

    #[cfg(feature = "test-support")]
    pub fn test_helper_running(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .is_some_and(|record| {
                record
                    .child
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_mut()
                    .is_some_and(|child| child.try_wait().ok().flatten().is_none())
            })
    }

    /// Remove ownership immediately; all process waits run on the reaper.
    pub fn close_detached(&self, id: &str) -> Result<(), SpiceErrorKind> {
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?;
        let reaper = self
            .reaper
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?;
        reaper
            .as_ref()
            .ok_or(SpiceErrorKind::HelperCrash)?
            .try_submit_with(|| {
                let record = sessions.remove(id)?;
                record.queue.close();
                if let Some(presentation) = &record.presentation {
                    presentation.client.close();
                }
                let _ = send_critical(&record.writer, SpiceControl::Disconnect);
                Some(record)
            })
            .map_err(map_io)
    }

    /// Blocking shutdown. Desktop schedules this outside GPUI callbacks.
    pub fn shutdown(&self) {
        self.live
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed = true;
        let records = std::mem::take(&mut *self.sessions.lock().unwrap_or_else(|p| p.into_inner()));
        for (_, record) in records {
            drop(record);
        }
        // Includes records already handed to the bounded reaper and starts
        // racing shutdown. No native process or compositor survives completion.
        let mut state = self.live.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.count != 0 {
            state = self
                .live
                .changed
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    fn ensure_reaper(&self) -> Result<(), SpiceErrorKind> {
        let mut reaper = self
            .reaper
            .lock()
            .map_err(|_| SpiceErrorKind::HelperCrash)?;
        if reaper.is_none() {
            *reaper = Some(Reaper::new("spice-reaper", 64, drop::<Record>).map_err(map_io)?);
        }
        Ok(())
    }
}

impl Drop for SpiceSessionManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_critical(writer: &IpcWriter, command: SpiceControl) -> Result<(), SpiceErrorKind> {
    writer
        .send_critical(encode_control(&command).map_err(|_| SpiceErrorKind::Protocol)?)
        .map_err(map_io)
}

fn map_io(error: io::Error) -> SpiceErrorKind {
    if error.kind() == io::ErrorKind::WouldBlock {
        SpiceErrorKind::ResourceLimit
    } else {
        SpiceErrorKind::HelperCrash
    }
}

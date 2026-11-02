use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::helper_process::{IpcWriter, cleanup_child, resolve_helper, spawn_helper};
use crate::{PROTOCOL_VERSION, RdpError, RdpErrorKind, RdpSessionConfig, read_packet};

use super::{
    HostCommand, HostEvent, NativeRdpConnectionEvent, NativeRdpRect, NativeRdpSessionDetails,
    decode, encode, validate_start,
};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Shared {
    events: VecDeque<NativeRdpConnectionEvent>,
    details: NativeRdpSessionDetails,
    details_changed: bool,
    pointer_inside: bool,
    viewport: Option<(NativeRdpRect, bool)>,
}

/// Non-blocking proxy. The worker owns spawning, pipes, waits and reaping.
pub struct NativeRdpControl {
    shared: Arc<Mutex<Shared>>,
    close: Arc<AtomicBool>,
    commands: SyncSender<HostCommand>,
    rect: NativeRdpRect,
    shown: bool,
    worker: Option<thread::JoinHandle<()>>,
}

impl NativeRdpControl {
    pub fn create(
        parent: isize,
        rect: NativeRdpRect,
        config: &RdpSessionConfig,
    ) -> Result<Self, String> {
        validate_start(config, rect).map_err(|error| error.to_string())?;
        if parent == 0 {
            return Err("Invalid ActiveX parent window".into());
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let close = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::sync_channel(16);
        let worker_shared = Arc::clone(&shared);
        let worker_close = Arc::clone(&close);
        let config = config.clone();
        let worker = thread::Builder::new()
            .name("nyaterm-activex-host".into())
            .spawn(move || {
                if let Err(error) = run(
                    parent as u64,
                    rect,
                    config,
                    receiver,
                    &worker_shared,
                    &worker_close,
                ) {
                    let mut state = worker_shared.lock().unwrap_or_else(|e| e.into_inner());
                    state.events.clear();
                    state
                        .events
                        .push_back(NativeRdpConnectionEvent::Failed(error));
                }
            })
            .map_err(|_| "Cannot start the ActiveX host worker".to_string())?;
        Ok(Self {
            shared,
            close,
            commands,
            rect,
            shown: false,
            worker: Some(worker),
        })
    }

    /// Must run on a shutdown worker, after the UI has stopped using the proxy.
    pub fn shutdown(mut self) {
        self.close.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    pub fn set_bounds(&mut self, rect: NativeRdpRect) {
        if rect != self.rect && rect.validate().is_ok() {
            self.rect = rect;
            self.queue_viewport();
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if self.shown != visible {
            self.shown = visible;
            self.queue_viewport();
        }
    }

    fn queue_viewport(&self) {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .viewport = Some((self.rect, self.shown));
    }

    pub fn focus(&self) {
        let _ = self.commands.try_send(HostCommand::Focus);
    }

    pub fn cursor_is_inside(&self) -> bool {
        self.shown
            && self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pointer_inside
    }

    pub fn poll_connection(&mut self) -> Option<NativeRdpConnectionEvent> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .pop_front()
    }

    pub fn session_details(&self) -> NativeRdpSessionDetails {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .details
            .clone()
    }

    /// Coalesce details notifications without copying session state into desktop.
    pub fn take_details_changed(&mut self) -> bool {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut shared.details_changed)
    }
}

impl Drop for NativeRdpControl {
    fn drop(&mut self) {
        self.close.store(true, Ordering::Release);
    }
}

fn run(
    parent: u64,
    rect: NativeRdpRect,
    config: RdpSessionConfig,
    commands: mpsc::Receiver<HostCommand>,
    shared: &Mutex<Shared>,
    close: &AtomicBool,
) -> Result<(), RdpError> {
    let path = resolve_helper("nyaterm-activex-helper", "NYATERM_ACTIVEX_HELPER")
        .map_err(|message| RdpError::new(RdpErrorKind::HelperMissing, message))?;
    let process = spawn_helper(&path, "activex")
        .map_err(|_| RdpError::new(RdpErrorKind::HelperMissing, "Cannot spawn the ActiveX host"))?;
    let mut child = Some(process.child);
    let mut stdout = process.stdout;
    let writer = match IpcWriter::spawn(process.stdin, "nyaterm-activex-writer".into()) {
        Ok(writer) => writer,
        Err(_) => {
            cleanup_child(&mut child, &mut None);
            process.stderr.finish();
            return Err(RdpError::new(
                RdpErrorKind::Ipc,
                "Cannot start the ActiveX IPC writer",
            ));
        }
    };
    let (events_tx, events_rx) = mpsc::sync_channel(64);
    let reader = thread::Builder::new()
        .name("nyaterm-activex-reader".into())
        .spawn(move || {
            while let Ok(Some(packet)) = read_packet(&mut stdout) {
                let Ok(event) = decode::<HostEvent>(&packet) else {
                    break;
                };
                // Never block a pipe reader on a stalled UI/worker.
                if events_tx.try_send(event).is_err() {
                    break;
                }
            }
        });
    let mut reader = match reader {
        Ok(reader) => Some(reader),
        Err(_) => {
            cleanup_child(&mut child, &mut None);
            process.stderr.finish();
            return Err(RdpError::new(
                RdpErrorKind::Ipc,
                "Cannot start the ActiveX IPC reader",
            ));
        }
    };
    let send =
        |message: &HostCommand| -> Result<(), RdpError> {
            writer
                .send_critical(encode(message).map_err(|_| {
                    RdpError::new(RdpErrorKind::Ipc, "Cannot encode ActiveX command")
                })?)
                .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "Cannot send ActiveX command"))
        };
    let result = (|| {
        send(&HostCommand::Hello {
            version: PROTOCOL_VERSION,
            parent_pid: std::process::id(),
        })?;
        let mut last_pong = Instant::now();
        let mut last_ping = Instant::now();
        let mut hello = false;
        let mut prepared = false;
        loop {
            if close.load(Ordering::Acquire) {
                let _ = send(&HostCommand::Close);
                return Ok(());
            }
            match events_rx.try_recv() {
                Ok(HostEvent::Hello { version }) if !hello && version == PROTOCOL_VERSION => {
                    hello = true;
                    send(&HostCommand::Prepare {
                        parent,
                        rect,
                        config: Box::new(config.clone()),
                    })?;
                }
                Ok(HostEvent::Hello { .. }) => {
                    return Err(RdpError::new(
                        RdpErrorKind::Protocol,
                        "ActiveX helper protocol mismatch",
                    ));
                }
                Ok(HostEvent::Prepared { child }) if hello && !prepared && child != 0 => {
                    prepared = true;
                    send(&HostCommand::Start)?;
                }
                Ok(HostEvent::Pong) if hello => last_pong = Instant::now(),
                Ok(HostEvent::Connection(event)) if hello => {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if state.events.len() >= 32 {
                        return Err(RdpError::new(
                            RdpErrorKind::ResourceExhausted,
                            "ActiveX event queue overflow",
                        ));
                    }
                    let terminal = matches!(
                        event,
                        NativeRdpConnectionEvent::Failed(_)
                            | NativeRdpConnectionEvent::Disconnected(_)
                    );
                    state.events.push_back(event);
                    if terminal {
                        return Ok(());
                    }
                }
                Ok(HostEvent::Details(details)) if hello => {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let changed = state.details != details;
                    state.details_changed |= changed;
                    state.details = details;
                }
                Ok(HostEvent::PointerInside(inside)) if hello => {
                    shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .pointer_inside = inside
                }
                Ok(HostEvent::Closed) => return Ok(()),
                Ok(_) => {
                    return Err(RdpError::new(
                        RdpErrorKind::Protocol,
                        "Unexpected ActiveX event before handshake",
                    ));
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(RdpError::new(
                        RdpErrorKind::HelperCrashed,
                        "ActiveX host exited unexpectedly",
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
            if last_pong.elapsed() > HEARTBEAT_TIMEOUT {
                return Err(RdpError::new(
                    RdpErrorKind::Timeout,
                    "ActiveX host is not responding",
                ));
            }
            if hello {
                let viewport = shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .viewport
                    .take();
                if let Some((rect, visible)) = viewport {
                    send(&HostCommand::Viewport { rect, visible })?;
                }
                while let Ok(command) = commands.try_recv() {
                    send(&command)?;
                }
                if last_ping.elapsed() >= Duration::from_secs(1) {
                    send(&HostCommand::Ping)?;
                    last_ping = Instant::now();
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    let _ = send(&HostCommand::Close);
    cleanup_child(&mut child, &mut reader);
    process.stderr.finish();
    result
}

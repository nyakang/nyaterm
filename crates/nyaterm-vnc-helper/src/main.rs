//! Isolated VNC helper process.
//!
//! The `vnc-rs` decoders (Tight, ZRLE, TRLE, and the zlib/JPEG paths they pull in)
//! parse server-controlled bytes, so they run here rather than inside the
//! application. Communication is the typed IPC protocol in `nyaterm-remote-desktop`:
//! [`VncControlMessage`] over `PacketType::Control`, framebuffer updates over the
//! protocol-neutral binary frame packets.
//!
//! This process owns the reconnect ladder and every server-facing policy gate
//! (`view_only`, `shared`, clipboard enablement). The application must not be the
//! only thing enforcing them.

use std::collections::VecDeque;
use std::io::{self, BufWriter, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::{
    CursorShape, CursorVisibility, MAX_VNC_CLIPBOARD_TEXT_BYTES, MAX_VNC_FRAMEBUFFER_HEIGHT,
    MAX_VNC_FRAMEBUFFER_WIDTH, MAX_VNC_INPUT_BATCH, PROTOCOL_VERSION, Packet, PixelFormat,
    RemoteCursorEvent, RemoteFrameEvent, RemotePoint, RemotePointerButton, RemotePointerEvent,
    RemoteWheelAxis, VncControlMessage, VncError, VncErrorKind, VncInputEvent, VncSecurityMode,
    VncServerCapabilities, VncSessionConfig, VncSessionState, decode_vnc_control,
    encode_cursor_packet, encode_frame_packet_owned, encode_vnc_control, read_packet,
    validate_committed_text, write_packet_into,
};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::sync::Notify;
use tokio::time::timeout;
use vnc::{
    ClientKeyEvent, ClientMouseEvent, PixelFormat as VncPixelFormat, Rect, Screen, VncClient,
    VncConnector, VncEncoding, VncEvent, VncLimits, VncSecurityPolicy, X11Event,
};
use zeroize::Zeroizing;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(8);
const UPDATE_REQUEST_INTERVAL: Duration = Duration::from_millis(16);
const STDOUT_BUFFER_BYTES: usize = 256 * 1024;
const WORKER_JOIN_TIMEOUT: Duration = Duration::from_millis(700);
const RELIABLE_WORKER_COMMAND_LIMIT: usize = 256;

enum Outbound {
    Control(VncControlMessage),
    Packet(Packet),
}

enum WorkerCommand {
    Input(ValidatedVncInputBatch),
    Clipboard(String),
    FullRefresh,
    ReleaseAllInputs,
    Close,
}

struct ValidatedVncInputBatch(Vec<VncInputEvent>);

#[derive(Default)]
struct WorkerMailboxState {
    reliable: VecDeque<WorkerCommand>,
    latest_move: Option<ValidatedVncInputBatch>,
    release_all: bool,
    resyncing: bool,
    closed: bool,
}

struct WorkerMailbox {
    state: Mutex<WorkerMailboxState>,
    changed: Notify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailboxError {
    Overflow,
    Resyncing,
    Closed,
}

impl WorkerMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(WorkerMailboxState::default()),
            changed: Notify::new(),
        }
    }

    fn enqueue_input(&self, batch: ValidatedVncInputBatch) -> Result<(), MailboxError> {
        let move_only = batch.0.iter().all(|event| {
            matches!(
                event,
                VncInputEvent::Pointer(RemotePointerEvent::Move { .. })
            )
        });
        let mut state = self.state.lock().map_err(|_| MailboxError::Closed)?;
        if state.closed {
            return Err(MailboxError::Closed);
        }
        if state.resyncing {
            return Err(MailboxError::Resyncing);
        }
        if move_only {
            state.latest_move = Some(batch);
        } else {
            enqueue_worker_reliable(&mut state, WorkerCommand::Input(batch))?;
        }
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    fn enqueue_reliable(&self, command: WorkerCommand) -> Result<(), MailboxError> {
        let mut state = self.state.lock().map_err(|_| MailboxError::Closed)?;
        if state.closed {
            return Err(MailboxError::Closed);
        }
        if state.resyncing {
            return Err(MailboxError::Resyncing);
        }
        enqueue_worker_reliable(&mut state, command)?;
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.reliable.clear();
            state.latest_move = None;
        }
        self.changed.notify_waiters();
    }

    fn try_pop(&self) -> Option<WorkerCommand> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return Some(WorkerCommand::Close);
        }
        if state.release_all {
            state.release_all = false;
            state.resyncing = false;
            return Some(WorkerCommand::ReleaseAllInputs);
        }
        state
            .reliable
            .pop_front()
            .or_else(|| state.latest_move.take().map(WorkerCommand::Input))
    }
}

fn enqueue_worker_reliable(
    state: &mut WorkerMailboxState,
    command: WorkerCommand,
) -> Result<(), MailboxError> {
    if state.reliable.len() >= RELIABLE_WORKER_COMMAND_LIMIT {
        state.reliable.clear();
        state.latest_move = None;
        state.release_all = true;
        state.resyncing = true;
        return Err(MailboxError::Overflow);
    }
    state.reliable.push_back(command);
    Ok(())
}

#[derive(Default)]
struct VncPointerState {
    x: u16,
    y: u16,
    button_mask: u8,
    vertical_wheel_remainder: i32,
    horizontal_wheel_remainder: i32,
}

impl ValidatedVncInputBatch {
    fn try_new(events: Vec<VncInputEvent>) -> Result<Self, VncError> {
        if events.len() > MAX_VNC_INPUT_BATCH {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                format!("VNC input batch exceeds {MAX_VNC_INPUT_BATCH} events"),
            ));
        }
        for event in &events {
            if let VncInputEvent::Text { text } = event {
                validate_committed_text(text).map_err(|error| {
                    VncError::new(
                        VncErrorKind::Protocol,
                        format!("invalid VNC committed text: {error}"),
                    )
                })?;
            }
        }
        Ok(Self(events))
    }
}

#[derive(Clone, Copy)]
enum ServerWriteKind {
    Input,
    Clipboard,
}

fn server_write_allowed(view_only: bool, clipboard_enabled: bool, kind: ServerWriteKind) -> bool {
    !view_only
        && match kind {
            ServerWriteKind::Input => true,
            ServerWriteKind::Clipboard => clipboard_enabled,
        }
}

/// A live session: the worker thread plus the channels that steer it.
struct Session {
    session_id: String,
    mailbox: Arc<WorkerMailbox>,
    worker: Option<JoinHandle<()>>,
    close_requested: Arc<AtomicBool>,
}

fn main() {
    match std::env::var("NYATERM_VNC_HELPER_TEST_MODE").as_deref() {
        Ok("crash") => std::process::exit(91),
        Ok("hang") => loop {
            thread::sleep(Duration::from_secs(1));
        },
        _ => {}
    }
    if let Err(error) = run() {
        eprintln!("VNC helper stopped: {error}");
        std::process::exit(1);
    }
}

fn validate_control_phase(message: &VncControlMessage, hello_complete: bool) -> io::Result<()> {
    if !hello_complete {
        if matches!(message, VncControlMessage::ClientHello { .. }) {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC IPC expected ClientHello before any other message",
        ));
    }

    match message {
        VncControlMessage::ClientHello { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC IPC ClientHello may only be sent once",
        )),
        VncControlMessage::ServerHello { .. }
        | VncControlMessage::DesktopReset { .. }
        | VncControlMessage::State { .. }
        | VncControlMessage::Error { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VNC IPC helper-only message received from application",
        )),
        VncControlMessage::Connect { .. }
        | VncControlMessage::Input { .. }
        | VncControlMessage::Clipboard { .. }
        | VncControlMessage::RequestFullFrame { .. }
        | VncControlMessage::Disconnect { .. } => Ok(()),
    }
}

fn validate_active_session_id(active_session_id: Option<&str>, received: &str) -> io::Result<()> {
    if let Some(active) = active_session_id
        && active != received
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "VNC IPC session id mismatch: active session is '{active}', received '{received}'"
            ),
        ));
    }
    Ok(())
}

fn run() -> io::Result<()> {
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let writer = spawn_stdout_writer(output_rx)?;
    let mut stdin = io::stdin().lock();
    let mut session: Option<Session> = None;
    let mut hello_complete = false;

    while let Some(packet) = read_packet(&mut stdin)? {
        let message = decode_vnc_control(&packet)?;
        validate_control_phase(&message, hello_complete)?;
        match message {
            VncControlMessage::ClientHello { version } => {
                if hello_complete {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC IPC ClientHello may only be sent once",
                    ));
                }
                if version != PROTOCOL_VERSION {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "VNC IPC protocol version {version} does not match {PROTOCOL_VERSION}"
                        ),
                    ));
                }
                send_control(
                    &output_tx,
                    VncControlMessage::ServerHello {
                        version: PROTOCOL_VERSION,
                        capabilities: VncServerCapabilities {
                            committed_unicode_keysyms: true,
                        },
                    },
                )?;
                hello_complete = true;
            }
            VncControlMessage::Connect { session_id, config } => {
                if !hello_complete {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC IPC Connect received before ClientHello",
                    ));
                }
                if session.is_some() {
                    send_error(
                        &output_tx,
                        &session_id,
                        VncErrorKind::Protocol,
                        "helper already owns a VNC session",
                        true,
                    )?;
                    continue;
                }
                session = Some(spawn_session(session_id, config, output_tx.clone()));
            }
            VncControlMessage::Input { session_id, events } => {
                if !hello_complete {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC IPC Input received before ClientHello",
                    ));
                }
                validate_active_session_id(
                    session.as_ref().map(|active| active.session_id.as_str()),
                    &session_id,
                )?;
                let batch = ValidatedVncInputBatch::try_new(events)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                let Some(active) = session.as_ref() else {
                    continue;
                };
                if active.mailbox.enqueue_input(batch) == Err(MailboxError::Overflow) {
                    send_error(
                        &output_tx,
                        &session_id,
                        VncErrorKind::Transport,
                        "InputBackpressure: VNC worker queue overflowed; releasing all inputs",
                        false,
                    )?;
                }
            }
            VncControlMessage::Clipboard { session_id, text } => {
                if !hello_complete {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC IPC Clipboard received before ClientHello",
                    ));
                }
                validate_active_session_id(
                    session.as_ref().map(|active| active.session_id.as_str()),
                    &session_id,
                )?;
                if let Some(active) = session.as_ref()
                    && active
                        .mailbox
                        .enqueue_reliable(WorkerCommand::Clipboard(text))
                        == Err(MailboxError::Overflow)
                {
                    send_error(
                        &output_tx,
                        &session_id,
                        VncErrorKind::Transport,
                        "InputBackpressure: VNC worker queue overflowed; releasing all inputs",
                        false,
                    )?;
                }
            }
            VncControlMessage::RequestFullFrame { session_id } => {
                validate_active_session_id(
                    session.as_ref().map(|active| active.session_id.as_str()),
                    &session_id,
                )?;
                if let Some(active) = session.as_ref()
                    && active.mailbox.enqueue_reliable(WorkerCommand::FullRefresh)
                        == Err(MailboxError::Overflow)
                {
                    send_error(
                        &output_tx,
                        &session_id,
                        VncErrorKind::Transport,
                        "InputBackpressure: VNC worker queue overflowed; releasing all inputs",
                        false,
                    )?;
                }
            }
            VncControlMessage::Disconnect { session_id } => {
                validate_active_session_id(
                    session.as_ref().map(|active| active.session_id.as_str()),
                    &session_id,
                )?;
                send_control(
                    &output_tx,
                    VncControlMessage::State {
                        session_id: session_id.clone(),
                        state: VncSessionState::Disconnecting,
                        message: None,
                    },
                )?;
                if let Some(active) = session.take() {
                    close_session(active);
                }
                send_control(
                    &output_tx,
                    VncControlMessage::State {
                        session_id,
                        state: VncSessionState::Disconnected,
                        message: None,
                    },
                )?;
                break;
            }
            // Messages the application never sends inbound.
            VncControlMessage::ServerHello { .. }
            | VncControlMessage::DesktopReset { .. }
            | VncControlMessage::State { .. }
            | VncControlMessage::Error { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "VNC IPC helper-only message received from application",
                ));
            }
        }
    }

    if let Some(active) = session.take() {
        close_session(active);
    }
    drop(output_tx);
    writer
        .join()
        .map_err(|_| io::Error::other("the VNC helper stdout writer panicked"))?
}

/// Pump outbound packets to stdout.
///
/// Everything already queued is coalesced into one buffered write and flushed
/// only once the queue drains. A single framebuffer update can carry up to
/// `max_rectangles_per_update` rectangles, so flushing per packet would turn one
/// update into a thousand write syscalls. Latency is unchanged: whenever the
/// producer is not ahead, each packet is flushed immediately.
fn spawn_stdout_writer(
    output_rx: mpsc::Receiver<Outbound>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    thread::Builder::new()
        .name("nyaterm-vnc-stdout".to_string())
        .spawn(move || {
            let mut stdout = BufWriter::with_capacity(STDOUT_BUFFER_BYTES, io::stdout().lock());
            while let Ok(outbound) = output_rx.recv() {
                write_outbound(&mut stdout, outbound)?;
                loop {
                    match output_rx.try_recv() {
                        Ok(outbound) => write_outbound(&mut stdout, outbound)?,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return stdout.flush(),
                    }
                }
                stdout.flush()?;
            }
            stdout.flush()
        })
}

fn write_outbound(writer: &mut impl io::Write, outbound: Outbound) -> io::Result<()> {
    let packet = match outbound {
        Outbound::Control(message) => encode_vnc_control(&message)?,
        Outbound::Packet(packet) => packet,
    };
    write_packet_into(writer, &packet)
}

fn spawn_session(
    session_id: String,
    config: VncSessionConfig,
    output_tx: mpsc::SyncSender<Outbound>,
) -> Session {
    let close_requested = Arc::new(AtomicBool::new(false));
    let mailbox = Arc::new(WorkerMailbox::new());
    let worker = spawn_worker(
        session_id.clone(),
        config,
        output_tx,
        Arc::clone(&close_requested),
        Arc::clone(&mailbox),
    );
    Session {
        session_id,
        mailbox,
        worker: Some(worker),
        close_requested,
    }
}

fn close_session(mut session: Session) {
    session.close_requested.store(true, Ordering::Release);
    session.mailbox.close();
    if let Some(worker) = session.worker.take() {
        let deadline = Instant::now() + WORKER_JOIN_TIMEOUT;
        while !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if worker.is_finished() {
            let _ = worker.join();
        }
        // Otherwise the worker is wedged in the vendored decoder. The process is
        // exiting anyway, and the parent kills it if this takes too long.
    }
}

fn spawn_worker(
    session_id: String,
    config: VncSessionConfig,
    output_tx: mpsc::SyncSender<Outbound>,
    close_requested: Arc<AtomicBool>,
    mailbox: Arc<WorkerMailbox>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nyaterm-vnc-{session_id}"))
        .spawn(move || {
            let panic_session_id = session_id.clone();
            let panic_output_tx = output_tx.clone();
            let result = catch_unwind(AssertUnwindSafe(move || match Runtime::new() {
                Ok(runtime) => {
                    let mut output = SessionOutput::new(session_id.clone(), output_tx.clone());
                    runtime.block_on(run_worker(&mut output, &config, &close_requested, mailbox));
                }
                Err(error) => {
                    let _ = output_tx.send(Outbound::Control(VncControlMessage::Error {
                        session_id,
                        error: VncError::new(
                            VncErrorKind::Internal,
                            format!("failed to start the VNC runtime: {error}"),
                        ),
                        fatal: true,
                    }));
                }
            }));
            if let Err(payload) = result {
                report_panic(&panic_output_tx, &panic_session_id, payload);
            }
        })
        .expect("failed to spawn the VNC worker")
}

/// Everything the worker sends upstream, plus the frame epoch it stamps.
///
/// The epoch lives here rather than in the application so a reconnect always
/// invalidates the previous framebuffer: it survives across generations and only
/// ever moves forward.
struct SessionOutput {
    session_id: String,
    output_tx: mpsc::SyncSender<Outbound>,
    epoch: u64,
    cursor_shape_id: u64,
}

impl SessionOutput {
    fn new(session_id: String, output_tx: mpsc::SyncSender<Outbound>) -> Self {
        Self {
            session_id,
            output_tx,
            epoch: 0,
            cursor_shape_id: 0,
        }
    }

    fn send(&self, outbound: Outbound) -> Result<(), VncError> {
        self.output_tx
            .send(outbound)
            .map_err(|_| VncError::new(VncErrorKind::Ipc, "the VNC helper stdout writer stopped"))
    }

    fn control(&self, message: VncControlMessage) -> Result<(), VncError> {
        self.send(Outbound::Control(message))
    }

    fn state(&self, state: VncSessionState, message: Option<String>) -> Result<(), VncError> {
        self.control(VncControlMessage::State {
            session_id: self.session_id.clone(),
            state,
            message,
        })
    }

    fn error(&self, error: VncError, fatal: bool) {
        let _ = self.control(VncControlMessage::Error {
            session_id: self.session_id.clone(),
            error,
            fatal,
        });
    }

    fn reset(&mut self, width: u32, height: u32) -> Result<u64, VncError> {
        self.epoch = self.epoch.saturating_add(1).max(1);
        self.control(VncControlMessage::DesktopReset {
            session_id: self.session_id.clone(),
            epoch: self.epoch,
            width,
            height,
        })?;
        Ok(self.epoch)
    }

    fn frame(&self, frame: RemoteFrameEvent) -> Result<(), VncError> {
        let packet = encode_frame_packet_owned(&self.session_id, frame)
            .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?;
        self.send(Outbound::Packet(packet))
    }

    fn cursor(&self, cursor: &RemoteCursorEvent) -> Result<(), VncError> {
        let packet = encode_cursor_packet(&self.session_id, cursor)
            .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?;
        self.send(Outbound::Packet(packet))
    }

    fn clipboard(&self, text: String) -> Result<(), VncError> {
        self.control(VncControlMessage::Clipboard {
            session_id: self.session_id.clone(),
            text,
        })
    }
}

async fn run_worker(
    output: &mut SessionOutput,
    config: &VncSessionConfig,
    close_requested: &AtomicBool,
    mailbox: Arc<WorkerMailbox>,
) {
    let mut attempt = 0;
    loop {
        if close_requested.load(Ordering::Acquire) {
            // The parent owns the Disconnecting/Disconnected pair.
            return;
        }
        let connecting_state = if attempt == 0 {
            VncSessionState::Connecting
        } else {
            VncSessionState::Reconnecting
        };
        if output.state(connecting_state, None).is_err() {
            return;
        }
        let result = run_generation(output, config, Arc::clone(&mailbox), close_requested).await;
        match result {
            Ok(()) => return,
            Err(error) => {
                if close_requested.load(Ordering::Acquire) {
                    return;
                }
                let retryable =
                    matches!(error.kind, VncErrorKind::Transport | VncErrorKind::Internal);
                if !retryable
                    || !config.reconnect.enabled
                    || attempt >= config.reconnect.max_attempts
                {
                    output.error(error, true);
                    return;
                }
                attempt += 1;
                tokio::time::sleep(reconnect_delay(attempt)).await;
            }
        }
    }
}

async fn run_generation(
    output: &mut SessionOutput,
    config: &VncSessionConfig,
    mailbox: Arc<WorkerMailbox>,
    close_requested: &AtomicBool,
) -> Result<(), VncError> {
    if let Some(relay) = &config.relay {
        relay
            .validate()
            .map_err(|error| VncError::new(VncErrorKind::Transport, error))?;
    }
    let connect = async {
        if let Some(relay) = &config.relay {
            TcpStream::connect(relay.address).await
        } else {
            TcpStream::connect((config.host.as_str(), config.port)).await
        }
    };
    let mut stream = timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| VncError::new(VncErrorKind::Transport, "VNC connection timed out"))?
        .map_err(|error| {
            VncError::new(
                VncErrorKind::Transport,
                format!("Unable to connect to the VNC server: {error}"),
            )
        })?;
    if let Some(relay) = &config.relay {
        use tokio::io::AsyncWriteExt as _;
        stream
            .write_all(relay.token.expose_secret().as_bytes())
            .await
            .map_err(|error| VncError::new(VncErrorKind::Transport, error.to_string()))?;
    }
    output.state(VncSessionState::Authenticating, None)?;
    let auth_password = Zeroizing::new(
        config
            .password
            .as_ref()
            .map(|password| password.expose_secret().to_owned())
            .unwrap_or_default(),
    );
    let connector = VncConnector::new(stream)
        .set_auth_method(async move { Ok(auth_password.to_string()) })
        .set_security_policy(security_policy(
            config.security.mode,
            config.password.is_some(),
        ))
        .set_pixel_format(VncPixelFormat::rgba())
        .set_limits(vnc_limits())
        .add_encoding(VncEncoding::DesktopSizePseudo)
        .add_encoding(VncEncoding::CursorPseudo)
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::Tight)
        .add_encoding(VncEncoding::Raw)
        .allow_shared(config.shared)
        .build()
        .map_err(classify_vnc_error)?;
    let client = timeout(HANDSHAKE_TIMEOUT, connector.try_start())
        .await
        .map_err(|_| {
            VncError::new(
                VncErrorKind::Transport,
                "VNC protocol negotiation timed out",
            )
        })?
        .and_then(|state| state.finish())
        .map_err(classify_vnc_error)?;
    output.state(VncSessionState::Negotiating, None)?;
    let mut pressed_keys = Vec::new();
    let mut pointer_state = VncPointerState::default();
    let mut poll = tokio::time::interval(EVENT_POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut refresh_due = false;
    let refresh_delay = tokio::time::sleep(Duration::from_secs(86_400));
    tokio::pin!(refresh_delay);
    loop {
        if close_requested.load(Ordering::Acquire) {
            release_all_inputs(&client, &mut pressed_keys, &mut pointer_state).await;
            let _ = client.close().await;
            return Ok(());
        }
        tokio::select! {
            _ = poll.tick() => {
                loop {
                    match client.poll_event().await {
                        Ok(Some(event)) => {
                            handle_vnc_event(output, event)?;
                            refresh_due = true;
                            refresh_delay.as_mut().reset(tokio::time::Instant::now() + UPDATE_REQUEST_INTERVAL);
                        }
                        Ok(None) => break,
                        Err(error) => return Err(classify_vnc_error(error)),
                    }
                }
            }
            _ = &mut refresh_delay, if refresh_due => {
                client.input(X11Event::Refresh).await.map_err(classify_vnc_error)?;
                refresh_due = false;
            }
            command = recv_worker_command(Arc::clone(&mailbox)) => {
                match command {
                    // view_only and clipboard.enabled are enforced here, not only
                    // in the application: this process is the authority.
                    Some(WorkerCommand::Input(batch))
                        if server_write_allowed(
                            config.view_only,
                            config.clipboard.enabled,
                            ServerWriteKind::Input,
                        ) =>
                    {
                        send_vnc_input_batch(
                            &client,
                            batch,
                            &mut pressed_keys,
                            &mut pointer_state,
                        )
                        .await?;
                    }
                    Some(WorkerCommand::Input(_)) => {}
                    Some(WorkerCommand::Clipboard(text))
                        if server_write_allowed(
                            config.view_only,
                            config.clipboard.enabled,
                            ServerWriteKind::Clipboard,
                        ) =>
                    {
                        client.input(X11Event::CopyText(text)).await.map_err(classify_vnc_error)?;
                    }
                    Some(WorkerCommand::Clipboard(_)) => {}
                    Some(WorkerCommand::FullRefresh) => {
                        client.input(X11Event::FullRefresh).await.map_err(classify_vnc_error)?;
                    }
                    Some(WorkerCommand::ReleaseAllInputs) => {
                        release_all_inputs(&client, &mut pressed_keys, &mut pointer_state).await;
                    }
                    Some(WorkerCommand::Close) | None => {
                        release_all_inputs(&client, &mut pressed_keys, &mut pointer_state).await;
                        let _ = client.close().await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn recv_worker_command(mailbox: Arc<WorkerMailbox>) -> Option<WorkerCommand> {
    loop {
        let changed = mailbox.changed.notified();
        if let Some(command) = mailbox.try_pop() {
            return Some(command);
        }
        changed.await;
    }
}

fn handle_vnc_event(output: &mut SessionOutput, event: VncEvent) -> Result<(), VncError> {
    match event {
        VncEvent::SetResolution(Screen { width, height }) => {
            validate_framebuffer_dimensions(u32::from(width), u32::from(height))?;
            output.reset(u32::from(width), u32::from(height))?;
        }
        VncEvent::RawImage(rect, pixels) => {
            // A server may send pixels before any resolution event; synthesize the
            // reset from the first rectangle so the epoch is never zero.
            let epoch = if output.epoch == 0 {
                let width = u32::from(rect.x) + u32::from(rect.width);
                let height = u32::from(rect.y) + u32::from(rect.height);
                validate_framebuffer_dimensions(width, height)?;
                output.reset(width, height)?
            } else {
                output.epoch
            };
            validate_rect(rect)?;
            let frame = RemoteFrameEvent::Bitmap {
                epoch,
                full: rect.x == 0 && rect.y == 0,
                x: u32::from(rect.x),
                y: u32::from(rect.y),
                width: u32::from(rect.width),
                height: u32::from(rect.height),
                stride: u32::from(rect.width) * 4,
                format: PixelFormat::Rgba8,
                pixels,
            };
            output.frame(frame)?;
            output.state(VncSessionState::Connected, None)?;
        }
        VncEvent::Text(text) if is_latin1_within_limit(&text) => {
            output.clipboard(text)?;
        }
        VncEvent::Error(message) => {
            return Err(VncError::new(VncErrorKind::Protocol, message));
        }
        VncEvent::JpegImage(_, _) => {
            return Err(VncError::new(
                VncErrorKind::Encoding,
                "The server sent a Tight JPEG event instead of decoded RGBA pixels",
            ));
        }
        VncEvent::SetCursor(rect, mut pixels) => {
            let width = u32::from(rect.width);
            let height = u32::from(rect.height);
            if width == 0 || height == 0 {
                output.cursor(&RemoteCursorEvent::Visibility(CursorVisibility {
                    visible: false,
                }))?;
                return Ok(());
            }
            let expected = usize::try_from(width)
                .ok()
                .and_then(|width| {
                    usize::try_from(height)
                        .ok()
                        .and_then(|height| width.checked_mul(height))
                })
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    VncError::new(VncErrorKind::Encoding, "VNC cursor dimensions overflow")
                })?;
            if pixels.len() != expected {
                return Err(VncError::new(
                    VncErrorKind::Encoding,
                    "VNC cursor pixel payload length does not match its dimensions",
                ));
            }
            rgba_to_bgra(&mut pixels);
            output.cursor_shape_id = output.cursor_shape_id.wrapping_add(1).max(1);
            output.cursor(&RemoteCursorEvent::Shape(CursorShape {
                shape_id: output.cursor_shape_id,
                width,
                height,
                hotspot: RemotePoint {
                    x: u32::from(rect.x),
                    y: u32::from(rect.y),
                },
                pixels,
            }))?;
            output.cursor(&RemoteCursorEvent::Visibility(CursorVisibility {
                visible: true,
            }))?;
        }
        VncEvent::Copy(_, _) => {
            return Err(VncError::new(
                VncErrorKind::Encoding,
                "The server sent an unrequested VNC encoding",
            ));
        }
        VncEvent::SetPixelFormat(_) | VncEvent::Bell => {}
        _ => {}
    }
    Ok(())
}

fn rgba_to_bgra(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

fn unicode_scalar_to_keysym(scalar: char) -> u32 {
    let scalar = u32::from(scalar);
    if scalar <= 0xff {
        scalar
    } else {
        0x0100_0000 | scalar
    }
}

fn committed_text_key_events(text: &str) -> impl Iterator<Item = (u32, bool)> + '_ {
    text.chars().flat_map(|scalar| {
        let keysym = unicode_scalar_to_keysym(scalar);
        [(keysym, true), (keysym, false)]
    })
}

async fn send_vnc_input_batch(
    client: &VncClient,
    batch: ValidatedVncInputBatch,
    pressed_keys: &mut Vec<u32>,
    pointer_state: &mut VncPointerState,
) -> Result<(), VncError> {
    for event in batch.0 {
        send_vnc_input(client, event, pressed_keys, pointer_state).await?;
    }
    Ok(())
}

async fn send_vnc_input(
    client: &VncClient,
    event: VncInputEvent,
    pressed_keys: &mut Vec<u32>,
    pointer_state: &mut VncPointerState,
) -> Result<(), VncError> {
    match event {
        VncInputEvent::Key { keysym, pressed } => {
            if pressed {
                if !pressed_keys.contains(&keysym) {
                    pressed_keys.push(keysym);
                }
            } else {
                pressed_keys.retain(|key| *key != keysym);
            }
            client
                .input(X11Event::KeyEvent(ClientKeyEvent {
                    keycode: keysym,
                    down: pressed,
                }))
                .await
                .map_err(classify_vnc_error)?;
        }
        VncInputEvent::Text { text } => {
            // The complete batch was validated before it crossed the worker
            // channel, so no VNC wire event can precede committed-text validation.
            // Committed text is always key input and never falls back to clipboard.
            for (keysym, pressed) in committed_text_key_events(&text) {
                client
                    .input(X11Event::KeyEvent(ClientKeyEvent {
                        keycode: keysym,
                        down: pressed,
                    }))
                    .await
                    .map_err(classify_vnc_error)?;
            }
        }
        VncInputEvent::Pointer(pointer) => {
            let position = match pointer {
                RemotePointerEvent::Move { position }
                | RemotePointerEvent::Button { position, .. }
                | RemotePointerEvent::Wheel { position, .. } => position,
            };
            pointer_state.x = u16::try_from(position.x).unwrap_or(u16::MAX);
            pointer_state.y = u16::try_from(position.y).unwrap_or(u16::MAX);
            match pointer {
                RemotePointerEvent::Move { .. } => {
                    send_vnc_pointer(client, pointer_state, pointer_state.button_mask).await?;
                }
                RemotePointerEvent::Button {
                    button, pressed, ..
                } => {
                    // Classic RFB has only an eight-bit button mask. Navigation
                    // buttons require a negotiated extension that vnc-rs does not
                    // currently expose, so never mis-map them to an ordinary button.
                    let bit = match button {
                        RemotePointerButton::Left => Some(0x01),
                        RemotePointerButton::Middle => Some(0x02),
                        RemotePointerButton::Right => Some(0x04),
                        RemotePointerButton::X1 | RemotePointerButton::X2 => None,
                    };
                    if let Some(bit) = bit {
                        if pressed {
                            pointer_state.button_mask |= bit;
                        } else {
                            pointer_state.button_mask &= !bit;
                        }
                        send_vnc_pointer(client, pointer_state, pointer_state.button_mask).await?;
                    }
                }
                RemotePointerEvent::Wheel {
                    axis,
                    rotation_units,
                    ..
                } => {
                    let (positive_mask, negative_mask, steps) = match axis {
                        RemoteWheelAxis::Vertical => (
                            0x08,
                            0x10,
                            accumulate_wheel_steps(
                                &mut pointer_state.vertical_wheel_remainder,
                                rotation_units,
                            ),
                        ),
                        RemoteWheelAxis::Horizontal => (
                            0x40,
                            0x20,
                            accumulate_wheel_steps(
                                &mut pointer_state.horizontal_wheel_remainder,
                                rotation_units,
                            ),
                        ),
                    };
                    for _ in 0..steps.unsigned_abs() {
                        let positive = steps > 0;
                        let mask = if positive {
                            positive_mask
                        } else {
                            negative_mask
                        };
                        send_vnc_pointer(client, pointer_state, pointer_state.button_mask | mask)
                            .await?;
                        send_vnc_pointer(client, pointer_state, pointer_state.button_mask).await?;
                    }
                }
            }
        }
        VncInputEvent::ReleaseAllInputs => {
            release_all_inputs(client, pressed_keys, pointer_state).await
        }
    }
    Ok(())
}

fn accumulate_wheel_steps(remainder: &mut i32, rotation_units: i16) -> i32 {
    *remainder = remainder.saturating_add(i32::from(rotation_units));
    let steps = *remainder / 120;
    *remainder -= steps * 120;
    steps
}

async fn send_vnc_pointer(
    client: &VncClient,
    state: &VncPointerState,
    button_mask: u8,
) -> Result<(), VncError> {
    client
        .input(X11Event::PointerEvent(ClientMouseEvent {
            position_x: state.x,
            position_y: state.y,
            bottons: button_mask,
        }))
        .await
        .map_err(classify_vnc_error)
}

async fn release_all_inputs(
    client: &VncClient,
    pressed_keys: &mut Vec<u32>,
    pointer_state: &mut VncPointerState,
) {
    release_pressed_keys(client, pressed_keys).await;
    if pointer_state.button_mask != 0 {
        pointer_state.button_mask = 0;
        pointer_state.vertical_wheel_remainder = 0;
        pointer_state.horizontal_wheel_remainder = 0;
        let _ = send_vnc_pointer(client, pointer_state, 0).await;
    }
}

async fn release_pressed_keys(client: &VncClient, pressed_keys: &mut Vec<u32>) {
    for keysym in pressed_keys.drain(..) {
        let _ = client
            .input(X11Event::KeyEvent(ClientKeyEvent {
                keycode: keysym,
                down: false,
            }))
            .await;
    }
}

fn classify_vnc_error(error: vnc::VncError) -> VncError {
    let (kind, message) = match error {
        vnc::VncError::NoPassword | vnc::VncError::WrongPassword => (
            VncErrorKind::Authentication,
            "VNC authentication failed".to_string(),
        ),
        vnc::VncError::UnsupportedSecurityType
        | vnc::VncError::RequiredSecurityTypeUnavailable(_)
        | vnc::VncError::InvalidSecurityType(_) => (
            VncErrorKind::Authentication,
            format!(
                "The VNC server requires an unsupported security type. Currently supported: None and VNC Authentication. Details: {error}"
            ),
        ),
        vnc::VncError::InvalidEncoding(_) | vnc::VncError::InvalidImageData => {
            (VncErrorKind::Encoding, error.to_string())
        }
        vnc::VncError::IoError(_) => (VncErrorKind::Transport, error.to_string()),
        vnc::VncError::LimitExceeded { .. }
        | vnc::VncError::InvalidDimensions
        | vnc::VncError::IntegerOverflow(_)
        | vnc::VncError::WrongPixelFormat
        | vnc::VncError::WrongServerMessage
        | vnc::VncError::InvalidSecurityResult(_)
        | vnc::VncError::SecurityFailure(_) => (VncErrorKind::Protocol, error.to_string()),
        _ => (VncErrorKind::Internal, error.to_string()),
    };
    VncError::new(kind, message)
}

fn security_policy(mode: VncSecurityMode, has_password: bool) -> VncSecurityPolicy {
    match mode {
        VncSecurityMode::None => VncSecurityPolicy::NoneOnly,
        VncSecurityMode::VncAuth => VncSecurityPolicy::VncAuthOnly,
        VncSecurityMode::Auto if has_password => VncSecurityPolicy::VncAuthOnly,
        VncSecurityMode::Auto => VncSecurityPolicy::NoneOnly,
    }
}

fn vnc_limits() -> VncLimits {
    VncLimits {
        max_framebuffer_width: u16::try_from(MAX_VNC_FRAMEBUFFER_WIDTH).unwrap_or(u16::MAX),
        max_framebuffer_height: u16::try_from(MAX_VNC_FRAMEBUFFER_HEIGHT).unwrap_or(u16::MAX),
        max_framebuffer_pixels: usize::try_from(
            MAX_VNC_FRAMEBUFFER_WIDTH * MAX_VNC_FRAMEBUFFER_HEIGHT,
        )
        .unwrap_or(usize::MAX),
        max_clipboard_bytes: MAX_VNC_CLIPBOARD_TEXT_BYTES,
        max_rectangles_per_update: 1024,
        max_encoded_payload_bytes: 64 * 1024 * 1024,
        max_decoded_payload_bytes: usize::try_from(
            MAX_VNC_FRAMEBUFFER_WIDTH * MAX_VNC_FRAMEBUFFER_HEIGHT * 4,
        )
        .unwrap_or(usize::MAX),
        channel_capacity: 32,
        ..VncLimits::default()
    }
}

fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 15,
        _ => 30,
    })
}

fn validate_framebuffer_dimensions(width: u32, height: u32) -> Result<(), VncError> {
    if width == 0
        || height == 0
        || width > MAX_VNC_FRAMEBUFFER_WIDTH
        || height > MAX_VNC_FRAMEBUFFER_HEIGHT
    {
        return Err(VncError::new(
            VncErrorKind::Protocol,
            format!("VNC framebuffer {width}x{height} is outside the supported range"),
        ));
    }
    Ok(())
}

fn validate_rect(rect: Rect) -> Result<(), VncError> {
    if rect.width == 0 || rect.height == 0 {
        return Err(VncError::new(
            VncErrorKind::Protocol,
            "VNC rectangle must be non-empty",
        ));
    }
    Ok(())
}

fn is_latin1_within_limit(text: &str) -> bool {
    text.len() <= MAX_VNC_CLIPBOARD_TEXT_BYTES && text.chars().all(|ch| u32::from(ch) <= 0xff)
}

fn send_control(
    output_tx: &mpsc::SyncSender<Outbound>,
    message: VncControlMessage,
) -> io::Result<()> {
    output_tx
        .send(Outbound::Control(message))
        .map_err(|_| io::Error::other("the VNC helper stdout writer stopped"))
}

fn send_error(
    output_tx: &mpsc::SyncSender<Outbound>,
    session_id: &str,
    kind: VncErrorKind,
    message: &str,
    fatal: bool,
) -> io::Result<()> {
    send_control(
        output_tx,
        VncControlMessage::Error {
            session_id: session_id.to_string(),
            error: VncError::new(kind, message),
            fatal,
        },
    )
}

/// Turn a decoder panic into a fatal error the application can act on.
///
/// Without this the worker thread would die silently and the session would hang
/// in whatever state it last reported.
fn report_panic(
    output_tx: &mpsc::SyncSender<Outbound>,
    session_id: &str,
    payload: Box<dyn std::any::Any + Send>,
) {
    let detail = if let Some(message) = payload.downcast_ref::<&str>() {
        *message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "non-string panic payload"
    };
    // One line, bounded: this crosses the IPC channel as a JSON string.
    let detail = detail.replace(['\r', '\n'], " ");
    let detail = detail.chars().take(512).collect::<String>();
    let _ = output_tx.send(Outbound::Control(VncControlMessage::Error {
        session_id: session_id.to_string(),
        error: VncError::new(
            VncErrorKind::HelperCrashed,
            format!("VNC decoder panicked: {detail}"),
        ),
        fatal: true,
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use nyaterm_remote_desktop::{
        CursorVisibility, MAX_COMMITTED_TEXT_BYTES, MAX_VNC_CLIPBOARD_TEXT_BYTES,
        MAX_VNC_INPUT_BATCH, RemoteCursorEvent, RemotePoint, RemotePointerEvent, VncControlMessage,
        VncErrorKind, VncInputEvent, VncSecurityMode, decode_cursor_packet_owned,
    };
    use vnc::{Rect, VncEvent, VncSecurityPolicy};

    use super::{
        MailboxError, Outbound, RELIABLE_WORKER_COMMAND_LIMIT, ServerWriteKind, SessionOutput,
        ValidatedVncInputBatch, WorkerCommand, WorkerMailbox, accumulate_wheel_steps,
        classify_vnc_error, committed_text_key_events, handle_vnc_event, is_latin1_within_limit,
        reconnect_delay, report_panic, security_policy, server_write_allowed,
        unicode_scalar_to_keysym, validate_active_session_id, validate_framebuffer_dimensions,
    };

    #[test]
    fn active_session_id_rejects_foreign_messages() {
        validate_active_session_id(None, "before-connect")
            .expect("pre-connect handling keeps its existing semantics");
        validate_active_session_id(Some("session-a"), "session-a").expect("matching session id");
        let error = validate_active_session_id(Some("session-a"), "session-b")
            .expect_err("foreign session id must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("session id mismatch"));
    }

    #[test]
    fn panic_payloads_are_reported_as_a_fatal_single_line_error() {
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        report_panic(
            &output_tx,
            "session",
            Box::new("zrle decoder\nindex out of bounds".to_string()),
        );
        let Ok(Outbound::Control(VncControlMessage::Error { error, fatal, .. })) = output_rx.recv()
        else {
            panic!("expected a fatal helper-crash error");
        };
        assert_eq!(error.kind, VncErrorKind::HelperCrashed);
        assert_eq!(
            error.message,
            "VNC decoder panicked: zrle decoder index out of bounds"
        );
        assert!(fatal);
    }

    #[test]
    fn only_transport_and_internal_failures_are_retryable() {
        // run_worker retries exactly these two kinds; the classifier decides which
        // vendored error lands where, so pin the boundary.
        assert_eq!(
            classify_vnc_error(vnc::VncError::WrongPassword).kind,
            VncErrorKind::Authentication
        );
        assert_eq!(
            classify_vnc_error(vnc::VncError::InvalidImageData).kind,
            VncErrorKind::Encoding
        );
        assert_eq!(
            classify_vnc_error(vnc::VncError::WrongServerMessage).kind,
            VncErrorKind::Protocol
        );
        assert_eq!(
            classify_vnc_error(vnc::VncError::IoError(std::io::Error::other("reset"))).kind,
            VncErrorKind::Transport
        );
    }

    #[test]
    fn security_policy_never_falls_back_to_a_weaker_mode() {
        assert!(matches!(
            security_policy(VncSecurityMode::None, true),
            VncSecurityPolicy::NoneOnly
        ));
        assert!(matches!(
            security_policy(VncSecurityMode::VncAuth, false),
            VncSecurityPolicy::VncAuthOnly
        ));
        assert!(matches!(
            security_policy(VncSecurityMode::Auto, true),
            VncSecurityPolicy::VncAuthOnly
        ));
        assert!(matches!(
            security_policy(VncSecurityMode::Auto, false),
            VncSecurityPolicy::NoneOnly
        ));
    }

    #[test]
    fn framebuffer_dimensions_outside_the_supported_range_are_rejected() {
        assert!(validate_framebuffer_dimensions(1920, 1080).is_ok());
        assert!(validate_framebuffer_dimensions(0, 1080).is_err());
        assert!(validate_framebuffer_dimensions(1920, 0).is_err());
        assert!(validate_framebuffer_dimensions(7681, 1080).is_err());
        assert!(validate_framebuffer_dimensions(1920, 4321).is_err());
    }

    #[test]
    fn cursor_events_preserve_hotspot_convert_bgra_and_toggle_visibility() {
        let (output_tx, output_rx) = mpsc::sync_channel(3);
        let mut output = SessionOutput::new("session".to_string(), output_tx);
        handle_vnc_event(
            &mut output,
            VncEvent::SetCursor(
                Rect {
                    x: 2,
                    y: 3,
                    width: 1,
                    height: 1,
                },
                vec![10, 20, 30, 128],
            ),
        )
        .expect("cursor shape");

        let Outbound::Packet(shape_packet) = output_rx.recv().expect("shape packet") else {
            panic!("expected cursor shape packet");
        };
        let (session_id, shape) =
            decode_cursor_packet_owned(shape_packet).expect("decode cursor shape");
        assert_eq!(session_id, "session");
        let RemoteCursorEvent::Shape(shape) = shape else {
            panic!("expected cursor shape");
        };
        assert_eq!(shape.shape_id, 1);
        assert_eq!(shape.hotspot, RemotePoint { x: 2, y: 3 });
        assert_eq!(shape.pixels, vec![30, 20, 10, 128]);

        let Outbound::Packet(visible_packet) = output_rx.recv().expect("visibility packet") else {
            panic!("expected cursor visibility packet");
        };
        assert_eq!(
            decode_cursor_packet_owned(visible_packet)
                .expect("decode visibility")
                .1,
            RemoteCursorEvent::Visibility(CursorVisibility { visible: true })
        );

        handle_vnc_event(
            &mut output,
            VncEvent::SetCursor(
                Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                Vec::new(),
            ),
        )
        .expect("hidden cursor");
        let Outbound::Packet(hidden_packet) = output_rx.recv().expect("hidden packet") else {
            panic!("expected hidden cursor packet");
        };
        assert_eq!(
            decode_cursor_packet_owned(hidden_packet)
                .expect("decode hidden")
                .1,
            RemoteCursorEvent::Visibility(CursorVisibility { visible: false })
        );
    }

    #[test]
    fn server_clipboard_text_must_be_latin1_within_the_limit() {
        assert!(is_latin1_within_limit("hello"));
        assert!(!is_latin1_within_limit("hello \u{0100}"));
        assert!(!is_latin1_within_limit(
            &"a".repeat(MAX_VNC_CLIPBOARD_TEXT_BYTES + 1)
        ));
    }

    #[test]
    fn unicode_scalars_convert_to_standard_x11_keysyms() {
        assert_eq!(unicode_scalar_to_keysym('A'), 0x41);
        assert_eq!(unicode_scalar_to_keysym('é'), 0xe9);
        assert_eq!(unicode_scalar_to_keysym('Ā'), 0x0100_0100);
        assert_eq!(unicode_scalar_to_keysym('文'), 0x0100_6587);
        assert_eq!(unicode_scalar_to_keysym('\u{10ffff}'), 0x0110_ffff);
    }

    #[test]
    fn committed_text_expands_only_to_ordered_key_down_up_pairs() {
        assert_eq!(
            committed_text_key_events("A文").collect::<Vec<_>>(),
            vec![
                (0x41, true),
                (0x41, false),
                (0x0100_6587, true),
                (0x0100_6587, false),
            ]
        );
    }

    #[test]
    fn input_batch_is_fully_validated_before_worker_dispatch() {
        let valid_prefix_then_invalid_text = vec![
            VncInputEvent::Key {
                keysym: 0x41,
                pressed: true,
            },
            VncInputEvent::Text {
                text: "valid 文本".to_string(),
            },
            VncInputEvent::Text {
                text: "invalid\u{001b}".to_string(),
            },
        ];
        assert!(ValidatedVncInputBatch::try_new(valid_prefix_then_invalid_text).is_err());

        let oversized_text = VncInputEvent::Text {
            text: "a".repeat(MAX_COMMITTED_TEXT_BYTES + 1),
        };
        assert!(ValidatedVncInputBatch::try_new(vec![oversized_text]).is_err());

        let oversized_batch = vec![VncInputEvent::ReleaseAllInputs; MAX_VNC_INPUT_BATCH + 1];
        assert!(ValidatedVncInputBatch::try_new(oversized_batch).is_err());
    }

    #[test]
    fn view_only_is_the_authoritative_input_and_clipboard_gate() {
        assert!(!server_write_allowed(true, true, ServerWriteKind::Input));
        assert!(!server_write_allowed(
            true,
            true,
            ServerWriteKind::Clipboard
        ));
        assert!(server_write_allowed(false, false, ServerWriteKind::Input));
        assert!(!server_write_allowed(
            false,
            false,
            ServerWriteKind::Clipboard
        ));
        assert!(server_write_allowed(
            false,
            true,
            ServerWriteKind::Clipboard
        ));
    }

    #[test]
    fn reconnect_delay_escalates_and_then_plateaus() {
        let delays: Vec<u64> = (0..8).map(|n| reconnect_delay(n).as_secs()).collect();
        assert_eq!(delays, vec![1, 1, 2, 4, 8, 15, 30, 30]);
    }

    #[test]
    fn worker_mailbox_coalesces_moves_after_reliable_commands() {
        let mailbox = WorkerMailbox::new();
        mailbox
            .enqueue_reliable(WorkerCommand::Clipboard("first".to_string()))
            .expect("reliable command");
        for x in [10, 20] {
            mailbox
                .enqueue_input(ValidatedVncInputBatch(vec![VncInputEvent::Pointer(
                    RemotePointerEvent::Move {
                        position: RemotePoint { x, y: 7 },
                    },
                )]))
                .expect("pointer move");
        }

        assert!(matches!(
            mailbox.try_pop(),
            Some(WorkerCommand::Clipboard(text)) if text == "first"
        ));
        assert!(matches!(
            mailbox.try_pop(),
            Some(WorkerCommand::Input(ValidatedVncInputBatch(events)))
                if matches!(
                    events.as_slice(),
                    [VncInputEvent::Pointer(RemotePointerEvent::Move {
                        position: RemotePoint { x: 20, y: 7 }
                    })]
                )
        ));
        assert!(mailbox.try_pop().is_none());
    }

    #[test]
    fn worker_mailbox_overflow_releases_and_resynchronizes_input() {
        let mailbox = WorkerMailbox::new();
        for index in 0..RELIABLE_WORKER_COMMAND_LIMIT {
            mailbox
                .enqueue_reliable(WorkerCommand::Clipboard(index.to_string()))
                .expect("queue has bounded capacity");
        }
        let overflow = mailbox.enqueue_input(ValidatedVncInputBatch(vec![VncInputEvent::Key {
            keysym: 0x41,
            pressed: true,
        }]));
        assert_eq!(overflow, Err(MailboxError::Overflow));
        assert_eq!(
            mailbox.enqueue_reliable(WorkerCommand::FullRefresh),
            Err(MailboxError::Resyncing)
        );
        assert!(matches!(
            mailbox.try_pop(),
            Some(WorkerCommand::ReleaseAllInputs)
        ));
        mailbox
            .enqueue_reliable(WorkerCommand::FullRefresh)
            .expect("release completion resumes input");
        assert!(matches!(
            mailbox.try_pop(),
            Some(WorkerCommand::FullRefresh)
        ));
    }

    #[test]
    fn wheel_units_emit_only_complete_rfb_pulses_and_keep_the_remainder() {
        let mut remainder = 0;
        assert_eq!(accumulate_wheel_steps(&mut remainder, 40), 0);
        assert_eq!(accumulate_wheel_steps(&mut remainder, 80), 1);
        assert_eq!(remainder, 0);
        assert_eq!(accumulate_wheel_steps(&mut remainder, -250), -2);
        assert_eq!(remainder, -10);
        assert_eq!(accumulate_wheel_steps(&mut remainder, 130), 1);
        assert_eq!(remainder, 0);
    }
}

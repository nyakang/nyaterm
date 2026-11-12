//! Screen subprocess supervision. Handles enqueue work; cleanup never runs in a UI destructor.
use crate::operation::AdbOperationControl;
use std::collections::VecDeque;
use std::io::{self, Read};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nyaterm_core::models::android::AndroidScreenSettings;
use sha2::{Digest, Sha256};

use crate::adb::client::{AdbClient, DeviceBinding};
use crate::adb::runtime::DeviceRuntime;
use crate::adb::shell::{
    EXIT, STDOUT, ShellDecoder, execute_with_cancel, open_shell_with_cancel, quote,
};
use crate::error::{AdbError, AdbResult};
use crate::ipc::{Control, Frame, PROTOCOL_VERSION, Packet, read_packet_reusing, write_packet};

fn validate_helper_hello(protocol: u32, version: &str) -> AdbResult<()> {
    if protocol != PROTOCOL_VERSION || version != env!("CARGO_PKG_VERSION") {
        return Err(AdbError::HelperVersionMismatch);
    }
    Ok(())
}

pub const SCRCPY_VERSION: &str = "5.0.1";
pub const SCRCPY_SHA256: &str = "764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536";
const SERVER: &[u8] = include_bytes!("../assets/scrcpy-server-v5.0.1");
const INPUT_LIMIT: usize = 256;

#[derive(Clone)]
pub struct CleanupSpec {
    pub client: AdbClient,
    pub binding: DeviceBinding,
    pub remote_path: String,
    pub port: u16,
    pub pid: Option<u32>,
    pub scid: String,
    /// Classified failure only; never retains shell output or command text.
    pub last_error: Option<String>,
}

impl CleanupSpec {
    fn stop_server_command(&self) -> AdbResult<Vec<u8>> {
        if self.scid.len() != 8 || !self.scid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AdbError::Protocol("cleanup session identity"));
        }
        let candidates = if let Some(pid) = self.pid {
            format!("/proc/{pid}/cmdline")
        } else {
            // Startup can be cancelled before its PID packet arrives. Match
            // complete NUL-separated arguments so the cleanup shell, grep and
            // other screen sessions cannot match our own command text.
            "/proc/[0-9]*/cmdline".to_owned()
        };
        Ok(format!(
            "for p in {candidates}; do [ -r \"$p\" ] || continue; \
             tr '\\000' '\\n' < \"$p\" | grep -Fx -- 'com.genymobile.scrcpy.Server' >/dev/null || continue; \
             tr '\\000' '\\n' < \"$p\" | grep -Fx -- 'scid={}' >/dev/null || continue; \
             p=${{p%/cmdline}}; p=${{p##*/}}; kill \"$p\"; done; ",
            self.scid
        ).into_bytes())
    }
    pub fn retry(&self) -> AdbResult<()> {
        let remote = format!("localabstract:scrcpy_{}", self.scid);
        // Discover by our unique destination even if cancellation prevented reading the port.
        // A restarted server can reuse the old port for a different owner.
        for line in self.client.command(&["forward", "--list"])?.lines() {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() == 3 && columns[0] == self.binding.serial && columns[2] == remote {
                self.client
                    .device_command(&self.binding, &["forward", "--remove", columns[1]])?;
            }
        }
        let mut command = self.stop_server_command()?;
        command.extend_from_slice(b"rm -f -- ");
        command.extend_from_slice(&quote(self.remote_path.as_bytes())?);
        // This command only removes uniquely owned resources and verifies the
        // PID before killing it. It is safe to retry an interrupted cleanup;
        // ordinary file mutations and terminal commands never use this path.
        for attempt in 0..3 {
            let deadline = Instant::now() + Duration::from_secs(2);
            let result = execute_with_cancel(&self.binding, &command, 4096, || {
                if Instant::now() >= deadline {
                    Err(AdbError::Timeout)
                } else {
                    Ok(())
                }
            })
            .map(|_| ());
            if attempt == 2 || !matches!(result, Err(AdbError::Timeout | AdbError::Io(_))) {
                return result;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        unreachable!("bounded cleanup attempts return")
    }
}

#[derive(Default)]
pub struct CleanupStatus {
    pending: Mutex<Option<CleanupSpec>>,
    finished: AtomicBool,
    started: AtomicBool,
}
impl CleanupStatus {
    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Option<CleanupSpec>>> {
        self.pending.lock()
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn is_complete(&self) -> bool {
        self.is_finished()
            && self
                .pending
                .try_lock()
                .is_ok_and(|pending| pending.is_none())
    }
}
pub type CleanupTicket = Arc<CleanupStatus>;

struct Resources {
    cleanup: CleanupSpec,
    server: Option<TcpStream>,
    helper: Option<Child>,
    ticket: CleanupTicket,
}

impl Drop for Resources {
    fn drop(&mut self) {
        if let Some(child) = &mut self.helper {
            let _ = crate::process::reap(child, Duration::from_millis(750));
        }
        // Keep the launching channel alive while checking/killing its PID.
        // Closing it first races adbd's process termination with /proc reads.
        if let Err(error) = self.cleanup.retry() {
            let mut pending = self.cleanup.clone();
            pending.last_error = Some(error.to_string());
            *self.ticket.lock().unwrap_or_else(|p| p.into_inner()) = Some(pending);
        }
        if let Some(server) = &self.server {
            let _ = server.shutdown(Shutdown::Both);
        }
        self.ticket.finished.store(true, Ordering::Release);
    }
}

struct State {
    input: VecDeque<Control>,
    events: VecDeque<Control>,
    latest: Option<Frame>,
    pixels: Vec<Vec<u8>>,
    closed: bool,
    finished: bool,
    hello: bool,
    pong: Instant,
}
impl State {
    fn acknowledge(&mut self, sequence: u64) {
        // Keep the reader bounded even if a faulty helper violates frame credit.
        self.input
            .retain(|control| !matches!(control, Control::FrameAck { .. }));
        self.input.push_back(Control::FrameAck { sequence });
    }
}

struct Mailbox {
    state: Mutex<State>,
    changed: Condvar,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                input: VecDeque::new(),
                events: VecDeque::new(),
                latest: None,
                pixels: Vec::new(),
                closed: false,
                finished: false,
                hello: false,
                pong: Instant::now(),
            }),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        }
    }
    fn wake(&self) {
        if let Some(waker) = self.waker.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            waker();
        }
    }
    fn fatal(&self, code: &str) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.events.len() >= 32 {
            state.events.pop_front();
        }
        state.events.push_back(Control::Fatal { code: code.into() });
        state.closed = true;
        self.changed.notify_all();
        drop(state);
        self.wake();
    }
}

pub struct ScreenHandle {
    mailbox: Arc<Mailbox>,
    pub cleanup_ticket: CleanupTicket,
}

impl ScreenHandle {
    pub fn start(
        client: AdbClient,
        binding: DeviceBinding,
        settings: AndroidScreenSettings,
    ) -> AdbResult<Self> {
        Self::start_with_helper(client, binding, settings, &resolve_helper()?)
    }

    pub fn start_with_helper(
        client: AdbClient,
        binding: DeviceBinding,
        settings: AndroidScreenSettings,
        helper: &Path,
    ) -> AdbResult<Self> {
        Self::start_with_helper_controlled(
            client,
            binding.clone(),
            settings,
            helper,
            AdbOperationControl::default(),
            Arc::new(CleanupStatus::default()),
            Arc::new(DeviceRuntime::new(binding.clone())),
        )
    }
    pub fn start_controlled(
        client: AdbClient,
        binding: DeviceBinding,
        settings: AndroidScreenSettings,
        control: AdbOperationControl,
        ticket: CleanupTicket,
        runtime: Arc<DeviceRuntime>,
    ) -> AdbResult<Self> {
        let result = control
            .check()
            .and_then(|()| resolve_helper())
            .and_then(|helper| {
                Self::start_with_helper_controlled(
                    client,
                    binding,
                    settings,
                    &helper,
                    control,
                    ticket.clone(),
                    runtime,
                )
            });
        if result.is_err() && !ticket.started.load(Ordering::Acquire) {
            ticket.finished.store(true, Ordering::Release);
        }
        result
    }
    pub fn start_with_helper_controlled(
        client: AdbClient,
        binding: DeviceBinding,
        settings: AndroidScreenSettings,
        helper: &Path,
        lifetime: AdbOperationControl,
        ticket: CleanupTicket,
        runtime: Arc<DeviceRuntime>,
    ) -> AdbResult<Self> {
        struct EarlyTicket(CleanupTicket);
        impl Drop for EarlyTicket {
            fn drop(&mut self) {
                if !self.0.started.load(Ordering::Acquire) {
                    self.0.finished.store(true, Ordering::Release);
                }
            }
        }
        let _early_ticket = EarlyTicket(ticket.clone());
        let control = lifetime.with_timeout(Duration::from_secs(30));
        runtime.check()?;
        if runtime.binding != binding {
            return Err(AdbError::Offline);
        }
        control.check()?;
        settings
            .validate()
            .map_err(|_| AdbError::Protocol("screen settings"))?;
        if hex::encode(Sha256::digest(SERVER)) != SCRCPY_SHA256 {
            return Err(AdbError::Protocol("server checksum"));
        }
        let scid = format!(
            "{:08x}",
            uuid::Uuid::new_v4().as_u128() as u32 & 0x7fff_ffff
        );
        let remote_path = format!("/data/local/tmp/nyaterm-scrcpy-{scid}.jar");
        ticket.started.store(true, Ordering::Release);
        let mut resources = Resources {
            cleanup: CleanupSpec {
                client,
                binding: binding.clone(),
                remote_path,
                port: 0,
                pid: None,
                scid,
                last_error: None,
            },
            server: None,
            helper: None,
            ticket: Arc::clone(&ticket),
        };
        let modified = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AdbError::Protocol("system time"))?
            .as_secs();
        let mut payload = SERVER;
        control.check()?;
        runtime.with_sync(
            || control.check(),
            |stream| {
                stream.send(
                    resources.cleanup.remote_path.as_bytes(),
                    &mut payload,
                    0o644,
                    modified,
                    |_| control.check(),
                )
            },
        )?;
        control.check()?;
        let target = format!("localabstract:scrcpy_{}", resources.cleanup.scid);
        let port = resources.cleanup.client.device_command_with_control(
            &binding,
            &["forward", "tcp:0", &target],
            &control,
        )?;
        resources.cleanup.port = port
            .trim()
            .parse()
            .map_err(|_| AdbError::Protocol("forward port"))?;
        let mut command = b"echo $$; CLASSPATH=".to_vec();
        command.extend_from_slice(&quote(resources.cleanup.remote_path.as_bytes())?);
        command.extend_from_slice(format!(" exec app_process / com.genymobile.scrcpy.Server {SCRCPY_VERSION} scid={} tunnel_forward=true audio=false control=true video_codec=h264 max_size={} max_fps={} video_bit_rate={} clipboard_autosync=false cleanup=false power_on=false log_level=error", resources.cleanup.scid, settings.max_size, settings.max_fps, settings.bit_rate).as_bytes());
        control.check()?;
        let mut server = open_shell_with_cancel(&binding, &command, false, || control.check())?;
        server.set_read_timeout(Some(Duration::from_millis(100)))?;
        resources.server = Some(server.try_clone()?);
        let mut decoder = ShellDecoder::default();
        let mut buffer = [0; 4096];
        let mut pid_bytes = Vec::new();
        while resources.cleanup.pid.is_none() {
            control.check()?;
            let count = match server.read(&mut buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                result => result?,
            };
            if count == 0 {
                return Err(AdbError::Protocol("server startup EOF"));
            }
            for packet in decoder.push(&buffer[..count])? {
                if packet.id == EXIT {
                    return Err(AdbError::ShellExit(packet.data[0]));
                }
                if packet.id == STDOUT {
                    pid_bytes.extend_from_slice(&packet.data);
                    if pid_bytes.len() > 4096 {
                        return Err(AdbError::Protocol("server PID output limit"));
                    }
                    if let Some(end) = pid_bytes.iter().position(|byte| *byte == b'\n') {
                        resources.cleanup.pid = std::str::from_utf8(&pid_bytes[..end])
                            .ok()
                            .and_then(|value| value.trim().parse().ok());
                        if resources.cleanup.pid.is_none() {
                            return Err(AdbError::Protocol("server PID"));
                        }
                    }
                }
            }
        }
        server.set_read_timeout(Some(Duration::from_millis(100)))?;
        resources.server = Some(server);
        control.check()?;
        let mut command = std::process::Command::new(helper);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or(AdbError::Protocol("helper stdout"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or(AdbError::Protocol("helper stdin"))?;
        resources.helper = Some(child);
        let mailbox = Arc::new(Mailbox::new());
        let reader_mailbox = Arc::clone(&mailbox);
        let binding_generation = binding.generation;
        let reader =
            nyaterm_core::runtime::worker::spawn("android-screen-reader".into(), move || {
                let mut pixels = Vec::new();
                loop {
                    if pixels.capacity() == 0 {
                        pixels = reader_mailbox
                            .state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .pixels
                            .pop()
                            .unwrap_or_default();
                    }
                    let packet = match read_packet_reusing(&mut stdout, &mut pixels) {
                        Ok(Some(packet)) => packet,
                        _ => {
                            reader_mailbox.fatal("HelperExited");
                            return;
                        }
                    };
                    let mut state = reader_mailbox
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    match packet {
                        Packet::Frame(frame) if frame.binding_generation == binding_generation => {
                            if let Some(old) = state.latest.replace(frame) {
                                state.acknowledge(old.sequence);
                                if state.pixels.len() < 2 {
                                    state.pixels.push(old.pixels);
                                }
                            }
                        }
                        Packet::Frame(frame) => state.acknowledge(frame.sequence),
                        Packet::Control(Control::Hello { protocol, version }) => {
                            if validate_helper_hello(protocol, &version).is_err() {
                                drop(state);
                                reader_mailbox.fatal("HelperVersionMismatch");
                                return;
                            }
                            state.hello = true;
                        }
                        Packet::Control(Control::Ready {
                            width,
                            height,
                            generation,
                        }) if !crate::ipc::valid_capture(width, height) || generation == 0 => {
                            drop(state);
                            reader_mailbox.fatal("HelperProtocolFailed");
                            return;
                        }
                        Packet::Control(Control::Pong { .. }) => state.pong = Instant::now(),
                        Packet::Control(control) => {
                            if matches!(control, Control::Fatal { .. } | Control::Disconnected) {
                                state.closed = true;
                            }
                            if state.events.len() >= 32 {
                                state.events.pop_front();
                            }
                            state.events.push_back(control);
                        }
                    }
                    reader_mailbox.changed.notify_all();
                    drop(state);
                    reader_mailbox.wake();
                }
            })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        {
            let mut state = mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
            while !state.hello
                && !state.closed
                && Instant::now() < deadline
                && control.check().is_ok()
            {
                state = mailbox
                    .changed
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
            if !state.hello || state.closed {
                let version_mismatch = state.events.iter().any(|event| {
                    matches!(event, Control::Fatal { code } if code == "HelperVersionMismatch")
                });
                drop(state);
                drop(resources);
                let _ = reader.join();
                control.check()?;
                return Err(if version_mismatch {
                    AdbError::HelperVersionMismatch
                } else {
                    AdbError::Protocol("helper handshake")
                });
            }
            state.input.push_back(Control::Connect {
                binding,
                endpoint: SocketAddr::from(([127, 0, 0, 1], resources.cleanup.port)),
                settings,
            });
        }
        let writer_mailbox = Arc::clone(&mailbox);
        let writer =
            nyaterm_core::runtime::worker::spawn("android-screen-writer".into(), move || {
                loop {
                    let control = {
                        let mut state = writer_mailbox
                            .state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner());
                        loop {
                            if let Some(control) = state.input.pop_front() {
                                break control;
                            }
                            if state.closed {
                                return;
                            }
                            state = writer_mailbox
                                .changed
                                .wait(state)
                                .unwrap_or_else(|p| p.into_inner());
                        }
                    };
                    if write_packet(&mut stdin, &Packet::Control(control)).is_err() {
                        writer_mailbox.fatal("HelperInputFailed");
                        return;
                    }
                }
            })?;
        let supervisor_mailbox = Arc::clone(&mailbox);
        let supervisor_runtime = runtime.clone();
        nyaterm_core::runtime::worker::spawn("android-screen-supervisor".into(), move || {
            let mut last_ping = Instant::now();
            let mut decoder = decoder;
            let mut sequence = 0;
            loop {
                if lifetime.check().is_err() || supervisor_runtime.check().is_err() {
                    supervisor_mailbox.fatal("DeviceDisconnected");
                    break;
                }
                {
                    let state = supervisor_mailbox
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if state.closed {
                        break;
                    }
                    if state.pong.elapsed() > Duration::from_secs(10) {
                        drop(state);
                        supervisor_mailbox.fatal("HelperHung");
                        break;
                    }
                }
                if resources
                    .helper
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
                    .is_some()
                {
                    supervisor_mailbox.fatal("HelperExited");
                    break;
                }
                if last_ping.elapsed() >= Duration::from_secs(1) {
                    sequence += 1;
                    let mut state = supervisor_mailbox
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if state.input.len() < INPUT_LIMIT {
                        state.input.push_back(Control::Ping { sequence });
                        supervisor_mailbox.changed.notify_all();
                    }
                    last_ping = Instant::now();
                }
                if let Some(server) = resources.server.as_mut() {
                    match server.read(&mut buffer) {
                        Ok(0) => {
                            supervisor_mailbox.fatal("DeviceDisconnected");
                            break;
                        }
                        Ok(count) => match decoder.push(&buffer[..count]) {
                            Ok(packets) if packets.iter().any(|packet| packet.id == EXIT) => {
                                supervisor_mailbox.fatal("ServerExited");
                                break;
                            }
                            Ok(_) => {}
                            Err(_) => {
                                supervisor_mailbox.fatal("ServerProtocolFailed");
                                break;
                            }
                        },
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                            ) => {}
                        Err(_) => {
                            supervisor_mailbox.fatal("DeviceDisconnected");
                            break;
                        }
                    }
                }
            }
            drop(resources);
            supervisor_mailbox.changed.notify_all();
            let _ = reader.join();
            let _ = writer.join();
            supervisor_mailbox
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .finished = true;
            supervisor_mailbox.changed.notify_all();
        })?;
        mailbox.changed.notify_all();
        {
            let mut state = mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
            while !state.closed
                && !state
                    .events
                    .iter()
                    .any(|event| matches!(event, Control::Ready { .. }))
            {
                if let Err(error) = control.check() {
                    state.closed = true;
                    mailbox.changed.notify_all();
                    return Err(error);
                }
                state = mailbox
                    .changed
                    .wait_timeout(state, Duration::from_millis(25))
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
            if state.closed {
                return Err(AdbError::Protocol("screen startup"));
            }
        }
        Ok(Self {
            mailbox,
            cleanup_ticket: ticket,
        })
    }

    pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.mailbox.waker.lock().unwrap_or_else(|p| p.into_inner()) = Some(waker);
        self.mailbox.wake();
    }
    pub fn send(&self, control: Control) -> io::Result<()> {
        control
            .validate_input()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut state = self.mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        // Visibility is state, so only its newest value needs to reach the
        // helper. Keep it deliverable even when device input has filled the queue.
        if matches!(
            control,
            Control::Presentation { .. }
                | Control::RefreshPresentation
                | Control::FrameAck { .. }
                | Control::ReleaseInputs
        ) {
            state.input.retain(|queued| {
                !matches!(
                    (&control, queued),
                    (Control::Presentation { .. }, Control::Presentation { .. })
                        | (Control::RefreshPresentation, Control::RefreshPresentation)
                        | (Control::ReleaseInputs, Control::ReleaseInputs)
                        | (Control::FrameAck { .. }, Control::FrameAck { .. })
                )
            });
            if state.input.len() >= INPUT_LIMIT {
                state.input.retain(|queued| {
                    matches!(
                        queued,
                        Control::Connect { .. }
                            | Control::FrameAck { .. }
                            | Control::Ping { .. }
                            | Control::Presentation { .. }
                            | Control::RefreshPresentation
                    )
                });
                state.input.push_back(Control::ReleaseInputs);
            }
            state.input.push_back(control);
            self.mailbox.changed.notify_all();
            return Ok(());
        }
        // Only adjacent move events may merge: never move a drag across Down/Up.
        if matches!(control, Control::Touch { action: 2, .. })
            && matches!(state.input.back(), Some(Control::Touch { action: 2, .. }))
        {
            state.input.pop_back();
        }
        if state.input.len() >= INPUT_LIMIT {
            // Input overflow must not lose presentation credit or a pending
            // handshake: otherwise decoding survives but display stalls forever.
            state.input.retain(|queued| {
                matches!(
                    queued,
                    Control::Connect { .. }
                        | Control::FrameAck { .. }
                        | Control::Ping { .. }
                        | Control::RefreshPresentation
                        | Control::Presentation { .. }
                )
            });
            state.input.push_back(Control::ReleaseInputs);
            self.mailbox.changed.notify_all();
            return Err(io::ErrorKind::WouldBlock.into());
        }
        state.input.push_back(control);
        self.mailbox.changed.notify_all();
        Ok(())
    }

    pub fn take_frame(&self) -> Option<Frame> {
        self.mailbox
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .latest
            .take()
    }
    /// Return presentation credit and a bounded reusable pixel allocation together.
    pub fn release_frame(&self, frame: Frame) {
        let sequence = frame.sequence;
        {
            let mut state = self.mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.pixels.len() < 2 {
                state.pixels.push(frame.pixels);
            }
        }
        self.acknowledge(sequence);
    }
    pub fn acknowledge(&self, sequence: u64) {
        let _ = self.send(Control::FrameAck { sequence });
    }
    pub fn events(&self) -> Vec<Control> {
        self.mailbox
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .events
            .drain(..)
            .collect()
    }
    pub fn is_closed(&self) -> bool {
        self.mailbox
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed
    }
    pub fn close(&self) {
        let mut state = self.mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return;
        }
        state.input.clear();
        state.input.push_back(Control::ReleaseInputs);
        state.input.push_back(Control::Disconnect);
        state.closed = true;
        self.mailbox.changed.notify_all();
    }
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
        while !state.finished && Instant::now() < deadline {
            state = self
                .mailbox
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        state.finished
    }
}

impl Drop for ScreenHandle {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn resolve_helper() -> AdbResult<PathBuf> {
    let name = if cfg!(windows) {
        "nyaterm-android-helper.exe"
    } else {
        "nyaterm-android-helper"
    };
    let path = if let Some(path) = std::env::var_os("NYATERM_ANDROID_HELPER") {
        PathBuf::from(path)
    } else {
        std::env::current_exe()?
            .parent()
            .ok_or(AdbError::ExecutableMissing)?
            .join(name)
    };
    path.is_file().then_some(path).ok_or(AdbError::Protocol(
        "Android screen helper is missing; build nyaterm-android-helper beside the application",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reader_credit_stays_bounded_when_helper_violates_backpressure() {
        use crate::ipc::Control;
        use crate::screen::Mailbox;
        let mailbox = Mailbox::new();
        let mut state = mailbox.state.lock().unwrap();
        for sequence in 1..1000 {
            state.acknowledge(sequence);
        }
        assert_eq!(state.input.len(), 1);
        assert!(matches!(
            state.input.front(),
            Some(Control::FrameAck { sequence: 999 })
        ));
    }
    #[test]
    fn mismatched_helper_protocol_or_package_version_is_explicitly_rejected() {
        use crate::error::AdbError;
        use crate::ipc::PROTOCOL_VERSION;
        use crate::screen::validate_helper_hello;
        assert!(validate_helper_hello(PROTOCOL_VERSION, env!("CARGO_PKG_VERSION")).is_ok());
        for (protocol, version) in [(2, env!("CARGO_PKG_VERSION")), (PROTOCOL_VERSION, "0.0.0")] {
            assert!(matches!(
                validate_helper_hello(protocol, version),
                Err(AdbError::HelperVersionMismatch)
            ));
        }
    }
    use crate::ipc::Control;
    use crate::screen::{INPUT_LIMIT, Mailbox, ScreenHandle};
    use std::sync::Arc;

    fn handle() -> ScreenHandle {
        ScreenHandle {
            mailbox: Arc::new(Mailbox::new()),
            cleanup_ticket: Arc::new(crate::screen::CleanupStatus::default()),
        }
    }

    #[test]
    fn input_overflow_releases_inputs_without_losing_frame_credit() {
        let handle = handle();
        handle.send(Control::FrameAck { sequence: 7 }).unwrap();
        for _ in 1..INPUT_LIMIT {
            handle
                .send(Control::Key {
                    action: 0,
                    keycode: 29,
                    repeat: 0,
                    metastate: 0,
                })
                .unwrap();
        }
        assert!(handle.send(Control::Rotate).is_err());
        let state = handle.mailbox.state.lock().unwrap();
        assert_eq!(state.input.len(), 2);
        assert!(matches!(state.input[0], Control::FrameAck { sequence: 7 }));
        assert!(matches!(state.input[1], Control::ReleaseInputs));
    }

    #[test]
    fn visibility_coalesces_and_survives_a_full_device_input_queue() {
        let handle = handle();
        for _ in 0..INPUT_LIMIT {
            handle.send(Control::Rotate).unwrap();
        }
        handle
            .send(Control::Presentation { visible: false })
            .unwrap();
        for index in 0..INPUT_LIMIT * 2 {
            handle
                .send(Control::Presentation {
                    visible: index % 2 == 0,
                })
                .unwrap();
        }
        let state = handle.mailbox.state.lock().unwrap();
        assert_eq!(state.input.len(), 2);
        assert!(matches!(state.input[0], Control::ReleaseInputs));
        assert!(matches!(
            state.input[1],
            Control::Presentation { visible: false }
        ));
    }

    #[test]
    fn refresh_and_credit_survive_overflow_and_pixel_pool_remains_bounded() {
        use crate::ipc::Frame;
        let handle = handle();
        handle.send(Control::RefreshPresentation).unwrap();
        for n in 0..1000 {
            handle.release_frame(Frame {
                sequence: n,
                binding_generation: 1,
                generation: 1,
                pts: 0,
                capture_width: 1,
                capture_height: 1,
                width: 1,
                height: 1,
                stride: 4,
                pixels: vec![0; 4],
            });
            let _ = handle.send(Control::Rotate);
        }
        let state = handle.mailbox.state.lock().unwrap();
        assert!(state.pixels.len() <= 2);
        assert!(state.input.len() <= INPUT_LIMIT);
        assert!(
            state
                .input
                .iter()
                .any(|v| matches!(v, Control::RefreshPresentation))
        );
        assert!(
            state
                .input
                .iter()
                .any(|v| matches!(v, Control::FrameAck { sequence: 999 }))
        );
    }
    #[test]
    fn drag_moves_merge_only_between_reliable_events() {
        let handle = handle();
        let touch = |action, x| Control::Touch {
            action,
            x,
            y: 1,
            width: 8,
            height: 8,
            generation: 1,
        };
        for control in [
            touch(0, 1),
            touch(2, 2),
            touch(2, 3),
            touch(1, 3),
            touch(0, 4),
            touch(2, 5),
        ] {
            handle.send(control).unwrap();
        }
        let state = handle.mailbox.state.lock().unwrap();
        assert_eq!(state.input.len(), 5);
        assert!(matches!(
            state.input[1],
            Control::Touch {
                action: 2,
                x: 3,
                ..
            }
        ));
        assert!(matches!(state.input[2], Control::Touch { action: 1, .. }));
        assert!(matches!(state.input[3], Control::Touch { action: 0, .. }));
    }
    #[test]
    fn invalid_text_is_rejected_before_queueing_and_screen_remains_usable() {
        let handle = handle();
        for text in [
            "private\0paste".to_owned(),
            "x".repeat(crate::ipc::MAX_TEXT_BYTES + 1),
        ] {
            let error = handle.send(Control::Text { text }).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(!handle.is_closed());
        }
        assert!(handle.mailbox.state.lock().unwrap().input.is_empty());
        handle.send(Control::Rotate).unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn cleanup_without_a_startup_pid_stops_only_the_exact_server_session() {
        use crate::adb::client::{AdbClient, DeviceBinding};
        use crate::screen::CleanupSpec;
        use std::process::{Child, Command};
        use std::time::{Duration, Instant};
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let scid = format!("{:08x}", uuid::Uuid::new_v4().as_u128() as u32);
        let spawn = |identity: &str| {
            OwnedChild(
                Command::new("python3")
                    .args([
                        "-c",
                        "import time; time.sleep(10)",
                        "com.genymobile.scrcpy.Server",
                        identity,
                    ])
                    .spawn()
                    .unwrap(),
            )
        };
        let mut owned = spawn(&format!("scid={scid}"));
        let mut unrelated = spawn(&format!("scid={scid}0"));
        let cleanup = CleanupSpec {
            client: AdbClient {
                executable: "unused".into(),
                server: "127.0.0.1:1".parse().unwrap(),
            },
            binding: DeviceBinding {
                server: "127.0.0.1:1".parse().unwrap(),
                serial: "fixture".into(),
                transport_id: None,
                generation: 1,
            },
            remote_path: "unused".into(),
            port: 0,
            pid: None,
            scid,
            last_error: None,
        };
        let command = String::from_utf8(cleanup.stop_server_command().unwrap()).unwrap();
        assert!(
            Command::new("sh")
                .args(["-c", &command])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while owned.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "owned server survived cleanup");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn completed_cleanup_is_collected_only_after_background_lock_is_released() {
        let ticket = crate::screen::CleanupStatus::default();
        assert!(!ticket.is_complete());
        ticket
            .finished
            .store(true, std::sync::atomic::Ordering::Release);
        let pending = ticket.lock().unwrap();
        assert!(!ticket.is_complete());
        drop(pending);
        assert!(ticket.is_complete());
    }
}

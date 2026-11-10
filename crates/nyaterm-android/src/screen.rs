//! Screen subprocess supervision. Handles enqueue work; cleanup never runs in a UI destructor.
use std::collections::VecDeque;
use std::io::{self, Read};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nyaterm_core::models::android::AndroidScreenSettings;
use sha2::{Digest, Sha256};

use crate::adb::client::{AdbClient, DeviceBinding};
use crate::adb::shell::{EXIT, STDOUT, ShellDecoder, execute_with_cancel, open_shell, quote};
use crate::adb::sync::SyncStream;
use crate::error::{AdbError, AdbResult};
use crate::ipc::{Control, Frame, PROTOCOL_VERSION, Packet, read_packet, write_packet};

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
    pub fn retry(&self) -> AdbResult<()> {
        if self.port != 0 {
            let _ = self.client.device_command(
                &self.binding,
                &["forward", "--remove", &format!("tcp:{}", self.port)],
            );
        }
        let mut command = Vec::new();
        if let Some(pid) = self.pid {
            // A PID can be reused. Verify the unique session argument before killing it.
            command.extend_from_slice(format!("if [ -r /proc/{pid}/cmdline ] && tr '\\000' ' ' < /proc/{pid}/cmdline | grep -F -- 'scid={}' >/dev/null; then kill {pid}; fi; ", self.scid).as_bytes());
        }
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

pub type CleanupTicket = Arc<Mutex<Option<CleanupSpec>>>;

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
    }
}

struct State {
    input: VecDeque<Control>,
    events: VecDeque<Control>,
    latest: Option<Frame>,
    closed: bool,
    finished: bool,
    hello: bool,
    pong: Instant,
}

struct Mailbox {
    state: Mutex<State>,
    changed: Condvar,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                input: VecDeque::new(),
                events: VecDeque::new(),
                latest: None,
                closed: false,
                finished: false,
                hello: false,
                pong: Instant::now(),
            }),
            changed: Condvar::new(),
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
        let ticket = Arc::new(Mutex::new(None));
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
        SyncStream::open(&binding)?.send(
            resources.cleanup.remote_path.as_bytes(),
            &mut payload,
            0o644,
            modified,
            |_| Ok(()),
        )?;
        let target = format!("localabstract:scrcpy_{}", resources.cleanup.scid);
        let port = resources
            .cleanup
            .client
            .device_command(&binding, &["forward", "tcp:0", &target])?;
        resources.cleanup.port = port
            .trim()
            .parse()
            .map_err(|_| AdbError::Protocol("forward port"))?;
        let mut command = b"echo $$; CLASSPATH=".to_vec();
        command.extend_from_slice(&quote(resources.cleanup.remote_path.as_bytes())?);
        command.extend_from_slice(format!(" exec app_process / com.genymobile.scrcpy.Server {SCRCPY_VERSION} scid={} tunnel_forward=true audio=false control=true video_codec=h264 max_size={} max_fps={} video_bit_rate={} clipboard_autosync=false cleanup=false power_on=false log_level=error", resources.cleanup.scid, settings.max_size, settings.max_fps, settings.bit_rate).as_bytes());
        let mut server = open_shell(&binding, &command, false)?;
        let mut decoder = ShellDecoder::default();
        let mut buffer = [0; 4096];
        let mut pid_bytes = Vec::new();
        while resources.cleanup.pid.is_none() {
            let count = server.read(&mut buffer)?;
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
                loop {
                    let packet = match read_packet(&mut stdout) {
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
                            state.latest = Some(frame)
                        }
                        Packet::Frame(frame) => state.input.push_back(Control::FrameAck {
                            sequence: frame.sequence,
                        }),
                        Packet::Control(Control::Hello { protocol, version }) => {
                            if protocol != PROTOCOL_VERSION || version != env!("CARGO_PKG_VERSION")
                            {
                                drop(state);
                                reader_mailbox.fatal("HelperVersionMismatch");
                                return;
                            }
                            state.hello = true;
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
                }
            })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        {
            let mut state = mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
            while !state.hello && !state.closed && Instant::now() < deadline {
                state = mailbox
                    .changed
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
            if !state.hello || state.closed {
                drop(state);
                drop(resources);
                let _ = reader.join();
                return Err(AdbError::Protocol("helper handshake"));
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
        nyaterm_core::runtime::worker::spawn("android-screen-supervisor".into(), move || {
            let mut last_ping = Instant::now();
            let mut decoder = decoder;
            let mut sequence = 0;
            loop {
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
        Ok(Self {
            mailbox,
            cleanup_ticket: ticket,
        })
    }

    pub fn send(&self, control: Control) -> io::Result<()> {
        let mut state = self.mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err(io::ErrorKind::BrokenPipe.into());
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
                    Control::Connect { .. } | Control::FrameAck { .. } | Control::Ping { .. }
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
    use crate::ipc::Control;
    use crate::screen::{INPUT_LIMIT, Mailbox, ScreenHandle};
    use std::sync::{Arc, Mutex};

    fn handle() -> ScreenHandle {
        ScreenHandle {
            mailbox: Arc::new(Mailbox::new()),
            cleanup_ticket: Arc::new(Mutex::new(None)),
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
}

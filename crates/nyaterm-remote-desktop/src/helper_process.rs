//! Process plumbing shared by the RDP and VNC helper session managers.
//!
//! Both protocols run their decoder in a child process and speak the typed IPC
//! packet protocol in [`crate::ipc`] over stdin/stdout. Only the control-message
//! vocabulary differs, so spawning, locating, and reaping the child lives here.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{Packet, write_packet};

/// How long a helper is given to exit on its own before it is killed.
const GRACEFUL_CLOSE_TIMEOUT: Duration = Duration::from_millis(750);
const RELIABLE_INPUT_LIMIT: usize = 256;
const CRITICAL_LIMIT: usize = 32;
const ORDINARY_CRITICAL_LIMIT: usize = CRITICAL_LIMIT - 1;

struct QueuedPacket {
    packet: Packet,
    completes_resync: bool,
}

#[derive(Default)]
struct WriterState {
    critical: VecDeque<QueuedPacket>,
    reliable: VecDeque<Packet>,
    latest_move: Option<Packet>,
    resyncing: bool,
    closed: bool,
    write_error: Option<String>,
}

struct WriterInner {
    state: Mutex<WriterState>,
    changed: Condvar,
}

/// Non-blocking application-side helper stdin writer.
///
/// UI callbacks only mutate this bounded mailbox. One dedicated thread owns
/// `ChildStdin`, so a stalled helper can never stall GPUI or a keyboard hook.
pub(crate) struct IpcWriter {
    inner: Arc<WriterInner>,
    worker: Option<JoinHandle<()>>,
}

impl IpcWriter {
    #[cfg(test)]
    pub(crate) fn test_mailbox() -> Self {
        Self {
            inner: Arc::new(WriterInner {
                state: Mutex::new(WriterState::default()),
                changed: Condvar::new(),
            }),
            worker: None,
        }
    }

    pub(crate) fn spawn(stdin: ChildStdin, thread_name: String) -> io::Result<Self> {
        let inner = Arc::new(WriterInner {
            state: Mutex::new(WriterState::default()),
            changed: Condvar::new(),
        });
        let worker_inner = Arc::clone(&inner);
        let worker = nyaterm_core::runtime::worker::spawn(thread_name, move || {
            writer_loop(stdin, worker_inner)
        })?;
        Ok(Self {
            inner,
            worker: Some(worker),
        })
    }

    pub(crate) fn send_critical(&self, packet: Packet) -> io::Result<()> {
        let mut state = self.lock_state()?;
        writer_ready(&state)?;
        // One slot is permanently reserved for overflow recovery. A saturated
        // control queue must never prevent ReleaseAllInputs from being queued.
        if state.critical.len() >= ORDINARY_CRITICAL_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "helper critical IPC queue is full",
            ));
        }
        state.critical.push_back(QueuedPacket {
            packet,
            completes_resync: false,
        });
        drop(state);
        self.inner.changed.notify_one();
        Ok(())
    }

    pub(crate) fn send_reliable(&self, packet: Packet, release_all: Packet) -> io::Result<()> {
        let mut state = self.lock_state()?;
        writer_ready(&state)?;
        if state.resyncing {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "remote input is waiting for state resynchronization",
            ));
        }
        if state.reliable.len() >= RELIABLE_INPUT_LIMIT {
            state.reliable.clear();
            state.latest_move = None;
            state.resyncing = true;
            state.critical.push_front(QueuedPacket {
                packet: release_all,
                completes_resync: true,
            });
            drop(state);
            self.inner.changed.notify_one();
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "remote input queue overflowed; releasing all remote input",
            ));
        }
        state.reliable.push_back(packet);
        drop(state);
        self.inner.changed.notify_one();
        Ok(())
    }

    pub(crate) fn send_latest_move(&self, packet: Packet) -> io::Result<()> {
        let mut state = self.lock_state()?;
        writer_ready(&state)?;
        if state.resyncing {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "remote input is waiting for state resynchronization",
            ));
        }
        state.latest_move = Some(packet);
        drop(state);
        self.inner.changed.notify_one();
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.closed = true;
        }
        self.inner.changed.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn lock_state(&self) -> io::Result<std::sync::MutexGuard<'_, WriterState>> {
        self.inner
            .state
            .lock()
            .map_err(|_| io::Error::other("helper IPC writer lock is poisoned"))
    }
}

impl Drop for IpcWriter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn writer_ready(state: &WriterState) -> io::Result<()> {
    if let Some(error) = &state.write_error {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, error.clone()));
    }
    if state.closed {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "helper IPC writer is closed",
        ));
    }
    Ok(())
}

fn writer_loop(mut stdin: ChildStdin, inner: Arc<WriterInner>) {
    loop {
        let queued = {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !state.closed
                && state.critical.is_empty()
                && state.reliable.is_empty()
                && state.latest_move.is_none()
            {
                state = inner
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.closed {
                return;
            }
            state
                .critical
                .pop_front()
                .or_else(|| {
                    state.reliable.pop_front().map(|packet| QueuedPacket {
                        packet,
                        completes_resync: false,
                    })
                })
                .or_else(|| {
                    state.latest_move.take().map(|packet| QueuedPacket {
                        packet,
                        completes_resync: false,
                    })
                })
        };
        let Some(queued) = queued else {
            continue;
        };
        if let Err(error) = write_packet(&mut stdin, &queued.packet) {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.write_error = Some(error.to_string());
            state.closed = true;
            inner.changed.notify_all();
            return;
        }
        if queued.completes_resync {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.resyncing = false;
        }
    }
}

/// A spawned helper with its pipes detached from the [`Child`].
pub(crate) struct HelperProcess {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildStdout,
    pub(crate) stderr: StderrCapture,
}

/// The pipe reader never invokes subscribers: a blocked log writer must not
/// stop draining the helper. Only bounded, validated metadata survives EOF.
pub(crate) struct StderrCapture {
    worker: JoinHandle<StderrSummary>,
    helper: &'static str,
}

impl StderrCapture {
    pub(crate) fn finish(self) {
        match self.worker.join() {
            Ok(summary) => tracing::debug!(
                helper = self.helper,
                discarded_lines = summary.discarded_lines,
                oversized_lines = summary.oversized_lines,
                read_failed = summary.read_failed,
                options = ?summary.options,
                "helper stderr summary (raw text discarded)"
            ),
            Err(_) => tracing::warn!(helper = self.helper, "helper stderr reader panicked"),
        }
    }
}

const STDERR_LINE_LIMIT: usize = 4096;

#[derive(Debug, Default, PartialEq, Eq)]
struct StderrSummary {
    options: Option<SessionOptions>,
    discarded_lines: u64,
    oversized_lines: u64,
    read_failed: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct SessionOptions {
    width: u32,
    height: u32,
    desktop_scale_factor: Option<u32>,
    color_depth: u32,
    graphics_requested: bool,
    graphics_effective: bool,
    performance_flags: u32,
    credssp_enabled: bool,
    administrative_session_requested: bool,
}

fn session_options(line: &[u8]) -> Option<SessionOptions> {
    fn decimal(value: &str) -> Option<u32> {
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| value.parse().ok())
            .flatten()
    }

    let text = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    let fields = text.strip_prefix("RDP session options ")?.split(' ');
    let mut width = None;
    let mut height = None;
    let mut desktop_scale_factor = None;
    let mut color_depth = None;
    let mut graphics_requested = None;
    let mut graphics_effective = None;
    let mut performance_flags = None;
    let mut credssp_enabled = None;
    let mut administrative_session_requested = None;
    for field in fields {
        let (name, value) = field.split_once('=')?;
        match name {
            "width" => width = decimal(value),
            "height" => height = decimal(value),
            "scale" => desktop_scale_factor = decimal(value),
            "depth" => color_depth = decimal(value),
            "gfx_requested" => graphics_requested = decimal(value),
            "gfx_effective" => graphics_effective = decimal(value),
            "perf" => performance_flags = u32::from_str_radix(value.strip_prefix("0x")?, 16).ok(),
            "nla" => credssp_enabled = decimal(value),
            "console" => administrative_session_requested = decimal(value),
            _ => return None,
        }
    }
    let width = width?;
    let height = height?;
    let color_depth = color_depth?;
    let graphics_requested = graphics_requested?;
    let graphics_effective = graphics_effective?;
    let performance_flags = performance_flags?;
    let credssp_enabled = credssp_enabled?;
    let administrative_session_requested = administrative_session_requested?;
    if width > 65535
        || height > 65535
        || desktop_scale_factor.is_some_and(|scale| scale > 500)
        || ![16, 24, 32].contains(&color_depth)
        || performance_flags & !0x1ff != 0
        || [
            graphics_requested,
            graphics_effective,
            credssp_enabled,
            administrative_session_requested,
        ]
        .iter()
        .any(|value| *value > 1)
    {
        return None;
    }
    Some(SessionOptions {
        width,
        height,
        desktop_scale_factor,
        color_depth,
        graphics_requested: graphics_requested != 0,
        graphics_effective: graphics_effective != 0,
        performance_flags,
        credssp_enabled: credssp_enabled != 0,
        administrative_session_requested: administrative_session_requested != 0,
    })
}

fn collect_stderr(mut reader: impl io::Read) -> StderrSummary {
    let mut summary = StderrSummary::default();
    let mut buffer = [0; 4096];
    let mut line = Vec::with_capacity(STDERR_LINE_LIMIT);
    let mut oversized = false;
    let finish_line = |line: &mut Vec<u8>, oversized: &mut bool, summary: &mut StderrSummary| {
        if *oversized {
            summary.oversized_lines = summary.oversized_lines.saturating_add(1);
        } else if !line.is_empty() && line.as_slice() != b"\r" {
            if let Some(options) = session_options(line) {
                summary.options = Some(options);
            } else {
                summary.discarded_lines = summary.discarded_lines.saturating_add(1);
            }
        }
        line.clear();
        *oversized = false;
    };
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                for byte in &buffer[..count] {
                    if *byte == b'\n' {
                        finish_line(&mut line, &mut oversized, &mut summary);
                    } else if line.len() < STDERR_LINE_LIMIT {
                        line.push(*byte);
                    } else {
                        oversized = true;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                summary.read_failed = true;
                break;
            }
        }
    }
    finish_line(&mut line, &mut oversized, &mut summary);
    summary
}

/// Locate a helper binary.
///
/// `env_var` overrides the search outright. Otherwise the helper must sit beside
/// the running executable: that is the layout every release package produces,
/// and `scripts/release/package_native.py` is what guarantees it.
pub(crate) fn resolve_helper(package: &str, env_var: &str) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(env_var).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| format!("{env_var} does not name an existing file"));
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve the NyaTerm executable: {error}"))?;
    let filename = if cfg!(windows) {
        format!("{package}.exe")
    } else {
        package.to_string()
    };
    let path = executable
        .parent()
        .map(|parent| parent.join(&filename))
        .ok_or_else(|| "the NyaTerm executable has no parent directory".to_string())?;
    if path.is_file() {
        Ok(path)
    } else {
        // Debug builds only produce the helper when its package is built too, so
        // name the command instead of leaving a bare missing-file error.
        Err(format!(
            "{filename} is missing beside NyaTerm ({}); build it with `cargo build -p {package}`",
            path.display()
        ))
    }
}

/// Spawn a helper with a continuously drained stderr and bounded metadata.
pub(crate) fn spawn_helper(path: &Path, log_target: &'static str) -> io::Result<HelperProcess> {
    let mut command = Command::new(path);
    configure_child(&mut command);
    spawn_helper_command(command, log_target)
}

fn spawn_helper_command(
    mut command: Command,
    log_target: &'static str,
) -> io::Result<HelperProcess> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let result = (|| {
        let stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "helper stdin is unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "helper stdout is unavailable")
        })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("helper stderr unavailable"))?;
        let worker = nyaterm_core::runtime::worker::spawn(
            format!("nyaterm-{log_target}-stderr"),
            move || collect_stderr(stderr),
        )?;
        Ok((
            stdin,
            stdout,
            StderrCapture {
                worker,
                helper: log_target,
            },
        ))
    })();
    let (stdin, stdout, stderr) = match result {
        Ok(pipes) => pipes,
        Err(error) => {
            // Never leave a live child with an undrained stderr pipe.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok(HelperProcess {
        child,
        stdin,
        stdout,
        stderr,
    })
}

/// Reap a helper, then join its reader thread.
///
/// The caller is expected to have already asked the helper to disconnect, so the
/// child normally exits within the grace period; killing is the fallback. Both
/// slots are cleared so a second call is a no-op.
pub(crate) fn cleanup_child(child: &mut Option<Child>, reader: &mut Option<JoinHandle<()>>) {
    if let Some(child) = child.as_mut() {
        let deadline = Instant::now() + GRACEFUL_CLOSE_TIMEOUT;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
    *child = None;
    if let Some(reader) = reader.take() {
        let _ = reader.join();
    }
}

#[cfg(windows)]
fn configure_child(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: helpers are console subsystem binaries and would flash a
    // console window otherwise.
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn configure_child(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};

    use crate::{PROTOCOL_VERSION, RdpControlMessage, encode_control};

    use super::{
        IpcWriter, RELIABLE_INPUT_LIMIT, WriterInner, WriterState, collect_stderr, resolve_helper,
        session_options,
    };

    fn packet(version: u32) -> crate::Packet {
        encode_control(&RdpControlMessage::ClientHello { version }).expect("small control packet")
    }

    fn mailbox_writer() -> IpcWriter {
        IpcWriter {
            inner: Arc::new(WriterInner {
                state: Mutex::new(WriterState::default()),
                changed: Condvar::new(),
            }),
            worker: None,
        }
    }

    #[test]
    fn stderr_keeps_only_validated_options_and_discards_private_text() {
        let options = b"RDP session options width=1920 height=1080 scale=100 depth=32 gfx_requested=1 gfx_effective=0 perf=0x86 nla=1 console=0";
        let mut input = b"password=secret username=private token=hidden\n\xff\n\r\n".to_vec();
        input.extend_from_slice(options);
        input.extend_from_slice(b"\r\n");
        input.extend_from_slice(options);
        input.extend_from_slice(b" password=secret\nlast private line");
        let summary = collect_stderr(input.as_slice());
        let options = session_options(options).unwrap();
        assert_eq!(options.desktop_scale_factor, Some(100));
        assert_eq!(summary.options, Some(options));
        assert_eq!(summary.discarded_lines, 4);
        assert!(!format!("{summary:?}").contains("secret"));
        assert!(session_options(b"RDP session options console=1").is_none());

        let legacy = b"RDP session options width=1920 height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0";
        assert_eq!(session_options(legacy).unwrap().desktop_scale_factor, None);

        let malformed: [&[u8]; 7] = [
            b"RDP session options width=70000 height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0",
            b"RDP session options width=1920 height=1080 scale=501 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0",
            b"RDP session options width=1920 height=1080 depth=8 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0",
            b"RDP session options width=1920 height=1080 depth=32 gfx_requested=2 gfx_effective=0 perf=0x86 nla=1 console=0",
            b"RDP session options width=1920 height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x200 nla=1 console=0",
            b"RDP session options width=1920 height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0 extra=1",
            b"RDP session options width=abc height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0",
        ];
        for line in malformed {
            assert!(
                session_options(line).is_none(),
                "out-of-range or malformed options must be rejected: {}",
                String::from_utf8_lossy(line)
            );
        }
    }

    #[test]
    fn stderr_drains_oversized_unterminated_lines_with_bounded_storage() {
        let input = vec![b'x'; 8 * 1024 * 1024];
        let summary = collect_stderr(input.as_slice());
        assert_eq!(summary.oversized_lines, 1);
        assert_eq!(summary.discarded_lines, 0);
        assert_eq!(summary.options, None);
        let mut input = input;
        input.extend_from_slice(b"\nprivate\n");
        let summary = collect_stderr(input.as_slice());
        assert_eq!(summary.oversized_lines, 1);
        assert_eq!(summary.discarded_lines, 1);
    }

    #[test]
    fn stderr_child_fixture() {
        let Some(mode) = std::env::var_os("NYATERM_TEST_STDERR_MODE") else {
            return;
        };
        use std::io::Write as _;
        let mut stderr = std::io::stderr().lock();
        for _ in 0..2048 {
            stderr.write_all(&[b'x'; 4096]).unwrap();
        }
        stderr.write_all(b"\nRDP session options width=1920 height=1080 depth=32 gfx_requested=0 gfx_effective=0 perf=0x86 nla=1 console=0\n").unwrap();
        stderr.flush().unwrap();
        drop(stderr);
        if mode == "crash" {
            std::process::exit(91);
        }
        if mode == "hang" {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }

    fn stderr_fixture_command(mode: &str) -> std::process::Command {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "helper_process::tests::stderr_child_fixture",
                "--nocapture",
            ])
            .env("NYATERM_TEST_STDERR_MODE", mode);
        super::configure_child(&mut command);
        command
    }

    #[test]
    fn stderr_pipe_drains_floods_and_is_joined_after_normal_crash_and_hung_children() {
        for mode in ["normal", "crash", "hang"] {
            let super::HelperProcess {
                child,
                stdin,
                stdout,
                stderr,
            } = super::spawn_helper_command(stderr_fixture_command(mode), "test").unwrap();
            drop(stdin);
            let start = std::time::Instant::now();
            let mut child = Some(child);
            if mode != "hang" {
                let deadline = start + std::time::Duration::from_secs(5);
                while child.as_mut().unwrap().try_wait().unwrap().is_none()
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            super::cleanup_child(&mut child, &mut None);
            assert!(child.is_none());
            let summary = stderr.worker.join().expect("stderr worker must terminate");
            // A hung helper can be killed before completing its flood.
            if mode != "hang" {
                assert_eq!(summary.oversized_lines, 1);
                assert!(summary.options.is_some());
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(6));
            drop(stdout);
        }
    }

    #[test]
    fn stderr_worker_creation_failure_reaps_the_child_before_returning() {
        let _failure = nyaterm_core::runtime::worker::fail_after(0);
        let start = std::time::Instant::now();
        let result = super::spawn_helper_command(stderr_fixture_command("hang"), "test");
        assert!(result.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn env_override_must_name_an_existing_file() {
        let error = resolve_helper("nyaterm-rdp-helper", "NYATERM_TEST_HELPER_OVERRIDE_MISSING")
            .expect_err("no helper sits beside the test binary");
        // The env var is unset, so the search falls through to the sibling lookup
        // and must name the package to build.
        assert!(
            error.contains("cargo build -p nyaterm-rdp-helper"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn env_override_pointing_at_a_directory_is_rejected() {
        let variable = "NYATERM_TEST_HELPER_OVERRIDE_DIR";
        // SAFETY-adjacent: this test owns a uniquely named variable, so it does
        // not race other tests in the same process.
        unsafe { std::env::set_var(variable, env!("CARGO_MANIFEST_DIR")) };
        let error = resolve_helper("nyaterm-vnc-helper", variable)
            .expect_err("a directory is not an executable");
        unsafe { std::env::remove_var(variable) };
        assert_eq!(error, format!("{variable} does not name an existing file"));
    }

    #[test]
    fn writer_mailbox_preserves_reliable_order_and_coalesces_moves() {
        let writer = mailbox_writer();
        let release = packet(PROTOCOL_VERSION);
        writer
            .send_reliable(packet(10), release.clone())
            .expect("first reliable packet");
        writer
            .send_reliable(packet(11), release)
            .expect("second reliable packet");
        writer.send_latest_move(packet(20)).expect("first move");
        writer.send_latest_move(packet(21)).expect("newer move");

        let state = writer.inner.state.lock().expect("writer state");
        assert_eq!(
            state
                .reliable
                .iter()
                .map(|packet| packet.payload.clone())
                .collect::<Vec<_>>(),
            vec![packet(10).payload, packet(11).payload]
        );
        assert_eq!(
            state
                .latest_move
                .as_ref()
                .map(|packet| packet.payload.clone()),
            Some(packet(21).payload)
        );
    }

    #[test]
    fn writer_mailbox_overflow_schedules_release_and_pauses_input() {
        let writer = mailbox_writer();
        let release = packet(PROTOCOL_VERSION);
        for index in 0..RELIABLE_INPUT_LIMIT {
            writer
                .send_reliable(packet(index as u32), release.clone())
                .expect("queue has bounded capacity");
        }
        writer.send_latest_move(packet(99)).expect("pending move");

        let error = writer
            .send_reliable(packet(100), release.clone())
            .expect_err("overflow must be visible");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(
            writer
                .send_latest_move(packet(101))
                .expect_err("input pauses until release is written")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );

        let state = writer.inner.state.lock().expect("writer state");
        assert!(state.reliable.is_empty());
        assert!(state.latest_move.is_none());
        assert!(state.resyncing);
        assert_eq!(state.critical.len(), 1);
        assert_eq!(
            state.critical.front().map(|queued| &queued.packet),
            Some(&release)
        );
        assert!(
            state
                .critical
                .front()
                .is_some_and(|queued| queued.completes_resync)
        );
    }
}

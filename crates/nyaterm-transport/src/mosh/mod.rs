//! Mosh session ownership. SSH bootstraps the server; UDP and auxiliary SSH have separate lifetimes.
mod bootstrap;
mod projection;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nyaterm_core::mosh::{MoshPrediction, MoshSettings};
use nyaterm_mosh::client::{Client, ConnectionStatus};
use nyaterm_mosh::framebuffer::Framebuffer;
use nyaterm_mosh::prediction::{DisplayPipeline, DisplayPreference};
use nyaterm_terminal::prediction::TerminalPrediction;

use crate::session_event_queue::SessionEventQueue;
use crate::session_notice::SessionNotice;
use crate::{SessionEvent, SessionInfo, SshMultiplexHandle, SshSessionConfig, TerminalTransport};

const INPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub struct MoshSessionConfig {
    pub ssh: SshSessionConfig,
    pub settings: MoshSettings,
}

/// One ordered confirmed update and its matching display-only prediction.
#[derive(Clone, PartialEq, Eq)]
pub struct MoshFrame {
    pub sequence: u64,
    pub cols: u16,
    pub rows: u16,
    pub data: Vec<u8>,
    pub prediction: Arc<TerminalPrediction>,
}

impl std::fmt::Debug for MoshFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoshFrame")
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

impl MoshFrame {
    pub fn byte_len(&self) -> usize {
        // Account for empty state updates too: a stalled consumer must not
        // accumulate an unbounded number of metadata-only frames.
        std::mem::size_of::<Self>()
            + self.data.len()
            + std::mem::size_of::<TerminalPrediction>()
            + self
                .prediction
                .cells
                .iter()
                .map(|patch| std::mem::size_of_val(patch) + patch.cell.text().len())
                .sum::<usize>()
    }
}

#[derive(Default)]
struct Input {
    keys: VecDeque<Vec<u8>>,
    bytes: usize,
    unconfirmed: usize,
    closed: bool,
    resize: Option<(u16, u16)>,
}

struct Publication {
    confirmed: Framebuffer,
    prediction: Arc<TerminalPrediction>,
    sequence: u64,
}

#[derive(Default)]
struct LatestFrame {
    frame: Option<Publication>,
    ended: bool,
    reason: Option<String>,
    status: Option<(bool, u64)>,
}

type FrameMailbox = Arc<(Mutex<LatestFrame>, Condvar)>;

pub(crate) struct MoshTransport {
    pub(crate) info: SessionInfo,
    input: Arc<(Mutex<Input>, Condvar)>,
    stopped: Arc<AtomicBool>,
    frames: FrameMailbox,
    events: SessionEventQueue,
    network: Option<JoinHandle<()>>,
    delivery: Option<JoinHandle<()>>,
    backspace: bool,
}

impl MoshTransport {
    pub(crate) fn open(
        info: SessionInfo,
        config: MoshSessionConfig,
        connection: SshMultiplexHandle,
        events: SessionEventQueue,
    ) -> Result<Self, String> {
        config.settings.validate().map_err(str::to_owned)?;
        config.ssh.attempt.check()?;
        let endpoint = bootstrap::start(&config, &connection)?;
        let mut hosts = Vec::new();
        if !config.settings.udp_host.is_empty() {
            hosts.push(config.settings.udp_host.as_str());
        } else {
            if let Some(address) = endpoint.address.as_deref() {
                hosts.push(address);
            }
            hosts.push(config.ssh.host.as_str());
        }
        let mut client = Client::dial_candidates_with_size(
            &hosts,
            endpoint.port,
            &endpoint.key,
            info.cols,
            info.rows,
        )
        .map_err(|_| "Unable to open Mosh UDP endpoint")?;
        drop(endpoint);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !client.is_attached() {
            if config.ssh.attempt.is_cancelled() || Instant::now() >= deadline || client.is_dead() {
                let _ = client.graceful_shutdown(Duration::from_millis(500));
                config.ssh.attempt.check()?;
                return Err(
                    "No authenticated response from Mosh server; check UDP connectivity".into(),
                );
            }
            if client.poll().is_err() {
                let _ = client.graceful_shutdown(Duration::from_millis(500));
                return Err("Mosh rejected the remote terminal state".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let input = Arc::new((Mutex::new(Input::default()), Condvar::new()));
        let frames = Arc::new((Mutex::new(LatestFrame::default()), Condvar::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_input = input.clone();
        let worker_frames = frames.clone();
        let worker_stop = stopped.clone();
        let post_login = config
            .ssh
            .post_login
            .clone()
            .filter(|command| command.enabled && !command.command.trim().is_empty());
        let preference = match config.settings.prediction {
            MoshPrediction::Adaptive => DisplayPreference::Adaptive,
            MoshPrediction::Always => DisplayPreference::Always,
            MoshPrediction::Never => DisplayPreference::Never,
        };
        let network =
            nyaterm_core::runtime::worker::spawn(format!("mosh-{}", info.id), move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_network(
                        &mut client,
                        preference,
                        &worker_input,
                        &worker_frames,
                        &worker_stop,
                        post_login,
                    )
                }));
                let _ = client.graceful_shutdown(Duration::from_secs(2));
                let mut latest = worker_frames.0.lock().unwrap_or_else(|p| p.into_inner());
                worker_input
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .closed = true;
                latest.ended = true;
                latest.reason = Some(match result {
                    Ok(Ok(())) => "Mosh session closed".into(),
                    Ok(Err(error)) => error,
                    Err(_) => "Mosh worker failed".into(),
                });
                worker_frames.1.notify_all();
            })
            .map_err(|_| "Unable to start Mosh worker")?;
        let delivery_frames = frames.clone();
        let delivery_stop = stopped.clone();
        let delivery_id = info.id.clone();
        let delivery_events = events.clone();
        let delivery = match nyaterm_core::runtime::worker::spawn(
            format!("mosh-output-{}", info.id),
            move || {
                deliver_frames(
                    &delivery_id,
                    &delivery_frames,
                    &delivery_stop,
                    &delivery_events,
                );
            },
        ) {
            Ok(worker) => worker,
            Err(_) => {
                stopped.store(true, Ordering::Release);
                input.1.notify_all();
                let _ = network.join();
                return Err("Unable to start Mosh output worker".into());
            }
        };
        Ok(Self {
            info,
            input,
            frames,
            events,
            stopped,
            network: Some(network),
            delivery: Some(delivery),
            backspace: matches!(
                config.ssh.backspace_mode.as_str(),
                "ctrl_h" | "ctrl-h" | "bs"
            ),
        })
    }

    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.events.cancel_session(&self.info.id);
        self.input.1.notify_all();
        self.frames.1.notify_all();
    }
}

impl Drop for MoshTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl TerminalTransport for MoshTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Mosh session is closed"
        );
        let mut input = self
            .input
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Mosh input unavailable"))?;
        anyhow::ensure!(!input.closed, "Mosh session is closed");
        anyhow::ensure!(
            input
                .bytes
                .saturating_add(input.unconfirmed)
                .saturating_add(data.len())
                <= INPUT_LIMIT,
            "Mosh input queue is full; wait for the server to acknowledge input"
        );
        input.bytes += data.len();
        input.keys.push_back(
            data.iter()
                .map(|b| if self.backspace && *b == 127 { 8 } else { *b })
                .collect(),
        );
        self.input.1.notify_one();
        Ok(())
    }
    fn write_raw(&mut self, _data: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("Raw terminal file transfer is unavailable over Mosh")
    }
    fn resize(&mut self, cols: u16, rows: u16, _width: u16, _height: u16) -> anyhow::Result<()> {
        let mut input = self
            .input
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Mosh input unavailable"))?;
        input.resize = Some((cols.max(1), rows.max(1)));
        self.info.cols = cols.max(1);
        self.info.rows = rows.max(1);
        self.input.1.notify_one();
        Ok(())
    }
    fn close(&mut self) -> anyhow::Result<()> {
        self.stop();
        if let Some(worker) = self.network.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.delivery.take() {
            let _ = worker.join();
        }
        Ok(())
    }
}

fn run_network(
    client: &mut Client,
    preference: DisplayPreference,
    input: &Arc<(Mutex<Input>, Condvar)>,
    frames: &FrameMailbox,
    stopped: &AtomicBool,
    mut post_login: Option<nyaterm_core::models::sessions::ConnectionPostLogin>,
) -> Result<(), String> {
    let initial = client.remote_framebuffer();
    let mut display = DisplayPipeline::new(initial.cols, initial.rows, preference);
    let mut remote_num = u64::MAX;
    let mut sequence = 0;
    let mut last_status = (true, 0);
    let connected_at = Instant::now();
    loop {
        if stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if post_login.as_ref().is_some_and(|command| {
            connected_at.elapsed() >= Duration::from_millis(command.delay_ms)
        }) {
            let command = post_login.take().expect("post-login command");
            let mut text = command.command.replace("\r\n", "\r").replace('\n', "\r");
            if !text.ends_with('\r') {
                text.push('\r');
            }
            if text.len() <= INPUT_LIMIT {
                client.send_keys(text.as_bytes());
            }
        }
        client
            .poll()
            .map_err(|_| "Mosh remote state was rejected")?;
        let mut changed = false;
        if client.remote_state_num() != remote_num {
            remote_num = client.remote_state_num();
            display.on_host_frame(client.remote_framebuffer());
            changed = true;
        }
        changed |= !display.set_srtt(Some(client.send_interval())).is_empty();
        changed |= !display
            .set_frames(
                client.sent_num(),
                client.acked_by_remote(),
                client.echo_ack(),
            )
            .is_empty();
        changed |= !display.tick(Instant::now()).is_empty();
        let mut pending = input.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cols, rows)) = pending.resize.take() {
            client.resize(cols, rows);
            display.resize(cols as usize, rows as usize);
            changed = true;
        }
        while let Some(keys) = pending.keys.pop_front() {
            pending.bytes -= keys.len();
            changed |= !display.on_keystroke(&keys).is_empty();
            client.send_keys(&keys);
        }
        pending.unconfirmed = client.pending_input_bytes();
        drop(pending);
        if changed {
            sequence += 1;
            let confirmed = client.remote_framebuffer().clone();
            let prediction = Arc::new(projection::prediction(
                &confirmed,
                &display.predicted_frame(),
            ));
            let mut latest = frames.0.lock().unwrap_or_else(|p| p.into_inner());
            latest.frame = Some(Publication {
                confirmed,
                prediction,
                sequence,
            });
            frames.1.notify_one();
        }
        let status = match client.connection_status() {
            ConnectionStatus::Online => (true, 0),
            ConnectionStatus::Connecting(age)
            | ConnectionStatus::LastContact(age)
            | ConnectionStatus::LastReply(age) => (false, age.as_secs()),
        };
        if status != last_status {
            last_status = status;
            frames.0.lock().unwrap_or_else(|p| p.into_inner()).status = Some(status);
            frames.1.notify_one();
        }
        if client.is_dead() || client.remote_shutdown_ack_sent() {
            return Ok(());
        }
        let pending = input.0.lock().unwrap_or_else(|p| p.into_inner());
        if pending.keys.is_empty() && pending.resize.is_none() {
            let _ = input.1.wait_timeout(pending, Duration::from_millis(8));
        }
    }
}

fn deliver_frames(
    id: &str,
    frames: &FrameMailbox,
    stopped: &AtomicBool,
    events: &SessionEventQueue,
) {
    let mut previous = None;
    loop {
        let mut latest = frames.0.lock().unwrap_or_else(|p| p.into_inner());
        while latest.frame.is_none()
            && latest.status.is_none()
            && !latest.ended
            && !stopped.load(Ordering::Acquire)
        {
            latest = frames.1.wait(latest).unwrap_or_else(|p| p.into_inner());
        }
        if stopped.load(Ordering::Acquire) {
            return;
        }
        if let Some((online, seconds)) = latest.status.take() {
            drop(latest);
            events.push(SessionEvent::Notice {
                session_id: id.into(),
                notice: SessionNotice::MoshNetwork { online, seconds },
            });
            continue;
        }
        if let Some(frame) = latest.frame.take() {
            drop(latest);
            let mut data = frame.confirmed.diff(previous.as_ref());
            if previous.is_none() {
                let mut initial = b"\x1b[?1049h".to_vec();
                initial.extend(data);
                data = initial;
            }
            let output = MoshFrame {
                sequence: frame.sequence,
                cols: frame.confirmed.cols as u16,
                rows: frame.confirmed.rows as u16,
                data,
                prediction: frame.prediction,
            };
            if !events.push_mosh_frame(id.to_owned(), output) {
                return;
            }
            previous = Some(frame.confirmed);
        } else if latest.ended {
            let reason = latest
                .reason
                .take()
                .unwrap_or_else(|| "Mosh session closed".into());
            drop(latest);
            events.push(SessionEvent::Exited {
                session_id: id.into(),
                reason,
            });
            return;
        }
    }
}

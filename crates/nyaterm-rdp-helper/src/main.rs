use std::io;
use std::io::{BufWriter, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use ironrdp_client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp_client::rdp::{
    AutoReconnectDecision, RdpClient, RdpInputEvent as IronInput, RdpInputSender, RdpOutputEvent,
};
use ironrdp_input::{
    Database as InputDatabase, MouseButton, MousePosition, Operation, Scancode, WheelRotations,
};
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use nyaterm_remote_desktop::{
    CursorPosition, CursorShape, CursorVisibility, PROTOCOL_VERSION, Packet, PixelFormat,
    RDP_FRAMEBUFFER_LIMITS, RdpCapability, RdpCertificateRequest, RdpCertificateResponse,
    RdpControlMessage, RdpError, RdpErrorKind, RdpFrameEvent, RdpInputEvent, RdpServerCapabilities,
    RdpSessionConfig, RdpSessionState, RemoteCursorEvent, RemotePoint, RemotePointerButton,
    RemotePointerEvent, RemoteWheelAxis, decode_control, encode_control, encode_cursor_packet,
    encode_frame_packet_owned, read_packet, validate_committed_text,
    validate_framebuffer_dimensions, write_packet_into,
};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;
use x509_cert::der::Decode as _;

mod clipboard;

use clipboard::ClipboardBridge;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const STDOUT_BUFFER_BYTES: usize = 256 * 1024;

enum Outbound {
    Control(RdpControlMessage),
    Packet(Packet),
}

#[derive(Default)]
struct CertificateGate {
    response: Mutex<Option<(String, RdpCertificateResponse)>>,
    changed: Condvar,
    waiting: AtomicBool,
}

fn validate_control_phase(message: &RdpControlMessage, hello_received: bool) -> anyhow::Result<()> {
    if !hello_received {
        if matches!(message, RdpControlMessage::ClientHello { .. }) {
            return Ok(());
        }
        anyhow::bail!("RDP IPC expected ClientHello before any other message");
    }

    match message {
        RdpControlMessage::ClientHello { .. } => {
            anyhow::bail!("duplicate RDP IPC ClientHello")
        }
        RdpControlMessage::ServerHello { .. }
        | RdpControlMessage::DesktopReset { .. }
        | RdpControlMessage::State { .. }
        | RdpControlMessage::CertificateRequest(_)
        | RdpControlMessage::Capability { .. }
        | RdpControlMessage::Error { .. } => {
            anyhow::bail!("RDP IPC helper-only message received from application")
        }
        RdpControlMessage::Connect { .. }
        | RdpControlMessage::Input { .. }
        | RdpControlMessage::SecureAttention { .. }
        | RdpControlMessage::Resize { .. }
        | RdpControlMessage::Clipboard { .. }
        | RdpControlMessage::CertificateResponse { .. }
        | RdpControlMessage::RequestFullFrame { .. }
        | RdpControlMessage::Disconnect { .. } => Ok(()),
    }
}

fn validate_active_session_id(
    active_session_id: Option<&str>,
    received: &str,
) -> anyhow::Result<()> {
    if let Some(active) = active_session_id
        && active != received
    {
        anyhow::bail!(
            "RDP IPC session id mismatch: active session is '{active}', received '{received}'"
        );
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let provider_error = install_crypto_provider()
        .err()
        .map(|error| error.to_string());
    match std::env::var("NYATERM_RDP_HELPER_TEST_MODE").as_deref() {
        Ok("crash") => std::process::exit(91),
        Ok("hang") => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        _ => {}
    }
    if let Err(error) = run(provider_error).await {
        eprintln!("RDP helper stopped: {error}");
        std::process::exit(1);
    }
}

fn install_crypto_provider() -> anyhow::Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install the rustls aws-lc-rs CryptoProvider"))
}

async fn run(provider_error: Option<String>) -> anyhow::Result<()> {
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let writer = spawn_stdout_writer(output_rx)?;
    let (control_tx, mut control_rx) =
        tokio_mpsc::unbounded_channel::<io::Result<RdpControlMessage>>();
    let reader = spawn_stdin_reader(control_tx)?;
    let certificate_gate = Arc::new(CertificateGate::default());
    let mut iron_input = None;
    let mut client_task = None;
    let mut output_task = None;
    let mut input_state = InputDatabase::default();
    let mut clipboard_bridge = None;
    let mut hello_received = false;
    let mut active_session_id: Option<String> = None;

    while let Some(message) = control_rx.recv().await {
        let message = message?;
        validate_control_phase(&message, hello_received)?;
        match message {
            RdpControlMessage::ClientHello { version } => {
                if version != PROTOCOL_VERSION {
                    anyhow::bail!("RDP IPC protocol version mismatch");
                }
                send_control(
                    &output_tx,
                    RdpControlMessage::ServerHello {
                        version: PROTOCOL_VERSION,
                        capabilities: RdpServerCapabilities {
                            committed_unicode_text: true,
                            secure_attention: true,
                        },
                    },
                )?;
                hello_received = true;
            }
            RdpControlMessage::Connect { session_id, config } => {
                if let Some(error) = provider_error.as_deref() {
                    send_error(&output_tx, &session_id, RdpErrorKind::Protocol, error, true)?;
                    return Ok(());
                }
                if client_task.is_some() {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Protocol,
                        "helper already owns an RDP session",
                        true,
                    )?;
                    continue;
                }
                send_control(
                    &output_tx,
                    RdpControlMessage::State {
                        session_id: session_id.clone(),
                        state: RdpSessionState::Connecting,
                        message: None,
                    },
                )?;
                let connection_gate = certificate_gate.clone();
                let (iron_config, bridge) = build_config(
                    &session_id,
                    &config,
                    output_tx.clone(),
                    connection_gate.clone(),
                )?;
                let (rdp_output_tx, rdp_output_rx) = tokio_mpsc::channel(64);
                let client = RdpClient::new(iron_config, rdp_output_tx);
                let input_sender = client.input_sender();
                bridge.set_input_sender(input_sender.clone());
                iron_input = Some(input_sender.clone());
                clipboard_bridge = Some(bridge);
                let client_session_id = session_id.clone();
                let client_output_tx = output_tx.clone();
                client_task = Some(
                    thread::Builder::new()
                        .name("nyaterm-ironrdp".to_string())
                        .spawn(move || {
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("failed to create IronRDP runtime");
                                runtime.block_on(client.run());
                            }));
                            if let Err(payload) = result {
                                report_ironrdp_panic(
                                    &client_output_tx,
                                    &client_session_id,
                                    payload,
                                );
                            }
                        })?,
                );
                let connected_session_id = session_id.clone();
                output_task = Some(tokio::spawn(forward_output(
                    session_id,
                    rdp_output_rx,
                    output_tx.clone(),
                    input_sender,
                    connection_gate,
                    CONNECTION_TIMEOUT,
                )));
                active_session_id = Some(connected_session_id);
            }
            RdpControlMessage::Input { session_id, events } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                let Some(sender) = iron_input.as_ref() else {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Protocol,
                        "RDP session is not connected",
                        false,
                    )?;
                    continue;
                };
                validate_input_events(&events)?;
                for event in events {
                    convert_and_send_input(sender, event, &mut input_state).await?;
                }
            }
            RdpControlMessage::SecureAttention { session_id } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                let Some(sender) = iron_input.as_ref() else {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Protocol,
                        "RDP session is not connected",
                        false,
                    )?;
                    continue;
                };
                send_reliable_iron_input(
                    sender,
                    IronInput::FastPath(secure_attention_input(&input_state)),
                )
                .await?;
            }
            RdpControlMessage::Resize {
                session_id,
                metrics,
            } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                let Some(sender) = iron_input.as_ref() else {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Protocol,
                        "RDP session is not connected",
                        false,
                    )?;
                    continue;
                };
                let width = u16::try_from(metrics.width).unwrap_or(u16::MAX);
                let height = u16::try_from(metrics.height).unwrap_or(u16::MAX);
                send_iron_input(
                    sender,
                    IronInput::Resize {
                        width,
                        height,
                        scale_factor: metrics.desktop_scale_factor,
                        physical_size: metrics.physical_size_mm,
                    },
                )?;
            }
            RdpControlMessage::RequestFullFrame { session_id } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                if let Some(sender) = iron_input.as_ref() {
                    send_iron_input(sender, IronInput::RequestFullFrame)?;
                } else {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Protocol,
                        "RDP session is not connected",
                        false,
                    )?;
                }
            }
            RdpControlMessage::CertificateResponse {
                request_id,
                response,
            } => {
                let mut slot = certificate_gate
                    .response
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *slot = Some((request_id, response));
                certificate_gate.changed.notify_all();
            }
            RdpControlMessage::Clipboard {
                session_id,
                text,
                generation: _,
            } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                let Some(bridge) = clipboard_bridge.as_ref() else {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Clipboard,
                        "RDP clipboard channel is not connected",
                        false,
                    )?;
                    continue;
                };
                if let Err(error) = bridge.set_local_text(text) {
                    send_error(
                        &output_tx,
                        &session_id,
                        RdpErrorKind::Clipboard,
                        &error.to_string(),
                        false,
                    )?;
                }
            }
            RdpControlMessage::Disconnect { session_id } => {
                validate_active_session_id(active_session_id.as_deref(), &session_id)?;
                send_control(
                    &output_tx,
                    RdpControlMessage::State {
                        session_id: session_id.clone(),
                        state: RdpSessionState::Disconnecting,
                        message: None,
                    },
                )?;
                if let Some(sender) = iron_input.take() {
                    // Bypasses the bounded input queue on purpose: a full queue must
                    // not be able to stop a disconnect from being requested.
                    sender.request_graceful_close();
                }
                if let Some(task) = client_task.take() {
                    let deadline = std::time::Instant::now() + Duration::from_millis(700);
                    while !task.is_finished() && std::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    if task.is_finished() {
                        let _ = task.join();
                    }
                }
                if let Some(task) = output_task.take() {
                    let _ = tokio::time::timeout(Duration::from_millis(50), task).await;
                }
                send_control(
                    &output_tx,
                    RdpControlMessage::State {
                        session_id,
                        state: RdpSessionState::Disconnected,
                        message: None,
                    },
                )?;
                break;
            }
            RdpControlMessage::ServerHello { .. }
            | RdpControlMessage::DesktopReset { .. }
            | RdpControlMessage::State { .. }
            | RdpControlMessage::CertificateRequest(_)
            | RdpControlMessage::Capability { .. }
            | RdpControlMessage::Error { .. } => {
                anyhow::bail!("RDP IPC helper-only message received from application");
            }
        }
    }

    if let Some(sender) = iron_input {
        sender.request_graceful_close();
    }
    drop(output_tx);
    let _ = reader.join();
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("RDP stdout writer panicked"))??;
    Ok(())
}

/// Pump outbound packets to stdout.
///
/// Everything already queued is coalesced into one buffered write and flushed
/// only once the queue drains, so a burst of frame packets costs a handful of
/// write syscalls instead of one per packet. Latency is unchanged: whenever the
/// producer is not ahead, each packet is flushed immediately.
fn spawn_stdout_writer(
    output_rx: mpsc::Receiver<Outbound>,
) -> io::Result<thread::JoinHandle<io::Result<()>>> {
    thread::Builder::new()
        .name("nyaterm-rdp-stdout".to_string())
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
        Outbound::Control(message) => encode_control(&message)?,
        Outbound::Packet(packet) => packet,
    };
    write_packet_into(writer, &packet)
}

fn spawn_stdin_reader(
    control_tx: tokio_mpsc::UnboundedSender<io::Result<RdpControlMessage>>,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("nyaterm-rdp-stdin".to_string())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                match read_packet(&mut stdin)
                    .and_then(|packet| packet.map(|packet| decode_control(&packet)).transpose())
                {
                    Ok(Some(message)) => {
                        if control_tx.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = control_tx.send(Err(error));
                        break;
                    }
                }
            }
        })
}

fn build_config(
    session_id: &str,
    config: &RdpSessionConfig,
    output_tx: mpsc::SyncSender<Outbound>,
    certificate_gate: Arc<CertificateGate>,
) -> anyhow::Result<(ironrdp_client::config::Config, Arc<ClipboardBridge>)> {
    let port = config.port;
    let certificate_output_tx = output_tx.clone();
    let host = config.host.clone();
    // IronRDP calls this only when the platform trust store rejects the chain, which
    // for RDP is the common case (self-signed host certificates). A chain the store
    // already trusts is accepted without a prompt.
    let verifier: ironrdp_tls::CertificateValidationCallback =
        Arc::new(move |der, endpoint, validation_error| {
            let _ = endpoint;
            let request_id = Uuid::new_v4().to_string();
            let fingerprint = Sha256::digest(der)
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(":");
            let parsed = x509_cert::Certificate::from_der(der).ok();
            let request = RdpCertificateRequest {
                request_id: request_id.clone(),
                host: host.clone(),
                port,
                sha256_fingerprint: fingerprint,
                subject: parsed
                    .as_ref()
                    .map(|cert| cert.tbs_certificate.subject.to_string()),
                issuer: parsed
                    .as_ref()
                    .map(|cert| cert.tbs_certificate.issuer.to_string()),
                valid_from: parsed
                    .as_ref()
                    .map(|cert| cert.tbs_certificate.validity.not_before.to_string()),
                valid_to: parsed
                    .as_ref()
                    .map(|cert| cert.tbs_certificate.validity.not_after.to_string()),
            };
            let _ = validation_error;
            if certificate_output_tx
                .send(Outbound::Control(RdpControlMessage::CertificateRequest(
                    request,
                )))
                .is_err()
            {
                return false;
            }
            certificate_gate.waiting.store(true, Ordering::Release);
            let deadline = std::time::Instant::now() + Duration::from_secs(120);
            let mut slot = certificate_gate
                .response
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let accepted = loop {
                if let Some((response_id, response)) = slot.take()
                    && response_id == request_id
                {
                    break !matches!(response, RdpCertificateResponse::Reject);
                }
                let now = std::time::Instant::now();
                if now >= deadline {
                    break false;
                }
                let waited = certificate_gate.changed.wait_timeout(slot, deadline - now);
                slot = match waited {
                    Ok((slot, _)) => slot,
                    Err(poisoned) => poisoned.into_inner().0,
                };
            };
            certificate_gate.waiting.store(false, Ordering::Release);
            accepted
        });
    let destination = Destination::new(format!("{}:{}", config.host, config.port))?;
    let platform = if cfg!(target_os = "windows") {
        MajorPlatformType::WINDOWS
    } else if cfg!(target_os = "macos") {
        MajorPlatformType::OSX
    } else {
        MajorPlatformType::UNIX
    };
    let color_depth = if config.display.color_depth == 16 {
        16
    } else {
        32
    };
    let clipboard_enabled = !matches!(
        config.clipboard.mode,
        nyaterm_remote_desktop::RdpClipboardMode::Disabled
    );
    let clipboard = ClipboardBridge::new(session_id.to_string(), output_tx);
    let clipboard_factory = clipboard.clone();
    let use_credssp = config.use_nla && config.password.is_some();
    let mut builder = ConfigBuilder::new()
        .with_destination(destination)
        .with_username(config.username.clone())
        .with_domain(config.domain.clone())
        .with_password(
            config
                .password
                .as_ref()
                .map(|password| password.expose_secret().to_owned())
                .unwrap_or_default(),
        )
        .with_client_build(10_001)
        .with_client_dir("C:\\Windows\\System32\\mstscax.dll")
        .with_client_name("NYATERM")
        .with_platform(platform)
        .with_desktop_width(u16::try_from(config.display.width).unwrap_or(1920))
        .with_desktop_height(u16::try_from(config.display.height).unwrap_or(1080))
        .with_color_depth(color_depth)
        // `none` authentication must not submit a password or enter CredSSP.
        .with_credssp(use_credssp)
        .with_tls(!use_credssp)
        .with_codecs(Vec::new())
        .with_pointer_software_rendering(false)
        .with_dirty_region_updates(true)
        .with_certificate_validation(ironrdp_tls::CertificateValidation::Strict)
        .with_certificate_validation_callback(verifier)
        // Keep IronRDP's native clipboard backend disabled on every platform. When text
        // redirection is enabled, the custom static channel below is the sole CLIPRDR backend.
        .with_clipboard(ClipboardType::Disable);
    if clipboard_enabled {
        builder = builder.with_static_channel(move |_| Some(clipboard_factory.cliprdr_client()));
    }
    if let Some(relay) = &config.relay {
        relay.validate().map_err(anyhow::Error::msg)?;
        builder = builder.with_direct_tcp_proxy(
            relay.address,
            relay.token.expose_secret().as_bytes().to_vec(),
        );
    }
    let config = builder.build()?;
    Ok((config, clipboard))
}

async fn forward_output(
    session_id: String,
    mut receiver: tokio_mpsc::Receiver<RdpOutputEvent>,
    output_tx: mpsc::SyncSender<Outbound>,
    input_tx: RdpInputSender,
    certificate_gate: Arc<CertificateGate>,
    connection_timeout: Duration,
) {
    let mut epoch = 0u64;
    let mut connected = false;
    let mut cursor_shape = CursorShape {
        shape_id: 0,
        width: 0,
        height: 0,
        hotspot: RemotePoint { x: 0, y: 0 },
        pixels: Vec::new(),
    };
    let mut cursor_position = CursorPosition::default();
    let mut cursor_visible = true;
    let connection_deadline = tokio::time::sleep(connection_timeout);
    tokio::pin!(connection_deadline);
    loop {
        let event = if connected {
            receiver.recv().await
        } else {
            tokio::select! {
                event = receiver.recv() => event,
                () = &mut connection_deadline => {
                    if certificate_gate.waiting.load(Ordering::Acquire) {
                        connection_deadline
                            .as_mut()
                            .reset(tokio::time::Instant::now() + connection_timeout);
                        continue;
                    }
                    let _ = output_tx.send(Outbound::Control(RdpControlMessage::Error {
                        session_id: session_id.clone(),
                        error: RdpError::new(
                            RdpErrorKind::Timeout,
                            format!(
                                "RDP security negotiation did not complete within {} seconds",
                                connection_timeout.as_secs()
                            ),
                        ),
                        fatal: true,
                    }));
                    // The negotiation stalled, so nothing is draining the bounded input
                    // queue; cancel outright rather than queueing a shutdown request.
                    input_tx.request_close();
                    return;
                }
            }
        };
        let Some(event) = event else {
            return;
        };
        let terminal = matches!(
            &event,
            RdpOutputEvent::ConnectionFailure(_) | RdpOutputEvent::Terminated(_)
        );
        let result: Result<(), ()> = match event {
            RdpOutputEvent::DesktopReset { width, height } => {
                // Double-side validation: the application rejects an out-of-range
                // reset in `Framebuffer::new`, but the helper is the authority for
                // the bytes it puts on the wire, so it refuses here too rather than
                // shipping a reset the consumer would only tear down.
                if validate_framebuffer_dimensions(
                    u32::from(width),
                    u32::from(height),
                    RDP_FRAMEBUFFER_LIMITS,
                )
                .is_err()
                {
                    let _ = output_tx.send(Outbound::Control(RdpControlMessage::Error {
                        session_id: session_id.clone(),
                        error: RdpError::new(
                            RdpErrorKind::Protocol,
                            format!(
                                "RDP desktop reset {width}x{height} is outside the supported range"
                            ),
                        ),
                        fatal: true,
                    }));
                    input_tx.request_close();
                    return;
                }
                epoch = epoch.wrapping_add(1);
                let reset = output_tx.send(Outbound::Control(RdpControlMessage::DesktopReset {
                    session_id: session_id.clone(),
                    epoch,
                    width: u32::from(width),
                    height: u32::from(height),
                }));
                if !connected {
                    connected = true;
                    let _ = output_tx.send(Outbound::Control(RdpControlMessage::State {
                        session_id: session_id.clone(),
                        state: RdpSessionState::Connected,
                        message: None,
                    }));
                }
                reset.map_err(|_| ()).and_then(|()| {
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Shape(cursor_shape.clone()),
                    )?;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Position(cursor_position),
                    )?;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Visibility(CursorVisibility {
                            visible: cursor_visible,
                        }),
                    )
                })
            }
            RdpOutputEvent::ImageRegion {
                buffer,
                x,
                y,
                width,
                height,
                stride,
                full,
            } => {
                let pixels = rgba_to_bgra(buffer);
                let frame = RdpFrameEvent::Bitmap {
                    epoch,
                    full,
                    x: u32::from(x),
                    y: u32::from(y),
                    width: u32::from(width),
                    height: u32::from(height),
                    stride: u32::try_from(stride).unwrap_or(u32::MAX),
                    format: PixelFormat::Bgra8,
                    pixels,
                };
                encode_frame_packet_owned(&session_id, frame)
                    .map(Outbound::Packet)
                    .and_then(|packet| {
                        output_tx.send(packet).map_err(|_| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "RDP stdout writer stopped")
                        })
                    })
                    .map_err(|_| ())
            }
            // The session could not resize in place and IronRDP is about to reconnect
            // with the new size. Report the capability so the UI stops offering dynamic
            // resize; the reconnect surfaces through the ordinary state events. The
            // reason is not carried over the IPC protocol, which has no field for it.
            RdpOutputEvent::DisplayResizeFallback(_reason) => output_tx
                .send(Outbound::Control(RdpControlMessage::Capability {
                    session_id: session_id.clone(),
                    capability: RdpCapability::DynamicResizeUnavailable,
                }))
                .map_err(|_| ()),
            RdpOutputEvent::PointerDefault => (|| {
                if cursor_shape.width != 0 || cursor_shape.height != 0 {
                    cursor_shape.shape_id = cursor_shape.shape_id.wrapping_add(1);
                    cursor_shape.width = 0;
                    cursor_shape.height = 0;
                    cursor_shape.hotspot = RemotePoint { x: 0, y: 0 };
                    cursor_shape.pixels.clear();
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Shape(cursor_shape.clone()),
                    )?;
                }
                if !cursor_visible {
                    cursor_visible = true;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Visibility(CursorVisibility { visible: true }),
                    )?;
                }
                Ok(())
            })(),
            RdpOutputEvent::PointerHidden => (|| {
                if cursor_visible {
                    cursor_visible = false;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
                    )?;
                }
                Ok(())
            })(),
            RdpOutputEvent::PointerPosition { x, y } => (|| {
                let position = CursorPosition {
                    x: u32::from(x),
                    y: u32::from(y),
                };
                if cursor_position != position {
                    cursor_position = position;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Position(position),
                    )?;
                }
                Ok(())
            })(),
            RdpOutputEvent::PointerBitmap(pointer) => (|| {
                let pixels = rgba_to_bgra(pointer.bitmap_data.clone());
                let width = u32::from(pointer.width);
                let height = u32::from(pointer.height);
                let hotspot = RemotePoint {
                    x: u32::from(pointer.hotspot_x),
                    y: u32::from(pointer.hotspot_y),
                };
                if cursor_shape.width != width
                    || cursor_shape.height != height
                    || cursor_shape.hotspot != hotspot
                    || cursor_shape.pixels != pixels
                {
                    cursor_shape.shape_id = cursor_shape.shape_id.wrapping_add(1);
                    cursor_shape.width = width;
                    cursor_shape.height = height;
                    cursor_shape.hotspot = hotspot;
                    cursor_shape.pixels = pixels;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Shape(cursor_shape.clone()),
                    )?;
                }
                if !cursor_visible {
                    cursor_visible = true;
                    send_cursor(
                        &output_tx,
                        &session_id,
                        &RemoteCursorEvent::Visibility(CursorVisibility { visible: true }),
                    )?;
                }
                Ok(())
            })(),
            RdpOutputEvent::ConnectionFailure(error) => output_tx
                .send(Outbound::Control(RdpControlMessage::Error {
                    session_id: session_id.clone(),
                    error: classify_error(error.to_string(), RdpErrorKind::Transport),
                    fatal: true,
                }))
                .map_err(|_| ()),
            RdpOutputEvent::Terminated(Ok(reason)) if connected => output_tx
                .send(Outbound::Control(RdpControlMessage::Error {
                    session_id: session_id.clone(),
                    error: RdpError::new(
                        RdpErrorKind::Session,
                        format!("active RDP session terminated: {reason:?}"),
                    ),
                    fatal: true,
                }))
                .map_err(|_| ()),
            RdpOutputEvent::Terminated(Ok(reason)) => output_tx
                .send(Outbound::Control(RdpControlMessage::State {
                    session_id: session_id.clone(),
                    state: RdpSessionState::Disconnected,
                    message: Some(format!("{reason:?}")),
                }))
                .map_err(|_| ()),
            RdpOutputEvent::Terminated(Err(error)) => output_tx
                .send(Outbound::Control(RdpControlMessage::Error {
                    session_id: session_id.clone(),
                    error: classify_error(error.to_string(), RdpErrorKind::Session),
                    fatal: true,
                }))
                .map_err(|_| ()),
            // Auto-reconnect is not enabled for this client (no maximum attempts is
            // configured), so this should not arrive. The decision channel still has to
            // be answered, or the session waits on it.
            RdpOutputEvent::AutoReconnecting { response, .. } => {
                let _ = response.send(AutoReconnectDecision::Stop);
                Ok(())
            }
            // Everything else IronRDP reports is either informational or for a feature
            // NyaTerm does not drive: connection/logon milestones (the helper derives
            // its own state from DesktopReset and Terminated), monitor layout, RAIL and
            // RemoteApp, and the server-side redraw requests, whose effect arrives as
            // ordinary region updates.
            _ => Ok(()),
        };
        if result.is_err() {
            break;
        }
        if terminal {
            break;
        }
    }
}

/// Queues ordinary input on IronRDP's bounded input queue.
///
/// A full queue means the session is not draining input fast enough, which is not a
/// disconnect, so the complete event is dropped rather than failing the session. A
/// closed queue is fatal. The return value is true only when the event was queued.
fn send_iron_input(sender: &RdpInputSender, event: IronInput) -> anyhow::Result<bool> {
    match sender.try_send(event) {
        Ok(()) => Ok(true),
        Err(tokio_mpsc::error::TrySendError::Full(_)) => Ok(false),
        Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
            Err(anyhow::anyhow!("IronRDP input channel closed"))
        }
    }
}

/// Reliably queues control input without blocking the helper's async runtime thread.
///
/// IronRDP exposes only non-blocking bounded-queue operations, so yield between
/// capacity probes. A closed queue means the session can no longer receive the
/// control input and is therefore fatal.
async fn send_reliable_iron_input(
    sender: &RdpInputSender,
    mut event: IronInput,
) -> anyhow::Result<()> {
    loop {
        match sender.try_send(event) {
            Ok(()) => return Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(returned)) => {
                event = returned;
                tokio::task::yield_now().await;
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                return Err(anyhow::anyhow!("IronRDP input channel closed"));
            }
        }
    }
}

fn validate_input_events(events: &[RdpInputEvent]) -> anyhow::Result<()> {
    for event in events {
        if let RdpInputEvent::Unicode { text } = event {
            validate_committed_text(text)
                .map_err(|error| anyhow::anyhow!("invalid RDP committed text: {error}"))?;
        }
    }
    Ok(())
}

async fn convert_and_send_input(
    sender: &RdpInputSender,
    event: RdpInputEvent,
    state: &mut InputDatabase,
) -> anyhow::Result<()> {
    let reliable = !matches!(
        event,
        RdpInputEvent::Pointer(RemotePointerEvent::Move { .. })
    );
    let Some(batch) = convert_input(event, state) else {
        return Ok(());
    };
    let input = IronInput::FastPath(batch);
    if reliable {
        send_reliable_iron_input(sender, input).await?;
    } else {
        let _ = send_iron_input(sender, input)?;
    }
    Ok(())
}

fn secure_attention_input(state: &InputDatabase) -> SmallVec<[FastPathInputEvent; 2]> {
    const CTRL: u8 = 0x1d;
    const ALT: u8 = 0x38;
    const DELETE: u8 = 0x53;

    let ctrl_held = state.is_key_pressed(Scancode::from_u8(false, CTRL))
        || state.is_key_pressed(Scancode::from_u8(true, CTRL));
    let alt_held = state.is_key_pressed(Scancode::from_u8(false, ALT))
        || state.is_key_pressed(Scancode::from_u8(true, ALT));
    let mut events = SmallVec::new();
    if !ctrl_held {
        events.push(FastPathInputEvent::KeyboardEvent(
            KeyboardFlags::empty(),
            CTRL,
        ));
    }
    if !alt_held {
        events.push(FastPathInputEvent::KeyboardEvent(
            KeyboardFlags::empty(),
            ALT,
        ));
    }
    events.push(FastPathInputEvent::KeyboardEvent(
        KeyboardFlags::EXTENDED,
        DELETE,
    ));
    events.push(FastPathInputEvent::KeyboardEvent(
        KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE,
        DELETE,
    ));
    if !alt_held {
        events.push(FastPathInputEvent::KeyboardEvent(
            KeyboardFlags::RELEASE,
            ALT,
        ));
    }
    if !ctrl_held {
        events.push(FastPathInputEvent::KeyboardEvent(
            KeyboardFlags::RELEASE,
            CTRL,
        ));
    }
    events
}

fn send_cursor(
    output_tx: &mpsc::SyncSender<Outbound>,
    session_id: &str,
    cursor: &RemoteCursorEvent,
) -> Result<(), ()> {
    match encode_cursor_packet(session_id, cursor) {
        Ok(packet) => output_tx.send(Outbound::Packet(packet)).map_err(|_| ()),
        Err(_) => Ok(()),
    }
}

fn rgba_to_bgra(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    pixels
}

fn convert_input(
    event: RdpInputEvent,
    state: &mut InputDatabase,
) -> Option<SmallVec<[FastPathInputEvent; 2]>> {
    let operations = match event {
        RdpInputEvent::KeyDown {
            scan_code,
            extended,
            ..
        } => {
            let code = u8::try_from(scan_code).ok()?;
            vec![Operation::KeyPressed(Scancode::from_u8(extended, code))]
        }
        RdpInputEvent::KeyUp {
            scan_code,
            extended,
            ..
        } => {
            let code = u8::try_from(scan_code).ok()?;
            vec![Operation::KeyReleased(Scancode::from_u8(extended, code))]
        }
        RdpInputEvent::Unicode { text } => {
            let mut operations = Vec::with_capacity(text.chars().count() * 2);
            for character in text.chars() {
                operations.push(Operation::UnicodeKeyPressed(character));
                operations.push(Operation::UnicodeKeyReleased(character));
            }
            operations
        }
        RdpInputEvent::Pointer(pointer) => {
            let (position, operation) = match pointer {
                RemotePointerEvent::Move { position } => (position, None),
                RemotePointerEvent::Button {
                    position,
                    button,
                    pressed,
                } => {
                    let button = match button {
                        RemotePointerButton::Left => MouseButton::Left,
                        RemotePointerButton::Middle => MouseButton::Middle,
                        RemotePointerButton::Right => MouseButton::Right,
                        RemotePointerButton::X1 => MouseButton::X1,
                        RemotePointerButton::X2 => MouseButton::X2,
                    };
                    let operation = if pressed {
                        Operation::MouseButtonPressed(button)
                    } else {
                        Operation::MouseButtonReleased(button)
                    };
                    (position, Some(operation))
                }
                RemotePointerEvent::Wheel {
                    position,
                    axis,
                    rotation_units,
                } => (
                    position,
                    Some(Operation::WheelRotations(WheelRotations {
                        is_vertical: matches!(axis, RemoteWheelAxis::Vertical),
                        rotation_units,
                    })),
                ),
            };
            let mut operations = vec![Operation::MouseMove(MousePosition {
                x: u16::try_from(position.x).unwrap_or(u16::MAX),
                y: u16::try_from(position.y).unwrap_or(u16::MAX),
            })];
            operations.extend(operation);
            operations
        }
        RdpInputEvent::ReleaseAllInputs => return Some(state.release_all()),
    };
    let result = state.apply(operations);
    (!result.is_empty()).then_some(result)
}

fn classify_error(message: String, fallback: RdpErrorKind) -> RdpError {
    let lower = message.to_ascii_lowercase();
    let kind = if lower.contains("certificate rejected") {
        RdpErrorKind::CertificateRejected
    } else if lower.contains("authentication")
        || lower.contains("credssp")
        || lower.contains("logon")
    {
        RdpErrorKind::Authentication
    } else if lower.contains("timed out") || lower.contains("timeout") {
        RdpErrorKind::Timeout
    } else if lower.contains("refused") {
        RdpErrorKind::ConnectionRefused
    } else if lower.contains("negotiat") || lower.contains("nla") || lower.contains("x224") {
        RdpErrorKind::Negotiation
    } else if lower.contains("tls") || lower.contains("rustls") || lower.contains("handshake") {
        RdpErrorKind::Tls
    } else if lower.contains("clipboard") || lower.contains("cliprdr") {
        RdpErrorKind::Clipboard
    } else if lower.contains("transport")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
    {
        RdpErrorKind::Transport
    } else {
        fallback
    };
    RdpError::new(kind, message)
}

fn send_control(
    output_tx: &mpsc::SyncSender<Outbound>,
    message: RdpControlMessage,
) -> anyhow::Result<()> {
    output_tx
        .send(Outbound::Control(message))
        .map_err(|_| anyhow::anyhow!("RDP stdout writer stopped"))
}

fn send_error(
    output_tx: &mpsc::SyncSender<Outbound>,
    session_id: &str,
    kind: RdpErrorKind,
    message: &str,
    fatal: bool,
) -> anyhow::Result<()> {
    send_control(
        output_tx,
        RdpControlMessage::Error {
            session_id: session_id.to_string(),
            error: RdpError::new(kind, message),
            fatal,
        },
    )
}

fn report_ironrdp_panic(
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
    let detail = detail.replace(['\r', '\n'], " ");
    let detail = detail.chars().take(512).collect::<String>();
    let _ = output_tx.send(Outbound::Control(RdpControlMessage::Error {
        session_id: session_id.to_string(),
        error: RdpError::new(
            RdpErrorKind::HelperCrashed,
            format!("IronRDP runtime panicked: {detail}"),
        ),
        fatal: true,
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use ironrdp_client::rdp::{RdpInputEvent as IronInput, RdpInputSender, RdpOutputEvent};
    use ironrdp_input::{Database as InputDatabase, Operation, Scancode};
    use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
    use ironrdp_pdu::input::mouse::PointerFlags;
    use ironrdp_pdu::input::mouse_x::PointerXFlags;
    use nyaterm_remote_desktop::{
        RdpControlMessage, RdpErrorKind, RdpInputEvent, RemotePoint, RemotePointerButton,
        RemotePointerEvent, RemoteWheelAxis,
    };
    use tokio::sync::mpsc as tokio_mpsc;

    use super::{
        CertificateGate, Outbound, classify_error, convert_and_send_input, convert_input,
        forward_output, install_crypto_provider, report_ironrdp_panic, secure_attention_input,
        send_iron_input, send_reliable_iron_input, validate_active_session_id,
        validate_input_events,
    };

    #[test]
    fn active_session_id_rejects_foreign_messages() {
        validate_active_session_id(None, "before-connect")
            .expect("pre-connect handling keeps its existing semantics");
        validate_active_session_id(Some("session-a"), "session-a").expect("matching session id");
        let error = validate_active_session_id(Some("session-a"), "session-b")
            .expect_err("foreign session id must fail closed");
        assert!(error.to_string().contains("session id mismatch"));
    }

    #[test]
    fn rustls_crypto_provider_installation_is_idempotent() {
        install_crypto_provider().expect("first install");
        install_crypto_provider().expect("second install");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    #[test]
    fn helper_error_classification_preserves_retry_boundaries() {
        assert_eq!(
            classify_error("TLS handshake failed".to_string(), RdpErrorKind::Transport).kind,
            RdpErrorKind::Tls
        );
        assert_eq!(
            classify_error("connection reset".to_string(), RdpErrorKind::Session).kind,
            RdpErrorKind::Transport
        );
        assert_eq!(
            classify_error("CredSSP logon failed".to_string(), RdpErrorKind::Transport).kind,
            RdpErrorKind::Authentication
        );
        assert_eq!(
            classify_error(
                "NLA negotiation handshake failed".to_string(),
                RdpErrorKind::Transport,
            )
            .kind,
            RdpErrorKind::Negotiation
        );
        assert_eq!(
            classify_error("channel closed".to_string(), RdpErrorKind::Session).kind,
            RdpErrorKind::Session
        );
    }

    #[tokio::test]
    async fn stalled_security_negotiation_reports_a_fatal_timeout() {
        let (rdp_output_tx, rdp_output_rx) = tokio_mpsc::channel::<RdpOutputEvent>(1);
        let (input_tx, mut input_rx) = RdpInputSender::channel(1);
        let (output_tx, output_rx) = mpsc::sync_channel(1);

        forward_output(
            "session".to_string(),
            rdp_output_rx,
            output_tx,
            input_tx,
            Arc::new(CertificateGate::default()),
            Duration::from_millis(10),
        )
        .await;

        let Outbound::Control(RdpControlMessage::Error { error, fatal, .. }) =
            output_rx.recv().unwrap()
        else {
            panic!("expected a timeout error");
        };
        assert_eq!(error.kind, RdpErrorKind::Timeout);
        assert!(fatal);
        // The cancellation goes out through IronRDP's close signal rather than the
        // bounded input queue, precisely because nothing is draining that queue while
        // negotiation is stalled. `RdpInputSender::channel` does not hand back the close
        // receiver, so what is checked here is that no ordinary input was queued either.
        assert!(input_rx.try_recv().is_err());
        drop(rdp_output_tx);
    }

    #[test]
    fn ironrdp_panic_is_forwarded_without_multiline_output() {
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        report_ironrdp_panic(
            &output_tx,
            "session",
            Box::new("connector assertion\nfailed".to_string()),
        );

        let Outbound::Control(RdpControlMessage::Error { error, fatal, .. }) =
            output_rx.recv().unwrap()
        else {
            panic!("expected a helper crash error");
        };
        assert_eq!(error.kind, RdpErrorKind::HelperCrashed);
        assert_eq!(
            error.message,
            "IronRDP runtime panicked: connector assertion failed"
        );
        assert!(fatal);
    }

    #[test]
    fn committed_text_is_validated_before_utf16_press_release_conversion() {
        let events = [
            RdpInputEvent::Unicode {
                text: "valid".to_string(),
            },
            RdpInputEvent::Unicode {
                text: "invalid\ntext".to_string(),
            },
        ];
        assert!(validate_input_events(&events).is_err());

        let mut state = InputDatabase::default();
        let converted = convert_input(
            RdpInputEvent::Unicode {
                text: "A😀".to_string(),
            },
            &mut state,
        )
        .expect("non-empty committed text");
        let expected = [
            (KeyboardFlags::empty(), 0x0041),
            (KeyboardFlags::RELEASE, 0x0041),
            (KeyboardFlags::empty(), 0xd83d),
            (KeyboardFlags::empty(), 0xde00),
            (KeyboardFlags::RELEASE, 0xd83d),
            (KeyboardFlags::RELEASE, 0xde00),
        ];
        assert_eq!(converted.len(), expected.len());
        for (event, (expected_flags, expected_unit)) in converted.iter().zip(expected) {
            let FastPathInputEvent::UnicodeKeyboardEvent(flags, unit) = event else {
                panic!("expected Unicode keyboard event");
            };
            assert_eq!(*flags, expected_flags);
            assert_eq!(*unit, expected_unit);
        }
        assert!(!state.is_unicode_key_pressed('A'));
        assert!(!state.is_unicode_key_pressed('😀'));
    }

    fn assert_keyboard_batch(batch: &[FastPathInputEvent], expected: &[(KeyboardFlags, u8)]) {
        assert_eq!(batch.len(), expected.len());
        for (event, (expected_flags, expected_code)) in batch.iter().zip(expected) {
            let FastPathInputEvent::KeyboardEvent(flags, code) = event else {
                panic!("Secure Attention must contain only keyboard events");
            };
            assert_eq!(*flags, *expected_flags);
            assert_eq!(*code, *expected_code);
        }
    }

    #[test]
    fn secure_attention_is_one_ordered_fast_path_batch() {
        let state = InputDatabase::default();
        let batch = secure_attention_input(&state);
        assert_keyboard_batch(
            &batch,
            &[
                (KeyboardFlags::empty(), 0x1d),
                (KeyboardFlags::empty(), 0x38),
                (KeyboardFlags::EXTENDED, 0x53),
                (KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x53),
                (KeyboardFlags::RELEASE, 0x38),
                (KeyboardFlags::RELEASE, 0x1d),
            ],
        );
        assert!(!state.is_key_pressed(Scancode::from_u8(false, 0x1d)));
    }

    #[test]
    fn secure_attention_reuses_held_ctrl_and_preserves_state() {
        let mut state = InputDatabase::default();
        let _ = state.apply([Operation::KeyPressed(Scancode::from_u8(true, 0x1d))]);

        let batch = secure_attention_input(&state);

        assert_keyboard_batch(
            &batch,
            &[
                (KeyboardFlags::empty(), 0x38),
                (KeyboardFlags::EXTENDED, 0x53),
                (KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x53),
                (KeyboardFlags::RELEASE, 0x38),
            ],
        );
        assert!(state.is_key_pressed(Scancode::from_u8(true, 0x1d)));
    }

    #[test]
    fn secure_attention_reuses_held_alt_and_preserves_state() {
        let mut state = InputDatabase::default();
        let _ = state.apply([Operation::KeyPressed(Scancode::from_u8(true, 0x38))]);

        let batch = secure_attention_input(&state);

        assert_keyboard_batch(
            &batch,
            &[
                (KeyboardFlags::empty(), 0x1d),
                (KeyboardFlags::EXTENDED, 0x53),
                (KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x53),
                (KeyboardFlags::RELEASE, 0x1d),
            ],
        );
        assert!(state.is_key_pressed(Scancode::from_u8(true, 0x38)));
    }

    #[test]
    fn secure_attention_reuses_both_held_modifiers_and_preserves_state() {
        let mut state = InputDatabase::default();
        let _ = state.apply([
            Operation::KeyPressed(Scancode::from_u8(false, 0x1d)),
            Operation::KeyPressed(Scancode::from_u8(true, 0x38)),
        ]);

        let batch = secure_attention_input(&state);

        assert_keyboard_batch(
            &batch,
            &[
                (KeyboardFlags::EXTENDED, 0x53),
                (KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x53),
            ],
        );
        assert!(state.is_key_pressed(Scancode::from_u8(false, 0x1d)));
        assert!(state.is_key_pressed(Scancode::from_u8(true, 0x38)));
    }

    #[tokio::test]
    async fn secure_attention_waits_for_full_queue_and_eventually_enqueues() {
        let (sender, mut receiver) = RdpInputSender::channel(1);
        assert!(send_iron_input(&sender, IronInput::RequestFullFrame).unwrap());
        let batch = secure_attention_input(&InputDatabase::default());
        let enqueue_sender = sender.clone();
        let enqueue = tokio::spawn(async move {
            send_reliable_iron_input(&enqueue_sender, IronInput::FastPath(batch)).await
        });

        tokio::task::yield_now().await;
        assert!(!enqueue.is_finished());
        assert!(matches!(
            receiver.recv().await.unwrap(),
            IronInput::RequestFullFrame
        ));
        tokio::time::timeout(Duration::from_secs(1), enqueue)
            .await
            .expect("reliable enqueue timed out")
            .expect("reliable enqueue task panicked")
            .expect("reliable enqueue failed");

        let IronInput::FastPath(batch) = receiver.recv().await.unwrap() else {
            panic!("expected queued Secure Attention batch");
        };
        assert_keyboard_batch(
            &batch,
            &[
                (KeyboardFlags::empty(), 0x1d),
                (KeyboardFlags::empty(), 0x38),
                (KeyboardFlags::EXTENDED, 0x53),
                (KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x53),
                (KeyboardFlags::RELEASE, 0x38),
                (KeyboardFlags::RELEASE, 0x1d),
            ],
        );
    }

    #[tokio::test]
    async fn secure_attention_reliable_enqueue_fails_when_queue_is_closed() {
        let (sender, receiver) = RdpInputSender::channel(1);
        drop(receiver);

        let error = send_reliable_iron_input(
            &sender,
            IronInput::FastPath(secure_attention_input(&InputDatabase::default())),
        )
        .await
        .expect_err("closed queue must reject Secure Attention");

        assert_eq!(error.to_string(), "IronRDP input channel closed");
    }

    #[tokio::test]
    async fn reliable_key_release_waits_for_capacity_and_updates_state() {
        let (sender, mut receiver) = RdpInputSender::channel(1);
        assert!(send_iron_input(&sender, IronInput::RequestFullFrame).unwrap());

        let mut state = InputDatabase::default();
        let _ = state.apply([Operation::KeyPressed(Scancode::from_u8(false, 0x1d))]);
        {
            let release = convert_and_send_input(
                &sender,
                RdpInputEvent::KeyUp {
                    scan_code: 0x1d,
                    extended: false,
                    repeat: false,
                },
                &mut state,
            );
            tokio::pin!(release);
            tokio::select! {
                _ = &mut release => panic!("release must wait while the queue is full"),
                _ = tokio::task::yield_now() => {}
            }
            assert!(matches!(
                receiver.recv().await,
                Some(IronInput::RequestFullFrame)
            ));
            release.await.unwrap();
        }
        assert!(!state.is_key_pressed(Scancode::from_u8(false, 0x1d)));
        assert!(matches!(
            receiver.recv().await,
            Some(IronInput::FastPath(_))
        ));

        let (closed_sender, closed_receiver) = RdpInputSender::channel(1);
        drop(closed_receiver);
        let _ = state.apply([Operation::KeyPressed(Scancode::from_u8(false, 0x38))]);
        assert!(
            convert_and_send_input(
                &closed_sender,
                RdpInputEvent::KeyUp {
                    scan_code: 0x38,
                    extended: false,
                    repeat: false,
                },
                &mut state,
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn pointer_buttons_wheels_and_extended_buttons_are_distinct_from_move() {
        let position = RemotePoint { x: 40, y: 50 };
        let mut state = InputDatabase::default();
        let movement = convert_input(
            RdpInputEvent::Pointer(RemotePointerEvent::Move { position }),
            &mut state,
        )
        .unwrap();
        let [FastPathInputEvent::MouseEvent(movement)] = movement.as_slice() else {
            panic!("expected one pointer move");
        };
        assert_eq!(movement.flags, PointerFlags::MOVE);

        let button = convert_input(
            RdpInputEvent::Pointer(RemotePointerEvent::Button {
                position,
                button: RemotePointerButton::Left,
                pressed: true,
            }),
            &mut state,
        )
        .unwrap();
        let [FastPathInputEvent::MouseEvent(button)] = button.as_slice() else {
            panic!("expected one pointer button event");
        };
        assert!(
            button
                .flags
                .contains(PointerFlags::LEFT_BUTTON | PointerFlags::DOWN)
        );
        assert!(!button.flags.contains(PointerFlags::MOVE));

        let wheel = convert_input(
            RdpInputEvent::Pointer(RemotePointerEvent::Wheel {
                position,
                axis: RemoteWheelAxis::Horizontal,
                rotation_units: -120,
            }),
            &mut state,
        )
        .unwrap();
        let [FastPathInputEvent::MouseEvent(wheel)] = wheel.as_slice() else {
            panic!("expected one wheel event");
        };
        assert!(wheel.flags.contains(PointerFlags::HORIZONTAL_WHEEL));
        assert!(!wheel.flags.contains(PointerFlags::MOVE));
        assert_eq!(wheel.number_of_wheel_rotation_units, -120);

        let extended = convert_input(
            RdpInputEvent::Pointer(RemotePointerEvent::Button {
                position,
                button: RemotePointerButton::X1,
                pressed: true,
            }),
            &mut state,
        )
        .unwrap();
        let [FastPathInputEvent::MouseEventEx(extended)] = extended.as_slice() else {
            panic!("expected one extended pointer event");
        };
        assert!(
            extended
                .flags
                .contains(PointerXFlags::BUTTON1 | PointerXFlags::DOWN)
        );
    }
}

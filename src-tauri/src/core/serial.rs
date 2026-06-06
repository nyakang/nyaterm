//! Serial port session: opens a serial device and bridges I/O to the session manager.

use super::session::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
use super::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemEvent, ZmodemTransfer,
};
use crate::config::AiExecutionProfile;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::{RecordingManager, SessionOutputCoalescer};
use crate::error::{AppError, AppResult};
use crate::observability::{log_event, log_rate_limited, StructuredLog, StructuredLogLevel};
use serialport::{DataBits, FlowControl, Parity, StopBits};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub name: String,
    pub backspace_mode: String,
}

/// Replace DEL (0x7F) with BS (0x08) in-place.
fn remap_del_to_bs(data: &mut [u8]) {
    for byte in data.iter_mut() {
        if *byte == 0x7f {
            *byte = 0x08;
        }
    }
}

pub fn list_serial_ports() -> AppResult<Vec<String>> {
    let mut port_names = serialport::available_ports()
        .map_err(|e| AppError::Config(format!("Failed to list serial ports: {e}")))?
        .into_iter()
        .map(|port| port.port_name)
        .collect::<Vec<_>>();
    port_names.sort_unstable();
    Ok(port_names)
}

fn parse_data_bits(v: u8) -> DataBits {
    match v {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    }
}

fn parse_parity(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}

fn parse_stop_bits(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}

pub async fn create_serial_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: SerialConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
) -> AppResult<String> {
    log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "session.lifecycle".to_string(),
        event: "session.create_start".to_string(),
        message: "Creating serial session".to_string(),
        ids: connection_id
            .as_ref()
            .map(|value| serde_json::json!({ "connection_id": value })),
        data: Some(serde_json::json!({
            "session_type": "Serial",
            "port_name": config.port_name.clone(),
            "baud_rate": config.baud_rate,
        })),
        error: None,
        client_timestamp: None,
    });

    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::Serial,
        connected: true,
        owner_window_label,
        ai_execution_profile: AiExecutionProfile::SendOnly,
        injection_active: false,
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(None));
    let session_handle = SessionHandle {
        info: session_info,
        cmd_tx,
        ssh_config: None,
        ssh_handle: None,
        cwd,
        remote_fs: None,
    };
    manager.add_session(session_handle).await;

    let sid = session_id.clone();
    let mgr = manager.clone();
    let rt_handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        serial_session_thread(app, sid, mgr, cmd_rx, rt_handle, config, connection_id);
    });

    Ok(session_id)
}

fn serial_session_thread(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    rt_handle: tokio::runtime::Handle,
    config: SerialConfig,
    connection_id: Option<String>,
) {
    let port = match serialport::new(&config.port_name, config.baud_rate)
        .data_bits(parse_data_bits(config.data_bits))
        .parity(parse_parity(&config.parity))
        .stop_bits(parse_stop_bits(&config.stop_bits))
        .flow_control(FlowControl::None)
        .timeout(Duration::from_millis(100))
        .open()
    {
        Ok(p) => p,
        Err(e) => {
            log_event(StructuredLog {
                level: StructuredLogLevel::Error,
                domain: "session.lifecycle".to_string(),
                event: "session.connection_failed".to_string(),
                message: "Failed to open serial port".to_string(),
                ids: Some(serde_json::json!({
                    "session_id": session_id.clone(),
                    "connection_id": connection_id.clone(),
                })),
                data: Some(serde_json::json!({
                    "session_type": "Serial",
                    "port_name": config.port_name.clone(),
                    "baud_rate": config.baud_rate,
                })),
                error: Some(serde_json::json!({ "message": e.to_string() })),
                client_timestamp: None,
            });
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to open serial port: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async { manager.remove_session(&session_id).await });
            return;
        }
    };

    let backspace_as_bs = config.backspace_mode == "ctrl_h";
    let mut reader_port = match port.try_clone() {
        Ok(port) => port,
        Err(e) => {
            log_event(StructuredLog {
                level: StructuredLogLevel::Error,
                domain: "session.lifecycle".to_string(),
                event: "session.connection_failed".to_string(),
                message: "Failed to clone serial port for reading".to_string(),
                ids: Some(serde_json::json!({
                    "session_id": session_id.clone(),
                    "connection_id": connection_id.clone(),
                })),
                data: Some(serde_json::json!({
                    "session_type": "Serial",
                    "port_name": config.port_name.clone(),
                    "baud_rate": config.baud_rate,
                })),
                error: Some(serde_json::json!({ "message": e.to_string() })),
                client_timestamp: None,
            });
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to clone serial port: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async { manager.remove_session(&session_id).await });
            return;
        }
    };

    let port_writer = Arc::new(Mutex::new(port));
    let output_event = format!("terminal-output-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);
    let output = SessionOutputCoalescer::for_app(app.clone(), output_event.clone());
    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|state| state.inner().clone());

    let capture_processor = Arc::new(Mutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();

    let zmodem_state: Arc<Mutex<Option<ZmodemTransfer>>> = Arc::new(Mutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let port_writer_reader = port_writer.clone();
    let output_reader = output.clone();
    let recording_mgr_reader = recording_mgr.clone();

    let reader_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let reader_flag = reader_running.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut zmodem_detector = ZmodemDetector::new();
        while reader_flag.load(std::sync::atomic::Ordering::Relaxed) {
            let result = reader_port.read(&mut buf);
            match result {
                Ok(0) => break,
                Ok(n) => {
                    let raw = &buf[..n];

                    // ZMODEM: if active, route to transfer.
                    {
                        let mut zm = zmodem_state_reader.lock().unwrap();
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.feed_incoming(raw);
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => {
                                        let mut p = port_writer_reader.lock().unwrap();
                                        let _ = p.write_all(&data);
                                        let _ = p.flush();
                                    }
                                    ZmodemAction::EmitEvent(event) => {
                                        let _ = app_reader.emit(&zmodem_event_reader, &event);
                                    }
                                }
                            }
                            if transfer.is_done() {
                                *zm = None;
                                zmodem_detector.reset();
                            }
                            continue;
                        }
                    }

                    // ZMODEM: detect header.
                    let process_raw = match zmodem_detector.feed(raw) {
                        ZmodemDetectResult::Detected {
                            direction,
                            passthrough,
                            initial_bytes,
                        } => {
                            if !passthrough.is_empty() {
                                let pre = String::from_utf8_lossy(&passthrough).to_string();
                                if !pre.is_empty() {
                                    if let Some(ref recorder) = recording_mgr_reader {
                                        recorder.write_output(&sid_reader, &pre);
                                    }
                                    output_reader.push_owned(pre);
                                }
                            }
                            let transfer = ZmodemTransfer::new(direction, &initial_bytes);
                            *zmodem_state_reader.lock().unwrap() = Some(transfer);
                            let _ = app_reader
                                .emit(&zmodem_event_reader, &ZmodemEvent::Detected { direction });
                            continue;
                        }
                        ZmodemDetectResult::NoMatch { passthrough } => {
                            if passthrough.is_empty() {
                                continue;
                            }
                            passthrough
                        }
                    };

                    let mut text = String::from_utf8_lossy(&process_raw).to_string();
                    if let Ok(mut proc) = capture_for_reader.lock() {
                        if proc.has_active() {
                            text = proc.process(&text);
                        }
                    }
                    if !text.is_empty() {
                        if let Some(ref recorder) = recording_mgr_reader {
                            recorder.write_output(&sid_reader, &text);
                        }
                        output_reader.push_owned(text);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => {
                    log_rate_limited(StructuredLog {
                        level: StructuredLogLevel::Warn,
                        domain: "session.lifecycle".to_string(),
                        event: "serial.read_error".to_string(),
                        message: "Serial read error".to_string(),
                        ids: Some(serde_json::json!({
                            "session_id": sid_reader.clone(),
                            "connection_id": connection_id.clone(),
                        })),
                        data: Some(serde_json::json!({
                            "session_type": "Serial",
                            "port_name": config.port_name.clone(),
                        })),
                        error: Some(serde_json::json!({ "message": e.to_string() })),
                        client_timestamp: None,
                    });
                    break;
                }
            }
        }
        output_reader.close();
        let _ = app_reader.emit(&format!("session-closed-{}", sid_reader), ());
    });

    // Command loop
    loop {
        let cmd = match cmd_rx.blocking_recv() {
            Some(c) => c,
            None => break,
        };
        match cmd {
            SessionCommand::Attach => {
                output.attach();
            }
            SessionCommand::Write(mut data) => {
                if zmodem_state.lock().unwrap().is_some() {
                    continue;
                }
                if backspace_as_bs {
                    remap_del_to_bs(&mut data);
                }
                if let Some(ref recorder) = recording_mgr {
                    recorder.write_input(&session_id, &data);
                }
                let mut p = port_writer.lock().unwrap();
                let _ = p.write_all(&data);
                let _ = p.flush();
            }
            SessionCommand::CaptureExec {
                marker_id,
                wrapped_command,
                result_tx,
            } => {
                if let Ok(mut proc) = capture_processor.lock() {
                    proc.register(marker_id, result_tx);
                }
                let mut p = port_writer.lock().unwrap();
                let _ = p.write_all(&wrapped_command);
                let _ = p.flush();
            }
            SessionCommand::Resize { .. } => {}
            SessionCommand::ZmodemAcceptDownload { save_dir } => {
                let mut zm = zmodem_state.lock().unwrap();
                if let Some(ref mut transfer) = *zm {
                    let actions = transfer.accept_download(save_dir);
                    for action in actions {
                        match action {
                            ZmodemAction::SendToRemote(data) => {
                                let mut p = port_writer.lock().unwrap();
                                let _ = p.write_all(&data);
                                let _ = p.flush();
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                    if transfer.is_done() {
                        *zm = None;
                    }
                }
            }
            SessionCommand::ZmodemAcceptUpload { files } => {
                let mut zm = zmodem_state.lock().unwrap();
                if let Some(ref mut transfer) = *zm {
                    let actions = transfer.accept_upload(files);
                    for action in actions {
                        match action {
                            ZmodemAction::SendToRemote(data) => {
                                let mut p = port_writer.lock().unwrap();
                                let _ = p.write_all(&data);
                                let _ = p.flush();
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                    if transfer.is_done() {
                        *zm = None;
                    }
                }
            }
            SessionCommand::ZmodemCancel => {
                let mut zm = zmodem_state.lock().unwrap();
                if let Some(ref mut transfer) = *zm {
                    let actions = transfer.cancel();
                    for action in actions {
                        match action {
                            ZmodemAction::SendToRemote(data) => {
                                let mut p = port_writer.lock().unwrap();
                                let _ = p.write_all(&data);
                                let _ = p.flush();
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                }
                *zm = None;
            }
            SessionCommand::Close => {
                break;
            }
        }
    }

    reader_running.store(false, std::sync::atomic::Ordering::Relaxed);
    output.close();

    if let Some(ref recorder) = recording_mgr {
        recorder.cleanup_session(&session_id);
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&closed_event, ());
}

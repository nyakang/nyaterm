//! Telnet session: raw TCP with basic IAC negotiation, bridged to the session manager.

use super::session::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
use super::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemEvent, ZmodemTransfer,
};
use crate::config::AiExecutionProfile;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::{RecordingManager, SessionOutputCoalescer};
use crate::error::AppResult;
use crate::observability::{log_event, log_rate_limited, StructuredLog, StructuredLogLevel};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex as TokioMutex};

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
const OPT_NAWS: u8 = 31;

/// Respond to a Telnet option negotiation request.
fn negotiate_response(command: u8, option: u8) -> Vec<u8> {
    match command {
        WILL => {
            if option == OPT_ECHO || option == OPT_SUPPRESS_GO_AHEAD {
                vec![IAC, DO, option]
            } else {
                vec![IAC, DONT, option]
            }
        }
        DO => {
            if option == OPT_NAWS {
                vec![IAC, WILL, option]
            } else {
                vec![IAC, WONT, option]
            }
        }
        WONT => vec![IAC, DONT, option],
        DONT => vec![IAC, WONT, option],
        _ => vec![],
    }
}

/// Build a NAWS (Negotiate About Window Size) subnegotiation sequence.
fn build_naws(cols: u16, rows: u16) -> Vec<u8> {
    vec![
        IAC,
        SB,
        OPT_NAWS,
        (cols >> 8) as u8,
        (cols & 0xff) as u8,
        (rows >> 8) as u8,
        (rows & 0xff) as u8,
        IAC,
        SE,
    ]
}

/// Strip IAC sequences from raw data, returning only user-visible bytes.
/// Calls `on_negotiate` for each IAC command/option pair encountered.
fn strip_telnet_commands(data: &[u8], on_negotiate: &mut impl FnMut(u8, u8)) -> Vec<u8> {
    let mut visible = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == IAC && i + 1 < data.len() {
            let cmd = data[i + 1];
            match cmd {
                IAC => {
                    visible.push(IAC);
                    i += 2;
                }
                WILL | WONT | DO | DONT => {
                    if i + 2 < data.len() {
                        on_negotiate(cmd, data[i + 2]);
                        i += 3;
                    } else {
                        i += 2;
                    }
                }
                SB => {
                    // Skip subnegotiation until IAC SE
                    i += 2;
                    while i < data.len() {
                        if data[i] == IAC && i + 1 < data.len() && data[i + 1] == SE {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    i += 2;
                }
            }
        } else {
            visible.push(data[i]);
            i += 1;
        }
    }
    visible
}

/// Replace DEL (0x7F) with BS (0x08) in-place.
fn remap_del_to_bs(data: &mut [u8]) {
    for byte in data.iter_mut() {
        if *byte == 0x7f {
            *byte = 0x08;
        }
    }
}

pub async fn create_telnet_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    host: String,
    port: u16,
    connection_id: Option<String>,
    name: String,
    backspace_mode: String,
    owner_window_label: Option<String>,
) -> AppResult<String> {
    log_event(StructuredLog {
        level: StructuredLogLevel::Info,
        domain: "session.lifecycle".to_string(),
        event: "session.create_start".to_string(),
        message: "Creating Telnet session".to_string(),
        ids: connection_id
            .as_ref()
            .map(|value| serde_json::json!({ "connection_id": value })),
        data: Some(serde_json::json!({
            "session_type": "Telnet",
            "host": host,
            "port": port,
        })),
        error: None,
        client_timestamp: None,
    });
    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name,
        session_type: SessionType::Telnet,
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

    tokio::spawn(async move {
        telnet_session_task(
            app,
            sid,
            mgr,
            cmd_rx,
            host,
            port,
            connection_id,
            backspace_mode,
        )
        .await;
    });

    Ok(session_id)
}

async fn telnet_session_task(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    host: String,
    port: u16,
    connection_id: Option<String>,
    backspace_mode: String,
) {
    let backspace_as_bs = backspace_mode == "ctrl_h";
    let addr = format!("{}:{}", host, port);
    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            log_event(StructuredLog {
                level: StructuredLogLevel::Error,
                domain: "session.lifecycle".to_string(),
                event: "session.connection_failed".to_string(),
                message: "Telnet connection failed".to_string(),
                ids: Some(serde_json::json!({
                    "session_id": session_id.clone(),
                    "connection_id": connection_id.clone(),
                })),
                data: Some(serde_json::json!({
                    "session_type": "Telnet",
                    "host": host,
                    "port": port,
                })),
                error: Some(serde_json::json!({ "message": e.to_string() })),
                client_timestamp: None,
            });
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Connection failed: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            manager.remove_session(&session_id).await;
            return;
        }
    };

    let (mut reader, mut writer) = stream.into_split();
    let output_event = format!("terminal-output-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);
    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|state| state.inner().clone());
    let output = SessionOutputCoalescer::for_app(app.clone(), output_event.clone());

    let capture_processor = Arc::new(TokioMutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();

    let zmodem_state: Arc<TokioMutex<Option<ZmodemTransfer>>> = Arc::new(TokioMutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();
    let (zmodem_out_tx, mut zmodem_out_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let output_reader = output.clone();
    let reader_connection_id = connection_id.clone();
    let recording_mgr_reader = recording_mgr.clone();

    let (negotiate_tx, mut negotiate_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let reader_handle = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut zmodem_detector = ZmodemDetector::new();
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let neg_tx = negotiate_tx.clone();
                    let visible = strip_telnet_commands(&buf[..n], &mut |cmd, opt| {
                        let resp = negotiate_response(cmd, opt);
                        if !resp.is_empty() {
                            let _ = neg_tx.send(resp);
                        }
                    });
                    if visible.is_empty() {
                        continue;
                    }

                    // ZMODEM: if active, route to transfer.
                    {
                        let mut zm = zmodem_state_reader.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.feed_incoming(&visible);
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => {
                                        let _ = zmodem_out_tx.send(data);
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
                    let process_visible = match zmodem_detector.feed(&visible) {
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
                            *zmodem_state_reader.lock().await = Some(transfer);
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

                    let mut text = String::from_utf8_lossy(&process_visible).to_string();
                    let mut proc = capture_for_reader.lock().await;
                    if proc.has_active() {
                        text = proc.process(&text);
                    }
                    drop(proc);
                    if !text.is_empty() {
                        if let Some(ref recorder) = recording_mgr_reader {
                            recorder.write_output(&sid_reader, &text);
                        }
                        output_reader.push_owned(text);
                    }
                }
                Err(e) => {
                    log_rate_limited(StructuredLog {
                        level: StructuredLogLevel::Warn,
                        domain: "session.lifecycle".to_string(),
                        event: "telnet.read_error".to_string(),
                        message: "Telnet read error".to_string(),
                        ids: Some(serde_json::json!({
                            "session_id": sid_reader.clone(),
                            "connection_id": reader_connection_id.clone(),
                        })),
                        data: Some(serde_json::json!({
                            "session_type": "Telnet",
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

    loop {
        tokio::select! {
            Some(neg_data) = negotiate_rx.recv() => {
                if let Err(e) = writer.write_all(&neg_data).await {
                    log_rate_limited(StructuredLog {
                        level: StructuredLogLevel::Warn,
                        domain: "session.lifecycle".to_string(),
                        event: "telnet.negotiate_write_error".to_string(),
                        message: "Telnet negotiate write error".to_string(),
                        ids: Some(serde_json::json!({
                            "session_id": session_id.clone(),
                            "connection_id": connection_id.clone(),
                        })),
                        data: Some(serde_json::json!({
                            "session_type": "Telnet",
                        })),
                        error: Some(serde_json::json!({ "message": e.to_string() })),
                        client_timestamp: None,
                    });
                    break;
                }
            }
            Some(zdata) = zmodem_out_rx.recv() => {
                let _ = writer.write_all(&zdata).await;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::Attach) => {
                        output.attach();
                    }
                    Some(SessionCommand::Write(mut data)) => {
                        if zmodem_state.lock().await.is_some() {
                            continue;
                        }
                        if backspace_as_bs {
                            remap_del_to_bs(&mut data);
                        }
                        if let Some(ref recorder) = recording_mgr {
                            recorder.write_input(&session_id, &data);
                        }
                        if let Err(e) = writer.write_all(&data).await {
                            log_rate_limited(StructuredLog {
                                level: StructuredLogLevel::Warn,
                                domain: "session.lifecycle".to_string(),
                                event: "telnet.write_error".to_string(),
                                message: "Telnet write error".to_string(),
                                ids: Some(serde_json::json!({
                                    "session_id": session_id.clone(),
                                    "connection_id": connection_id.clone(),
                                })),
                                data: Some(serde_json::json!({
                                    "session_type": "Telnet",
                                })),
                                error: Some(serde_json::json!({ "message": e.to_string() })),
                                client_timestamp: None,
                            });
                            break;
                        }
                    }
                    Some(SessionCommand::CaptureExec { marker_id, wrapped_command, result_tx }) => {
                        capture_processor.lock().await.register(marker_id, result_tx);
                        if let Err(e) = writer.write_all(&wrapped_command).await {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %e,
                                "Failed to write capture command to Telnet"
                            );
                        }
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        let naws = build_naws(cols as u16, rows as u16);
                        let _ = writer.write_all(&naws).await;
                    }
                    Some(SessionCommand::ZmodemAcceptDownload { save_dir }) => {
                        let mut zm = zmodem_state.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.accept_download(save_dir);
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => { let _ = writer.write_all(&data).await; }
                                    ZmodemAction::EmitEvent(event) => { let _ = app.emit(&zmodem_event_name, &event); }
                                }
                            }
                            if transfer.is_done() { *zm = None; }
                        }
                    }
                    Some(SessionCommand::ZmodemAcceptUpload { files }) => {
                        let mut zm = zmodem_state.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.accept_upload(files);
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => { let _ = writer.write_all(&data).await; }
                                    ZmodemAction::EmitEvent(event) => { let _ = app.emit(&zmodem_event_name, &event); }
                                }
                            }
                            if transfer.is_done() { *zm = None; }
                        }
                    }
                    Some(SessionCommand::ZmodemCancel) => {
                        let mut zm = zmodem_state.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.cancel();
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => { let _ = writer.write_all(&data).await; }
                                    ZmodemAction::EmitEvent(event) => { let _ = app.emit(&zmodem_event_name, &event); }
                                }
                            }
                        }
                        *zm = None;
                    }
                    Some(SessionCommand::Close) | None => {
                        break;
                    }
                }
            }
        }
    }

    output.close();
    reader_handle.abort();
    if let Some(ref recorder) = recording_mgr {
        recorder.cleanup_session(&session_id);
    }
    manager.remove_session(&session_id).await;
    let _ = app.emit(&closed_event, ());
}

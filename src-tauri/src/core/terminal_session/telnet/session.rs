struct PendingTelnetStartupCommand {
    input: Vec<u8>,
    delay_ms: u64,
}

fn build_telnet_startup_command_input(
    command: &str,
    enter_mode: TelnetEnterMode,
) -> Option<Vec<u8>> {
    if command.trim().is_empty() {
        return None;
    }

    let normalized = command.replace("\r\n", "\r").replace('\n', "\r");
    let mut input = normalized.into_bytes();
    if !input.ends_with(b"\r") {
        input.push(b'\r');
    }
    Some(normalize_enter_bytes(&input, enter_mode))
}

fn arm_telnet_startup_timer(
    pending_startup: &Option<PendingTelnetStartupCommand>,
    startup_deadline: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
) {
    if startup_deadline.is_none() {
        if let Some(pending) = pending_startup.as_ref() {
            *startup_deadline = Some(Box::pin(tokio::time::sleep(
                std::time::Duration::from_millis(pending.delay_ms),
            )));
        }
    }
}

async fn telnet_session_task(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: SessionCommandReceiver,
    output_control_tx: SessionCommandSender,
    config: TelnetSessionConfig,
    connection_id: Option<String>,
    encoding: String,
    startup_command: Option<TelnetStartupCommand>,
) {
    let backspace_as_bs = config.backspace_mode == "ctrl_h";
    let host = config.host.clone();
    let port = config.port;
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
    let output =
        SessionOutputCoalescer::for_app(app.clone(), output_event.clone(), output_control_tx);

    let capture_processor = Arc::new(TokioMutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();
    let auto_login = Arc::new(TokioMutex::new(TelnetAutoLogin::new(
        config.auto_login.clone(),
        TelnetAutoLoginCredentials {
            username: config.username.clone(),
            password: config.password.clone(),
        },
        config.enter_mode,
        std::time::Instant::now(),
    )));
    let auto_login_for_reader = auto_login.clone();

    let zmodem_state: Arc<TokioMutex<Option<ZmodemTransfer>>> = Arc::new(TokioMutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_upload_drain = Arc::new(TokioMutex::new(ZmodemUploadDrain::new()));
    let zmodem_upload_drain_reader = zmodem_upload_drain.clone();
    let zmodem_download_oo_drain = Arc::new(TokioMutex::new(ZmodemDownloadOoDrain::new()));
    let zmodem_download_oo_drain_reader = zmodem_download_oo_drain.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();
    let (zmodem_out_tx, mut zmodem_out_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let manager_reader = manager.clone();
    let output_reader = output.clone();
    let reader_connection_id = connection_id.clone();
    let recording_mgr_reader = recording_mgr.clone();
    let (pause_tx, mut pause_rx) = tokio::sync::watch::channel(false);

    let (negotiate_tx, mut negotiate_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (auto_login_tx, mut auto_login_rx) = mpsc::unbounded_channel::<TelnetAutoLoginAction>();
    let (reader_done_tx, mut reader_done_rx) = mpsc::unbounded_channel::<()>();

    let reader_config = config.clone();
    let encoding_reader = encoding.clone();
    let reader_handle = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut zmodem_detector = ZmodemDetector::new();
        let mut output_decoder = TerminalOutputDecoder::new(&encoding_reader);
        'reader: loop {
            while *pause_rx.borrow() {
                if pause_rx.changed().await.is_err() {
                    break 'reader;
                }
            }
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let visible = if reader_config.raw_tcp_cli {
                        unescape_iac_iac(&buf[..n])
                    } else {
                        let neg_tx = negotiate_tx.clone();
                        strip_telnet_commands(&buf[..n], &mut |cmd, opt| {
                            let resp = negotiate_response(
                                cmd,
                                opt,
                                reader_config.send_naws,
                                reader_config.send_sga,
                            );
                            if !resp.is_empty() {
                                let _ = neg_tx.send(resp);
                            }
                        })
                    };
                    if visible.is_empty() {
                        continue;
                    }

                    let visible = if zmodem_upload_drain_reader.lock().await.is_active() {
                        let mut drain = zmodem_upload_drain_reader.lock().await;
                        drain.filter(&visible, std::time::Instant::now()).to_vec()
                    } else {
                        visible
                    };
                    if visible.is_empty() {
                        continue;
                    }

                    // ZMODEM: if active, route to transfer.
                    {
                        let mut zm = zmodem_state_reader.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let direction = transfer.direction();
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
                                if direction == ZmodemDirection::Upload {
                                    zmodem_upload_drain_reader
                                        .lock()
                                        .await
                                        .start(std::time::Instant::now());
                                } else if direction == ZmodemDirection::Download {
                                    zmodem_download_oo_drain_reader
                                        .lock()
                                        .await
                                        .start(std::time::Instant::now());
                                }
                            }
                            continue;
                        }
                    }

                    let visible = if zmodem_download_oo_drain_reader.lock().await.is_active() {
                        let mut drain = zmodem_download_oo_drain_reader.lock().await;
                        drain
                            .filter(&visible, std::time::Instant::now())
                            .to_vec()
                    } else {
                        visible
                    };
                    if visible.is_empty() {
                        continue;
                    }

                    // ZMODEM: detect header.
                    let process_visible = match zmodem_detector.feed(&visible) {
                        ZmodemDetectResult::Detected {
                            direction,
                            passthrough,
                            initial_bytes,
                        } => {
                            if !passthrough.is_empty() {
                                if let Some(ref recorder) = recording_mgr_reader {
                                    recorder.write_raw_output(&sid_reader, &passthrough);
                                }
                                let pre = output_decoder.decode(&passthrough);
                                if !pre.is_empty() {
                                    if let Some(ref recorder) = recording_mgr_reader {
                                        recorder.write_output(&sid_reader, &pre);
                                    }
                                    output_reader.push_owned(pre);
                                }
                            }
                            let prepared_upload = if direction == ZmodemDirection::Upload {
                                manager_reader.take_pending_zmodem_upload(&sid_reader).await
                            } else {
                                None
                            };
                            let (transfer, bootstrap_actions) =
                                start_zmodem_transfer(direction, &initial_bytes, prepared_upload);
                            for action in bootstrap_actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => {
                                        let _ = zmodem_out_tx.send(data);
                                    }
                                    ZmodemAction::EmitEvent(event) => {
                                        let _ = app_reader.emit(&zmodem_event_reader, &event);
                                    }
                                }
                            }
                            *zmodem_state_reader.lock().await = Some(transfer);
                            let _ = app_reader
                                .emit(&zmodem_event_reader, &ZmodemEvent::Detected { direction });
                            continue;
                        }
                        ZmodemDetectResult::NoMatch { passthrough } => {
                            if passthrough.is_empty() {
                                continue;
                            }
                            if let Some(ref recorder) = recording_mgr_reader {
                                recorder.write_raw_output(&sid_reader, &passthrough);
                            }
                            passthrough
                        }
                    };

                    let mut text = output_decoder.decode(&process_visible);
                    let mut proc = capture_for_reader.lock().await;
                    if proc.has_active() {
                        text = proc.process(&text);
                    }
                    drop(proc);
                    if !text.is_empty() {
                        {
                            let mut auto = auto_login_for_reader.lock().await;
                            if let Some(auto) = auto.as_mut() {
                                for action in
                                    auto.handle_text(&text, std::time::Instant::now())
                                {
                                    let _ = auto_login_tx.send(action);
                                }
                            }
                        }
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
        let _ = reader_done_tx.send(());
    });

    let line_edit_active = config.raw_tcp_cli && config.local_line_edit;
    let mut line_editor = TelnetLineEditor::default();
    let mut pending_startup = startup_command.and_then(|command| {
        build_telnet_startup_command_input(&command.command, config.enter_mode).map(|input| {
            PendingTelnetStartupCommand {
                input,
                delay_ms: command.delay_ms,
            }
        })
    });
    let mut startup_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    if auto_login.lock().await.is_none() {
        arm_telnet_startup_timer(&pending_startup, &mut startup_deadline);
    }

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
            Some(action) = auto_login_rx.recv() => {
                match action {
                    TelnetAutoLoginAction::Send(data) => {
                        let send_data = encode_terminal_input(&data, &encoding);
                        let _ = writer.write_all(&send_data).await;
                    }
                    TelnetAutoLoginAction::Complete => {
                        arm_telnet_startup_timer(&pending_startup, &mut startup_deadline);
                    }
                    TelnetAutoLoginAction::Disable => {
                        pending_startup = None;
                        startup_deadline = None;
                    }
                }
            }
            _ = async {
                if let Some(deadline) = startup_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if startup_deadline.is_some() => {
                startup_deadline = None;
                if let Some(pending) = pending_startup.take() {
                    let send_input = encode_terminal_input(&pending.input, &encoding);
                    if let Err(e) = writer.write_all(&send_input).await {
                        log_rate_limited(StructuredLog {
                            level: StructuredLogLevel::Warn,
                            domain: "session.lifecycle".to_string(),
                            event: "telnet.startup_command_write_error".to_string(),
                            message: "Telnet startup command write error".to_string(),
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
            }
            reader_done = reader_done_rx.recv() => {
                let _ = reader_done;
                break;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::AttachConfirmed { ack }) => {
                        output.attach_confirmed(ack);
                    }
                    Some(SessionCommand::DetachRenderer) => {
                        output.detach();
                    }
                    Some(SessionCommand::Write { mut data, automated, .. }) => {
                        if !automated {
                            let mut auto = auto_login.lock().await;
                            if let Some(auto) = auto.as_mut() {
                                if let Some(TelnetAutoLoginAction::Disable) =
                                    auto.handle_user_input(false)
                                {
                                    pending_startup = None;
                                    startup_deadline = None;
                                }
                            }
                        }

                        if zmodem_state.lock().await.is_some()
                            || zmodem_upload_drain
                                .lock()
                                .await
                                .should_suppress(std::time::Instant::now())
                        {
                            continue;
                        }

                        let mut write_failed = None;
                        if line_edit_active {
                            let edit_result = line_editor.process(&data, config.enter_mode);
                            if !edit_result.display.is_empty() {
                                output.push_owned(edit_result.display);
                            }

                            for data in edit_result.writes {
                                let send_data = encode_terminal_input(&data, &encoding);
                                if let Err(e) = writer.write_all(&send_data).await {
                                    write_failed = Some(e);
                                    break;
                                }
                            }
                        } else {
                            if backspace_as_bs {
                                remap_del_to_bs(&mut data);
                            }
                            let data = normalize_enter_bytes(&data, config.enter_mode);
                            if config.local_echo {
                                let echoed = local_echo_text(&data);
                                if !echoed.is_empty() {
                                    output.push_owned(echoed);
                                }
                            }
                            let send_data = encode_terminal_input(&data, &encoding);
                            for chunk in split_write_chunks(&send_data, config.force_character_at_a_time) {
                                if let Err(e) = writer.write_all(&chunk).await {
                                    write_failed = Some(e);
                                    break;
                                }
                            }
                        }

                        if let Some(e) = write_failed {
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
                        let send_command = encode_terminal_input(&wrapped_command, &encoding);
                        if let Err(e) = writer.write_all(&send_command).await {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %e,
                                "Failed to write capture command to Telnet"
                            );
                        }
                    }
                    Some(SessionCommand::CancelCapture { marker_id }) => {
                        capture_processor.lock().await.cancel(&marker_id);
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        if let Some(naws) = maybe_build_naws(cols as u16, rows as u16, &config) {
                            let _ = writer.write_all(&naws).await;
                        }
                    }
                    Some(SessionCommand::PauseOutput) => {
                        let _ = pause_tx.send(true);
                    }
                    Some(SessionCommand::ResumeOutput) => {
                        let _ = pause_tx.send(false);
                    }
                    Some(SessionCommand::AckOutput { bytes }) => {
                        output.ack(bytes);
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
                    Some(SessionCommand::ZmodemAcceptUpload {
                        files,
                        conflict_mode,
                        preserve_timestamps,
                    }) => {
                        let mut zm = zmodem_state.lock().await;
                        if let Some(ref mut transfer) = *zm {
                            let actions =
                                transfer.accept_upload(files, conflict_mode, preserve_timestamps);
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
                        manager.clear_pending_zmodem_upload(&session_id).await;
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

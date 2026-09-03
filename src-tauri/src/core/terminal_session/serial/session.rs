fn serial_session_thread(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: SessionCommandReceiver,
    reader_shutdown_tx: SessionCommandSender,
    output_control_tx: SessionCommandSender,
    rt_handle: tokio::runtime::Handle,
    config: SerialConfig,
    connection_id: Option<String>,
    encoding: String,
    port: Box<dyn SerialPort>,
    mut reader_port: Box<dyn SerialPort>,
) {
    let backspace_as_bs = config.backspace_mode == "ctrl_h";
    let port_writer = Arc::new(Mutex::new(port));
    let output_event = format!("terminal-output-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);
    let output =
        SessionOutputCoalescer::for_app(app.clone(), output_event.clone(), output_control_tx);
    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|state| state.inner().clone());

    let capture_processor = Arc::new(Mutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();
    let output_pause = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let output_pause_reader = output_pause.clone();

    let zmodem_state: Arc<Mutex<Option<ZmodemTransfer>>> = Arc::new(Mutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let manager_reader = manager.clone();
    let rt_handle_reader = rt_handle.clone();
    let port_writer_reader = port_writer.clone();
    let output_reader = output.clone();
    let recording_mgr_reader = recording_mgr.clone();
    let encoding_reader = encoding.clone();

    let reader_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let reader_flag = reader_running.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut zmodem_detector = ZmodemDetector::new();
        let mut output_decoder = TerminalOutputDecoder::new(&encoding_reader);
        while reader_flag.load(std::sync::atomic::Ordering::Relaxed) {
            {
                let (lock, cvar) = &*output_pause_reader;
                let mut paused = lock.lock().unwrap();
                while *paused && reader_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    paused = cvar.wait(paused).unwrap();
                }
            }
            if !reader_flag.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
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
                                rt_handle_reader.block_on(async {
                                    manager_reader.take_pending_zmodem_upload(&sid_reader).await
                                })
                            } else {
                                None
                            };
                            let (transfer, bootstrap_actions) =
                                start_zmodem_transfer(direction, &initial_bytes, prepared_upload);
                            for action in bootstrap_actions {
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
                            *zmodem_state_reader.lock().unwrap() = Some(transfer);
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

                    let mut text = output_decoder.decode(&process_raw);
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
        let _ = reader_shutdown_tx.send(SessionCommand::Close);
    });

    // Command loop
    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            SessionCommand::AttachConfirmed { ack } => {
                output.attach_confirmed(ack);
            }
            SessionCommand::DetachRenderer => {
                output.detach();
            }
            SessionCommand::Write { mut data, .. } => {
                if zmodem_state.lock().unwrap().is_some() {
                    continue;
                }
                if backspace_as_bs {
                    remap_del_to_bs(&mut data);
                }
                let send_data = encode_terminal_input(&data, &encoding);
                let mut p = port_writer.lock().unwrap();
                let _ = p.write_all(&send_data);
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
                let send_command = encode_terminal_input(&wrapped_command, &encoding);
                let mut p = port_writer.lock().unwrap();
                let _ = p.write_all(&send_command);
                let _ = p.flush();
            }
            SessionCommand::CancelCapture { marker_id } => {
                if let Ok(mut proc) = capture_processor.lock() {
                    proc.cancel(&marker_id);
                }
            }
            SessionCommand::Resize { .. } => {}
            SessionCommand::PauseOutput => {
                let (lock, _) = &*output_pause;
                if let Ok(mut paused) = lock.lock() {
                    *paused = true;
                }
            }
            SessionCommand::ResumeOutput => {
                let (lock, cvar) = &*output_pause;
                if let Ok(mut paused) = lock.lock() {
                    *paused = false;
                    cvar.notify_all();
                }
            }
            SessionCommand::AckOutput { bytes } => {
                output.ack(bytes);
            }
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
            SessionCommand::ZmodemAcceptUpload {
                files,
                conflict_mode,
                preserve_timestamps,
            } => {
                let mut zm = zmodem_state.lock().unwrap();
                if let Some(ref mut transfer) = *zm {
                    let actions =
                        transfer.accept_upload(files, conflict_mode, preserve_timestamps);
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
                rt_handle.block_on(async {
                    manager.clear_pending_zmodem_upload(&session_id).await;
                });
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
    {
        let (lock, cvar) = &*output_pause;
        if let Ok(mut paused) = lock.lock() {
            *paused = false;
            cvar.notify_all();
        }
    }
    output.close();

    if let Some(ref recorder) = recording_mgr {
        recorder.cleanup_session(&session_id);
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&closed_event, ());
}

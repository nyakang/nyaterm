use self::xymodem::{XyModemAction, XyModemTransfer};

fn detect_serial_zmodem(
    detector: &mut ZmodemDetector,
    raw: &[u8],
    flow_control: crate::config::SerialFlowControl,
) -> ZmodemDetectResult {
    // XON/XOFF consumes binary payload bytes in the OS serial layer. Keep incoming
    // data on the terminal path rather than automatically starting a transfer.
    if flow_control == crate::config::SerialFlowControl::Software {
        return ZmodemDetectResult::NoMatch {
            passthrough: raw.to_vec(),
        };
    }
    detector.feed(raw)
}

fn process_xymodem_actions(
    app: &AppHandle,
    event_name: &str,
    port_writer: &Arc<Mutex<Box<dyn SerialPort>>>,
    actions: Vec<XyModemAction>,
) {
    for action in actions {
        match action {
            XyModemAction::SendToRemote(data) => {
                let mut port = port_writer.lock().unwrap();
                let _ = port.write_all(&data);
                let _ = port.flush();
            }
            XyModemAction::EmitEvent(event) => {
                let _ = app.emit(event_name, &event);
            }
        }
    }
}

fn store_started_zmodem_transfer(
    slot: &mut Option<ZmodemTransfer>,
    transfer: ZmodemTransfer,
) -> bool {
    if transfer.is_done() {
        *slot = None;
        false
    } else {
        *slot = Some(transfer);
        true
    }
}

fn teardown_serial_io(
    reader_running: Arc<std::sync::atomic::AtomicBool>,
    output_pause: Arc<(Mutex<bool>, std::sync::Condvar)>,
    reader_thread: std::thread::JoinHandle<()>,
    port_writer: Arc<Mutex<Box<dyn SerialPort>>>,
) {
    reader_running.store(false, std::sync::atomic::Ordering::Relaxed);
    {
        let (lock, cvar) = &*output_pause;
        if let Ok(mut paused) = lock.lock() {
            *paused = false;
            cvar.notify_all();
        }
    }

    let _ = reader_thread.join();
    drop(port_writer);
}

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
    let xymodem_state: Arc<Mutex<Option<XyModemTransfer>>> = Arc::new(Mutex::new(None));
    let xymodem_state_reader = xymodem_state.clone();
    let serial_modem_event_name = format!("serial-modem-event-{session_id}");
    let serial_modem_event_reader = serial_modem_event_name.clone();
    let modem_upload_protocol = config.modem_upload_protocol;
    let flow_control = config.flow_control;

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

    let reader_thread = std::thread::spawn(move || {
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
                    let mut raw = &buf[..n];

                    let xymodem_result = {
                        let mut state = xymodem_state_reader.lock().unwrap();
                        if let Some(ref mut transfer) = *state {
                            let result = transfer.feed_incoming_result(raw);
                            let consumed = result.consumed.min(raw.len());
                            process_xymodem_actions(
                                &app_reader,
                                &serial_modem_event_reader,
                                &port_writer_reader,
                                result.actions,
                            );
                            let done = transfer.is_done();
                            if done {
                                *state = None;
                            }
                            Some((consumed, done))
                        } else {
                            None
                        }
                    };
                    if let Some((consumed, done)) = xymodem_result {
                        if !done || consumed >= raw.len() {
                            continue;
                        }
                        raw = &raw[consumed..];
                    }

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
                    let detected = detect_serial_zmodem(&mut zmodem_detector, raw, flow_control);
                    let process_raw = match detected {
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
                            let mut zmodem_guard = zmodem_state_reader.lock().unwrap();
                            let prepared_upload = if direction == ZmodemDirection::Upload {
                                rt_handle_reader.block_on(async {
                                    manager_reader.take_pending_zmodem_upload(&sid_reader).await
                                })
                            } else {
                                None
                            };
                            let prepared_upload_started = prepared_upload.is_some();
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
                            if !store_started_zmodem_transfer(&mut zmodem_guard, transfer) {
                                zmodem_detector.reset();
                            }
                            drop(zmodem_guard);
                            if !prepared_upload_started {
                                let _ = app_reader.emit(
                                    &zmodem_event_reader,
                                    &ZmodemEvent::Detected { direction },
                                );
                            }
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
                    let mut state = xymodem_state_reader.lock().unwrap();
                    if let Some(ref mut transfer) = *state {
                        let actions = transfer.tick();
                        process_xymodem_actions(
                            &app_reader,
                            &serial_modem_event_reader,
                            &port_writer_reader,
                            actions,
                        );
                        if transfer.is_done() {
                            *state = None;
                        }
                    }
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
            SessionCommand::Write { data, raw, .. } => {
                if zmodem_state.lock().unwrap().is_some() || xymodem_state.lock().unwrap().is_some()
                {
                    continue;
                }
                let send_data = prepare_terminal_write_input(data, &encoding, raw, backspace_as_bs);
                let mut p = port_writer.lock().unwrap();
                // On Windows, flush waits for pending serial writes to transmit and can stall
                // subsequent input. Write directly without draining the port for each command.
                let _ = p.write_all(&send_data);
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
                    let actions = transfer.accept_upload(files, conflict_mode, preserve_timestamps);
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
            SessionCommand::SerialModemUpload {
                files,
                conflict_mode,
                preserve_timestamps,
                result_tx,
            } => {
                if let Err(error) = validate_serial_modem_flow_control(flow_control) {
                    let _ = result_tx.send(Err(error));
                    continue;
                }
                let result = match modem_upload_protocol {
                    crate::config::SerialModemUploadProtocol::Xmodem
                    | crate::config::SerialModemUploadProtocol::Ymodem => {
                        if zmodem_state.lock().unwrap().is_some() {
                            Err("A ZMODEM transfer is already active".to_string())
                        } else {
                            let mut xy = xymodem_state.lock().unwrap();
                            if xy.is_some() {
                                Err("A Serial modem upload is already active".to_string())
                            } else {
                                match XyModemTransfer::new(modem_upload_protocol, files) {
                                    Ok((transfer, actions)) => {
                                        process_xymodem_actions(
                                            &app,
                                            &serial_modem_event_name,
                                            &port_writer,
                                            actions,
                                        );
                                        *xy = Some(transfer);
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                        }
                    }
                    crate::config::SerialModemUploadProtocol::Zmodem => {
                        if xymodem_state.lock().unwrap().is_some() {
                            Err("An XMODEM/YMODEM upload is already active".to_string())
                        } else {
                            let mut zm = zmodem_state.lock().unwrap();
                            if let Some(ref mut transfer) = *zm {
                                if transfer.direction() != ZmodemDirection::Upload {
                                    Err("A ZMODEM download is already active".to_string())
                                } else if !transfer.is_waiting_for_user() {
                                    Err("A ZMODEM upload is already active".to_string())
                                } else {
                                    let actions = transfer.accept_upload(
                                        files,
                                        conflict_mode,
                                        preserve_timestamps,
                                    );
                                    for action in actions {
                                        match action {
                                            ZmodemAction::SendToRemote(data) => {
                                                let mut port = port_writer.lock().unwrap();
                                                let _ = port.write_all(&data);
                                                let _ = port.flush();
                                            }
                                            ZmodemAction::EmitEvent(event) => {
                                                let _ = app.emit(&zmodem_event_name, &event);
                                            }
                                        }
                                    }
                                    if transfer.is_done() {
                                        *zm = None;
                                    }
                                    Ok(())
                                }
                            } else {
                                rt_handle
                                    .block_on(async {
                                        manager
                                            .prepare_zmodem_upload(
                                                &session_id,
                                                crate::core::zmodem::ZmodemPreparedUpload {
                                                    files,
                                                    conflict_mode,
                                                    preserve_timestamps,
                                                },
                                            )
                                            .await
                                    })
                                    .map_err(|error| error.to_string())
                            }
                        }
                    }
                };
                let _ = result_tx.send(result);
            }
            SessionCommand::TmuxCommand { .. } | SessionCommand::TmuxDetach => {}
            SessionCommand::Close => {
                break;
            }
        }
    }

    teardown_serial_io(reader_running, output_pause, reader_thread, port_writer);
    output.close();

    if let Some(ref recorder) = recording_mgr {
        recorder.disconnect_session(&session_id);
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&closed_event, ());
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    fn open_pseudo_terminal() -> (File, String) {
        // SAFETY: posix_openpt returns a new owned file descriptor on success.
        let master_fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(
            master_fd >= 0,
            "failed to open pseudo-terminal master: {}",
            std::io::Error::last_os_error()
        );

        // SAFETY: master_fd was just returned by posix_openpt and ownership is transferred to File.
        let master = unsafe { File::from_raw_fd(master_fd) };
        // SAFETY: master is a valid pseudo-terminal master descriptor.
        assert_eq!(
            unsafe { libc::grantpt(master.as_raw_fd()) },
            0,
            "failed to grant pseudo-terminal slave: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: master is a valid pseudo-terminal master descriptor.
        assert_eq!(
            unsafe { libc::unlockpt(master.as_raw_fd()) },
            0,
            "failed to unlock pseudo-terminal slave: {}",
            std::io::Error::last_os_error()
        );

        let mut slave_name = [0 as libc::c_char; 128];
        // SAFETY: slave_name is a writable buffer and master is a valid pseudo-terminal master.
        let rc = unsafe {
            libc::ptsname_r(
                master.as_raw_fd(),
                slave_name.as_mut_ptr(),
                slave_name.len(),
            )
        };
        assert_eq!(
            rc,
            0,
            "failed to resolve pseudo-terminal slave: {}",
            std::io::Error::from_raw_os_error(rc)
        );
        // SAFETY: ptsname_r wrote a NUL-terminated path into slave_name on success.
        let slave_path = unsafe { CStr::from_ptr(slave_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        (master, slave_path)
    }

    #[test]
    fn serial_teardown_releases_port_before_closed_event() {
        let (_master, slave_path) = open_pseudo_terminal();
        let port = serialport::new(&slave_path, 115_200)
            .timeout(Duration::from_millis(10))
            .open()
            .expect("failed to open pseudo-terminal slave as serial port");
        let reader_port = port
            .try_clone()
            .expect("failed to clone pseudo-terminal serial port");
        let port_writer = Arc::new(Mutex::new(port));
        let port_writer_reader = port_writer.clone();

        assert!(
            serialport::new(&slave_path, 115_200)
                .timeout(Duration::from_millis(10))
                .open()
                .is_err(),
            "serial port should remain exclusive while session handles are alive"
        );

        let reader_running = Arc::new(AtomicBool::new(true));
        let reader_flag = reader_running.clone();
        let output_pause = Arc::new((Mutex::new(true), std::sync::Condvar::new()));
        let output_pause_reader = output_pause.clone();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let _reader_port = reader_port;
            let (lock, cvar) = &*output_pause_reader;
            let mut paused = lock.lock().unwrap();
            waiting_tx.send(()).unwrap();
            while *paused && reader_flag.load(Ordering::Relaxed) {
                paused = cvar.wait(paused).unwrap();
            }
            drop(paused);
            drop(port_writer_reader);
        });

        waiting_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reader thread did not enter paused state");

        teardown_serial_io(reader_running, output_pause, reader_thread, port_writer);

        let reopened = serialport::new(&slave_path, 115_200)
            .timeout(Duration::from_millis(10))
            .open()
            .expect("serial port should reopen after teardown releases all handles");
        drop(reopened);
    }
}

#[cfg(test)]
mod serial_modem_bootstrap_tests {
    use super::*;
    use zmodem2::{Encoding, Frame, Header};

    fn zrinit() -> Vec<u8> {
        let mut wire = Vec::new();
        Header::new(Encoding::ZHEX, Frame::ZRINIT, &[0; 4])
            .write(&mut wire)
            .expect("write ZRINIT")
            .expect("complete ZRINIT");
        wire
    }

    #[test]
    fn serial_flow_control_rejects_modem_uploads_only_for_software() {
        use crate::config::SerialFlowControl;
        assert!(validate_serial_modem_flow_control(SerialFlowControl::None).is_ok());
        assert!(validate_serial_modem_flow_control(SerialFlowControl::Hardware).is_ok());
        let reason = validate_serial_modem_flow_control(SerialFlowControl::Software).unwrap_err();
        assert!(reason.contains("XMODEM/YMODEM/ZMODEM"));
        assert!(reason.contains("XON/XOFF"));
    }

    #[test]
    fn serial_flow_control_software_passes_through_split_zmodem_headers() {
        for frame in [Frame::ZRINIT, Frame::ZRQINIT] {
            let mut wire = Vec::new();
            Header::new(Encoding::ZHEX, frame, &[0; 4])
                .write(&mut wire)
                .unwrap()
                .unwrap();
            let mut detector = ZmodemDetector::new();
            for chunk in wire.chunks(3) {
                let result = detect_serial_zmodem(
                    &mut detector,
                    chunk,
                    crate::config::SerialFlowControl::Software,
                );
                assert!(
                    matches!(result, ZmodemDetectResult::NoMatch { passthrough } if passthrough == chunk)
                );
            }
            assert!(!detector.has_pending_prefix());
        }
        for flow_control in [
            crate::config::SerialFlowControl::None,
            crate::config::SerialFlowControl::Hardware,
        ] {
            assert!(matches!(
                detect_serial_zmodem(&mut ZmodemDetector::new(), &zrinit(), flow_control),
                ZmodemDetectResult::Detected {
                    direction: ZmodemDirection::Upload,
                    ..
                }
            ));
        }
    }

    #[test]
    fn serial_modem_missing_prepared_zmodem_file_does_not_stay_active() {
        let missing = std::env::temp_dir().join(format!(
            "nyaterm-missing-prepared-zmodem-{}",
            uuid::Uuid::new_v4()
        ));
        let (transfer, actions) = start_zmodem_transfer(
            ZmodemDirection::Upload,
            &zrinit(),
            Some(crate::core::zmodem::ZmodemPreparedUpload {
                files: vec![missing],
                conflict_mode: crate::core::zmodem::ZmodemUploadConflictMode::Overwrite,
                preserve_timestamps: false,
            }),
        );

        assert!(transfer.is_done());
        assert!(actions.iter().any(|action| matches!(
            action,
            ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason })
                if reason.contains("Failed to open")
        )));

        let mut active = None;
        assert!(!store_started_zmodem_transfer(&mut active, transfer));
        assert!(
            active.is_none(),
            "failed bootstrap must not block normal Serial I/O"
        );
    }
}

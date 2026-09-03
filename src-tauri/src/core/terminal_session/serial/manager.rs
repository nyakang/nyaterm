pub async fn create_serial_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: SerialConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
    session_ready_hook: Option<SessionReadyHook>,
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
    let port = match open_serial_port(&config) {
        Ok(port) => port,
        Err(e) => {
            log_serial_connection_failed(&session_id, connection_id.as_ref(), &config, &e);
            return Err(AppError::Config(format!(
                "Failed to open serial port '{}': {}",
                config.port_name, e
            )));
        }
    };
    let reader_port = match port.try_clone() {
        Ok(port) => port,
        Err(e) => {
            log_serial_connection_failed(&session_id, connection_id.as_ref(), &config, &e);
            return Err(AppError::Config(format!(
                "Failed to open serial port '{}': {}",
                config.port_name, e
            )));
        }
    };
    let (cmd_tx, cmd_rx) = session_command_channel(session_id.clone());
    let reader_shutdown_tx = cmd_tx.clone();
    let output_control_tx = cmd_tx.clone();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::Serial,
        started_at: crate::core::now_session_started_at(),
        connection_id: connection_id.clone(),
        connected: true,
        owner_window_label,
        ai_execution_profile: AiExecutionProfile::SendOnly,
        injection_active: false,
        dynamic_title_capabilities: DynamicTitleCapabilities::default(),
        remote_file_browser_enabled: false,
        remote_stats_enabled: false,
        ssh_profile: None,
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(Default::default()));
    let session_handle = SessionHandle {
        info: session_info.clone(),
        cmd_tx,
        startup_input_barrier: None,
        ssh_config: None,
        ssh_handle: None,
        cwd,
        remote_fs: None,
    };
    manager.add_session(session_handle).await;
    if let Some(hook) = session_ready_hook.as_ref() {
        hook(&session_info);
    }

    let sid = session_id.clone();
    let mgr = manager.clone();
    let rt_handle = tokio::runtime::Handle::current();
    let encoding = if !config.encoding.is_empty() {
        config.encoding.clone()
    } else {
        crate::config::load_app_settings(&app)
            .map(|settings| settings.interaction.default_encoding)
            .unwrap_or_else(|_| "UTF-8".to_string())
    };

    std::thread::spawn(move || {
        serial_session_thread(
            app,
            sid,
            mgr,
            cmd_rx,
            reader_shutdown_tx,
            output_control_tx,
            rt_handle,
            config,
            connection_id,
            encoding,
            port,
            reader_port,
        );
    });

    Ok(session_id)
}

fn open_serial_port(config: &SerialConfig) -> serialport::Result<Box<dyn SerialPort>> {
    serialport::new(&config.port_name, config.baud_rate)
        .data_bits(parse_data_bits(config.data_bits))
        .parity(parse_parity(&config.parity))
        .stop_bits(parse_stop_bits(&config.stop_bits))
        .flow_control(FlowControl::None)
        .timeout(Duration::from_millis(10))
        .open()
}

fn log_serial_connection_failed(
    session_id: &str,
    connection_id: Option<&String>,
    config: &SerialConfig,
    error: &dyn std::fmt::Display,
) {
    log_event(StructuredLog {
        level: StructuredLogLevel::Error,
        domain: "session.lifecycle".to_string(),
        event: "session.connection_failed".to_string(),
        message: "Failed to open serial port".to_string(),
        ids: Some(serde_json::json!({
            "session_id": session_id,
            "connection_id": connection_id,
        })),
        data: Some(serde_json::json!({
            "session_type": "Serial",
            "port_name": config.port_name.clone(),
            "baud_rate": config.baud_rate,
        })),
        error: Some(serde_json::json!({ "message": error.to_string() })),
        client_timestamp: None,
    });
}

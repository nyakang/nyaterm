pub async fn create_telnet_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: TelnetSessionConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
    startup_command: Option<TelnetStartupCommand>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    let host = config.host.clone();
    let port = config.port;
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
    let (cmd_tx, cmd_rx) = session_command_channel(session_id.clone());
    let output_control_tx = cmd_tx.clone();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: config.name.clone(),
        session_type: SessionType::Telnet,
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
    let encoding = if !config.encoding.is_empty() {
        config.encoding.clone()
    } else {
        crate::config::load_app_settings(&app)
            .map(|settings| settings.interaction.default_encoding)
            .unwrap_or_else(|_| "UTF-8".to_string())
    };

    tokio::spawn(async move {
        telnet_session_task(
            app,
            sid,
            mgr,
            cmd_rx,
            output_control_tx,
            config,
            connection_id,
            encoding,
            startup_command,
        )
        .await;
    });

    Ok(session_id)
}

pub async fn create_telnet_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: TelnetSessionConfig,
    connection_id: Option<String>,
    owner_window_label: Option<String>,
    cancel_rx: Option<oneshot::Receiver<()>>,
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
    let stream = match await_telnet_connection(
        async {
            crate::core::network::open_tcp_transport(
                &app,
                &host,
                port,
                config.network.as_ref(),
                owner_window_label.clone(),
            )
            .await
            .map(|opened| opened.stream)
            .map_err(std::io::Error::other)
        },
        cancel_rx,
    )
    .await
    {
        Ok(stream) => stream,
        Err(AppError::Cancelled(message)) => {
            log_event(StructuredLog {
                level: StructuredLogLevel::Info,
                domain: "session.lifecycle".to_string(),
                event: "session.create_cancelled".to_string(),
                message: "Telnet session creation cancelled".to_string(),
                ids: Some(serde_json::json!({
                    "session_id": session_id,
                    "connection_id": connection_id,
                })),
                data: Some(serde_json::json!({
                    "session_type": "Telnet",
                    "host": host,
                    "port": port,
                })),
                error: None,
                client_timestamp: None,
            });
            return Err(AppError::Cancelled(message));
        }
        Err(error) => {
            log_event(StructuredLog {
                level: StructuredLogLevel::Error,
                domain: "session.lifecycle".to_string(),
                event: "session.connection_failed".to_string(),
                message: "Telnet connection failed".to_string(),
                ids: Some(serde_json::json!({
                    "session_id": session_id,
                    "connection_id": connection_id,
                })),
                data: Some(serde_json::json!({
                    "session_type": "Telnet",
                    "host": host,
                    "port": port,
                })),
                error: Some(serde_json::json!({ "message": error.to_string() })),
                client_timestamp: None,
            });
            return Err(error);
        }
    };
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
        cwd_tracking_active: false,
        dynamic_title_capabilities: DynamicTitleCapabilities::default(),
        remote_file_browser_enabled: false,
        remote_stats_enabled: false,
        ssh_profile: None,
        ssh_runtime_mode: None,
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
            stream,
            config,
            connection_id,
            encoding,
            startup_command,
        )
        .await;
    });

    Ok(session_id)
}

async fn await_telnet_connection<F, T>(
    connect: F,
    cancel_rx: Option<oneshot::Receiver<()>>,
) -> AppResult<T>
where
    F: std::future::Future<Output = std::io::Result<T>>,
{
    if let Some(mut cancel_rx) = cancel_rx {
        return tokio::select! {
            result = connect => result.map_err(AppError::Io),
            _ = &mut cancel_rx => Err(AppError::Cancelled("Session creation cancelled".to_string())),
        };
    }

    connect.await.map_err(AppError::Io)
}

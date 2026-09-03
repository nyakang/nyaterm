/// Spawns a local shell in a PTY and registers the session with the manager.
pub async fn create_local_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: Option<LocalSessionConfig>,
    owner_window_label: Option<String>,
    session_ready_hook: Option<SessionReadyHook>,
) -> AppResult<String> {
    tracing::info!("Creating local PTY session");
    let resolved_shell_spec = match &config {
        Some(cfg) if !cfg.shell_path.trim().is_empty() => resolve_shell_command(
            &cfg.shell_path,
            &cfg.shell_args,
        )
        .map_err(crate::error::AppError::Config)?,
        _ => default_shell_spec(),
    };
    validate_working_dir_before_spawn(config.as_ref(), &resolved_shell_spec)?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = session_command_channel(session_id.clone());
    let output_control_tx = cmd_tx.clone();

    let session_name = config
        .as_ref()
        .map_or("Local Terminal".to_string(), |c| c.name.clone());

    let shell_name = resolved_shell_spec.program.clone();
    let resolved_shell_args_empty = resolved_shell_spec.args.is_empty();
    let resolution_source = resolved_shell_spec.resolution_source;
    let ai_execution_profile = infer_local_ai_execution_profile(&shell_name);
    let ready_marker = build_ready_marker(&session_id);
    let dynamic_title_enabled = config
        .as_ref()
        .is_some_and(|cfg| cfg.dynamic_tab_title);
    let allow_injection = config.as_ref().is_none_or(|cfg| {
        should_allow_local_injection(
            &cfg.shell_path,
            &shell_name,
            &cfg.shell_args,
            resolved_shell_args_empty,
            resolution_source,
        )
    });
    let startup = build_local_startup_script(
        &shell_name,
        &ready_marker,
        dynamic_title_enabled,
        allow_injection,
    );
    if dynamic_title_enabled && !startup.dynamic_title_integration_requested {
        tracing::info!(
            shell = %shell_name,
            custom_argv = config
                .as_ref()
                .is_some_and(|cfg| !cfg.shell_args.is_empty()),
            "Local dynamic-title hook unavailable; continuing in passive title mode"
        );
    }
    let LocalStartupScript {
        script: startup_script,
        shell_init_args,
        pwsh_init_args,
        cmd_prompt,
        dynamic_title_integration_requested,
    } = startup;
    let startup_input_barrier = startup_script
        .as_ref()
        .map(|_| Arc::new(StartupInputBarrier::new()));
    // Dynamic-title opt-in must not change HEAD's Local command-history timing.
    let injection_active = false;
    let trusted_initial_title = if cfg!(target_os = "windows") && dynamic_title_enabled {
        Some(shell_name.clone())
    } else {
        None
    };

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: session_name,
        session_type: SessionType::Local,
        started_at: crate::core::now_session_started_at(),
        connection_id: config.as_ref().and_then(|cfg| cfg.connection_id.clone()),
        connected: true,
        owner_window_label,
        ai_execution_profile,
        injection_active,
        // Integration becomes active only after a session-bound ready marker.
        dynamic_title_capabilities: DynamicTitleCapabilities::new(
            dynamic_title_enabled,
            trusted_initial_title,
        ),
        remote_file_browser_enabled: false,
        remote_stats_enabled: false,
        ssh_profile: None,
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(Default::default()));
    let session_handle = SessionHandle {
        info: session_info.clone(),
        cmd_tx,
        startup_input_barrier: startup_input_barrier.clone(),
        ssh_config: None,
        ssh_handle: None,
        cwd: cwd.clone(),
        remote_fs: None,
    };
    manager.add_session(session_handle).await;
    if let Some(hook) = session_ready_hook.as_ref() {
        hook(&session_info);
    }

    let sid = session_id.clone();
    let mgr = manager.clone();
    let rt_handle = tokio::runtime::Handle::current();
    let encoding = config
        .as_ref()
        .map(|c| c.encoding.clone())
        .unwrap_or_else(|| {
            crate::config::load_app_settings(&app)
                .map(|settings| settings.interaction.default_encoding)
                .unwrap_or_else(|_| "UTF-8".to_string())
        });

    std::thread::spawn(move || {
        pty_session_thread(
            app,
            sid,
            mgr,
            cmd_rx,
            output_control_tx,
            rt_handle,
            cwd,
            resolved_shell_spec,
            shell_init_args,
            pwsh_init_args,
            cmd_prompt,
            config,
            startup_script,
            startup_input_barrier,
            dynamic_title_integration_requested,
            ready_marker,
            encoding,
        );
    });

    Ok(session_id)
}

type LocalStartupPipeline = (OscStripper, StartupOutputGate);

fn managed_cwd_runtime_ready(
    managed_requested: bool,
    integration_available: bool,
    integration_ready: bool,
    ready_in_chunk: bool,
) -> bool {
    !managed_requested || (integration_available && (integration_ready || ready_in_chunk))
}

/// The Bash/Zsh cwd hook runs before the PS1-embedded ready marker, and PTY
/// packet boundaries may split those sequences. Retain only the latest cwd
/// emitted while our source-output gate is active, then release it when the
/// session-bound marker arrives. Profile/application OSC 7 emitted before
/// injection remains untrusted and is not staged.
#[allow(clippy::too_many_arguments)]
fn reconcile_managed_cwd_events(
    events: &mut Vec<CwdPayloadEvent>,
    pending: &mut Option<CwdPayloadEvent>,
    managed_requested: bool,
    integration_available: bool,
    integration_ready: bool,
    ready_in_chunk: bool,
    source_gate_was_active: bool,
    integration_failed: bool,
) {
    if !managed_requested {
        // Passive and disabled Local sessions still need the bounded event
        // stream for safe legacy cwd consumers; only managed pre-ready state
        // is held back by this reconciliation step.
        *pending = None;
        return;
    }
    if !integration_available || integration_failed {
        *pending = None;
        events.clear();
        return;
    }
    if integration_ready || ready_in_chunk {
        if events.is_empty() {
            if let Some(event) = pending.take() {
                events.push(event);
            }
        } else {
            *pending = None;
        }
        return;
    }

    if source_gate_was_active {
        *pending = events.last().cloned();
    }
    events.clear();
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn reconcile_managed_cwd_payloads(
    payloads: &mut Vec<String>,
    pending: &mut Option<String>,
    managed_requested: bool,
    integration_available: bool,
    integration_ready: bool,
    ready_in_chunk: bool,
    source_gate_was_active: bool,
    integration_failed: bool,
) {
    let mut events: Vec<CwdPayloadEvent> = payloads
        .drain(..)
        .map(CwdPayloadEvent::Payload)
        .collect();
    let mut pending_event = pending.take().map(CwdPayloadEvent::Payload);
    reconcile_managed_cwd_events(
        &mut events,
        &mut pending_event,
        managed_requested,
        integration_available,
        integration_ready,
        ready_in_chunk,
        source_gate_was_active,
        integration_failed,
    );
    *payloads = events
        .into_iter()
        .filter_map(|event| match event {
            CwdPayloadEvent::Payload(payload) => Some(payload),
            CwdPayloadEvent::Invalidated => None,
        })
        .collect();
    *pending = pending_event.and_then(|event| match event {
        CwdPayloadEvent::Payload(payload) => Some(payload),
        CwdPayloadEvent::Invalidated => None,
    });
}

fn detect_local_zmodem(
    detector: &mut ZmodemDetector,
    raw: &[u8],
) -> ZmodemDetectResult {
    detector.feed(raw)
}

fn input_cancels_startup_injection(origin: InputOrigin) -> bool {
    origin != InputOrigin::TerminalResponse
}

fn cancel_startup_for_pending_zmodem(
    barrier: Option<&Arc<StartupInputBarrier>>,
) -> bool {
    barrier.is_some_and(|barrier| barrier.cancel_pending())
}

fn discard_startup_pipeline(pipeline: &Arc<StdMutex<LocalStartupPipeline>>) -> bool {
    let mut pipeline = lock_or_recover(pipeline, "local_startup_pipeline");
    let (stripper, gate) = &mut *pipeline;
    stripper.flush();
    gate.discard()
}

fn clear_managed_cwd_runtime(
    runtime: &tokio::runtime::Handle,
    cwd: &SharedCwd,
    app: &AppHandle,
    presentation_event: &str,
) {
    let changes = runtime.block_on(async {
        replace_cwd_state(
            cwd,
            SessionCwdReplacement {
                legacy_path: None,
                operational_path: None,
                presentation: None,
            },
        )
        .await
    });
    if changes.presentation_changed {
        let _ = app.emit(presentation_event, &changes.presentation);
    }
}

fn disable_managed_integration(
    runtime: &tokio::runtime::Handle,
    manager: &Arc<SessionManager>,
    session_id: &str,
    availability: &AtomicBool,
    cwd: &SharedCwd,
    app: &AppHandle,
    presentation_event: &str,
) -> bool {
    if !availability.swap(false, Ordering::AcqRel) {
        return false;
    }
    runtime.block_on(manager.set_dynamic_title_integration_active(session_id, false));
    clear_managed_cwd_runtime(runtime, cwd, app, presentation_event);
    true
}

fn drain_startup_pipeline_on_cancel(
    pipeline: &Arc<StdMutex<LocalStartupPipeline>>,
) -> String {
    let mut pipeline = lock_or_recover(pipeline, "local_startup_pipeline");
    let (stripper, gate) = &mut *pipeline;
    if gate.is_active() {
        stripper.flush();
        gate.discard();
        String::new()
    } else {
        // Before injection, buffered bytes are ordinary shell output (for
        // example one half of an OSC color query), never source echo.
        stripper.flush()
    }
}

fn cancel_startup_pipeline(
    pipeline: &Arc<StdMutex<LocalStartupPipeline>>,
    output: &Arc<SessionOutputCoalescer>,
    recording: Option<&Arc<RecordingManager>>,
    session_id: &str,
) {
    let visible = drain_startup_pipeline_on_cancel(pipeline);
    if visible.is_empty() {
        return;
    }
    if let Some(recording) = recording {
        recording.write_output(session_id, &visible);
    }
    output.push_owned(visible);
}

#[allow(clippy::too_many_arguments)]
fn cancel_pending_startup_injection(
    pending_script: &mut Option<String>,
    schedule: &mut Option<StartupInjectionSchedule>,
    pipeline: Option<&Arc<StdMutex<LocalStartupPipeline>>>,
    output_order: Option<&Arc<StdMutex<()>>>,
    startup_file: Option<&Arc<LocalStartupScriptFile>>,
    output: &Arc<SessionOutputCoalescer>,
    recording: Option<&Arc<RecordingManager>>,
    runtime: &tokio::runtime::Handle,
    manager: &Arc<SessionManager>,
    session_id: &str,
    availability: &AtomicBool,
    cwd: &SharedCwd,
    app: &AppHandle,
    presentation_event: &str,
) -> bool {
    // Serialize cancellation drain with the reader's parse-and-publish step.
    // Without this guard, a trailing half of an escape sequence can be flushed
    // before the visible prefix that the reader already parsed.
    let _output_order_guard =
        output_order.map(|order| lock_or_recover(order, "local_startup_output_order"));
    let was_pending = pending_script.take().is_some() || schedule.take().is_some();
    *schedule = None;
    let was_suppressing = pipeline.is_some_and(|pipeline| {
        lock_or_recover(pipeline, "local_startup_pipeline")
            .1
            .is_active()
    });
    if !was_pending && !was_suppressing {
        return false;
    }
    if let Some(pipeline) = pipeline {
        cancel_startup_pipeline(pipeline, output, recording, session_id);
    }
    // A pending command was never written, so its source file is unused. Once
    // suppression is active the short source command may already be queued;
    // keep the file until its marker, timeout or reader teardown.
    if was_pending {
        if let Some(file) = startup_file {
            file.remove();
        }
    }
    disable_managed_integration(
        runtime,
        manager,
        session_id,
        availability,
        cwd,
        app,
        presentation_event,
    );
    true
}

fn pty_session_thread(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: SessionCommandReceiver,
    output_control_tx: SessionCommandSender,
    rt_handle: tokio::runtime::Handle,
    cwd: SharedCwd,
    resolved_shell_spec: ShellCommandSpec,
    shell_init_args: Option<Vec<String>>,
    pwsh_init_args: Option<Vec<String>>,
    cmd_prompt: Option<String>,
    config: Option<LocalSessionConfig>,
    startup_script: Option<String>,
    startup_input_barrier: Option<Arc<StartupInputBarrier>>,
    dynamic_title_integration_requested: bool,
    ready_marker: String,
    encoding: String,
) {
    let mut dynamic_title_integration_requested = dynamic_title_integration_requested;
    let (startup_script, startup_script_file) = match startup_script {
        Some(source) => match prepare_local_startup_injection(source) {
            Ok(prepared) => prepared,
            Err(error) => {
                dynamic_title_integration_requested = false;
                if let Some(barrier) = startup_input_barrier.as_ref() {
                    barrier.cancel_pending();
                }
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "Could not prepare Local shell integration; continuing in passive mode"
                );
                (String::new(), None)
            }
        },
        None => (String::new(), None),
    };
    let startup_script = (!startup_script.is_empty()).then_some(startup_script);

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to open PTY: {}", e);
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to open PTY: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async {
                manager.remove_session(&session_id).await;
            });
            return;
        }
    };

    // Use the exact command spec that authorized integration. Re-resolving a
    // Windows Terminal profile here could turn a profile into a fallback (or
    // vice versa) after startup hooks and metadata have already been chosen.
    let (mut cmd, _) = build_shell_command_from_spec(&resolved_shell_spec);

    if let Some(ref cfg) = config {
        if let Some(ref dir) = cfg.working_dir {
            if apply_working_dir_to_command(&mut cmd, dir)
                == WorkingDirectoryOutcome::MissingLocalDirectory
            {
                if cfg.fail_on_missing_working_dir {
                    tracing::error!(
                        working_dir = %dir,
                        "Explicit local terminal working directory does not exist"
                    );
                    let _ = app.emit(
                        &format!("session-error-{}", session_id),
                        format!("Working directory '{}' does not exist.", dir),
                    );
                    let _ = app.emit(&format!("session-closed-{}", session_id), ());
                    rt_handle.block_on(async {
                        manager.remove_session(&session_id).await;
                    });
                    return;
                }
                tracing::warn!(
                    working_dir = %dir,
                    "Configured local terminal working directory does not exist; using default working directory"
                );
                let _ = app.emit(
                    &format!("session-warning-{}", session_id),
                    format!(
                        "Configured working directory '{}' does not exist; using the default working directory.",
                        dir
                    ),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    ensure_macos_interactive_path(&mut cmd);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    configure_local_pty_environment(&mut cmd);

    // Structured shell init arguments are used only when the saved argv is
    // empty. Fish `--init-command` runs after user config and before the first
    // prompt, avoiding PTY echo/query races entirely.
    if let Some(args) = shell_init_args.as_ref() {
        if !args.is_empty() {
            cmd.args(args.iter().map(String::as_str));
        }
    }

    // PowerShell prompt injection rides on spawn args (PTY-written scripts are
    // unreliable for interactive pwsh on Windows). Silently skipped when the
    // user supplied custom shell arguments — we never mutate their command.
    if let Some(args) = pwsh_init_args.as_ref() {
        if !args.is_empty() {
            cmd.args(args.iter().map(String::as_str));
        }
    }
    if let Some(prompt) = cmd_prompt.as_deref() {
        cmd.env("PROMPT", prompt);
    }

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to spawn shell: {}", e);
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to spawn shell: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async {
                manager.remove_session(&session_id).await;
            });
            return;
        }
    };
    let shell_process_id = child.process_id();
    drop(pair.slave);

    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Failed to take PTY writer: {}", e);
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to take PTY writer: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async {
                manager.remove_session(&session_id).await;
            });
            return;
        }
    };

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to clone PTY reader: {}", e);
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to clone PTY reader: {}", e),
            );
            let _ = app.emit(&format!("session-closed-{}", session_id), ());
            rt_handle.block_on(async {
                manager.remove_session(&session_id).await;
            });
            return;
        }
    };
    let master = pair.master;

    let output_event = format!("terminal-output-{}", session_id);
    let output =
        SessionOutputCoalescer::for_app(app.clone(), output_event.clone(), output_control_tx);

    let capture_processor = Arc::new(StdMutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();
    let output_pause = Arc::new((StdMutex::new(false), std::sync::Condvar::new()));
    let output_pause_reader = output_pause.clone();

    let zmodem_state: Arc<StdMutex<Option<ZmodemTransfer>>> = Arc::new(StdMutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_input_blocked = Arc::new(AtomicBool::new(false));
    let zmodem_input_blocked_reader = zmodem_input_blocked.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();
    let (zmodem_out_tx, mut zmodem_out_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_read = app.clone();
    let sid_read = session_id.clone();
    let cwd_event = format!("cwd-changed-{}", session_id);
    let cwd_presentation_event = format!("cwd-presentation-changed-{}", session_id);
    // Operational cwd consumers are always security-sensitive, even when
    // dynamic title presentation is disabled. Parse every Local OSC 7 through
    // the bounded strict projection before preserving any safe legacy spelling.
    let cwd_context = LocalCwdContext::new(&session_id);
    let rt_for_reader = rt_handle.clone();
    let recording_mgr_reader: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|s| s.inner().clone());
    let sid_for_rec_reader = session_id.clone();
    let output_reader = output.clone();
    let manager_reader = manager.clone();
    let integration_available = Arc::new(AtomicBool::new(dynamic_title_integration_requested));
    let integration_available_reader = integration_available.clone();
    let integration_ready = Arc::new(AtomicBool::new(false));
    let integration_ready_reader = integration_ready.clone();
    let startup_pipeline = startup_script.as_ref().map(|_| {
        Arc::new(StdMutex::new((
            OscStripper::with_cwd_payloads(&ready_marker, true),
            StartupOutputGate::new_inactive(),
        )))
    });
    let startup_pipeline_reader = startup_pipeline.clone();
    let startup_output_order = startup_script
        .as_ref()
        .map(|_| Arc::new(StdMutex::new(())));
    let startup_output_order_reader = startup_output_order.clone();
    let startup_script_file_reader = startup_script_file.clone();
    let startup_input_barrier_reader = startup_input_barrier.clone();
    let startup_terminal_sequence_pending = Arc::new(AtomicBool::new(false));
    let startup_terminal_sequence_pending_reader =
        startup_terminal_sequence_pending.clone();
    let cwd_reader = cwd.clone();
    let cwd_presentation_event_reader = cwd_presentation_event.clone();
    let encoding_for_reader = encoding.clone();
    let startup_activity = startup_script
        .as_ref()
        .map(|_| Arc::new(StdMutex::new(None::<Instant>)));
    let startup_activity_reader = startup_activity.clone();
    let (reader_done_tx, reader_done_rx) = std_mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut raw_buf = [0u8; 4096];
        let mut direct_stripper = startup_pipeline_reader
            .is_none()
            .then(|| OscStripper::with_cwd_payloads(&ready_marker, true));
        let mut zmodem_detector = ZmodemDetector::new();
        let mut terminal_query_detector = TerminalQueryDetector::new();
        let mut pending_managed_cwd_event: Option<CwdPayloadEvent> = None;
        let mut output_decoder = TerminalOutputDecoder::new(&encoding_for_reader);
        loop {
            {
                let (lock, cvar) = &*output_pause_reader;
                let mut paused = lock.lock().unwrap();
                while *paused {
                    paused = cvar.wait(paused).unwrap();
                }
            }
            match reader.read(&mut raw_buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(activity) = startup_activity_reader.as_ref() {
                        *lock_or_recover(activity, "local_startup_activity") =
                            Some(Instant::now());
                    }
                    let raw = &raw_buf[..n];

                    // ZMODEM: if active, route raw bytes to the transfer.
                    {
                        let mut zm = zmodem_state_reader.lock().unwrap();
                        if let Some(ref mut transfer) = *zm {
                            let actions = transfer.feed_incoming(raw);
                            for action in actions {
                                match action {
                                    ZmodemAction::SendToRemote(data) => {
                                        let _ = zmodem_out_tx.send(data);
                                    }
                                    ZmodemAction::EmitEvent(event) => {
                                        let _ = app_read.emit(&zmodem_event_reader, &event);
                                    }
                                }
                            }
                            if transfer.is_done() {
                                *zm = None;
                                zmodem_input_blocked_reader.store(false, Ordering::Release);
                                zmodem_detector.reset();
                            }
                            continue;
                        }
                    }

                    // ZMODEM detection always runs, including while startup
                    // output is gated. Otherwise a profile-started transfer or
                    // a ready+header packet can be irrecoverably decoded as text.
                    let startup_suppressed = startup_pipeline_reader.as_ref().is_some_and(
                        |pipeline| {
                            lock_or_recover(pipeline, "local_startup_pipeline")
                                .1
                                .is_active()
                        },
                    );
                    let process_raw = match detect_local_zmodem(&mut zmodem_detector, raw) {
                            ZmodemDetectResult::Detected {
                                direction,
                                passthrough,
                                initial_bytes,
                            } => {
                            // Publish the protocol-input barrier before any
                            // potentially blocking upload lookup/bootstrap.
                            zmodem_input_blocked_reader.store(true, Ordering::Release);
                            if let Some(barrier) = startup_input_barrier_reader.as_ref() {
                                barrier.cancel_pending();
                            }
                            if let Some(file) = startup_script_file_reader.as_ref() {
                                file.remove();
                            }
                            if startup_suppressed {
                                if let Some(pipeline) = startup_pipeline_reader.as_ref() {
                                    discard_startup_pipeline(pipeline);
                                }
                                disable_managed_integration(
                                    &rt_for_reader,
                                    &manager_reader,
                                    &sid_for_rec_reader,
                                    &integration_available_reader,
                                    &cwd_reader,
                                    &app_read,
                                    &cwd_presentation_event_reader,
                                );
                            } else if !passthrough.is_empty() {
                                    if let Some(rec) = recording_mgr_reader.as_ref() {
                                        rec.write_raw_output(&sid_for_rec_reader, &passthrough);
                                    }
                                    let pre = output_decoder.decode(&passthrough);
                                    if !pre.is_empty() {
                                        output_reader.push_owned(pre);
                                    }
                                }

                                let prepared_upload = if direction == ZmodemDirection::Upload {
                                    rt_for_reader.block_on(async {
                                        manager_reader.take_pending_zmodem_upload(&sid_read).await
                                    })
                                } else {
                                    None
                                };
                                let (transfer, bootstrap_actions) = start_zmodem_transfer(
                                    direction,
                                    &initial_bytes,
                                    prepared_upload,
                                );
                                for action in bootstrap_actions {
                                    match action {
                                        ZmodemAction::SendToRemote(data) => {
                                            let _ = zmodem_out_tx.send(data);
                                        }
                                        ZmodemAction::EmitEvent(event) => {
                                            let _ = app_read.emit(&zmodem_event_reader, &event);
                                        }
                                    }
                                }
                                *zmodem_state_reader.lock().unwrap() = Some(transfer);
                                let _ = app_read.emit(
                                    &zmodem_event_reader,
                                    &ZmodemEvent::Detected { direction },
                                );
                                continue;
                            }
                            ZmodemDetectResult::NoMatch { passthrough } => {
                            if zmodem_detector.has_pending_prefix() {
                                let cancelled_pending_startup =
                                    cancel_startup_for_pending_zmodem(
                                        startup_input_barrier_reader.as_ref(),
                                    );
                                if cancelled_pending_startup {
                                    if let Some(file) = startup_script_file_reader.as_ref() {
                                        file.remove();
                                    }
                                }
                                if cancelled_pending_startup && startup_suppressed {
                                    if let Some(pipeline) = startup_pipeline_reader.as_ref() {
                                        discard_startup_pipeline(pipeline);
                                    }
                                    disable_managed_integration(
                                        &rt_for_reader,
                                        &manager_reader,
                                        &sid_for_rec_reader,
                                        &integration_available_reader,
                                        &cwd_reader,
                                        &app_read,
                                        &cwd_presentation_event_reader,
                                    );
                                    // `passthrough` may contain the hidden
                                    // source-command echo that preceded this
                                    // partial header. Never re-run it through
                                    // the now-inactive gate.
                                    continue;
                                }
                            }
                                if passthrough.is_empty() {
                                    continue;
                                }
                            if !startup_suppressed {
                                if let Some(rec) = recording_mgr_reader.as_ref() {
                                    rec.write_raw_output(&sid_for_rec_reader, &passthrough);
                                }
                            }
                            passthrough
                        }
                    };

                    let text = output_decoder.decode(&process_raw);
                    let _startup_output_order_guard = startup_output_order_reader
                        .as_ref()
                        .map(|order| lock_or_recover(order, "local_startup_output_order"));
                    if let Some(barrier) = startup_input_barrier_reader.as_ref() {
                        barrier.note_terminal_queries(terminal_query_detector.push(&text));
                        startup_terminal_sequence_pending_reader.store(
                            terminal_query_detector.has_pending_sequence(),
                            Ordering::Release,
                        );
                    }
                    let (mut result, gated_output, source_gate_was_active) =
                        if let Some(pipeline) = startup_pipeline_reader.as_ref() {
                            let mut pipeline =
                                lock_or_recover(pipeline, "local_startup_pipeline");
                            let (stripper, gate) = &mut *pipeline;
                            let source_gate_was_active = gate.is_active();
                    let mut result = stripper.push(&text);
                            let gated = gate.consume(
                                std::mem::take(&mut result.visible),
                                std::mem::take(&mut result.visible_after_ready),
                                result.ready || result.ready_failed,
                            );
                            if matches!(gated, StartupGateOutput::SuppressedOverflow(_)) {
                                stripper.flush();
                            }
                            (result, Some(gated), source_gate_was_active)
                        } else {
                            (
                                direct_stripper
                                    .as_mut()
                                    .expect("direct Local OSC parser")
                                    .push(&text),
                                None,
                                false,
                            )
                        };

                    let ready_seen = result.ready;
                    if result.ready || result.ready_failed {
                        if let Some(file) = startup_script_file_reader.as_ref() {
                            file.remove();
                        }
                    }
                    let integration_available_now =
                        integration_available_reader.load(Ordering::Acquire);
                    let integration_ready_now =
                        integration_ready_reader.load(Ordering::Acquire);
                    let integration_failed = result.ready_failed
                        || matches!(
                            &gated_output,
                            Some(StartupGateOutput::SuppressedOverflow(_))
                        );
                    reconcile_managed_cwd_events(
                        &mut result.cwd_payload_events,
                        &mut pending_managed_cwd_event,
                        dynamic_title_integration_requested,
                        integration_available_now,
                        integration_ready_now,
                        ready_seen,
                        source_gate_was_active,
                        integration_failed,
                    );
                    let managed_runtime_ready = managed_cwd_runtime_ready(
                        dynamic_title_integration_requested,
                        integration_available_now,
                        integration_ready_now,
                        ready_seen,
                    );
                    // Every Local OSC 7 is strictly parsed before it can feed
                    // operational consumers. Managed hooks additionally wait
                    // for their session-bound ready marker; passive sessions
                    // retain safe legacy spelling only.
                    {
                        debug_assert!(managed_runtime_ready || result.cwd_payload_events.is_empty());
                        let cwd_events = result.cwd_payload_events.clone();
                        for event in cwd_events {
                            let replacement = match event {
                                CwdPayloadEvent::Payload(payload) => {
                                    let report = parse_local_cwd_report(&payload, &cwd_context);
                                    cwd_state_replacement(
                                        &report,
                                        dynamic_title_integration_requested,
                                    )
                                }
                                CwdPayloadEvent::Invalidated => SessionCwdReplacement {
                                    legacy_path: None,
                                    operational_path: None,
                                    presentation: None,
                                },
                            };
                            let changes = rt_for_reader.block_on(async {
                                replace_cwd_state(&cwd_reader, replacement).await
                            });
                            if changes.operational_changed {
                                let next_cwd = changes.operational_path.as_deref().unwrap_or("");
                                let _ = app_read.emit(&cwd_event, next_cwd);
                            }
                            if changes.presentation_changed {
                                let _ = app_read.emit(
                                    &cwd_presentation_event_reader,
                                    &changes.presentation,
                                );
                            }
                        }
                    }

                    for command in &result.accepted_commands {
                        let accepted = rt_for_reader.block_on(
                            manager_reader
                                .confirm_command_submission(&sid_for_rec_reader, command.clone()),
                        );
                        if accepted {
                        let _ = app_read.emit(
                            "session-command-accepted",
                            serde_json::json!({
                                "sessionId": &sid_for_rec_reader,
                                "command": command,
                            }),
                        );
                    }
                    }

                    result.visible = match gated_output.unwrap_or_else(|| {
                        StartupGateOutput::Pass(std::mem::take(&mut result.visible))
                    }) {
                        StartupGateOutput::Buffered(visible)
                        | StartupGateOutput::Ready(visible)
                        | StartupGateOutput::Pass(visible) => visible,
                        StartupGateOutput::SuppressedOverflow(visible) => {
                            disable_managed_integration(
                                &rt_for_reader,
                                &manager_reader,
                                &sid_for_rec_reader,
                                &integration_available_reader,
                                &cwd_reader,
                                &app_read,
                                &cwd_presentation_event_reader,
                            );
                            tracing::warn!(
                                session_id = %sid_read,
                                max_bytes = MAX_STARTUP_OUTPUT_BUFFER,
                                "Local shell integration output exceeded its startup buffer; suppressing injection and continuing in passive mode"
                            );
                            visible
                        }
                    };

                    if result.ready_failed
                        && disable_managed_integration(
                            &rt_for_reader,
                            &manager_reader,
                            &sid_for_rec_reader,
                            &integration_available_reader,
                            &cwd_reader,
                            &app_read,
                            &cwd_presentation_event_reader,
                        )
                    {
                        tracing::warn!(
                            session_id = %sid_read,
                            "Local shell rejected dynamic-title integration; continuing in passive mode"
                        );
                    }

                    let ready_accepted = ready_seen
                        && !result.ready_failed
                        && integration_available_reader.load(Ordering::Acquire);
                    if ready_accepted
                        && !integration_ready_reader.swap(true, Ordering::AcqRel)
                    {
                        rt_for_reader.block_on(
                            manager_reader.set_dynamic_title_integration_active(
                                &sid_for_rec_reader,
                                true,
                            ),
                        );
                    }

                    if let Ok(mut proc) = capture_for_reader.lock() {
                        if proc.has_active() {
                            result.visible = proc.process(&result.visible);
                        }
                    }

                    if !result.visible.is_empty() {
                        if let Some(rec) = recording_mgr_reader.as_ref() {
                            rec.write_output(&sid_for_rec_reader, &result.visible);
                        }
                        output_reader.push_owned(result.visible);
                    }
                }
                Err(error) => {
                    tracing::debug!(
                        session_id = %sid_read,
                        error = %error,
                        "Local PTY reader exited"
                    );
                    break;
                }
            }
        }
        integration_available_reader.store(false, Ordering::Release);
        if let Some(file) = startup_script_file_reader.as_ref() {
            file.remove();
        }
        if let Some(pipeline) = startup_pipeline_reader.as_ref() {
            discard_startup_pipeline(pipeline);
        }
        output_reader.close();
        let _ = reader_done_tx.send(());
    });

    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|s| s.inner().clone());
    let has_unix_startup_script = startup_script.is_some();
    let mut pending_startup_script = startup_script;
    let mut startup_injection_schedule = pending_startup_script.as_ref().map(|_| {
        StartupInjectionSchedule::new(
            Instant::now(),
            LOCAL_SHELL_STARTUP_IDLE,
            LOCAL_SHELL_STARTUP_TIMEOUT,
        )
    });

    if dynamic_title_integration_requested && !has_unix_startup_script {
        let availability = integration_available.clone();
        let ready = integration_ready.clone();
        let watchdog_runtime = rt_handle.clone();
        let watchdog_manager = manager.clone();
        let watchdog_cwd = cwd.clone();
        let watchdog_app = app.clone();
        let watchdog_presentation_event = cwd_presentation_event.clone();
        let watchdog_session_id = session_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            if !ready.load(Ordering::Acquire)
                && disable_managed_integration(
                    &watchdog_runtime,
                    &watchdog_manager,
                    &watchdog_session_id,
                    &availability,
                    &watchdog_cwd,
                    &watchdog_app,
                    &watchdog_presentation_event,
                )
            {
            tracing::warn!(
                    session_id = %watchdog_session_id,
                    "Local shell dynamic-title hook did not report ready within 10 seconds; continuing in passive mode"
            );
        }
        });
    }
    loop {
        match reader_done_rx.try_recv() {
            Ok(()) | Err(std_mpsc::TryRecvError::Disconnected) => {
                tracing::debug!(
                    session_id = %session_id,
                    "Local PTY reader signalled session completion"
                );
                break;
            }
            Err(std_mpsc::TryRecvError::Empty) => {}
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::info!(
                    session_id = %session_id,
                    exit_status = ?status,
                    "Local PTY child exited"
                );
                break;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "Failed to query local PTY child status; closing session"
                );
                break;
            }
        }

        if startup_injection_schedule.is_some() {
            if startup_input_barrier
                .as_ref()
                .is_some_and(|barrier| barrier.is_cancelled())
                || lock_or_recover(&zmodem_state, "local_zmodem_state").is_some()
            {
                cancel_pending_startup_injection(
                    &mut pending_startup_script,
                    &mut startup_injection_schedule,
                    startup_pipeline.as_ref(),
                    startup_output_order.as_ref(),
                    startup_script_file.as_ref(),
                    &output,
                    recording_mgr.as_ref(),
                    &rt_handle,
                    &manager,
                    &session_id,
                    &integration_available,
                    &cwd,
                    &app,
                    &cwd_presentation_event,
                );
            }
            if let Some(activity) = startup_activity.as_ref() {
                if let Some(activity_at) =
                    lock_or_recover(activity, "local_startup_activity").take()
                {
                    if let Some(schedule) = startup_injection_schedule.as_mut() {
                        schedule.note_activity(activity_at);
                    }
                }
            }

            let scheduled_decision = startup_injection_schedule
                .as_ref()
                .map(|schedule| schedule.decision(Instant::now()))
                .unwrap_or(StartupInjectionDecision::Wait);
            let decision = if startup_injection_schedule.is_none() {
                StartupInjectionDecision::Wait
            } else if scheduled_decision == StartupInjectionDecision::Timeout {
                StartupInjectionDecision::Timeout
            } else if !output.is_attached()
                || !local_shell_line_editor_ready(&*master, shell_process_id)
                || startup_input_barrier
                    .as_ref()
                    .is_some_and(|barrier| barrier.has_pending_terminal_query())
                || startup_terminal_sequence_pending.load(Ordering::Acquire)
                || !cmd_rx.is_empty()
            {
                // Wait for an attached xterm.js renderer, the shell's actual
                // foreground line editor (not an idle gap inside a slow rc),
                // and a drained terminal-response queue before injecting.
                StartupInjectionDecision::Wait
            } else {
                scheduled_decision
            };
            match decision {
                StartupInjectionDecision::Wait => {}
                StartupInjectionDecision::Timeout => {
                    if let Some(barrier) = startup_input_barrier.as_ref() {
                        barrier.cancel_pending();
                    }
                    cancel_pending_startup_injection(
                        &mut pending_startup_script,
                        &mut startup_injection_schedule,
                        startup_pipeline.as_ref(),
                        startup_output_order.as_ref(),
                        startup_script_file.as_ref(),
                        &output,
                        recording_mgr.as_ref(),
                        &rt_handle,
                        &manager,
                        &session_id,
                        &integration_available,
                        &cwd,
                        &app,
                        &cwd_presentation_event,
                    );
                    tracing::warn!(
                        session_id = %session_id,
                        "Local shell did not reach the safe renderer/line-editor startup barrier before timeout; continuing in passive mode"
                    );
                }
                StartupInjectionDecision::Inject => {
                    let script = pending_startup_script
                        .as_ref()
                        .expect("scheduled Local startup script")
                        .clone();
                    let injection_result = if let Some(barrier) = startup_input_barrier.as_ref() {
                        barrier.try_inject(|| {
                            if let Some(pipeline) = startup_pipeline.as_ref() {
                                lock_or_recover(pipeline, "local_startup_pipeline")
                                    .1
                                    .activate();
                            }
                            write_to_pty(&mut *writer, script.as_bytes())
                        })
                    } else {
                        Ok(StartupInjectionAttempt::Cancelled)
                    };
                    match injection_result {
                        Ok(StartupInjectionAttempt::WaitForTerminalResponse) => {}
                        Ok(StartupInjectionAttempt::Cancelled) => {
                            cancel_pending_startup_injection(
                                &mut pending_startup_script,
                                &mut startup_injection_schedule,
                                startup_pipeline.as_ref(),
                                startup_output_order.as_ref(),
                                startup_script_file.as_ref(),
                                &output,
                                recording_mgr.as_ref(),
                                &rt_handle,
                                &manager,
                                &session_id,
                                &integration_available,
                                &cwd,
                                &app,
                                &cwd_presentation_event,
                            );
                        }
                        Ok(StartupInjectionAttempt::Injected(())) => {
                            startup_injection_schedule = None;
                            pending_startup_script = None;
                            if let Some(pipeline) = startup_pipeline.clone() {
                                let availability = integration_available.clone();
                                let startup_file_watchdog = startup_script_file.clone();
                                let watchdog_runtime = rt_handle.clone();
                                let watchdog_manager = manager.clone();
                                let watchdog_cwd = cwd.clone();
                                let watchdog_app = app.clone();
                                let watchdog_presentation_event =
                                    cwd_presentation_event.clone();
                                let watchdog_session_id = session_id.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(LOCAL_SHELL_STARTUP_TIMEOUT);
                                    if let Some(file) = startup_file_watchdog.as_ref() {
                                        file.remove();
                                    }
                                    let discarded = discard_startup_pipeline(&pipeline);
                                    if discarded {
                                        disable_managed_integration(
                                            &watchdog_runtime,
                                            &watchdog_manager,
                                            &watchdog_session_id,
                                            &availability,
                                            &watchdog_cwd,
                                            &watchdog_app,
                                            &watchdog_presentation_event,
                                        );
                                        tracing::warn!(
                                            session_id = %watchdog_session_id,
                                            "Local shell integration did not become ready within 10 seconds; continuing in passive mode"
                                        );
                                    }
                                });
                            }
                        }
                        Err(error) => {
                            startup_injection_schedule = None;
                            pending_startup_script = None;
                            if let Some(pipeline) = startup_pipeline.as_ref() {
                                discard_startup_pipeline(pipeline);
                            }
                            if let Some(file) = startup_script_file.as_ref() {
                                file.remove();
                            }
                            disable_managed_integration(
                                &rt_handle,
                                &manager,
                                &session_id,
                                &integration_available,
                                &cwd,
                                &app,
                                &cwd_presentation_event,
                            );
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "Failed to write Local dynamic-title startup script; continuing without integration"
                            );
                        }
                    }
                }
            }
        }

        // Drain any ZMODEM outgoing data first (non-blocking).
        while let Ok(data) = zmodem_out_rx.try_recv() {
            let _ = write_to_pty(&mut *writer, &data);
        }

        let cmd = match cmd_rx.try_recv() {
            Ok(cmd) => cmd,
            Err(mpsc::error::TryRecvError::Disconnected) => break,
            Err(mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        match cmd {
            SessionCommand::AttachConfirmed { ack } => {
                output.attach_confirmed(ack);
            }
            SessionCommand::DetachRenderer => {
                output.detach();
            }
            SessionCommand::Write { data, origin, .. } => {
                if input_cancels_startup_injection(origin)
                    && startup_input_barrier
                        .as_ref()
                        .is_some_and(|barrier| barrier.cancel_for_input(origin))
                {
                    cancel_pending_startup_injection(
                        &mut pending_startup_script,
                        &mut startup_injection_schedule,
                        startup_pipeline.as_ref(),
                        startup_output_order.as_ref(),
                        startup_script_file.as_ref(),
                        &output,
                        recording_mgr.as_ref(),
                        &rt_handle,
                        &manager,
                        &session_id,
                        &integration_available,
                        &cwd,
                        &app,
                        &cwd_presentation_event,
                    );
                }
                if zmodem_input_blocked.load(Ordering::Acquire) {
                    continue;
                }
                let send_data = encode_terminal_input(&data, &encoding);
                let write_started_at = Instant::now();
                match write_to_pty(&mut *writer, &send_data) {
                    Ok(()) => {
                        if origin == InputOrigin::TerminalResponse {
                            let resolved_startup_query = startup_input_barrier
                                .as_ref()
                                .is_some_and(|barrier| barrier.resolve_terminal_response());
                            if resolved_startup_query {
                                if let Some(schedule) = startup_injection_schedule.as_mut() {
                                    schedule.require_output_after_terminal_response(write_started_at);
                                }
                            }
                        }
                    }
                    Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "Failed to write to local PTY"
                    );
                }
            }
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
                if let Err(error) = write_to_pty(&mut *writer, &send_command) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "Failed to write capture command to local PTY"
                    );
                }
            }
            SessionCommand::CancelCapture { marker_id } => {
                if let Ok(mut proc) = capture_processor.lock() {
                    proc.cancel(&marker_id);
                }
            }
            SessionCommand::Resize { cols, rows } => {
                let _ = master.resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
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
                                let _ = write_to_pty(&mut *writer, &data);
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                    if transfer.is_done() {
                        *zm = None;
                        zmodem_input_blocked.store(false, Ordering::Release);
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
                                let _ = write_to_pty(&mut *writer, &data);
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                    if transfer.is_done() {
                        *zm = None;
                        zmodem_input_blocked.store(false, Ordering::Release);
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
                                let _ = write_to_pty(&mut *writer, &data);
                            }
                            ZmodemAction::EmitEvent(event) => {
                                let _ = app.emit(&zmodem_event_name, &event);
                            }
                        }
                    }
                }
                *zm = None;
                zmodem_input_blocked.store(false, Ordering::Release);
            }
            SessionCommand::Close => {
                break;
            }
        }
    }

    {
        let (lock, cvar) = &*output_pause;
        if let Ok(mut paused) = lock.lock() {
            *paused = false;
            cvar.notify_all();
        }
    }

    drop(writer);
    drop(master);
    let _ = reader_done_rx.recv_timeout(Duration::from_millis(250));
    output.close();

    if let Some(ref rec) = recording_mgr {
        rec.cleanup_session(&session_id);
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&format!("session-closed-{}", session_id), ());
}

fn validate_working_dir_before_spawn(
    config: Option<&LocalSessionConfig>,
    resolved_shell_spec: &ShellCommandSpec,
) -> AppResult<()> {
    let Some(cfg) = config else {
        return Ok(());
    };
    if !cfg.fail_on_missing_working_dir {
        return Ok(());
    }
    let Some(dir) = cfg.working_dir.as_deref() else {
        return Ok(());
    };
    if dir.trim().is_empty() {
        return Err(crate::error::AppError::Config(
            "Working directory is empty.".to_string(),
        ));
    }

    let (mut cmd, _) = build_shell_command_from_spec(resolved_shell_spec);

    if apply_working_dir_to_command(&mut cmd, dir) == WorkingDirectoryOutcome::MissingLocalDirectory
    {
        return Err(crate::error::AppError::Config(format!(
            "Working directory '{}' does not exist.",
            dir
        )));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkingDirectoryOutcome {
    Skipped,
    AppliedLocalCwd,
    AppliedWslCd,
    WslCdAlreadyConfigured,
    MissingLocalDirectory,
}

fn apply_working_dir_to_command(
    cmd: &mut CommandBuilder,
    working_dir: &str,
) -> WorkingDirectoryOutcome {
    let working_dir = working_dir.trim();
    if working_dir.is_empty() {
        return WorkingDirectoryOutcome::Skipped;
    }

    if command_uses_wsl(cmd) {
        if command_has_wsl_cd_arg(cmd) {
            return WorkingDirectoryOutcome::WslCdAlreadyConfigured;
        }

        let argv = cmd.get_argv_mut();
        argv.insert(1, working_dir.into());
        argv.insert(1, "--cd".into());
        return WorkingDirectoryOutcome::AppliedWslCd;
    }

    if Path::new(working_dir).is_dir() {
        cmd.cwd(working_dir);
        WorkingDirectoryOutcome::AppliedLocalCwd
    } else {
        WorkingDirectoryOutcome::MissingLocalDirectory
    }
}

fn command_uses_wsl(cmd: &CommandBuilder) -> bool {
    let Some(program) = cmd.get_argv().first() else {
        return false;
    };

    let program = program.to_string_lossy();
    let normalized = program.replace('\\', "/");
    let file_name = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_ascii_lowercase();

    matches!(file_name.as_str(), "wsl" | "wsl.exe")
}

fn command_has_wsl_cd_arg(cmd: &CommandBuilder) -> bool {
    cmd.get_argv()
        .iter()
        .skip(1)
        .any(|arg| {
            let arg = arg.to_string_lossy();
            arg == "--cd" || arg.starts_with("--cd=")
        })
}

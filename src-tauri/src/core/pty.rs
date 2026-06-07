//! Local PTY (pseudo-terminal) session creation and management.
//!
//! Spawns the user's shell (PowerShell on Windows, $SHELL elsewhere) and bridges I/O to Tauri.

use super::recording::RecordingManager;
use super::session::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
use super::update_cwd_if_changed;
use super::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemEvent, ZmodemTransfer,
};
use crate::config::AiExecutionProfile;
use crate::core::SessionOutputCoalescer;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::ssh::osc::{self, OscStripper, ShellKind};
use crate::error::AppResult;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

/// Per-connection local terminal config.
pub struct LocalSessionConfig {
    pub shell_path: String,
    pub shell_args: String,
    pub working_dir: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellCommandSpec {
    program: String,
    args: Vec<String>,
}

fn build_shell_command(
    shell_path: &str,
    shell_args: &str,
) -> Result<(CommandBuilder, String), String> {
    let spec = resolve_shell_command(shell_path, shell_args)?;
    let mut builder = CommandBuilder::new(&spec.program);
    if !spec.args.is_empty() {
        builder.args(spec.args.iter().map(String::as_str));
    }
    Ok((builder, spec.program))
}

fn default_local_shell_args(program: &str) -> Vec<String> {
    if cfg!(windows) {
        return vec![];
    }

    let shell_name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();

    match shell_name.as_str() {
        "bash" | "zsh" | "fish" => vec!["--login".to_string(), "-i".to_string()],
        _ => vec![],
    }
}

fn resolve_shell_command(shell_path: &str, shell_args: &str) -> Result<ShellCommandSpec, String> {
    let raw_program = shell_path.trim();
    let program = trim_wrapping_quotes(raw_program);
    if program.is_empty() {
        let (_, shell_name) = platform_default_shell();
        let args = parse_shell_args(shell_args)?;
        return Ok(ShellCommandSpec {
            args: if args.is_empty() {
                default_local_shell_args(&shell_name)
            } else {
                args
            },
            program: shell_name,
        });
    }

    let args = parse_shell_args(shell_args)?;
    if !args.is_empty() {
        return Ok(ShellCommandSpec {
            program: program.to_string(),
            args,
        });
    }

    if should_treat_as_literal_program(raw_program) {
        return Ok(ShellCommandSpec {
            program: program.to_string(),
            args: default_local_shell_args(program),
        });
    }

    let mut legacy_parts = parse_shell_args(program)?;
    if legacy_parts.is_empty() {
        return Err("Shell path is required".to_string());
    }
    let legacy_program = legacy_parts.remove(0);
    Ok(ShellCommandSpec {
        program: legacy_program,
        args: legacy_parts,
    })
}

fn should_treat_as_literal_program(value: &str) -> bool {
    !value.chars().any(char::is_whitespace)
        || path_exists(value)
        || looks_like_path(value)
        || is_quoted(value)
}

fn path_exists(value: &str) -> bool {
    Path::new(trim_wrapping_quotes(value)).exists()
}

fn looks_like_path(value: &str) -> bool {
    value.contains('\\') || value.contains('/') || Path::new(value).is_absolute()
}

fn is_quoted(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
}

fn trim_wrapping_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if is_quoted(trimmed) {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

pub(crate) fn parse_shell_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.trim().chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            let escapes_next = chars.peek().is_some_and(|next| match quote {
                Some(active_quote) => *next == active_quote || *next == '\\',
                None => next.is_whitespace() || *next == '"' || *next == '\'' || *next == '\\',
            });
            if escapes_next {
                escaped = true;
            } else {
                current.push(ch);
            }
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }

        if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
            while chars.peek().is_some_and(|next| next.is_whitespace()) {
                let _ = chars.next();
            }
            continue;
        }

        current.push(ch);
    }

    if escaped {
        current.push('\\');
    }
    if quote.is_some() {
        return Err("Unclosed quote in shell arguments".to_string());
    }
    if !current.is_empty() {
        args.push(current);
    }

    Ok(args)
}

fn platform_default_shell() -> (CommandBuilder, String) {
    #[cfg(target_os = "windows")]
    {
        (
            CommandBuilder::new("powershell.exe"),
            "powershell.exe".to_string(),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut builder = CommandBuilder::new(&shell);
        let default_args = default_local_shell_args(&shell);
        if !default_args.is_empty() {
            builder.args(default_args.iter().map(String::as_str));
        }
        (builder, shell)
    }
}

#[cfg(target_os = "macos")]
fn ensure_macos_interactive_path(cmd: &mut CommandBuilder) {
    let current_path = std::env::var("PATH").unwrap_or_default();
    let mut paths: Vec<String> = current_path
        .split(':')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();

    for path in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
        if !paths.iter().any(|existing| existing == path) && Path::new(path).exists() {
            paths.push(path.to_string());
        }
    }

    if !paths.is_empty() {
        cmd.env("PATH", paths.join(":"));
    }
}

#[cfg(target_os = "macos")]
fn configure_local_pty_environment(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    set_utf8_env_if_missing_or_non_utf8(cmd, "LANG", "en_US.UTF-8");
    set_utf8_env_if_missing_or_non_utf8(cmd, "LC_CTYPE", "UTF-8");
}

#[cfg(target_os = "macos")]
fn set_utf8_env_if_missing_or_non_utf8(cmd: &mut CommandBuilder, key: &str, default_value: &str) {
    let value = std::env::var(key)
        .ok()
        .filter(|value| is_utf8_locale(value))
        .unwrap_or_else(|| default_value.to_string());
    cmd.env(key, value);
}

#[cfg(target_os = "macos")]
fn is_utf8_locale(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace('_', "-");
    normalized.contains("utf-8") || normalized.contains("utf8")
}

fn infer_local_ai_execution_profile(shell_name: &str) -> AiExecutionProfile {
    let shell = shell_name.to_ascii_lowercase();
    if shell.contains("powershell") || shell.contains("pwsh") {
        AiExecutionProfile::Powershell
    } else if shell.contains("cmd") {
        AiExecutionProfile::Cmd
    } else if shell.contains("bash")
        || shell.contains("zsh")
        || shell.contains("fish")
        || shell.contains("wsl")
        || shell.ends_with("sh")
        || shell.contains("/sh")
        || shell.contains("\\sh")
    {
        AiExecutionProfile::Posix
    } else {
        AiExecutionProfile::SendOnly
    }
}

struct LocalStartupScript {
    script: Option<String>,
    shell_integration_active: bool,
}

fn build_local_startup_script(shell_name: &str, ready_marker: &str) -> LocalStartupScript {
    build_local_startup_script_for_platform(
        shell_name,
        ready_marker,
        cfg!(not(target_os = "windows")),
    )
}

fn build_local_startup_script_for_platform(
    shell_name: &str,
    ready_marker: &str,
    allow_unix_prelude: bool,
) -> LocalStartupScript {
    let shell_kind = ShellKind::from_name(shell_name);
    let shell_integration_script = osc::injection_script(shell_kind, ready_marker);
    let shell_integration_active = shell_integration_script.is_some();
    let backspace_prelude =
        local_backspace_compat_prelude(shell_name, shell_kind, allow_unix_prelude);

    let script = match (backspace_prelude, shell_integration_script) {
        (Some(mut prelude), Some(integration)) => {
            prelude.push_str(&integration);
            Some(prelude)
        }
        (Some(mut prelude), None) => {
            prelude.push_str(&build_ready_marker_printf(ready_marker));
            Some(prelude)
        }
        (None, integration) => integration,
    };

    LocalStartupScript {
        script,
        shell_integration_active,
    }
}

fn local_backspace_compat_prelude(
    shell_name: &str,
    shell_kind: ShellKind,
    allow_unix_prelude: bool,
) -> Option<String> {
    if !allow_unix_prelude || is_windows_style_shell(shell_name) {
        return None;
    }

    match shell_kind {
        ShellKind::Bash => Some(
            concat!(
                "stty erase '^?' 2>/dev/null || true\n",
                "if [ -z \"${NYATERM_BASHRC_SOURCED:-}\" ] && [ -r \"$HOME/.bashrc\" ]; then\n",
                "  export NYATERM_BASHRC_SOURCED=1\n",
                "  . \"$HOME/.bashrc\"\n",
                "fi\n",
            )
            .to_string(),
        ),
        ShellKind::Fish | ShellKind::PosixSh => {
            Some("stty erase '^?' 2>/dev/null || true\n".to_string())
        }
        ShellKind::Zsh => Some("stty erase '^?' 2>/dev/null || true\n".to_string()),
        ShellKind::Unknown => None,
    }
}

fn is_windows_style_shell(shell_name: &str) -> bool {
    let lower = shell_name.to_ascii_lowercase();
    let program = lower
        .rsplit(|ch| ch == '/' || ch == '\\')
        .next()
        .unwrap_or(lower.as_str());

    matches!(
        program,
        "cmd" | "cmd.exe" | "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    )
}

fn build_ready_marker_printf(ready_marker: &str) -> String {
    let ready_osc = ready_marker
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007");
    format!("printf '{}' 2>/dev/null\n", ready_osc)
}

fn should_emit_visible_output(suppress_visible: &mut bool, ready: bool) -> bool {
    if !*suppress_visible {
        return true;
    }

    if !ready {
        return false;
    }

    *suppress_visible = false;
    true
}

fn write_to_pty(writer: &mut dyn Write, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}

/// Spawns a local shell in a PTY and registers the session with the manager.
/// If `config` is provided, uses the shell path and working dir from it.
pub async fn create_local_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
    config: Option<LocalSessionConfig>,
    owner_window_label: Option<String>,
) -> AppResult<String> {
    tracing::info!("Creating local PTY session");
    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let session_name = config
        .as_ref()
        .map_or("Local Terminal".to_string(), |c| c.name.clone());

    let (_, shell_name) = match &config {
        Some(cfg) if !cfg.shell_path.trim().is_empty() => {
            build_shell_command(&cfg.shell_path, &cfg.shell_args)
                .map_err(crate::error::AppError::Config)?
        }
        _ => platform_default_shell(),
    };
    let ai_execution_profile = infer_local_ai_execution_profile(&shell_name);
    let ready_marker = osc::build_ready_marker(&session_id);
    let startup_script = build_local_startup_script(&shell_name, &ready_marker);
    let injection_active = startup_script.shell_integration_active;

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: session_name,
        session_type: SessionType::Local,
        connected: true,
        owner_window_label,
        ai_execution_profile,
        injection_active,
    };

    let cwd: SharedCwd = Arc::new(tokio::sync::Mutex::new(None));
    let session_handle = SessionHandle {
        info: session_info,
        cmd_tx,
        ssh_config: None,
        ssh_handle: None,
        cwd: cwd.clone(),
        remote_fs: None,
    };
    manager.add_session(session_handle).await;

    let sid = session_id.clone();
    let mgr = manager.clone();
    let rt_handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        pty_session_thread(
            app,
            sid,
            mgr,
            cmd_rx,
            rt_handle,
            cwd,
            config,
            startup_script.script,
            ready_marker,
        );
    });

    Ok(session_id)
}

fn pty_session_thread(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    rt_handle: tokio::runtime::Handle,
    cwd: SharedCwd,
    config: Option<LocalSessionConfig>,
    startup_script: Option<String>,
    ready_marker: String,
) {
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

    let (mut cmd, _) = match &config {
        Some(cfg) if !cfg.shell_path.trim().is_empty() => {
            match build_shell_command(&cfg.shell_path, &cfg.shell_args) {
                Ok(command) => command,
                Err(error) => {
                    tracing::error!("Failed to build shell command: {}", error);
                    let _ = app.emit(
                        &format!("session-error-{}", session_id),
                        format!("Failed to build shell command: {}", error),
                    );
                    let _ = app.emit(&format!("session-closed-{}", session_id), ());
                    rt_handle.block_on(async {
                        manager.remove_session(&session_id).await;
                    });
                    return;
                }
            }
        }
        _ => platform_default_shell(),
    };

    if let Some(ref cfg) = config {
        if let Some(ref dir) = cfg.working_dir {
            if !dir.is_empty() {
                cmd.cwd(dir);
            }
        }
    }

    #[cfg(target_os = "macos")]
    ensure_macos_interactive_path(&mut cmd);
    #[cfg(target_os = "macos")]
    configure_local_pty_environment(&mut cmd);

    let mut _child = match pair.slave.spawn_command(cmd) {
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
    let output = SessionOutputCoalescer::for_app(app.clone(), output_event.clone());

    let capture_processor = Arc::new(StdMutex::new(OutputCaptureProcessor::new()));
    let capture_for_reader = capture_processor.clone();

    let zmodem_state: Arc<StdMutex<Option<ZmodemTransfer>>> = Arc::new(StdMutex::new(None));
    let zmodem_state_reader = zmodem_state.clone();
    let zmodem_event_name = format!("zmodem-event-{session_id}");
    let zmodem_event_reader = zmodem_event_name.clone();
    let (zmodem_out_tx, mut zmodem_out_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let app_read = app.clone();
    let sid_read = session_id.clone();
    let cwd_event = format!("cwd-changed-{}", session_id);
    let rt_for_reader = rt_handle.clone();
    let recording_mgr_reader: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|s| s.inner().clone());
    let sid_for_rec_reader = session_id.clone();
    let output_reader = output.clone();
    let manager_reader = manager.clone();
    let suppress_startup_output = startup_script.is_some();
    std::thread::spawn(move || {
        let mut raw_buf = [0u8; 4096];
        let mut stripper = OscStripper::new(&ready_marker);
        let mut suppress_visible = suppress_startup_output;
        let mut zmodem_detector = ZmodemDetector::new();
        loop {
            match reader.read(&mut raw_buf) {
                Ok(0) => break,
                Ok(n) => {
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
                                zmodem_detector.reset();
                            }
                            continue;
                        }
                    }

                    // ZMODEM: detect header in raw bytes.
                    let process_raw = if !suppress_visible {
                        match zmodem_detector.feed(raw) {
                            ZmodemDetectResult::Detected {
                                direction,
                                passthrough,
                                initial_bytes,
                            } => {
                                if !passthrough.is_empty() {
                                    let pre = String::from_utf8_lossy(&passthrough).to_string();
                                    if !pre.is_empty() {
                                        output_reader.push_owned(pre);
                                    }
                                }
                                let transfer = ZmodemTransfer::new(direction, &initial_bytes);
                                *zmodem_state_reader.lock().unwrap() = Some(transfer);
                                let _ = app_read.emit(
                                    &zmodem_event_reader,
                                    &ZmodemEvent::Detected { direction },
                                );
                                continue;
                            }
                            ZmodemDetectResult::NoMatch { passthrough } => {
                                if passthrough.is_empty() {
                                    continue;
                                }
                                passthrough
                            }
                        }
                    } else {
                        raw.to_vec()
                    };

                    let text = String::from_utf8_lossy(&process_raw).to_string();
                    let mut result = stripper.push(&text);

                    for path in &result.cwd_paths {
                        let cwd_ev = cwd_event.clone();
                        let app_ref = app_read.clone();
                        let next_cwd = rt_for_reader
                            .block_on(async { update_cwd_if_changed(&cwd, path).await });
                        if let Some(next_cwd) = next_cwd {
                            let _ = app_ref.emit(&cwd_ev, &next_cwd);
                        }
                    }

                    for command in &result.accepted_commands {
                        rt_for_reader.block_on(
                            manager_reader
                                .confirm_command_submission(&sid_for_rec_reader, command.clone()),
                        );
                    }

                    if !should_emit_visible_output(&mut suppress_visible, result.ready) {
                        continue;
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
        output_reader.close();
        let _ = app_read.emit(&format!("session-closed-{}", sid_read), ());
    });

    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|s| s.inner().clone());
    if let Some(script) = startup_script.as_deref() {
        if let Err(error) = write_to_pty(&mut *writer, script.as_bytes()) {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "Failed to write local PTY startup script"
            );
        }
    }
    loop {
        // Drain any ZMODEM outgoing data first (non-blocking).
        while let Ok(data) = zmodem_out_rx.try_recv() {
            let _ = write_to_pty(&mut *writer, &data);
        }

        let cmd = match cmd_rx.blocking_recv() {
            Some(c) => c,
            None => break,
        };
        match cmd {
            SessionCommand::Attach => {
                output.attach();
            }
            SessionCommand::Write(data) => {
                if zmodem_state.lock().unwrap().is_some() {
                    continue;
                }
                if let Some(ref rec) = recording_mgr {
                    rec.write_input(&session_id, &data);
                }
                if let Err(error) = write_to_pty(&mut *writer, &data) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "Failed to write to local PTY"
                    );
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
                if let Err(error) = write_to_pty(&mut *writer, &wrapped_command) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "Failed to write capture command to local PTY"
                    );
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
                                let _ = write_to_pty(&mut *writer, &data);
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
                                let _ = write_to_pty(&mut *writer, &data);
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

    output.close();

    if let Some(ref rec) = recording_mgr {
        rec.cleanup_session(&session_id);
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&format!("session-closed-{}", session_id), ());
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::configure_local_pty_environment;
    #[cfg(target_os = "macos")]
    use super::is_utf8_locale;
    use super::{
        build_local_startup_script_for_platform, parse_shell_args, resolve_shell_command,
        should_emit_visible_output,
    };
    #[cfg(target_os = "macos")]
    use portable_pty::CommandBuilder;

    fn ready_marker() -> String {
        crate::core::ssh::osc::build_ready_marker("session-1")
    }

    #[test]
    fn shell_path_with_spaces_stays_single_program() {
        let spec =
            resolve_shell_command(r"D:\Soft wares\Git\bin\bash.exe", "").expect("command spec");

        assert_eq!(spec.program, r"D:\Soft wares\Git\bin\bash.exe");
        assert!(spec.args.is_empty());
    }

    #[test]
    fn shell_args_are_split_separately_from_program() {
        let spec = resolve_shell_command("pwsh.exe", "-NoLogo -NoExit").expect("command spec");

        assert_eq!(spec.program, "pwsh.exe");
        assert_eq!(spec.args, vec!["-NoLogo", "-NoExit"]);
    }

    #[test]
    fn unix_bash_defaults_to_login_interactive_when_args_are_empty() {
        let spec = resolve_shell_command("/bin/bash", "").expect("command spec");

        assert_eq!(spec.program, "/bin/bash");
        if cfg!(windows) {
            assert!(spec.args.is_empty());
        } else {
            assert_eq!(spec.args, vec!["--login", "-i"]);
        }
    }

    #[test]
    fn explicit_shell_args_override_unix_interactive_defaults() {
        let spec = resolve_shell_command("/bin/bash", "--noprofile --norc").expect("command spec");

        assert_eq!(spec.program, "/bin/bash");
        assert_eq!(spec.args, vec!["--noprofile", "--norc"]);
    }

    #[test]
    fn legacy_shell_path_command_with_args_is_still_supported() {
        let spec = resolve_shell_command("pwsh.exe -NoLogo", "").expect("command spec");

        assert_eq!(spec.program, "pwsh.exe");
        assert_eq!(spec.args, vec!["-NoLogo"]);
    }

    #[test]
    fn shell_args_support_quotes_and_windows_paths() {
        let args = parse_shell_args(r#"-Command "echo hi" "C:\Program Files\Tool""#).expect("args");

        assert_eq!(args, vec!["-Command", "echo hi", r"C:\Program Files\Tool"]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn utf8_locale_detection_accepts_common_spellings() {
        assert!(is_utf8_locale("en_US.UTF-8"));
        assert!(is_utf8_locale("zh_CN.utf8"));
        assert!(!is_utf8_locale("C"));
        assert!(!is_utf8_locale("zh_CN.GBK"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn local_pty_environment_sets_terminal_and_utf8_locale() {
        let mut cmd = CommandBuilder::new("/bin/zsh");
        configure_local_pty_environment(&mut cmd);

        assert_eq!(
            cmd.get_env("TERM").and_then(|value| value.to_str()),
            Some("xterm-256color")
        );
        assert!(
            cmd.get_env("LANG")
                .and_then(|value| value.to_str())
                .is_some_and(is_utf8_locale)
        );
        assert!(
            cmd.get_env("LC_CTYPE")
                .and_then(|value| value.to_str())
                .is_some_and(is_utf8_locale)
        );
    }

    #[test]
    fn startup_suppression_hides_chunks_until_ready_marker() {
        let mut suppress_visible = true;

        assert!(!should_emit_visible_output(&mut suppress_visible, false));
        assert!(suppress_visible);
    }

    #[test]
    fn startup_suppression_emits_visible_text_from_ready_chunk() {
        let mut suppress_visible = true;

        assert!(should_emit_visible_output(&mut suppress_visible, true));
        assert!(!suppress_visible);
    }

    #[test]
    fn startup_suppression_emits_subsequent_chunks_after_ready() {
        let mut suppress_visible = true;

        assert!(should_emit_visible_output(&mut suppress_visible, true));
        assert!(should_emit_visible_output(&mut suppress_visible, false));
        assert!(!suppress_visible);
    }

    #[test]
    fn zsh_startup_sets_del_erase_without_overriding_bindkeys() {
        let marker = ready_marker();
        let startup = build_local_startup_script_for_platform("/bin/zsh", &marker, true);
        let script = startup.script.expect("startup script");

        assert!(startup.shell_integration_active);
        assert!(script.contains("stty erase '^?' 2>/dev/null || true"));
        assert!(!script.contains("bindkey"));

        let stty_pos = script.find("stty erase '^?'").expect("stty prelude");
        let ready_pos = script.find("NyaTermReady:session-1").expect("ready marker");

        assert!(stty_pos < ready_pos);
    }

    #[test]
    fn bash_and_fish_startup_set_del_erase_without_zsh_bindkeys() {
        let marker = ready_marker();

        for shell in ["/bin/bash", "/usr/local/bin/fish"] {
            let startup = build_local_startup_script_for_platform(shell, &marker, true);
            let script = startup.script.expect("startup script");

            assert!(startup.shell_integration_active);
            assert!(script.contains("stty erase '^?' 2>/dev/null || true"));
            assert!(!script.contains("bindkey -M emacs"));
            assert!(!script.contains("bindkey -M viins"));
            assert!(script.contains("NyaTermReady:session-1"));
        }
    }

    #[test]
    fn bash_startup_sources_user_bashrc_before_prompt_hook() {
        let marker = ready_marker();
        let startup = build_local_startup_script_for_platform("/bin/bash", &marker, true);
        let script = startup.script.expect("startup script");

        assert!(script.contains("NYATERM_BASHRC_SOURCED"));
        assert!(script.contains(". \"$HOME/.bashrc\""));

        let stty_pos = script.find("stty erase '^?'").expect("stty prelude");
        let bashrc_pos = script.find(". \"$HOME/.bashrc\"").expect("bashrc source");
        let prompt_hook_pos = script.find("PROMPT_COMMAND").expect("prompt hook");

        assert!(stty_pos < bashrc_pos);
        assert!(bashrc_pos < prompt_hook_pos);
    }

    #[test]
    fn posix_startup_sets_del_erase_and_ready_marker_without_shell_integration() {
        let marker = ready_marker();
        let startup = build_local_startup_script_for_platform("/bin/sh", &marker, true);
        let script = startup.script.expect("startup script");

        assert!(!startup.shell_integration_active);
        assert!(script.contains("stty erase '^?' 2>/dev/null || true"));
        assert!(!script.contains("bindkey"));
        assert!(script.contains("NyaTermReady:session-1"));
    }

    #[test]
    fn unknown_and_windows_style_shells_do_not_get_unix_backspace_prelude() {
        let marker = ready_marker();

        for shell in ["nu", "powershell.exe", r"C:\Windows\System32\cmd.exe"] {
            let startup = build_local_startup_script_for_platform(shell, &marker, true);

            assert!(!startup.shell_integration_active);
            assert!(
                startup.script.is_none(),
                "{shell} should not receive a startup script"
            );
        }
    }

    #[test]
    fn unix_backspace_prelude_is_disabled_on_non_unix_platforms() {
        let marker = ready_marker();
        let startup = build_local_startup_script_for_platform("/bin/zsh", &marker, false);
        let script = startup.script.expect("zsh integration script");

        assert!(startup.shell_integration_active);
        assert!(!script.contains("stty erase '^?'"));
        assert!(!script.contains("bindkey -M emacs '^?'"));
        assert!(script.contains("NyaTermReady:session-1"));
    }
}

//! Local PTY (pseudo-terminal) session creation and management.
//!
//! Spawns the user's shell (PowerShell on Windows, $SHELL elsewhere) and bridges I/O to Tauri.

use crate::error::AppResult;
use crate::session::{SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

struct OutputBuffer {
    attached: bool,
    buffer: Vec<String>,
}

fn get_default_shell(app: Option<&AppHandle>) -> CommandBuilder {
    let mut shell_cmd = String::new();

    if let Some(app) = app {
        if let Ok(settings) = crate::config::load_app_settings(app) {
            let user_shell = settings.general.default_local_shell;
            if !user_shell.trim().is_empty() {
                shell_cmd = user_shell;
            }
        }
    }

    if shell_cmd.is_empty() {
        #[cfg(target_os = "windows")]
        {
            shell_cmd = "powershell.exe".to_string();
        }
        #[cfg(not(target_os = "windows"))]
        {
            shell_cmd = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        }
    }

    let parts: Vec<&str> = shell_cmd.split_whitespace().collect();
    if parts.is_empty() {
        #[cfg(target_os = "windows")]
        return CommandBuilder::new("powershell.exe");
        #[cfg(not(target_os = "windows"))]
        return CommandBuilder::new("/bin/bash");
    } else {
        let mut builder = CommandBuilder::new(parts[0]);
        if parts.len() > 1 {
            builder.args(&parts[1..]);
        }
        builder
    }
}

/// Spawns a local shell in a PTY and registers the session with the manager.
pub async fn create_local_session(
    app: AppHandle,
    manager: Arc<SessionManager>,
) -> AppResult<String> {
    tracing::info!("Creating local PTY session");
    let session_id = uuid::Uuid::new_v4().to_string();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();

    let session_info = SessionInfo {
        id: session_id.clone(),
        name: "Local Terminal".to_string(),
        session_type: SessionType::Local,
        connected: true,
    };

    let session_handle = SessionHandle {
        info: session_info,
        cmd_tx,
        ssh_config: None,
        ssh_handle: None,
    };
    manager.add_session(session_handle).await;

    let sid = session_id.clone();
    let mgr = manager.clone();
    let rt_handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        pty_session_thread(app, sid, mgr, cmd_rx, rt_handle);
    });

    Ok(session_id)
}

fn pty_session_thread(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    rt_handle: tokio::runtime::Handle,
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
            return;
        }
    };

    let cmd = get_default_shell(Some(&app));
    let mut _child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to spawn shell: {}", e);
            let _ = app.emit(
                &format!("session-error-{}", session_id),
                format!("Failed to spawn shell: {}", e),
            );
            return;
        }
    };
    drop(pair.slave);

    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Failed to take PTY writer: {}", e);
            return;
        }
    };
    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to clone PTY reader: {}", e);
            return;
        }
    };
    let master = pair.master;

    let output_buf = Arc::new(Mutex::new(OutputBuffer {
        attached: false,
        buffer: Vec::new(),
    }));

    let app_read = app.clone();
    let sid_read = session_id.clone();
    let output_event = format!("terminal-output-{}", session_id);
    let buf_reader = output_buf.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let emit_now = {
                        let mut ob = buf_reader.lock().unwrap();
                        if ob.attached {
                            true
                        } else {
                            ob.buffer.push(text.clone());
                            false
                        }
                    };
                    if emit_now {
                        let _ = app_read.emit(&output_event, &text);
                    }
                }
                Err(_) => break,
            }
        }
        let _ = app_read.emit(&format!("session-closed-{}", sid_read), ());
    });

    let output_event_cmd = format!("terminal-output-{}", session_id);
    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            SessionCommand::Attach => {
                let buffered = {
                    let mut ob = output_buf.lock().unwrap();
                    ob.attached = true;
                    ob.buffer.drain(..).collect::<Vec<_>>()
                };
                for text in buffered {
                    let _ = app.emit(&output_event_cmd, &text);
                }
            }
            SessionCommand::Write(data) => {
                let _ = writer.write_all(&data);
                let _ = writer.flush();
            }
            SessionCommand::Resize { cols, rows } => {
                let _ = master.resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            SessionCommand::Close => {
                break;
            }
        }
    }

    rt_handle.block_on(async {
        manager.remove_session(&session_id).await;
    });
    let _ = app.emit(&format!("session-closed-{}", session_id), ());
}

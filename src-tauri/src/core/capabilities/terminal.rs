use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::config::AiExecutionProfile;
use crate::core::ai::AiCaptureEvent;
use crate::core::capture;
use crate::core::session::{SessionCommand, SessionManager};
use crate::core::{InputOrigin, InputSensitivity};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct TerminalExecuteRequest {
    pub session_id: String,
    pub command: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExecuteResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub source_truncated: bool,
}

#[derive(Clone)]
pub struct TerminalExecutionPresentation {
    pub app: AppHandle,
    pub step_index: u16,
    pub max_lines: u16,
    pub send_only_output: Option<String>,
    pub disabled_error: Option<String>,
    pub source: Option<String>,
}

struct CaptureGuard {
    manager: Arc<SessionManager>,
    session_id: String,
    marker_id: String,
    finished: bool,
    presentation: Option<TerminalExecutionPresentation>,
    end_sent: Arc<AtomicBool>,
    started: Instant,
}

impl CaptureGuard {
    async fn cancel(&self) {
        let _ = self
            .manager
            .send_command(
                &self.session_id,
                SessionCommand::CancelCapture {
                    marker_id: self.marker_id.clone(),
                },
            )
            .await;
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if !self.end_sent.swap(true, Ordering::SeqCst) {
            emit_error(
                self.presentation.as_ref(),
                &self.session_id,
                &AppError::Cancelled("Terminal command was cancelled.".into()),
                self.started.elapsed(),
            );
        }
        let manager = self.manager.clone();
        let session_id = self.session_id.clone();
        let marker_id = self.marker_id.clone();
        tokio::spawn(async move {
            let _ = manager
                .send_command(&session_id, SessionCommand::CancelCapture { marker_id })
                .await;
        });
    }
}

pub async fn execute_terminal_command(
    manager: Arc<SessionManager>,
    request: TerminalExecuteRequest,
    presentation: Option<TerminalExecutionPresentation>,
    cancellation: CancellationToken,
) -> AppResult<TerminalExecuteResult> {
    if request.command.trim().is_empty() {
        return Err(AppError::Config(
            "Terminal command must not be empty.".to_string(),
        ));
    }
    let info = manager.session_info(&request.session_id).await?;
    if info.ai_execution_profile == AiExecutionProfile::Disabled {
        return Err(AppError::Config(
            presentation
                .as_ref()
                .and_then(|value| value.disabled_error.clone())
                .unwrap_or_else(|| {
                    "Terminal command execution is disabled for this session.".to_string()
                }),
        ));
    }
    if matches!(
        info.ai_execution_profile,
        AiExecutionProfile::Auto | AiExecutionProfile::SendOnly
    ) {
        if !matches!(
            manager.terminal_execution_state(&request.session_id),
            crate::core::session::TerminalExecutionState::Idle
        ) {
            return Err(AppError::SessionBusy(
                "This session is executing an automated command.".into(),
            ));
        }
        emit_start(presentation.as_ref(), &request.session_id, &request.command);
        let started = Instant::now();
        let mut data = request.command.as_bytes().to_vec();
        data.push(b'\n');
        tokio::select! {
            _ = cancellation.cancelled() => {
                let error = AppError::Cancelled("Terminal command was cancelled.".to_string());
                emit_error(presentation.as_ref(), &request.session_id, &error, started.elapsed());
                return Err(error);
            }
            result = manager.send_command(
                &request.session_id,
                SessionCommand::Write {
                    data,
                    raw: false,
                    automated: true,
                    origin: InputOrigin::AiAgent,
                    sensitivity: InputSensitivity::Normal,
                },
            ) => if let Err(error) = result {
                emit_error(presentation.as_ref(), &request.session_id, &error, started.elapsed());
                return Err(error);
            }
        }
        let result = TerminalExecuteResult {
            output: presentation
                .as_ref()
                .and_then(|value| value.send_only_output.clone())
                .unwrap_or_else(|| "Command sent to a send-only terminal; captured output and exit status are unavailable.".to_string()),
            exit_code: None,
            duration_ms: started.elapsed().as_millis() as u64,
            timed_out: false,
            source_truncated: false,
        };
        emit_end(presentation.as_ref(), &request.session_id, &result);
        return Ok(result);
    }

    let marker_id = if info.ai_execution_profile == AiExecutionProfile::Powershell {
        uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    let mut execution = manager
        .begin_terminal_execution(&request.session_id, &marker_id)
        .await?;
    let wrapped = if info.ai_execution_profile == AiExecutionProfile::Powershell
        && info.session_type == crate::core::session::SessionType::Local
    {
        let dispatch = manager.powershell_dispatch.lock().unwrap().get(&request.session_id).cloned()
            .ok_or_else(|| AppError::Unsupported("PowerShell capture is unavailable for this custom shell startup. Open a PowerShell session with default arguments or use send-only execution.".into()))?;
        if cfg!(windows) && !info.dynamic_title_capabilities.integration_active {
            return Err(AppError::SessionBusy("PowerShell is still initializing its interactive prompt. Retry after the prompt is ready.".into()));
        }
        let (wrapped, command_file) = dispatch.prepare(&marker_id, &request.command)?;
        execution.command_file = Some(command_file);
        wrapped
    } else {
        capture::build_capture_command(info.ai_execution_profile, &marker_id, &request.command)
            .ok_or_else(|| {
                AppError::Config("Terminal execution profile does not support capture.".to_string())
            })?
    };
    let end_sent = Arc::new(AtomicBool::new(false));
    let completion_sent = end_sent.clone();
    let completion_presentation = presentation.clone();
    let completion_manager = manager.clone();
    let completion_session = request.session_id.clone();
    execution.on_complete(move |captured| {
        let result = TerminalExecuteResult {
            output: strip_ansi_escapes::strip_str(&captured.output),
            exit_code: captured.exit_code,
            duration_ms: captured.duration_ms,
            timed_out: false,
            source_truncated: captured.source_truncated,
        };
        completion_manager.append_recent_output(&completion_session, &result.output);
        if !completion_sent.swap(true, Ordering::SeqCst) {
            emit_end(
                completion_presentation.as_ref(),
                &completion_session,
                &result,
            );
        }
    });
    emit_start(presentation.as_ref(), &request.session_id, &request.command);
    let (tx, rx) = oneshot::channel();
    let mut guard = CaptureGuard {
        manager: manager.clone(),
        session_id: request.session_id.clone(),
        marker_id: marker_id.clone(),
        finished: false,
        presentation: presentation.clone(),
        end_sent: end_sent.clone(),
        started: Instant::now(),
    };
    let started = Instant::now();
    tokio::select! {
        _ = cancellation.cancelled() => {
            let error = AppError::Cancelled("Terminal command was cancelled.".to_string());
            return Err(error);
        }
        result = manager.send_command(
            &request.session_id,
            SessionCommand::CaptureExec {
                marker_id,
                wrapped_command: wrapped.into_bytes(),
                result_tx: tx,
                execution,
            },
        ) => if let Err(error) = result {
            if !end_sent.swap(true, Ordering::SeqCst) {
                emit_error(presentation.as_ref(), &request.session_id, &error, started.elapsed());
            }
            return Err(error);
        }
    }

    let timeout = tokio::time::sleep(Duration::from_millis(request.timeout_ms));
    tokio::pin!(timeout);
    let result = tokio::select! {
        _ = cancellation.cancelled() => {
            guard.cancel().await;
            Err(AppError::Cancelled("Terminal command was cancelled.".to_string()))
        }
        _ = &mut timeout => {
            guard.cancel().await;
            Ok(TerminalExecuteResult { output: "(command timed out — the shell may still be running; wait or press Ctrl+C)".to_string(), exit_code: None, duration_ms: request.timeout_ms, timed_out: true, source_truncated: false })
        }
        captured = rx => match captured {
            Ok(captured) => Ok(TerminalExecuteResult { output: strip_ansi_escapes::strip_str(&captured.output), exit_code: captured.exit_code, duration_ms: captured.duration_ms, timed_out: false, source_truncated: captured.source_truncated }),
            Err(_) => {
                guard.cancel().await;
                Err(AppError::Channel("Capture channel closed — session may have disconnected".to_string()))
            },
        }
    };
    guard.finished = true;
    match &result {
        Ok(value) => {
            if !end_sent.swap(true, Ordering::SeqCst) {
                manager.append_recent_output(&request.session_id, &value.output);
                emit_end(presentation.as_ref(), &request.session_id, value);
            }
        }
        Err(error) => {
            if !end_sent.swap(true, Ordering::SeqCst) {
                emit_error(
                    presentation.as_ref(),
                    &request.session_id,
                    error,
                    started.elapsed(),
                );
            }
        }
    }
    result
}

fn emit_error(
    presentation: Option<&TerminalExecutionPresentation>,
    session_id: &str,
    error: &AppError,
    duration: Duration,
) {
    emit_end(
        presentation,
        session_id,
        &TerminalExecuteResult {
            output: error.to_string(),
            exit_code: None,
            duration_ms: duration.as_millis() as u64,
            timed_out: false,
            source_truncated: false,
        },
    );
}

fn emit_start(
    presentation: Option<&TerminalExecutionPresentation>,
    session_id: &str,
    command: &str,
) {
    if let Some(presentation) = presentation {
        let _ = presentation.app.emit(
            &format!("ai-capture-{session_id}"),
            AiCaptureEvent::CommandStart {
                command: command.to_string(),
                step_index: presentation.step_index,
                source: presentation.source.clone(),
            },
        );
    }
}

fn emit_end(
    presentation: Option<&TerminalExecutionPresentation>,
    session_id: &str,
    result: &TerminalExecuteResult,
) {
    if let Some(presentation) = presentation {
        let lines = result.output.lines().collect::<Vec<_>>();
        let truncated = lines.len() > presentation.max_lines as usize;
        let output = if truncated {
            lines[..presentation.max_lines as usize].join("\n")
        } else {
            result.output.clone()
        };
        let _ = presentation.app.emit(
            &format!("ai-capture-{session_id}"),
            AiCaptureEvent::CommandEnd {
                output,
                exit_code: result.exit_code,
                duration_ms: result.duration_ms,
                truncated,
            },
        );
    }
}

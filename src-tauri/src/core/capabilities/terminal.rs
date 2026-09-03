use std::sync::Arc;
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
}

struct CaptureGuard {
    manager: Arc<SessionManager>,
    session_id: String,
    marker_id: String,
    finished: bool,
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
    emit_start(presentation.as_ref(), &request.session_id, &request.command);
    if matches!(
        info.ai_execution_profile,
        AiExecutionProfile::Auto | AiExecutionProfile::SendOnly
    ) {
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

    let marker_id = uuid::Uuid::new_v4().to_string();
    let wrapped =
        capture::build_capture_command(info.ai_execution_profile, &marker_id, &request.command)
            .ok_or_else(|| {
                AppError::Config("Terminal execution profile does not support capture.".to_string())
            })?;
    let (tx, rx) = oneshot::channel();
    let mut guard = CaptureGuard {
        manager: manager.clone(),
        session_id: request.session_id.clone(),
        marker_id: marker_id.clone(),
        finished: false,
    };
    let started = Instant::now();
    tokio::select! {
        _ = cancellation.cancelled() => {
            let error = AppError::Cancelled("Terminal command was cancelled.".to_string());
            emit_error(presentation.as_ref(), &request.session_id, &error, started.elapsed());
            return Err(error);
        }
        result = manager.send_command(
            &request.session_id,
            SessionCommand::CaptureExec {
                marker_id,
                wrapped_command: wrapped.into_bytes(),
                result_tx: tx,
            },
        ) => if let Err(error) = result {
            emit_error(presentation.as_ref(), &request.session_id, &error, started.elapsed());
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
            Ok(TerminalExecuteResult { output: "(command timed out — markers not detected in PTY output)".to_string(), exit_code: None, duration_ms: request.timeout_ms, timed_out: true, source_truncated: false })
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
            manager.append_recent_output(&request.session_id, &value.output);
            emit_end(presentation.as_ref(), &request.session_id, value);
        }
        Err(error) => emit_error(
            presentation.as_ref(),
            &request.session_id,
            error,
            started.elapsed(),
        ),
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

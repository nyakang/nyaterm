mod parser;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::core::{InputOrigin, InputSensitivity, SessionCommand, SessionManager, SessionType};
use crate::error::{AppError, AppResult};

use self::parser::{Program, Statement, Value, WaitCondition, WaitMatcher};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOG_ENTRIES: usize = 100;
const MAX_LOG_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NyaScriptRunState {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NyaScriptRunStatus {
    pub run_id: String,
    pub state: NyaScriptRunState,
    pub current_line: Option<usize>,
    pub active_alias: Option<String>,
    pub session_id: Option<String>,
    pub logs: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NyaScriptRunEvent {
    run_id: String,
    event: String,
    state: NyaScriptRunState,
    current_line: Option<usize>,
    active_alias: Option<String>,
    session_id: Option<String>,
    log: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NyaScriptSessionOpenRequestEvent {
    pub request_id: String,
    pub target: String,
    pub target_window_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NyaScriptSessionOpenCancelEvent {
    pub request_id: String,
    pub target_window_label: String,
}

struct PendingSessionOpen {
    owner_window_label: String,
    responder: oneshot::Sender<Result<String, String>>,
}

struct RunEntry {
    owner_window_label: String,
    cancellation: CancellationToken,
    status: Mutex<NyaScriptRunStatus>,
}

#[derive(Debug, Clone)]
struct BoundSession {
    session_id: String,
    cursor: u64,
}

struct RunContext {
    aliases: HashMap<String, BoundSession>,
    current_alias: Option<String>,
    default_timeout: Duration,
    last_wait: Option<WaitOutcome>,
    generated_alias: usize,
}

impl RunContext {
    fn new() -> Self {
        Self {
            aliases: HashMap::new(),
            current_alias: None,
            default_timeout: DEFAULT_TIMEOUT,
            last_wait: None,
            generated_alias: 0,
        }
    }

    fn current(&self) -> Result<(&str, &BoundSession), RunFailure> {
        let alias = self.current_alias.as_deref().ok_or_else(|| {
            RunFailure::Failed(
                "NyaScript has no active session; use 'connect' or bind a current session first."
                    .into(),
            )
        })?;
        let bound = self.aliases.get(alias).ok_or_else(|| {
            RunFailure::Failed(format!("NyaScript session alias '{alias}' is not bound."))
        })?;
        Ok((alias, bound))
    }

    fn current_mut(&mut self) -> Result<(&str, &mut BoundSession), RunFailure> {
        let alias = self.current_alias.clone().ok_or_else(|| {
            RunFailure::Failed(
                "NyaScript has no active session; use 'connect' or bind a current session first."
                    .into(),
            )
        })?;
        let bound = self.aliases.get_mut(&alias).ok_or_else(|| {
            RunFailure::Failed(format!("NyaScript session alias '{alias}' is not bound."))
        })?;
        Ok((self.current_alias.as_deref().expect("current alias"), bound))
    }

    fn next_alias(&mut self) -> String {
        self.generated_alias += 1;
        format!("session{}", self.generated_alias)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Matched,
    Timeout,
}

#[derive(Debug)]
enum RunFailure {
    Cancelled,
    Failed(String),
}

type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<(), RunFailure>> + Send + 'a>>;

pub struct NyaScriptManager {
    sessions: Arc<SessionManager>,
    app: OnceLock<AppHandle>,
    runs: Mutex<HashMap<String, Arc<RunEntry>>>,
    pending_session_opens: Mutex<HashMap<String, PendingSessionOpen>>,
}

impl NyaScriptManager {
    pub fn new(sessions: Arc<SessionManager>) -> Arc<Self> {
        Arc::new(Self {
            sessions,
            app: OnceLock::new(),
            runs: Mutex::new(HashMap::new()),
            pending_session_opens: Mutex::new(HashMap::new()),
        })
    }

    pub fn initialize(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    pub async fn start_run(
        self: &Arc<Self>,
        owner_window_label: String,
        source: String,
        current_session_id: Option<String>,
    ) -> AppResult<String> {
        let program =
            parser::parse(&source).map_err(|error| AppError::Config(error.to_string()))?;
        let mut context = RunContext::new();
        if let Some(session_id) = current_session_id {
            let info = self.sessions.session_info(&session_id).await?;
            if !info.connected {
                return Err(AppError::Config(
                    "The current terminal session is not connected.".into(),
                ));
            }
            if info.owner_window_label.as_deref() != Some(owner_window_label.as_str()) {
                return Err(AppError::Config(
                    "The current terminal session belongs to another window.".into(),
                ));
            }
            let cursor = self
                .sessions
                .recent_output_tail_cursor(&session_id)
                .map_err(|error| AppError::Config(error.to_string()))?;
            context
                .aliases
                .insert("current".into(), BoundSession { session_id, cursor });
            context.current_alias = Some("current".into());
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let entry = Arc::new(RunEntry {
            owner_window_label,
            cancellation: CancellationToken::new(),
            status: Mutex::new(NyaScriptRunStatus {
                run_id: run_id.clone(),
                state: NyaScriptRunState::Running,
                current_line: None,
                active_alias: context.current_alias.clone(),
                session_id: context
                    .current_alias
                    .as_ref()
                    .and_then(|alias| context.aliases.get(alias))
                    .map(|bound| bound.session_id.clone()),
                logs: Vec::new(),
                error: None,
            }),
        });
        self.runs.lock().await.insert(run_id.clone(), entry.clone());
        self.emit_event(&entry, "started", None, None).await;

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run_program(entry, program, context).await;
        });
        Ok(run_id)
    }

    pub async fn cancel_run(&self, run_id: &str) -> AppResult<()> {
        let entry = self
            .runs
            .lock()
            .await
            .get(run_id)
            .cloned()
            .ok_or_else(|| AppError::Config(format!("NyaScript run '{run_id}' not found.")))?;
        entry.cancellation.cancel();
        Ok(())
    }

    pub async fn run_status(&self, run_id: &str) -> AppResult<NyaScriptRunStatus> {
        let entry = self
            .runs
            .lock()
            .await
            .get(run_id)
            .cloned()
            .ok_or_else(|| AppError::Config(format!("NyaScript run '{run_id}' not found.")))?;
        Ok(entry.status.lock().await.clone())
    }

    pub async fn respond_session_open(
        &self,
        owner_window_label: &str,
        request_id: &str,
        session_id: Option<String>,
        error: Option<String>,
    ) -> AppResult<()> {
        let pending = self
            .pending_session_opens
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| {
                AppError::Config("The NyaScript session-open request is no longer pending.".into())
            })?;
        if pending.owner_window_label != owner_window_label {
            self.pending_session_opens
                .lock()
                .await
                .insert(request_id.to_string(), pending);
            return Err(AppError::Config(
                "Only the target main window can complete this NyaScript session-open request."
                    .into(),
            ));
        }
        let result = match (session_id, error) {
            (Some(session_id), None) if !session_id.trim().is_empty() => Ok(session_id),
            (None, Some(error)) if !error.trim().is_empty() => Err(error),
            _ => Err("Invalid NyaScript session-open response.".into()),
        };
        pending.responder.send(result).map_err(|_| {
            AppError::Cancelled("The NyaScript session-open request was cancelled.".into())
        })
    }

    async fn run_program(
        self: Arc<Self>,
        entry: Arc<RunEntry>,
        program: Program,
        mut context: RunContext,
    ) {
        let result = self
            .execute_block(&entry, &program.statements, &mut context)
            .await;
        match result {
            Ok(()) => {
                self.set_final_state(&entry, NyaScriptRunState::Completed, None)
                    .await;
                self.emit_event(&entry, "completed", None, None).await;
            }
            Err(RunFailure::Cancelled) => {
                self.set_final_state(&entry, NyaScriptRunState::Cancelled, None)
                    .await;
                self.emit_event(&entry, "cancelled", None, None).await;
            }
            Err(RunFailure::Failed(message)) => {
                let message = sanitize_message(&message);
                self.set_final_state(&entry, NyaScriptRunState::Failed, Some(message.clone()))
                    .await;
                self.emit_event(&entry, "failed", None, Some(message)).await;
            }
        }
    }

    fn execute_block<'a>(
        self: &'a Arc<Self>,
        entry: &'a Arc<RunEntry>,
        statements: &'a [Statement],
        context: &'a mut RunContext,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            for statement in statements {
                if entry.cancellation.is_cancelled() {
                    return Err(RunFailure::Cancelled);
                }
                self.mark_line(entry, statement.line(), context).await;
                match statement {
                    Statement::Connect { target, alias, .. } => {
                        let session_id = self
                            .request_session_open(
                                &entry.owner_window_label,
                                target,
                                &entry.cancellation,
                            )
                            .await?;
                        let cursor = self
                            .sessions
                            .recent_output_tail_cursor(&session_id)
                            .map_err(|error| RunFailure::Failed(error.to_string()))?;
                        let alias = alias.clone().unwrap_or_else(|| context.next_alias());
                        if context.aliases.contains_key(&alias) {
                            return Err(RunFailure::Failed(format!(
                                "NyaScript alias '{alias}' is already bound."
                            )));
                        }
                        context
                            .aliases
                            .insert(alias.clone(), BoundSession { session_id, cursor });
                        context.current_alias = Some(alias);
                        self.mark_line(entry, statement.line(), context).await;
                    }
                    Statement::Use { alias, .. } => {
                        if !context.aliases.contains_key(alias) {
                            return Err(RunFailure::Failed(format!(
                                "NyaScript alias '{alias}' is not bound."
                            )));
                        }
                        context.current_alias = Some(alias.clone());
                        self.mark_line(entry, statement.line(), context).await;
                    }
                    Statement::Send { value, newline, .. } => {
                        let (_, bound) = context.current()?;
                        let (mut data, sensitivity) = match value {
                            Value::Literal(value) => (value.clone(), InputSensitivity::Normal),
                            Value::Secret(reference) => {
                                (self.resolve_secret(reference)?, InputSensitivity::Secret)
                            }
                        };
                        if *newline {
                            data.push('\r');
                        }
                        self.sessions
                            .send_command(
                                &bound.session_id,
                                SessionCommand::Write {
                                    data: data.into_bytes(),
                                    raw: false,
                                    automated: true,
                                    origin: InputOrigin::NyaScript,
                                    sensitivity,
                                },
                            )
                            .await
                            .map_err(|error| RunFailure::Failed(error.to_string()))?;
                    }
                    Statement::Wait { matcher, .. } => {
                        let timeout = context.default_timeout;
                        let (_, bound) = context.current_mut()?;
                        let outcome = self
                            .wait_for_match(
                                &bound.session_id,
                                &mut bound.cursor,
                                matcher,
                                timeout,
                                &entry.cancellation,
                            )
                            .await?;
                        context.last_wait = Some(outcome);
                    }
                    Statement::Timeout { duration, .. } => {
                        context.default_timeout = *duration;
                    }
                    Statement::If {
                        condition,
                        then_body,
                        else_body,
                        ..
                    } => {
                        let last_wait = context.last_wait.ok_or_else(|| {
                            RunFailure::Failed(
                                "NyaScript 'if' requires a previous wait or wait_regex result."
                                    .into(),
                            )
                        })?;
                        let matched = matches!(
                            (condition, last_wait),
                            (WaitCondition::Matched, WaitOutcome::Matched)
                                | (WaitCondition::Timeout, WaitOutcome::Timeout)
                        );
                        let body = if matched { then_body } else { else_body };
                        self.execute_block(entry, body, context).await?;
                    }
                    Statement::Repeat { count, body, .. } => {
                        for _ in 0..*count {
                            self.execute_block(entry, body, context).await?;
                        }
                    }
                    Statement::Log { message, .. } => {
                        self.append_log(entry, message).await;
                    }
                }
            }
            Ok(())
        })
    }

    async fn wait_for_match(
        &self,
        session_id: &str,
        cursor: &mut u64,
        matcher: &WaitMatcher,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<WaitOutcome, RunFailure> {
        let notify = self
            .sessions
            .recent_output_notification(session_id)
            .map_err(|error| RunFailure::Failed(error.to_string()))?;
        let deadline = Instant::now() + timeout;

        loop {
            let notified = notify.notified();
            let snapshot = self
                .sessions
                .recent_output_since(session_id, *cursor)
                .map_err(|error| RunFailure::Failed(error.to_string()))?;
            if snapshot.closed {
                return Err(RunFailure::Failed(format!(
                    "NyaScript target session '{session_id}' closed while waiting."
                )));
            }
            let match_end = match matcher {
                WaitMatcher::Literal(literal) => snapshot
                    .text
                    .find(literal)
                    .map(|start| start + literal.len()),
                WaitMatcher::Regex(regex) => {
                    regex.find(&snapshot.text).map(|matched| matched.end())
                }
            };
            if let Some(match_end) = match_end {
                *cursor = cursor.saturating_add(match_end as u64);
                return Ok(WaitOutcome::Matched);
            }

            let sleep = tokio::time::sleep_until(deadline);
            tokio::pin!(sleep);
            tokio::select! {
                _ = cancellation.cancelled() => return Err(RunFailure::Cancelled),
                _ = &mut sleep => return Ok(WaitOutcome::Timeout),
                _ = notified => {}
            }
        }
    }

    async fn request_session_open(
        &self,
        owner_window_label: &str,
        target: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, RunFailure> {
        let app = self
            .app
            .get()
            .ok_or_else(|| RunFailure::Failed("NyaScript runtime is not initialized.".into()))?;
        let window = app.get_webview_window(owner_window_label).ok_or_else(|| {
            RunFailure::Failed("The NyaScript owner window is unavailable.".into())
        })?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending_session_opens.lock().await.insert(
            request_id.clone(),
            PendingSessionOpen {
                owner_window_label: owner_window_label.to_string(),
                responder: tx,
            },
        );
        let event = NyaScriptSessionOpenRequestEvent {
            request_id: request_id.clone(),
            target: target.to_string(),
            target_window_label: owner_window_label.to_string(),
        };
        if let Err(error) = window.emit("nyascript-session-open-request", event) {
            self.pending_session_opens.lock().await.remove(&request_id);
            return Err(RunFailure::Failed(format!(
                "Failed to send NyaScript session-open request: {error}"
            )));
        }

        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                self.pending_session_opens.lock().await.remove(&request_id);
                let _ = window.emit("nyascript-session-open-cancel", NyaScriptSessionOpenCancelEvent {
                    request_id: request_id.clone(),
                    target_window_label: owner_window_label.to_string(),
                });
                return Err(RunFailure::Cancelled);
            }
            result = rx => result.map_err(|_| {
                RunFailure::Cancelled
            })?,
        };
        let session_id = result.map_err(RunFailure::Failed)?;
        let info = self
            .sessions
            .session_info(&session_id)
            .await
            .map_err(|error| RunFailure::Failed(error.to_string()))?;
        if !info.connected
            || info.owner_window_label.as_deref() != Some(owner_window_label)
            || !matches!(
                info.session_type,
                SessionType::SSH | SessionType::Local | SessionType::Telnet | SessionType::Serial
            )
        {
            return Err(RunFailure::Failed(
                "The opened NyaScript session is not a connected terminal in the target window."
                    .into(),
            ));
        }
        if let Some(connection_id) = target.strip_prefix("saved:")
            && info.connection_id.as_deref() != Some(connection_id)
        {
            return Err(RunFailure::Failed(
                "The opened NyaScript session does not match the requested saved connection."
                    .into(),
            ));
        }
        Ok(session_id)
    }

    fn resolve_secret(&self, reference: &str) -> Result<String, RunFailure> {
        let app = self
            .app
            .get()
            .ok_or_else(|| RunFailure::Failed("NyaScript runtime is not initialized.".into()))?;
        let credentials =
            config::load_credentials(app).map_err(|error| RunFailure::Failed(error.to_string()))?;
        let id = if credentials
            .credentials
            .iter()
            .any(|credential| credential.id == reference)
        {
            reference.to_string()
        } else {
            let matches = credentials
                .credentials
                .iter()
                .filter(|credential| credential.name == reference)
                .map(|credential| credential.id.clone())
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [id] => id.clone(),
                [] => {
                    return Err(RunFailure::Failed(format!(
                        "Credential reference '{reference}' was not found."
                    )));
                }
                _ => {
                    return Err(RunFailure::Failed(format!(
                        "Credential name '{reference}' is ambiguous; use its credential ID."
                    )));
                }
            }
        };
        let credential = config::load_credential_by_id(app, &id)
            .map_err(|error| RunFailure::Failed(error.to_string()))?;
        credential
            .password
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RunFailure::Failed(format!(
                    "Credential reference '{reference}' has no decryptable password."
                ))
            })
    }

    async fn mark_line(&self, entry: &RunEntry, line: usize, context: &RunContext) {
        let (active_alias, session_id) = context
            .current_alias
            .as_ref()
            .map(|alias| {
                (
                    Some(alias.clone()),
                    context
                        .aliases
                        .get(alias)
                        .map(|bound| bound.session_id.clone()),
                )
            })
            .unwrap_or((None, None));
        {
            let mut status = entry.status.lock().await;
            status.current_line = Some(line);
            status.active_alias = active_alias;
            status.session_id = session_id;
        }
        self.emit_event(entry, "line", None, None).await;
    }

    async fn append_log(&self, entry: &RunEntry, message: &str) {
        let message = sanitize_message(message);
        {
            let mut status = entry.status.lock().await;
            status.logs.push(message.clone());
            if status.logs.len() > MAX_LOG_ENTRIES {
                let excess = status.logs.len() - MAX_LOG_ENTRIES;
                status.logs.drain(..excess);
            }
        }
        self.emit_event(entry, "log", Some(message), None).await;
    }

    async fn set_final_state(
        &self,
        entry: &RunEntry,
        state: NyaScriptRunState,
        error: Option<String>,
    ) {
        let mut status = entry.status.lock().await;
        status.state = state;
        status.error = error;
    }

    async fn emit_event(
        &self,
        entry: &RunEntry,
        event: &str,
        log: Option<String>,
        error: Option<String>,
    ) {
        let Some(app) = self.app.get() else {
            return;
        };
        let Some(window) = app.get_webview_window(&entry.owner_window_label) else {
            return;
        };
        let status = entry.status.lock().await.clone();
        let _ = window.emit(
            "nyascript-run-event",
            NyaScriptRunEvent {
                run_id: status.run_id,
                event: event.to_string(),
                state: status.state,
                current_line: status.current_line,
                active_alias: status.active_alias,
                session_id: status.session_id,
                log,
                error,
            },
        );
    }
}

fn sanitize_message(message: &str) -> String {
    message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(MAX_LOG_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nyascript_wait_catches_fast_output_and_consumes_only_through_match() {
        let sessions = Arc::new(SessionManager::new());
        sessions.append_recent_output("s", "");
        let manager = NyaScriptManager::new(sessions.clone());
        let mut cursor = sessions.recent_output_tail_cursor("s").unwrap();
        sessions.append_recent_output("s", "fast prompt>tail");
        let outcome = manager
            .wait_for_match(
                "s",
                &mut cursor,
                &WaitMatcher::Literal("prompt>".into()),
                Duration::from_secs(1),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, WaitOutcome::Matched);
        assert_eq!(
            sessions.recent_output_since("s", cursor).unwrap().text,
            "tail"
        );
    }

    #[tokio::test]
    async fn nyascript_wait_regex_times_out_without_consuming_output() {
        let sessions = Arc::new(SessionManager::new());
        sessions.append_recent_output("s", "booting");
        let manager = NyaScriptManager::new(sessions.clone());
        let mut cursor = 0;
        let outcome = manager
            .wait_for_match(
                "s",
                &mut cursor,
                &WaitMatcher::Regex(regex::Regex::new("ready>").unwrap()),
                Duration::from_millis(5),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, WaitOutcome::Timeout);
        assert_eq!(cursor, 0);
    }

    #[tokio::test]
    async fn nyascript_wait_cancellation_finishes_immediately() {
        let sessions = Arc::new(SessionManager::new());
        sessions.append_recent_output("s", "");
        let manager = NyaScriptManager::new(sessions);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut cursor = 0;
        let result = manager
            .wait_for_match(
                "s",
                &mut cursor,
                &WaitMatcher::Literal("never".into()),
                Duration::from_secs(60),
                &cancellation,
            )
            .await;
        assert!(matches!(result, Err(RunFailure::Cancelled)));
    }

    #[test]
    fn nyascript_log_sanitizer_removes_control_characters_and_bounds_length() {
        let input = format!("ok\n{}end", "x".repeat(MAX_LOG_CHARS + 100));
        let sanitized = sanitize_message(&input);
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.chars().count() <= MAX_LOG_CHARS);
    }
}

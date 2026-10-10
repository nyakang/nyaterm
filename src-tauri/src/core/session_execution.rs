/// A timeout only stops result collection. Ownership stays with the I/O loop
/// until the shell emits END or the session is torn down.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalExecutionState {
    Idle,
    Running,
    AwaitingEnd,
}

struct ExecutionEntry {
    marker_id: String,
    state: TerminalExecutionState,
}

pub struct TerminalExecutionGuard {
    manager: Arc<SessionManager>,
    session_id: String,
    marker_id: String,
    completion: Option<Box<dyn FnOnce(&CapturedOutput) + Send>>,
    pub command_file: Option<super::capture::PowershellCommandFile>,
}

impl TerminalExecutionGuard {
    pub fn on_complete(&mut self, callback: impl FnOnce(&CapturedOutput) + Send + 'static) {
        self.completion = Some(Box::new(callback));
    }

    pub fn complete(&mut self, result: &CapturedOutput) {
        if let Some(callback) = self.completion.take() {
            callback(result);
        }
    }

    pub fn awaiting_end(&self) {
        let mut executions = self.manager.terminal_executions.lock().unwrap();
        if let Some(entry) = executions.get_mut(&self.session_id) {
            if entry.marker_id == self.marker_id {
                entry.state = TerminalExecutionState::AwaitingEnd;
                self.manager
                    .emit_execution_state(&self.session_id, entry.state);
            }
        }
    }
}

impl Drop for TerminalExecutionGuard {
    fn drop(&mut self) {
        let mut executions = self.manager.terminal_executions.lock().unwrap();
        if executions
            .get(&self.session_id)
            .is_some_and(|entry| entry.marker_id == self.marker_id)
        {
            executions.remove(&self.session_id);
            self.manager
                .emit_execution_state(&self.session_id, TerminalExecutionState::Idle);
        }
    }
}

impl SessionManager {
    pub async fn begin_terminal_execution(
        self: &Arc<Self>,
        session_id: &str,
        marker_id: &str,
    ) -> AppResult<TerminalExecutionGuard> {
        // Use the same lock order as send_command, so input cannot pass its
        // busy check and then get enqueued after automation takes ownership.
        let _sessions = self.sessions.lock().await;
        let mut executions = self.terminal_executions.lock().unwrap();
        if executions.contains_key(session_id) {
            return Err(AppError::SessionBusy("This session is executing an automated command. Wait for it to finish or interrupt it with Ctrl+C.".into()));
        }
        executions.insert(
            session_id.to_string(),
            ExecutionEntry {
                marker_id: marker_id.to_string(),
                state: TerminalExecutionState::Running,
            },
        );
        self.emit_execution_state(session_id, TerminalExecutionState::Running);
        Ok(TerminalExecutionGuard {
            manager: self.clone(),
            session_id: session_id.to_string(),
            marker_id: marker_id.to_string(),
            completion: None,
            command_file: None,
        })
    }

    pub fn terminal_execution_state(&self, session_id: &str) -> TerminalExecutionState {
        self.terminal_executions
            .lock()
            .unwrap()
            .get(session_id)
            .map_or(TerminalExecutionState::Idle, |entry| entry.state)
    }

    pub(crate) fn publish_terminal_execution_state(&self, session_id: &str) {
        let executions = self.terminal_executions.lock().unwrap();
        self.emit_execution_state(
            session_id,
            executions
                .get(session_id)
                .map_or(TerminalExecutionState::Idle, |entry| entry.state),
        );
    }

    pub(crate) fn emit_execution_state(&self, session_id: &str, state: TerminalExecutionState) {
        if let Some(app) = self.app_handle.get() {
            let _ = app.emit(&format!("terminal-execution-{session_id}"), state);
        }
    }
}

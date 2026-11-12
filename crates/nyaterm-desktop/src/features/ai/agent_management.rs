use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::Value;

use super::helper_resolver::resolve_mcp_helper;
use crate::thread_owner::spawn_joinable;
use nyaterm_agent_client::executable::resolve_codex_executable;

use nyaterm_agent_client::account::{CodexAccountClient, CodexAccountRequest};
use nyaterm_agent_client::probe::{claude_auth_status, cli_version, resolve_claude_executable};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct AgentManagementView {
    pub codex_version: Option<String>,
    pub codex_models: Vec<String>,
    pub claude_version: Option<String>,
    pub claude_connected: bool,
    pub mcp_helper_path: Option<PathBuf>,
    pub codex_connected: bool,
    pub codex_auth_mode: Option<String>,
    pub codex_plan: Option<String>,
    pub codex_email: Option<String>,
    pub login_id: Option<String>,
    pub auth_url: Option<String>,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
    pub pending: bool,
    pub error: Option<String>,
}

pub(in crate::features) enum AgentCommand {
    Models {
        codex: Option<String>,
    },
    Refresh {
        codex: Option<String>,
        claude: Option<String>,
    },
    Account {
        codex: Option<String>,
    },
    ClaudeAccount {
        claude: Option<String>,
    },
    Login {
        codex: Option<String>,
        device_code: bool,
    },
    Cancel {
        codex: Option<String>,
        login_id: String,
    },
    Logout {
        codex: Option<String>,
    },
}

pub(in crate::features) enum AgentEvent {
    Models(Result<Value, String>),
    Detected {
        codex: Option<String>,
        claude: Option<String>,
        claude_connected: bool,
        mcp_helper_path: Option<PathBuf>,
        account: Result<Value, String>,
    },
    Account(Result<Value, String>),
    AccountUpdated(Value),
    ClaudeAccount(bool),
    Login(Result<Value, String>),
    Cancel(Result<(), String>),
    Logout(Result<(), String>),
}

pub(in crate::features) struct AgentManagementState {
    view: AgentManagementView,
    generation: u64,
    commands: Option<mpsc::Sender<(u64, AgentCommand)>>,
    events: Option<UnboundedReceiver<AgentJobEvent>>,
    worker: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}

pub(in crate::features) struct AgentJobEvent {
    generation: u64,
    event: AgentEvent,
}

impl AgentManagementState {
    pub fn new() -> Self {
        Self {
            view: AgentManagementView::default(),
            generation: 0,
            commands: None,
            events: None,
            worker: None,
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn view(&self) -> &AgentManagementView {
        &self.view
    }

    pub fn set_error(&mut self, error: String) {
        self.view.error = Some(error);
    }

    pub fn submit(&mut self, command: AgentCommand) -> bool {
        self.generation = self.generation.wrapping_add(1);
        if self.commands.is_none() {
            let (command_tx, command_rx) = mpsc::channel();
            let (event_tx, event_rx) = unbounded();
            self.stopping = Arc::new(AtomicBool::new(false));
            let stopping = Arc::clone(&self.stopping);
            let worker = match spawn_joinable("nyaterm-agent-settings", move || {
                run_agent_worker(command_rx, event_tx, stopping)
            }) {
                Ok(worker) => worker,
                Err(_) => {
                    self.view.error = Some("Failed to start the agent settings worker".to_string());
                    return false;
                }
            };
            self.commands = Some(command_tx);
            self.events = Some(event_rx);
            self.worker = Some(worker);
        }
        self.view.pending = true;
        self.view.error = None;
        if self
            .commands
            .as_ref()
            .is_some_and(|tx| tx.send((self.generation, command)).is_ok())
        {
            true
        } else {
            self.view.pending = false;
            self.view.error = Some("Agent settings worker stopped".to_string());
            self.stop_worker();
            false
        }
    }

    pub fn take_events(&mut self) -> Option<UnboundedReceiver<AgentJobEvent>> {
        self.events.take()
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.view.pending = false;
        self.clear_login();
    }

    pub fn apply_job(&mut self, event: AgentJobEvent) -> Option<String> {
        if event.generation != self.generation {
            return None;
        }
        self.apply(event.event)
    }

    pub fn apply(&mut self, event: AgentEvent) -> Option<String> {
        let unsolicited = matches!(event, AgentEvent::AccountUpdated(_));
        if !unsolicited {
            self.view.pending = false;
        }
        let open_login_url = matches!(event, AgentEvent::Login(Ok(_)));
        let result = match event {
            AgentEvent::Models(result) => result.map(|value| {
                self.view.codex_models = value["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item["model"].as_str().map(str::to_owned))
                    .collect();
            }),
            AgentEvent::Detected {
                codex,
                claude,
                claude_connected,
                mcp_helper_path,
                account,
            } => {
                self.view.codex_version = codex;
                self.view.claude_version = claude;
                self.view.claude_connected = claude_connected;
                self.view.mcp_helper_path = mcp_helper_path;
                account.map(|value| self.set_account(&value))
            }
            AgentEvent::Account(result) => result.map(|value| self.set_account(&value)),
            AgentEvent::AccountUpdated(value) => {
                self.set_account(&value);
                Ok(())
            }
            AgentEvent::ClaudeAccount(connected) => {
                self.view.claude_connected = connected;
                Ok(())
            }
            AgentEvent::Login(result) => result.map(|value| {
                self.view.login_id = value
                    .get("loginId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.view.auth_url = value
                    .get("authUrl")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.view.verification_url = value
                    .get("verificationUrl")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.view.user_code = value
                    .get("userCode")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }),
            AgentEvent::Cancel(result) => result.map(|()| self.clear_login()),
            AgentEvent::Logout(result) => result.map(|()| {
                self.clear_login();
                self.view.codex_connected = false;
                self.view.codex_auth_mode = None;
                self.view.codex_plan = None;
                self.view.codex_email = None;
            }),
        };
        if !unsolicited {
            self.view.error = result.err();
        }
        open_login_url.then(|| self.view.auth_url.clone()).flatten()
    }

    fn set_account(&mut self, value: &Value) {
        let account = &value["account"];
        self.view.codex_auth_mode = account["type"].as_str().map(str::to_owned);
        self.view.codex_connected = self.view.codex_auth_mode.is_some();
        self.view.codex_plan = account["planType"].as_str().map(str::to_owned);
        self.view.codex_email = account["email"].as_str().map(str::to_owned);
        if self.view.codex_connected {
            self.clear_login();
        }
    }

    fn clear_login(&mut self) {
        self.view.login_id = None;
        self.view.auth_url = None;
        self.view.verification_url = None;
        self.view.user_code = None;
    }

    pub(in crate::features) fn begin_shutdown(&mut self) -> Option<JoinHandle<()>> {
        self.stopping.store(true, Ordering::Release);
        self.commands.take();
        self.worker.take()
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.begin_shutdown() {
            let _ = worker.join();
        }
    }
}

impl Drop for AgentManagementState {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn run_agent_worker(
    commands: mpsc::Receiver<(u64, AgentCommand)>,
    events: UnboundedSender<AgentJobEvent>,
    stopping: Arc<AtomicBool>,
) {
    let mut codex_client: Option<CodexAccountClient> = None;
    let mut generation = 0;
    loop {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let command = match commands.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(client) = codex_client.as_mut()
                    && let Some(account) = client.poll_account_update()
                    && events
                        .unbounded_send(AgentJobEvent {
                            generation,
                            event: AgentEvent::AccountUpdated(account),
                        })
                        .is_err()
                {
                    break;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        generation = command.0;
        let event = match command.1 {
            AgentCommand::Refresh { codex, claude } => {
                let codex_path = resolve_codex_executable(codex.as_deref()).ok();
                let codex_version = codex_path
                    .as_deref()
                    .and_then(|path| cli_version(path, &stopping));
                let claude_version = resolve_claude_executable(claude.as_deref())
                    .as_deref()
                    .and_then(|path| cli_version(path, &stopping));
                let account = request_codex(
                    &mut codex_client,
                    &stopping,
                    codex_path,
                    CodexAccountRequest::Account,
                );
                AgentEvent::Detected {
                    codex: codex_version,
                    claude: claude_version,
                    claude_connected: claude_auth_status(claude.as_deref(), &stopping),
                    mcp_helper_path: resolve_mcp_helper().ok(),
                    account,
                }
            }
            AgentCommand::Models { codex } => AgentEvent::Models(request_codex(
                &mut codex_client,
                &stopping,
                resolve_codex_executable(codex.as_deref()).ok(),
                CodexAccountRequest::Models,
            )),
            AgentCommand::Account { codex } => AgentEvent::Account(request_codex(
                &mut codex_client,
                &stopping,
                resolve_codex_executable(codex.as_deref()).ok(),
                CodexAccountRequest::Account,
            )),
            AgentCommand::ClaudeAccount { claude } => {
                AgentEvent::ClaudeAccount(claude_auth_status(claude.as_deref(), &stopping))
            }
            AgentCommand::Login { codex, device_code } => AgentEvent::Login(request_codex(
                &mut codex_client,
                &stopping,
                resolve_codex_executable(codex.as_deref()).ok(),
                CodexAccountRequest::Login { device_code },
            )),
            AgentCommand::Cancel { codex, login_id } => AgentEvent::Cancel(
                request_codex(
                    &mut codex_client,
                    &stopping,
                    resolve_codex_executable(codex.as_deref()).ok(),
                    CodexAccountRequest::CancelLogin { login_id },
                )
                .map(|_| ()),
            ),
            AgentCommand::Logout { codex } => AgentEvent::Logout(
                request_codex(
                    &mut codex_client,
                    &stopping,
                    resolve_codex_executable(codex.as_deref()).ok(),
                    CodexAccountRequest::Logout,
                )
                .map(|_| ()),
            ),
        };
        if events
            .unbounded_send(AgentJobEvent { generation, event })
            .is_err()
        {
            break;
        }
    }
}

fn request_codex(
    client: &mut Option<CodexAccountClient>,
    stopping: &Arc<AtomicBool>,
    path: Option<PathBuf>,
    request: CodexAccountRequest,
) -> Result<Value, String> {
    let result = with_codex(client, path, stopping).and_then(|client| client.request(request));
    if result.is_err() {
        *client = None;
    }
    result
}

fn with_codex<'a>(
    client: &'a mut Option<CodexAccountClient>,
    path: Option<PathBuf>,
    stopping: &Arc<AtomicBool>,
) -> Result<&'a mut CodexAccountClient, String> {
    if stopping.load(Ordering::Acquire) {
        return Err("Agent settings worker cancelled".into());
    }
    let path = path.ok_or_else(|| "Codex CLI was not found".to_string())?;
    if client
        .as_ref()
        .is_some_and(|current| current.path() != path)
    {
        *client = None;
    }
    if client.as_mut().is_some_and(|current| !current.is_alive()) {
        *client = None;
    }
    if client.is_none() {
        *client = Some(CodexAccountClient::start(path, Arc::clone(stopping))?);
    }
    Ok(client.as_mut().expect("client was initialized"))
}

#[cfg(test)]
mod tests {
    use super::{AgentEvent, AgentJobEvent, AgentManagementState};
    use serde_json::json;
    #[test]
    fn stale_agent_results_cannot_restore_login_or_account_state() {
        let mut state = AgentManagementState::new();
        let generation = state.generation;
        state.invalidate();
        assert!(
            state
                .apply_job(AgentJobEvent {
                    generation,
                    event: AgentEvent::Login(Ok(
                        json!({ "loginId": "stale", "authUrl": "https://example.invalid/login" })
                    )),
                })
                .is_none()
        );
        state.apply_job(AgentJobEvent {
            generation,
            event: AgentEvent::AccountUpdated(json!({ "account": { "type": "chatgpt" } })),
        });
        assert!(state.view().login_id.is_none());
        assert!(!state.view().codex_connected);
        state.apply_job(AgentJobEvent {
            generation: state.generation,
            event: AgentEvent::Account(Ok(json!({ "account": { "type": "chatgpt" } }))),
        });
        assert!(state.view().codex_connected);
    }

    #[test]
    fn account_login_cancel_and_logout_update_only_transient_state() {
        let mut state = AgentManagementState::new();
        state.apply(AgentEvent::Detected {
            codex: Some("codex 1.2".to_string()),
            claude: Some("claude fixture".into()),
            claude_connected: false,
            mcp_helper_path: None,
            account: Ok(json!({ "account": null })),
        });
        assert_eq!(state.view().codex_version.as_deref(), Some("codex 1.2"));
        assert!(!state.view().codex_connected);
        assert!(!state.view().claude_connected);
        state.apply(AgentEvent::ClaudeAccount(true));
        assert!(state.view().claude_connected);
        state.apply(AgentEvent::ClaudeAccount(false));
        assert!(!state.view().claude_connected);
        state.view.pending = true;
        state.apply(AgentEvent::AccountUpdated(json!({
            "account": { "type": "chatgpt" }
        })));
        assert!(state.view().pending);

        let url = state.apply(AgentEvent::Login(Ok(json!({
            "loginId": "login-1",
            "authUrl": "https://example.invalid/login",
            "verificationUrl": "https://example.invalid/device",
            "userCode": "TEST-CODE"
        }))));
        assert_eq!(url.as_deref(), Some("https://example.invalid/login"));
        assert_eq!(state.view().user_code.as_deref(), Some("TEST-CODE"));
        assert!(state.apply(AgentEvent::Account(Ok(json!({
            "account": { "type": "chatgpt", "planType": "plus", "email": "test@example.invalid" }
        })))).is_none());
        assert!(state.view().codex_connected);
        assert!(state.view().user_code.is_none());

        state.apply(AgentEvent::Login(Ok(
            json!({ "loginId": "login-2", "userCode": "SECOND" }),
        )));
        state.apply(AgentEvent::Cancel(Err("cancel failed".to_string())));
        assert_eq!(state.view().login_id.as_deref(), Some("login-2"));
        assert_eq!(state.view().error.as_deref(), Some("cancel failed"));
        state.apply(AgentEvent::Cancel(Ok(())));
        assert!(state.view().login_id.is_none());
        state.apply(AgentEvent::Logout(Err("logout failed".to_string())));
        assert!(state.view().codex_connected);
        state.apply(AgentEvent::Logout(Ok(())));
        assert!(!state.view().codex_connected);
    }
}

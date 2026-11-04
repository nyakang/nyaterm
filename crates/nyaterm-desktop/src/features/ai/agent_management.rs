use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_core::ai::{codex_initialize_request, codex_initialized_notification};
use serde_json::{Value, json};

use super::helper_resolver::resolve_codex_executable;
use super::helper_resolver::resolve_mcp_helper;
use crate::thread_owner::spawn_joinable;

use crate::external_process::child_process::AgentChild;

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
                    "account/read",
                    json!({ "refreshToken": false }),
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
                "model/list",
                json!({"limit": 100}),
            )),
            AgentCommand::Account { codex } => AgentEvent::Account(request_codex(
                &mut codex_client,
                &stopping,
                resolve_codex_executable(codex.as_deref()).ok(),
                "account/read",
                json!({ "refreshToken": false }),
            )),
            AgentCommand::ClaudeAccount { claude } => {
                AgentEvent::ClaudeAccount(claude_auth_status(claude.as_deref(), &stopping))
            }
            AgentCommand::Login { codex, device_code } => AgentEvent::Login(request_codex(
                &mut codex_client,
                &stopping,
                resolve_codex_executable(codex.as_deref()).ok(),
                "account/login/start",
                if device_code {
                    json!({ "type": "chatgptDeviceCode" })
                } else {
                    json!({ "type": "chatgpt", "useHostedLoginSuccessPage": true, "appBrand": "codex" })
                },
            )),
            AgentCommand::Cancel { codex, login_id } => AgentEvent::Cancel(
                request_codex(
                    &mut codex_client,
                    &stopping,
                    resolve_codex_executable(codex.as_deref()).ok(),
                    "account/login/cancel",
                    json!({ "loginId": login_id }),
                )
                .map(|_| ()),
            ),
            AgentCommand::Logout { codex } => AgentEvent::Logout(
                request_codex(
                    &mut codex_client,
                    &stopping,
                    resolve_codex_executable(codex.as_deref()).ok(),
                    "account/logout",
                    json!({}),
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
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let result =
        with_codex(client, path, stopping).and_then(|client| client.request(method, params));
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
    if client.as_ref().is_some_and(|current| current.path != path) {
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

fn resolve_claude_executable(configured: Option<&str>) -> Option<PathBuf> {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    if let Some(path) = configured {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.cmd", "claude.exe", "claude"]
    } else {
        &["claude"]
    };
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
            .find(|path| path.is_file())
    })
}

fn cli_version(path: &Path, stopping: &AtomicBool) -> Option<String> {
    cli_output(path, &["--version"], stopping)?
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn claude_auth_status(configured: Option<&str>, stopping: &AtomicBool) -> bool {
    resolve_claude_executable(configured)
        .and_then(|path| cli_output(&path, &["auth", "status", "--json"], stopping))
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .is_some_and(|value| value["loggedIn"] == true)
}

fn cli_output(path: &Path, args: &[&str], stopping: &AtomicBool) -> Option<String> {
    if stopping.load(Ordering::Acquire) {
        return None;
    }
    let mut command = Command::new(path);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_window(&mut command);
    let mut child = AgentChild::spawn(&mut command).ok()?;
    let stdout = child.child_mut().stdout.take()?;
    let (output_tx, output_rx) = mpsc::channel();
    let reader = spawn_joinable("nyaterm-agent-version-reader", move || {
        use std::io::Read as _;
        let mut output = String::new();
        let result = stdout
            .take(64 * 1024)
            .read_to_string(&mut output)
            .ok()
            .map(|_| output);
        let _ = output_tx.send(result);
    })
    .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = None;
    while !stopping.load(Ordering::Acquire) && Instant::now() < deadline {
        match child.child_mut().try_wait() {
            Ok(Some(status)) if status.success() => match output_rx.try_recv() {
                Ok(value) => {
                    output = value;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
            },
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {}
        }
        std::thread::sleep(WORKER_POLL_INTERVAL);
    }
    child.terminate();
    let _ = reader.join();
    output
}

struct CodexAccountClient {
    path: PathBuf,
    child: AgentChild,
    writer: Option<BufWriter<ChildStdin>>,
    lines: mpsc::Receiver<String>,
    next_id: u64,
    account_update: Option<Value>,
    reader: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}

impl CodexAccountClient {
    fn start(path: PathBuf, stopping: Arc<AtomicBool>) -> Result<Self, String> {
        let mut command = Command::new(&path);
        command.args(["app-server", "--listen", "stdio://"]);
        Self::start_with_command(path, command, stopping)
    }

    fn start_with_command(
        path: PathBuf,
        mut command: Command,
        stopping: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_window(&mut command);
        let mut child = AgentChild::spawn(&mut command)
            .map_err(|error| format!("Failed to start Codex: {error}"))?;
        let writer = BufWriter::new(
            child
                .child_mut()
                .stdin
                .take()
                .ok_or("Codex stdin unavailable")?,
        );
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .ok_or("Codex stdout unavailable")?;
        let (tx, lines) = mpsc::channel();
        let reader = spawn_joinable("nyaterm-codex-account-reader", move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|error| error.to_string())?;
        let mut client = Self {
            path,
            child,
            writer: Some(writer),
            lines,
            next_id: 1,
            account_update: None,
            reader: Some(reader),
            stopping,
        };
        client.send(&codex_initialize_request(1, env!("CARGO_PKG_VERSION")))?;
        client.wait_response(1)?;
        client.send(&codex_initialized_notification())?;
        client.next_id = 2;
        Ok(client)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "id": id, "method": method, "params": params }))?;
        self.wait_response(id)
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.child_mut().try_wait(), Ok(None))
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("Agent settings worker cancelled".into());
        }
        let writer = self.writer.as_mut().ok_or("Codex stdin closed")?;
        serde_json::to_writer(&mut *writer, value).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    }

    fn wait_response(&mut self, id: u64) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if self.stopping.load(Ordering::Acquire) {
                return Err("Agent settings worker cancelled".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("Codex account request timed out".to_string());
            }
            let line = match self.lines.recv_timeout(remaining.min(WORKER_POLL_INTERVAL)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Codex app-server closed".into());
                }
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(account) = account_update_from_notification(&value) {
                self.account_update = Some(account);
                continue;
            }
            if let (Some(request_id), Some(_)) = (
                value.get("id").and_then(Value::as_u64),
                value.get("method").and_then(Value::as_str),
            ) {
                self.send(&json!({ "id": request_id, "error": { "code": -32601, "message": "Unsupported request" } }))?;
                continue;
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex request failed")
                    .to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn poll_account_update(&mut self) -> Option<Value> {
        if let Some(account) = self.account_update.take() {
            return Some(account);
        }
        while !self.stopping.load(Ordering::Acquire)
            && let Ok(line) = self.lines.try_recv()
        {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(account) = account_update_from_notification(&value) {
                return Some(account);
            }
        }
        None
    }
}

fn account_update_from_notification(value: &Value) -> Option<Value> {
    (value.get("method").and_then(Value::as_str) == Some("account/updated")).then(|| {
        let params = &value["params"];
        json!({ "account": {
            "type": params.get("authMode"),
            "planType": params.get("planType")
        } })
    })
}

impl Drop for CodexAccountClient {
    fn drop(&mut self) {
        // Close stdin and the entire process tree before waiting for pipe EOF.
        if let Some(writer) = self.writer.take() {
            // Drop must not flush buffered protocol bytes into a stalled stdin.
            let (stdin, _) = writer.into_parts();
            drop(stdin);
        }
        self.child.terminate();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, Write};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use serde_json::json;

    use super::{
        AgentEvent, AgentJobEvent, AgentManagementState, CodexAccountClient,
        account_update_from_notification,
    };

    #[test]
    fn mock_codex_app_server() {
        if std::env::var_os("NYATERM_MOCK_CODEX_ACCOUNT_SERVER").is_none() {
            return;
        }
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout().lock();
        if std::env::var_os("NYATERM_MOCK_CODEX_KEEP_OPEN").is_some() {
            writeln!(stdout, "{}", json!({"fixturePid": std::process::id()})).unwrap();
            stdout.flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        for line in stdin.lock().lines() {
            let line = line.expect("request line");
            let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(id) = request.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let method = request["method"].as_str().unwrap_or_default();
            if method == "account/read" {
                writeln!(
                    stdout,
                    "{}",
                    json!({ "method": "account/updated", "params": { "authMode": "chatgpt", "planType": "plus" } })
                )
                .unwrap();
            }
            let response = match method {
                "initialize" => json!({ "id": id, "result": {} }),
                "account/read" => {
                    json!({ "id": id, "result": { "account": { "type": "chatgpt", "email": "fixture@example.invalid" } } })
                }
                "account/login/start" => {
                    json!({ "id": id, "result": { "loginId": "fixture-login", "userCode": "FIXTURE-CODE" } })
                }
                "account/login/cancel" | "account/logout" => json!({ "id": id, "result": {} }),
                _ => {
                    json!({ "id": id, "error": { "code": -32601, "message": "mock rejected request" } })
                }
            };
            writeln!(stdout, "{response}").unwrap();
            stdout.flush().unwrap();
        }
    }

    #[test]
    fn codex_account_client_handles_interleaved_updates_and_login_lifecycle() {
        let path = std::env::current_exe().expect("test executable");
        let mut command = Command::new(&path);
        command
            .args([
                "--exact",
                "features::ai::agent_management::tests::mock_codex_app_server",
                "--nocapture",
            ])
            .env("NYATERM_MOCK_CODEX_ACCOUNT_SERVER", "1");
        let mut client =
            CodexAccountClient::start_with_command(path, command, Arc::new(AtomicBool::new(false)))
                .expect("start mock app-server");
        let account = client
            .request("account/read", json!({ "refreshToken": false }))
            .expect("account response");
        assert_eq!(account["account"]["email"], "fixture@example.invalid");
        assert_eq!(
            client.poll_account_update().expect("interleaved update")["account"]["planType"],
            "plus"
        );
        let login = client
            .request(
                "account/login/start",
                json!({ "type": "chatgptDeviceCode" }),
            )
            .expect("login response");
        assert_eq!(login["loginId"], "fixture-login");
        client
            .request(
                "account/login/cancel",
                json!({ "loginId": "fixture-login" }),
            )
            .expect("cancel response");
        client
            .request("account/logout", json!({}))
            .expect("logout response");
        assert_eq!(
            client.request("unknown", json!({})).unwrap_err(),
            "mock rejected request"
        );
    }

    #[test]
    fn cancellation_interrupts_a_pending_account_response_and_reaps_the_client() {
        let path = std::env::current_exe().expect("test executable");
        let mut command = Command::new(&path);
        command
            .args([
                "--exact",
                "features::ai::agent_management::tests::mock_codex_app_server",
                "--nocapture",
            ])
            .env("NYATERM_MOCK_CODEX_ACCOUNT_SERVER", "1");
        let stopping = Arc::new(AtomicBool::new(false));
        let mut client =
            CodexAccountClient::start_with_command(path, command, Arc::clone(&stopping))
                .expect("start mock app-server");
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = client.wait_response(999);
            drop(client);
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        stopping.store(true, Ordering::Release);
        let error = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cancellation must interrupt the 20 second wait")
            .unwrap_err();
        assert!(error.contains("cancelled"));
        worker.join().unwrap();
    }

    #[test]
    fn account_notification_extracts_state_without_exposing_device_code() {
        let update = account_update_from_notification(&json!({
            "method": "account/updated",
            "params": { "authMode": "chatgpt", "planType": "plus" }
        }))
        .expect("account update");
        assert_eq!(update["account"]["type"], "chatgpt");
        assert_eq!(update["account"]["planType"], "plus");
        assert!(
            account_update_from_notification(&json!({
                "method": "account/login/completed",
                "params": { "userCode": "SECRET" }
            }))
            .is_none()
        );
    }

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

//! Codex account protocol; the caller owns scheduling and settings state.
use crate::external_process::{child_process::AgentChild, spawn_reader};
use crate::probe::hide_window;
use nyaterm_core::ai::{codex_initialize_request, codex_initialized_notification};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub enum CodexAccountRequest {
    Models,
    Account,
    Login { device_code: bool },
    CancelLogin { login_id: String },
    Logout,
}

impl std::fmt::Debug for CodexAccountRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Models => "Models",
            Self::Account => "Account",
            Self::Login { .. } => "Login",
            Self::CancelLogin { .. } => "CancelLogin([REDACTED])",
            Self::Logout => "Logout",
        })
    }
}

pub struct CodexAccountClient {
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
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn request(&mut self, request: CodexAccountRequest) -> Result<Value, String> {
        let (method, params) = match request {
            CodexAccountRequest::Models => ("model/list", json!({"limit": 100})),
            CodexAccountRequest::Account => ("account/read", json!({"refreshToken": false})),
            CodexAccountRequest::Login { device_code } => (
                "account/login/start",
                if device_code {
                    json!({"type": "chatgptDeviceCode"})
                } else {
                    json!({"type": "chatgpt", "useHostedLoginSuccessPage": true, "appBrand": "codex"})
                },
            ),
            CodexAccountRequest::CancelLogin { login_id } => {
                ("account/login/cancel", json!({"loginId": login_id}))
            }
            CodexAccountRequest::Logout => ("account/logout", json!({})),
        };
        self.request_raw(method, params)
    }

    pub fn start(path: PathBuf, stopping: Arc<AtomicBool>) -> Result<Self, String> {
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
        let reader = spawn_reader("nyaterm-codex-account-reader", move || {
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

    fn request_raw(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "id": id, "method": method, "params": params }))?;
        self.wait_response(id)
    }

    pub fn is_alive(&mut self) -> bool {
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

    pub fn poll_account_update(&mut self) -> Option<Value> {
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

#[cfg(test)]
mod tests {
    use super::{CodexAccountClient, CodexAccountRequest, account_update_from_notification};
    use serde_json::json;
    use std::io::{BufRead, Write};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
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
            match method {
                "account/read" => assert_eq!(request["params"], json!({"refreshToken":false})),
                "model/list" => assert_eq!(request["params"], json!({"limit":100})),
                "account/login/start" if request["params"]["type"] == "chatgpt" => {
                    assert_eq!(
                        request["params"],
                        json!({"type":"chatgpt", "useHostedLoginSuccessPage":true, "appBrand":"codex"})
                    );
                }
                "account/login/start" => {
                    assert_eq!(request["params"], json!({"type":"chatgptDeviceCode"}))
                }
                "account/login/cancel" => {
                    assert_eq!(request["params"], json!({"loginId":"fixture-login"}))
                }
                "account/logout" => assert_eq!(request["params"], json!({})),
                _ => {}
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
                "account::tests::mock_codex_app_server",
                "--nocapture",
            ])
            .env("NYATERM_MOCK_CODEX_ACCOUNT_SERVER", "1");
        let mut client =
            CodexAccountClient::start_with_command(path, command, Arc::new(AtomicBool::new(false)))
                .expect("start mock app-server");
        let account = client
            .request(CodexAccountRequest::Account)
            .expect("account response");
        assert_eq!(account["account"]["email"], "fixture@example.invalid");
        assert_eq!(
            client.poll_account_update().expect("interleaved update")["account"]["planType"],
            "plus"
        );
        let login = client
            .request(CodexAccountRequest::Login { device_code: true })
            .expect("login response");
        assert_eq!(login["loginId"], "fixture-login");
        client
            .request(CodexAccountRequest::Login { device_code: false })
            .unwrap();
        assert!(
            !format!(
                "{:?}",
                CodexAccountRequest::CancelLogin {
                    login_id: "private-fixture".into()
                }
            )
            .contains("private-fixture")
        );
        client
            .request(CodexAccountRequest::CancelLogin {
                login_id: "fixture-login".into(),
            })
            .expect("cancel response");
        client
            .request(CodexAccountRequest::Logout)
            .expect("logout response");
        assert_eq!(
            client.request(CodexAccountRequest::Models).unwrap_err(),
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
                "account::tests::mock_codex_app_server",
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
}

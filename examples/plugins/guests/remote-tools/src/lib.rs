use nyaterm_plugin_api::protocol::{HostRequest, PanelElement, SessionHandle, TaskEvent};
use nyaterm_plugin_api::{
    ActionInput, ActionResult, ErrorKind, Identity, Plugin, PluginError, Value, host,
};

#[derive(Default)]
struct RemoteTools {
    deploy: bool,
    session: Option<SessionHandle>,
    pending: Option<u64>,
    phase: u8,
    offset: u64,
    chunk_size: usize,
    root: String,
    local: String,
    remote: String,
    command: String,
    output: String,
}

fn error(message: &str) -> PluginError {
    PluginError {
        kind: ErrorKind::Failed,
        message: message.into(),
    }
}

fn argument(input: &ActionInput, name: &str, default: &str) -> String {
    input
        .arguments
        .iter()
        .find_map(|argument| match &argument.value {
            Value::Text(value) if argument.name == name => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| default.into())
}

impl RemoteTools {
    fn submit(&mut self, request: HostRequest, phase: u8) -> Result<(), PluginError> {
        self.pending = Some(
            host::submit(&request)
                .map_err(|_| error("Authorize the requested scope in extension settings"))?,
        );
        self.phase = phase;
        Ok(())
    }

    fn panel(&self, status: &str, log: Option<String>) {
        let mut elements = Vec::new();
        if self.deploy {
            for (id, label, value) in [
                ("local_root", "Local directory", &self.root),
                ("local_path", "Relative local file", &self.local),
                ("remote_path", "Remote destination", &self.remote),
                ("command", "Deployment command", &self.command),
            ] {
                elements.push(PanelElement::Input {
                    id: id.into(),
                    label: label.into(),
                    value: value.clone(),
                });
            }
            elements.push(PanelElement::Button {
                action: "deploy".into(),
                label: "Upload and deploy".into(),
            });
        } else {
            elements.push(PanelElement::Input {
                id: "remote_path".into(),
                label: "Log path".into(),
                value: self.remote.clone(),
            });
            elements.push(PanelElement::Button {
                action: "refresh".into(),
                label: "Read log".into(),
            });
        }
        elements.push(PanelElement::Button {
            action: "stop".into(),
            label: "Stop".into(),
        });
        elements.push(PanelElement::Text {
            text: status.into(),
        });
        if let Some(mut text) = log {
            let mut start = text.len().saturating_sub(128 * 1024);
            while !text.is_char_boundary(start) {
                start += 1;
            }
            if start > 0 {
                text.drain(..start);
            }
            elements.push(PanelElement::Log { text });
        }
        // The completion callback drains this UI task too.
        let _ = host::submit(&HostRequest::ShowPanel {
            session: self.session.clone(),
            id: "main".into(),
            title: if self.deploy {
                "Upload and deploy"
            } else {
                "Remote log"
            }
            .into(),
            elements,
        });
    }
}

impl Plugin for RemoteTools {
    fn initialize(&mut self, identity: Identity) -> Result<(), PluginError> {
        self.deploy = identity.id == "upload-deploy";
        self.remote = if self.deploy {
            "/tmp/deployment.bin"
        } else {
            "/var/log/syslog"
        }
        .into();
        Ok(())
    }

    fn invoke(&mut self, input: ActionInput) -> Result<ActionResult, PluginError> {
        if let Some(text) = input.text.as_deref() {
            if let Ok(context) = serde_json::from_str::<serde_json::Value>(text) {
                if let Ok(session) =
                    serde_json::from_value::<SessionHandle>(context["session"].clone())
                {
                    if self.pending.is_some() && self.session.as_ref() != Some(&session) {
                        return Err(error(
                            "This extension already has a task running in another session",
                        ));
                    }
                    self.session = Some(session);
                }
            }
        }
        match input.action.as_str() {
            "open" => {
                let context: serde_json::Value =
                    serde_json::from_str(input.text.as_deref().unwrap_or("{}"))
                        .map_err(|_| error("Open this tool from a session or remote file menu"))?;
                self.session = Some(
                    serde_json::from_value(context["session"].clone())
                        .map_err(|_| error("Open this tool from an SSH session menu"))?,
                );
                if !self.deploy
                    && let Some(path) = context["paths"][0].as_str()
                {
                    self.remote = path.into();
                }
                self.panel("Ready", None);
            }
            "stop" => {
                if let Some(task) = self.pending.take() {
                    let _ = host::cancel(task);
                }
                self.panel("Stopped", None);
            }
            "refresh" | "deploy" => {
                if self.pending.is_some() {
                    return Err(error("Stop the running operation first"));
                }
                let session = self
                    .session
                    .clone()
                    .ok_or_else(|| error("Open the tool from an SSH session menu"))?;
                self.remote = argument(&input, "remote_path", &self.remote);
                if input.action == "refresh" {
                    self.submit(
                        HostRequest::RemoteRead {
                            session,
                            path: self.remote.clone(),
                            offset: 0,
                            length: 128 * 1024,
                        },
                        0,
                    )?;
                } else {
                    self.root = argument(&input, "local_root", "");
                    self.local = argument(&input, "local_path", "");
                    self.command = argument(&input, "command", "");
                    if self.command.trim().is_empty() {
                        return Err(error("Enter a deployment command"));
                    }
                    self.offset = 0;
                    self.output.clear();
                    self.submit(
                        HostRequest::LocalRead {
                            root: self.root.clone(),
                            path: self.local.clone(),
                            offset: 0,
                            length: 256 * 1024,
                        },
                        1,
                    )?;
                }
                self.panel("Running", None);
            }
            _ => return Err(error("Unknown action")),
        }
        Ok(ActionResult::Text(String::new()))
    }

    fn task_ready(&mut self, task: u64) {
        let event = host::poll(task);
        if self.pending != Some(task) {
            return;
        }
        if let Ok(TaskEvent::Output { bytes }) = &event {
            self.output.push_str(&String::from_utf8_lossy(bytes));
            self.panel("Running", Some(self.output.clone()));
            return;
        }
        if matches!(event, Ok(TaskEvent::Pending)) {
            return;
        }
        self.pending = None;
        let Ok(TaskEvent::Completed { bytes, exit_code }) = event else {
            self.panel(
                "Operation failed or was cancelled; check access permissions",
                None,
            );
            return;
        };
        let session = self.session.clone().unwrap();
        let result = match self.phase {
            0 => {
                self.panel(
                    "Log loaded (first 128 KiB)",
                    Some(String::from_utf8_lossy(&bytes).into_owned()),
                );
                Ok(())
            }
            1 => {
                self.chunk_size = bytes.len();
                if bytes.is_empty() && self.offset > 0 {
                    self.submit(
                        HostRequest::RemoteExecute {
                            session,
                            command: self.command.clone(),
                        },
                        3,
                    )
                } else {
                    self.submit(
                        HostRequest::RemoteWrite {
                            session,
                            path: self.remote.clone(),
                            offset: self.offset,
                            bytes,
                        },
                        2,
                    )
                }
            }
            2 => {
                self.offset += self.chunk_size as u64;
                if self.chunk_size == 0 {
                    self.submit(
                        HostRequest::RemoteExecute {
                            session,
                            command: self.command.clone(),
                        },
                        3,
                    )
                } else {
                    self.submit(
                        HostRequest::LocalRead {
                            root: self.root.clone(),
                            path: self.local.clone(),
                            offset: self.offset,
                            length: 256 * 1024,
                        },
                        1,
                    )
                }
            }
            _ => {
                self.panel(
                    &format!("Deployment finished: exit {}", exit_code.unwrap_or(-1)),
                    Some(String::from_utf8_lossy(&bytes).into_owned()),
                );
                Ok(())
            }
        };
        if result.is_err() {
            self.panel("Operation needs additional access authorization", None);
        }
    }
}

nyaterm_plugin_api::register_plugin!(RemoteTools);

use nyaterm_plugin_api::{ActionInput, ActionResult, Identity, Plugin, PluginError};

#[derive(Default)]
struct Fixture {
    calls: u32,
    task: Option<u64>,
}

impl Plugin for Fixture {
    fn initialize(&mut self, identity: Identity) -> Result<(), PluginError> {
        if identity.id == "init-loop" {
            loop {
                std::hint::black_box(&mut self.calls);
            }
        }
        if identity.id == "init-fail" {
            panic!("fixture initialization trap");
        }
        Ok(())
    }

    fn invoke(&mut self, input: ActionInput) -> Result<ActionResult, PluginError> {
        match input.action.as_str() {
            "host-submit" => {
                self.task = Some(
                    nyaterm_plugin_api::host::submit(
                        &nyaterm_plugin_api::protocol::HostRequest::StorageWrite {
                            key: "probe".into(),
                            bytes: b"sdk-host-round-trip".to_vec(),
                        },
                    )
                    .map_err(|_| PluginError {
                        kind: nyaterm_plugin_api::ErrorKind::Failed,
                        message: "submit failed".into(),
                    })?,
                );
                Ok(ActionResult::Text("submitted".into()))
            }
            "host-poll" => {
                let task = self.task.unwrap();
                let result = nyaterm_plugin_api::host::poll(task).map_err(|_| PluginError {
                    kind: nyaterm_plugin_api::ErrorKind::Failed,
                    message: "poll failed".into(),
                })?;
                Ok(ActionResult::Text(
                    match result {
                        nyaterm_plugin_api::protocol::TaskEvent::Completed { .. } => "complete",
                        nyaterm_plugin_api::protocol::TaskEvent::Pending => "pending",
                        _ => "failed",
                    }
                    .into(),
                ))
            }
            "loop" => loop {
                std::hint::black_box(&mut self.calls);
            },
            "memory" => {
                let mut blocks = Vec::new();
                loop {
                    blocks.push(vec![1u8; 1024 * 1024]);
                    std::hint::black_box(&blocks);
                }
            }
            "trap" => panic!("fixture trap"),
            "oversize" => Ok(ActionResult::Text("x".repeat(300 * 1024))),
            "controls" => Ok(ActionResult::Text("\x1b[31m".into())),
            _ => {
                self.calls += 1;
                Ok(ActionResult::Text(self.calls.to_string()))
            }
        }
    }
}

nyaterm_plugin_api::register_plugin!(Fixture);

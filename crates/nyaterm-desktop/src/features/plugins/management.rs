use gpui::{Context, Task};
use nyaterm_core::plugins::management::PluginKey;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_plugin_host::native::NativeAction;
use nyaterm_plugin_host::service::{PluginOperation, PluginService};
use std::sync::Arc;

use crate::features::plugins::process::PluginProcess;

#[derive(Clone)]
pub(super) enum ManagementAction {
    Native(
        nyaterm_core::plugins::native::NativeComponentId,
        NativeAction,
    ),
    Wasm(PluginOperation),
    Refresh,
    UpdateAll,
}

pub(super) struct ManagementJob {
    pub action: ManagementAction,
    pub task: Option<Task<()>>,
    pub error: Option<PluginError>,
}

impl ManagementAction {
    pub fn key(&self) -> String {
        match self {
            Self::Native(id, _) => id.plugin_id(),
            Self::Wasm(operation) => match operation {
                PluginOperation::InstallFromStore { id }
                | PluginOperation::Update { id, .. }
                | PluginOperation::Enable { id, .. }
                | PluginOperation::Reload { id }
                | PluginOperation::Uninstall { id }
                | PluginOperation::Authorize { id, .. }
                | PluginOperation::RevokeAuthorizations { id } => id.clone(),
                _ => "\0global".into(),
            },
            Self::Refresh | Self::UpdateAll => "\0global".into(),
        }
    }
}

async fn execute(service: &Arc<PluginService>, action: ManagementAction) -> PluginResult<()> {
    let interrupted = || PluginError::new(ErrorCode::Shutdown, "Plugin service stopped");
    match action {
        ManagementAction::Native(id, action) => service
            .native
            .submit(id, action)?
            .await
            .map_err(|_| interrupted())?,
        ManagementAction::Wasm(operation) => service
            .submit(operation)?
            .await
            .map_err(|_| interrupted())?
            .map(|_| ()),
        ManagementAction::Refresh | ManagementAction::UpdateAll => unreachable!(),
    }
}

impl PluginProcess {
    pub(super) fn job_busy(&self, id: &str) -> bool {
        self.jobs.get(id).is_some_and(|job| job.task.is_some())
    }

    pub(super) fn manage(&mut self, action: ManagementAction, cx: &mut Context<Self>) {
        let key = action.key();
        if self.job_busy(&key) || self.job_busy("\0global") {
            return;
        }
        let Some(service) = self.service() else {
            return;
        };
        let update_all = matches!(action, ManagementAction::UpdateAll);
        let batch = matches!(
            action,
            ManagementAction::Refresh | ManagementAction::UpdateAll
        );
        // A batch owns the store worker until its sequential operations finish.
        if batch && self.jobs.values().any(|job| job.task.is_some()) {
            return;
        }
        let saved_action = action.clone();
        let job_key = key.clone();
        self.batch_summary = None;
        #[cfg(test)]
        let batch_fixture = self.batch_fixture.take();
        let task = cx.spawn(async move |this, cx| {
            let actions = match action {
                ManagementAction::Refresh => {
                    let mut actions = vec![ManagementAction::Wasm(PluginOperation::RefreshStore)];
                    actions.extend(
                        nyaterm_core::plugins::native::NativeComponentId::ALL
                            .into_iter()
                            .filter(|id| id.supported())
                            .map(|id| ManagementAction::Native(id, NativeAction::Probe)),
                    );
                    actions
                }
                ManagementAction::UpdateAll => this
                    .update(cx, |this, _| {
                        this.refresh();
                        super::projection::entries(this)
                            .into_iter()
                            .filter(|entry| entry.update_available && entry.compatible)
                            .map(|entry| match entry.key {
                                PluginKey::Native(id) => {
                                    ManagementAction::Native(id, NativeAction::Install)
                                }
                                PluginKey::Wasm(id) => {
                                    ManagementAction::Wasm(PluginOperation::InstallFromStore { id })
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                action => vec![action],
            };
            #[cfg(test)]
            let actions = batch_fixture.unwrap_or(actions);
            let mut succeeded = 0;
            let mut failed = 0;
            let mut last_error = None;
            for action in actions {
                let item_key = action.key();
                if update_all
                    && !this
                        .update(cx, |this, _| {
                            this.refresh();
                            super::projection::entries(this).iter().any(|entry| {
                                entry.key.id() == item_key
                                    && entry.installed
                                    && entry.update_available
                                    && entry.compatible
                                    && !entry.pending_uninstall
                                    && !entry.busy
                            })
                        })
                        .unwrap_or(false)
                {
                    continue;
                }
                if batch && item_key != "\0global" {
                    let _ = this.update(cx, |this, _| {
                        this.jobs.remove(&item_key);
                    });
                }
                let result = execute(&service, action.clone()).await;
                match &result {
                    Ok(()) => succeeded += 1,
                    Err(error) if error.code != ErrorCode::Cancelled => {
                        failed += 1;
                        last_error = Some(error.clone());
                    }
                    Err(_) => {}
                }
                let _ = this.update(cx, |this, cx| {
                    this.refresh();
                    if batch && item_key != "\0global" {
                        this.jobs.insert(
                            item_key,
                            ManagementJob {
                                action,
                                task: None,
                                error: result
                                    .err()
                                    .filter(|error| error.code != ErrorCode::Cancelled),
                            },
                        );
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(job) = this.jobs.get_mut(&job_key) {
                    if let Some(task) = job.task.take() {
                        task.detach();
                    }
                    job.error = last_error;
                }
                if batch {
                    this.batch_summary = Some((succeeded, failed));
                }
                cx.notify();
            });
        });
        self.jobs.insert(
            key,
            ManagementJob {
                action: saved_action,
                task: Some(task),
                error: None,
            },
        );
        cx.notify();
    }

    pub(super) fn retry(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(action) = self.jobs.get(key).map(|job| job.action.clone()) {
            self.manage(action, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::features::plugins::management::ManagementAction;
    use crate::features::plugins::process::PluginProcess;
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext, TestAppContext};
    use nyaterm_core::runtime::{AppRuntime, RuntimeMode};
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_plugin_host::service::PluginOperation;
    use std::time::{Duration, Instant};

    #[test]
    fn batch_continues_after_failure_and_global_items_do_not_cancel_the_batch_owner() {
        let root = TestTempDir::new("nyaterm-plugin-batch");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(
            crate_root.join("../../examples/plugins/diagnostic-command/plugin.toml"),
            source.join("plugin.toml"),
        )
        .unwrap();
        std::fs::copy(
            crate_root.join("../nyaterm-plugin-host/tests/fixtures/diagnostic-command.wasm"),
            source.join("plugin.wasm"),
        )
        .unwrap();
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let process = cx.new(|cx| PluginProcess::new(runtime, cx));
        cx.update_entity(&process, |process, cx| {
            process.batch_fixture = Some(vec![
                ManagementAction::Wasm(PluginOperation::Reload {
                    id: "missing".into(),
                }),
                ManagementAction::Wasm(PluginOperation::Install {
                    source,
                    development: false,
                }),
            ]);
            process.manage(ManagementAction::Refresh, cx);
            assert!(process.job_busy("\0global"));
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while cx.read(|cx| process.read(cx).job_busy("\0global")) {
            cx.run_until_parked();
            assert!(Instant::now() < deadline, "batch did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.read(|cx| {
            let process = process.read(cx);
            assert_eq!(process.batch_summary, Some((1, 1)));
            assert!(process.jobs["missing"].error.is_some());
            assert!(
                process
                    .snapshot
                    .plugins
                    .iter()
                    .any(|plugin| plugin.id == "diagnostic-command")
            );
            process.service().unwrap().shutdown();
        });
    }
}

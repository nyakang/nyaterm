use futures::StreamExt;
use gpui::Context;
use nyaterm_core::runtime::AppRuntime;
use nyaterm_plugin_host::manager::CatalogSnapshot;
use nyaterm_plugin_host::service::PluginService;
use std::sync::Arc;

/// Only DesktopController constructs a live process service. Window entities
/// observe this immutable presentation, rather than creating their own runtimes.
pub(crate) struct PluginProcess {
    pub(in crate::features) service: Option<Arc<PluginService>>,
    pub(in crate::features) snapshot: CatalogSnapshot,
}

impl PluginProcess {
    pub(crate) fn new(runtime: AppRuntime, cx: &mut Context<Self>) -> Self {
        match PluginService::start(runtime) {
            Ok((service, mut events)) => {
                cx.spawn(async move |this, cx| {
                    while events.next().await.is_some() {
                        if this
                            .update(cx, |this, cx| {
                                if let Some(service) = &this.service {
                                    this.snapshot = service.snapshot();
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                Self {
                    service: Some(service),
                    snapshot: CatalogSnapshot::default(),
                }
            }
            Err(error) => Self {
                service: None,
                snapshot: CatalogSnapshot {
                    startup_error: Some(error),
                    ..CatalogSnapshot::default()
                },
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            service: None,
            snapshot: CatalogSnapshot::default(),
        }
    }

    pub(crate) fn service(&self) -> Option<Arc<PluginService>> {
        self.service.clone()
    }
    #[cfg(test)]
    pub(crate) fn restart(&mut self, runtime: AppRuntime, cx: &mut Context<Self>) {
        *self = Self::new(runtime, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use crate::app_shell::{AppShellStartup, DesktopController};
    use crate::features::plugins::state::PluginFeatureState;
    use crate::features::test_support::app_with_visible_local_session;
    use futures::executor::block_on;
    use gpui::AppContext;
    use nyaterm_core::plugins::invocation::{ActionInput, ActionResult};
    use nyaterm_core::runtime::{AppRuntime, RuntimeMode};
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_plugin_host::service::{OperationReply, PluginOperation};
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn workspace_features_share_controller_service_and_closing_one_keeps_other_alive() {
        let mut cx = gpui::TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-plugin-multiwindow");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let process = cx.read(|cx| controller.read(cx).plugin_process());
        let first_root = TestTempDir::new("nyaterm-plugin-window-one");
        let second_root = TestTempDir::new("nyaterm-plugin-window-two");
        let first = app_with_visible_local_session(&mut cx, first_root.path(), "first");
        let second = app_with_visible_local_session(&mut cx, second_root.path(), "second");
        for app in [&first, &second] {
            cx.update_entity(app, |app, cx| {
                app.plugins = PluginFeatureState::new(cx.entity().downgrade(), process.clone(), cx);
            });
        }
        assert_ne!(
            cx.read(|cx| first.read(cx).plugins.panel.entity_id()),
            cx.read(|cx| second.read(cx).plugins.panel.entity_id()),
        );
        let one = cx.read(|cx| {
            first
                .read(cx)
                .plugins
                .panel
                .read(cx)
                .process
                .read(cx)
                .service()
                .unwrap()
        });
        let two = cx.read(|cx| {
            second
                .read(cx)
                .plugins
                .panel
                .read(cx)
                .process
                .read(cx)
                .service()
                .unwrap()
        });
        assert!(Arc::ptr_eq(&one, &two));
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../nyaterm-plugin-host/tests/fixtures");
        let package = root.join("example");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/plugins/diagnostic-command/plugin.toml"),
            package.join("plugin.toml"),
        )
        .unwrap();
        std::fs::copy(
            source.join("diagnostic-command.wasm"),
            package.join("plugin.wasm"),
        )
        .unwrap();
        block_on(
            one.submit(PluginOperation::Install {
                source: package,
                development: false,
            })
            .unwrap(),
        )
        .unwrap()
        .unwrap();
        let invoke = |service: &nyaterm_plugin_host::service::PluginService| {
            let reply = block_on(
                service
                    .submit(PluginOperation::Invoke {
                        contribution_id: "diagnostic-command:diagnose".into(),
                        expected_revision: service.snapshot().contributions[0].revision,
                        input: ActionInput::default(),
                    })
                    .unwrap(),
            )
            .unwrap()
            .unwrap();
            match reply {
                OperationReply::Invocation(invocation) => block_on(invocation.result()).unwrap(),
                _ => panic!("missing invocation"),
            }
        };
        assert!(
            matches!(invoke(&one), ActionResult::Command { title, .. } if title == "Diagnostic draft 1")
        );
        let other_package = root.join("other-example");
        std::fs::create_dir_all(&other_package).unwrap();
        let manifest = std::fs::read_to_string(root.join("example/plugin.toml")).unwrap();
        std::fs::write(
            other_package.join("plugin.toml"),
            manifest.replace("diagnostic-command", "other-plugin"),
        )
        .unwrap();
        std::fs::copy(
            root.join("example/plugin.wasm"),
            other_package.join("plugin.wasm"),
        )
        .unwrap();
        let accepted = one
            .submit(PluginOperation::Install {
                source: other_package,
                development: false,
            })
            .unwrap();
        drop(first);
        drop(accepted);
        assert!(
            matches!(invoke(&two), ActionResult::Command { title, .. } if title == "Diagnostic draft 2")
        );
        // Other-plugin preparation no longer blocks this window's invocation.
        // An ordered management request verifies the abandoned install commits.
        block_on(
            two.submit(PluginOperation::Reload {
                id: "other-plugin".into(),
            })
            .unwrap(),
        )
        .unwrap()
        .unwrap();
        assert!(
            two.snapshot()
                .contributions
                .iter()
                .any(|c| c.plugin_id == "other-plugin")
        );
        one.shutdown();
        assert!(two.snapshot().stopped);
        // An explicit service restart keeps existing windows observing the same Entity.
        cx.update_entity(&controller, |controller, cx| controller.restart_plugins(cx));
        let restarted = cx.read(|cx| process.read(cx).service().unwrap());
        assert!(!Arc::ptr_eq(&one, &restarted));
        block_on(
            restarted
                .submit(PluginOperation::Reload {
                    id: "diagnostic-command".into(),
                })
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        cx.run_until_parked();
        assert!(
            matches!(invoke(&restarted), ActionResult::Command { title, .. } if title == "Diagnostic draft 1")
        );
        assert!(!cx.read(|cx| {
            second
                .read(cx)
                .plugins
                .panel
                .read(cx)
                .process
                .read(cx)
                .snapshot
                .stopped
        }));
        restarted.shutdown();
        drop(second);
        drop(two);
        drop(one);
        drop(process);
        drop(controller);
        drop(cx);
    }
}

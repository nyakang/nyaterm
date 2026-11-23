//! TFTP configuration actions: directory prompt, draft reset and save/restart.
//!
//! Layout lives in [`form`]; field rules live in [`validation`].

mod form;
mod validation;

use gpui::{App, Context, PathPromptOptions, SharedString};
use nyaterm_core::models::networktools::{
    DEFAULT_TFTP_BIND_ADDRESS, NetworkToolsSettings, TftpSettings,
};
use nyaterm_store::StoreDomain;
use nyaterm_transport::networktools::NetworkToolKind;
use nyaterm_ui::notification::NyaNotificationKind;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::ids;
use nyaterm_transport::networktools::runtime::RuntimeCommand;

const KIND: NetworkToolKind = NetworkToolKind::Tftp;

/// Input ids are namespaced by service so two configuration windows can use the
/// shared text input registry without sharing field state.
pub(super) fn config_input_id(field: &str) -> String {
    ids::config_input(KIND, field)
}

impl NyaTermApp {
    fn prompt_tftp_root_dir(&mut self, cx: &mut Context<Self>) {
        if !self.networktools.is_config_open(KIND) {
            return;
        }
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("networktools.common.field.rootDir").to_string(),
            )),
        };
        let receiver = cx.prompt_for_paths(options);
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                let Some(path) = selected else {
                    return;
                };
                if !this.networktools.is_config_open(KIND) {
                    return;
                }
                let path = path.display().to_string();
                this.networktools.tftp_draft_mut().root_dir = path.clone();
                this.reset_text_input(&config_input_id("root"), &path, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reset_tftp_config_draft(&mut self, cx: &mut Context<Self>) {
        let mut defaults = TftpSettings::default();
        if let Some(directory) = super::default_networktools_share_dir() {
            defaults.root_dir = directory.display().to_string();
        }
        let draft = defaults.clone();
        self.networktools.set_tftp_draft(draft);
        self.reset_text_input(&config_input_id("port"), &defaults.port.to_string(), cx);
        self.reset_text_input(&config_input_id("root"), &defaults.root_dir, cx);
        self.reset_text_input(
            &config_input_id("block-size"),
            &defaults.block_size.to_string(),
            cx,
        );
        self.reset_text_input(
            &config_input_id("timeout"),
            &defaults.timeout_secs.to_string(),
            cx,
        );
        self.reset_text_input(
            &config_input_id("concurrency"),
            &defaults.max_concurrency.to_string(),
            cx,
        );
        self.networktools.set_status_message(KIND, None);
        cx.notify();
    }

    pub(in crate::features) fn save_tftp_config(&mut self, cx: &mut Context<Self>) {
        self.ensure_networktools_pump(cx);
        if self.networktools.is_busy(KIND) || !self.networktools.is_config_open(KIND) {
            return;
        }
        let mut draft = self.networktools.tftp_draft();
        let port_value = self.config_input_value("port", cx).unwrap_or_default();
        let root_value = self.config_input_value("root", cx).unwrap_or_default();
        let block_value = self
            .config_input_value("block-size", cx)
            .unwrap_or_default();
        let timeout_value = self.config_input_value("timeout", cx).unwrap_or_default();
        let concurrency_value = self
            .config_input_value("concurrency", cx)
            .unwrap_or_default();
        // Render-time validation has already disabled Save for pure parse
        // errors; repeat it here because the button state can lag behind the
        // latest keystroke.
        let errors = validation::field_errors(
            &draft.bind_address,
            &port_value,
            &root_value,
            &block_value,
            &timeout_value,
            &concurrency_value,
            None,
        );
        if errors.any() {
            cx.notify();
            return;
        }

        let root = root_value.trim().to_string();
        let (Ok(port), Ok(block_size), Ok(timeout_secs), Ok(max_concurrency)) = (
            port_value.trim().parse::<u16>(),
            block_value.trim().parse::<u16>(),
            timeout_value.trim().parse::<u8>(),
            concurrency_value.trim().parse::<u16>(),
        ) else {
            cx.notify();
            return;
        };
        draft.bind_address = if draft.bind_address.trim().is_empty() {
            DEFAULT_TFTP_BIND_ADDRESS.to_string()
        } else {
            draft.bind_address.trim().to_string()
        };
        draft.root_dir = root;
        draft.port = port;
        draft.block_size = block_size;
        draft.timeout_secs = timeout_secs;
        draft.max_concurrency = max_concurrency;

        let result = NetworkToolsSettings {
            tftp: draft.clone(),
        }
        .validate()
        .map_err(|error| {
            t!(
                "networktools.common.error.invalid",
                message = error.to_string()
            )
            .to_string()
        });

        if let Err(message) = result {
            self.networktools.set_status_message(KIND, Some(message));
            cx.notify();
            return;
        }
        if !self.networktools.begin_operation(KIND) {
            return;
        }
        let revision = self.networktools.config_revision(KIND);
        let was_running = self.networktools.is_running(KIND);
        let probe = cx.background_executor().spawn(async move {
            let usable = std::fs::read_dir(&draft.root_dir).is_ok();
            (draft, usable)
        });
        cx.spawn(async move |this, cx| {
            let (draft, usable) = probe.await;
            let _ = this.update(cx, |this, cx| {
                if !this.networktools.config_matches(KIND, revision) {
                    this.networktools.finish_operation(KIND);
                    cx.notify();
                    return;
                }
                this.networktools
                    .set_root_dir_probe(KIND, draft.root_dir.clone(), usable);
                if !usable {
                    this.networktools.finish_operation(KIND);
                    cx.notify();
                    return;
                }
                this.persist_tftp_config(draft, revision, was_running, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn persist_tftp_config(
        &mut self,
        draft: TftpSettings,
        revision: u64,
        was_running: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = NetworkToolsSettings { tftp: draft };
        let queued = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Settings, move |store| {
                store.save_networktools_settings(settings)
            }),
            move |this, event, cx| {
                this.apply_tftp_save_result(event.outcome, revision, was_running, cx)
            },
            cx,
        );
        if !queued {
            self.networktools.finish_operation(KIND);
            self.networktools.set_status_message(
                KIND,
                Some(t!("networktools.common.status.saveFailed").to_string()),
            );
            cx.notify();
        }
    }

    fn apply_tftp_save_result(
        &mut self,
        outcome: Result<NetworkToolsSettings, nyaterm_store::StoreOperationError>,
        revision: u64,
        was_running: bool,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(settings) => {
                let draft = settings.tftp.clone();
                // Persistence succeeds before the draft becomes saved
                // state or a running service is interrupted.
                self.networktools.apply_saved_settings(settings);
                if self.networktools.config_matches(KIND, revision) {
                    self.networktools.set_tftp_draft(draft);
                    self.networktools.commit_config_edits(KIND);
                }
                if was_running {
                    self.networktools.close_on_restart(KIND, revision);
                    let command =
                        RuntimeCommand::Restart(KIND, self.networktools_runtime_settings());
                    if let Err(message) = self.networktools.submit(command) {
                        self.networktools.take_close_on_restart(KIND);
                        self.networktools.finish_operation(KIND);
                        self.networktools_operation_failed(KIND, message, cx);
                    }
                } else {
                    self.networktools.finish_operation(KIND);
                    self.networktools.set_status_message(KIND, None);
                    if self.networktools.config_matches(KIND, revision) {
                        self.end_networktools_config_session(KIND, false, cx);
                    }
                }
            }
            Err(error) => {
                self.networktools.finish_operation(KIND);
                let message = format!("{}: {error}", t!("networktools.common.status.saveFailed"));
                self.networktools
                    .set_status_message(KIND, Some(message.clone()));
                self.notify_background_operation(
                    ids::status_notification(KIND),
                    NyaNotificationKind::Error,
                    message,
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn config_input_value(&self, field: &str, cx: &App) -> Option<String> {
        self.existing_text_input(config_input_id(field))
            .map(|input| input.read(cx).value(cx))
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, Entity, TestAppContext};
    use nyaterm_core::models::networktools::{NetworkToolsSettings, TftpSettings};
    use nyaterm_core::test_support::TestTempDir;

    use super::KIND;
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;

    fn wait_for_operation(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let busy = cx.update_entity(app, |app, cx| {
                app.drain_networktools_events(cx);
                app.networktools.is_busy(KIND)
            });
            if !busy {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "network tools operation did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn start(cx: &mut TestAppContext, dir: &TestTempDir) -> Entity<NyaTermApp> {
        let app = app_with_visible_local_session(cx, dir.path(), "networktools-save");
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        cx.update_entity(&app, |app, cx| {
            app.networktools.apply_saved_settings(NetworkToolsSettings {
                tftp: TftpSettings {
                    bind_address: "127.0.0.1".into(),
                    port,
                    root_dir: dir.path().display().to_string(),
                    ..TftpSettings::default()
                },
            });
            app.toggle_networktools_service(KIND, cx);
        });
        wait_for_operation(cx, &app);
        cx.update_entity(&app, |app, _| {
            assert!(app.networktools.is_running(KIND));
            app.networktools.begin_window_open(KIND);
            app.networktools.begin_config_edit(KIND);
        });
        app
    }

    fn complete_save(cx: &mut TestAppContext, app: &Entity<NyaTermApp>, draft: TftpSettings) {
        let (task, revision) = cx.update_entity(app, |app, _| {
            let revision = app.networktools.config_revision(KIND);
            assert!(app.networktools.begin_operation(KIND));
            let task = app
                .store_ui
                .try_submit(
                    0,
                    nyaterm_store::store_mutation(
                        nyaterm_store::StoreDomain::Settings,
                        move |store| {
                            store.save_networktools_settings(NetworkToolsSettings { tftp: draft })
                        },
                    ),
                )
                .unwrap();
            (task, revision)
        });
        let event = futures::executor::block_on(task);
        cx.update_entity(app, |app, cx| {
            app.apply_tftp_save_result(event.outcome, revision, true, cx)
        });
    }

    fn shutdown(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) {
        let cleanup = cx.update_entity(app, |app, cx| app.shutdown_blocking_jobs(cx));
        cleanup.detach();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn failed_save_preserves_saved_settings_the_running_service_and_the_window(
        cx: &mut TestAppContext,
    ) {
        let dir = TestTempDir::new("nyaterm-networktools-save-failure");
        let app = start(cx, &dir);
        let before = cx.update_entity(&app, |app, _| app.networktools.settings().clone());
        let mut invalid = before.tftp.clone();
        invalid.port = 0; // The real store rejects before changing its document.
        complete_save(cx, &app, invalid);
        wait_for_operation(cx, &app);
        cx.update_entity(&app, |app, _| {
            assert_eq!(app.networktools.settings(), &before);
            assert!(app.networktools.is_running(KIND));
            assert!(app.networktools.is_config_open(KIND));
            assert!(app.networktools.status_message(KIND).is_some());
        });
        shutdown(cx, &app);
    }

    #[gpui::test]
    fn restart_failure_keeps_the_saved_config_and_open_window_and_releases_old_listener(
        cx: &mut TestAppContext,
    ) {
        let dir = TestTempDir::new("nyaterm-networktools-restart-failure");
        let app = start(cx, &dir);
        let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
        let new_port = occupied.local_addr().unwrap().port();
        let mut draft = cx.update_entity(&app, |app, _| app.networktools.settings().tftp.clone());
        let old_port = draft.port;
        draft.port = new_port;
        complete_save(cx, &app, draft);
        wait_for_operation(cx, &app);
        cx.update_entity(&app, |app, _| {
            assert_eq!(app.networktools.settings().tftp.port, new_port);
            assert!(!app.networktools.is_running(KIND));
            assert!(app.networktools.is_config_open(KIND));
            assert!(app.networktools.status_message(KIND).is_some());
        });
        assert!(UdpSocket::bind(("127.0.0.1", old_port)).is_ok());
        shutdown(cx, &app);
    }
}

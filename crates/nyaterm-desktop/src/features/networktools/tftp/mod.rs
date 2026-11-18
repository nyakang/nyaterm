//! TFTP configuration actions: directory prompt, draft reset and save/restart.
//!
//! Layout lives in [`form`]; field rules live in [`validation`].

mod form;
mod validation;

use gpui::{App, Context, PathPromptOptions, SharedString};
use nyaterm_core::{DEFAULT_TFTP_BIND_ADDRESS, NetworkToolsSettings, TftpSettings};
use nyaterm_store::StoreDomain;
use nyaterm_transport::networktools::NetworkToolKind;
use nyaterm_ui::notification::NyaNotificationKind;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::ids;

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
        // Existence and readability are checked here, never in the render path.
        let root_usable = std::fs::read_dir(&root).is_ok();
        self.networktools
            .set_root_dir_probe(KIND, root.clone(), root_usable);
        if !root_usable {
            cx.notify();
            return;
        }

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

        match result {
            Ok(()) => {
                let was_running = self.networktools.is_running(KIND);
                self.networktools.set_tftp_draft(draft);
                self.networktools.commit_config_edits(KIND);
                self.persist_networktools_settings(cx);
                // Saved settings only take effect on the listener at start, so a
                // running service is stopped (which joins its listener thread)
                // and bound again with the new values. A failed restart leaves
                // the window open with the reason and no stale or half-started
                // listener behind.
                if was_running {
                    self.networktools.registry.stop(KIND);
                    self.networktools.clear_transfers(KIND);
                    match self.start_networktools_service(KIND) {
                        Ok(()) => self.networktools.set_status_message(KIND, None),
                        Err(message) => {
                            let text = t!(
                                "networktools.common.status.restartFailed",
                                service = super::service_label(KIND),
                                message = message
                            )
                            .to_string();
                            self.networktools
                                .set_status_message(KIND, Some(text.clone()));
                            self.notify_background_operation(
                                ids::status_notification(KIND),
                                NyaNotificationKind::Error,
                                text,
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                    }
                }
                self.end_networktools_config_session(KIND, false, cx);
            }
            Err(message) => {
                self.networktools
                    .set_status_message(KIND, Some(message.clone()));
                self.notify_background_operation(
                    ids::status_notification(KIND),
                    NyaNotificationKind::Error,
                    message,
                    cx,
                );
                cx.notify();
            }
        }
    }

    fn persist_networktools_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self.networktools.settings().clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Settings, move |store| {
                store.save_networktools_settings(settings)
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(_) => this.networktools.set_status_message(KIND, None),
                    Err(error) => {
                        let message =
                            format!("{}: {error}", t!("networktools.common.status.saveFailed"));
                        this.networktools
                            .set_status_message(KIND, Some(message.clone()));
                        this.notify_background_operation(
                            ids::status_notification(KIND),
                            NyaNotificationKind::Error,
                            message,
                            cx,
                        );
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    fn config_input_value(&self, field: &str, cx: &App) -> Option<String> {
        self.existing_text_input(config_input_id(field))
            .map(|input| input.read(cx).value(cx))
    }
}

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, IntoElement as _, ParentElement as _, PathPromptOptions, Styled as _, Window, div,
};
use nyaterm_core::activation::{ExternalConnectionRequest, RawActivationArg};
use nyaterm_ui::{NyaScrollable as _, NyaSwitch};

use super::spice_file::ImportedSpiceFile;
use super::state::ExternalDialog;
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;

impl NyaTermApp {
    pub(in crate::features) fn prompt_spice_connection_file(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(rust_i18n::t!("spice.openFile").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(path) = paths.into_iter().next() {
                        let accepted =
                            this.activation
                                .enqueue(ExternalConnectionRequest::SpiceFile {
                                    path: RawActivationArg::from_os_string(path.into_os_string()),
                                });
                        if !accepted {
                            this.shell
                                .set_status(rust_i18n::t!("externalOpen.queueFull").to_string());
                        }
                        this.ensure_external_activation_clock(cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn load_spice_file(&mut self, path: RawActivationArg, cx: &mut Context<Self>) {
        self.activation.waiting_for_start = false;
        let token = nyaterm_core::uuid();
        self.activation.spice_loading = Some(token.clone());
        let task = self.blocking_jobs.submit_task("spice-file-read", move |_| {
            let path = path.to_os_string().ok_or("read")?;
            ImportedSpiceFile::read(path.into())
        });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if this.activation.spice_loading.as_ref() != Some(&token) {
                    return;
                }
                this.activation.spice_loading = None;
                match result {
                    Some(imported) => {
                        let fullscreen = imported.document.fullscreen;
                        this.activation.dialog = Some(ExternalDialog::Spice {
                            imported: Arc::new(imported),
                            delete_source: false,
                            fullscreen,
                        });
                    }
                    None => this
                        .shell
                        .set_status(rust_i18n::t!("spice.fileInvalid").to_string()),
                }
                this.ensure_external_activation_clock(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_spice_import_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form_dialog(
            (
                rust_i18n::t!("spice.openFile").to_string(),
                560.,
                rust_i18n::t!("savedConnections.connect").to_string(),
                |app: &mut Self, _, cx| {
                    let Some(ExternalDialog::Spice {
                        imported,
                        delete_source,
                        fullscreen,
                    }) = app.activation.dialog.as_ref()
                    else {
                        return div().into_any_element();
                    };
                    let document = &imported.document;
                    let count: usize = document.unhandled.values().map(|group| group.len()).sum();
                    let mut content = div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .max_h(gpui::px(400.))
                        .child(rust_i18n::t!("spice.temporaryNotice").to_string())
                        .child(if document.settings.security.require_tls {
                            rust_i18n::t!("spice.tlsNotice").to_string()
                        } else {
                            rust_i18n::t!("spice.plaintextNotice").to_string()
                        });
                    let host = document.endpoint.host.expose_secret();
                    let visible_host = if !host.contains(':')
                        || host
                            .trim_start_matches('[')
                            .trim_end_matches(']')
                            .parse::<std::net::Ipv6Addr>()
                            .is_ok()
                    {
                        host.to_owned()
                    } else {
                        rust_i18n::t!("spice.ticketEndpoint").to_string()
                    };
                    content = content.child(
                        rust_i18n::t!(
                            "spice.importTarget",
                            host = visible_host,
                            port = document
                                .endpoint
                                .tls_port
                                .or(document.endpoint.port)
                                .unwrap_or_default()
                        )
                        .to_string(),
                    );
                    if !document.title.is_empty() {
                        content = content.child(
                            document
                                .title
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(160)
                                .collect::<String>(),
                        );
                    }
                    if count > 0 {
                        let names = document
                            .unhandled
                            .iter()
                            .flat_map(|(group, fields)| {
                                fields.keys().map(move |key| format!("{group}.{key}"))
                            })
                            .take(16)
                            .collect::<Vec<_>>()
                            .join(", ");
                        content = content.child(
                            rust_i18n::t!("spice.unsupportedOptions", count = count, names = names)
                                .to_string(),
                        );
                    }
                    if document.delete_after_open {
                        content = content.child(rust_i18n::t!("spice.deleteRequested").to_string());
                    }
                    content
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .gap_3()
                                .child(rust_i18n::t!("spice.fullscreen").to_string())
                                .child(
                                    NyaSwitch::new("spice-import-fullscreen")
                                        .checked(*fullscreen)
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            if let Some(ExternalDialog::Spice {
                                                fullscreen, ..
                                            }) = app.activation.dialog.as_mut()
                                            {
                                                *fullscreen = !*fullscreen;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .gap_3()
                                .child(rust_i18n::t!("spice.deleteSource").to_string())
                                .child(
                                    NyaSwitch::new("spice-import-delete")
                                        .checked(*delete_source)
                                        .disabled(!ImportedSpiceFile::source_deletion_supported())
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            if let Some(ExternalDialog::Spice {
                                                delete_source,
                                                ..
                                            }) = app.activation.dialog.as_mut()
                                            {
                                                *delete_source = !*delete_source;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(!ImportedSpiceFile::source_deletion_supported(), |content| {
                            content.child(rust_i18n::t!("spice.deleteManually").to_string())
                        })
                        .overflow_y_scrollbar()
                        .into_any_element()
                },
                |app: &mut Self, window, cx| {
                    if app.security.screen_locked()
                        || app.remote_start_blocked_by_configuration_replacement(cx)
                    {
                        return false;
                    }
                    let Some(ExternalDialog::Spice {
                        imported,
                        delete_source,
                        fullscreen,
                    }) = app.activation.dialog.take()
                    else {
                        return true;
                    };
                    app.start_temporary_spice(imported.document.clone(), fullscreen, window, cx);
                    if delete_source {
                        let task = app
                            .blocking_jobs
                            .submit_task("spice-file-delete", move |_| imported.delete_unchanged());
                        cx.spawn(async move |this, cx| {
                            if !matches!(await_blocking_job(task).await, Ok(Ok(()))) {
                                let _ = this.update(cx, |this, cx| {
                                    this.shell.set_status(
                                        rust_i18n::t!("spice.deleteFailed").to_string(),
                                    );
                                    cx.notify();
                                });
                            }
                        })
                        .detach();
                    }
                    true
                },
                |app: &mut Self, _| {
                    app.activation.dialog = None;
                },
            ),
            window,
            cx,
        );
    }
}

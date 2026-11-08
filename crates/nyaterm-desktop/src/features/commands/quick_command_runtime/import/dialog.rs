use rust_i18n::t;

use gpui::{Context, IntoElement, PathPromptOptions, SharedString, Window};
use nyaterm_core::export_quick_commands_json;
use nyaterm_store::{StorageError, StoreDomain};
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use crate::models::{QuickCommandImportPathPromptKind, QuickCommandImportPathPromptResult};

use super::sources::parse_quick_commands_from_path;
use nyaterm_core::quick_command_import::merge_import;

impl NyaTermApp {
    pub(in crate::features) fn open_quick_command_import_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commands.quick_import_path_prompt().is_some() || window.has_active_nya_dialog(cx) {
            self.shell
                .set_status("quick command import picker is already open".to_string());
            cx.notify();
            return;
        }

        self.shell
            .set_status("select a quick command import source".to_string());
        self.open_content_dialog(
            t!("quickCommands.importTitle").to_string(),
            380.,
            |app, _, cx| {
                app.quick_command_import_dialog_content(cx)
                    .into_any_element()
            },
            |_, _| {},
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn select_quick_command_import_source(
        &mut self,
        kind: QuickCommandImportPathPromptKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_nya_dialog(cx);
        self.prompt_quick_command_import(kind, cx);
    }

    pub(in crate::features) fn prompt_quick_command_export(&mut self, cx: &mut Context<Self>) {
        let directory = self.runtime.config_dir().to_path_buf();
        let receiver = cx.prompt_for_new_path(&directory, Some("nyaterm-quick-commands.json"));
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        self.shell
            .set_status("selecting quick command export destination".to_string());
        let origin_window = cx
            .active_window()
            .or_else(|| self.shell.main_window().map(Into::into));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(path))) => {
                    let task = scheduler.submit_task("quick-command-export", move |_| {
                        let config = store
                            .request_fn(StoreDomain::Commands, |database| {
                                database.load_quick_commands()
                            })
                            .map_err(|error| error.to_string())?;
                        let raw = export_quick_commands_json(config)
                            .map_err(|error| error.to_string())?;
                        std::fs::write(&path, raw).map_err(|error| error.to_string())?;
                        Ok::<_, String>(path)
                    });
                    await_blocking_job(task).await.and_then(|result| result)
                }
                Ok(Ok(None)) => Err("cancelled".to_string()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("closed".to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        this.shell
                            .set_status(format!("quick commands exported to {}", path.display()));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                        this.notify_operation_at(
                            origin_window,
                            "quick-command-export",
                            nyaterm_ui::notification::NyaNotificationKind::Success,
                            t!("quickCommands.exportSuccess").to_string(),
                            cx,
                        );
                    }
                    Err(error) if error == "cancelled" => {
                        this.shell
                            .set_status("quick command export cancelled".to_string());
                    }
                    Err(error) if error == "closed" => {
                        this.shell.set_status(
                            "quick command export picker closed before returning".to_string(),
                        );
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("quick command export failed: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                        this.notify_operation_at(
                            origin_window,
                            "quick-command-export",
                            nyaterm_ui::notification::NyaNotificationKind::Error,
                            t!("common.operationFailed").to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn prompt_quick_command_import(
        &mut self,
        kind: QuickCommandImportPathPromptKind,
        cx: &mut Context<Self>,
    ) {
        if !self.commands.request_quick_import_path(kind) {
            self.shell
                .set_status("quick command import picker is already open".to_string());
            cx.notify();
            return;
        }

        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(t!(kind.prompt_label_key()))),
        };
        let receiver = cx.prompt_for_paths(options);
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        self.shell.set_status(kind.selecting_status().to_string());

        let origin_window = cx
            .active_window()
            .or_else(|| self.shell.main_window().map(Into::into));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => {
                        let task = scheduler.submit_task("quick-command-import", move |_| {
                            let import_config = match parse_quick_commands_from_path(kind, &path) {
                                Ok(config) => config,
                                Err(error) => {
                                    return QuickCommandImportPathPromptResult::Failed(error);
                                }
                            };
                            match store.request_mutation_fn(
                                StoreDomain::Commands,
                                move |database| {
                                    let mut config = database.load_quick_commands()?;
                                    let mut summary = merge_import(&mut config, import_config)
                                        .map_err(StorageError::InvalidData)?;
                                    database.save_quick_commands(config.clone())?;
                                    summary.total_commands = config.commands.len();
                                    summary.total_categories = config.categories.len();
                                    Ok(summary)
                                },
                            ) {
                                Ok(summary) => QuickCommandImportPathPromptResult::Imported {
                                    imported_commands: summary.imported_commands,
                                    imported_categories: summary.imported_categories,
                                    updated_commands: summary.updated_commands,
                                    total_commands: summary.total_commands,
                                    total_categories: summary.total_categories,
                                },
                                Err(error) => {
                                    QuickCommandImportPathPromptResult::Failed(error.to_string())
                                }
                            }
                        });
                        await_blocking_job(task)
                            .await
                            .unwrap_or_else(QuickCommandImportPathPromptResult::Failed)
                    }
                    None => QuickCommandImportPathPromptResult::Cancelled,
                },
                Ok(Ok(None)) => QuickCommandImportPathPromptResult::Cancelled,
                Ok(Err(error)) => QuickCommandImportPathPromptResult::Failed(error.to_string()),
                Err(_) => QuickCommandImportPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_quick_command_import_result(result, origin_window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_quick_command_import_result(
        &mut self,
        result: QuickCommandImportPathPromptResult,
        origin_window: Option<gpui::AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        self.commands.finish_quick_import_path();
        match result {
            QuickCommandImportPathPromptResult::Imported {
                imported_commands,
                imported_categories,
                updated_commands,
                total_commands,
                total_categories,
            } => {
                self.refresh_quick_commands(cx);
                self.request_shared_state_refresh(
                    crate::app_shell::SharedStateDomain::Commands,
                    cx,
                );
                self.shell.set_status(format!(
                    "imported {imported_commands} quick command(s), updated {updated_commands}, categories +{imported_categories}, total {total_commands}/{total_categories}"
                ));
                self.settings
                    .update_store_status(self.shell.status().to_string(), true);
                self.notify_operation_at(
                    origin_window,
                    "quick-command-import",
                    nyaterm_ui::notification::NyaNotificationKind::Success,
                    t!(
                        "quickCommands.importSuccess",
                        imported = imported_commands,
                        updated = updated_commands,
                        total = total_commands
                    )
                    .to_string(),
                    cx,
                );
            }
            QuickCommandImportPathPromptResult::Cancelled => {
                self.shell
                    .set_status("quick command import cancelled".to_string());
            }
            QuickCommandImportPathPromptResult::Failed(error) => {
                self.shell
                    .set_status(format!("quick command import failed: {error}"));
                self.settings
                    .update_store_status(self.shell.status().to_string(), false);
                self.notify_operation_at(
                    origin_window,
                    "quick-command-import",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    t!("common.operationFailed").to_string(),
                    cx,
                );
            }
            QuickCommandImportPathPromptResult::Closed => {
                self.shell
                    .set_status("quick command import picker closed before returning".to_string());
            }
        }
    }
}

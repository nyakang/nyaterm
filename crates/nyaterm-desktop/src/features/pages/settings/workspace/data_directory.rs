use gpui::{
    Context, IntoElement, PathPromptOptions, SharedString, Window, div, prelude::*, px, rgb,
};
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_store::data_directory::{self, DataDirectoryError, PreparedSwitch, SwitchMode};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::widgets::small_button;

use super::super::settings_form_section;

impl SettingsPanel {
    pub(in crate::features) fn data_directory_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let path = self.settings.data_directory.clone();
        let custom = self.settings.data_directory_custom;
        let editable = self.settings.data_directory_editable && !self.data_directory_checking;
        let restart_pending = self.settings.data_directory_restart_pending;
        let notice = self.settings.data_directory_notice.clone();
        let sync_client = self.settings.data_directory_sync_client.clone();
        let choice = self.data_directory_choice.clone();

        let choose_button = editable.then(|| {
            small_button(
                palette,
                "data-directory-choose",
                t!("settings.dataDirectoryChange"),
                cx.listener(|this, _, _, cx| {
                    this.choose_data_directory(cx);
                }),
            )
        });
        let reset_button = (editable && custom).then(|| {
            small_button(
                palette,
                "data-directory-reset",
                t!("settings.dataDirectoryReset"),
                cx.listener(|this, _, _, cx| {
                    this.reset_data_directory(cx);
                }),
            )
        });

        let pending_line = restart_pending.then(|| {
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.warning))
                .child(t!("settings.dataDirectoryRestartRequired"))
        });
        let notice_line = notice.map(|notice| {
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.danger))
                .child(SharedString::from(format!(
                    "{} {}",
                    t!("settings.dataDirectoryNotice"),
                    notice
                )))
        });
        let sync_line = sync_client.map(|client| {
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.warning))
                .child(SharedString::from(
                    t!(
                        "settings.dataDirectorySyncWarning",
                        client = client.to_string()
                    )
                    .to_string(),
                ))
        });
        let confirm_block =
            choice.map(|choice| self.data_directory_confirm_block(palette, choice, cx));
        let auto_hint = div()
            .text_size(px(12.))
            .text_color(rgb(palette.text_dimmed))
            .child(SharedString::from(
                t!(
                    "settings.dataDirectoryAutoHint",
                    dir = AppFlavor::current().config_directory_name()
                )
                .to_string(),
            ));

        settings_form_section(
            palette,
            Some(t!("settings.dataDirectory")),
            Some(t!("settings.dataDirectoryDesc")),
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.input))
                                .px_3()
                                .py_2()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(rgb(palette.text))
                                        .child(path),
                                ),
                        )
                        .children(choose_button)
                        .children(reset_button),
                )
                .child(auto_hint)
                .when(self.data_directory_checking, |content| {
                    content.child(t!("common.loading").to_string())
                })
                .children(confirm_block)
                .children(pending_line)
                .children(notice_line)
                .children(sync_line),
        )
    }

    fn data_directory_confirm_block(
        &self,
        palette: crate::theme::ThemePalette,
        choice: PreparedSwitch,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target = choice.target.display().to_string();
        let (title, description) = match choice.mode {
            SwitchMode::Migrate => (
                t!("settings.dataDirectoryMigrateTitle"),
                t!("settings.dataDirectoryMigrateDesc", path = target.clone()),
            ),
            SwitchMode::ReplaceWithBackup => (
                t!("settings.dataDirectoryRestoreTitle"),
                t!("settings.dataDirectoryRestoreDesc", path = target.clone()),
            ),
            SwitchMode::Adopt => (
                t!("settings.dataDirectoryAdoptTitle"),
                t!("settings.dataDirectoryAdoptDesc", path = target.clone()),
            ),
        };
        let sync_warning = choice.sync_client.map(|client| {
            t!(
                "settings.dataDirectorySyncWarning",
                client = client.to_string()
            )
            .to_string()
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.warning))
            .px_3()
            .py_3()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(description.to_string()),
            )
            .children(sync_warning.map(|warning| {
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.warning))
                    .child(warning)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(small_button(
                        palette,
                        "data-directory-confirm-cancel",
                        t!("settings.dataDirectoryCancel"),
                        cx.listener(|this, _, _, cx| {
                            this.data_directory_choice = None;
                            cx.notify();
                        }),
                    ))
                    .when(choice.mode == SwitchMode::ReplaceWithBackup, |row| {
                        row.child(small_button(
                            palette,
                            "data-directory-use-history",
                            t!("settings.dataDirectoryUseHistory"),
                            cx.listener(|this, _, _, cx| {
                                if let Some(choice) = &mut this.data_directory_choice {
                                    choice.mode = SwitchMode::Adopt;
                                }
                                cx.notify();
                            }),
                        ))
                    })
                    .child(small_button(
                        palette,
                        "data-directory-confirm-apply",
                        t!("settings.dataDirectoryApply"),
                        cx.listener(|this, _, window, cx| {
                            this.apply_data_directory_choice(window, cx);
                        }),
                    )),
            )
    }

    fn choose_data_directory(&mut self, cx: &mut Context<Self>) {
        if !self.settings.data_directory_editable {
            return;
        }
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(t!("settings.dataDirectoryPickPrompt"))),
        };
        let receiver = cx.prompt_for_paths(options);
        cx.spawn(async move |this, cx| {
            let paths = match receiver.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            let Some(target) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |panel, cx| {
                panel.prepare_data_directory_choice(target, false, cx)
            });
        })
        .detach();
    }

    fn prepare_data_directory_choice(
        &mut self,
        target: std::path::PathBuf,
        migrate_current: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.app_runtime(cx) else {
            return;
        };
        self.data_directory_generation = self.data_directory_generation.wrapping_add(1);
        let generation = self.data_directory_generation;
        self.data_directory_choice = None;
        self.data_directory_checking = true;
        cx.notify();
        let prepared = cx.background_spawn(async move {
            let prepare = if migrate_current {
                data_directory::prepare_migration
            } else {
                data_directory::prepare_switch
            };
            prepare(
                runtime.data_dir(),
                runtime.local_dir(),
                &target,
                AppFlavor::current(),
            )
        });
        cx.spawn(async move |this, cx| {
            let prepared = prepared.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.data_directory_generation != generation {
                    return;
                }
                panel.data_directory_checking = false;
                cx.notify();
                match prepared {
                    Ok(prepared) => {
                        panel.data_directory_choice = Some(prepared);
                        cx.notify();
                    }
                    Err(error) => panel.report_data_directory_error(error, cx),
                }
            });
        })
        .detach();
    }

    fn apply_data_directory_choice(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.data_directory_choice.clone() else {
            return;
        };
        self.with_app(cx, |app, cx| app.request_data_directory_restart(choice, cx));
        self.data_directory_choice = None;
        cx.notify();
    }

    fn reset_data_directory(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.app_runtime(cx) else {
            return;
        };
        self.prepare_data_directory_choice(runtime.local_dir().to_path_buf(), true, cx);
    }

    fn report_data_directory_error(&mut self, error: DataDirectoryError, cx: &mut Context<Self>) {
        let message = data_directory_error_message(&error);
        self.with_app(cx, |app, _| {
            app.shell.set_status(message);
        });
    }
}

fn data_directory_error_message(error: &DataDirectoryError) -> String {
    let detail = error.to_string();
    match error.code() {
        "same" => t!("settings.dataDirectoryErrorSame").to_string(),
        "nested" => t!("settings.dataDirectoryErrorNested").to_string(),
        "not_empty" => t!("settings.dataDirectoryErrorNotEmpty").to_string(),
        "foreign_data" => t!("settings.dataDirectoryErrorForeignData").to_string(),
        "in_use" => t!("settings.dataDirectoryErrorInUse").to_string(),
        "network" => t!("settings.dataDirectoryErrorNetwork").to_string(),
        "not_directory" | "relative" => t!("settings.dataDirectoryErrorInvalidPath").to_string(),
        "not_writable" => t!("settings.dataDirectoryErrorNotWritable", error = detail).to_string(),
        _ => t!("settings.dataDirectoryErrorGeneric", error = detail).to_string(),
    }
}

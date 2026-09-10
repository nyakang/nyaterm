use rust_i18n::t;

use gpui::{Context, FontWeight, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::RuntimeMode;
use nyaterm_ui::NyaScrollable;

use crate::features::NyaTermApp;
use crate::features::view_widgets::dialog_action_button;
use crate::widgets::small_button;

const RELEASES_URL: &str = "https://github.com/nyakang/nyaterm/releases";

impl NyaTermApp {
    pub(in crate::features) fn update_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let update_info = self.update.info().cloned();
        let checking = self.update.is_pending();
        let failed = !checking
            && update_info.is_none()
            && self.update.status().starts_with("update check failed:");
        let available =
            !checking && !failed && update_info.as_ref().is_some_and(|info| info.available);
        let portable = self.runtime.mode() == RuntimeMode::Portable;
        let download_state = self.update.download.clone();
        let download_error = if let crate::features::update::download::DownloadState::Failed(
            error,
        ) = &download_state
        {
            Some(error.clone())
        } else {
            None
        };
        let can_install = crate::features::update::download::supports_native_install(portable);
        let (_, viewport_h) = self.shell.viewport_size();
        let release_url = update_info
            .as_ref()
            .and_then(|info| info.html_url.clone())
            .unwrap_or_else(|| RELEASES_URL.to_string());
        let title = if checking {
            t!("updater.checking")
        } else if failed {
            t!("updater.updateFailed")
        } else if available && portable {
            t!("updater.portableManualTitle")
        } else if available {
            t!("updater.newVersionAvailable")
        } else {
            t!("updater.noUpdate")
        };

        div()
            .id("update-dialog-content")
            .flex()
            .flex_col()
            .gap_4()
            .max_h(px((viewport_h - 32.).max(220.)))
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(title),
                    )
                    .when_some(update_info.as_ref(), |this, info| {
                        if info.available {
                            this.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!(
                                        "{}: v{}",
                                        t!("updater.currentVersion"),
                                        info.current_version
                                    ))
                                    .child(format!(
                                        "{}: v{}",
                                        t!("updater.newVersion"),
                                        info.latest_version
                                    ))
                                    .when_some(info.release_date.clone(), |this, date| {
                                        this.child(format!(
                                            "{}: {}",
                                            t!("updater.releaseDate"),
                                            date
                                        ))
                                    }),
                            )
                        } else {
                            this.child(div().text_xs().text_color(rgb(palette.text_muted)).child(
                                format!(
                                    "{}: v{}",
                                    t!("updater.currentVersion"),
                                    info.current_version
                                ),
                            ))
                        }
                    })
                    .when(failed, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .line_height(px(18.))
                                .text_color(rgb(palette.danger))
                                .child(self.update.status().to_string()),
                        )
                    })
                    .when(available && portable, |this| {
                        this.child(
                            div()
                                .mt_1()
                                .text_xs()
                                .line_height(px(18.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("updater.portableManualDesc")),
                        )
                    }),
            )
            .when_some(
                available
                    .then(|| update_info.as_ref()?.release_notes.clone())
                    .flatten(),
                |this, notes| {
                    this.child(
                        div()
                            .id("update-release-notes")
                            .max_h(px((viewport_h * 0.42).clamp(120., 320.)))
                            .overflow_y_scrollbar()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .p_3()
                            .child(
                                div()
                                    .mb_2()
                                    .text_xs()
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("updater.releaseNotes")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(20.))
                                    .whitespace_normal()
                                    .text_color(rgb(palette.text))
                                    .child(notes),
                            ),
                    )
                },
            )
            .when_some(download_error, |this, error| {
                this.child(div().text_xs().text_color(rgb(palette.danger)).child(error))
            })
            .when(can_install && available, |this| {
                use crate::features::update::download::DownloadState;
                this.child(match download_state {
                    DownloadState::Downloading { received, total } => div()
                        .flex()
                        .gap_2()
                        .child(format!(
                            "{} / {} MiB",
                            received / 1048576,
                            total
                                .map(|value| (value / 1048576).to_string())
                                .unwrap_or_else(|| "?".into())
                        ))
                        .child(
                            nyaterm_ui::NyaButton::new(
                                "update-cancel-download",
                                t!("common.cancel"),
                            )
                            .on_click(
                                cx.listener(|app, _, _, cx| app.cancel_native_update_download(cx)),
                            ),
                        )
                        .into_any_element(),
                    DownloadState::Ready { .. } => nyaterm_ui::NyaButton::new(
                        "update-install",
                        t!("updater.installAndRestart"),
                    )
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.request_native_update_install(window, cx)
                    }))
                    .into_any_element(),
                    DownloadState::Idle | DownloadState::Failed(_) => div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(matches!(download_state, DownloadState::Failed(_)), |this| {
                            this.child(t!("updater.downloadFailed"))
                        })
                        .child(
                            nyaterm_ui::NyaButton::new(
                                "update-download",
                                t!("updater.downloadUpdate"),
                            )
                            .on_click(
                                cx.listener(|app, _, _, cx| app.start_native_update_download(cx)),
                            ),
                        )
                        .into_any_element(),
                })
            })
            .when(!checking, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(small_button(
                            palette,
                            "update-close",
                            t!("common.close"),
                            cx.listener(|this, _, window, cx| {
                                this.close_update_dialog(window, cx);
                            }),
                        ))
                        .when(failed, |this| {
                            this.child(dialog_action_button(
                                palette,
                                "update-retry",
                                t!("updater.retry"),
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.start_update_check(cx);
                                }),
                            ))
                        })
                        .when(available, |this| {
                            this.child(dialog_action_button(
                                palette,
                                "update-open-releases",
                                t!("updater.openReleases"),
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_external_url_for_ui(&release_url, cx);
                                }),
                            ))
                        }),
                )
            })
    }
}

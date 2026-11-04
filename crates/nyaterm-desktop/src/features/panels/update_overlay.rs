use rust_i18n::t;

use gpui::{AnyElement, Context, FontWeight, IntoElement, div, prelude::*, px, relative, rgb, svg};
use nyaterm_core::NativeUpdateInfo;
use nyaterm_core::RuntimeMode;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaMarkdown, NyaScrollable};

use crate::features::NyaTermApp;
use crate::features::shell::gpui_code_font_family;
use crate::features::update::{
    UpdateFailure, UpdateFailureKind, UpdatePhase, classify_update_failure,
};
use crate::features::view_widgets::dialog_action_button;
use crate::theme::ThemePalette;
use crate::widgets::small_button;

const RELEASES_URL: &str = "https://github.com/nyakang/nyaterm/releases";

fn format_download_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.;
    const MIB: f64 = KIB * 1024.;
    const GIB: f64 = MIB * 1024.;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn update_dialog_title_key(phase: &UpdatePhase) -> &'static str {
    match phase {
        UpdatePhase::Checking => "updater.checking",
        UpdatePhase::Downloading { .. } => "updater.downloading",
        UpdatePhase::Ready => "updater.readyTitle",
        UpdatePhase::Applying => "updater.installing",
        UpdatePhase::Failed { message, download } => {
            classify_update_failure(*download, message).title_key()
        }
        UpdatePhase::Available => "updater.newVersionAvailable",
        UpdatePhase::Idle | UpdatePhase::UpToDate => "updater.upToDate",
    }
}

/// Calendar day carried by the release timestamp. The offset belongs to the
/// publisher, so a `+08:00` publication stays on that calendar day instead of
/// shifting backward on machines west of it.
fn release_calendar_date(raw: &str) -> Option<(i32, u8, u8)> {
    let (date, time) = raw.split_once('T').unwrap_or((raw, ""));
    if !time.is_empty() && !time.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    let mut parts = date.split('-');
    let year = parse_ascii_number(parts.next().filter(|part| part.len() == 4)?)?;
    let month = parse_ascii_number(parts.next().filter(|part| part.len() == 2)?)?;
    let day = parse_ascii_number(parts.next().filter(|part| part.len() == 2)?)?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((
        i32::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
    ))
}

fn parse_ascii_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn format_release_date_for_locale(raw: &str, locale: &str) -> String {
    let Some((year, month, day)) = release_calendar_date(raw) else {
        return raw.to_string();
    };
    let (month, day) = if matches!(locale, "zh-CN" | "zh-TW" | "ja" | "ko") {
        (month.to_string(), day.to_string())
    } else {
        (format!("{month:02}"), format!("{day:02}"))
    };
    t!(
        "updater.releaseDateShort",
        locale = locale,
        year = year,
        month = month,
        day = day
    )
    .into_owned()
}

fn format_release_date(raw: &str) -> String {
    format_release_date_for_locale(raw, rust_i18n::locale().as_ref())
}

impl NyaTermApp {
    pub(in crate::features) fn update_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (update_info, phase, notes_expanded) = {
            let update = self.update.read(cx);
            (
                update.info().cloned(),
                update.phase().clone(),
                update.release_notes_expanded(),
            )
        };
        let checking = matches!(phase, UpdatePhase::Checking);
        let available = update_info.as_ref().is_some_and(|info| info.available);
        let downloading = matches!(phase, UpdatePhase::Downloading { .. });
        let ready = matches!(phase, UpdatePhase::Ready);
        let applying = matches!(phase, UpdatePhase::Applying);
        let failure = match &phase {
            UpdatePhase::Failed { message, download } => {
                Some(classify_update_failure(*download, message))
            }
            _ => None,
        };
        let portable = self.runtime.mode() == RuntimeMode::Portable;
        let can_install = crate::features::update::download::supports_native_install(portable);
        let (_, viewport_h) = self.shell.viewport_size();
        let release_url = update_info
            .as_ref()
            .and_then(|info| info.html_url.clone())
            .unwrap_or_else(|| RELEASES_URL.to_string());
        let notes = available
            .then(|| update_info.as_ref()?.release_notes.clone())
            .flatten()
            .filter(|notes| !notes.trim().is_empty());
        let title = t!(update_dialog_title_key(&phase));
        let actions = update_action_flags(&phase, available, can_install, failure.as_ref());
        let show_actions = !checking && !downloading && !applying;
        // A lone Close sits on the version row. A separate footer under two
        // lines of text leaves the wide dialog looking empty.
        let inline_close = show_actions && actions.dismiss_only() && update_info.is_some();

        div()
            .id("update-dialog-content")
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(update_heading(
                palette,
                title,
                update_info.as_ref(),
                inline_close
                    .then(|| dismiss_button(palette, actions.dismiss_later, cx).into_any_element()),
            ))
            .when(ready, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("updater.readyToRestart")),
                )
            })
            .when_some(failure.clone(), |this, failure| {
                this.child(failure_notice(palette, failure, cx))
            })
            .when(can_install && downloading, |this| {
                let UpdatePhase::Downloading { received, total } = &phase else {
                    return this;
                };
                this.child(download_progress(palette, *received, *total, cx))
            })
            .when_some(notes, |this, notes| {
                this.child(release_notes(
                    palette,
                    notes,
                    notes_expanded,
                    viewport_h,
                    cx,
                ))
            })
            .when(show_actions && !inline_close, |this| {
                this.child(update_actions(palette, actions, release_url, cx))
            })
    }
}

fn update_heading(
    palette: ThemePalette,
    title: impl IntoElement,
    info: Option<&NativeUpdateInfo>,
    trailing: Option<AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pr(px(28.))
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight(700.))
                .text_color(rgb(palette.text))
                .child(title),
        )
        .when_some(info, |this, info| {
            this.child(version_line(palette, info, trailing))
        })
}

fn version_line(
    palette: ThemePalette,
    info: &NativeUpdateInfo,
    trailing: Option<AnyElement>,
) -> impl IntoElement {
    let current = format!("v{}", info.current_version);
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .min_w_0()
                .when(info.available, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(current.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child("→"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(format!("v{}", info.latest_version)),
                    )
                })
                .when(!info.available, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(current),
                    )
                })
                .when_some(info.release_date.clone(), |this, date| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child("·"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(format_release_date(&date)),
                    )
                }),
        )
        .children(trailing)
}

fn failure_notice(
    palette: ThemePalette,
    failure: UpdateFailure,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let detail = failure.visible_detail().map(str::to_string);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .border_l_2()
        .border_color(rgb(palette.danger))
        .pl_3()
        .child(
            div()
                .text_sm()
                .line_height(px(20.))
                .text_color(rgb(palette.text))
                .child(t!(failure.summary_key())),
        )
        .when_some(detail, |this, detail| {
            let copied = detail.clone();
            this.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .line_height(px(18.))
                            .font_family(gpui_code_font_family())
                            .text_color(rgb(palette.text_muted))
                            .child(detail),
                    )
                    .child(
                        NyaButton::new("update-copy-detail", t!("common.copyToClipboard"))
                            .icon("icons/copy.svg")
                            .variant(NyaButtonVariant::Ghost)
                            .small()
                            .compact()
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.copy_text_with_feedback(
                                    "update-detail-copied",
                                    copied.clone(),
                                    cx,
                                );
                            })),
                    ),
            )
        })
        .into_any_element()
}

fn download_progress(
    palette: ThemePalette,
    received: u64,
    total: Option<u64>,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let ratio = total
        .filter(|total| *total > 0)
        .map(|total| received as f32 / total as f32)
        .unwrap_or(0.)
        .clamp(0., 1.);
    let percent = (ratio * 100.).round() as u32;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(format!(
                    "{} / {}",
                    format_download_bytes(received),
                    total
                        .map(format_download_bytes)
                        .unwrap_or_else(|| "...".to_string())
                ))
                .child(format!("{percent}%")),
        )
        .child(
            div()
                .h(px(8.))
                .w_full()
                .rounded_sm()
                .bg(rgb(palette.hover))
                .child(
                    div()
                        .h_full()
                        .w(relative(ratio))
                        .rounded_sm()
                        .bg(rgb(palette.primary)),
                ),
        )
        .child(
            div().flex().justify_end().child(
                NyaButton::new("update-cancel-download", t!("common.cancel"))
                    .variant(NyaButtonVariant::Secondary)
                    .small()
                    .compact()
                    .on_click(cx.listener(|app, _, _, cx| app.cancel_native_update_download(cx))),
            ),
        )
        .into_any_element()
}

fn release_notes(
    palette: ThemePalette,
    notes: String,
    expanded: bool,
    viewport_h: f32,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let max_height = px((viewport_h * 0.42).clamp(120., 320.));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id("update-release-notes-toggle")
                .flex()
                .items_center()
                .gap_1()
                .w_full()
                .cursor_pointer()
                .text_xs()
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text_muted))
                .rounded_sm()
                .hover(|style| style.bg(rgb(palette.hover)))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.update.update(cx, |update, cx| {
                        update.set_release_notes_expanded(!expanded);
                        cx.notify();
                    });
                }))
                .child(
                    svg()
                        .size(px(12.))
                        .path(if expanded {
                            "icons/chevron-up.svg"
                        } else {
                            "icons/chevron-down.svg"
                        })
                        .text_color(rgb(palette.text_muted)),
                )
                .child(if expanded {
                    t!("updater.hideReleaseNotes")
                } else {
                    t!("updater.showReleaseNotes")
                }),
        )
        .when(expanded, |this| {
            this.child(
                div()
                    .id("update-release-notes")
                    .max_h(max_height)
                    .overflow_y_scrollbar()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .p_3()
                    .child(NyaMarkdown::new("update-release-notes-markdown", notes)),
            )
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
struct UpdateActionFlags {
    dismiss_later: bool,
    view_secondary: bool,
    view_primary: bool,
    retry_check: bool,
    retry_download: bool,
    offer_download: bool,
    offer_install: bool,
}

impl UpdateActionFlags {
    fn dismiss_only(self) -> bool {
        !self.view_secondary
            && !self.view_primary
            && !self.retry_check
            && !self.retry_download
            && !self.offer_download
            && !self.offer_install
    }
}

fn update_action_flags(
    phase: &UpdatePhase,
    available: bool,
    can_install: bool,
    failure: Option<&UpdateFailure>,
) -> UpdateActionFlags {
    let kind = failure.map(|failure| failure.kind);
    let package_failure = kind == Some(UpdateFailureKind::Package);
    let download_failure = kind == Some(UpdateFailureKind::Download);
    let offer_download = can_install && available && matches!(phase, UpdatePhase::Available);
    let offer_install = matches!(phase, UpdatePhase::Ready);
    let retry_download = can_install && download_failure;
    let view_primary =
        package_failure || (available && !can_install && matches!(phase, UpdatePhase::Available));
    UpdateActionFlags {
        dismiss_later: matches!(phase, UpdatePhase::Available | UpdatePhase::Ready),
        view_secondary: !view_primary
            && available
            && (offer_download || offer_install || retry_download),
        view_primary,
        retry_check: kind == Some(UpdateFailureKind::Check),
        retry_download,
        offer_download,
        offer_install,
    }
}

fn dismiss_button(
    palette: ThemePalette,
    later: bool,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    small_button(
        palette,
        "update-close",
        if later {
            t!("updater.later")
        } else {
            t!("common.close")
        },
        cx.listener(|this, _, window, cx| {
            this.close_update_dialog(window, cx);
        }),
    )
}

fn update_actions(
    palette: ThemePalette,
    actions: UpdateActionFlags,
    release_url: String,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let secondary_release_url = release_url.clone();

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap_2()
        .child(dismiss_button(palette, actions.dismiss_later, cx))
        .when(actions.view_secondary, |this| {
            this.child(small_button(
                palette,
                "update-open-releases",
                t!("updater.viewRelease"),
                cx.listener(move |this, _, _, cx| {
                    this.open_external_url_for_ui(&secondary_release_url, cx);
                }),
            ))
        })
        .when(actions.retry_check || actions.retry_download, |this| {
            this.child(dialog_action_button(
                palette,
                "update-retry",
                t!("updater.retry"),
                false,
                cx.listener(move |this, _, _, cx| {
                    if actions.retry_check {
                        this.start_update_check(cx);
                    } else {
                        this.start_native_update_download(cx);
                    }
                }),
            ))
        })
        .when(actions.offer_download, |this| {
            this.child(dialog_action_button(
                palette,
                "update-download",
                t!("updater.downloadUpdate"),
                false,
                cx.listener(|app, _, _, cx| app.start_native_update_download(cx)),
            ))
        })
        .when(actions.offer_install, |this| {
            this.child(dialog_action_button(
                palette,
                "update-install",
                t!("updater.installAndRestart"),
                false,
                cx.listener(|app, _, window, cx| app.request_native_update_install(window, cx)),
            ))
        })
        .when(actions.view_primary, |this| {
            this.child(dialog_action_button(
                palette,
                "update-open-releases",
                t!("updater.viewRelease"),
                false,
                cx.listener(move |this, _, _, cx| {
                    this.open_external_url_for_ui(&release_url, cx);
                }),
            ))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{format_release_date_for_locale, release_calendar_date, update_dialog_title_key};
    use crate::features::update::UpdatePhase;

    #[test]
    fn update_dialog_heading_tracks_check_download_ready_and_failure() {
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Checking),
            "updater.checking"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Available),
            "updater.newVersionAvailable"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Downloading {
                received: 1,
                total: Some(2),
            }),
            "updater.downloading"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Ready),
            "updater.readyTitle"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Applying),
            "updater.installing"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::UpToDate),
            "updater.upToDate"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Failed {
                message: "offline".into(),
                download: false,
            }),
            "updater.checkFailedTitle"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Failed {
                message: "update download truncated".into(),
                download: true,
            }),
            "updater.downloadIncompleteTitle"
        );
        assert_eq!(
            update_dialog_title_key(&UpdatePhase::Failed {
                message: "portable update archive contains an unexpected file: conpty\\LICENSE.txt"
                    .into(),
                download: true,
            }),
            "updater.couldNotInstall"
        );
    }

    #[test]
    fn release_timestamp_keeps_the_publishers_calendar_day() {
        assert_eq!(
            release_calendar_date("2026-11-01T14:32:45+08:00"),
            Some((2026, 11, 1))
        );
        assert_eq!(
            format_release_date_for_locale("2026-11-01T14:32:45+08:00", "zh-CN"),
            "2026年11月1日"
        );
        assert_eq!(
            format_release_date_for_locale("2026-11-01T14:32:45+08:00", "en"),
            "2026-11-01"
        );
        assert_eq!(
            format_release_date_for_locale("2026-11-01", "ja"),
            "2026年11月1日"
        );
        assert_eq!(
            format_release_date_for_locale("not-a-date", "en"),
            "not-a-date"
        );
    }
}

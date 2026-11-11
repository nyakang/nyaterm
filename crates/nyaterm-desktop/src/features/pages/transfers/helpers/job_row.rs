use super::transfer_progress_ratio;
use crate::features::formatting::format_rate;
use crate::features::pages::transfers::panel::TransferPanel;
use crate::features::shell::gpui_code_font_family;
use crate::features::transfers::format_file_size;
use crate::models::{TransferJobKind, TransferJobRowSnapshot, TransferJobStatus};
use crate::theme::ThemePalette;
use gpui::{
    Animation, AnimationExt as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, div, prelude::FluentBuilder as _, px, relative, rgb, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::NyaTooltip;
use std::time::Duration;

#[derive(Clone)]
pub(in crate::features::pages::transfers) struct TransferJobRowLabels {
    pub(in crate::features::pages::transfers) queued: String,
    pub(in crate::features::pages::transfers) transferring: String,
    pub(in crate::features::pages::transfers) paused: String,
    pub(in crate::features::pages::transfers) cancelling: String,
    pub(in crate::features::pages::transfers) cancelled: String,
    pub(in crate::features::pages::transfers) completed: String,
    pub(in crate::features::pages::transfers) failed: String,
    pub(in crate::features::pages::transfers) unknown_size: String,
}

pub(in crate::features::pages::transfers) fn transfer_job_row(
    palette: ThemePalette,
    job: TransferJobRowSnapshot,
    directory_progress: Option<String>,
    labels: TransferJobRowLabels,
    _selected_remote_path: Option<String>,
    selected_job_id: Option<String>,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let status_color = match job.status {
        TransferJobStatus::Queued => rgb(palette.text_muted),
        TransferJobStatus::Running => rgb(palette.warning),
        TransferJobStatus::Paused => rgb(palette.link),
        TransferJobStatus::Cancelling => rgb(palette.warning),
        TransferJobStatus::Cancelled => rgb(palette.text_muted),
        TransferJobStatus::Completed => rgb(0x34d399),
        TransferJobStatus::Failed => rgb(0xfb7185),
    };
    let job_selected = selected_job_id.as_deref() == Some(job.id.as_str());
    let file_name = transfer_job_file_name(&job);
    let icon_path = transfer_job_icon_path(&job.kind, &job);
    let direction_color = transfer_job_direction_color(&job.kind);
    let detail = transfer_job_detail(&job, directory_progress, &labels);
    let progress_label = transfer_job_right_label(&job, &labels);
    let progress_percent = transfer_job_progress_percent(&job);
    let indeterminate = matches!(
        job.status,
        TransferJobStatus::Queued | TransferJobStatus::Running | TransferJobStatus::Paused
    ) && job.progress.is_some()
        && progress_percent.is_none();
    let context_job_id = job.id.clone();

    div()
        .id(SharedString::from(format!("transfer-job-row-{}", job.id)))
        .debug_selector(|| format!("transfer-job-row-{}", job.id))
        .w_full()
        .min_w_0()
        .rounded_sm()
        .bg(if job_selected {
            rgb(palette.hover)
        } else {
            rgb(palette.surface)
        })
        .border_1()
        .border_color(if job_selected {
            rgb(palette.link)
        } else {
            rgb(palette.surface)
        })
        .px_2()
        .py(px(6.))
        .cursor_pointer()
        .on_click({
            let job_id = job.id.clone();
            cx.listener(move |panel, _, window, cx| {
                panel.with_app(cx, |this, cx| {
                    window.focus(this.transfer.queue_focus(), cx);
                    this.select_transfer_job(job_id.clone(), cx);
                })
            })
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |panel, event: &MouseDownEvent, window, cx| {
                panel.with_app(cx, |this, cx| {
                    cx.stop_propagation();
                    this.open_transfer_job_menu(context_job_id.clone(), event, window, cx);
                })
            }),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path(icon_path)
                        .text_color(direction_color),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .id(SharedString::from(format!("transfer-job-name-{}", job.id)))
                                .debug_selector(|| format!("transfer-job-name-{}", job.id))
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text))
                                .tooltip({
                                    let file_name = file_name.clone();
                                    move |window, cx| {
                                        NyaTooltip::new(file_name.clone()).build(window, cx)
                                    }
                                })
                                .child(file_name),
                        )
                        .child(
                            div()
                                .debug_selector(|| format!("transfer-job-detail-{}", job.id))
                                .min_w_0()
                                .truncate()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .child(detail),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| format!("transfer-job-status-{}", job.id))
                        .flex_none()
                        .min_w(px(68.))
                        .whitespace_nowrap()
                        .when(job.status == TransferJobStatus::Running, |this| {
                            this.font_family(gpui_code_font_family())
                        })
                        .text_align(gpui::TextAlign::Right)
                        .text_size(px(10.))
                        .font_weight(FontWeight(700.))
                        .text_color(status_color)
                        .child(progress_label),
                ),
        )
        .when(indeterminate, |this| {
            let indicator = div()
                .h_full()
                .w(relative(0.3))
                .rounded_full()
                .bg(direction_color);
            let indicator = if job.status == TransferJobStatus::Running {
                indicator
                    .with_animation(
                        SharedString::from(format!("copy-progress-{}", job.id)),
                        Animation::new(Duration::from_secs(2)).repeat(),
                        |bar, progress| bar.ml(relative(0.7 * progress)),
                    )
                    .into_any_element()
            } else {
                indicator.into_any_element()
            };
            this.child(
                div()
                    .mt_1()
                    .h(px(4.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(palette.border))
                    .child(indicator),
            )
        })
        .when_some(progress_percent, |this, percent| {
            this.child(
                div()
                    .mt_1()
                    .h(px(4.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(palette.border))
                    .child(
                        div()
                            .h_full()
                            .w(relative(percent))
                            .rounded_full()
                            .bg(direction_color),
                    ),
            )
        })
}

fn transfer_job_right_label(job: &TransferJobRowSnapshot, labels: &TransferJobRowLabels) -> String {
    if job.status == TransferJobStatus::Running {
        format_rate(job.speed_bytes_per_sec)
    } else {
        transfer_job_status_label(job.status, labels).to_string()
    }
}

fn transfer_job_file_name(job: &TransferJobRowSnapshot) -> String {
    (!job.display_name.trim().is_empty())
        .then(|| job.display_name.clone())
        .or_else(|| {
            job.summary.as_ref().map(|summary| match &job.kind {
                TransferJobKind::Upload { .. } => local_file_name(&summary.local_path),
                _ => remote_file_name(&summary.remote_path),
            })
        })
        .unwrap_or_else(|| match &job.kind {
            TransferJobKind::DragExport { remote_path }
            | TransferJobKind::Download { remote_path, .. }
            | TransferJobKind::OpenExternal { remote_path, .. }
            | TransferJobKind::LoadEditor { remote_path, .. }
            | TransferJobKind::SaveEditor { remote_path, .. }
            | TransferJobKind::LoadProperties { remote_path }
            | TransferJobKind::UpdateProperties { remote_path, .. }
            | TransferJobKind::AiFileAction { remote_path, .. } => remote_file_name(remote_path),
            TransferJobKind::Upload { local_path, .. } => local_file_name(local_path),
            TransferJobKind::ZmodemUpload { file_name, .. }
            | TransferJobKind::XmodemUpload { file_name, .. }
            | TransferJobKind::YmodemUpload { file_name, .. }
            | TransferJobKind::ZmodemDownload { file_name, .. }
            | TransferJobKind::TrzszDownload { file_name, .. }
            | TransferJobKind::TrzszUpload { file_name, .. } => file_name.clone(),
            other => truncate_preview(&format!("{other:?}"), 48),
        })
}

fn transfer_job_detail(
    job: &TransferJobRowSnapshot,
    directory_progress: Option<String>,
    labels: &TransferJobRowLabels,
) -> String {
    let time = format_transfer_row_time(job.created_at_ms);
    let size_detail = job.progress.as_ref().and_then(|progress| {
        if let Some(total) = progress.total_bytes.filter(|total| *total > 0) {
            return Some(format!(
                "{} / {}",
                format_file_size(Some(progress.bytes_transferred)),
                format_file_size(Some(total))
            ));
        }
        (progress.bytes_transferred > 0).then(|| {
            format!(
                "{} / {}",
                format_file_size(Some(progress.bytes_transferred)),
                labels.unknown_size
            )
        })
    });
    let completed_size = job.summary.as_ref().and_then(|summary| {
        (summary.bytes > 0).then(|| {
            format!(
                "{} / {}",
                format_file_size(Some(summary.bytes)),
                format_file_size(Some(summary.bytes))
            )
        })
    });
    let size = if job.status == TransferJobStatus::Completed {
        completed_size.or(size_detail)
    } else {
        size_detail.or(completed_size)
    };
    let text = match (directory_progress, size) {
        (Some(items), Some(size)) => Some(format!("{items} · {size}")),
        (items, size) => items.or(size),
    }
    .or_else(|| (job.status == TransferJobStatus::Failed).then(|| job.detail.clone()))
    .filter(|value| !value.trim().is_empty());
    match text {
        Some(text) => format!("{time} · {text}"),
        None => time,
    }
}

fn transfer_job_progress_percent(job: &TransferJobRowSnapshot) -> Option<f32> {
    matches!(
        job.status,
        TransferJobStatus::Queued | TransferJobStatus::Running | TransferJobStatus::Paused
    )
    .then(|| {
        job.progress.as_ref().and_then(|progress| {
            if matches!(job.kind, TransferJobKind::SendTo { .. }) && progress.total_bytes.is_none()
            {
                None
            } else {
                transfer_progress_ratio(progress)
            }
        })
    })
    .flatten()
}

fn transfer_job_status_label(status: TransferJobStatus, labels: &TransferJobRowLabels) -> &str {
    match status {
        TransferJobStatus::Queued => &labels.queued,
        TransferJobStatus::Running => &labels.transferring,
        TransferJobStatus::Paused => &labels.paused,
        TransferJobStatus::Cancelling => &labels.cancelling,
        TransferJobStatus::Cancelled => &labels.cancelled,
        TransferJobStatus::Completed => &labels.completed,
        TransferJobStatus::Failed => &labels.failed,
    }
}

fn format_transfer_row_time(created_at_ms: u128) -> String {
    let timestamp = i64::try_from(created_at_ms / 1000).unwrap_or(i64::MAX);
    let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(timestamp) else {
        return "--:--:--".to_string();
    };
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let format = time::macros::format_description!("[hour]:[minute]:[second]");
    datetime
        .to_offset(offset)
        .format(&format)
        .unwrap_or_else(|_| "--:--:--".to_string())
}

fn transfer_job_icon_path(kind: &TransferJobKind, job: &TransferJobRowSnapshot) -> &'static str {
    if transfer_job_is_directory(job) {
        return "icons/conn/folder.svg";
    }
    match kind {
        TransferJobKind::Upload { .. }
        | TransferJobKind::ZmodemUpload { .. }
        | TransferJobKind::XmodemUpload { .. }
        | TransferJobKind::YmodemUpload { .. }
        | TransferJobKind::TrzszUpload { .. } => "icons/fe/upload.svg",
        TransferJobKind::SendTo { .. } => "icons/send.svg",
        _ => "icons/fe/download.svg",
    }
}

fn transfer_job_is_directory(job: &TransferJobRowSnapshot) -> bool {
    job.progress
        .as_ref()
        .is_some_and(|progress| progress.item_count_total.is_some())
}

fn transfer_job_direction_color(kind: &TransferJobKind) -> gpui::Rgba {
    match kind {
        TransferJobKind::Upload { .. }
        | TransferJobKind::ZmodemUpload { .. }
        | TransferJobKind::XmodemUpload { .. }
        | TransferJobKind::YmodemUpload { .. }
        | TransferJobKind::TrzszUpload { .. } => rgb(0x4ade80),
        _ => rgb(0x60a5fa),
    }
}

fn remote_file_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn local_file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        TransferJobRowLabels, format_transfer_row_time, transfer_job_detail,
        transfer_job_file_name, transfer_job_icon_path, transfer_job_progress_percent,
        transfer_job_right_label, transfer_job_status_label,
    };
    use crate::models::{TransferJobKind, TransferJobRowSnapshot, TransferJobStatus};
    use nyaterm_files::sftp_transfer_types::SftpTransferProgress;
    use std::path::PathBuf;

    fn labels() -> TransferJobRowLabels {
        TransferJobRowLabels {
            queued: rust_i18n::t!("fileTransfer.queued").to_string(),
            transferring: "transferring-i18n".to_string(),
            paused: "paused-i18n".to_string(),
            cancelling: "cancelling-i18n".to_string(),
            cancelled: "cancelled-i18n".to_string(),
            completed: "completed-i18n".to_string(),
            failed: "failed-i18n".to_string(),
            unknown_size: "unknown-size-i18n".to_string(),
        }
    }

    fn progress(
        remote_path: &str,
        bytes_transferred: u64,
        total_bytes: Option<u64>,
        item_count_completed: Option<u64>,
        item_count_total: Option<u64>,
    ) -> SftpTransferProgress {
        SftpTransferProgress {
            phase: None,
            remote_path: remote_path.to_string(),
            local_path: PathBuf::from("/local/target"),
            bytes_transferred,
            total_bytes,
            item_count_completed,
            item_count_total,
        }
    }

    fn job(status: TransferJobStatus) -> TransferJobRowSnapshot {
        let kind = TransferJobKind::Download {
            remote_path: "/remote/file.bin".to_string(),
            raw_path_token: None,
            local_path: PathBuf::from("/local/file.bin"),
        };
        crate::models::TransferJobState {
            id: "job-1".to_string(),
            session_id: Some("session-a".to_string()),
            display_name: crate::models::TransferJobState::display_name_for_kind(&kind),
            kind,
            status,
            detail: String::new(),
            created_at_ms: 1_785_555_123_000,
            entries: Vec::new(),
            summary: None,
            progress: Some(progress("/remote/file.bin", 25, Some(100), None, None)),
            control: None,
            speed: Default::default(),
        }
        .row_snapshot()
    }

    #[test]
    fn completed_staged_copy_shows_content_size_instead_of_two_leg_work_size() {
        let mut job = job(TransferJobStatus::Completed);
        job.kind = TransferJobKind::SendTo {
            source_path: "/source".into(),
            target_session_id: "target".into(),
            target_path: "/target".into(),
            target_parent_path: "/".into(),
        };
        job.progress = Some(progress("/target", 2048, Some(2048), Some(2), Some(2)));
        job.summary = Some(nyaterm_files::sftp_transfer_types::SftpTransferSummary {
            remote_path: "/target".into(),
            local_path: PathBuf::new(),
            bytes: 1024,
            skipped: false,
        });
        let detail = transfer_job_detail(&job, Some("2 / 2".into()), &labels());
        assert!(detail.contains("1.0 KiB / 1.0 KiB"));
        assert!(!detail.contains("2.0 KiB"));
    }

    #[test]
    fn unknown_copy_byte_total_is_indeterminate_but_zero_byte_copy_uses_items() {
        let mut job = job(TransferJobStatus::Running);
        job.kind = TransferJobKind::SendTo {
            source_path: "/source".into(),
            target_session_id: "target".into(),
            target_path: "/target".into(),
            target_parent_path: "/".into(),
        };
        job.progress = Some(progress("/target", 100, None, Some(1), Some(2)));
        assert!(transfer_job_progress_percent(&job).is_none());
        job.progress = Some(progress("/target", 0, Some(0), Some(1), Some(2)));
        assert_eq!(transfer_job_progress_percent(&job), Some(0.5));
    }

    #[test]
    fn running_transfers_show_speed_instead_of_percentage_even_without_known_size() {
        let labels = labels();
        let mut job = job(TransferJobStatus::Running);
        assert_eq!(transfer_job_right_label(&job, &labels), "0 B/s");
        job.speed_bytes_per_sec = 1024. * 1024. * 2.5;
        assert_eq!(transfer_job_right_label(&job, &labels), "2.5 MiB/s");
        job.progress.as_mut().expect("progress").total_bytes = None;
        job.speed_bytes_per_sec = 2048.;
        assert_eq!(transfer_job_right_label(&job, &labels), "2.0 KiB/s");
        job.progress = None;
        job.speed_bytes_per_sec = 128.;
        assert_eq!(transfer_job_right_label(&job, &labels), "128 B/s");
    }

    #[test]
    fn non_running_transfers_show_status_instead_of_last_speed() {
        let labels = labels();
        for status in [
            TransferJobStatus::Paused,
            TransferJobStatus::Cancelling,
            TransferJobStatus::Cancelled,
            TransferJobStatus::Completed,
            TransferJobStatus::Failed,
        ] {
            let mut job = job(status);
            job.speed_bytes_per_sec = 2048.;
            assert_eq!(
                transfer_job_right_label(&job, &labels),
                transfer_job_status_label(status, &labels)
            );
        }
    }

    #[test]
    fn completed_transfer_keeps_stable_timestamp_text() {
        let created_at_ms = 1_785_555_123_000;

        assert_eq!(
            format_transfer_row_time(created_at_ms),
            format_transfer_row_time(created_at_ms)
        );
    }

    #[test]
    fn progress_bar_is_only_visible_for_running_or_paused_jobs() {
        assert!(transfer_job_progress_percent(&job(TransferJobStatus::Running)).is_some());
        assert!(transfer_job_progress_percent(&job(TransferJobStatus::Paused)).is_some());
        assert!(transfer_job_progress_percent(&job(TransferJobStatus::Completed)).is_none());
        assert!(transfer_job_progress_percent(&job(TransferJobStatus::Failed)).is_none());
        assert!(transfer_job_progress_percent(&job(TransferJobStatus::Cancelled)).is_none());
    }

    #[test]
    fn directory_transfer_title_stays_on_root_item_name() {
        let kind = TransferJobKind::Download {
            remote_path: "/remote/project".to_string(),
            raw_path_token: None,
            local_path: PathBuf::from("/downloads/project"),
        };
        let job = crate::models::TransferJobState {
            id: "job-1".to_string(),
            session_id: Some("session-a".to_string()),
            display_name: crate::models::TransferJobState::display_name_for_kind(&kind),
            kind,
            status: TransferJobStatus::Running,
            detail: String::new(),
            created_at_ms: 1,
            entries: Vec::new(),
            summary: None,
            progress: Some(progress(
                "/remote/project/src/main.rs",
                10,
                Some(100),
                Some(1),
                Some(4),
            )),
            control: None,
            speed: Default::default(),
        }
        .row_snapshot();

        assert_eq!(transfer_job_file_name(&job), "project");
        assert_eq!(
            transfer_job_icon_path(&job.kind, &job),
            "icons/conn/folder.svg"
        );
    }

    #[test]
    fn status_labels_come_from_i18n_values() {
        let labels = labels();

        assert_eq!(
            transfer_job_status_label(TransferJobStatus::Completed, &labels),
            "completed-i18n"
        );
        assert_eq!(
            transfer_job_status_label(TransferJobStatus::Failed, &labels),
            "failed-i18n"
        );
    }
}

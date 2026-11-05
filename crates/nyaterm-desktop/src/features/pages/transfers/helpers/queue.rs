use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};
use crate::theme::ThemePalette;
use gpui::{
    App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rgb, svg,
};
use nyaterm_files::sftp_transfer_types::SftpTransferProgress;
use nyaterm_ui::NyaTooltip;

pub(in crate::features::pages::transfers) fn queue_action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<String>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip = tooltip.into();
    div()
        .id(SharedString::from(id.into()))
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(if enabled {
            rgb(palette.text_muted)
        } else {
            rgb(palette.text_dimmed)
        })
        .when(enabled, |this| {
            this.cursor_pointer().hover(|this| {
                this.bg(rgb(palette.surface_elevated))
                    .text_color(rgb(palette.text))
            })
        })
        .when(!enabled, |this| this.opacity(0.45))
        .when(enabled, |this| this.on_click(on_click))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(if enabled {
                    rgb(palette.text_muted)
                } else {
                    rgb(palette.text_dimmed)
                }),
        )
}

pub(in crate::features::pages::transfers) fn transfer_job_has_local_target(
    job: &TransferJobState,
) -> bool {
    let download_like = matches!(
        job.kind,
        TransferJobKind::Download { .. }
            | TransferJobKind::OpenExternal { .. }
            | TransferJobKind::ZmodemDownload { .. }
            | TransferJobKind::TrzszDownload { .. }
    );

    download_like
        && (job.summary.is_some()
            || job.progress.is_some()
            || matches!(
                job.kind,
                TransferJobKind::Download { .. } | TransferJobKind::OpenExternal { .. }
            ))
}

pub(in crate::features::pages::transfers) fn transfer_job_can_retry(
    job: &TransferJobState,
) -> bool {
    matches!(
        job.status,
        TransferJobStatus::Failed | TransferJobStatus::Cancelled
    ) && matches!(
        job.kind,
        TransferJobKind::Download { .. } | TransferJobKind::Upload { .. }
    )
}

pub(in crate::features::pages::transfers) fn transfer_progress_ratio(
    progress: &SftpTransferProgress,
) -> Option<f32> {
    if let Some(total) = progress.total_bytes.filter(|total| *total > 0) {
        return Some((progress.bytes_transferred as f32 / total as f32).clamp(0., 1.));
    }
    progress
        .item_count_total
        .filter(|total| *total > 0)
        .zip(progress.item_count_completed)
        .map(|(total, completed)| (completed as f32 / total as f32).clamp(0., 1.))
}

#[cfg(test)]
mod tests {
    use super::transfer_progress_ratio;
    use nyaterm_files::sftp_transfer_types::SftpTransferProgress;

    #[test]
    fn drag_export_never_offers_an_invented_local_target_or_retry() {
        let mut job = crate::models::TransferJobState {
            id: "export".into(),
            session_id: Some("session".into()),
            kind: crate::models::TransferJobKind::DragExport {
                remote_path: "/remote/file".into(),
            },
            status: crate::models::TransferJobStatus::Failed,
            detail: String::new(),
            created_at_ms: 0,
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: Some(progress(4, Some(8), None, None)),
            control: Some(nyaterm_files::sftp::SftpTransferControl::new()),
            speed: Default::default(),
        };
        assert!(job.is_user_transfer());
        assert!(!super::transfer_job_has_local_target(&job));
        assert!(!super::transfer_job_can_retry(&job));
        job.status = crate::models::TransferJobStatus::Completed;
        assert!(!super::transfer_job_has_local_target(&job));
    }

    fn progress(
        bytes_transferred: u64,
        total_bytes: Option<u64>,
        item_count_completed: Option<u64>,
        item_count_total: Option<u64>,
    ) -> SftpTransferProgress {
        SftpTransferProgress {
            remote_path: "/remote".to_string(),
            local_path: std::path::PathBuf::from("/local"),
            bytes_transferred,
            total_bytes,
            item_count_completed,
            item_count_total,
        }
    }

    #[test]
    fn progress_ratio_prefers_bytes_and_falls_back_to_items() {
        assert_eq!(
            transfer_progress_ratio(&progress(50, Some(200), Some(3), Some(4))),
            Some(0.25)
        );
        assert_eq!(
            transfer_progress_ratio(&progress(0, None, Some(3), Some(4))),
            Some(0.75)
        );
        assert_eq!(
            transfer_progress_ratio(&progress(0, None, Some(5), Some(4))),
            Some(1.0)
        );
        assert_eq!(
            transfer_progress_ratio(&progress(0, None, None, None)),
            None
        );
    }
}

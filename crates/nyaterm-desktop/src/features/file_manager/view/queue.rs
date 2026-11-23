use gpui::{AnyElement, Context};
use nyaterm_file_browser_gpui::queue::{
    QueueAction, QueueActionHandler, QueueRow, QueueSnapshot, QueueStatus,
};
use rust_i18n::t;

use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::features::pages::transfers::format_browser_file_size;
use crate::models::{TransferJobKind, TransferJobStatus};
use crate::theme::ThemePalette;

impl FileManagerWorkspace {
    pub(super) fn queue_view(&self, palette: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        let jobs = self.read_app(cx, |app| {
            app.transfer
                .transfer_jobs()
                .iter()
                .filter_map(|job| {
                    let context = app.transfer.file_operation(&job.id).cloned();
                    (context.is_some() || self.show_all_jobs && job.is_user_transfer())
                        .then(|| (job.clone(), context))
                })
                .collect::<Vec<_>>()
        });
        let rows = jobs
            .into_iter()
            .map(|(job, context)| {
                let (source_path, target_path) = match &job.kind {
                    TransferJobKind::SendTo {
                        source_path,
                        target_path,
                        ..
                    } => (source_path.clone(), target_path.clone()),
                    _ => (String::new(), String::new()),
                };
                let labels = context
                    .as_ref()
                    .map(|context| {
                        format!(
                            "{} · {} → {}",
                            t!(context.action_key()),
                            context.source_label,
                            context.target_label
                        )
                    })
                    .unwrap_or_default();
                let tooltip = format!("{labels}\n{source_path}\n{target_path}\n{}", job.detail);
                let progress = job
                    .progress
                    .as_ref()
                    .map(|progress| {
                        let transferred =
                            format_browser_file_size(Some(progress.bytes_transferred));
                        if let Some(total) = progress.total_bytes {
                            format!(
                                "{transferred} / {} · {:.0}%",
                                format_browser_file_size(Some(total)),
                                progress.bytes_transferred.min(total) as f64 / total.max(1) as f64
                                    * 100.
                            )
                        } else {
                            format!("{transferred} · …")
                        }
                    })
                    .unwrap_or_else(|| {
                        if job.status == TransferJobStatus::Completed {
                            format!(
                                "{} · 100%",
                                format_browser_file_size(
                                    job.summary.as_ref().map(|summary| summary.bytes)
                                )
                            )
                        } else {
                            "…".into()
                        }
                    });
                let speed = if job.status == TransferJobStatus::Running {
                    format!(
                        "{}/s",
                        format_browser_file_size(Some(job.speed.bytes_per_second().max(0.) as u64))
                    )
                } else {
                    String::new()
                };
                let name = if job.display_name.is_empty() {
                    job.detail.clone()
                } else {
                    job.display_name.clone()
                };
                QueueRow {
                    id: job.id,
                    detail: job.detail,
                    name,
                    tooltip,
                    labels,
                    progress,
                    speed,
                    status: match job.status {
                        TransferJobStatus::Queued => QueueStatus::Queued,
                        TransferJobStatus::Running => QueueStatus::Running,
                        TransferJobStatus::Paused => QueueStatus::Paused,
                        TransferJobStatus::Cancelling => QueueStatus::Cancelling,
                        TransferJobStatus::Completed => QueueStatus::Completed,
                        TransferJobStatus::Failed => QueueStatus::Failed,
                        TransferJobStatus::Cancelled => QueueStatus::Cancelled,
                    },
                    can_pause: context.as_ref().is_none_or(|context| context.can_pause()),
                    can_retry: context.is_some(),
                }
            })
            .collect();
        let owner = cx.weak_entity();
        let handler: QueueActionHandler = std::rc::Rc::new(move |action, window, cx| {
            let _ = owner.update(cx, |this, cx| {
                match action {
                    QueueAction::Toggle => {
                        this.queue_expanded = !this.queue_expanded;
                        this.save_layout(window, cx);
                    }
                    QueueAction::Filter(filter) => this.queue_filter = filter,
                    QueueAction::ToggleAll => this.show_all_jobs = !this.show_all_jobs,
                    QueueAction::Resize(y) => this.queue_resize = Some((y, this.queue_height)),
                    QueueAction::Pause(id, paused) => {
                        this.dispatch_app(window, cx, move |app, _, cx| {
                            if paused {
                                app.resume_transfer_job(&id, cx);
                            } else {
                                app.pause_transfer_job(&id, cx);
                            }
                        })
                    }
                    QueueAction::Cancel(id) => this.dispatch_app(window, cx, move |app, _, cx| {
                        app.cancel_transfer_job(&id, cx)
                    }),
                    QueueAction::Retry(id) => {
                        this.dispatch_app(window, cx, move |app, window, cx| {
                            app.retry_transfer_job(id, window, cx)
                        })
                    }
                }
                cx.notify();
            });
        });
        nyaterm_file_browser_gpui::queue::queue_view(
            QueueSnapshot {
                rows,
                expanded: self.queue_expanded,
                filter: self.queue_filter,
                show_all: self.show_all_jobs,
                height: self.queue_height,
            },
            palette,
            handler,
        )
    }
}

use gpui::{
    AnyElement, App, CursorStyle, IntoElement, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaTooltip, ThemePalette};
use rust_i18n::t;
use std::rc::Rc;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileManagerQueueFilter {
    All,
    Active,
    Completed,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum QueueStatus {
    Queued,
    Running,
    Paused,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone)]
pub struct QueueRow {
    pub id: String,
    pub status: QueueStatus,
    pub name: String,
    pub detail: String,
    pub tooltip: String,
    pub labels: String,
    pub progress: String,
    pub speed: String,
    pub can_pause: bool,
    pub can_retry: bool,
}
pub struct QueueSnapshot {
    pub rows: Vec<QueueRow>,
    pub expanded: bool,
    pub filter: FileManagerQueueFilter,
    pub show_all: bool,
    pub height: f32,
}
#[derive(Clone)]
pub enum QueueAction {
    Toggle,
    Filter(FileManagerQueueFilter),
    ToggleAll,
    Resize(f32),
    Pause(String, bool),
    Cancel(String),
    Retry(String),
}
pub type QueueActionHandler = Rc<dyn Fn(QueueAction, &mut Window, &mut App)>;
fn on_action(
    handler: &QueueActionHandler,
    action: QueueAction,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let handler = handler.clone();
    move |_, window, cx| handler(action.clone(), window, cx)
}
pub fn queue_view(
    snapshot: QueueSnapshot,
    palette: ThemePalette,
    handler: QueueActionHandler,
) -> AnyElement {
    let jobs = snapshot.rows;
    let active = jobs
        .iter()
        .filter(|job| {
            matches!(
                job.status,
                QueueStatus::Queued
                    | QueueStatus::Running
                    | QueueStatus::Paused
                    | QueueStatus::Cancelling
            )
        })
        .count();
    let failed = jobs
        .iter()
        .filter(|job| job.status == QueueStatus::Failed)
        .count();
    let mut header = div()
        .h(px(30.))
        .min_w_0()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .border_t_1()
        .border_color(rgb(palette.border))
        .child(
            NyaButton::new(
                "fm-queue-toggle",
                format!(
                    "{} · {active} {} · {failed} {}",
                    t!("panel.fileTransfer"),
                    t!("fileManager.filterActive"),
                    t!("fileManager.filterFailed")
                ),
            )
            .small()
            .on_click(on_action(&handler, QueueAction::Toggle)),
        );
    if snapshot.expanded {
        for (filter, key) in [
            (FileManagerQueueFilter::All, "fileManager.filterAll"),
            (FileManagerQueueFilter::Active, "fileManager.filterActive"),
            (
                FileManagerQueueFilter::Completed,
                "fileManager.filterCompleted",
            ),
            (FileManagerQueueFilter::Failed, "fileManager.filterFailed"),
        ] {
            header = header.child(
                NyaButton::new(format!("fm-filter-{filter:?}"), t!(key).to_string())
                    .small()
                    .variant(if snapshot.filter == filter {
                        NyaButtonVariant::Primary
                    } else {
                        NyaButtonVariant::Ghost
                    })
                    .on_click(on_action(&handler, QueueAction::Filter(filter))),
            );
        }
        header = header.child(
            NyaButton::new("fm-all-jobs", t!("fileManager.allJobs").to_string())
                .small()
                .variant(if snapshot.show_all {
                    NyaButtonVariant::Primary
                } else {
                    NyaButtonVariant::Ghost
                })
                .on_click(on_action(&handler, QueueAction::ToggleAll)),
        );
    }
    let mut surface = div()
        .flex_none()
        .min_w_0()
        .flex()
        .flex_col()
        .child(header.overflow_x_scrollbar());
    if snapshot.expanded {
        surface = surface.child(
            div()
                .id("fm-queue-resize")
                .h(px(4.))
                .w_full()
                .cursor(CursorStyle::ResizeUpDown)
                .on_mouse_down(MouseButton::Left, {
                    let handler = handler.clone();
                    move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        handler(QueueAction::Resize(f32::from(event.position.y)), window, cx);
                        cx.stop_propagation();
                    }
                }),
        );
        let mut list = div()
            .id("fm-queue-rows")
            .h(px(snapshot.height))
            .min_w_0()
            .overflow_y_scrollbar()
            .px_2()
            .flex()
            .flex_col()
            .gap_1();
        let mut count = 0;
        for job in jobs.into_iter().filter(|job| match snapshot.filter {
            FileManagerQueueFilter::All => true,
            FileManagerQueueFilter::Active => matches!(
                job.status,
                QueueStatus::Queued
                    | QueueStatus::Running
                    | QueueStatus::Paused
                    | QueueStatus::Cancelling
            ),
            FileManagerQueueFilter::Completed => job.status == QueueStatus::Completed,
            FileManagerQueueFilter::Failed => {
                matches!(job.status, QueueStatus::Failed | QueueStatus::Cancelled)
            }
        }) {
            count += 1;
            let QueueRow {
                tooltip,
                labels,
                progress,
                speed,
                name,
                ..
            } = job.clone();
            let mut actions = div().flex().items_center().gap_1();
            let can_pause = job.can_pause;
            if can_pause
                && matches!(
                    job.status,
                    QueueStatus::Queued | QueueStatus::Running | QueueStatus::Paused
                )
            {
                let id = job.id.clone();
                let paused = job.status == QueueStatus::Paused;
                actions = actions.child(
                    NyaButton::new(
                        format!("fm-pause-{id}"),
                        t!(if paused {
                            "fileTransfer.resume"
                        } else {
                            "fileTransfer.pause"
                        })
                        .to_string(),
                    )
                    .small()
                    .on_click(on_action(&handler, QueueAction::Pause(id, paused))),
                );
            }
            if matches!(
                job.status,
                QueueStatus::Queued | QueueStatus::Running | QueueStatus::Paused
            ) {
                let id = job.id.clone();
                actions = actions.child(
                    NyaButton::new(
                        format!("fm-cancel-{id}"),
                        t!("fileTransfer.cancel").to_string(),
                    )
                    .small()
                    .on_click(on_action(&handler, QueueAction::Cancel(id))),
                );
            }
            if matches!(job.status, QueueStatus::Failed | QueueStatus::Cancelled) && job.can_retry {
                let id = job.id.clone();
                actions = actions.child(
                    NyaButton::new(
                        format!("fm-retry-{id}"),
                        t!("fileTransfer.retry").to_string(),
                    )
                    .small()
                    .on_click(on_action(&handler, QueueAction::Retry(id))),
                );
            }
            list = list.child(
                div()
                    .id(gpui::SharedString::from(format!("fm-job-{}", job.id)))
                    .h(px(54.))
                    .flex_none()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .text_xs()
                            .child(div().truncate().child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!("{labels} · {progress} · {speed}")),
                            ),
                    )
                    .child(
                        div()
                            .max_w(px(240.))
                            .truncate()
                            .text_xs()
                            .text_color(rgb(if job.status == QueueStatus::Failed {
                                palette.warning
                            } else {
                                palette.text_muted
                            }))
                            .child(job.detail),
                    )
                    .child(actions),
            );
        }
        if count == 0 {
            list = list.child(
                div()
                    .py_3()
                    .text_xs()
                    .child(t!("fileTransfer.noTransfers").to_string()),
            );
        }
        surface = surface.child(list);
    }
    surface.into_any_element()
}

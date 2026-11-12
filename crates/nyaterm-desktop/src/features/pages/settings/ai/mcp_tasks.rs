use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_mcp_protocol::{TerminalJobStatus, TerminalProcessState};
use nyaterm_ui::{NyaButton, NyaScrollable};
use rust_i18n::t;

fn status_label(status: TerminalJobStatus) -> String {
    let key = match status {
        TerminalJobStatus::Starting => "starting",
        TerminalJobStatus::Running => "running",
        TerminalJobStatus::Cancelling => "cancelling",
        TerminalJobStatus::Completed => "completed",
        TerminalJobStatus::Failed => "failed",
        TerminalJobStatus::TimedOut => "timedOut",
        TerminalJobStatus::Cancelled => "cancelled",
        TerminalJobStatus::Disconnected => "disconnected",
        TerminalJobStatus::Unknown => "unknown",
    };
    let key = format!("ai.mcp.tasks.{key}");
    t!(&key).to_string()
}

fn process_label(state: TerminalProcessState) -> String {
    let key = match state {
        TerminalProcessState::NotStarted => "notStarted",
        TerminalProcessState::Active => "active",
        TerminalProcessState::ExitConfirmed => "exitConfirmed",
        TerminalProcessState::Unknown => "unknown",
    };
    let key = format!("ai.mcp.tasks.{key}");
    t!(&key).to_string()
}

fn stop_label(state: &str) -> String {
    let key = match state {
        "not_requested" => "notRequested",
        "accepted" => "stopAccepted",
        "sent" => "signalSent",
        "confirmed_ended" | "exit_signal_observed" => "exitConfirmed",
        "observer_unavailable" => "observerUnavailable",
        _ => "unconfirmed",
    };
    let key = format!("ai.mcp.tasks.{key}");
    t!(&key).to_string()
}

impl SettingsPanel {
    pub(super) fn mcp_background_tasks_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.theme_palette();
        let view = self.ai.background_tasks.clone();
        let mut body = div().flex().flex_col().gap_3().child(
            NyaButton::new("mcp-background-tasks", t!("ai.mcp.tasks.title"))
                .small()
                .on_click(cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| app.toggle_background_tasks(cx));
                })),
        );
        if !view.open {
            return body;
        }
        let mut list = div()
            .id("mcp-background-task-list")
            .flex()
            .flex_col()
            .gap_2()
            .max_h(px(240.))
            .overflow_y_scrollbar();
        if view.jobs.is_empty() {
            list = list.child(t!("ai.mcp.tasks.empty"));
        }
        for job in view.jobs {
            let id = job.id.clone();
            let timestamp =
                crate::features::formatting::format_history_timestamp_ms(job.started_at_ms);
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        NyaButton::new(
                            format!("mcp-job-{}", job.id),
                            format!("{} · {}", job.host, status_label(job.status)),
                        )
                        .small()
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel
                                .with_app(cx, |app, cx| app.select_background_task(id.clone(), cx));
                        })),
                    )
                    .child(div().text_size(px(12.)).child(job.command))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(format!("{} · {timestamp}", job.client)),
                    ),
            );
        }
        body = body.child(list);
        if let Some(detail) = view.selected {
            let stop_id = detail.id.clone();
            let connection_id = detail.connection_id;
            let controls = div()
                .flex()
                .gap_2()
                .child(
                    NyaButton::new("mcp-job-stop", t!("ai.mcp.tasks.stop"))
                        .small()
                        .disabled(!detail.can_stop)
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                let _ = app.stop_connection_job(&stop_id, cx);
                            });
                        })),
                )
                .child(
                    NyaButton::new("mcp-job-open-terminal", t!("ai.mcp.tasks.openTerminal"))
                        .small()
                        .on_click(cx.listener(move |panel, _, window, cx| {
                            panel.with_app(cx, |app, cx| {
                                if let Some(connection) = app
                                    .connection_state
                                    .connection_by_id(&connection_id)
                                    .cloned()
                                {
                                    app.start_saved_connection(connection, window, cx);
                                }
                            });
                        })),
                );
            body = body
                .child(controls)
                .child(div().text_size(px(12.)).child(format!(
                        "{}: {} · {}: {}",
                        t!("ai.mcp.tasks.exitCode"),
                        detail
                            .exit_code
                            .map_or_else(|| "—".into(), |code| code.to_string()),
                        t!("ai.mcp.tasks.stopState"),
                        stop_label(&detail.stop_state)
                    )))
                .child(div().text_size(px(12.)).child(format!(
                    "{}: {}",
                    t!("ai.mcp.tasks.processState"),
                    process_label(detail.process_state)
                )))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.mcp.tasks.independentTerminal")),
                )
                .when(detail.truncated, |body| {
                    body.child(t!("ai.mcp.tasks.truncated"))
                })
                .child(
                    div()
                        .id("mcp-job-output")
                        .max_h(px(320.))
                        .overflow_y_scrollbar()
                        .child(
                            div()
                                .font_family("monospace")
                                .text_size(px(12.))
                                .child(detail.output),
                        ),
                );
        }
        body
    }
}

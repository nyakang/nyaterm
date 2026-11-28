use crate::features::formatting::format_ai_message_time;
use crate::features::mcp::activity::model::{ActivityOutcome, ActivityTarget, PAGE_SIZE};
use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaButton, NyaScrollable, NyaSelectOption};
use rust_i18n::t;

fn target_label(target: &ActivityTarget) -> String {
    match &target.name {
        Some(name) => format!("{name} · {}", target.id),
        None => format!(
            "{} · {}",
            if target.connection {
                t!("ai.mcp.activity.deletedConnection")
            } else {
                t!("ai.mcp.activity.closedSession")
            },
            target.id
        ),
    }
}
fn outcome_label(outcome: ActivityOutcome) -> String {
    let key = format!("ai.mcp.activity.{}", outcome.key());
    t!(&key).to_string()
}

impl SettingsPanel {
    pub(in crate::features::pages::settings) fn mcp_activity_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let view = self.ai.mcp_activity.clone();
        let mut body = div().flex().flex_col().gap_3().min_w_0().child(
            NyaButton::new("mcp-activity-toggle", t!("ai.mcp.activity.title"))
                .form_control()
                .on_click(cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| app.toggle_mcp_activity(cx));
                })),
        );
        if !view.open {
            return body;
        }
        let mut clients = vec![NyaSelectOption::new(
            "__all__",
            t!("ai.mcp.activity.allClients"),
        )];
        for client in &view.data.clients {
            clients.push(NyaSelectOption::new(
                format!("client:{client}"),
                client.clone(),
            ));
        }
        if let Some(client) = &view.filter.client
            && !view.data.clients.contains(client)
        {
            clients.push(NyaSelectOption::new(
                format!("client:{client}"),
                client.clone(),
            ));
        }
        let mut targets = vec![NyaSelectOption::new(
            "__all__",
            t!("ai.mcp.activity.allTargets"),
        )];
        for target in &view.data.targets {
            targets.push(NyaSelectOption::new(
                target.key.clone(),
                target_label(target),
            ));
        }
        if let Some(target) = &view.filter.target
            && !view.data.targets.iter().any(|item| &item.key == target)
        {
            targets.push(NyaSelectOption::new(
                target.clone(),
                t!("ai.mcp.activity.unavailableTarget"),
            ));
        }
        let mut outcomes = vec![NyaSelectOption::new(
            "__all__",
            t!("ai.mcp.activity.allResults"),
        )];
        outcomes.extend(
            [
                ActivityOutcome::Success,
                ActivityOutcome::Failed,
                ActivityOutcome::Denied,
                ActivityOutcome::Event,
            ]
            .into_iter()
            .map(|outcome| NyaSelectOption::new(outcome.key(), outcome_label(outcome))),
        );
        let client_select = self.form_select_control(
            "ai-mcp-activity-client",
            clients,
            Some(
                view.filter
                    .client
                    .as_ref()
                    .map(|client| format!("client:{client}"))
                    .unwrap_or_else(|| "__all__".into()),
            ),
            false,
            cx,
        );
        let target_select = self.form_select_control(
            "ai-mcp-activity-target",
            targets,
            Some(
                view.filter
                    .target
                    .clone()
                    .unwrap_or_else(|| "__all__".into()),
            ),
            false,
            cx,
        );
        let outcome_select = self.form_select_control(
            "ai-mcp-activity-outcome",
            outcomes,
            Some(
                view.filter
                    .outcome
                    .map(|outcome| outcome.key().into())
                    .unwrap_or_else(|| "__all__".into()),
            ),
            false,
            cx,
        );
        body = body
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.mcp.activity.description")),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(if self.content_width >= 1024. { 3 } else { 1 })
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .debug_selector(|| "ai-mcp-activity-client".into())
                            .child(client_select),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .debug_selector(|| "ai-mcp-activity-target".into())
                            .child(target_select),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .debug_selector(|| "ai-mcp-activity-outcome".into())
                            .child(outcome_select),
                    ),
            )
            .child(
                NyaButton::new("mcp-activity-refresh", t!("ai.mcp.activity.refresh"))
                    .disabled(view.loading)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.refresh_mcp_activity(cx));
                    })),
            );
        if view.loading {
            body = body.child(div().text_sm().child(t!("ai.mcp.activity.loading")));
        }
        if view.failed {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(rgb(palette.warning))
                    .child(t!("ai.mcp.activity.loadFailed")),
            );
        }
        let mut list = div()
            .id("mcp-activity-list")
            .flex()
            .flex_col()
            .gap_2()
            .max_h(px(380.))
            .overflow_y_scrollbar();
        if view.data.rows.is_empty() && !view.loading && !view.failed {
            list = list.child(if view.data.retained_count == 0 {
                t!("ai.mcp.activity.empty")
            } else {
                t!("ai.mcp.activity.noMatches")
            });
        }
        for row in &view.data.rows {
            let event_key = format!("ai.mcp.activity.{}", row.event);
            let target = row
                .target
                .as_ref()
                .map(target_label)
                .unwrap_or_else(|| t!("ai.mcp.activity.noTarget").to_string());
            let result_color = match row.outcome {
                ActivityOutcome::Failed | ActivityOutcome::Denied => palette.warning,
                _ => palette.text_muted,
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .rounded_md()
                    .child(div().text_size(px(12.)).child(format!(
                        "{} · {}",
                        row.client.as_deref().unwrap_or("—"),
                        target
                    )))
                    .child(div().text_size(px(12.)).child(format!(
                        "{} · {}",
                        row.tool.as_deref().unwrap_or("—"),
                        t!(&event_key)
                    )))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(result_color))
                            .child(format!(
                                "{} · {} · {}",
                                format_ai_message_time(&row.created_at)
                                    .unwrap_or_else(|| "—".into()),
                                outcome_label(row.outcome),
                                row.duration_ms
                                    .map(|duration| format!("{duration} ms"))
                                    .unwrap_or_else(|| "—".into())
                            )),
                    ),
            );
        }
        let pages = view.data.matched_count.div_ceil(PAGE_SIZE).max(1);
        body.child(list).child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    NyaButton::new("mcp-activity-prev", t!("ai.mcp.activity.previous"))
                        .disabled(view.loading || view.data.page == 0)
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.change_mcp_activity_page(false, cx));
                        })),
                )
                .child(format!(
                    "{} / {} · {}",
                    view.data.page + 1,
                    pages,
                    view.data.matched_count
                ))
                .child(
                    NyaButton::new("mcp-activity-next", t!("ai.mcp.activity.next"))
                        .disabled(view.loading || view.data.page + 1 >= pages)
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.change_mcp_activity_page(true, cx));
                        })),
                ),
        )
    }
}

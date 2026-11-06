//! The terminal the panel talks about, and any extra `@` targets.

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, rgba, svg};
use nyaterm_core::truncate_preview;
use nyaterm_ui::NyaTooltip;
use rust_i18n::t;

use super::{AiPanel, AiPanelSnapshot, AiTerminalContextState};

impl AiPanel {
    pub(super) fn ai_context_bar(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = snapshot.chrome.palette;
        let compact = self.layout.is_compact();
        let mut bar = div()
            .id("ai-context-bar")
            .debug_selector(|| "ai-context-bar".to_string())
            .flex_none()
            .w_full()
            .min_w_0()
            .h(px(32.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(palette.border))
            .text_size(px(11.))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/conn/terminal.svg")
                    .text_color(rgb(palette.text_muted)),
            );
        let Some(label) = snapshot.owner_terminal_label.clone() else {
            return bar
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_ellipsis()
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("ai.context.noTerminal")),
                )
                .into_any_element();
        };
        if !compact {
            bar = bar.child(
                div()
                    .flex_none()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.context.currentTerminal")),
            );
        }
        let (status_color, status_label) = match snapshot.owner_terminal_state {
            AiTerminalContextState::Connected => (palette.success, t!("ai.context.connected")),
            AiTerminalContextState::Local => (palette.link, t!("ai.context.local")),
            AiTerminalContextState::Disconnected => (palette.danger, t!("ai.context.disconnected")),
        };
        bar = bar
            .child(
                div()
                    .debug_selector(|| "ai-context-terminal".to_string())
                    .min_w_0()
                    .h(px(20.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(rgb(palette.hover))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .child(truncate_preview(&label, 40)),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "ai-context-status".to_string())
                    .flex_none()
                    .h(px(18.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(status_color))
                    .text_size(px(10.))
                    .text_color(rgb(status_color))
                    .child(div().size(px(6.)).rounded_full().bg(rgb(status_color)))
                    .when(!compact, |pill| pill.child(status_label)),
            );
        let extra: Vec<_> = snapshot.target_sessions.iter().collect();
        if compact && !extra.is_empty() {
            let names = extra
                .iter()
                .map(|target| target.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            bar = bar.child(
                div()
                    .id("ai-context-extra-targets")
                    .debug_selector(|| "ai-context-extra-targets".to_string())
                    .flex_none()
                    .h(px(18.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(rgb(palette.hover))
                    .text_size(px(10.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.link))
                    .child(format!("+{}", extra.len()))
                    .tooltip(move |window, cx| NyaTooltip::new(names.clone()).build(window, cx)),
            );
        } else {
            for target in extra {
                bar =
                    bar.child(self.ai_target_chip(snapshot, &target.session_id, &target.label, cx));
            }
        }
        let mention_disabled = snapshot.running || !snapshot.enabled;
        bar.child(div().flex_1().min_w_0())
            .child(
                div()
                    .id("ai-context-add-target")
                    .debug_selector(|| "ai-context-add-target".to_string())
                    .flex_none()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(if mention_disabled {
                        palette.text_dimmed
                    } else {
                        palette.text_muted
                    }))
                    .when(!mention_disabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(move |this| {
                                this.bg(rgb(palette.surface_elevated))
                                    .text_color(rgb(palette.text))
                            })
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.with_app(cx, |app, cx| {
                                    let draft = app.ai.chat_prompt_draft();
                                    let draft = if draft.is_empty()
                                        || draft.ends_with(char::is_whitespace)
                                    {
                                        format!("{draft}@")
                                    } else {
                                        format!("{draft} @")
                                    };
                                    app.set_ai_prompt_draft(draft, cx);
                                    app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                                });
                            }))
                    })
                    .tooltip(|window, cx| {
                        NyaTooltip::new(t!("ai.context.addTarget")).build(window, cx)
                    })
                    .child("@"),
            )
            .into_any_element()
    }

    fn ai_target_chip(
        &self,
        snapshot: &AiPanelSnapshot,
        session_id: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let remove_id = session_id.to_string();
        div()
            .debug_selector({
                let id = session_id.to_string();
                move || format!("ai-context-target-{id}")
            })
            .min_w_0()
            .max_w(px(160.))
            .h(px(20.))
            .pl_2()
            .pr_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .border_1()
            .border_color(rgb(palette.link))
            .bg(rgba(0x00000000))
            .text_size(px(10.))
            .font_weight(FontWeight(600.))
            .text_color(rgb(palette.link))
            .child(
                div()
                    .min_w_0()
                    .text_ellipsis()
                    .child(truncate_preview(label, 32)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("ai-target-remove-{session_id}")))
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.surface_elevated)))
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        let session_id = remove_id.clone();
                        panel.with_app(cx, move |app, cx| {
                            app.remove_ai_target_session(session_id, cx);
                        });
                    }))
                    .child(
                        svg()
                            .size(px(11.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted)),
                    ),
            )
    }
}

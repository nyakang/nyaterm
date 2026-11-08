//! Prompt box, and the mode, agent, model and execution controls.

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, deferred, div, prelude::*, px, rgb, svg,
};
use nyaterm_core::{AgentCommandExecutionMode, AiAgentKind, AiMode};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaInputShell, NyaMenuAnchor, NyaMenuItem,
    NyaSearchInput,
};
use rust_i18n::t;

use super::components::{agent_kind_display_name, ai_send_button, execution_mode_label};
use super::{AiPanel, AiPanelSnapshot, ai_agent_status_badge};
use crate::features::view_widgets::APP_OVERLAY_PRIORITY;

const CONTROL_HEIGHT: f32 = 28.;

impl AiPanel {
    pub(super) fn ai_composer(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let compact = self.layout.is_compact();
        let prompt_input = NyaInputShell::new("ai.chat.prompt", &snapshot.prompt_input)
            .multi_line()
            .gpui_context_menu([
                t!("menu.cut").into(),
                t!("menu.copy").into(),
                t!("menu.paste").into(),
                t!("menu.selectAll").into(),
            ])
            .into_any_element();
        let model_search_input = snapshot
            .model_search_input
            .as_ref()
            .map(|field| NyaSearchInput::new("ai-model-search", field).into_any_element());
        let composer_disabled = snapshot.history_pending || !snapshot.enabled;
        let send_disabled = !snapshot.running
            && (snapshot.history_pending
                || !snapshot.enabled
                || (!snapshot.external_agent && !snapshot.selected_model_exists)
                || snapshot.prompt_draft.trim().is_empty());

        let mode_controls = div()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .child(self.ai_work_mode_toggle(snapshot, cx))
            .when(snapshot.agent_mode, |row| {
                row.child(self.ai_agent_kind_selector(snapshot, cx))
            });
        let model_control = if snapshot.external_agent {
            ai_agent_status_badge(snapshot).into_any_element()
        } else {
            self.ai_model_selector(snapshot, model_search_input, cx)
                .into_any_element()
        };
        let trailing = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .when(snapshot.agent_mode, |row| {
                row.child(self.ai_execution_mode_chip(snapshot, cx))
            })
            .child(ai_send_button(palette, snapshot.running, send_disabled, cx));
        let footer = if compact {
            div()
                .debug_selector(|| "ai-composer-controls".to_string())
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(mode_controls)
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(model_control)
                        .child(trailing),
                )
        } else {
            div()
                .debug_selector(|| "ai-composer-controls".to_string())
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .child(mode_controls)
                .child(model_control)
                .child(trailing)
        };

        div()
            .debug_selector(|| "ai-composer".to_string())
            .relative()
            .flex_none()
            .border_t_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.transparent_section_header)
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(snapshot.quoted_text.clone(), |this, quoted_text| {
                this.child(self.ai_quote_bar(palette, quoted_text, cx))
            })
            .when(!snapshot.target_sessions.is_empty(), |this| {
                this.child(self.ai_extra_targets(snapshot, cx))
            })
            .child(
                div()
                    .debug_selector(|| "ai-prompt".to_string())
                    .w_full()
                    .relative()
                    .flex()
                    .when(composer_disabled, |this| this.opacity(0.56))
                    .on_key_down(cx.listener(|panel, event: &gpui::KeyDownEvent, _, cx| {
                        if panel.with_app(cx, |app, cx| app.handle_ai_prompt_key_down(event, cx)) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(div().min_w_0().flex_1().child(prompt_input))
                    .when(snapshot.mention_open, |this| {
                        this.child(self.ai_mention_popover(snapshot, cx))
                    }),
            )
            .child(footer)
            .when(snapshot.file_action_ready, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.warning))
                        .child(t!("ai.fileActionReady")),
                )
            })
            // Resize handles paint in the default deferred layer; the menu must
            // paint above them and own the pointer where they overlap.
            .when(snapshot.execution_menu_open, |this| {
                this.child(
                    deferred(self.ai_execution_mode_menu(snapshot, cx))
                        .with_priority(APP_OVERLAY_PRIORITY),
                )
            })
            .into_any_element()
    }

    fn ai_work_mode_toggle(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let segment = |id: &'static str, label: String, selected: bool, mode: AiMode| {
            let restore = snapshot.last_agent_kind.clone();
            div().debug_selector(move || id.to_string()).child(
                NyaButton::new(id, label)
                    .small()
                    .compact()
                    .height(px(CONTROL_HEIGHT - 6.))
                    .selected(selected)
                    .variant(if selected {
                        NyaButtonVariant::Secondary
                    } else {
                        NyaButtonVariant::Ghost
                    })
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        let kind = if mode == AiMode::Ask {
                            AiAgentKind::Nyaterm
                        } else {
                            restore.clone()
                        };
                        let mode = mode.clone();
                        panel.with_app(cx, move |app, cx| {
                            app.set_ai_run_mode(mode, kind, cx);
                            app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                        });
                    })),
            )
        };
        div()
            .debug_selector(|| "ai-work-mode-toggle".to_string())
            .flex_none()
            .h(px(CONTROL_HEIGHT))
            .p(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .child(segment(
                "ai-work-mode-ask",
                t!("ai.modeAsk").to_string(),
                !snapshot.agent_mode,
                AiMode::Ask,
            ))
            .child(segment(
                "ai-work-mode-agent",
                t!("ai.modeAgent").to_string(),
                snapshot.agent_mode,
                AiMode::Agent,
            ))
            .into_any_element()
    }

    fn ai_agent_kind_selector(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let kinds = [
            (AiAgentKind::Nyaterm, true),
            (AiAgentKind::Codex, snapshot.codex_enabled),
            (AiAgentKind::ClaudeCode, snapshot.claude_code_enabled),
        ];
        let items = kinds
            .into_iter()
            .map(|(kind, enabled)| {
                let selected = snapshot.agent_kind == kind;
                let name = agent_kind_display_name(&kind);
                let label = if enabled {
                    name.to_string()
                } else {
                    format!("{name} · {}", t!("ai.agentKindDisabled"))
                };
                NyaMenuItem::action(label)
                    .checked(selected)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        let kind = kind.clone();
                        panel.with_app(cx, move |app, cx| {
                            app.set_ai_run_mode(AiMode::Agent, kind, cx);
                            app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                        });
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .debug_selector(|| "ai-agent-kind-control".to_string())
            .h(px(CONTROL_HEIGHT))
            .min_w_0()
            .max_w(px(if self.layout.is_compact() { 140. } else { 120. }))
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input_surface()))
            .child(
                NyaDropdownMenu::new("ai-agent-kind-selector")
                    .anchor(NyaMenuAnchor::BottomLeft)
                    .min_width(px(180.))
                    .tooltip(t!("ai.agentKind"))
                    .content(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(px(11.))
                                    .text_ellipsis()
                                    .child(agent_kind_display_name(&snapshot.agent_kind)),
                            )
                            .child(
                                svg()
                                    .size(px(12.))
                                    .flex_none()
                                    .path("icons/chevron-down.svg"),
                            ),
                    )
                    .items(items),
            )
            .into_any_element()
    }

    fn ai_execution_mode_chip(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let mode = &snapshot.command_execution_mode;
        let icon = match mode {
            AgentCommandExecutionMode::Auto => "icons/ai/exec-auto.svg",
            AgentCommandExecutionMode::Smart => "icons/ai/exec-smart.svg",
            AgentCommandExecutionMode::ConfirmEach => "icons/ai/exec-confirm.svg",
        };
        let label = if self.layout.is_compact() {
            String::new()
        } else {
            execution_mode_label(mode)
        };
        let mut button = NyaButton::new("ai-execution-mode-chip", label)
            .small()
            .compact()
            .height(px(CONTROL_HEIGHT))
            .icon(icon)
            .variant(NyaButtonVariant::Ghost)
            .selected(snapshot.execution_menu_open)
            .disabled(snapshot.running)
            .tooltip(format!(
                "{} · {}",
                t!("ai.agentCommandExecutionMode"),
                execution_mode_label(mode)
            ))
            .on_click(cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| {
                    app.ai.toggle_execution_menu();
                    app.defer_ai_panel_snapshot_flush(cx);
                });
            }));
        if *mode == AgentCommandExecutionMode::Auto {
            button = button.text_color(rgb(palette.warning));
        }
        div()
            .debug_selector(|| "ai-execution-mode-chip".to_string())
            .flex_none()
            // The chip is part of its popover: keep its mouse-down away from
            // the outside-click handler so a second click closes the menu.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button)
            .into_any_element()
    }
}

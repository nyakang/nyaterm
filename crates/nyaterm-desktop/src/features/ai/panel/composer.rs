//! Prompt box, and the mode, agent, model and execution controls.

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, MouseDownEvent, SharedString, deferred, div,
    prelude::*, px, rgb, svg,
};
use nyaterm_core::{AgentCommandExecutionMode, AiAgentKind, AiMode};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaInputShell, NyaMenuAnchor, NyaMenuItem,
    NyaScrollable, NyaSearchInput,
};
use rust_i18n::t;

use super::components::{agent_kind_display_name, ai_send_button, execution_mode_label};
use super::layout::{COMPOSER_GAP, COMPOSER_HEIGHT_MAX, COMPOSER_PADDING};
use super::{AI_COMPOSER_RESIZE_HANDLE_ID, AiPanel, AiPanelSnapshot, ai_agent_status_badge};
use crate::features::view_widgets::{APP_OVERLAY_PRIORITY, horizontal_resize_handle_borderless};

const CONTROL_HEIGHT: f32 = 28.;

impl AiPanel {
    pub(super) fn ai_composer(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let compact = self.layout.is_compact();
        let minimum = snapshot
            .prompt_input
            .read(cx)
            .textarea_min_height(cx)
            .unwrap_or(px(38.));
        let limits = self.composer_layout.limits(minimum);
        let scroll_context = limits
            .is_some_and(|limits| self.composer_layout.context_height + minimum > limits.editable);
        let prompt_input = NyaInputShell::new("ai.chat.prompt", &snapshot.prompt_input)
            .multi_line()
            .height_limits(
                px(snapshot.composer_min_height),
                limits.map_or(COMPOSER_HEIGHT_MAX, |limits| limits.prompt),
            )
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
        }
        .flex_none();

        let prompt_index = usize::from(snapshot.quoted_text.is_some())
            + usize::from(!snapshot.target_sessions.is_empty());
        let body_entity = cx.weak_entity();
        let composer_entity = body_entity.clone();
        let editable = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(COMPOSER_GAP)
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(panel) = body_entity.upgrade() else {
                    return;
                };
                let prompt_height = bounds[prompt_index].size.height;
                let context_height = bounds
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != prompt_index)
                    .map(|(_, bounds)| bounds.size.height)
                    .sum::<gpui::Pixels>()
                    + COMPOSER_GAP * bounds.len().saturating_sub(1) as f32;
                let layout = panel.read(cx).composer_layout;
                if (layout.context_height - context_height).abs() <= px(0.5)
                    && (layout.prompt_height - prompt_height).abs() <= px(0.5)
                {
                    return;
                }
                cx.defer(move |cx| {
                    panel.update(cx, |panel, cx| {
                        let layout = panel.composer_layout;
                        if (layout.context_height - context_height).abs() <= px(0.5)
                            && (layout.prompt_height - prompt_height).abs() <= px(0.5)
                        {
                            return;
                        }
                        panel.composer_layout.context_height = context_height;
                        panel.composer_layout.prompt_height = prompt_height;
                        cx.notify();
                    })
                });
            })
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
                    .flex_none()
                    .flex()
                    .when(composer_disabled, |this| this.opacity(0.56))
                    .on_key_down(cx.listener(|panel, event: &gpui::KeyDownEvent, _, cx| {
                        if panel.with_app(cx, |app, cx| app.handle_ai_prompt_key_down(event, cx)) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(div().min_w_0().flex_1().child(prompt_input))
                    .when(snapshot.mention_open && !scroll_context, |this| {
                        this.child(self.ai_mention_popover(snapshot, cx))
                    }),
            )
            .when(snapshot.file_action_ready, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.warning))
                        .child(t!("ai.fileActionReady")),
                )
            });
        let editable_viewport = div()
            .min_h_0()
            .min_w_0()
            .w_full()
            .when_some(limits, |this, limits| this.max_h(limits.editable))
            .child(editable);
        let editable_viewport = if scroll_context {
            editable_viewport.overflow_y_scrollbar().into_any_element()
        } else {
            editable_viewport.into_any_element()
        };
        // Keep the viewport's layout outside the wrapper's scroll-translated
        // content node; the footer and menus remain siblings of this shell.
        let editable_viewport = div()
            .debug_selector(|| "ai-composer-editable-viewport".to_string())
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(limits, |this, limits| this.max_h(limits.editable))
            .child(editable_viewport);

        div()
            .debug_selector(|| "ai-composer".to_string())
            .relative()
            .flex_none()
            .border_t_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.transparent_section_header)
            .p(COMPOSER_PADDING)
            .flex()
            .flex_col()
            .gap(COMPOSER_GAP)
            .when_some(limits, |this, limits| this.max_h(limits.composer))
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(panel) = composer_entity.upgrade() else {
                    return;
                };
                let footer_height = bounds[1].size.height;
                if panel.read(cx).composer_layout.footer_height == footer_height {
                    return;
                }
                cx.defer(move |cx| {
                    panel.update(cx, |panel, cx| {
                        if panel.composer_layout.footer_height != footer_height {
                            panel.composer_layout.footer_height = footer_height;
                            cx.notify();
                        }
                    })
                });
            })
            .child(editable_viewport)
            .child(footer)
            .when(snapshot.mention_open && scroll_context, |this| {
                this.child(self.ai_mention_popover(snapshot, cx))
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

    /// Top-edge strip that pins the prompt box taller for long drafts. It sits
    /// between the transcript and the composer like the transfer queue's
    /// divider, and the drag itself stays on the app through `with_app` for the
    /// same reason.
    pub(super) fn ai_composer_resize_handle(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = SharedString::from(AI_COMPOSER_RESIZE_HANDLE_ID);
        let hover_id = id.clone();
        let drag_id = id.clone();
        deferred(
            horizontal_resize_handle_borderless(
                snapshot.chrome.palette,
                snapshot.composer_resizing,
                snapshot.composer_handle_active,
            )
            .id(id.clone())
            .debug_selector(|| "ai-composer-resize-handle".to_string())
            .cursor_row_resize()
            .on_hover(cx.listener(move |panel, hovered: &bool, _, cx| {
                let hovered = *hovered;
                let hover_id = hover_id.clone();
                panel.with_app(cx, |app, cx| {
                    app.update_resize_handle_hover(hover_id, hovered, cx);
                });
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                    let event = event.clone();
                    let drag_id = drag_id.clone();
                    let start_height = panel.composer_visible_height();
                    panel.with_app(cx, |app, cx| {
                        app.activate_resize_handle_immediately(drag_id, cx);
                        app.start_ai_composer_resize(&event, start_height, cx);
                    });
                }),
            ),
        )
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

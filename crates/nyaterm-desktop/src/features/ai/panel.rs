use std::sync::Arc;

use rust_i18n::t;

use gpui::{
    App, ClickEvent, ClipboardItem, Context, Entity, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, Rgba, SharedString, WeakEntity, Window, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::{
    AgentCommandExecutionMode, AiAction, AiAgentKind, AiCommandCard, AiMessage, AiMessageRole,
    AiMode, AiModelConfigItem, AiSession, truncate_preview,
};
use nyaterm_ui::{NyaInputShell, NyaScrollable, NyaSearchInput};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    ai_agent_step_status_style, extract_think_content, group_ai_sessions_by_date, short_id,
};
use crate::features::shell::gpui_code_font_family;
use crate::features::text_inputs::TextInputSetup;
use crate::features::view_widgets::{
    full_window_input_layer, markdown_content_view, tab_menu_separator,
};
use crate::models::{
    AiDetectedErrorState, AiMessageMenuState, AiPreparedRequest, NavItem, SettingsTab,
};
use crate::theme::ThemePalette;
use crate::widgets::{mode_button, small_button, status_pill, svg_icon_button};

use crate::features::runtime_jobs::{AiAgentStepStatus, AiAgentStepView};

mod components;
use components::{
    AiCommandCardPresentation, ai_message_menu_button, ai_message_menu_position, ai_send_button,
    ai_setup_step, ai_user_pre_wrap_text,
};

#[derive(Clone, Copy)]
pub(in crate::features) struct AiPanelChrome {
    pub palette: ThemePalette,
    pub transparent_surface: Rgba,
    pub transparent_section_header: Rgba,
    pub surface: Rgba,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

#[derive(Clone)]
pub(in crate::features) struct AiModelChoice {
    pub model: AiModelConfigItem,
    pub provider_label: String,
}

#[derive(Clone)]
pub(in crate::features) struct AiMentionCandidate {
    pub session_id: String,
    pub label: String,
    pub kind: String,
    pub selected: bool,
}

#[derive(Clone)]
pub(in crate::features) struct AiTargetSession {
    pub session_id: String,
    pub label: String,
}

#[derive(Clone)]
pub(in crate::features) struct AiAgentStepPresentation {
    pub step: AiAgentStepView,
    pub thought_open: bool,
    pub output_open: bool,
}

#[derive(Clone)]
pub(in crate::features) struct AiPanelSnapshot {
    pub chrome: AiPanelChrome,
    pub enabled: bool,
    pub agent_mode: bool,
    pub running: bool,
    pub agent_kind: AiAgentKind,
    pub external_agent: bool,
    pub selected_model_id: Option<String>,
    pub selected_model_exists: bool,
    pub model_label: String,
    pub enabled_models: Arc<[AiModelConfigItem]>,
    pub model_choices: Arc<[AiModelChoice]>,
    pub discovery_menu_open: bool,
    pub discovery_index: usize,
    pub prompt_draft: String,
    pub prompt_input: Entity<nyaterm_ui::NyaInputState>,
    pub model_search_input: Option<Entity<nyaterm_ui::NyaInputState>>,
    pub history_search_input: Option<Entity<nyaterm_ui::NyaInputState>>,
    pub file_action_ready: bool,
    pub messages: Arc<[Arc<AiMessage>]>,
    pub streaming_assistant_id: Option<String>,
    pub command_cards: Arc<[AiCommandCard]>,
    pub agent_steps: Arc<[AiAgentStepPresentation]>,
    pub target_sessions: Arc<[AiTargetSession]>,
    pub mention_open: bool,
    pub mention_index: usize,
    pub mention_candidates: Arc<[AiMentionCandidate]>,
    pub quoted_text: Option<String>,
    pub detected_error: Option<AiDetectedErrorState>,
    pub message_menu: Option<AiMessageMenuState>,
    pub history_open: bool,
    pub history_query: String,
    pub history_sessions: Arc<[AiSession]>,
    pub history_pending: bool,
    pub history_actions_disabled: bool,
    pub execution_menu_open: bool,
    pub command_execution_mode: AgentCommandExecutionMode,
    pub background_execution_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct AiHeaderPresentation {
    pub running: bool,
    pub selected_model_id: Option<String>,
    pub model_label: String,
    pub execution_mode: AgentCommandExecutionMode,
}

pub(in crate::features) struct AiPanel {
    app: WeakEntity<NyaTermApp>,
    snapshot: Option<AiPanelSnapshot>,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    snapshot_set_count: usize,
}

impl AiPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>) -> Self {
        Self {
            app,
            snapshot: None,
            #[cfg(test)]
            paint_count: 0,
            #[cfg(test)]
            snapshot_set_count: 0,
        }
    }

    pub(in crate::features) fn set_snapshot(
        &mut self,
        snapshot: AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.snapshot = Some(snapshot);
        #[cfg(test)]
        {
            self.snapshot_set_count += 1;
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::features) fn snapshot(&self) -> Option<&AiPanelSnapshot> {
        self.snapshot.as_ref()
    }

    #[cfg(test)]
    pub(in crate::features) fn paint_count(&self) -> usize {
        self.paint_count
    }

    #[cfg(test)]
    pub(in crate::features) fn snapshot_set_count(&self) -> usize {
        self.snapshot_set_count
    }

    pub(in crate::features) fn with_app<R: Default>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>) -> R,
    ) -> R {
        let Some(app) = self.app.upgrade() else {
            return R::default();
        };
        app.update(cx, |app, cx| {
            let result = f(app, cx);
            app.defer_ai_panel_snapshot_flush(cx);
            result
        })
    }

    fn panel(&self) -> Option<&AiPanelSnapshot> {
        self.snapshot.as_ref()
    }

    fn palette(&self) -> ThemePalette {
        self.panel()
            .map(|snapshot| snapshot.chrome.palette)
            .expect("AI panel render requires a snapshot")
    }

    fn render_panel(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(snapshot) = self.snapshot.clone() else {
            return div().size_full().into_any_element();
        };
        let palette = snapshot.chrome.palette;
        let command_rows = self.ai_command_card_list(&snapshot, cx);
        let agent_step_rows = self.ai_agent_step_list(&snapshot, cx);
        let prompt_input = NyaInputShell::new("ai.chat.prompt", &snapshot.prompt_input)
            .multi_line()
            .into_any_element();
        let model_search_input = snapshot
            .model_search_input
            .as_ref()
            .map(|field| NyaSearchInput::new("ai-model-search", field).into_any_element());
        let composer_disabled = snapshot.running || !snapshot.enabled;
        let send_disabled = !snapshot.running
            && (!snapshot.enabled
                || (!snapshot.external_agent && !snapshot.selected_model_exists)
                || snapshot.prompt_draft.trim().is_empty());

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(snapshot.chrome.transparent_surface)
            .relative()
            .when_some(snapshot.detected_error.clone(), |this, detected| {
                this.child(self.ai_detected_error_banner(&snapshot, detected, cx))
            })
            .child(
                div()
                    .id(SharedString::from("ai-transcript-scroll"))
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .overflow_y_scrollbar()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.ai_transcript_body(&snapshot, agent_step_rows, command_rows, cx)),
            )
            .child(
                div()
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
                        this.child(self.ai_target_sessions_row(&snapshot, cx))
                    })
                    .when(snapshot.mention_open, |this| {
                        this.child(self.ai_mention_popover(&snapshot, cx))
                    })
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .when(composer_disabled, |this| this.opacity(0.56))
                            .on_key_down(cx.listener(|panel, event: &gpui::KeyDownEvent, _, cx| {
                                if panel.with_app(cx, |app, cx| {
                                    app.handle_ai_prompt_key_down(event, cx)
                                }) {
                                    cx.stop_propagation();
                                }
                            }))
                            .child(div().min_w_0().flex_1().child(prompt_input)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(self.ai_mode_switch(&snapshot, cx))
                                    .when(snapshot.external_agent, |this| {
                                        this.child(ai_agent_status_badge(&snapshot))
                                    })
                                    .when(!snapshot.external_agent, |this| {
                                        this.child(self.ai_model_selector(
                                            &snapshot,
                                            model_search_input,
                                            cx,
                                        ))
                                    }),
                            )
                            .child(ai_send_button(palette, snapshot.running, send_disabled, cx)),
                    )
                    .when(snapshot.file_action_ready, |this| {
                        this.child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(palette.warning))
                                .child(t!("ai.fileActionReady")),
                        )
                    }),
            )
            // Absolute popovers must paint after the transcript and composer;
            // later siblings otherwise cover an open menu even though its
            // position is correct.
            .when(snapshot.history_open, |this| {
                this.child(self.ai_history_popover(&snapshot, cx))
            })
            .when(snapshot.execution_menu_open, |this| {
                this.child(self.ai_execution_mode_menu(&snapshot, cx))
            })
            .when_some(snapshot.message_menu.clone(), |this, menu| {
                this.child(self.ai_message_context_menu_overlay(&snapshot, menu, cx))
            })
            .into_any_element()
    }

    fn ai_detected_error_banner(
        &self,
        snapshot: &AiPanelSnapshot,
        detected: AiDetectedErrorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let analyze_state = detected.clone();
        div()
            .flex_none()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(rgba(0xf59e0b1a))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(0xd97706))
                            .child(t!("ai.errorDetected")),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(format!("session {}", short_id(&detected.session_id))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(small_button(
                        palette,
                        "ai-detected-error-analyze",
                        "Analyze",
                        cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.analyze_ai_detected_error(analyze_state.clone(), cx);
                            });
                        }),
                    ))
                    .child(small_button(
                        palette,
                        "ai-detected-error-close",
                        "Close",
                        cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.dismiss_ai_detected_error(cx);
                            });
                        }),
                    )),
            )
    }

    fn ai_quote_bar(
        &self,
        palette: ThemePalette,
        quoted_text: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.link))
            .bg(rgb(palette.hover))
            .flex()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .child(div().w(px(3.)).h(px(28.)).flex_none().bg(rgb(palette.link)))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(palette.link))
                    .child(t!("ai.quote")),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .overflow_hidden()
                    .child(truncate_preview(quoted_text.trim(), 140)),
            )
            .child(
                div()
                    .id(SharedString::from("ai-quote-clear"))
                    .size(px(20.))
                    .mr_1()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_color(rgb(palette.text_muted))
                    .cursor_pointer()
                    .hover(move |this| {
                        this.bg(rgb(palette.surface_elevated))
                            .text_color(rgb(palette.text))
                    })
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.clear_ai_quote(cx);
                        });
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted)),
                    ),
            )
    }

    fn ai_target_sessions_row(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let mut target_row = div().flex().flex_wrap().items_center().gap_1().child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text_muted))
                .child(format!("{}:", t!("ai.targetSession"))),
        );
        for target in snapshot.target_sessions.iter() {
            let session_id = target.session_id.clone();
            let label = target.label.clone();
            target_row = target_row.child(
                div()
                    .min_w_0()
                    .max_w(px(220.))
                    .h(px(20.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(palette.link))
                    .bg(rgb(palette.hover))
                    .text_size(px(10.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.link))
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .flex_none()
                            .bg(rgb(palette.link)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(truncate_preview(&label, 32)),
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
                            .hover(move |this| {
                                this.bg(rgb(palette.surface_elevated))
                                    .text_color(rgb(palette.danger))
                            })
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                let session_id = session_id.clone();
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
                    ),
            );
        }
        target_row
    }

    fn ai_mention_popover(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let mut popover = div()
            .max_h(px(192.))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.surface)
            .shadow_lg()
            .flex()
            .flex_col()
            .p_1();
        if snapshot.mention_candidates.is_empty() {
            return popover
                .child(
                    div()
                        .h(px(44.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.noSessions")),
                )
                .into_any_element();
        }
        for (index, candidate) in snapshot.mention_candidates.iter().enumerate().take(8) {
            let focused = index == snapshot.mention_index;
            let candidate = candidate.clone();
            popover = popover.child(
                div()
                    .id(SharedString::from(format!(
                        "ai-mention-session-{}",
                        candidate.session_id
                    )))
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .bg(if focused || candidate.selected {
                        rgb(palette.hover)
                    } else {
                        rgba(0x00000000)
                    })
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, move |app, cx| {
                            app.ai.set_chat_mention_index(index);
                            app.select_ai_mention_candidate(cx);
                        });
                    }))
                    .child(div().size(px(7.)).rounded_full().flex_none().bg(
                        if candidate.selected {
                            rgb(palette.link)
                        } else {
                            rgb(palette.text_dimmed)
                        },
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(truncate_preview(&candidate.label, 34)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(candidate.kind),
                    ),
            );
        }
        popover.into_any_element()
    }

    fn ai_mode_switch(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        div()
            .h(px(28.))
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .p(px(1.))
            .gap_0()
            .child(mode_button(
                "ai-mode-ask",
                "Ask",
                !snapshot.agent_mode,
                palette,
                cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.set_ai_mode(AiMode::Ask, cx);
                    });
                }),
            ))
            .child(mode_button(
                "ai-mode-agent",
                "Agent",
                snapshot.agent_mode,
                palette,
                cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.set_ai_mode(AiMode::Agent, cx);
                    });
                }),
            ))
    }

    fn ai_model_selector(
        &self,
        snapshot: &AiPanelSnapshot,
        model_search_input: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let selected_id = snapshot.selected_model_id.clone();
        let discovery_menu_open = snapshot.discovery_menu_open;
        let model_label = snapshot.model_label.clone();
        div()
            .min_w_0()
            .flex_1()
            .relative()
            .child(
                div()
                    .id(SharedString::from("ai-model-selector"))
                    .min_w_0()
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .hover(move |this| {
                        this.border_color(rgb(palette.link))
                            .text_color(rgb(palette.text))
                    })
                    .on_click(cx.listener(|panel, _, window, cx| {
                        panel.with_app(cx, |app, cx| {
                            let selected_index = app.ai_selected_model_index();
                            let opening = app.ai.toggle_discovery_menu(selected_index);
                            if opening {
                                app.reset_text_input("ai.model-search", "", cx);
                                let field = app.text_input(
                                    "ai.model-search",
                                    "",
                                    TextInputSetup::placeholder("Search models"),
                                    cx,
                                );
                                window.focus(&field.read(cx).focus_handle(), cx);
                            }
                        });
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(truncate_preview(&model_label, 24)),
                    )
                    .child(
                        svg()
                            .size(px(14.))
                            .flex_none()
                            .path("icons/chevron-down.svg")
                            .text_color(rgb(palette.text_dimmed)),
                    ),
            )
            .when(discovery_menu_open, |this| {
                this.child(self.ai_model_menu(snapshot, selected_id, model_search_input, cx))
            })
    }

    fn ai_model_menu(
        &self,
        snapshot: &AiPanelSnapshot,
        selected_id: Option<String>,
        model_search_input: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let mut menu = div()
            .absolute()
            // The selector shares its row with the mode switch and send
            // button. Expanding from its left edge made the fixed-width menu
            // cross the side-panel boundary; anchoring the trailing edges
            // keeps the popup inside the panel at its normal width.
            .right_0()
            .bottom(px(34.))
            .w(px(260.))
            .max_h(px(280.))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.surface)
            .shadow_lg()
            .p_1()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if snapshot.enabled_models.is_empty() {
            return menu
                .child(
                    div()
                        .px_2()
                        .py_2()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.noEnabledModels")),
                )
                .child(
                    div()
                        .id(SharedString::from("ai-model-open-settings"))
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(palette.link))
                        .hover(move |this| this.bg(rgb(palette.hover)))
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.ai.close_discovery_menu();
                                app.shell.set_settings_active_tab(SettingsTab::AiModels);
                                app.open_page(NavItem::Settings, cx);
                            });
                        }))
                        .child(t!("ai.models")),
                );
        }
        if let Some(model_search_input) = model_search_input {
            menu = menu.child(
                div()
                    .mb_1()
                    .on_key_down(cx.listener(|panel, event: &gpui::KeyDownEvent, _, cx| {
                        if panel
                            .with_app(cx, |app, cx| app.handle_ai_model_search_key_down(event, cx))
                        {
                            cx.stop_propagation();
                        }
                    }))
                    .child(model_search_input),
            );
        }
        if snapshot.model_choices.is_empty() {
            return menu.child(
                div()
                    .h(px(52.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.noModelMatches")),
            );
        }
        let mut rows = div()
            .id(SharedString::from("ai-model-choice-list"))
            .max_h(px(220.))
            .overflow_y_scrollbar()
            .flex()
            .flex_col();
        for (index, choice) in snapshot.model_choices.iter().enumerate() {
            let model = choice.model.clone();
            let provider_label = choice.provider_label.clone();
            let model_id = model.id.clone();
            let is_selected = selected_id.as_deref() == Some(model.id.as_str());
            let focused = index == snapshot.discovery_index;
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("ai-model-choice-{}", model.id)))
                    .h(px(34.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .bg(if focused || is_selected {
                        rgb(palette.hover)
                    } else {
                        rgba(0x00000000)
                    })
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        let model_id = model_id.clone();
                        panel.with_app(cx, move |app, cx| {
                            app.ai.set_discovery_index(index);
                            app.ai.close_discovery_menu();
                            app.set_ai_default_model(model_id, cx);
                        });
                    }))
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(palette.link))
                            .when(is_selected, |this| {
                                this.child(
                                    svg()
                                        .size(px(13.))
                                        .path("icons/check.svg")
                                        .text_color(rgb(palette.link)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(truncate_preview(&model.name, 32)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(120.))
                            .overflow_hidden()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(truncate_preview(&provider_label, 24)),
                    ),
            );
        }
        menu.child(rows)
    }

    fn ai_transcript_body(
        &self,
        snapshot: &AiPanelSnapshot,
        agent_step_rows: impl IntoElement,
        command_rows: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut body = div().min_w_0().w_full().flex().flex_col().gap_2();
        if snapshot.messages.is_empty() {
            body = body.child(self.ai_empty_transcript(snapshot, cx));
        } else {
            for message in snapshot.messages.iter() {
                body = body.child(self.ai_message_bubble(snapshot, message, cx));
            }
        }
        body.child(agent_step_rows).child(command_rows)
    }

    fn ai_empty_transcript(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = snapshot.chrome.palette;
        let has_model = snapshot.selected_model_id.is_some() || !snapshot.enabled_models.is_empty();
        if !snapshot.enabled {
            return div()
                .min_h(px(192.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .px_3()
                .child(
                    svg()
                        .size(px(36.))
                        .path("icons/ai.svg")
                        .text_color(rgb(palette.text_muted)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.goToSettingsToEnable")),
                )
                .into_any_element();
        }
        if !has_model {
            return div()
                .min_h(px(240.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .px_4()
                .child(
                    div()
                        .size(px(48.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(0x9e6a03))
                        .bg(rgb(0x3d2e00))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::features::view_widgets::mono_icon(
                            "icons/warning.svg",
                            rgb(palette.warning).into(),
                            22.,
                        )),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .child(t!("ai.setupTitle")),
                )
                .child(ai_setup_step(palette, "1", t!("ai.setupStep1")))
                .child(ai_setup_step(palette, "2", t!("ai.setupStep2")))
                .child(
                    div()
                        .id(SharedString::from("ai-empty-open-settings-setup"))
                        .mt_1()
                        .h(px(30.))
                        .px_3()
                        .rounded_md()
                        .bg(rgb(palette.success))
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(0xffffff))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(0x2ea043)))
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.shell.set_settings_active_tab(SettingsTab::AiGeneral);
                                app.open_page(NavItem::Settings, cx);
                            });
                        }))
                        .child(t!("ai.setupAction")),
                )
                .into_any_element();
        }
        div()
            .min_h(px(180.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_3()
            .child(
                svg()
                    .size(px(40.))
                    .path("icons/ai.svg")
                    .text_color(rgb(palette.text_muted)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.empty")),
            )
            .into_any_element()
    }

    fn ai_message_bubble(
        &self,
        snapshot: &AiPanelSnapshot,
        message: &AiMessage,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let is_user = matches!(message.role, AiMessageRole::User);
        let streaming = snapshot
            .streaming_assistant_id
            .as_deref()
            .is_some_and(|id| id == message.id);
        let role_label = if is_user { "User" } else { "AI" };
        let raw = if message.content.trim().is_empty() {
            String::new()
        } else {
            message.content.clone()
        };
        let (visible, think_reasoning) = extract_think_content(&raw);
        let mut reasoning = message
            .reasoning_content
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if reasoning.is_none() {
            reasoning = think_reasoning;
        }
        let display = if visible.trim().is_empty() {
            if streaming { String::new() } else { visible }
        } else {
            visible
        };
        let menu_text = if display.trim().is_empty() {
            raw.clone()
        } else {
            display.clone()
        };
        let menu_message_id = message.id.clone();

        let mut bubble = div()
            .id(SharedString::from(format!("ai-msg-{}", message.id)))
            .min_w_0()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(if is_user {
                rgb(0x1f6feb)
            } else {
                rgb(palette.border)
            })
            .bg(if is_user {
                rgb(palette.hover)
            } else {
                rgb(palette.bg)
            })
            .px_2()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let menu = AiMessageMenuState {
                        message_id: menu_message_id.clone(),
                        text: menu_text.clone(),
                        x: event.position.x,
                        y: event.position.y,
                    };
                    panel.with_app(cx, move |app, _| {
                        app.ai.open_message_menu(menu);
                    });
                }),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_muted))
                    .child(role_label),
            );

        if let Some(reasoning) = reasoning {
            bubble = bubble.child(
                div()
                    .min_w_0()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(if streaming {
                        rgb(0x1f6feb)
                    } else {
                        rgb(palette.border)
                    })
                    .bg(if streaming {
                        rgb(palette.hover)
                    } else {
                        rgb(palette.bg)
                    })
                    .px_2()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight(700.))
                            .text_color(if streaming {
                                rgb(palette.link)
                            } else {
                                rgb(palette.text_muted)
                            })
                            .child(if streaming {
                                t!("ai.thinking")
                            } else {
                                t!("ai.thoughtComplete")
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .w_full()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .line_height(px(16.))
                            .child(markdown_content_view(
                                palette,
                                &truncate_preview(&reasoning, 1200),
                            )),
                    ),
            );
        } else if streaming && display.trim().is_empty() {
            bubble = bubble.child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x1f6feb))
                    .bg(rgb(palette.hover))
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(palette.link))
                    .child(t!("ai.thinking")),
            );
        }

        if !display.trim().is_empty() {
            if is_user {
                bubble = bubble.child(ai_user_pre_wrap_text(palette, &display));
            } else {
                bubble = bubble.child(markdown_content_view(
                    palette,
                    &truncate_preview(&display, 8000),
                ));
            }
        }

        for (card_index, card) in message.command_cards.iter().cloned().enumerate() {
            bubble = bubble.child(self.ai_command_card_view_for_card(
                palette,
                format!("{}-{}", message.id, card_index),
                card,
                cx,
            ));
        }
        bubble
    }

    fn ai_agent_step_list(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = snapshot.chrome.palette;
        let mut rows = div();
        if snapshot.agent_mode || !snapshot.agent_steps.is_empty() {
            rows = rows
                .mt_2()
                .border_t_1()
                .border_color(rgb(palette.border))
                .pt_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.agentSteps")),
                );
            if snapshot.agent_steps.is_empty() {
                rows = rows.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("ai.agentNoSteps")),
                );
            } else {
                for step in snapshot.agent_steps.iter().rev().take(16).rev() {
                    rows = rows.child(self.ai_agent_step_card(palette, step.clone(), cx));
                }
            }
        }
        rows.into_any_element()
    }

    fn ai_agent_step_card(
        &self,
        palette: ThemePalette,
        presentation: AiAgentStepPresentation,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let step = presentation.step;
        let (label, fg, bg) = ai_agent_step_status_style(step.status);
        let border = match step.status {
            AiAgentStepStatus::Completed => rgb(palette.success),
            AiAgentStepStatus::Failed | AiAgentStepStatus::Cancelled => rgb(palette.danger),
            AiAgentStepStatus::Running | AiAgentStepStatus::Tool => rgb(palette.link),
            AiAgentStepStatus::NeedsApproval => rgb(palette.warning),
            AiAgentStepStatus::Planning => rgb(palette.text_muted),
        };
        let step_index = step.step_index;
        let thought_open = presentation.thought_open;
        let output_open = presentation.output_open;
        let thought = step
            .thought
            .clone()
            .filter(|value| !value.trim().is_empty());
        let command = step
            .command
            .clone()
            .or_else(|| {
                if step.detail.trim().is_empty()
                    || thought
                        .as_ref()
                        .is_some_and(|thought| thought == &step.detail)
                {
                    None
                } else {
                    Some(step.detail.clone())
                }
            })
            .filter(|value| !value.trim().is_empty());
        let observation = step
            .observation
            .clone()
            .filter(|value| !value.trim().is_empty());
        let thought_label = if thought.is_some() {
            if thought_open {
                "Hide thought"
            } else {
                "Show thought"
            }
        } else if matches!(
            step.status,
            AiAgentStepStatus::Completed | AiAgentStepStatus::Planning
        ) {
            "Step"
        } else {
            ""
        };

        let mut card = div()
            .id(SharedString::from(format!("ai-agent-step-{step_index}")))
            .flex()
            .flex_col()
            .gap_1()
            .pb_2()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "ai-agent-step-thought-toggle-{step_index}"
                    )))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, move |app, cx| {
                            app.toggle_ai_agent_thought_expanded(step_index, cx);
                        });
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path(if thought_open {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/fe/forward.svg"
                            })
                            .text_color(rgb(palette.text_muted)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(format!("#{}", step.step_index.saturating_add(1))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .overflow_hidden()
                            .child(if thought_label.is_empty() {
                                truncate_preview(&step.title, 36)
                            } else {
                                format!("{} · {}", thought_label, truncate_preview(&step.title, 28))
                            }),
                    )
                    .child(status_pill(label, rgb(fg), rgb(bg))),
            );

        if thought_open && let Some(thought_text) = thought.clone() {
            card = card.child(
                div()
                    .ml_4()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .line_height(px(16.))
                    .child(markdown_content_view(
                        palette,
                        &truncate_preview(&thought_text, 800),
                    )),
            );
        }

        if let Some(command_text) = command {
            let mut shell = div()
                .ml_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .border_l_2()
                .border_color(border)
                .bg(rgb(palette.bg))
                .overflow_hidden()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(palette.surface_elevated))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight(700.))
                                .text_color(rgb(palette.text_muted))
                                .child("SHELL"),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(truncate_preview(&step.title, 24)),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .font_family(gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .line_height(px(16.))
                        .child(truncate_preview(&command_text, 600)),
                );

            if let Some(obs) = observation.clone() {
                shell = shell.child(
                    div()
                        .id(SharedString::from(format!(
                            "ai-agent-step-output-toggle-{step_index}"
                        )))
                        .px_2()
                        .py_1()
                        .border_t_1()
                        .border_color(rgb(palette.surface_elevated))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .hover(move |this| this.bg(rgb(palette.surface)))
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, move |app, cx| {
                                app.toggle_ai_agent_output_expanded(step_index, cx);
                            });
                        }))
                        .child(
                            svg()
                                .size(px(13.))
                                .flex_none()
                                .path(if output_open {
                                    "icons/chevron-down.svg"
                                } else {
                                    "icons/fe/forward.svg"
                                })
                                .text_color(rgb(palette.text_muted)),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .child(if output_open {
                                    "Hide output"
                                } else {
                                    "Show output"
                                }),
                        ),
                );
                if output_open {
                    shell = shell.child(
                        div()
                            .px_2()
                            .py_1()
                            .max_h(px(120.))
                            .overflow_hidden()
                            .font_family(gpui_code_font_family())
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .line_height(px(14.))
                            .child(truncate_preview(&obs, 1200)),
                    );
                }
            } else if matches!(
                step.status,
                AiAgentStepStatus::Running | AiAgentStepStatus::Tool
            ) {
                shell = shell.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_t_1()
                        .border_color(rgb(palette.surface_elevated))
                        .text_size(px(10.))
                        .text_color(rgb(palette.link))
                        .child(t!("ai.agentExecuting")),
                );
            }
            card = card.child(shell);
        } else if let Some(obs) = observation {
            card = card.child(
                div()
                    .ml_4()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.bg))
                    .px_2()
                    .py_1()
                    .font_family(gpui_code_font_family())
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(truncate_preview(&obs, 400)),
            );
        }

        card
    }

    fn ai_command_card_list(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut rows = div().mt_2().flex().flex_col().gap_2();
        for (index, card) in snapshot.command_cards.iter().take(8).cloned().enumerate() {
            rows = rows.child(self.ai_command_card_view(snapshot.chrome.palette, index, card, cx));
        }
        rows.into_any_element()
    }

    fn ai_command_card_view(
        &self,
        palette: ThemePalette,
        index: usize,
        card: AiCommandCard,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.ai_command_card_shell(
            AiCommandCardPresentation::new(palette, format!("idx-{index}"), card),
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, move |app, cx| {
                    app.insert_ai_command_card(index, cx);
                });
            }),
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, move |app, cx| {
                    app.save_ai_command_card(index, cx);
                });
            }),
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, move |app, cx| {
                    app.run_ai_command_card(index, cx);
                });
            }),
            cx,
        )
    }

    fn ai_command_card_view_for_card(
        &self,
        palette: ThemePalette,
        key: String,
        card: AiCommandCard,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let card_id = card.id.clone();
        let insert_id = card_id.clone();
        let save_id = card_id.clone();
        let run_id = card_id;
        self.ai_command_card_shell(
            AiCommandCardPresentation::new(palette, key, card),
            cx.listener(move |panel, _, _, cx| {
                let insert_id = insert_id.clone();
                panel.with_app(cx, move |app, cx| {
                    app.insert_ai_command_card_by_id(insert_id, cx);
                });
            }),
            cx.listener(move |panel, _, _, cx| {
                let save_id = save_id.clone();
                panel.with_app(cx, move |app, cx| {
                    app.save_ai_command_card_by_id(save_id, cx);
                });
            }),
            cx.listener(move |panel, _, _, cx| {
                let run_id = run_id.clone();
                panel.with_app(cx, move |app, cx| {
                    app.run_ai_command_card_by_id(run_id, cx);
                });
            }),
            cx,
        )
    }

    fn ai_command_card_shell(
        &self,
        presentation: AiCommandCardPresentation,
        on_insert: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_save: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let AiCommandCardPresentation {
            palette,
            key,
            risk,
            title,
            command,
            explanation,
            expected,
            rollback,
        } = presentation;
        let command_for_copy = command.clone();
        div()
            .id(SharedString::from(format!("ai-command-card-{key}")))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .overflow_hidden()
                            .child(truncate_preview(&title, 48)),
                    )
                    .child(status_pill(risk, rgb(palette.warning), rgb(palette.hover))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("ai-command-body-{key}")))
                    .max_h(px(128.))
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface))
                    .px_2()
                    .py_1()
                    .font_family(gpui_code_font_family())
                    .text_size(px(11.))
                    .text_color(rgb(palette.text))
                    .line_height(px(16.))
                    .child(truncate_preview(&command, 1600)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .line_height(px(16.))
                            .child(truncate_preview(&explanation, 320)),
                    )
                    .when(!expected.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .line_height(px(16.))
                                .child(truncate_preview(&expected, 220)),
                        )
                    })
                    .when(!rollback.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .line_height(px(16.))
                                .child(format!("Rollback: {}", truncate_preview(&rollback, 160))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(small_button(
                        palette,
                        format!("ai-command-insert-{key}"),
                        "Insert",
                        on_insert,
                    ))
                    .child(small_button(
                        palette,
                        format!("ai-command-copy-{key}"),
                        "Copy",
                        cx.listener(move |panel, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                command_for_copy.clone(),
                            ));
                            panel.with_app(cx, |app, _| {
                                app.ai.set_panel_status("command copied");
                            });
                        }),
                    ))
                    .child(small_button(
                        palette,
                        format!("ai-command-save-{key}"),
                        "Save",
                        on_save,
                    ))
                    .child(small_button(
                        palette,
                        format!("ai-command-run-{key}"),
                        "Run",
                        on_run,
                    )),
            )
            .into_any_element()
    }

    fn ai_execution_mode_menu(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        div()
            .id(SharedString::from("ai-execution-mode-menu"))
            .absolute()
            .top(px(4.))
            .right(px(8.))
            .w(px(260.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.surface)
            .shadow_lg()
            .py_1()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text))
                    .child(t!("ai.agentCommandExecutionMode")),
            )
            .child(self.ai_execution_mode_item(
                "ai-exec-confirm",
                t!("ai.executionModeConfirmEach"),
                t!("ai.executionModeConfirmEachDesc"),
                AgentCommandExecutionMode::ConfirmEach,
                snapshot.command_execution_mode == AgentCommandExecutionMode::ConfirmEach,
                cx,
            ))
            .child(self.ai_execution_mode_item(
                "ai-exec-smart",
                t!("ai.executionModeSmart"),
                t!("ai.executionModeSmartDesc"),
                AgentCommandExecutionMode::Smart,
                snapshot.command_execution_mode == AgentCommandExecutionMode::Smart,
                cx,
            ))
            .child(self.ai_execution_mode_item(
                "ai-exec-auto",
                t!("ai.executionModeAuto"),
                t!("ai.executionModeAutoDesc"),
                AgentCommandExecutionMode::Auto,
                snapshot.command_execution_mode == AgentCommandExecutionMode::Auto,
                cx,
            ))
            .child(tab_menu_separator(palette))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text))
                    .child(t!("ai.executionMethod")),
            )
            .child(self.ai_background_execution_item(snapshot, cx))
    }

    fn ai_background_execution_item(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let enabled = snapshot.background_execution_enabled;
        div()
            .id(SharedString::from("ai-exec-background"))
            .px_3()
            .py_2()
            .flex()
            .items_start()
            .gap_2()
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(palette.surface_elevated)))
            .on_click(cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| {
                    app.toggle_ai_background_execution(cx);
                });
            }))
            .child(
                div()
                    .mt(px(1.))
                    .size(px(14.))
                    .rounded_sm()
                    .border_1()
                    .border_color(if enabled {
                        rgb(palette.link)
                    } else {
                        rgb(palette.border)
                    })
                    .bg(if enabled {
                        rgb(palette.link)
                    } else {
                        rgb(palette.input)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(palette.bg))
                    .when(enabled, |this| {
                        this.child(
                            svg()
                                .size(px(11.))
                                .path("icons/check.svg")
                                .text_color(rgb(palette.bg)),
                        )
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("ai.backgroundAgentExecution")),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("ai.backgroundAgentExecutionDesc")),
                    ),
            )
    }

    fn ai_execution_mode_item(
        &self,
        id: &'static str,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        mode: AgentCommandExecutionMode,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title: SharedString = title.into();
        let detail: SharedString = detail.into();
        let palette = self.palette();
        div()
            .id(SharedString::from(id))
            .px_3()
            .py_2()
            .flex()
            .items_start()
            .gap_2()
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(palette.surface_elevated)))
            .on_click(cx.listener(move |panel, _, window, cx| {
                let mode = mode.clone();
                panel.with_app(cx, move |app, cx| {
                    if mode == AgentCommandExecutionMode::Auto
                        && app.ai.settings_config().agent_command_execution_mode
                            != AgentCommandExecutionMode::Auto
                    {
                        app.open_ai_auto_execution_confirm(window, cx);
                        return;
                    }
                    app.set_ai_command_mode(mode.clone(), cx);
                    app.ai.close_execution_menu();
                    app.ai.set_panel_status(format!(
                        "Agent execution mode: {}",
                        match mode {
                            AgentCommandExecutionMode::ConfirmEach => "confirm each",
                            AgentCommandExecutionMode::Smart => "smart",
                            AgentCommandExecutionMode::Auto => "auto",
                        }
                    ));
                });
            }))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(if selected {
                                rgb(palette.link)
                            } else {
                                rgb(palette.text)
                            })
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(detail),
                    ),
            )
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .text_color(rgb(palette.link))
                    .when(selected, |this| {
                        this.child(
                            svg()
                                .size(px(13.))
                                .path("icons/check.svg")
                                .text_color(rgb(palette.link)),
                        )
                    }),
            )
    }

    fn ai_history_popover(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let query = snapshot.history_query.trim().to_ascii_lowercase();
        let filtered: Vec<_> = snapshot
            .history_sessions
            .iter()
            .filter(|session| {
                query.is_empty()
                    || session.title.to_ascii_lowercase().contains(&query)
                    || session.id.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let total_count = snapshot.history_sessions.len();
        let filtered_count = filtered.len();
        let grouped = group_ai_sessions_by_date(&filtered);
        let mut search_input = snapshot.history_search_input.as_ref().map(|field| {
            NyaSearchInput::new("ai-history-search", field).on_key_down(cx.listener(
                |panel, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        panel.with_app(cx, |app, cx| {
                            app.ai.close_history();
                            app.forget_text_inputs("ai.history-search");
                            app.defer_ai_panel_snapshot_flush(cx);
                        });
                    }
                },
            ))
        });
        if !snapshot.history_query.is_empty()
            && let Some(input) = search_input.take()
        {
            search_input = Some(
                input.trailing(
                    div()
                        .id(SharedString::from("ai-history-search-clear"))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .cursor_pointer()
                        .hover(move |this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.ai.clear_history_query();
                                app.reset_text_input("ai.history-search", "", cx);
                            });
                        }))
                        .child(
                            svg()
                                .size(px(13.))
                                .path("icons/window/close.svg")
                                .text_color(rgb(palette.text_muted)),
                        ),
                ),
            );
        }

        let mut rows = div().flex().flex_col().gap_1().p_2();
        if filtered_count == 0 {
            rows = rows.child(
                div()
                    .py_4()
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(if snapshot.history_pending {
                        "Loading history..."
                    } else if total_count == 0 {
                        "No chat history yet"
                    } else {
                        "No matching history"
                    }),
            );
        } else {
            for (group, sessions) in grouped {
                if sessions.is_empty() {
                    continue;
                }
                rows = rows.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(10.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(group.label()),
                );
                for session in sessions.into_iter().take(48) {
                    let session_id = session.id.clone();
                    let delete_id = session.id.clone();
                    let active = snapshot
                        .messages
                        .first()
                        .is_some_and(|message| message.session_id == session.id);
                    rows = rows.child(
                        div()
                            .id(SharedString::from(format!("ai-session-{}", session.id)))
                            .h(px(32.))
                            .px_2()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_1()
                            .bg(if active {
                                rgb(palette.hover)
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(move |this| this.bg(rgb(palette.surface_elevated)))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "ai-session-open-{}",
                                        session.id
                                    )))
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.text))
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .child(truncate_preview(&session.title, 28))
                                    .on_click(cx.listener(move |panel, _, _, cx| {
                                        let session_id = session_id.clone();
                                        panel.with_app(cx, move |app, cx| {
                                            app.load_ai_session_messages(session_id, cx);
                                        });
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(format!(
                                        "{}{}",
                                        agent_kind_label(&session.agent_kind),
                                        if session.external_session_id.is_some() {
                                            " · resume"
                                        } else {
                                            ""
                                        }
                                    )),
                            )
                            .child(svg_icon_button(
                                format!("ai-session-delete-{}", session.id),
                                "icons/fe/delete.svg",
                                14.,
                                palette,
                                cx.listener(move |panel, _, _, cx| {
                                    let delete_id = delete_id.clone();
                                    panel.with_app(cx, move |app, cx| {
                                        app.delete_ai_session(delete_id, cx);
                                    });
                                }),
                            )),
                    );
                }
            }
        }

        div()
            .id(SharedString::from("ai-history-popover"))
            .absolute()
            .top(px(4.))
            .left(px(8.))
            .right(px(8.))
            .max_h(px(352.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.surface)
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(search_input, |this, search_input| {
                this.child(
                    div()
                        .p_2()
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .child(search_input),
                )
            })
            .child(
                div()
                    .h(px(32.))
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(t!("ai.history")),
                    )
                    .child(
                        div()
                            .id(SharedString::from("ai-history-clear-all"))
                            .h(px(22.))
                            .px_2()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .text_color(if snapshot.history_actions_disabled {
                                rgb(palette.border)
                            } else {
                                rgb(palette.text_muted)
                            })
                            .when(!snapshot.history_actions_disabled, |this| {
                                this.cursor_pointer().hover(move |this| {
                                    this.bg(rgb(palette.surface_elevated))
                                        .text_color(rgb(palette.text))
                                })
                            })
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.with_app(cx, |app, cx| {
                                    if app.ai.history_actions_are_disabled() {
                                        return;
                                    }
                                    app.open_ai_clear_history_confirm(window, cx);
                                });
                            }))
                            .child(t!("ai.clearHistory")),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from("ai-history-scroll"))
                    .flex_1()
                    .min_h_0()
                    .max_h(px(280.))
                    .overflow_scrollbar()
                    .child(rows),
            )
    }

    fn ai_message_context_menu_overlay(
        &self,
        snapshot: &AiPanelSnapshot,
        state: AiMessageMenuState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let quote_text = state.text.clone();
        let copy_text = state.text.clone();
        let (menu_x, menu_y, menu_max_h) = ai_message_menu_position(
            f32::from(state.x),
            f32::from(state.y),
            128.,
            64.,
            snapshot.chrome.viewport_width,
            snapshot.chrome.viewport_height,
        );
        full_window_input_layer("ai-message-context-menu-overlay")
            .on_click(cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| {
                    app.close_ai_message_menu(cx);
                });
            }))
            .child(
                div()
                    .id(SharedString::from("ai-message-context-menu"))
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(128.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(snapshot.chrome.surface)
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .max_h(px(menu_max_h))
                            .overflow_y_scrollbar()
                            .py_1()
                            .flex()
                            .flex_col()
                            .child(ai_message_menu_button(
                                palette,
                                "ai-message-menu-quote",
                                "icons/quote.svg",
                                t!("ai.quote"),
                                cx.listener(move |panel, _, _, cx| {
                                    let quote_text = quote_text.clone();
                                    panel.with_app(cx, move |app, cx| {
                                        app.quote_ai_message_text(quote_text, cx);
                                    });
                                }),
                            ))
                            .child(ai_message_menu_button(
                                palette,
                                "ai-message-menu-copy",
                                "icons/copy.svg",
                                t!("ai.copy"),
                                cx.listener(move |panel, _, _, cx| {
                                    let copy_text = copy_text.clone();
                                    panel.with_app(cx, move |app, cx| {
                                        app.copy_ai_message_text(copy_text, cx);
                                    });
                                }),
                            )),
                    ),
            )
    }
}

impl gpui::Render for AiPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.paint_count += 1;
        }
        self.render_panel(cx)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn ai_header_presentation(&self) -> AiHeaderPresentation {
        let selected_model_id = self.ai_selected_model_id();
        let model_label = selected_model_id
            .as_deref()
            .and_then(|model_id| {
                self.ai
                    .settings_config()
                    .models
                    .iter()
                    .find(|model| model.id == model_id)
                    .map(|model| truncate_preview(&model.name, 28))
            })
            .unwrap_or_else(|| t!("ai.notConfigured").to_string());
        AiHeaderPresentation {
            running: self.ai.chat_or_agent_is_running(),
            selected_model_id,
            model_label,
            execution_mode: self
                .ai
                .settings_config()
                .agent_command_execution_mode
                .clone(),
        }
    }

    pub(in crate::features) fn notify_root_if_ai_header_changed(
        &self,
        before: AiHeaderPresentation,
        cx: &mut Context<Self>,
    ) -> bool {
        if before == self.ai_header_presentation() {
            return false;
        }
        cx.notify();
        true
    }

    pub(in crate::features) fn dismiss_ai_detected_error(&mut self, cx: &mut Context<Self>) {
        self.ai.dismiss_detected_error();
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn close_ai_message_menu(&mut self, cx: &mut Context<Self>) {
        self.ai.close_message_menu();
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn quote_ai_message_text(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.ai.quote_message(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn copy_ai_message_text(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let value = text.trim().to_string();
        let copied = !value.is_empty();
        if copied {
            cx.write_to_clipboard(ClipboardItem::new_string(value));
        }
        self.ai.finish_copy_message(copied);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn clear_ai_quote(&mut self, cx: &mut Context<Self>) {
        self.ai.clear_quote();
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn analyze_ai_detected_error(
        &mut self,
        detected: AiDetectedErrorState,
        cx: &mut Context<Self>,
    ) {
        if self.ai.chat_or_agent_is_running() {
            self.ai
                .set_chat_response_preview("AI request already running");
            self.ai.set_panel_status("AI request already running");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let mut context = self.ai_terminal_context_for_session(Some(&detected.session_id));
        context.selected_text = detected.output.clone();
        let request = AiPreparedRequest {
            action: AiAction::AnalyzeError,
            context,
            source_label: "Detected terminal error".to_string(),
        };
        self.ai
            .prepare_detected_error_request(request, detected.session_id.clone());
        self.set_ai_prompt_draft("Analyze detected error", cx);
        self.start_ai_ask(cx);
    }

    pub(in crate::features) fn open_ai_clear_history_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ai.request_history_clear_confirm() {
            return;
        }
        self.open_confirm_dialog(
            (
                t!("ai.clearHistoryTitle").to_string(),
                t!("ai.clearHistoryDesc").to_string(),
                t!("ai.clearHistory").to_string(),
                true,
                |app, _, cx| app.confirm_ai_clear_history(cx),
            ),
            window,
            cx,
        );
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn confirm_ai_clear_history(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ai.confirm_history_clear() {
            return false;
        }
        self.clear_all_ai_history(cx);
        true
    }

    pub(in crate::features) fn open_ai_auto_execution_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ai.request_agent_auto_confirm();
        self.open_confirm_dialog(
            (
                t!("ai.autoExecutionConfirmTitle").to_string(),
                t!("ai.autoExecutionConfirmDesc").to_string(),
                t!("ai.enableAutoExecution").to_string(),
                true,
                |app, _, cx| app.confirm_ai_auto_execution(cx),
            ),
            window,
            cx,
        );
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn confirm_ai_auto_execution(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self.ai_header_presentation();
        if !self.ai.confirm_agent_auto_execution() {
            return false;
        }
        self.persist_ai_settings_now(cx);
        self.defer_ai_panel_snapshot_flush(cx);
        self.notify_root_if_ai_header_changed(before, cx);
        true
    }

    pub(in crate::features) fn defer_ai_panel_snapshot_flush(&mut self, cx: &mut Context<Self>) {
        if !self.ai.request_panel_refresh() {
            return;
        }
        self.defer_app_update(cx, |app, cx| {
            if !app.ai.take_panel_refresh_request() {
                return;
            }
            app.flush_ai_panel_snapshot(cx);
        });
    }

    pub(in crate::features) fn flush_ai_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        self.ai.clear_panel_refresh_request();
        let snapshot = self.build_ai_panel_snapshot(cx);
        let panel = self.ai_panel.clone();
        panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
    }

    fn build_ai_panel_snapshot(&mut self, cx: &mut Context<Self>) -> AiPanelSnapshot {
        let palette = self.theme_palette();
        let enabled = self.ai.settings_config().enabled;
        let agent_mode = self.ai.settings_config().default_mode == AiMode::Agent;
        let running = self.ai.chat_or_agent_is_running();
        let agent_kind = self.ai.settings_config().default_agent_kind.clone();
        let external_agent = agent_mode && agent_kind != AiAgentKind::Nyaterm;
        let selected_model_id = self.ai_selected_model_id();
        let enabled_models: Arc<[AiModelConfigItem]> = self.ai_enabled_models().into();
        let selected_model_exists = selected_model_id
            .as_deref()
            .is_some_and(|model_id| enabled_models.iter().any(|model| model.id == model_id));
        let model_label = selected_model_id
            .as_deref()
            .and_then(|model_id| enabled_models.iter().find(|model| model.id == model_id))
            .map(|model| model.name.clone())
            .unwrap_or_else(|| t!("ai.notConfigured").to_string());
        let model_choices_vec = self.ai_filtered_model_choices();
        self.ai.clamp_discovery_index(model_choices_vec.len());
        let model_choices = model_choices_vec
            .into_iter()
            .map(|(model, provider_label)| AiModelChoice {
                model,
                provider_label,
            })
            .collect::<Vec<_>>()
            .into();
        let target_session_ids = self.ai.chat_target_session_ids().to_vec();
        let target_sessions = target_session_ids
            .iter()
            .filter_map(|session_id| {
                self.session
                    .session_info(session_id)
                    .map(|session| AiTargetSession {
                        session_id: session_id.clone(),
                        label: self.session.display_name_by_info(&session),
                    })
            })
            .collect::<Vec<_>>()
            .into();
        let mention_candidates: Arc<[AiMentionCandidate]> = if self.ai.chat_mention_is_open() {
            self.ai_mention_candidates()
                .into_iter()
                .map(|session| AiMentionCandidate {
                    selected: target_session_ids
                        .iter()
                        .any(|session_id| session_id == &session.id),
                    kind: crate::features::formatting::session_kind_label(session.kind).to_string(),
                    label: self.session.display_name_by_info(&session),
                    session_id: session.id,
                })
                .collect::<Vec<_>>()
                .into()
        } else {
            Vec::<AiMentionCandidate>::new().into()
        };
        self.ai.clamp_chat_mention_index(mention_candidates.len());

        let prompt_placeholder = if !enabled {
            "Go to Settings to enable AI"
        } else if agent_mode {
            "Describe a task for the agent..."
        } else {
            "Ask about the terminal or generate a command..."
        };
        let prompt_draft = self.ai.chat_prompt_draft().to_string();
        self.ensure_text_input(
            "ai.chat.prompt",
            &prompt_draft,
            TextInputSetup::multi_line(prompt_placeholder).submit_on_enter(),
            cx,
        );
        let prompt_input = self
            .existing_text_input("ai.chat.prompt")
            .expect("AI prompt input was just built");

        let model_search_input = if self.ai.discovery_menu_is_open() {
            let query = self.ai.discovery_query().to_string();
            self.ensure_text_input(
                "ai.model-search",
                &query,
                TextInputSetup::placeholder("Search models"),
                cx,
            );
            self.existing_text_input("ai.model-search")
        } else {
            None
        };
        let history_search_input = if self.ai.history_is_open() {
            let query = self.ai.history_query().to_string();
            self.ensure_text_input(
                "ai.history-search",
                &query,
                TextInputSetup::placeholder("Search history..."),
                cx,
            );
            self.existing_text_input("ai.history-search")
        } else {
            None
        };
        let (viewport_width, viewport_height) = self.shell.viewport_size();

        AiPanelSnapshot {
            chrome: AiPanelChrome {
                palette,
                transparent_surface: self.shell_transparent_color(palette.surface),
                transparent_section_header: self.shell_transparent_color(palette.section_header),
                surface: self.shell_surface_color(palette.surface),
                viewport_width,
                viewport_height,
            },
            enabled,
            agent_mode,
            running,
            agent_kind,
            external_agent,
            selected_model_id,
            selected_model_exists,
            model_label,
            enabled_models,
            model_choices,
            discovery_menu_open: self.ai.discovery_menu_is_open(),
            discovery_index: self.ai.discovery_index(),
            prompt_draft,
            prompt_input,
            model_search_input,
            history_search_input,
            file_action_ready: self
                .ai
                .chat_prepared_request()
                .is_some_and(|request| request.action == AiAction::CustomFileAction),
            messages: self.ai.chat_snapshot_messages(),
            streaming_assistant_id: self.ai.chat_streaming_assistant_id().map(str::to_string),
            command_cards: self.ai.chat_command_cards().to_vec().into(),
            agent_steps: self
                .ai
                .agent_steps()
                .iter()
                .cloned()
                .map(|step| AiAgentStepPresentation {
                    thought_open: self.ai.agent_thought_is_expanded(step.step_index),
                    output_open: self.ai.agent_output_is_expanded(step.step_index),
                    step,
                })
                .collect::<Vec<_>>()
                .into(),
            target_sessions,
            mention_open: self.ai.chat_mention_is_open(),
            mention_index: self.ai.chat_mention_index(),
            mention_candidates,
            quoted_text: self.ai.chat_quote().map(str::to_string),
            detected_error: self.ai.panel_detected_error().cloned(),
            message_menu: self.ai.chat_message_menu().cloned(),
            history_open: self.ai.history_is_open(),
            history_query: self.ai.history_query().to_string(),
            history_sessions: self.ai.history_sessions().to_vec().into(),
            history_pending: self.ai.history_is_pending(),
            history_actions_disabled: self.ai.history_actions_are_disabled(),
            execution_menu_open: self.ai.panel_execution_menu_is_open(),
            command_execution_mode: self
                .ai
                .settings_config()
                .agent_command_execution_mode
                .clone(),
            background_execution_enabled: self
                .ai
                .settings_config()
                .agent_background_execution_enabled,
        }
    }
}

fn agent_kind_label(kind: &AiAgentKind) -> &'static str {
    match kind {
        AiAgentKind::Nyaterm => "NyaTerm",
        AiAgentKind::Codex => "Codex",
        AiAgentKind::ClaudeCode => "Claude",
    }
}

fn ai_agent_status_badge(snapshot: &AiPanelSnapshot) -> impl IntoElement {
    let palette = snapshot.chrome.palette;
    div()
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .text_size(px(10.))
        .text_color(rgb(if snapshot.running {
            palette.success
        } else {
            palette.text_muted
        }))
        .child(format!(
            "{} · {}MCP-only",
            agent_kind_label(&snapshot.agent_kind),
            if snapshot.running { "running · " } else { "" }
        ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div, px,
    };
    use nyaterm_core::{
        AgentCommandExecutionMode, AiMode, AiModelConfigItem, AiModelSource, AiProviderKind,
        AiSettings, AppRuntime, RuntimeMode,
    };
    use nyaterm_ui::NyaInputEvent;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::{NyaTermApp, runtime_jobs::AiChatJobOutput};
    use crate::test_support::TestConfigDir;

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let app = self.app.read(cx);
            div()
                .w(px(360.))
                .h(px(720.))
                .flex()
                .gap_1()
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        app.ai_panel
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
                .child(
                    div().w(px(260.)).min_h_0().overflow_hidden().child(
                        app.connection_panel
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
                .child(
                    div().w(px(260.)).min_h_0().overflow_hidden().child(
                        app.transfer_panel
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
                .child(
                    div().w(px(260.)).min_h_0().overflow_hidden().child(
                        app.settings_panel
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
        }
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.flush_ai_panel_snapshot(cx);
            app.flush_connection_panel_snapshot(cx);
            app.flush_transfer_panel_snapshot(cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost {
            app: host_app.clone(),
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn draw(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    fn ai_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).ai_panel.read(cx).paint_count()
    }

    fn ai_snapshot_sets(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).ai_panel.read(cx).snapshot_set_count()
    }

    fn connection_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).connection_panel.read(cx).paint_count()
    }

    fn transfer_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).transfer_panel.read(cx).paint_count()
    }

    fn settings_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).settings_panel.read(cx).paint_count()
    }

    #[test]
    fn detected_terminal_error_refreshes_ai_panel_only() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let before_snapshots = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
        let before_ai_paints = vcx.update(|_, cx| ai_paints(&app, cx));
        let before_connection_paints = vcx.update(|_, cx| connection_paints(&app, cx));
        let before_transfer_paints = vcx.update(|_, cx| transfer_paints(&app, cx));
        let before_settings_paints = vcx.update(|_, cx| settings_paints(&app, cx));

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(app.ai.note_detected_error(
                    "session-a".to_string(),
                    "permission denied".to_string(),
                    Instant::now(),
                ));
                app.defer_ai_panel_snapshot_flush(cx);
            });
        });
        vcx.run_until_parked();

        assert_eq!(
            vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
            before_snapshots + 1,
            "detected terminal errors should rebuild the AiPanel snapshot"
        );
        draw(&app, vcx);
        assert!(
            vcx.update(|_, cx| ai_paints(&app, cx)) > before_ai_paints,
            "the AiPanel should repaint after its snapshot changes"
        );
        assert_eq!(
            vcx.update(|_, cx| connection_paints(&app, cx)),
            before_connection_paints,
            "AI-owned refreshes must not repaint the connections panel"
        );
        assert_eq!(
            vcx.update(|_, cx| transfer_paints(&app, cx)),
            before_transfer_paints,
            "AI-owned refreshes must not repaint the transfer panel"
        );
        assert_eq!(
            vcx.update(|_, cx| settings_paints(&app, cx)),
            before_settings_paints,
            "AI-owned refreshes must not repaint the settings panel"
        );
    }

    #[test]
    fn repeated_ai_refresh_requests_coalesce() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let before = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.defer_ai_panel_snapshot_flush(cx);
                app.defer_ai_panel_snapshot_flush(cx);
                app.defer_ai_panel_snapshot_flush(cx);
            });
        });
        vcx.run_until_parked();

        let after = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
        assert_eq!(
            after,
            before + 1,
            "same-cycle refresh requests should build/set one snapshot"
        );

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.defer_ai_panel_snapshot_flush(cx);
            });
        });
        vcx.run_until_parked();

        assert_eq!(
            vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
            after + 1,
            "a completed flush must not lock out the next refresh request"
        );

        let after_single = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
        vcx.update(|_, cx| {
            let panel = app.read(cx).ai_panel.clone();
            panel.update(cx, |panel, cx| {
                panel.with_app(cx, |app, cx| {
                    app.defer_ai_panel_snapshot_flush(cx);
                });
            });
        });
        vcx.run_until_parked();

        assert_eq!(
            vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
            after_single + 1,
            "with_app fallback plus an explicit refresh should still coalesce"
        );
    }

    #[test]
    fn ai_header_running_transition_notifies_root() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);

            let idle = app.ai_header_presentation();
            assert!(!idle.running);
            let launch = app
                .ai
                .begin_chat_request("inspect".to_string(), AiMode::Ask, None);
            let running = app.ai_header_presentation();
            assert!(running.running);
            assert_ne!(idle, running, "idle -> running should move the header");

            assert!(app.ai.apply_chat_delta(launch.job_id, "hello", None));
            assert_eq!(
                app.ai_header_presentation(),
                running,
                "ordinary streaming deltas must not move the root header projection"
            );

            app.ai
                .finish_chat_job(
                    launch.job_id,
                    launch.session_id,
                    Ok(AiChatJobOutput {
                        mode: AiMode::Ask,
                        text: "done".to_string(),
                        reasoning: None,
                        command_cards: Vec::new(),
                        auto_execute_first: false,
                        approval_note: None,
                    }),
                )
                .expect("matching job should finish");
            let finished = app.ai_header_presentation();
            assert!(!finished.running);
            assert_ne!(running, finished, "running -> idle should move the header");

            let cancel_launch =
                app.ai
                    .begin_chat_request("cancel me".to_string(), AiMode::Ask, None);
            let cancel_running = app.ai_header_presentation();
            assert!(cancel_running.running);
            app.ai.cancel_chat_and_agent();
            assert!(
                cancel_launch
                    .cancel
                    .load(std::sync::atomic::Ordering::Relaxed)
            );
            let cancelled = app.ai_header_presentation();
            assert!(!cancelled.running);
            assert_ne!(
                cancel_running, cancelled,
                "cancel should move running back to idle"
            );

            let execution_before = app.ai_header_presentation();
            app.ai
                .set_settings_command_mode(AgentCommandExecutionMode::Auto);
            let execution_after = app.ai_header_presentation();
            assert_ne!(
                execution_before, execution_after,
                "execution mode is part of the root header projection"
            );

            let settings = AiSettings {
                models: vec![
                    AiModelConfigItem {
                        backend: Default::default(),
                        id: "openai:model-a".to_string(),
                        name: "Model A".to_string(),
                        provider_kind: Some(AiProviderKind::Openai),
                        credential_id: None,
                        enabled: true,
                        source: AiModelSource::Manual,
                        last_seen_at: None,
                    },
                    AiModelConfigItem {
                        backend: Default::default(),
                        id: "openai:model-b".to_string(),
                        name: "Model B".to_string(),
                        provider_kind: Some(AiProviderKind::Openai),
                        credential_id: None,
                        enabled: true,
                        source: AiModelSource::Manual,
                        last_seen_at: None,
                    },
                ],
                default_model_id: Some("openai:model-a".to_string()),
                ..AiSettings::default()
            };
            app.ai.replace_settings_config(settings, true);
            let model_before = app.ai_header_presentation();
            app.ai.set_settings_default_model("openai:model-b");
            let model_after = app.ai_header_presentation();
            assert_ne!(
                model_before, model_after,
                "selected model is part of the root header projection"
            );
            assert_eq!(model_after.model_label, "Model B");
        });
    }

    #[test]
    fn unrelated_app_notify_does_not_repaint_cached_ai_panel() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let before = vcx.update(|_, cx| ai_paints(&app, cx));
        assert!(
            before > 0,
            "the panel must have painted at least once, or this proves nothing"
        );

        for _ in 0..5 {
            draw(&app, vcx);
        }

        assert_eq!(
            vcx.update(|_, cx| ai_paints(&app, cx)),
            before,
            "unrelated app notifies must not repaint the cached AI panel"
        );
    }

    #[test]
    fn streaming_delta_repaints_ai_panel_without_repainting_sibling_panels() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| {
            (
                ai_paints(&app, cx),
                connection_paints(&app, cx),
                transfer_paints(&app, cx),
                settings_paints(&app, cx),
            )
        });

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let launch = app
                    .ai
                    .begin_chat_request("inspect".to_string(), AiMode::Ask, None);
                assert!(app.ai.apply_chat_delta(launch.job_id, "hello", None));
                app.flush_ai_panel_snapshot(cx);
            });
        });
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let after = vcx.update(|_, cx| {
            (
                ai_paints(&app, cx),
                connection_paints(&app, cx),
                transfer_paints(&app, cx),
                settings_paints(&app, cx),
            )
        });
        assert!(after.0 > before.0, "streaming delta must repaint AI panel");
        assert_eq!(after.1, before.1, "connections panel must stay cached");
        assert_eq!(after.2, before.2, "transfers panel must stay cached");
        assert_eq!(after.3, before.3, "settings panel must stay cached");
    }

    #[test]
    fn prompt_subscription_refreshes_snapshot_before_next_paint() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let prompt_input = vcx.update(|_, cx| {
            app.read(cx)
                .ai_panel
                .read(cx)
                .snapshot()
                .expect("hosted panel has snapshot")
                .prompt_input
                .clone()
        });

        vcx.update(|_, cx| {
            prompt_input.update(cx, |_, cx| {
                cx.emit(NyaInputEvent::Changed("explain status".to_string()));
            });
            assert_eq!(
                app.read(cx)
                    .ai_panel
                    .read(cx)
                    .snapshot()
                    .expect("snapshot remains available")
                    .prompt_draft,
                "",
                "the snapshot must wait for the deferred input flush"
            );
        });
        vcx.run_until_parked();

        vcx.update(|window, cx| {
            let snapshot = app.read(cx).ai_panel.read(cx).snapshot().cloned();
            assert_eq!(
                snapshot.expect("deferred flush ran").prompt_draft,
                "explain status"
            );
            _ = window.draw(cx);
        });
    }

    #[test]
    fn message_menu_position_stays_inside_viewport() {
        assert_eq!(
            super::components::ai_message_menu_position(1240., 780., 128., 64., 1280., 800.),
            (1144., 728., 784.)
        );
        assert_eq!(
            super::components::ai_message_menu_position(240., 180., 128., 64., 200., 120.),
            (64., 48., 104.)
        );
    }
}

use std::sync::Arc;

use rust_i18n::t;

use gpui::{
    Context, Entity, FontWeight, IntoElement, MouseButton, RenderImage, Rgba, ScrollHandle,
    SharedString, WeakEntity, Window, deferred, div, img, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::{
    AgentCommandExecutionMode, AiAgentKind, AiCommandCard, AiMessage, AiMode, AiModelConfigItem,
    AiProviderKind, AiReasoningEffort, AiSession, AiSessionScopeType, truncate_preview,
};
use nyaterm_ui::chat::{NyaMessageScroller, NyaMessageScrollerState};
use nyaterm_ui::{
    NyaDropdownMenu, NyaInputShell, NyaMenuAnchor, NyaMenuItem, NyaScrollable, NyaSearchInput,
    NyaTooltip,
};

use crate::features::NyaTermApp;
use crate::features::capability_runtime::approvals::NativeCapabilityApproval;
use crate::features::formatting::{group_ai_sessions_by_date, short_id};
use crate::features::text_inputs::TextInputSetup;
use crate::features::view_widgets::{
    APP_OVERLAY_PRIORITY, full_window_input_layer, tab_menu_separator,
};
use crate::models::{AiDetectedErrorState, AiMessageMenuState, NavItem, SettingsTab};
use crate::theme::ThemePalette;
use crate::widgets::{small_button, svg_icon_button};

use super::presentation::AiResponsePhase;
use crate::features::runtime_jobs::AiAgentStepView;

mod actions;
mod approvals;
mod command_syntax;
mod components;
mod content;
mod execution;
pub(super) mod harness;
mod index;
mod messages;
mod snapshot;
mod transcript;
use components::{ai_message_menu_button, ai_message_menu_position, ai_send_button, ai_setup_step};
use transcript::{AiTranscriptRow, AiTranscriptUpdate};

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
    pub provider_kind: Option<AiProviderKind>,
    pub provider_icon: Option<Arc<RenderImage>>,
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

#[derive(Clone, PartialEq, Eq)]
pub(in crate::features) struct AiAgentStepPresentation {
    pub step: AiAgentStepView,
    pub thought_open: bool,
    pub output_open: bool,
}

#[derive(Clone)]
pub(in crate::features) struct AiPanelSnapshot {
    index: Arc<index::AiSnapshotIndex>,
    pub native_run: Option<super::state::harness::NativeRunView>,
    pub native_approvals: Arc<[NativeCapabilityApproval]>,
    pub native_answer_inputs: Vec<Entity<nyaterm_ui::NyaInputState>>,
    pub chrome: AiPanelChrome,
    pub ui_font_family: SharedString,
    pub enabled: bool,
    pub agent_mode: bool,
    pub running: bool,
    pub agent_kind: AiAgentKind,
    pub codex_enabled: bool,
    pub claude_code_enabled: bool,
    pub reasoning_effort: AiReasoningEffort,
    pub reasoning_choices: Arc<[AiReasoningEffort]>,
    pub external_agent: bool,
    pub external_model_label: String,
    pub selected_model_id: Option<String>,
    pub selected_model_exists: bool,
    pub model_label: String,
    pub selected_provider_kind: Option<AiProviderKind>,
    pub selected_provider_icon: Option<Arc<RenderImage>>,
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
    pub response_phase: AiResponsePhase,
    pub expanded_message_thoughts: Arc<[String]>,
    pub expanded_command_details: Arc<[String]>,
    pub expanded_command_scripts: Arc<[String]>,
    pub agent_history_expanded: bool,
    pub expanded_execution_groups: Arc<[String]>,
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
    pub history_running_ids: Arc<[String]>,
    pub current_ai_session_id: String,
    pub owner_terminal_id: Option<String>,
    pub owner_connection_id: Option<String>,
    pub history_pending: bool,
    pub history_error: Option<String>,
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
    transcript_scroll: Entity<NyaMessageScrollerState>,
    transcript_rows: Arc<[AiTranscriptRow]>,
    transcript_text_style: Option<(gpui::TextStyle, gpui::Pixels, String)>,
    command_syntax: command_syntax::CommandSyntaxCache,
    content: content::ContentCache,
    mention_scroll: ScrollHandle,
    model_scroll: ScrollHandle,
    history_scroll: ScrollHandle,
    picker_reveal_pending: bool,
    focused_question: Option<(String, String)>,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    snapshot_set_count: usize,
}

impl AiPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>, cx: &mut Context<Self>) -> Self {
        let transcript_scroll = cx.new(|cx| NyaMessageScrollerState::new(0, cx));
        cx.observe(&transcript_scroll, |_, _, cx| cx.notify())
            .detach();
        Self {
            app,
            snapshot: None,
            transcript_scroll,
            transcript_rows: Arc::from([]),
            transcript_text_style: None,
            command_syntax: command_syntax::CommandSyntaxCache::default(),
            content: content::ContentCache::default(),
            mention_scroll: ScrollHandle::new(),
            model_scroll: ScrollHandle::new(),
            history_scroll: ScrollHandle::new(),
            picker_reveal_pending: false,
            focused_question: None,
            #[cfg(test)]
            paint_count: 0,
            #[cfg(test)]
            snapshot_set_count: 0,
        }
    }

    pub(in crate::features) fn set_snapshot(
        &mut self,
        mut snapshot: AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) {
        snapshot.index = Arc::new(index::AiSnapshotIndex::build(&snapshot));
        self.refresh_content(&snapshot, cx);
        self.refresh_command_syntax(&snapshot, cx);
        let rows = AiTranscriptRow::project(&snapshot);
        let update = AiTranscriptUpdate::between(
            self.snapshot.as_ref(),
            &snapshot,
            &self.transcript_rows,
            &rows,
        );
        self.transcript_scroll.update(cx, |state, cx| {
            if update.reset {
                state.reset(rows.len(), cx);
            } else {
                if let Some((range, count)) = update.splice {
                    state.splice(range, count, cx);
                }
                if update.remeasure_all {
                    state.remeasure(cx);
                } else {
                    for range in update.remeasure {
                        state.remeasure_items(range, cx);
                    }
                }
            }
        });
        self.transcript_rows = rows.into();
        if snapshot.mention_open
            && self.snapshot.as_ref().is_none_or(|previous| {
                !previous.mention_open
                    || previous.mention_index != snapshot.mention_index
                    || previous.prompt_draft != snapshot.prompt_draft
            })
        {
            self.mention_scroll.scroll_to_item(snapshot.mention_index);
            self.picker_reveal_pending = true;
        }
        if snapshot.discovery_menu_open
            && self.snapshot.as_ref().is_none_or(|previous| {
                !previous.discovery_menu_open
                    || previous.discovery_index != snapshot.discovery_index
                    || previous.reasoning_choices != snapshot.reasoning_choices
                    || !previous
                        .model_choices
                        .iter()
                        .map(|choice| &choice.model.id)
                        .eq(snapshot.model_choices.iter().map(|choice| &choice.model.id))
            })
        {
            let index = snapshot.discovery_index
                + if snapshot.discovery_index < snapshot.reasoning_choices.len() {
                    1
                } else {
                    2
                };
            self.model_scroll.scroll_to_item(index);
            self.picker_reveal_pending = true;
        }
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
            let before = app.ai_header_presentation();
            let result = f(app, cx);
            app.defer_ai_panel_snapshot_flush(cx);
            app.notify_root_if_ai_header_changed(before, cx);
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
        let prompt_input = NyaInputShell::new("ai.chat.prompt", &snapshot.prompt_input)
            .multi_line()
            .gpui_context_menu([
                t!("menu.cut").into(),
                t!("menu.copy").into(),
                t!("menu.paste").into(),
                t!("menu.selectAll").into(),
            ])
            .height(px(64.))
            .into_any_element();
        let model_search_input = snapshot
            .model_search_input
            .as_ref()
            .map(|field| NyaSearchInput::new("ai-model-search", field).into_any_element());
        let panel_entity = cx.weak_entity();
        let composer_disabled = snapshot.running || snapshot.history_pending || !snapshot.enabled;
        let send_disabled = !snapshot.running
            && (snapshot.history_pending
                || !snapshot.enabled
                || (!snapshot.external_agent && !snapshot.selected_model_exists)
                || snapshot.prompt_draft.trim().is_empty());

        div()
            .tab_group()
            .font_family(snapshot.ui_font_family.clone())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(snapshot.chrome.transparent_surface)
            .relative()
            .on_children_prepainted(move |_, _, cx| {
                let Some(panel) = panel_entity.upgrade() else {
                    return;
                };
                if !panel.read(cx).picker_reveal_pending {
                    return;
                }
                // A mounted handle learns its viewport and overflow mode during
                // prepaint. Queue reveal only after those bounds are measured,
                // including when filtering moves a bottom-anchored popup.
                cx.defer(move |cx| {
                    panel.update(cx, |panel, cx| {
                        if !std::mem::take(&mut panel.picker_reveal_pending) {
                            return;
                        }
                        if let Some(snapshot) = panel.snapshot.as_ref() {
                            if snapshot.mention_open {
                                panel.mention_scroll.scroll_to_item(snapshot.mention_index);
                            }
                            if snapshot.discovery_menu_open {
                                let headings = if snapshot.discovery_index
                                    < snapshot.reasoning_choices.len()
                                {
                                    1
                                } else {
                                    2
                                };
                                panel
                                    .model_scroll
                                    .scroll_to_item(snapshot.discovery_index + headings);
                            }
                        }
                        cx.notify();
                    })
                });
            })
            .when_some(snapshot.detected_error.clone(), |this, detected| {
                this.child(self.ai_detected_error_banner(&snapshot, detected, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .relative()
                    .child(
                        div()
                            .id(SharedString::from("ai-transcript-scroll"))
                            .debug_selector(|| "ai-transcript-viewport".to_string())
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(self.ai_transcript_body(&snapshot, cx)),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "ai-composer".to_string())
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
                    .child(
                        div()
                            .debug_selector(|| "ai-prompt".to_string())
                            .w_full()
                            .relative()
                            .flex()
                            .when(composer_disabled, |this| this.opacity(0.56))
                            .on_key_down(cx.listener(|panel, event: &gpui::KeyDownEvent, _, cx| {
                                if panel.with_app(cx, |app, cx| {
                                    app.handle_ai_prompt_key_down(event, cx)
                                }) {
                                    cx.stop_propagation();
                                }
                            }))
                            .child(div().min_w_0().flex_1().child(prompt_input))
                            .when(snapshot.mention_open, |this| {
                                this.child(self.ai_mention_popover(&snapshot, cx))
                            }),
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
                        t!("ai.analyze"),
                        cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.analyze_ai_detected_error(analyze_state.clone(), cx);
                            });
                        }),
                    ))
                    .child(small_button(
                        palette,
                        "ai-detected-error-close",
                        t!("common.close"),
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
        let popover = div()
            .absolute()
            .bottom(gpui::relative(1.))
            .mb_1()
            .left_0()
            .right_0()
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
        let mut rows = div()
            .id("ai-mention-list")
            .max_h(px(192.))
            .overflow_y_scroll()
            .track_scroll(&self.mention_scroll)
            .flex()
            .flex_col();
        for (index, candidate) in snapshot.mention_candidates.iter().enumerate() {
            let focused = index == snapshot.mention_index;
            let candidate = candidate.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!(
                        "ai-mention-session-{}",
                        candidate.session_id
                    )))
                    .debug_selector(move || format!("ai-mention-row-{index}"))
                    .h(px(30.))
                    .flex_none()
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
        popover
            .child(
                div()
                    .relative()
                    .child(rows)
                    .vertical_scrollbar(&self.mention_scroll),
            )
            .into_any_element()
    }

    fn ai_mode_switch(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let modes = [
            (AiMode::Ask, AiAgentKind::Nyaterm, t!("ai.modeAsk"), true),
            (
                AiMode::Agent,
                AiAgentKind::Nyaterm,
                t!("ai.modeNyatermAgent"),
                true,
            ),
            (
                AiMode::Agent,
                AiAgentKind::Codex,
                t!("ai.modeCodexAgent"),
                snapshot.codex_enabled,
            ),
            (
                AiMode::Agent,
                AiAgentKind::ClaudeCode,
                t!("ai.modeClaudeCodeAgent"),
                snapshot.claude_code_enabled,
            ),
        ];
        let mut items = Vec::new();
        let mut selected_label = t!("ai.modeAsk");
        for (mode, kind, label, enabled) in modes {
            let selected = if mode == AiMode::Ask {
                !snapshot.agent_mode
            } else {
                snapshot.agent_mode && snapshot.agent_kind == kind
            };
            if selected {
                selected_label = label.clone();
            }
            items.push(
                NyaMenuItem::action(label)
                    .checked(selected)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.set_ai_run_mode(mode.clone(), kind.clone(), cx);
                            app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                        });
                    })),
            );
        }
        div()
            .debug_selector(|| "ai-mode-control".to_string())
            .w(px(108.))
            .min_w_0()
            .max_w(gpui::relative(0.45))
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .child(
                NyaDropdownMenu::new("ai-mode-selector")
                    .anchor(NyaMenuAnchor::BottomLeft)
                    .min_width(px(170.))
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
                                    .child(selected_label),
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
    }

    fn ai_model_selector(
        &self,
        snapshot: &AiPanelSnapshot,
        model_search_input: Option<gpui::AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let content = div()
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .gap_1()
            .when(snapshot.selected_model_exists, |this| {
                this.child(ai_model_provider_badge(
                    palette,
                    snapshot.selected_provider_kind.as_ref(),
                    snapshot.selected_provider_icon.as_ref(),
                ))
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(11.))
                    .text_ellipsis()
                    .child(snapshot.model_label.clone()),
            )
            .when(snapshot.selected_model_exists, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(format!(
                            "· {}",
                            super::reasoning_effort_label(&snapshot.reasoning_effort)
                        )),
                )
            })
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/chevron-down.svg"),
            );
        div()
            .min_w_0()
            .flex_1()
            .relative()
            .child(
                div()
                    .debug_selector(|| "ai-model-control".to_string())
                    .h(px(28.))
                    .min_w_0()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        nyaterm_ui::NyaButton::new("ai-model-selector", "")
                            .variant(nyaterm_ui::NyaButtonVariant::Ghost)
                            .small()
                            .height(px(28.))
                            .full_width()
                            .content(content)
                            .disabled(snapshot.enabled_models.is_empty())
                            .tooltip(format!(
                                "{} · {}",
                                snapshot.model_label,
                                super::reasoning_effort_label(&snapshot.reasoning_effort)
                            ))
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.with_app(cx, |app, cx| {
                                    let selected_index = app.ai_selected_model_index();
                                    if app.ai.toggle_discovery_menu(selected_index) {
                                        app.reset_text_input("ai.model-search", "", cx);
                                        let field = app.text_input(
                                            "ai.model-search",
                                            "",
                                            TextInputSetup::placeholder(t!("ai.searchModels")),
                                            cx,
                                        );
                                        window.focus(&field.read(cx).focus_handle(), cx);
                                    } else {
                                        app.focus_text_input_if_present(
                                            "ai.chat.prompt",
                                            window,
                                            cx,
                                        );
                                    }
                                });
                            })),
                    ),
            )
            .when(snapshot.discovery_menu_open, |this| {
                // Resize handles paint in the default deferred layer; the menu
                // must paint above them and own the pointer where they overlap.
                this.child(
                    deferred(self.ai_model_menu(
                        snapshot,
                        snapshot.selected_model_id.clone(),
                        model_search_input,
                        cx,
                    ))
                    .with_priority(APP_OVERLAY_PRIORITY),
                )
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
            .id("ai-model-menu")
            .debug_selector(|| "ai-model-menu".to_string())
            .occlude()
            .absolute()
            // The selector shares its row with the mode switch and send
            // button. Expanding from its left edge made the fixed-width menu
            // cross the side-panel boundary; anchoring the trailing edges
            // keeps the popup inside the panel at its normal width.
            .right_0()
            .bottom(px(34.))
            .w(px(260.))
            .max_h(px(360.))
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
                    .on_key_down(
                        cx.listener(|panel, event: &gpui::KeyDownEvent, window, cx| {
                            if panel.with_app(cx, |app, cx| {
                                let handled = app.handle_ai_model_search_key_down(event, cx);
                                if handled && !app.ai.discovery_menu_is_open() {
                                    app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                                }
                                handled
                            }) {
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .child(model_search_input),
            );
        }
        let mut rows = div()
            .id(SharedString::from("ai-model-choice-list"))
            .min_h_0()
            .max_h(px(300.))
            .overflow_y_scroll()
            .track_scroll(&self.model_scroll)
            .flex()
            .flex_col()
            .child(ai_menu_heading(palette, t!("ai.reasoningIntensity")));
        for (index, effort) in snapshot.reasoning_choices.iter().enumerate() {
            let effort = effort.clone();
            let selected = snapshot.reasoning_effort == effort;
            let label = super::reasoning_effort_label(&effort);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("ai-reasoning-choice-{index}")))
                    .h(px(28.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text))
                    .bg(if snapshot.discovery_index == index {
                        rgb(palette.hover)
                    } else {
                        rgba(0x00000000)
                    })
                    .cursor_pointer()
                    .hover(move |this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.ai.set_discovery_index(index);
                            app.set_ai_reasoning_effort(effort.clone(), cx);
                        });
                    }))
                    .child(ai_choice_check(palette, selected))
                    .child(label),
            );
        }
        rows = rows.child(ai_menu_heading(palette, t!("ai.models")));
        if snapshot.model_choices.is_empty() {
            rows = rows.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.noModelMatches")),
            );
        }
        for (index, choice) in snapshot.model_choices.iter().enumerate() {
            let model = choice.model.clone();
            let provider_label = choice.provider_label.clone();
            let model_id = model.id.clone();
            let is_selected = selected_id.as_deref() == Some(model.id.as_str());
            let choice_index = index + snapshot.reasoning_choices.len();
            let focused = choice_index == snapshot.discovery_index;
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("ai-model-choice-{}", model.id)))
                    .h(px(34.))
                    .flex_none()
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
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        let model_id = model_id.clone();
                        panel.with_app(cx, move |app, cx| {
                            app.ai.set_discovery_index(choice_index);
                            app.ai.close_discovery_menu();
                            app.set_ai_default_model(model_id, cx);
                            app.focus_text_input_if_present("ai.chat.prompt", window, cx);
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
                    .child(ai_model_provider_badge(
                        palette,
                        choice.provider_kind.as_ref(),
                        choice.provider_icon.as_ref(),
                    ))
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
        menu.child(
            div()
                .relative()
                .min_h_0()
                .child(rows)
                .vertical_scrollbar(&self.model_scroll),
        )
    }

    fn ai_transcript_body(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.transcript_rows.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .px_3()
                .py_2()
                .child(self.ai_empty_transcript(snapshot, cx))
                .into_any_element();
        }
        let panel = cx.weak_entity();
        let snapshot = snapshot.clone();
        let rows = Arc::clone(&self.transcript_rows);
        let mut row_style = gpui::StyleRefinement::default();
        row_style.padding.bottom = Some(px(8.).into());
        NyaMessageScroller::new(
            "ai-transcript",
            self.transcript_scroll.clone(),
            move |index, _, cx| {
                panel
                    .update(cx, |panel, cx| {
                        panel.ai_transcript_row(&snapshot, &rows[index], cx)
                    })
                    .unwrap_or_else(|_| div().into_any_element())
            },
        )
        .size_full()
        .with_row_style(row_style)
        .with_jump_button_label(t!("ai.jumpToLatest"))
        .into_any_element()
    }

    fn ai_empty_transcript(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = snapshot.chrome.palette;
        let has_model = snapshot.external_agent
            || snapshot.selected_model_id.is_some()
            || !snapshot.enabled_models.is_empty();
        if !snapshot.enabled {
            return div()
                .flex_1()
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
                .flex_1()
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
            .debug_selector(|| "ai-empty-transcript".to_string())
            .flex_1()
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
                            .id("ai-background-execution-help")
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("ai.backgroundAgentExecution"))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/menu/info.svg")
                                    .text_color(rgb(palette.text_muted)),
                            )
                            .tooltip(|window, cx| {
                                NyaTooltip::new(t!("ai.backgroundAgentExecutionHelp"))
                                    .build(window, cx)
                            }),
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
        let query = snapshot.history_query.trim().to_lowercase();
        let filtered: Vec<_> = snapshot
            .history_sessions
            .iter()
            .filter(|session| {
                query.is_empty()
                    || session.title.to_lowercase().contains(&query)
                    || session.id.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let total_count = snapshot.history_sessions.len();
        let filtered_count = filtered.len();
        let mut grouped = [
            (t!("ai.historyCurrentTerminal"), Vec::new()),
            (t!("ai.historySameConnection"), Vec::new()),
            (t!("ai.historyOtherSessions"), Vec::new()),
        ];
        for session in filtered {
            let group = if session.scope.r#type == AiSessionScopeType::Terminal
                && session.scope.target_id == snapshot.owner_terminal_id
                && snapshot.owner_terminal_id.is_some()
            {
                0
            } else if snapshot
                .owner_connection_id
                .as_ref()
                .is_some_and(|connection_id| {
                    session.connection_id.as_ref() == Some(connection_id)
                        || session.scope.connection_ids.contains(connection_id)
                })
            {
                1
            } else {
                2
            };
            grouped[group].1.push(session);
        }
        let mut search_input = snapshot.history_search_input.as_ref().map(|field| {
            NyaSearchInput::new("ai-history-search", field).on_key_down(cx.listener(
                |panel, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        panel.with_app(cx, |app, cx| {
                            app.close_ai_history(window, cx);
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
                        t!("ai.historyLoading")
                    } else if total_count == 0 {
                        t!("ai.noHistory")
                    } else {
                        t!("ai.noHistoryMatches")
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
                        .child(group),
                );
                for (date, date_sessions) in group_ai_sessions_by_date(&sessions) {
                    rows = rows.child(
                        div()
                            .px_2()
                            .text_size(px(9.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(t!(date.label_key())),
                    );
                    for session in date_sessions {
                        let session_id = session.id.clone();
                        let delete_id = session.id.clone();
                        let active = snapshot.current_ai_session_id == session.id;
                        let occupied = snapshot
                            .history_running_ids
                            .iter()
                            .any(|id| id == &session.id);
                        let open_disabled =
                            snapshot.history_pending || snapshot.running || occupied;
                        let delete_disabled = snapshot.history_pending || occupied;
                        rows = rows.child(
                            div()
                                .id(SharedString::from(format!("ai-session-{}", session.id)))
                                .debug_selector({
                                    let id = session.id.clone();
                                    move || format!("ai-history-session-{id}")
                                })
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
                                        .when(!open_disabled, |this| this.cursor_pointer())
                                        .when(open_disabled, |this| this.opacity(0.5))
                                        .child(truncate_preview(&session.title, 28))
                                        .on_click(cx.listener(move |panel, _, _, cx| {
                                            if open_disabled {
                                                return;
                                            }
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
                                            if occupied {
                                                t!("ai.historyInUse")
                                            } else if session.external_session_id.is_some() {
                                                t!("ai.historyResume")
                                            } else {
                                                "".into()
                                            }
                                        )),
                                )
                                .child(svg_icon_button(
                                    format!("ai-session-delete-{}", session.id),
                                    "icons/fe/delete.svg",
                                    14.,
                                    t!("common.delete").to_string(),
                                    palette,
                                    cx.listener(move |panel, _, window, cx| {
                                        if delete_disabled {
                                            return;
                                        }
                                        let delete_id = delete_id.clone();
                                        panel.with_app(cx, move |app, cx| {
                                            app.open_ai_delete_history_confirm(
                                                delete_id, window, cx,
                                            );
                                        });
                                    }),
                                )),
                        );
                    }
                }
            }
        }

        div()
            .id(SharedString::from("ai-history-popover"))
            .debug_selector(|| "ai-history-popover".to_string())
            .absolute()
            .top(px(4.))
            .left(px(8.))
            .right(px(8.))
            .h(px(if filtered_count == 0 { 156. } else { 352. }))
            .max_h(gpui::relative(0.95))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(snapshot.chrome.surface)
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(search_input, |this, search_input| {
                this.child(
                    div()
                        .flex_none()
                        .p_2()
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .child(search_input),
                )
            })
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
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
                            .child(if snapshot.history_pending {
                                t!("ai.historyLoading")
                            } else {
                                t!("ai.history")
                            }),
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
            .when_some(snapshot.history_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .flex_none()
                        .max_h(px(42.))
                        .overflow_hidden()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(palette.danger))
                        .child(format!("{}: {error}", t!("ai.historyLoadFailed"))),
                )
            })
            .child(
                div()
                    .id(SharedString::from("ai-history-scroll"))
                    .debug_selector(|| "ai-history-viewport".to_string())
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        div()
                            .id("ai-history-list")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.history_scroll)
                            .child(rows),
                    )
                    .vertical_scrollbar(&self.history_scroll),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.history_open)
            && let Some(app) = self.app.upgrade()
            && !app.read(cx).ai.history_is_open()
            && app.read(cx).ai.history_has_restore_focus()
        {
            let search_focused = app
                .read(cx)
                .existing_text_input("ai.history-search")
                .is_some_and(|field| {
                    let input = field.read(cx);
                    input.focus_handle().contains_focused(window, cx)
                        || input
                            .component_focus_handle(cx)
                            .contains_focused(window, cx)
                });
            let focus = app.update(cx, |app, _| {
                let focus = app.ai.take_history_focus();
                app.forget_text_inputs("ai.history-search");
                focus
            });
            if search_focused && let Some(focus) = focus {
                window.focus(&focus, cx);
            }
        }
        if let Some(snapshot) = &self.snapshot
            && let Some(view) = snapshot.native_run.as_ref().filter(|view| {
                view.status == nyaterm_core::ai::harness::AgentRunStatus::WaitingForUser
            })
            && let (Some(call_id), Some(input)) =
                (&view.call_id, snapshot.native_answer_inputs.first())
        {
            let key = (view.run_id.clone(), call_id.clone());
            if self.focused_question.as_ref() != Some(&key) {
                if let Some(index) = self.transcript_rows.iter().position(|row| {
                    matches!(row, AiTranscriptRow::NativeRun { run_id, .. } if run_id == &view.run_id)
                }) {
                    self.transcript_scroll.update(cx, |scroll, cx| {
                        scroll.scroll_to_item(index, cx);
                    });
                }
                window.focus(&input.read(cx).focus_handle(), cx);
                self.focused_question = Some(key);
            }
        }
        // The virtual list detects width changes itself. Font metrics and
        // translated labels also invalidate heights of off-screen rows.
        let text_style = (
            window.text_style(),
            window.rem_size(),
            rust_i18n::locale().to_string(),
        );
        if self.transcript_text_style.as_ref() != Some(&text_style) {
            self.transcript_text_style = Some(text_style);
            self.transcript_scroll
                .update(cx, |state, cx| state.remeasure(cx));
        }
        #[cfg(test)]
        {
            self.paint_count += 1;
        }
        self.render_panel(cx)
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
        .min_w_0()
        .flex_1()
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .text_size(px(11.))
        .text_color(rgb(palette.text_muted))
        .child(
            div()
                .min_w_0()
                .text_ellipsis()
                .child(snapshot.external_model_label.clone()),
        )
}

fn ai_menu_heading(palette: ThemePalette, label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .h(px(24.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .text_size(px(10.))
        .text_color(rgb(palette.text_muted))
        .child(label.into())
}

fn ai_choice_check(palette: ThemePalette, selected: bool) -> impl IntoElement {
    div().size(px(14.)).flex_none().when(selected, |this| {
        this.child(
            svg()
                .size(px(13.))
                .path("icons/check.svg")
                .text_color(rgb(palette.link)),
        )
    })
}

fn ai_model_provider_badge(
    palette: ThemePalette,
    kind: Option<&AiProviderKind>,
    image: Option<&Arc<RenderImage>>,
) -> gpui::AnyElement {
    if let Some(image) = image {
        return img(image.clone())
            .size(px(16.))
            .flex_none()
            .rounded_full()
            .into_any_element();
    }
    let path = match kind {
        Some(AiProviderKind::Openai) => "icons/settings/openai.svg",
        Some(AiProviderKind::Anthropic) => "icons/settings/anthropic.svg",
        Some(AiProviderKind::Gemini) => "icons/settings/gemini.svg",
        Some(AiProviderKind::Deepseek) => "icons/settings/deepseek.svg",
        Some(AiProviderKind::Xai) => "icons/settings/xai.svg",
        Some(AiProviderKind::Zai) => "icons/settings/zai.svg",
        Some(AiProviderKind::Ollama) => "icons/settings/ollama.svg",
        Some(AiProviderKind::Mimo) => "icons/settings/mimo.svg",
        _ => "icons/settings/cloud.svg",
    };
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when(kind == Some(&AiProviderKind::Cohere), |this| {
            this.text_size(px(10.))
                .text_color(rgb(palette.accent))
                .child("C")
        })
        .when(kind != Some(&AiProviderKind::Cohere), |this| {
            this.child(
                svg()
                    .size(px(14.))
                    .path(path)
                    .text_color(rgb(palette.accent)),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests;

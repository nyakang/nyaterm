use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb, rgba,
};

use super::terminal_action_prompt_text;
use crate::features::NyaTermApp;
use crate::features::formatting::recent_terminal_output;
use crate::features::view_widgets::tab_action_button;
use crate::models::{BottomPanelMode, RightFocus, TerminalSearchMode};
use crate::widgets::small_button;

impl NyaTermApp {
    pub(in crate::features) fn terminal_actions_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let visible_text = self.active_terminal_visible_text();
        let buffer_tail = recent_terminal_output(self.active_terminal_buffer_tail(), usize::MAX);
        let visible_lines = visible_text.lines().count();
        let buffer_chars = buffer_tail.chars().count();
        let summary = t!(
            "terminalActions.summary",
            lines = visible_lines,
            chars = buffer_chars
        );
        let visible_for_translate = visible_text.clone();
        let visible_for_ai = terminal_action_prompt_text(&visible_text, 2_800);
        let buffer_for_ai = terminal_action_prompt_text(&buffer_tail, 4_000);
        let has_visible_text = !visible_text.trim().is_empty();
        let has_buffer_text = !buffer_tail.trim().is_empty();
        let _has_selection = self.selected_terminal_text().is_some();

        div()
            .id(SharedString::from("terminal-actions-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x030508d8))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(96.))
            .track_focus(self.terminal.actions_focus())
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(this.terminal.actions_focus(), cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                cx.stop_propagation();
                match event.keystroke.key.as_str() {
                    "escape" => this.close_terminal_actions(window, cx),
                    "v" | "V"
                        if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                    {
                        this.terminal.close_actions();
                        this.paste_from_clipboard(window, cx);
                    }
                    "f" | "F"
                        if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                    {
                        this.open_terminal_search(window, cx);
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .id(SharedString::from("terminal-actions-dialog"))
                    .w(px((viewport_w - 32.).clamp(280., 660.)))
                    .max_h(px((viewport_h - 24.).max(260.)))
                    .max_w_full()
                    .mx_4()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(0x0b0f16))
                    .shadow_lg()
                    .p_4()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight(800.))
                                            .text_color(rgb(0xe5edf7))
                                            .child(t!("terminalActions.title")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(palette.text_muted))
                                            .child(summary),
                                    ),
                            )
                            .child(small_button(palette,
                                "terminal-actions-close",
                                t!("common.close"),
                                cx.listener(|this, _, window, cx| {
                                    this.close_terminal_actions(window, cx);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .mt_4()
                            .grid()
                            .grid_cols(4)
                            .gap_2()
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-copy-visible",
                                t!("terminalCtx.copy"),
                                t!("terminalActions.selectionScreen"),
                                cx.listener(|this, _, _, cx| {
                                    this.copy_terminal_selection_or_visible(cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-select-all",
                                t!("terminalCtx.selectAll"),
                                t!("terminalActions.visibleGrid"),
                                cx.listener(|this, _, _, cx| {
                                    this.select_all_terminal(cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-find",
                                t!("terminalCtx.find"),
                                t!("terminalActions.searchBuffer"),
                                cx.listener(|this, _, window, cx| {
                                    this.open_terminal_search(window, cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-sync-groups",
                                t!("syncGroup.title"),
                                t!("terminalActions.broadcastInput"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.open_sync_groups(window, cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-paste",
                                t!("terminalCtx.paste"),
                                t!("terminalActions.clipboardText"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.paste_from_clipboard(window, cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-clear-screen",
                                t!("terminalCtx.clearScreen"),
                                t!("terminalActions.shellClear"),
                                cx.listener(|this, _, _, cx| {
                                    this.send_terminal_clear_screen(cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-clear-all",
                                t!("terminalCtx.clearAll"),
                                t!("terminalActions.dropBuffer"),
                                cx.listener(|this, _, _, cx| {
                                    this.terminal.close_actions();
                                    this.clear_terminal(cx);
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-temporary-ssh-link",
                                t!("terminalActions.tempSsh"),
                                t!("terminalActions.pasteLink"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.open_temporary_ssh_link_dialog(window, cx);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .mt_3()
                            .grid()
                            .grid_cols(4)
                            .gap_2()
                            .child(
                                div().when(!has_visible_text, |this| this.opacity(0.45)).child(
                                    tab_action_button(
                                        palette,
                                        "terminal-actions-translate-visible",
                                        t!("translation.title"),
                                        t!("terminalActions.visibleScreen"),
                                        cx.listener(move |this, _, window, cx| {
                                            this.terminal.close_actions();
                                            if visible_for_translate.trim().is_empty() {
                                                this.shell.set_status("terminal visible screen is empty".to_string());
                                            } else {
                                                let provider =
                                                    this.translation.provider().to_string();
                                                let provider_label = match provider.as_str() {
                                                    "google" => t!("translation.google"),
                                                    "microsoft" => {
                                                        t!("translation.microsoft")
                                                    }
                                                    "deepl" => t!("translation.deepl"),
                                                    "baidu" => t!("translation.baidu"),
                                                    "ali" => t!("translation.ali"),
                                                    "youdao" => t!("translation.youdao"),
                                                    _ => Cow::Borrowed(provider.as_str()),
                                                }
                                                .to_string();
                                                this.open_translation_dialog(
                                                    visible_for_translate.clone(),
                                                    provider,
                                                    provider_label,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            cx.notify();
                                        }),
                                    ),
                                ),
                            )
                            .child(
                                div().when(!has_visible_text, |this| this.opacity(0.45)).child(
                                    tab_action_button(
                                        palette,
                                        "terminal-actions-ai-visible",
                                        t!("ai.title"),
                                        t!("terminalActions.visibleScreen"),
                                        cx.listener(move |this, _, window, cx| {
                                            this.terminal.close_actions();
                                            if visible_for_ai.trim().is_empty() {
                                                this.ai.set_panel_status(
                                                    "terminal visible screen is empty",
                                                );
                                            } else {
                                                this.set_ai_prompt_draft(format!(
                                                    "Explain this terminal output:\n\n{}",
                                                    visible_for_ai
                                                ), cx);
                                                this.ai.set_panel_status(
                                                    "terminal output loaded into AI prompt",
                                                );
                                                window.focus(this.ai.chat_focus(), cx);
                                            }
                                            cx.notify();
                                        }),
                                    ),
                                ),
                            )
                            .child(
                                div().when(!has_buffer_text, |this| this.opacity(0.45)).child(
                                    tab_action_button(
                                        palette,
                                        "terminal-actions-ai-buffer",
                                        t!("terminalActions.aiBuffer"),
                                        t!("terminalActions.bufferContext"),
                                        cx.listener(move |this, _, window, cx| {
                                            this.terminal.close_actions();
                                            if buffer_for_ai.trim().is_empty() {
                                                this.ai
                                                    .set_panel_status("terminal buffer is empty");
                                            } else {
                                                this.set_ai_prompt_draft(format!(
                                                    "Review this terminal buffer and summarize issues or next actions:\n\n{}",
                                                    buffer_for_ai
                                                ), cx);
                                                this.ai.set_panel_status(
                                                    "terminal buffer loaded into AI prompt",
                                                );
                                                window.focus(this.ai.chat_focus(), cx);
                                            }
                                            cx.notify();
                                        }),
                                    ),
                                ),
                            )
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-command-send",
                                t!("terminalActions.sendPanel"),
                                t!("terminalActions.bottomSender"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.set_bottom_panel_mode(BottomPanelMode::CommandSend);
                                    this.focus_send_command_composer(window, cx);
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .mt_3()
                            .grid()
                            .grid_cols(4)
                            .gap_2()
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-history-search",
                                t!("suggestions.title"),
                                t!("terminalActions.commandHistory"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.terminal.set_search_mode(TerminalSearchMode::History);
                                    this.open_terminal_search(window, cx);
                                    this.shell.set_status("command history search focused".to_string());
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-quick-commands",
                                t!("quickCommands.title"),
                                t!("terminalActions.quickCommands"),
                                cx.listener(|this, _, _, cx| {
                                    this.terminal.close_actions();
                                    this.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
                                    cx.notify();
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-recording",
                                t!("terminalActions.recording"),
                                t!("terminalActions.sessionLog"),
                                cx.listener(|this, _, _, cx| {
                                    this.terminal.close_actions();
                                    this.shell.set_right_focus(RightFocus::Recording);
                                    cx.notify();
                                }),
                            ))
                            .child(tab_action_button(
                                palette,
                                "terminal-actions-session-info",
                                t!("tabCtx.sessionInfo"),
                                t!("terminalActions.sessionDetails"),
                                cx.listener(|this, _, window, cx| {
                                    this.terminal.close_actions();
                                    this.open_active_session_info(window, cx);
                                }),
                            )),
                    )
            )
    }
}

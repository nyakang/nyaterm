use rust_i18n::t;

use gpui::{
    Context, Focusable, FontWeight, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px,
    rgb, rgba,
};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaDocumentEditor};

use crate::features::NyaTermApp;
use crate::models::normalize_paste_newlines;

impl NyaTermApp {
    pub(in crate::features) fn multi_line_paste_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let editor = self
            .terminal
            .paste_review_editor()
            .expect("paste review overlay requires an active editor");
        let paste_focus = editor.read(cx).focus_handle(cx);
        let draft_text = editor.read(cx).value(cx);
        let normalized = normalize_paste_newlines(&draft_text);
        let stats = t!(
            "terminal.multiLinePasteStats",
            lines = normalized.split('\n').count(),
            chars = draft_text.chars().count()
        );
        let can_send = !draft_text.is_empty();
        let preview_height = (viewport_h - 160.).clamp(128., 288.);
        let focus_editor = editor.clone();

        div()
            .id(SharedString::from("multi-line-paste-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&paste_focus)
            .on_click(move |_, window, cx| {
                focus_editor.update(cx, |editor, cx| editor.focus(window, cx));
            })
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_multi_line_paste_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id(SharedString::from("multi-line-paste-dialog"))
                    .w(px((viewport_w - 32.).clamp(280., 576.)))
                    .max_h(px((viewport_h - 24.).max(240.)))
                    .max_w_full()
                    .mx_4()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .p_6()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(800.))
                            .text_color(rgb(palette.text))
                            .child(t!("terminal.multiLinePasteTitle")),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(stats),
                    )
                    .child(
                        div()
                            .id(SharedString::from("multi-line-paste-text"))
                            .mt_3()
                            .h(px(preview_height))
                            .min_h_0()
                            .min_w_0()
                            .overflow_hidden()
                            .rounded_sm()
                            .border_1()
                            .border_color(if can_send {
                                rgb(palette.border)
                            } else {
                                rgb(0x7f1d1d)
                            })
                            .bg(rgb(palette.input))
                            .child(NyaDocumentEditor::new(&editor)),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                NyaButton::new("multi-line-paste-cancel", t!("common.cancel"))
                                    .variant(NyaButtonVariant::Secondary)
                                    .small()
                                    .compact()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_multi_line_paste(window, cx);
                                    })),
                            )
                            .child(
                                NyaButton::new(
                                    "multi-line-paste-direct",
                                    t!("terminal.multiLinePasteDirect"),
                                )
                                .variant(NyaButtonVariant::Primary)
                                .small()
                                .compact()
                                .disabled(!can_send)
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.direct_multi_line_paste(window, cx);
                                    },
                                )),
                            )
                            .child(
                                NyaButton::new(
                                    "multi-line-paste-line",
                                    t!("terminal.multiLinePasteSendLineByLine"),
                                )
                                .variant(NyaButtonVariant::Secondary)
                                .small()
                                .compact()
                                .disabled(!can_send)
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.send_multi_line_paste_by_line(window, cx);
                                    },
                                )),
                            ),
                    ),
            )
    }
}

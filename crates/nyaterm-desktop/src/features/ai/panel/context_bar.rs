//! Extra `@` targets for the next message.
//!
//! The owning terminal is the session already on screen, so the panel does not
//! repeat its name and connection state in a status row. Chips appear only
//! after the user adds another target, and each chip is the way to remove it.

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;

use super::{AiPanel, AiPanelSnapshot};

impl AiPanel {
    pub(super) fn ai_extra_targets(
        &self,
        snapshot: &AiPanelSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = div()
            .debug_selector(|| "ai-extra-targets".to_string())
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap_1();
        for target in snapshot.target_sessions.iter() {
            row = row.child(self.ai_target_chip(snapshot, &target.session_id, &target.label, cx));
        }
        row.into_any_element()
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

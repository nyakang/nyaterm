use gpui::{Context, IntoElement, div, prelude::*, px};

use crate::features::NyaTermApp;

mod composer;
mod controls;
mod footer;
mod hex_composer;
mod state;
mod target_picker;
#[cfg(test)]
mod tests;
mod text_composer;

impl NyaTermApp {
    pub(in crate::features) fn bottom_command_send_bar(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.send_command_bar_view_state(cx);
        let palette = state.palette;
        let controls = self.send_command_bar_controls(&state, cx);
        let editor = self.send_command_bar_composer(&state, cx);

        let footer = self.send_command_bar_footer(&state, cx);
        let app = cx.entity().downgrade();
        // The fixed toolbar leaves all remaining height to the composer.
        div()
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let app = app.clone();
                        window.defer(cx, move |_, cx| {
                            let _ = app.update(cx, |app, cx| {
                                if app
                                    .send_command
                                    .set_viewport_width(f32::from(bounds.size.width))
                                {
                                    cx.notify();
                                }
                            });
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .h(px(self.shell.command_send_height()))
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .bg(self.shell_surface_color(palette.surface))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px_2()
                    .py(px(4.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(controls)
                    .child(editor)
                    .child(footer),
            )
    }
}

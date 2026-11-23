use gpui::{AnyElement, Div, div, prelude::*};
pub fn controls(
    compact: bool,
    mode_controls: AnyElement,
    model_control: AnyElement,
    trailing: AnyElement,
) -> Div {
    if compact {
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
    .flex_none()
}

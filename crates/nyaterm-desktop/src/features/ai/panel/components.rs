use super::AiPanel;
use gpui::{Context, IntoElement};
pub(super) use nyaterm_ai_gpui::components::{
    agent_kind_display_name, ai_command_target_label, ai_setup_step, execution_mode_label,
};
use nyaterm_ui::ThemePalette;
pub(super) fn ai_send_button(
    _palette: ThemePalette,
    running: bool,
    disabled: bool,
    cx: &mut Context<AiPanel>,
) -> impl IntoElement {
    nyaterm_ai_gpui::components::ai_send_button(
        _palette,
        running,
        disabled,
        cx.listener(move |panel, _, _, cx| {
            panel.with_app(cx, move |app, cx| {
                if app.ai.chat_or_agent_is_running() {
                    app.cancel_ai_chat(cx);
                } else if !disabled {
                    app.start_ai_ask(cx);
                }
            });
        }),
    )
}

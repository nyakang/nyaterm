use crate::features::NyaTermApp;
use crate::features::transfers::privilege_runtime::PASSWORD_INPUT;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, div, px};
use nyaterm_ui::NyaInputShell;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn file_sudo_busy_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .child(t!("fileSudo.busy").to_string())
            .into_any_element()
    }

    pub(in crate::features) fn file_sudo_prompt_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(t!("fileSudo.password").to_string())
            .children(self.existing_text_input(PASSWORD_INPUT).map(|field| {
                div()
                    .w_full()
                    .h(px(32.))
                    .child(NyaInputShell::new("file-sudo-password", &field).height(px(32.)))
            }))
            .children(
                self.file_sudo_prompt_error()
                    .map(|error| div().child(error.to_owned())),
            )
            .into_any_element()
    }
}

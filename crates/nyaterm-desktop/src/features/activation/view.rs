use gpui::{Context, IntoElement as _, ParentElement as _, Styled as _, Window, div};
use nyaterm_ui::{NyaButton, NyaDialogWindowExt as _, NyaScrollable as _};

use super::state::ExternalDialog;
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(super) fn open_external_match_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_content_dialog(
            rust_i18n::t!("externalOpen.matchTitle").to_string(),
            480.,
            |app, _, cx| {
                let Some(ExternalDialog::Choose { connections, .. }) =
                    app.activation.dialog.clone()
                else {
                    return div().into_any_element();
                };
                let mut content = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .max_h(gpui::px(400.))
                    .child(rust_i18n::t!("externalOpen.matchDescription").to_string());
                for (index, connection) in connections.into_iter().enumerate() {
                    content = content.child(
                        NyaButton::new(
                            format!("external-match-{index}"),
                            format!("{} ({})", connection.name, connection.endpoint()),
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.choose_external_connection(Some(index), window, cx)
                            },
                        )),
                    );
                }
                content
                    .child(
                        NyaButton::new(
                            "external-temporary",
                            rust_i18n::t!("externalOpen.useTemporary"),
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.choose_external_connection(None, window, cx)
                        })),
                    )
                    .overflow_y_scrollbar()
                    .into_any_element()
            },
            |app, _| {
                app.activation.dialog = None;
            },
            window,
            cx,
        );
    }

    fn choose_external_connection(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ExternalDialog::Choose {
            request,
            connections,
        }) = self.activation.dialog.take()
        else {
            return;
        };
        window.close_nya_dialog(cx);
        if let Some(connection) = index.and_then(|index| connections.get(index)).cloned() {
            let mode = match &request {
                nyaterm_core::activation::ExternalConnectionRequest::Ssh {
                    runtime_mode, ..
                } => *runtime_mode,
                _ => None,
            };
            self.prepare_external_saved(connection, mode, cx);
        } else {
            self.start_external_temporary(request, cx);
        }
        cx.notify();
    }
}

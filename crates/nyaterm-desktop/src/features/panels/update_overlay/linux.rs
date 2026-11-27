use crate::features::NyaTermApp;
use crate::features::update::UpdatePhase;
use crate::features::view_widgets::dialog_action_button;
use crate::widgets::small_button;
use gpui::{
    AnyElement, AppContext as _, Context, IntoElement, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px,
};
use nyaterm_ui::{NyaInput, NyaInputKind, NyaInputState, NyaScrollable};
use nyaterm_updater::linux::Authorization;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn package_update_content(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let phase = self.update.read(cx).phase().clone();
        let detail = self.update.read(cx).install_detail.clone();
        let palette = self.theme_palette();
        let message = match phase {
            UpdatePhase::LinuxInstalling => t!("updater.packageAuthorizing"),
            UpdatePhase::LinuxPassword => t!("updater.sudoPassword"),
            UpdatePhase::LinuxAuthorizationFailed => t!("updater.authorizationFailed"),
            UpdatePhase::LinuxInstallFailed => t!("updater.packageInstallFailed"),
            UpdatePhase::LinuxInstalled => t!("updater.packageInstalled"),
            _ => return None,
        };
        let password = matches!(phase, UpdatePhase::LinuxPassword);
        let focus_password = password && self.update.read(cx).password_input.is_none();
        let input = if password {
            Some(self.update.update(cx, |store, cx| {
                store
                    .password_input
                    .get_or_insert_with(|| {
                        cx.new(|cx| NyaInputState::new(cx, "").kind(NyaInputKind::Password))
                    })
                    .clone()
            }))
        } else {
            None
        };
        let app = cx.entity().downgrade();
        let mut content = div().flex().flex_col().gap_3().child(message);
        if let Some(input) = input {
            if focus_password {
                input.update(cx, |input, cx| input.focus_with_cursor_at_end(window, cx));
            }
            content = content.child(div().w_full().h(px(34.)).child(
                NyaInput::new(&input).on_enter(move |_, _, cx| {
                    let _ = app.update(cx, |app, cx| app.submit_update_password(cx));
                }),
            ));
        }
        let busy = matches!(phase, UpdatePhase::LinuxInstalling);
        let installed = matches!(phase, UpdatePhase::LinuxInstalled);
        Some(
            content
                .when_some(detail, |content, detail| {
                    content.child(
                        div()
                            .max_h(px(180.))
                            .overflow_y_scrollbar()
                            .text_xs()
                            .child(detail),
                    )
                })
                .when(!busy, |content| {
                    content.child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(small_button(
                                palette,
                                "package-dismiss",
                                if installed {
                                    t!("updater.later")
                                } else {
                                    t!("common.close")
                                },
                                cx.listener(|app, _, window, cx| {
                                    app.cancel_update_password(cx);
                                    app.close_update_dialog(window, cx);
                                }),
                            ))
                            .when(password, |row| {
                                row.child(dialog_action_button(
                                    palette,
                                    "package-password",
                                    t!("updater.authorizeInstall"),
                                    false,
                                    cx.listener(|app, _, _, cx| app.submit_update_password(cx)),
                                ))
                            })
                            .when(
                                matches!(phase, UpdatePhase::LinuxAuthorizationFailed),
                                |row| {
                                    row.child(dialog_action_button(
                                        palette,
                                        "package-sudo",
                                        t!("updater.useSudo"),
                                        false,
                                        cx.listener(|app, _, _, cx| {
                                            app.update.update(cx, |store, cx| {
                                                store.request_password();
                                                cx.notify();
                                            });
                                        }),
                                    ))
                                },
                            )
                            .when(matches!(phase, UpdatePhase::LinuxInstallFailed), |row| {
                                row.child(dialog_action_button(
                                    palette,
                                    "package-retry",
                                    t!("updater.retryInstall"),
                                    false,
                                    cx.listener(|app, _, _, cx| {
                                        app.start_package_install(Authorization::Polkit, cx)
                                    }),
                                ))
                            })
                            .when(!installed && !password, |row| {
                                row.child(small_button(
                                    palette,
                                    "package-manual",
                                    t!("updater.viewRelease"),
                                    cx.listener(|app, _, _, cx| {
                                        app.open_external_url_for_ui(
                                            "https://github.com/nyakang/nyaterm/releases",
                                            cx,
                                        )
                                    }),
                                ))
                            })
                            .when(installed, |row| {
                                row.child(dialog_action_button(
                                    palette,
                                    "package-restart",
                                    t!("updater.restartNow"),
                                    false,
                                    cx.listener(|app, _, window, cx| {
                                        app.request_native_update_install(window, cx)
                                    }),
                                ))
                            }),
                    )
                })
                .into_any_element(),
        )
    }
}

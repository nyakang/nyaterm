use rust_i18n::t;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaScrollable, NyaTabItem, NyaTabs};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::SecurityKeyEditorState;
use crate::widgets::small_button;

use super::super::view_helpers::security_editor_field;

impl NyaTermApp {
    pub(in crate::features) fn security_private_key_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (name, value, error) = self
            .security
            .private_key_view()
            .map(|(name, value, error)| {
                (
                    name.to_string(),
                    value.to_string(),
                    error.map(str::to_string),
                )
            })
            .unwrap_or_default();
        let loading = value.is_empty() && error.is_none();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("settings.privateKey")),
                    )
                    .child(
                        nyaterm_ui::NyaIconButton::new(
                            "security-private-key-copy",
                            "icons/copy.svg",
                        )
                        .tooltip(t!("common.copyToClipboard"))
                        .disabled(loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_security_private_key(cx);
                        })),
                    ),
            )
            .child(
                div()
                    .id("security-private-key-content")
                    .min_h(px(288.))
                    .max_h(px(480.))
                    .overflow_y_scrollbar()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .font_family(crate::features::shell::gpui_code_font_family())
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .when_else(
                        loading,
                        |this| this.child(t!("common.loading").to_string()),
                        |this| {
                            if let Some(error) = error {
                                this.child(error)
                            } else {
                                this.child(nyaterm_ui::NyaSelectableText::new(
                                    "security-private-key-selectable-content",
                                    value,
                                ))
                            }
                        },
                    ),
            )
    }

    pub(in crate::features) fn security_key_editor_view(
        &mut self,
        editor: SecurityKeyEditorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .track_focus(self.security.key_editor_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_security_key_editor_key_down(event, window, cx);
            }))
            .child(security_editor_field(
                self,
                "key-name",
                t!("securityAuth.nameLabel"),
                editor.name.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("securityAuth.privateKey")),
                            )
                            .child(
                                div().w(px(160.)).child(
                                    NyaTabs::new("security-key-material-mode")
                                        .items([
                                            NyaTabItem::new(t!("settings.keyInputContentMode"))
                                                .disabled(self.security.editor_busy()),
                                            NyaTabItem::new(t!("settings.keyInputFileMode"))
                                                .disabled(self.security.editor_busy()),
                                        ])
                                        .selected_index(usize::from(!editor.key_content_mode))
                                        .on_select(cx.listener(|this, index, _, cx| {
                                            this.toggle_security_key_content_mode(
                                                false,
                                                *index == 0,
                                                cx,
                                            )
                                        })),
                                ),
                            ),
                    )
                    .when(editor.key_content_mode, |this| {
                        this.child(security_editor_field(
                            self,
                            "key-data",
                            t!("settings.keyInputContentMode"),
                            editor.key_data.expose_secret().to_owned(),
                            TextInputSetup {
                                placeholder: t!("settings.keyContentPlaceholder").into(),
                                masked: false,
                                multi_line: true,
                                submit_on_enter: false,
                                code: false,
                            },
                            cx,
                        ))
                    })
                    .when(!editor.key_content_mode, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().min_w_0().flex_1().child(security_editor_field(
                                    self,
                                    "key-path",
                                    "",
                                    editor.key_file_path.clone(),
                                    TextInputSetup::placeholder(t!(
                                        "settings.keyFilePathPlaceholder"
                                    )),
                                    cx,
                                )))
                                .child(small_button(
                                    palette,
                                    "security-key-browse",
                                    t!("securityAuth.browse"),
                                    cx.listener(|this, _, window, cx| {
                                        this.pick_security_key_file(false, window, cx);
                                    }),
                                )),
                        )
                    }),
            )
            .when(!editor.cert_expanded, |this| {
                this.child(small_button(
                    palette,
                    "security-key-add-cert",
                    t!("settings.addCertificate"),
                    cx.listener(|this, _, _, cx| this.toggle_security_key_certificate(cx)),
                ))
            })
            .when(editor.cert_expanded, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_muted))
                                        .child(t!("securityAuth.certificate")),
                                )
                                .child(
                                    div().w(px(160.)).child(
                                        NyaTabs::new("security-cert-material-mode")
                                            .items([
                                                NyaTabItem::new(t!("settings.keyInputContentMode"))
                                                    .disabled(self.security.editor_busy()),
                                                NyaTabItem::new(t!("settings.keyInputFileMode"))
                                                    .disabled(self.security.editor_busy()),
                                            ])
                                            .selected_index(usize::from(!editor.cert_content_mode))
                                            .on_select(cx.listener(|this, index, _, cx| {
                                                this.toggle_security_key_content_mode(
                                                    true,
                                                    *index == 0,
                                                    cx,
                                                )
                                            })),
                                    ),
                                ),
                        )
                        .when(editor.cert_content_mode, |this| {
                            this.child(security_editor_field(
                                self,
                                "key-cert-data",
                                t!("settings.keyInputContentMode"),
                                editor.cert_data.expose_secret().to_owned(),
                                TextInputSetup {
                                    placeholder: t!("settings.certContentPlaceholder").into(),
                                    masked: false,
                                    multi_line: true,
                                    submit_on_enter: false,
                                    code: false,
                                },
                                cx,
                            ))
                        })
                        .when(!editor.cert_content_mode, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().min_w_0().flex_1().child(security_editor_field(
                                        self,
                                        "key-cert-path",
                                        "",
                                        editor.cert_file_path.clone(),
                                        TextInputSetup::placeholder(t!(
                                            "settings.certFilePathPlaceholder"
                                        )),
                                        cx,
                                    )))
                                    .child(small_button(
                                        palette,
                                        "security-cert-browse",
                                        t!("securityAuth.browse"),
                                        cx.listener(|this, _, window, cx| {
                                            this.pick_security_key_file(true, window, cx);
                                        }),
                                    )),
                            )
                        }),
                )
            })
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_1()
                    .child(div().min_w_0().flex_1().child(security_editor_field(
                        self,
                        "key-passphrase",
                        t!("securityAuth.passphrase"),
                        editor.passphrase.expose_secret().to_owned(),
                        TextInputSetup {
                            placeholder: "".into(),
                            masked: !editor.show_passphrase,
                            multi_line: false,
                            submit_on_enter: false,
                            code: false,
                        },
                        cx,
                    )))
                    .child(
                        nyaterm_ui::NyaIconButton::new(
                            "security-key-passphrase-visible",
                            if editor.show_passphrase {
                                "icons/eye-off.svg"
                            } else {
                                "icons/eye.svg"
                            },
                        )
                        .tooltip(t!(if editor.show_passphrase {
                            "settings.hidePassphrase"
                        } else {
                            "settings.showPassphrase"
                        }))
                        .disabled(self.security.editor_busy())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_security_key_passphrase_visibility(cx)
                        })),
                    ),
            )
            .when_some(editor.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.danger))
                        .child(error),
                )
            })
    }
}

use crate::models::ConnectionEditorSection;
use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb,
};
use nyaterm_remote_desktop::parse_rdp_drive_letters;
use nyaterm_ui::{NyaCheckbox, NyaSwitch};

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{
    ConnectionEditorField, ConnectionEditorSelect, ConnectionEditorState, local_drive_letters,
};

use super::super::super::list::{
    ConnectionEditorFields, ConnectionEditorRenderContext, connection_editor_select, editor_field,
    editor_stepper_field, required,
};
use super::ConnectionEditorSectionContext;

fn rdp_switch_row(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    checked: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .px_3()
        .py_2()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_xs().font_weight(FontWeight(500.)).child(label))
                .when_some(description, |this, description| {
                    this.child(
                        div()
                            .mt_1()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(description),
                    )
                }),
        )
        .child(
            NyaSwitch::new(id)
                .checked(checked)
                .on_click(move |_, window, cx| {
                    on_click(&gpui::ClickEvent::default(), window, cx);
                }),
        )
}

/// A button that opens the native directory picker. The selected path is shown
/// in place of a manually editable input, so the stored value is always a
/// directory the user actually chose.
fn rdp_drive_directory_picker(
    palette: crate::theme::ThemePalette,
    editor: &ConnectionEditorState,
    fields: &ConnectionEditorFields,
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let label: SharedString = label.into();
    let configured = editor.rdp_redirection.custom_drive_path.trim();
    let display = if configured.is_empty() {
        t!("dialog.rdpCustomDriveSelect").to_string()
    } else {
        configured.to_string()
    };

    div()
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(label),
            )
        })
        .child(
            nyaterm_ui::NyaButton::new(id, display)
                .form_control()
                .full_width()
                .label_left()
                .icon("icons/conn/folder.svg")
                .tooltip(t!("dialog.rdpCustomDrivePickPrompt"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.prompt_connection_editor_custom_drive_path(cx);
                })),
        )
        .when_some(
            fields
                .errors
                .get(&ConnectionEditorField::RdpCustomDrivePath),
            |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.danger))
                        .child(error.clone()),
                )
            },
        )
}

/// Shown only while the Drives switch is on and the built-in/IronRDP backend
/// is selected. Windows offers per-letter picks plus one custom directory;
/// Linux/macOS have no letters, so only the single directory is configured.
fn rdp_drive_picker(
    palette: crate::theme::ThemePalette,
    editor: &ConnectionEditorState,
    fields: &ConnectionEditorFields,
    cx: &mut Context<NyaTermApp>,
) -> Option<gpui::Div> {
    if !editor.rdp_redirection.drives || editor.uses_mstsc_rdp() {
        // mstscax only exposes an all-or-nothing RedirectDrives switch; no
        // single-letter or single-directory property exists, so a picker here
        // would be misleading.
        return None;
    }

    let mut picker = div().mt_1().pl_1().flex().flex_col().gap_2();

    if cfg!(windows) {
        let available = local_drive_letters();
        if !available.is_empty() {
            let selected = parse_rdp_drive_letters(&editor.rdp_redirection.drive_letters);
            picker = picker
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("dialog.rdpDrivePickerHint")),
                )
                .child({
                    let mut letters = div().flex().flex_wrap().gap_2();
                    for letter in available {
                        let id = format!("connection-rdp-drive-{letter}");
                        let checked = selected.contains(&letter);
                        let app = cx.weak_entity();
                        letters = letters.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(NyaCheckbox::new(id).checked(checked).on_click(
                                    move |_, _, cx| {
                                        let _ = app.update(cx, |app, cx| {
                                            app.toggle_connection_editor_flag(
                                                ConnectionEditorToggle::RdpRedirectDriveLetter(
                                                    letter,
                                                ),
                                                cx,
                                            );
                                        });
                                    },
                                ))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text))
                                        .child(format!("{letter}:")),
                                ),
                        );
                    }
                    letters
                });
        }

        let custom_checked = !editor.rdp_redirection.custom_drive_path.trim().is_empty();
        let app = cx.weak_entity();
        picker = picker.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            NyaCheckbox::new("connection-rdp-custom-drive")
                                .checked(custom_checked)
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        if custom_checked {
                                            app.toggle_connection_editor_flag(
                                                ConnectionEditorToggle::RdpCustomDriveToggled,
                                                cx,
                                            );
                                        } else {
                                            app.prompt_connection_editor_custom_drive_path(cx);
                                        }
                                    });
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette.text))
                                .child(t!("dialog.rdpCustomDrive")),
                        ),
                )
                .child(rdp_drive_directory_picker(
                    palette,
                    editor,
                    fields,
                    "connection-editor-rdp-custom-drive-picker",
                    "",
                    cx,
                )),
        );
    } else {
        picker = picker
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.rdpCustomDriveHint")),
            )
            .child(rdp_drive_directory_picker(
                palette,
                editor,
                fields,
                "connection-editor-rdp-custom-drive-picker",
                t!("dialog.rdpCustomDrive"),
                cx,
            ));
    }

    Some(picker)
}

pub(super) fn connection_editor_rdp_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic => div()
            .flex()
            .flex_col()
            .gap_3()
            .child(rdp_basic(section, cx))
            .when(!section.editor.uses_mstsc_rdp(), |this| {
                this.child(rdp_authentication(section, cx))
            }),
        ConnectionEditorSection::Security if !section.editor.uses_mstsc_rdp() => {
            rdp_security(section, cx)
        }
        ConnectionEditorSection::Network => rdp_network(section, cx),
        ConnectionEditorSection::Display if !section.editor.uses_mstsc_rdp() => {
            rdp_display(section, cx)
        }
        ConnectionEditorSection::Session => rdp_session(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-rdp-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .when(section.editor.uses_mstsc_rdp(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(section.palette.text_muted))
                    .child(t!("dialog.rdpMstscDesc")),
            )
        })
        .child(content)
}

fn rdp_basic(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        narrow,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .flex()
            .when(narrow, |this| this.flex_col())
            .gap_3()
            .child(div().min_w_0().flex_1().child(editor_field(
                palette,
                required(t!("dialog.host")),
                ConnectionEditorField::Host,
                fields,
                cx,
            )))
            .child(
                div()
                    .when(!narrow, |this| this.w(px(150.)).flex_none())
                    .when(narrow, |this| this.w_full())
                    .child(editor_stepper_field(
                        palette,
                        required(t!("dialog.port")),
                        ConnectionEditorField::Port,
                        fields,
                        cx,
                    )),
            ),
    )
}

fn rdp_authentication(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        narrow,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .when(!narrow, |this| this.flex_wrap())
                .when(narrow, |this| this.flex_col())
                .gap_3()
                .child(div().min_w(px(180.)).flex_1().child(editor_field(
                    palette,
                    required(t!("dialog.username")),
                    ConnectionEditorField::Username,
                    fields,
                    cx,
                )))
                .child(div().min_w(px(160.)).flex_1().child(editor_field(
                    palette,
                    t!("dialog.rdpDomain"),
                    ConnectionEditorField::Domain,
                    fields,
                    cx,
                ))),
        )
        .child(super::authentication::password_authentication(section, cx))
}

fn rdp_security(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(rdp_switch_row(
            palette,
            "connection-rdp-use-nla",
            t!("dialog.rdpUseNla"),
            Some(t!("dialog.rdpUseNlaDesc").into()),
            editor.rdp_security.use_nla,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpUseNla, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-administrative-session",
            t!("dialog.rdpAdministrativeSession"),
            Some(t!("dialog.rdpAdministrativeSessionDesc").into()),
            editor.rdp_security.administrative_session,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(
                    ConnectionEditorToggle::RdpAdministrativeSession,
                    cx,
                )
            }),
        ))
        .when(editor.uses_windows_rdp(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.rdpWindowsAuthentication")),
            )
        })
        .when(!editor.uses_windows_rdp(), |this| {
            this.child(connection_editor_select(
                ConnectionEditorRenderContext {
                    palette,
                    fields,
                    cx,
                },
                "connection-editor-rdp-certificate-policy",
                t!("dialog.rdpCertificatePolicy"),
                ConnectionEditorSelect::RdpCertificatePolicy,
            ))
        })
}

fn rdp_network(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette, fields, ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-proxy",
            t!("dialog.proxySelect"),
            ConnectionEditorSelect::Proxy,
        ))
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-jump",
            t!("dialog.proxyJump"),
            ConnectionEditorSelect::ProxyJump,
        ))
}

fn rdp_display(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-display-mode",
            t!("dialog.rdpDisplayMode"),
            ConnectionEditorSelect::RdpDisplayMode,
        ))
        .when(editor.rdp_display.mode == "fixed", |this| {
            this.child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().min_w_0().flex_1().child(editor_stepper_field(
                        palette,
                        t!("dialog.rdpWidth"),
                        ConnectionEditorField::RdpDisplayWidth,
                        fields,
                        cx,
                    )))
                    .child(div().min_w_0().flex_1().child(editor_stepper_field(
                        palette,
                        t!("dialog.rdpHeight"),
                        ConnectionEditorField::RdpDisplayHeight,
                        fields,
                        cx,
                    ))),
            )
        })
        .when(!editor.uses_mstsc_rdp(), |this| {
            this.child(rdp_switch_row(
                palette,
                "connection-rdp-graphics-pipeline",
                t!("dialog.rdpGraphicsPipeline"),
                Some(t!("dialog.rdpGraphicsPipelineDesc").into()),
                editor.rdp_display.graphics_pipeline,
                cx.listener(|this, _, _, cx| {
                    this.toggle_connection_editor_flag(
                        ConnectionEditorToggle::RdpGraphicsPipeline,
                        cx,
                    )
                }),
            ))
        })
        .child(rdp_switch_row(
            palette,
            "connection-rdp-display-wallpaper",
            t!("dialog.rdpWallpaper"),
            None,
            editor.rdp_display.display_wallpaper,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpDisplayWallpaper, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-display-themes",
            t!("dialog.rdpThemes"),
            None,
            editor.rdp_display.display_themes,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpDisplayThemes, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-font-smoothing",
            t!("dialog.rdpFontSmoothing"),
            None,
            editor.rdp_display.enable_font_smoothing,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpFontSmoothing, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-desktop-composition",
            t!("dialog.rdpDesktopComposition"),
            Some(t!("dialog.rdpDesktopCompositionDesc").into()),
            editor.rdp_display.enable_desktop_composition,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(
                    ConnectionEditorToggle::RdpDesktopComposition,
                    cx,
                )
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-full-window-drag",
            t!("dialog.rdpDisableFullWindowDrag"),
            Some(t!("dialog.rdpDisableFullWindowDragDesc").into()),
            editor.rdp_display.disable_full_window_drag,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpFullWindowDrag, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-menu-animations",
            t!("dialog.rdpDisableMenuAnimations"),
            None,
            editor.rdp_display.disable_menu_animations,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpMenuAnimations, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-cursor-shadow",
            t!("dialog.rdpDisableCursorShadow"),
            None,
            editor.rdp_display.disable_cursor_shadow,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpCursorShadow, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-cursor-blinking",
            t!("dialog.rdpDisableCursorBlinking"),
            None,
            editor.rdp_display.disable_cursor_blinking,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpCursorBlinking, cx)
            }),
        ))
}

fn rdp_session(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-backend",
            t!("dialog.rdpBackend"),
            ConnectionEditorSelect::RdpBackend,
        ))
        .when(!editor.uses_mstsc_rdp(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.rdpBackendDesc")),
            )
        })
        .flex()
        .flex_col()
        .gap_3()
        .when(!editor.uses_mstsc_rdp(), |this| {
            this.child(rdp_switch_row(
                palette,
                "connection-rdp-auto-reconnect",
                t!("dialog.rdpAutoReconnect"),
                Some(t!("dialog.rdpAutoReconnectDesc").into()),
                editor.rdp_reconnect.enabled,
                cx.listener(|this, _, _, cx| {
                    this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpReconnect, cx)
                }),
            ))
            .child(editor_stepper_field(
                palette,
                t!("dialog.rdpReconnectAttempts"),
                ConnectionEditorField::RdpReconnectAttempts,
                fields,
                cx,
            ))
        })
        .child(
            div()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(t!("dialog.rdpLocalResources")),
                )
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-clipboard",
                    t!("dialog.rdpRedirectClipboard"),
                    None,
                    editor.rdp_redirection.clipboard,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectClipboard,
                            cx,
                        )
                    }),
                ))
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-drives",
                    t!("dialog.rdpRedirectDrives"),
                    editor
                        .uses_mstsc_rdp()
                        .then(|| t!("dialog.rdpDrivesAllActiveX").into()),
                    editor.rdp_redirection.drives,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectDrives,
                            cx,
                        )
                    }),
                ))
                .when_some(
                    rdp_drive_picker(palette, editor, fields, cx),
                    |this, picker| this.child(picker),
                )
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-printers",
                    t!("dialog.rdpRedirectPrinters"),
                    None,
                    editor.rdp_redirection.printers,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectPrinters,
                            cx,
                        )
                    }),
                ))
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-ports",
                    t!("dialog.rdpRedirectPorts"),
                    None,
                    editor.rdp_redirection.ports,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectPorts,
                            cx,
                        )
                    }),
                ))
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-smartcards",
                    t!("dialog.rdpRedirectSmartcards"),
                    None,
                    editor.rdp_redirection.smartcards,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectSmartcards,
                            cx,
                        )
                    }),
                ))
                .child(rdp_switch_row(
                    palette,
                    "connection-rdp-redirect-microphone",
                    t!("dialog.rdpRedirectMicrophone"),
                    None,
                    editor.rdp_redirection.microphone,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_connection_editor_flag(
                            ConnectionEditorToggle::RdpRedirectMicrophone,
                            cx,
                        )
                    }),
                ))
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-rdp-audio",
                    t!("dialog.rdpAudio"),
                    ConnectionEditorSelect::RdpAudio,
                )),
        )
}

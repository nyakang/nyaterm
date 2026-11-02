use crate::models::ConnectionEditorSection;
use nyaterm_ui::NyaSwitch;
use rust_i18n::t;

use gpui::{
    Context, FontWeight, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb,
};

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};

use super::super::super::list::{
    ConnectionEditorRenderContext, connection_editor_select, editor_field,
};

use super::{ConnectionEditorSectionContext, recording::connection_editor_recording_section};

pub(super) fn connection_editor_local_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic => local_startup(section, cx),
        ConnectionEditorSection::Terminal => local_terminal(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-local-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .child(content)
}

fn local_startup(
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
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("dialog.shellPath")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            div()
                                .when(!narrow, |this| this.w(px(144.)))
                                .when(narrow, |this| this.w_full())
                                .flex_none()
                                .child(connection_editor_select(
                                    ConnectionEditorRenderContext {
                                        palette,
                                        fields,
                                        cx,
                                    },
                                    "connection-editor-shell-preset",
                                    "",
                                    ConnectionEditorSelect::Shell,
                                )),
                        )
                        .child(div().min_w_0().flex_1().child(editor_field(
                            palette,
                            "",
                            ConnectionEditorField::ShellPath,
                            fields,
                            cx,
                        )))
                        .child(
                            nyaterm_ui::NyaButton::new("connection-editor-shell-browse", "")
                                .form_control()
                                .icon("icons/conn/folder.svg")
                                .tooltip(t!("settings.browse"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.prompt_connection_editor_shell_path(cx);
                                })),
                        ),
                ),
        )
        .child(editor_field(
            palette,
            t!("dialog.shellArgs"),
            ConnectionEditorField::ShellArgs,
            fields,
            cx,
        ))
        .child(
            div()
                .flex()
                .items_end()
                .gap_2()
                .child(div().min_w_0().flex_1().child(editor_field(
                    palette,
                    t!("dialog.workingDir"),
                    ConnectionEditorField::WorkingDir,
                    fields,
                    cx,
                )))
                .child(
                    nyaterm_ui::NyaButton::new(
                        "connection-editor-cwd-browse",
                        t!("settings.browse"),
                    )
                    .form_control()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prompt_connection_editor_working_dir(cx);
                    })),
                ),
        )
}

fn local_terminal(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .id("connection-editor-local-terminal")
            .debug_selector(|| "connection-editor-local-advanced".to_string())
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(connection_editor_select(
                ConnectionEditorRenderContext {
                    palette,
                    fields,
                    cx,
                },
                "connection-editor-local-encoding",
                t!("connection.encoding"),
                ConnectionEditorSelect::Encoding,
            ))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight(500.))
                                    .child(t!("dialog.dynamicTabTitle")),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("dialog.dynamicTabTitleDesc")),
                            )
                            .when(
                                editor.dynamic_tab_title && !editor.shell_args.trim().is_empty(),
                                |this| {
                                    this.child(
                                        div()
                                            .mt_1()
                                            .text_size(px(10.))
                                            .text_color(rgb(palette.warning))
                                            .child(t!("dialog.dynamicTabTitleCustomArgsHint")),
                                    )
                                },
                            ),
                    )
                    .child(
                        NyaSwitch::new("connection-editor-local-dynamic-tab-title")
                            .checked(editor.dynamic_tab_title)
                            .tooltip(t!("dialog.dynamicTabTitle"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_connection_editor_flag(
                                    ConnectionEditorToggle::DynamicTabTitle,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(connection_editor_recording_section(section, cx)),
    )
}

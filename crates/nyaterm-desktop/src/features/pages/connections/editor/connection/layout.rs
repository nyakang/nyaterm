use super::super::super::list::EDITOR_CONTROL_HEIGHT_PX;
use super::connection_editor_footer_button;
use crate::features::NyaTermApp;
use crate::models::{ConnectionEditorSection, ConnectionEditorState, ConnectionKindTab};
use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, KeyDownEvent, SharedString, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb, rgba,
};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaScrollable, NyaSelect, NyaSelectState, NyaTabItem, NyaTabs,
};
use rust_i18n::t;

pub(super) struct ConnectionEditorFrame<'a> {
    pub palette: crate::theme::ThemePalette,
    pub editor: &'a ConnectionEditorState,
    pub title: std::borrow::Cow<'static, str>,
    pub content: AnyElement,
    pub navigation_focus: gpui::FocusHandle,
    pub editor_focus: gpui::FocusHandle,
    pub scroll: gpui::ScrollHandle,
    pub native_window: bool,
    pub height: f32,
    pub wide: bool,
    pub navigation_width: f32,
    pub protocol_select: Entity<NyaSelectState>,
    pub validation_error: Option<String>,
    pub save_enabled: bool,
}

pub(super) fn connection_editor_frame(
    frame: ConnectionEditorFrame<'_>,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let ConnectionEditorFrame {
        palette,
        editor,
        title,
        content,
        navigation_focus,
        editor_focus,
        scroll,
        native_window,
        height,
        wide,
        navigation_width,
        protocol_select,
        validation_error,
        save_enabled,
    } = frame;
    let sections = ConnectionEditorSection::for_kind(editor.kind, editor.defaults_mode);
    let local_label = t!("dialog.localTerminal");
    let serial_label = t!("dialog.serial");
    let cancel_label = t!("common.cancel");
    let save_label = t!("common.save");
    let navigation = div()
        .id("connection-editor-navigation")
        .track_focus(&navigation_focus)
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .w(px(navigation_width))
        .flex_none()
        .overflow_y_scrollbar()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            let Some(editor) = this.connection_state.active_editor_draft() else {
                return;
            };
            let sections = ConnectionEditorSection::for_kind(editor.kind, editor.defaults_mode);
            let index = sections
                .iter()
                .position(|section| *section == editor.section)
                .unwrap_or(0);
            let next = match event.keystroke.key.as_str() {
                "up" => Some(index.saturating_sub(1)),
                "down" => Some((index + 1).min(sections.len() - 1)),
                "home" => Some(0),
                "end" => Some(sections.len() - 1),
                "tab" if !event.keystroke.modifiers.shift => {
                    window.focus(&this.connection_state.editor_section_focus(), cx);
                    window.focus_next(cx);
                    cx.stop_propagation();
                    None
                }
                _ => None,
            };
            if let Some(index) = next {
                this.set_connection_editor_section(sections[index], cx);
                cx.stop_propagation();
            }
        }))
        .children(sections.iter().copied().map(|section| {
            NyaButton::new(
                format!("connection-editor-nav-{}", section.id()),
                section.label(),
            )
            .variant(NyaButtonVariant::Ghost)
            .full_width()
            .height(px(40.))
            .label_left()
            .tooltip(section.label())
            .selected(editor.section == section)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_connection_editor_section(section, cx);
                window.focus(&this.connection_state.editor_navigation_focus(), cx);
            }))
        }));
    let navigation = div()
        .debug_selector(|| "connection-editor-navigation".to_string())
        .w(px(navigation_width))
        .h_full()
        .min_h_0()
        .flex_none()
        .child(navigation.size_full());

    let card = div()
        .id(SharedString::from("connection-editor-panel"))
        .debug_selector(|| "connection-editor-panel".to_string())
        .w_full()
        .when(native_window, |this| this.size_full())
        .when(!native_window, |this| this.h(px(height.max(0.))))
        .flex()
        .flex_col()
        // No blanket focus grab here: it existed to keep the old label-div
        // inputs "focused", and would now steal focus back from whichever
        // field the pointer just landed on, since click follows mouse-down.
        .track_focus(&editor_focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key != "escape"
                && this.connection_editor_select_is_focused(window, cx)
            {
                return;
            }
            if this.handle_connection_editor_key_down(event, window, cx) {
                cx.stop_propagation();
            }
        }))
        .when(!native_window, |this| {
            this.child(
                div()
                    .px_4()
                    .pt_4()
                    .text_size(px(15.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text))
                    .child(title),
            )
        })
        .child(
            div()
                .flex_none()
                .px(px(20.))
                .pt(px(20.))
                .when(!native_window, |this| this.pt_3())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pb_3()
                        .when(wide, |this| {
                            this.child(
                                NyaTabs::new("connection-kind-tabs")
                                    .items([
                                        NyaTabItem::new("SSH"),
                                        NyaTabItem::new(local_label),
                                        NyaTabItem::new("Telnet"),
                                        NyaTabItem::new(serial_label),
                                        NyaTabItem::new("RDP"),
                                        NyaTabItem::new("VNC"),
                                        NyaTabItem::new("ADB"),
                                    ])
                                    .selected_index(match editor.kind {
                                        ConnectionKindTab::Ssh => 0,
                                        ConnectionKindTab::Local => 1,
                                        ConnectionKindTab::Telnet => 2,
                                        ConnectionKindTab::Serial => 3,
                                        ConnectionKindTab::Rdp => 4,
                                        ConnectionKindTab::Vnc => 5,
                                        ConnectionKindTab::Adb => 6,
                                    })
                                    .on_select(cx.listener(|this, index, _, cx| {
                                        let kind = match *index {
                                            0 => ConnectionKindTab::Ssh,
                                            1 => ConnectionKindTab::Local,
                                            2 => ConnectionKindTab::Telnet,
                                            3 => ConnectionKindTab::Serial,
                                            4 => ConnectionKindTab::Rdp,
                                            5 => ConnectionKindTab::Vnc,
                                            _ => ConnectionKindTab::Adb,
                                        };
                                        this.set_connection_editor_kind(kind, cx);
                                    })),
                            )
                        })
                        .when(!wide, |this| {
                            this.child(
                                div()
                                    .w_full()
                                    .h(px(EDITOR_CONTROL_HEIGHT_PX))
                                    .child(NyaSelect::new(&protocol_select)),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(navigation)
                .child(content)
                .vertical_scrollbar(&scroll),
        )
        .when_some(editor.error.clone(), |this, error| {
            this.child(
                div()
                    .id("connection-editor-error")
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.danger))
                    .bg(rgba((palette.danger << 8) | 0x18))
                    .p_2()
                    .text_size(px(12.))
                    .text_color(rgb(palette.danger))
                    .child(error),
            )
        })
        .child(
            div()
                .h(px(60.))
                .flex_none()
                .border_t_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.section_header))
                .px_5()
                .py_3()
                .flex()
                .items_center()
                .justify_end()
                .gap_3()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(connection_editor_footer_button(
                            palette,
                            "connection-editor-close",
                            cancel_label,
                            false,
                            true,
                            None,
                            cx.listener(|this, _, _, cx| {
                                this.close_connection_editor(cx);
                            }),
                        ))
                        .child(connection_editor_footer_button(
                            palette,
                            "connection-editor-save",
                            save_label,
                            true,
                            save_enabled,
                            validation_error.clone(),
                            cx.listener(|this, _, window, cx| {
                                this.save_connection_editor(window, cx);
                            }),
                        )),
                ),
        );
    card.into_any_element()
}

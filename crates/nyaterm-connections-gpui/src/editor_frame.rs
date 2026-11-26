use nyaterm_ui::NYA_FORM_CONTROL_HEIGHT_PX as EDITOR_CONTROL_HEIGHT_PX;

use crate::models::{ConnectionEditorSection, ConnectionKindTab};
use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, KeyDownEvent, SharedString, Window, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb, rgba,
};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaScrollable, NyaSelect, NyaSelectState, NyaTabItem, NyaTabs,
};
use rust_i18n::t;

/// Everything the shared chrome renders from, with no application state.
///
/// The connection editor and the asset batch editor both produce this; the
/// frame therefore never reads a concrete editor-state type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorChrome {
    pub kind: ConnectionKindTab,
    pub section: ConnectionEditorSection,
    pub defaults_mode: bool,
    pub save_enabled: bool,
    pub test_enabled: bool,
    pub test_running: bool,
}

/// Event routing for the shared editor frame.
///
/// The frame owns layout only, so every interaction is forwarded to the host
/// rather than reaching into `NyaTermApp` state from this module.
pub trait EditorFrameHost: 'static + Sized {
    fn editor_frame_select_section(
        &mut self,
        section: ConnectionEditorSection,
        focus_navigation: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    fn editor_frame_select_kind(
        &mut self,
        kind: ConnectionKindTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// Handles Up/Down/Home/End/Tab while the navigation list has focus.
    /// Returns whether the event was consumed.
    fn editor_frame_navigation_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool;

    fn editor_frame_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool;

    fn editor_frame_save(&mut self, window: &mut Window, cx: &mut Context<Self>);

    /// Probe the current draft without saving it.
    ///
    /// The default keeps the shared frame usable by editors that have no
    /// connection test (for example the asset batch editor).
    fn editor_frame_test_connection(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn editor_frame_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>);
}

pub struct ConnectionEditorFrame {
    pub palette: nyaterm_ui::ThemePalette,
    pub chrome: EditorChrome,
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
    pub protocol_kinds: Vec<ConnectionKindTab>,
    pub validation_error: Option<String>,
    pub test_disabled_reason: Option<String>,
}

pub fn connection_editor_frame<H: EditorFrameHost>(
    frame: ConnectionEditorFrame,
    cx: &mut Context<H>,
) -> AnyElement {
    let ConnectionEditorFrame {
        palette,
        chrome,
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
        protocol_kinds,
        validation_error,
        test_disabled_reason,
    } = frame;
    let sections = ConnectionEditorSection::for_kind(chrome.kind, chrome.defaults_mode);
    let cancel_label = t!("common.cancel");
    let save_label = t!("common.save");
    let test_label = if chrome.test_running {
        t!("dialog.testingConnection")
    } else {
        t!("dialog.testConnection")
    };
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
            if this.editor_frame_navigation_key(event, window, cx) {
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
            .selected(chrome.section == section)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editor_frame_select_section(section, true, window, cx);
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
            if this.editor_frame_key_down(event, window, cx) {
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
                        .debug_selector(|| "connection-editor-protocols".to_string())
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pb_3()
                        .when(wide, |this| {
                            this.child(
                                NyaTabs::new("connection-kind-tabs")
                                    .items(
                                        protocol_kinds
                                            .iter()
                                            .map(|kind| NyaTabItem::new(kind.display_label())),
                                    )
                                    .selected_index(
                                        protocol_kinds
                                            .iter()
                                            .position(|kind| *kind == chrome.kind)
                                            .unwrap_or(0),
                                    )
                                    .on_select(cx.listener(move |this, index, window, cx| {
                                        if let Some(kind) = protocol_kinds.get(*index) {
                                            this.editor_frame_select_kind(*kind, window, cx);
                                        }
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
        .when_some(validation_error.clone(), |this, error| {
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
                            "connection-editor-test",
                            test_label,
                            FooterButtonOptions {
                                primary: false,
                                enabled: chrome.test_enabled,
                                disabled_tooltip: test_disabled_reason,
                                loading: chrome.test_running,
                            },
                            cx.listener(|this, _, window, cx| {
                                this.editor_frame_test_connection(window, cx);
                            }),
                        ))
                        .child(connection_editor_footer_button(
                            palette,
                            "connection-editor-close",
                            cancel_label,
                            FooterButtonOptions {
                                enabled: true,
                                ..FooterButtonOptions::default()
                            },
                            cx.listener(|this, _, window, cx| {
                                this.editor_frame_cancel(window, cx);
                            }),
                        ))
                        .child(connection_editor_footer_button(
                            palette,
                            "connection-editor-save",
                            save_label,
                            FooterButtonOptions {
                                primary: true,
                                enabled: chrome.save_enabled,
                                disabled_tooltip: validation_error.clone(),
                                ..FooterButtonOptions::default()
                            },
                            cx.listener(|this, _, window, cx| {
                                this.editor_frame_save(window, cx);
                            }),
                        )),
                ),
        );
    card.into_any_element()
}

#[derive(Clone, Default)]
struct FooterButtonOptions {
    primary: bool,
    enabled: bool,
    disabled_tooltip: Option<String>,
    loading: bool,
}

fn connection_editor_footer_button(
    _palette: nyaterm_ui::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    options: FooterButtonOptions,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let FooterButtonOptions {
        primary,
        enabled,
        disabled_tooltip,
        loading,
    } = options;
    let mut button = nyaterm_ui::NyaButton::new(id, label)
        .height(px(36.))
        .variant(if primary {
            nyaterm_ui::NyaButtonVariant::Primary
        } else {
            nyaterm_ui::NyaButtonVariant::Secondary
        })
        .disabled(!enabled || loading)
        .loading(loading)
        .on_click(on_click);
    if !enabled
        && !loading
        && let Some(tooltip) = disabled_tooltip
    {
        button = button.tooltip(tooltip);
    }
    div().debug_selector(move || id.to_string()).child(button)
}

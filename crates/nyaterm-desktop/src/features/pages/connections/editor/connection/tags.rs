use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaIconButton, NyaInput, NyaTag};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::pages::connections::list::{ConnectionEditorFields, EDITOR_CONTROL_HEIGHT_PX};
use crate::features::text_inputs::{ordinary_input_focus_ring, ordinary_input_shell_border_color};
use crate::models::{ConnectionEditorField, ConnectionEditorState};
use crate::theme::ThemePalette;

pub(super) fn connection_tags_field(
    palette: ThemePalette,
    editor: &ConnectionEditorState,
    fields: &ConnectionEditorFields,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let entity = fields.get(&ConnectionEditorField::NewTag);
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    let mut tags = div()
        .id("connection-editor-tags")
        .debug_selector(|| "connection-editor-tags".to_string())
        .w_full()
        .min_w_0()
        .min_h(px(EDITOR_CONTROL_HEIGHT_PX))
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(ordinary_input_shell_border_color(palette, focused))
        .when(focused, |this| {
            this.shadow(ordinary_input_focus_ring(palette))
        })
        .bg(rgb(palette.input_surface()))
        .cursor_text()
        .when_some(handle.clone(), |this, handle| {
            this.on_click(move |_, window, cx| window.focus(&handle, cx))
        });
    for (index, tag) in editor.tags.iter().enumerate() {
        let remove = tag.clone();
        let input_focus = handle.clone();
        tags = tags.child(
            NyaTag::secondary()
                .max_w(px(192.))
                .min_w_0()
                .h(px(24.))
                .py_0()
                .px_1p5()
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .truncate()
                        .child(tag.clone()),
                )
                .child(
                    NyaIconButton::new(format!("connection-remove-tag-{index}"), "icons/close.svg")
                        .size(px(16.))
                        .icon_size(px(10.))
                        .tooltip(t!("dialog.removeTag", tag = tag))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.remove_connection_editor_tag(&remove, cx);
                            if let Some(handle) = &input_focus {
                                window.focus(handle, cx);
                            }
                        })),
                ),
        );
    }
    tags = tags.children(entity.map(|field| {
        div()
            .h(px(24.))
            .min_w(px(80.))
            .flex_1()
            .text_color(rgb(palette.text))
            .child(NyaInput::new(field))
    }));
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.tags")),
        )
        .child(tags)
}

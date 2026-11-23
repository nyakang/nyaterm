use crate::models::ConnectionEditorFocusTarget;
use std::collections::HashMap;

use gpui::{
    App, Context, Entity, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb,
};

use crate::features::{
    NyaTermApp, text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
    text_inputs::ordinary_input_focus_ring, text_inputs::ordinary_input_shell_border_color,
};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInput, NyaInputShell, NyaInputState, NyaNumberInput,
    NyaNumberInputState, NyaSelect, NyaSelectState,
};

pub(in crate::features) use crate::features::connections::state::list_model::{
    ConnectionListRow, ConnectionSectionHeader,
};

pub(in crate::features) use nyaterm_connections_gpui::forms::connection_tree_indent_px;

#[derive(Clone)]
pub(in crate::features) struct ConnectionEditorChoice {
    pub value: Option<String>,
    pub label: String,
    pub search_text: Option<String>,
    pub subtitle: Option<String>,
    pub selected: bool,
    pub disabled: bool,
}

impl ConnectionEditorChoice {
    pub(super) fn new(value: Option<String>, label: impl Into<String>, selected: bool) -> Self {
        Self {
            value,
            label: label.into(),
            search_text: None,
            subtitle: None,
            selected,
            disabled: false,
        }
    }

    pub(super) fn search_text(mut self, search_text: impl Into<String>) -> Self {
        self.search_text = Some(search_text.into());
        self
    }

    pub(super) fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

pub(in crate::features) struct ConnectionEditorRenderContext<'a, 'cx> {
    pub palette: crate::theme::ThemePalette,
    pub fields: &'a ConnectionEditorFields,
    pub cx: &'a mut Context<'cx, NyaTermApp>,
}

pub(in crate::features) fn connection_editor_select(
    render: ConnectionEditorRenderContext<'_, '_>,
    id: &'static str,
    label: impl Into<FieldLabel>,
    select: ConnectionEditorSelect,
) -> impl IntoElement {
    let _ = &render.cx;
    let anchor = render
        .fields
        .anchors
        .get(&ConnectionEditorFocusTarget::Select(select))
        .cloned();
    nyaterm_connections_gpui::forms::select_field(
        render.palette,
        id,
        label,
        anchor,
        NyaSelect::new(&render.fields.select(select)),
    )
}

pub(in crate::features) use nyaterm_connections_gpui::forms::editor_switch_row;

/// What the editor's inputs and selects need from the app, resolved once per
/// render.
///
/// Sections receive this rather than the app, because they are free functions
/// that already hold a `Context` — reading the app entity again from inside its
/// own update panics.
pub(in crate::features) struct ConnectionEditorFields {
    pub(super) account: Option<nyaterm_core::SavedPassword>,
    pub(super) account_password_locked: bool,
    pub(super) errors: HashMap<ConnectionEditorField, String>,
    pub(super) anchors: HashMap<ConnectionEditorFocusTarget, gpui::ScrollAnchor>,
    fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
    number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
    selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
    forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
    forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
}

impl ConnectionEditorFields {
    pub(super) fn new(
        fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
        number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
        selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
        forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
        forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
    ) -> Self {
        Self {
            account: None,
            account_password_locked: false,
            errors: HashMap::new(),
            anchors: HashMap::new(),
            fields,
            number_fields,
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        }
    }

    pub(super) fn get(&self, field: &ConnectionEditorField) -> Option<&Entity<NyaInputState>> {
        self.fields.get(field)
    }

    pub(super) fn get_number(
        &self,
        field: &ConnectionEditorField,
    ) -> Option<&Entity<NyaNumberInputState>> {
        self.number_fields.get(field)
    }

    pub(super) fn select(&self, select: ConnectionEditorSelect) -> Entity<NyaSelectState> {
        self.selects
            .get(&select)
            .cloned()
            .expect("connection editor select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_select(&self, index: usize) -> Entity<NyaSelectState> {
        self.forwarding_endpoint_selects
            .get(&index)
            .cloned()
            .expect("forwarding endpoint select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_field(
        &self,
        index: usize,
        field: ConnectionEditorField,
    ) -> Entity<NyaInputState> {
        self.forwarding_endpoint_fields
            .get(&(index, field))
            .cloned()
            .expect("forwarding endpoint field registered before rendering")
    }
}

pub(in crate::features) use nyaterm_connections_gpui::forms::{
    FieldLabel, field_caption, required,
};

/// The height every input, select and stepper in the editor shares.
pub(in crate::features) const EDITOR_CONTROL_HEIGHT_PX: f32 = NYA_FORM_CONTROL_HEIGHT_PX;

/// A caption above one editable field.
///
/// Tauri puts the caption above the box rather than inside it; a box that also
/// holds its label has half the room for what is typed into it, which is what
/// made long hosts and paths unreadable here.
pub(in crate::features) fn editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &Context<NyaTermApp>,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(format!(
            "connection-editor-field-{field:?}"
        )))
        .debug_selector(move || format!("connection-editor-field-{field:?}"))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Field(field))
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(editor_field_box(palette, field, fields, cx))
        .when_some(fields.errors.get(&field), |this, error| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            )
        })
}

pub(super) fn connection_editor_select_on_open(
    render: ConnectionEditorRenderContext<'_, '_>,
    id: &'static str,
    label: impl Into<FieldLabel>,
    select: ConnectionEditorSelect,
    on_open: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let _ = &render.cx;
    let anchor = render
        .fields
        .anchors
        .get(&ConnectionEditorFocusTarget::Select(select))
        .cloned();
    nyaterm_connections_gpui::forms::select_field(
        render.palette,
        id,
        label,
        anchor,
        NyaSelect::new(&render.fields.select(select)).on_open(on_open),
    )
}

pub(super) fn forwarding_endpoint_editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    index: usize,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.forwarding_endpoint_field(index, field);
    let handle = entity.read(cx).focus_handle();
    let focused = entity.read(cx).has_focus();
    div()
        .id(SharedString::from(format!(
            "connection-editor-forwarding-{index}-{field:?}"
        )))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::ForwardingEndpoint { index, field })
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .id(SharedString::from(format!(
                    "connection-agent-endpoint-{index}-field"
                )))
                .min_w_0()
                .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                .flex()
                .items_center()
                .rounded_sm()
                .border_1()
                .border_color(ordinary_input_shell_border_color(palette, focused))
                .when(focused, |this| {
                    this.shadow(ordinary_input_focus_ring(palette))
                })
                .bg(rgb(palette.input_surface()))
                .cursor_text()
                .on_click(move |_, window, cx| {
                    window.focus(&handle, cx);
                })
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaInput::new(&entity)),
                ),
        )
}

/// The bordered box holding one field, without a caption.
pub(in crate::features) fn editor_field_box(
    palette: crate::theme::ThemePalette,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &Context<NyaTermApp>,
) -> impl IntoElement {
    let entity = fields.get(&field);
    if field == ConnectionEditorField::PostLoginCommand {
        let app = cx.weak_entity();
        return div()
            .w_full()
            .min_w_0()
            .flex_none()
            .children(entity.map(|field| {
                NyaInputShell::new("connection-editor-post-login-command", field)
                    .multi_line()
                    .height(px(168.))
                    .tab_navigation()
                    .on_secondary_enter(move |window, cx| {
                        let _ = app.update(cx, |app, cx| app.save_connection_editor(window, cx));
                    })
            }))
            .into_any_element();
    }
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    div()
        .h(px(if field == ConnectionEditorField::Description {
            112.
        } else {
            EDITOR_CONTROL_HEIGHT_PX
        }))
        .id("connection-list-search-input-shell")
        .min_w_0()
        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(ordinary_input_shell_border_color(palette, focused))
        .when(focused, |this| {
            this.shadow(ordinary_input_focus_ring(palette))
        })
        .bg(rgb(palette.input_surface()))
        .cursor_text()
        .when_some(handle, |row, handle| {
            row.on_click(move |_, window, cx| {
                window.focus(&handle, cx);
            })
        })
        .children(entity.map(|field| {
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaInput::new(field))
        }))
        .into_any_element()
}

/// A numeric field backed by gpui-kit's spinner control.
pub(in crate::features) fn editor_stepper_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    _cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.get_number(&field);
    div()
        .id(SharedString::from(format!(
            "connection-editor-field-{field:?}"
        )))
        .debug_selector(move || format!("connection-editor-field-{field:?}"))
        .anchor_scroll(
            fields
                .anchors
                .get(&ConnectionEditorFocusTarget::Field(field))
                .cloned(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .flex()
                .items_center()
                .children(entity.map(|field| {
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaNumberInput::new(field))
                })),
        )
        .when_some(fields.errors.get(&field), |this, error| {
            this.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(error.clone()),
            )
        })
}

pub(in crate::features) use nyaterm_connections_gpui::forms::icon_action_button;

/// [`icon_action_button`] with the two knobs the sort control needs: a tint that
/// marks the button as active, and a vertical flip that turns an ascending glyph
/// into a descending one.
pub(in crate::features) use nyaterm_connections_gpui::forms::icon_action_button_styled;

pub(super) use nyaterm_connections_gpui::details::{
    ConnectionDetailRow, connection_detail_rows, connection_share_text,
};

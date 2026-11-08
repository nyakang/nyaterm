//! Connection editor groups presentation.

use crate::features::NyaTermApp;
use crate::features::pages::connections::list::{ConnectionEditorFields, EDITOR_CONTROL_HEIGHT_PX};
use crate::features::selects::PENDING_CONNECTION_GROUP_VALUE;
use crate::features::text_inputs::{
    ORDINARY_INPUT_SHELL_PADDING_X_PX, ordinary_input_focus_ring, ordinary_input_shell_border_color,
};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect, ConnectionEditorState};
use gpui::prelude::*;
use gpui::{
    Bounds, Context, IntoElement, Pixels, Point, SharedString, anchored, deferred, div, px, rgb,
    rgba, svg,
};
use nyaterm_core::{Group, natural_compare};
use nyaterm_ui::{NyaInput, NyaScrollArea};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

pub(super) fn connection_group_path_label(groups: &[Group], group_id: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut next = Some(group_id);
    let mut seen = HashSet::new();
    while let Some(id) = next {
        if !seen.insert(id.to_string()) {
            break;
        }
        let group = groups.iter().find(|group| group.id == id)?;
        parts.push(group.name.clone());
        next = group.parent_id.as_deref();
    }
    parts.reverse();
    Some(parts.join(" / "))
}

pub(super) struct ConnectionEditorGroupControlArgs<'a> {
    pub(super) palette: crate::theme::ThemePalette,
    pub(super) label: Cow<'static, str>,
    pub(super) display_label: String,
    pub(super) select_open: bool,
    pub(super) trigger_bounds: Option<Bounds<Pixels>>,
    pub(super) parent_hint: String,
    pub(super) choices: &'a [ConnectionEditorGroupMenuOption],
    pub(super) fields: &'a ConnectionEditorFields,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConnectionEditorGroupMenuOption {
    pub(super) value: Option<String>,
    pub(super) label: String,
    pub(super) depth: usize,
    pub(super) selected: bool,
    pub(super) disabled: bool,
    pub(super) none_option: bool,
}

pub(super) fn connection_editor_group_control(
    args: ConnectionEditorGroupControlArgs<'_>,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let ConnectionEditorGroupControlArgs {
        palette,
        label,
        display_label,
        select_open,
        trigger_bounds,
        parent_hint,
        choices,
        fields,
    } = args;
    let new_group_entity = fields.get(&ConnectionEditorField::NewGroupName);
    let new_group_field = new_group_entity.cloned();
    let new_group_focused = new_group_entity.is_some_and(|field| field.read(cx).has_focus());
    let can_add = new_group_entity.is_some_and(|field| !field.read(cx).value(cx).trim().is_empty());
    let popup_position = trigger_bounds.map(connection_editor_group_popup_position);
    let popup_width = trigger_bounds.map(|bounds| bounds.size.width.max(px(192.)));
    let app = cx.entity();

    div()
        .id("connection-editor-group")
        .relative()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .on_children_prepainted({
                    let app = app.clone();
                    move |bounds, _, cx| {
                        let Some(bounds) = bounds.into_iter().next() else {
                            return;
                        };
                        app.update(cx, |this, cx| {
                            this.set_connection_group_select_trigger_bounds(bounds, cx);
                        });
                    }
                })
                .id("connection-editor-group-row")
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .flex()
                .items_center()
                .child(
                    div()
                        .id("connection-editor-group-trigger")
                        .h(px(EDITOR_CONTROL_HEIGHT_PX))
                        .min_w_0()
                        .flex_1()
                        .px_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(if select_open {
                            rgb(palette.primary)
                        } else {
                            rgb(palette.border)
                        })
                        .bg(rgb(palette.input_surface()))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(palette.text))
                                .child(display_label),
                        )
                        .child(
                            svg()
                                .size(px(14.))
                                .path("icons/chevron-down.svg")
                                .text_color(rgb(palette.text_muted)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_connection_group_select(cx);
                        })),
                ),
        )
        .when(select_open, |this| {
            let Some(popup_position) = popup_position else {
                return this;
            };
            let popup_width = popup_width.unwrap_or(px(192.));
            this.child(
                deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .position(popup_position)
                        .child(
                            div()
                                .occlude()
                                .w(popup_width)
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface_elevated))
                                .shadow_lg()
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .child(
                                    div().flex_none().w_full().max_h(px(192.)).bg(rgb(
                                        palette.surface_elevated,
                                    )).child(
                                        NyaScrollArea::new("connection-editor-group-options")
                                            .max_h(px(192.))
                                            .child(connection_editor_group_option_list(
                                                palette, choices, cx,
                                            )),
                                    ),
                                )
                                .child(
                                    div()
                                        .id("connection-editor-new-group-footer")
                                        .flex_none()
                                        .p_2()
                                        .border_t_1()
                                        .border_color(rgb(palette.border))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .h(px(32.))
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .id("connection-editor-new-group-input")
                                                        .h(px(32.))
                                                        .min_w_0()
                                                        .flex_1()
                                                        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                                                        .flex()
                                                        .items_center()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(
                                                            ordinary_input_shell_border_color(
                                                                palette,
                                                                new_group_focused,
                                                            ),
                                                        )
                                                        .when(new_group_focused, |this| {
                                                            this.shadow(
                                                                ordinary_input_focus_ring(palette),
                                                            )
                                                        })
                                                        .bg(rgb(palette.input_surface()))
                                                        .children(new_group_field.map(|field| {
                                                            div()
                                                                .min_w_0()
                                                                .flex_1()
                                                                .text_xs()
                                                                .child(NyaInput::new(&field))
                                                        })),
                                                )
                                                .child(
                                                    div()
                                                        .id("connection-editor-new-group-add")
                                                        .size(px(32.))
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(palette.border))
                                                        .bg(rgb(palette.input))
                                                        .text_color(rgb(palette.text_muted))
                                                        .opacity(if can_add { 1.0 } else { 0.45 })
                                                        .when(can_add, |this| {
                                                            this.cursor_pointer()
                                                                .hover(|this| {
                                                                    this.bg(rgb(palette.hover))
                                                                })
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.commit_connection_editor_new_group(cx);
                                                                    },
                                                                ))
                                                        })
                                                        .child(
                                                            svg()
                                                                .size(px(15.))
                                                                .path("icons/conn/add.svg")
                                                                .text_color(rgb(
                                                                    palette.text_muted,
                                                                )),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(palette.text_dimmed))
                                                .child(parent_hint),
                                        ),
                                )
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.close_connection_group_select(cx);
                                })),
                        ),
                )
                .with_priority(1),
            )
        })
}

pub(super) fn connection_editor_group_option_list(
    palette: crate::theme::ThemePalette,
    choices: &[ConnectionEditorGroupMenuOption],
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    choices
        .iter()
        .fold(div().w_full().flex().flex_col(), |list, choice| {
            let selected = choice.selected;
            let value = choice.value.clone();
            let disabled = choice.disabled;
            let none_option = choice.none_option;
            let padding_left = px(12. + (choice.depth as f32 * 16.));
            let row_id = value
                .as_deref()
                .map(|id| format!("connection-editor-group-option-{id}"))
                .unwrap_or_else(|| "connection-editor-group-option-none".to_string());
            list.child(
                div()
                    .id(SharedString::from(row_id))
                    .w_full()
                    .h(px(28.))
                    .pl(padding_left)
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .text_color(if selected {
                        rgb(palette.primary)
                    } else if none_option || disabled {
                        rgb(palette.text_muted)
                    } else {
                        rgb(palette.text)
                    })
                    .when(!disabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(rgb(palette.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_connection_editor_select_value(
                                    ConnectionEditorSelect::Group,
                                    value.as_deref(),
                                    cx,
                                );
                            }))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .child(choice.label.clone()),
                    ),
            )
        })
}

pub(super) fn connection_editor_group_popup_position(bounds: Bounds<Pixels>) -> Point<Pixels> {
    Point {
        x: bounds.origin.x,
        y: bounds.origin.y + bounds.size.height + px(4.),
    }
}

pub(super) fn connection_editor_group_display_label(
    groups: &[Group],
    editor: &ConnectionEditorState,
    none_label: impl Into<SharedString>,
    new_group_label: impl Into<SharedString>,
) -> String {
    let none_label: SharedString = none_label.into();
    let new_group_label: SharedString = new_group_label.into();
    if let Some(name) = editor.pending_group_name.as_deref() {
        return format!("{name} ({new_group_label})");
    }
    editor
        .group_id
        .as_deref()
        .and_then(|id| connection_group_path_label(groups, id))
        .unwrap_or_else(|| none_label.to_string())
}

pub(super) fn connection_editor_group_menu_options(
    groups: &[Group],
    editor: &ConnectionEditorState,
    none_label: impl Into<SharedString>,
    new_group_label: impl Into<SharedString>,
) -> Vec<ConnectionEditorGroupMenuOption> {
    let none_label: SharedString = none_label.into();
    let new_group_label: SharedString = new_group_label.into();
    let mut options = vec![ConnectionEditorGroupMenuOption {
        value: None,
        label: none_label.to_string(),
        depth: 0,
        selected: editor.group_id.is_none() && editor.pending_group_name.is_none(),
        disabled: false,
        none_option: true,
    }];

    if let Some(name) = editor.pending_group_name.as_deref() {
        options.push(ConnectionEditorGroupMenuOption {
            value: Some(PENDING_CONNECTION_GROUP_VALUE.to_string()),
            label: format!("{name} ({new_group_label})"),
            depth: 0,
            selected: true,
            disabled: true,
            none_option: false,
        });
    }

    options.extend(
        ordered_connection_groups(groups)
            .into_iter()
            .map(|(group, depth)| {
                let selected = editor.pending_group_name.is_none()
                    && editor.group_id.as_deref() == Some(group.id.as_str());
                ConnectionEditorGroupMenuOption {
                    value: Some(group.id.clone()),
                    label: group.name,
                    depth,
                    selected,
                    disabled: false,
                    none_option: false,
                }
            }),
    );

    options
}

pub(in crate::features::pages::connections) fn ordered_connection_groups(
    groups: &[Group],
) -> Vec<(Group, usize)> {
    let group_ids = groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<HashSet<_>>();
    let mut children = HashMap::<Option<String>, Vec<Group>>::new();
    for group in groups {
        let parent_id = group
            .parent_id
            .clone()
            .filter(|parent_id| parent_id != &group.id && group_ids.contains(parent_id));
        children.entry(parent_id).or_default().push(group.clone());
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                // Same rule as the panel tree, so the dropdown and the list agree.
                .then_with(|| natural_compare(&left.name, &right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn append_group(
        group: Group,
        depth: usize,
        children: &HashMap<Option<String>, Vec<Group>>,
        visited: &mut HashSet<String>,
        ordered: &mut Vec<(Group, usize)>,
    ) {
        if !visited.insert(group.id.clone()) {
            return;
        }
        let group_id = group.id.clone();
        ordered.push((group, depth));
        for child in children.get(&Some(group_id)).cloned().unwrap_or_default() {
            append_group(child, depth + 1, children, visited, ordered);
        }
    }

    let mut ordered = Vec::with_capacity(groups.len());
    let mut visited = HashSet::new();
    for group in children.get(&None).cloned().unwrap_or_default() {
        append_group(group, 0, &children, &mut visited, &mut ordered);
    }
    let mut remaining = groups
        .iter()
        .filter(|group| !visited.contains(&group.id))
        .cloned()
        .collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    for group in remaining {
        append_group(group, 0, &children, &mut visited, &mut ordered);
    }
    ordered
}

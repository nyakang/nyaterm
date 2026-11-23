use nyaterm_ui::icons::{resolve_connection_icon, saved_connection_icon};
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, FontWeight, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, relative, rgb, rgba, svg,
};
use nyaterm_core::SavedConnection;
use nyaterm_ui::NyaInput;

use crate::features::{
    connections::ConnectionDragKind, connections::ConnectionDragPayload,
    connections::ConnectionDragPreview, connections::ConnectionDropPosition,
    text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX, text_inputs::ordinary_input_focus_ring,
    text_inputs::ordinary_input_shell_border_color, view_widgets::mono_icon,
};

use super::super::list::{
    ConnectionDetailRow, ConnectionSectionHeader, connection_detail_rows, connection_share_text,
    connection_tree_indent_px,
};
use super::super::panel::{ConnectionListSnapshot, ConnectionPanel};

fn connection_group_icon(expanded: bool) -> gpui::Svg {
    // Tauri GroupNodeItem uses Tailwind 4's text-amber-500/70.
    let path = if expanded {
        "icons/session/folder-open.svg"
    } else {
        "icons/conn/folder.svg"
    };
    mono_icon(path, rgb(0xfe9a00).into(), 16.).opacity(0.7)
}

pub(in crate::features::pages::connections) fn connection_section(
    snapshot: &ConnectionListSnapshot,
    section: ConnectionSectionHeader,
    viewport: &gpui::ScrollHandle,
    cx: &mut Context<ConnectionPanel>,
) -> impl IntoElement {
    // The flat list only ever emits headers for real groups; root sections are
    // spread into `Connection` rows by `flatten_connection_rows`. The nested
    // body this used to draw under a header went away with virtualization.
    debug_assert!(!section.is_root, "root sections do not become header rows");
    let palette = snapshot.chrome.palette;
    let expanded = snapshot.group_is_expanded(section.group_id.as_deref());
    let group_id = section.group_id.clone();
    let group_label = section.label.clone();
    let count = section.total_count;
    let editing_group = section
        .group_id
        .as_deref()
        .is_some_and(|id| snapshot.group_editor_is_renaming(id));
    let editor_error = editing_group
        .then(|| {
            snapshot
                .group_editor
                .as_ref()
                .and_then(|editor| editor.error.clone())
        })
        .flatten();
    let group_header = div()
        .id(SharedString::from(format!(
            "connection-section-{}",
            section.group_id.clone().unwrap_or_else(|| "root".into())
        )))
        .relative()
        .h(px(28.))
        .min_w(relative(1.))
        .flex()
        .items_center()
        .gap(px(6.))
        .px_2()
        .pl(px(8. + section.depth as f32 * 16.))
        .rounded_sm()
        .cursor_default()
        .bg({
            let drop_inside = snapshot.drop_position_for_kind_target(
                ConnectionDragKind::Group,
                section.group_id.as_deref(),
            ) == Some(ConnectionDropPosition::Inside);
            if drop_inside || snapshot.group_is_hovered(section.group_id.as_deref()) {
                rgb(palette.hover)
            } else {
                rgba(0x00000000)
            }
        })
        .when(
            snapshot.drop_position_for_kind_target(
                ConnectionDragKind::Group,
                section.group_id.as_deref(),
            ) == Some(ConnectionDropPosition::Inside),
            |this| {
                this.border_1()
                    .border_color(rgb(snapshot.chrome.palette.link))
            },
        )
        .on_hover({
            let hover_group = section.group_id.clone();
            cx.listener(move |panel, hovered: &bool, _, cx| {
                panel.with_app(cx, |this, cx| {
                    if let Some(group_id) = hover_group.clone()
                        && this
                            .connection_state
                            .set_list_group_hover(group_id, *hovered)
                    {
                        cx.notify();
                    }
                })
            })
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        // Aims the list's one context menu at this group. Capture, so it runs
        // before the menu is built and regardless of who stops the bubble.
        .capture_any_mouse_down({
            let menu_group_id = section.group_id.clone().unwrap_or_default();
            cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                panel.with_app(cx, |this, _cx| {
                    if event.button == MouseButton::Right {
                        this.connection_state
                            .prepare_list_group_context_menu(menu_group_id.clone());
                    }
                })
            })
        })
        .when_some(
            (!editing_group)
                .then_some(section.group_id.clone())
                .flatten(),
            |this, drag_group_id| {
                let label = section.label.clone();
                let drag_scroll = viewport.clone();
                this.on_drag(
                    ConnectionDragPayload {
                        icon: None,
                        custom_icon: None,
                        palette,
                        panel: cx.weak_entity(),
                        kind: ConnectionDragKind::Group,
                        id: drag_group_id.clone(),
                        label,
                    },
                    move |payload, position, window, cx| {
                        // GPUI installs the active drag after this constructor returns.
                        window.defer(cx, |window, cx| {
                            cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                        });
                        let _ = payload.panel.update(cx, |panel, cx| {
                            panel.arm_tree_drag(
                                payload,
                                window.mouse_position(),
                                drag_scroll.bounds(),
                                drag_scroll.clone(),
                                cx,
                            )
                        });
                        cx.new(|_| ConnectionDragPreview::new(payload.clone(), position))
                    },
                )
            },
        )
        .on_click(cx.listener(move |panel, _, _, cx| {
            panel.with_app(cx, |this, cx| {
                cx.stop_propagation();
                if editing_group {
                    return;
                }
                if let Some(group_id) = group_id.clone() {
                    this.toggle_connection_group_expanded(group_id, cx);
                }
            })
        }))
        .on_key_down(cx.listener(|panel, event: &KeyDownEvent, _, cx| {
            panel.with_app(cx, |this, cx| {
                this.handle_connection_group_editor_key_down(event, cx);
            })
        }))
        // The name takes the slack so the count sits against the right
        // edge of the panel, where Tauri puts it.
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(if expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/fe/forward.svg"
                })
                .text_color(rgb(palette.text_muted)),
        )
        .child(connection_group_icon(expanded))
        .child(if editing_group {
            connection_group_editor_input_box(snapshot, cx)
        } else {
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.text_muted))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(group_label.clone())
                .into_any_element()
        })
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(rgb(if editor_error.is_some() {
                    palette.danger
                } else {
                    palette.text_dimmed
                }))
                .child(editor_error.unwrap_or_else(|| count.to_string())),
        );
    div().flex().flex_col().child(group_header)
}

pub(in crate::features::pages::connections) fn connection_inline_group_editor_row(
    snapshot: &ConnectionListSnapshot,
    parent_id: Option<String>,
    depth: usize,
    cx: &mut Context<ConnectionPanel>,
) -> impl IntoElement {
    let palette = snapshot.chrome.palette;
    let editor_error = snapshot
        .group_editor
        .as_ref()
        .and_then(|editor| editor.error.clone());
    div()
        .id(SharedString::from(format!(
            "connection-inline-group-editor-{}",
            parent_id.unwrap_or_else(|| "root".to_string())
        )))
        .relative()
        .h(px(28.))
        .min_w(relative(1.))
        .flex()
        .items_center()
        .gap(px(6.))
        .px_2()
        .pl(px(connection_tree_indent_px(depth)))
        .rounded_sm()
        .bg(rgb(palette.hover))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        .on_key_down(cx.listener(|panel, event: &KeyDownEvent, _, cx| {
            panel.with_app(cx, |this, cx| {
                this.handle_connection_group_editor_key_down(event, cx);
            })
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/fe/forward.svg")
                .text_color(rgb(palette.text_muted)),
        )
        .child(connection_group_icon(false))
        .child(connection_group_editor_input_box(snapshot, cx))
        .when_some(editor_error, |this, error| {
            this.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(palette.danger))
                    .child(error),
            )
        })
}

fn connection_group_editor_input_box(
    snapshot: &ConnectionListSnapshot,
    cx: &mut Context<ConnectionPanel>,
) -> AnyElement {
    let palette = snapshot.chrome.palette;
    let Some(field) = snapshot.group_editor_field.clone() else {
        return div().min_w_0().flex_1().into_any_element();
    };
    let handle = field.read(cx).focus_handle();
    let focused = field.read(cx).has_focus();
    let has_error = snapshot
        .group_editor
        .as_ref()
        .is_some_and(|editor| editor.error.is_some());
    div()
        .h(px(24.))
        .min_w_0()
        .flex_1()
        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(if has_error {
            rgb(palette.danger)
        } else {
            ordinary_input_shell_border_color(palette, focused)
        })
        .when(focused, |this| {
            this.shadow(ordinary_input_focus_ring(palette))
        })
        .bg(rgb(palette.input_surface()))
        .cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _, window, cx| {
                cx.stop_propagation();
                window.focus(&handle, cx);
            }),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaInput::new(&field)),
        )
        .into_any_element()
}

pub(in crate::features::pages::connections) fn saved_connection_row(
    snapshot: &ConnectionListSnapshot,
    connection: SavedConnection,
    depth: usize,
    hover_viewport: &gpui::ScrollHandle,
    cx: &mut Context<ConnectionPanel>,
) -> impl IntoElement {
    let drag_scroll = hover_viewport.clone();
    let selected = snapshot.is_selected(&connection.id);
    // The arrow keys walk filtered results without disturbing the selection,
    // so the active row gets its own fainter wash plus a ring.
    let keyboard_active = snapshot.is_keyboard_active(&connection.id);
    let connect_connection_dbl = connection.clone();
    let select_id = connection.id.clone();
    let menu_id = connection.id.clone();
    let kind = connection.kind_label();
    let icon_def = resolve_connection_icon(connection.icon.as_deref(), kind);
    let custom_icon = connection
        .icon
        .as_ref()
        .and_then(|id| snapshot.custom_icons.get(id))
        .cloned();
    let details_rows: Arc<[ConnectionDetailRow]> =
        connection_detail_rows(&connection, &snapshot.connections_by_id).into();
    let share_text = connection_share_text(&connection, &details_rows);
    let drop_position = snapshot
        .drop_position_for_kind_target(ConnectionDragKind::Connection, Some(&connection.id));
    let show_before = drop_position == Some(ConnectionDropPosition::Before);
    let show_after = drop_position == Some(ConnectionDropPosition::After);
    let show_inside = drop_position == Some(ConnectionDropPosition::Inside);
    let palette = snapshot.chrome.palette;
    nyaterm_connections_gpui::rows::saved_connection_row(
        nyaterm_connections_gpui::rows::ConnectionRow {
            id: connection.id.clone(),
            name: connection.name.clone(),
            depth,
            palette,
            selected,
            keyboard_active,
            drop_before: show_before,
            drop_after: show_after,
            drop_inside: show_inside,
            icon: saved_connection_icon(palette, icon_def, custom_icon.clone(), selected, 16.)
                .into_any_element(),
            details: details_rows,
            share_text,
        },
        hover_viewport,
    )
    .on_drag(
        ConnectionDragPayload {
            icon: Some(icon_def),
            custom_icon: custom_icon.clone(),
            palette,
            panel: cx.weak_entity(),
            kind: ConnectionDragKind::Connection,
            id: connection.id.clone(),
            label: connection.name.clone(),
        },
        move |payload, position, window, cx| {
            // GPUI installs the active drag after this constructor returns.
            window.defer(cx, |window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
            });
            let _ = payload.panel.update(cx, |panel, cx| {
                panel.arm_tree_drag(
                    payload,
                    window.mouse_position(),
                    drag_scroll.bounds(),
                    drag_scroll.clone(),
                    cx,
                )
            });
            cx.new(|_| ConnectionDragPreview::new(payload.clone(), position))
        },
    )
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(|_, _, _, cx| {
            cx.stop_propagation();
        }),
    )
    // Aims the list's one context menu at this row. Capture, so it runs
    // before the menu is built and regardless of who stops the bubble.
    .capture_any_mouse_down(
        cx.listener(move |panel, event: &MouseDownEvent, window, cx| {
            window.focus(panel.focus_handle(), cx);
            panel.with_app(cx, |this, cx| {
                if event.button == MouseButton::Right {
                    this.prepare_connection_context_menu(menu_id.clone(), cx);
                }
            })
        }),
    )
    .on_click(
        cx.listener(move |panel, event: &gpui::ClickEvent, window, cx| {
            panel.with_app(cx, |this, cx| {
                cx.stop_propagation();
                if event.click_count() >= 2 {
                    this.start_saved_connection(connect_connection_dbl.clone(), window, cx);
                    return;
                }
                let modifiers = event.modifiers();
                let additive = modifiers.control || modifiers.platform;
                let range = modifiers.shift;
                this.select_connection(select_id.clone(), additive, range, cx);
            })
        }),
    )
}

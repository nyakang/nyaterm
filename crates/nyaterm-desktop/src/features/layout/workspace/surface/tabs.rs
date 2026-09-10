use rust_i18n::t;

use gpui::{
    Anchor, ClickEvent, Context, FontWeight, IntoElement, MouseButton, ScrollDelta,
    ScrollWheelEvent, SharedString, div, point, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaHoverCard, NyaPopover, NyaPopoverAlign, NyaPopoverPlacement};

use crate::features::NyaTermApp;
use crate::features::formatting::session_kind_label;
use crate::features::icons::resolve_connection_icon;
use crate::features::shell::{
    NewSessionMenuAnchor, SessionTabDragPayload, SessionTabDragPreview, SessionTabTooltip,
};
use crate::features::view_widgets::{connection_spinner, themed_icon};

use super::super::super::view_helpers::session_kind_icon_path;

const SESSION_TAB_MIN_WIDTH: f32 = 118.;
const SESSION_TAB_MAX_WIDTH: f32 = 236.;
const SESSION_TAB_ACTION_SIZE: f32 = 36.;
const SESSION_TAB_END_DROP_TARGET_MIN_WIDTH: f32 = 28.;
const TAB_STRIP_OVERFLOW_TOLERANCE: f32 = 2.;

fn pending_tab_insert_index(
    session_count: usize,
    after_position: Option<usize>,
    insert_index: Option<usize>,
) -> usize {
    insert_index
        .or_else(|| after_position.map(|index| index + 1))
        .unwrap_or(session_count)
        .min(session_count)
}

fn tab_drop_insert_after(source_index: usize, target_index: usize) -> Option<bool> {
    match target_index.cmp(&source_index) {
        std::cmp::Ordering::Less => Some(false),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(true),
    }
}

fn tab_scroll_x(current_x: f32, max_x: f32, delta_x: f32, delta_y: f32) -> f32 {
    let dominant = if delta_x.abs() > delta_y.abs() {
        delta_x
    } else {
        delta_y
    };
    (current_x + dominant).clamp(-max_x.max(0.), 0.)
}

fn tab_strip_overflows(
    max_offset: f32,
    overflow_control_is_rendered: bool,
    end_drop_target_is_rendered: bool,
) -> bool {
    let occupied_control_width = if overflow_control_is_rendered {
        SESSION_TAB_ACTION_SIZE
    } else {
        0.
    };
    let end_drop_target_width = if end_drop_target_is_rendered {
        SESSION_TAB_END_DROP_TARGET_MIN_WIDTH
    } else {
        0.
    };
    max_offset > occupied_control_width + end_drop_target_width + TAB_STRIP_OVERFLOW_TOLERANCE
}

type TransientSessionTab = (usize, u64, String, String, String, Option<String>);
type SessionTabOrderKey = (usize, u64);

fn transient_tab_precedes_session(
    transient: &TransientSessionTab,
    session_key: SessionTabOrderKey,
) -> bool {
    (transient.0, transient.1) < session_key
}

impl NyaTermApp {
    fn pending_session_tab(
        &mut self,
        request_id: String,
        pending_name: String,
        tab_number: usize,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let hover_bg = self.shell_surface_color(palette.hover);
        let close_request_id = request_id.clone();
        let spinner_id = SharedString::from(format!("pending-session-spinner-{request_id}"));
        div()
            .id(SharedString::from(format!(
                "pending-session-tab-{request_id}"
            )))
            .h_full()
            .min_w(px(SESSION_TAB_MIN_WIDTH))
            .max_w(px(SESSION_TAB_MAX_WIDTH))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .relative()
            .border_r_1()
            .border_color(rgb(palette.border))
            .when(!active, |this| this.border_b_1())
            .bg(if active {
                self.shell_surface_color(palette.hover)
            } else {
                self.shell_surface_color(palette.bg)
            })
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .when(active, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(palette.primary)),
                )
            })
            .child(
                div()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(connection_spinner(
                        spinner_id,
                        rgb(palette.primary).into(),
                        14.,
                    )),
            )
            .child(
                div()
                    .min_w(px(12.))
                    .text_size(px(11.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!("{tab_number}")),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(if active {
                        FontWeight(600.)
                    } else {
                        FontWeight(500.)
                    })
                    .text_color(if active {
                        rgb(palette.text)
                    } else {
                        rgb(palette.text_muted)
                    })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(truncate_preview(&pending_name, 28)),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "pending-session-tab-close-{close_request_id}"
                    )))
                    .group(SharedString::from(format!(
                        "pending-session-tab-close-group-{close_request_id}"
                    )))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .hover(|this| this.bg(rgb(palette.border)).text_color(rgb(palette.danger)))
                    .child(
                        svg()
                            .size(px(13.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted))
                            .group_hover(
                                SharedString::from(format!(
                                    "pending-session-tab-close-group-{close_request_id}"
                                )),
                                |this| this.text_color(rgb(palette.danger)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_pending_session_start(close_request_id.clone(), cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_pending_session_start(request_id.clone(), cx);
            }))
    }

    fn failed_session_tab(
        &mut self,
        request_id: String,
        failed_name: String,
        failed_error: String,
        tab_number: usize,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let close_request_id = request_id.clone();
        let select_request_id = request_id.clone();
        div()
            .id(SharedString::from(format!(
                "failed-session-tab-{request_id}"
            )))
            .h_full()
            .min_w(px(SESSION_TAB_MIN_WIDTH))
            .max_w(px(SESSION_TAB_MAX_WIDTH))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .relative()
            .border_r_1()
            .border_color(rgb(palette.border))
            .when(!active, |this| this.border_b_1())
            .bg(if active {
                rgba((palette.danger << 8) | 0x24)
            } else {
                rgba((palette.danger << 8) | 0x12)
            })
            .cursor_pointer()
            .hover(|this| this.bg(rgba((palette.danger << 8) | 0x24)))
            .when(active, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(palette.danger)),
                )
            })
            .child(
                div()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/session/disconnect.svg")
                            .text_color(rgb(palette.danger)),
                    ),
            )
            .child(
                div()
                    .min_w(px(12.))
                    .text_size(px(11.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!("{tab_number}")),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(if active {
                        FontWeight(600.)
                    } else {
                        FontWeight(500.)
                    })
                    .text_color(rgb(palette.danger))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(truncate_preview(&failed_name, 28)),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "failed-session-tab-close-{close_request_id}"
                    )))
                    .group(SharedString::from(format!(
                        "failed-session-tab-close-group-{close_request_id}"
                    )))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .hover(|this| this.bg(rgb(palette.border)).text_color(rgb(palette.danger)))
                    .child(
                        svg()
                            .size(px(13.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted))
                            .group_hover(
                                SharedString::from(format!(
                                    "failed-session-tab-close-group-{close_request_id}"
                                )),
                                |this| this.text_color(rgb(palette.danger)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_failed_session_start(close_request_id.clone(), cx);
                    })),
            )
            .tooltip(move |_, cx| {
                cx.new(|_| SessionTabTooltip::new(failed_name.clone(), vec![failed_error.clone()]))
                    .into()
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_failed_session_start(select_request_id.clone(), cx);
            }))
    }

    pub(in crate::features) fn main_surface(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // The main surface always hosts the terminal workspace. Side panels are
        // rendered by the shell around this surface to match the Tauri layout.
        let palette = self.theme_palette();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(self.shell_transparent_color(palette.bg))
            .child(self.workspace_view(cx))
    }

    pub(in crate::features) fn session_tab_strip(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let shell_hover_bg = self.shell_surface_color(palette.hover);
        let sessions = self.ordered_tab_sessions();
        let session_count = sessions.len();
        let mut transient_tabs: Vec<TransientSessionTab> = self
            .session
            .start_pending_entries()
            .filter_map(|(request_id, pending)| {
                if pending.reconnect_session_id.is_some() {
                    return None;
                }
                let name = pending
                    .custom_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&pending.connection_name)
                    .to_string();
                let after_position = pending.after_session_id.as_ref().and_then(|after_id| {
                    sessions.iter().position(|session| session.id == *after_id)
                });
                let index = pending.tab_placement.map_or_else(
                    || {
                        pending_tab_insert_index(
                            session_count,
                            after_position,
                            pending.insert_index,
                        )
                    },
                    |placement| placement.insert_index,
                );
                Some((
                    index,
                    pending
                        .tab_placement
                        .map(|placement| placement.request_sequence)
                        .unwrap_or(u64::MAX),
                    pending.connection_name.clone(),
                    request_id.clone(),
                    name,
                    None,
                ))
            })
            .collect::<Vec<_>>();
        transient_tabs.extend(
            self.session
                .start_failed_entries()
                .map(|(request_id, failed)| {
                    let pending = &failed.pending;
                    let name = pending
                        .custom_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&pending.connection_name)
                        .to_string();
                    let after_position = pending.after_session_id.as_ref().and_then(|after_id| {
                        sessions.iter().position(|session| session.id == *after_id)
                    });
                    let index = pending.tab_placement.map_or_else(
                        || {
                            pending_tab_insert_index(
                                session_count,
                                after_position,
                                pending.insert_index,
                            )
                        },
                        |placement| placement.insert_index,
                    );
                    (
                        index,
                        pending
                            .tab_placement
                            .map(|placement| placement.request_sequence)
                            .unwrap_or(u64::MAX),
                        pending.connection_name.clone(),
                        request_id.clone(),
                        name,
                        Some(failed.error.clone()),
                    )
                }),
        );
        transient_tabs.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
        });
        if self.shell.session_tab_scroll_into_view_pending() {
            if let Some(active_id) = self.session.active_id()
                && let Some(index) = sessions
                    .iter()
                    .position(|session| session.id == self.tab_root_for_session(active_id))
            {
                let active_key = self
                    .session
                    .session_start_tab_placement(&sessions[index].id)
                    .map(|placement| (placement.insert_index, placement.request_sequence))
                    .unwrap_or((index, u64::MAX));
                let pending_count = transient_tabs
                    .iter()
                    .filter(|transient| transient_tab_precedes_session(transient, active_key))
                    .count();
                let child_index = index + pending_count;
                self.shell
                    .session_tab_strip_scroll()
                    .scroll_to_item(child_index);
            }
            self.shell.consume_session_tab_scroll_into_view();
        }
        let tab_scroll = self.shell.session_tab_strip_scroll().clone();
        let tab_scroll_for_wheel = tab_scroll.clone();
        let mut tabs = div()
            .id("session-tab-strip-scroll")
            .h_full()
            .w_full()
            .flex()
            .items_center()
            // Tauri tab-strip-scroll: horizontal overflow instead of clipping tabs.
            .overflow_x_scroll()
            .overflow_y_hidden()
            .track_scroll(&tab_scroll)
            .on_scroll_wheel(cx.listener(move |_, event: &ScrollWheelEvent, _, cx| {
                let (delta_x, delta_y) = match event.delta {
                    ScrollDelta::Lines(delta) => (delta.x * 36., delta.y * 36.),
                    ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                };
                let max_x = f32::from(tab_scroll_for_wheel.max_offset().x).max(0.);
                if max_x <= 0. || (delta_x == 0. && delta_y == 0.) {
                    return;
                }
                let current = tab_scroll_for_wheel.offset();
                let next_x = tab_scroll_x(f32::from(current.x), max_x, delta_x, delta_y);
                if next_x != f32::from(current.x) {
                    tab_scroll_for_wheel.set_offset(point(px(next_x), px(0.)));
                    cx.stop_propagation();
                }
            }));

        let mut transient_cursor = 0usize;
        for (tab_index, session) in sessions.into_iter().enumerate() {
            let session_key = self
                .session
                .session_start_tab_placement(&session.id)
                .map(|placement| (placement.insert_index, placement.request_sequence))
                .unwrap_or((tab_index, u64::MAX));
            while transient_cursor < transient_tabs.len()
                && transient_tab_precedes_session(&transient_tabs[transient_cursor], session_key)
            {
                let (_, _, _, request_id, name, error) = transient_tabs[transient_cursor].clone();
                let tab_number = tab_index + transient_cursor + 1;
                let active = self.session.start_request_is_active(&request_id);
                tabs = tabs.child(match error {
                    Some(error) => self
                        .failed_session_tab(request_id, name, error, tab_number, active, cx)
                        .into_any_element(),
                    None => self
                        .pending_session_tab(request_id, name, tab_number, active, cx)
                        .into_any_element(),
                });
                transient_cursor += 1;
            }
            let display_name = self.session.display_name_by_info(&session);
            let session_id = session.id.clone();
            let close_session_id = session.id.clone();
            let tab_group_name = SharedString::from(format!("session-tab-group-{session_id}"));
            let tab_number = tab_index + transient_cursor + 1;
            let kind_icon = session_kind_icon_path(session.kind);
            let custom_icon = self
                .session
                .metadata(&session.id)
                .and_then(|metadata| metadata.source_connection_id.as_ref())
                .and_then(|id| {
                    self.connection_state
                        .connections()
                        .iter()
                        .find(|connection| &connection.id == id)
                })
                .and_then(|connection| connection.icon.as_ref())
                .and_then(|id| self.connection_state.custom_icons.images.get(id))
                .cloned();
            let saved_icon = self
                .session
                .metadata(&session.id)
                .and_then(|metadata| metadata.source_connection_id.as_deref())
                .and_then(|connection_id| {
                    self.connection_state
                        .connections()
                        .iter()
                        .find(|connection| connection.id == connection_id)
                })
                .and_then(|connection| connection.icon.as_deref())
                .filter(|icon| !icon.trim().is_empty())
                .map(|icon| {
                    let kind = match session.kind {
                        nyaterm_transport::SessionKind::LocalPty => "Local",
                        nyaterm_transport::SessionKind::Ssh => "SSH",
                        nyaterm_transport::SessionKind::Telnet => "Telnet",
                        nyaterm_transport::SessionKind::Serial => "Serial",
                        nyaterm_transport::SessionKind::RawTcp => "SSH",
                        nyaterm_transport::SessionKind::Rdp => "RDP",
                        nyaterm_transport::SessionKind::Vnc => "VNC",
                    };
                    resolve_connection_icon(Some(icon), kind)
                });
            let is_locked = self.tab_tree_is_locked(&session.id);
            let drop_target_session_id = session.id.clone();
            let drag_over_target_session_id = session.id.clone();
            let is_drag_source = self.session.tab_drag_source_is(&session.id);
            let custom_color = self.session.tab_color(&session.id);
            // Active when any leaf under this tab root is focused.
            let is_active = self
                .session
                .active_id()
                .is_some_and(|id| self.tab_root_for_session(id) == session.id);
            let leaf_ids = self
                .shell
                .workspace_pane_root(&session.id)
                .map(|root| root.session_ids())
                .unwrap_or_else(|| vec![session.id.clone()]);
            let is_disconnected = leaf_ids.iter().any(|id| self.session.is_disconnected(id));
            let tab_title = truncate_preview(&display_name, 28);
            let has_unread = leaf_ids
                .iter()
                .any(|id| self.terminal.session_has_unread(id));
            let sync_group = leaf_ids
                .iter()
                .find_map(|id| self.sync_input.active_group_for_session(id));
            let sync_paused = leaf_ids
                .iter()
                .any(|id| self.sync_input.session_is_paused_in_active_group(id));
            let show_sync_indicator = self.sync_input.broadcast_to_all() || sync_group.is_some();
            let sync_indicator_color = sync_group.map(|group| group.color).unwrap_or(palette.link);
            let accent_color = if is_active {
                custom_color.unwrap_or(palette.primary)
            } else if let Some(custom_color) = custom_color {
                custom_color
            } else if is_disconnected {
                palette.danger
            } else if has_unread {
                palette.warning
            } else {
                palette.text_dimmed
            };
            let accent = rgb(accent_color);
            let bg = if let Some(custom_color) = custom_color {
                rgba((custom_color << 8) | if is_active { 0x24 } else { 0x14 })
            } else if is_active {
                self.shell_surface_color(palette.bg)
            } else {
                rgba(0x00000000)
            };
            let hover_bg = if let Some(custom_color) = custom_color {
                rgba((custom_color << 8) | if is_active { 0x32 } else { 0x22 })
            } else {
                self.shell_surface_color(palette.hover)
            };
            // Match the Zed tab affordance through NyaTerm's theme projection:
            // the whole target uses the theme hover surface and the insertion
            // edge uses the focus/drag border color.
            let drag_target_bg = self.shell_surface_color(palette.hover);
            let drag_payload = SessionTabDragPayload {
                session_id: session.id.clone(),
                order_index: tab_index,
                display_name: display_name.clone(),
                kind_label: session_kind_label(session.kind),
                kind_icon,
                preview_background: palette.surface,
                preview_border: palette.border,
                preview_text: palette.text,
                preview_text_muted: palette.text_muted,
                preview_accent: accent_color,
            };
            let hover_card = self.session_tab_hover_card(&session.id, palette);
            let hover_card_id = format!("session-tab-hover-card-{session_id}");
            let tab = div()
                .id(SharedString::from(format!("session-tab-{session_id}")))
                .group(tab_group_name.clone())
                // HoverCard adds trigger-host elements between this tab and
                // the 36px strip. Keep the tab's old explicit strip height so
                // those hosts cannot collapse `h_full()` to content height.
                .h(px(36.))
                .min_w(px(SESSION_TAB_MIN_WIDTH))
                .max_w(px(SESSION_TAB_MAX_WIDTH))
                .flex_none()
                .pl_3()
                .pr_2()
                .flex()
                .items_center()
                .gap_2()
                .relative()
                .border_r_1()
                .border_color(rgb(palette.border))
                .when(!is_active, |this| this.border_b_1())
                .bg(bg)
                .when(is_disconnected, |this| this.opacity(0.78))
                .when(is_drag_source, |this| this.opacity(0.55))
                .cursor_pointer()
                .hover(move |this| this.bg(hover_bg))
                .cursor_move()
                .on_drag(drag_payload, |payload, position, _, cx| {
                    cx.new(|_| SessionTabDragPreview::new(payload.clone(), position))
                })
                .drag_over::<SessionTabDragPayload>(move |this, payload, _, _| {
                    let Some(insert_after) = tab_drop_insert_after(payload.order_index, tab_index)
                    else {
                        return this;
                    };
                    if payload.session_id == drag_over_target_session_id {
                        return this;
                    }
                    let target = this
                        .bg(drag_target_bg)
                        .border_0()
                        .border_color(rgb(palette.focus_ring));
                    if insert_after {
                        target.border_r_2()
                    } else {
                        target.border_l_2()
                    }
                })
                .on_drag_move(cx.listener(
                    move |this, event: &gpui::DragMoveEvent<SessionTabDragPayload>, _, cx| {
                        let payload = event.drag(cx);
                        this.update_session_tab_drag(payload.session_id.clone(), cx);
                    },
                ))
                .on_drop(
                    cx.listener(move |this, payload: &SessionTabDragPayload, _, cx| {
                        let Some(insert_after) =
                            tab_drop_insert_after(payload.order_index, tab_index)
                        else {
                            this.clear_session_tab_drag(cx);
                            return;
                        };
                        this.reorder_session_relative(
                            payload.session_id.clone(),
                            drop_target_session_id.clone(),
                            insert_after,
                            cx,
                        );
                    }),
                )
                .when(custom_color.is_some(), move |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(3.))
                            .bg(accent),
                    )
                })
                // Tauri tab: top accent when active, icon + name + close.
                .when(is_active, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(2.))
                            .bg(accent),
                    )
                })
                .child(
                    div()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if let Some(image) = custom_icon {
                            gpui::img(image).size(px(14.)).into_any_element()
                        } else if let Some(icon) = saved_icon {
                            themed_icon(palette, icon, is_active, 14.)
                        } else {
                            svg()
                                .size(px(14.))
                                .path(kind_icon)
                                .text_color(accent)
                                .into_any_element()
                        }),
                )
                .child(
                    div()
                        .min_w(px(12.))
                        .text_size(px(12.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(format!("{tab_number}")),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .max_w(px(160.))
                        .text_size(px(12.))
                        .font_weight(if is_active {
                            FontWeight(700.)
                        } else {
                            FontWeight(500.)
                        })
                        .text_color(if is_disconnected {
                            rgb(palette.text_dimmed)
                        } else if is_active {
                            rgb(palette.text)
                        } else {
                            rgb(palette.text_muted)
                        })
                        .overflow_hidden()
                        // Without this the title wraps inside the tab and the
                        // strip shows whichever line happens to land on the
                        // one row of height it has — "ste", out of "System32".
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(tab_title.clone()),
                )
                .when(show_sync_indicator, |this| {
                    this.child(
                        div()
                            .size(px(12.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(if sync_paused { 0.4 } else { 1. })
                            .child(
                                svg()
                                    .size(px(10.))
                                    .path("icons/sync.svg")
                                    .text_color(rgb(sync_indicator_color)),
                            ),
                    )
                })
                .when(has_unread && !is_active, |this| {
                    this.child(div().size(px(7.)).rounded_full().bg(rgb(palette.success)))
                })
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "session-tab-close-{close_session_id}"
                        )))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .when(!is_active && !is_locked, |this| {
                            this.opacity(0.)
                                .group_hover(tab_group_name.clone(), |style| style.opacity(1.))
                        })
                        .hover(|this| {
                            this.bg(rgb(palette.border)).text_color(if is_locked {
                                rgb(palette.warning)
                            } else {
                                rgb(palette.danger)
                            })
                        })
                        .child(
                            svg()
                                .size(px(13.))
                                .path(if is_locked {
                                    "icons/lock.svg"
                                } else {
                                    "icons/window/close.svg"
                                })
                                .text_color(rgb(palette.text_muted)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if is_locked {
                                this.notify_locked_tab_close_blocked(cx);
                            } else {
                                this.close_session(close_session_id.clone(), cx);
                            }
                        })),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.handle_session_tab_click(session_id.clone(), event, window, cx);
                }))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener({
                        let session_id = session.id.clone();
                        move |this, event, window, cx| {
                            this.handle_session_tab_mouse_down(
                                session_id.clone(),
                                event,
                                window,
                                cx,
                            );
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let session_id = session.id.clone();
                        move |this, event, window, cx| {
                            this.handle_session_tab_mouse_down(
                                session_id.clone(),
                                event,
                                window,
                                cx,
                            );
                        }
                    }),
                );
            tabs = tabs
                .child(NyaHoverCard::new(hover_card_id, tab, hover_card).anchor(Anchor::TopLeft));
        }
        while transient_cursor < transient_tabs.len() {
            let (_, _, _, request_id, name, error) = transient_tabs[transient_cursor].clone();
            let tab_number = session_count + transient_cursor + 1;
            let active = self.session.start_request_is_active(&request_id);
            tabs = tabs.child(match error {
                Some(error) => self
                    .failed_session_tab(request_id, name, error, tab_number, active, cx)
                    .into_any_element(),
                None => self
                    .pending_session_tab(request_id, name, tab_number, active, cx)
                    .into_any_element(),
            });
            transient_cursor += 1;
        }

        if session_count > 1 {
            tabs = tabs.child(
                div()
                    .id("session-tab-drop-end")
                    .h_full()
                    .min_w(px(SESSION_TAB_END_DROP_TARGET_MIN_WIDTH))
                    .flex_1()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .hover(move |this| this.bg(shell_hover_bg))
                    .drag_over::<SessionTabDragPayload>(move |this, payload, _, _| {
                        if payload.order_index + 1 >= session_count {
                            this
                        } else {
                            this.bg(shell_hover_bg)
                                .border_l_2()
                                .border_color(rgb(palette.focus_ring))
                        }
                    })
                    .on_drag_move(cx.listener(
                        |this, event: &gpui::DragMoveEvent<SessionTabDragPayload>, _, cx| {
                            this.update_session_tab_drag(event.drag(cx).session_id.clone(), cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, payload: &SessionTabDragPayload, _, cx| {
                        this.reorder_session_to_end(payload.session_id.clone(), cx);
                    })),
            );
        } else {
            tabs = tabs.child(
                div()
                    .h_full()
                    .flex_1()
                    .border_b_1()
                    .border_color(rgb(palette.border)),
            );
        }

        // Tauri TabBar trailing chrome: optional open-tabs overflow menu + new session menu.
        let open_tabs_menu = self.shell.open_tabs_menu_is_open();
        let new_session_anchor = NewSessionMenuAnchor::MainTabStrip;
        let new_session_menu = self.shell.new_session_menu_is_open_at(&new_session_anchor);
        let new_session_has_submenu = self.shell.new_session_all_sessions_is_open();
        let open_tabs_label = t!("terminal.openTabs").to_string();
        let new_session_label = t!("terminal.newSession").to_string();
        let tab_strip_has_overflow = self.shell.session_tab_strip_has_overflow();
        let show_open_tabs_menu = tab_strip_has_overflow || open_tabs_menu;

        let mut session_actions = div()
            .h_full()
            .flex()
            .items_center()
            .gap_0()
            .border_l_1()
            .border_b_1()
            .border_color(rgb(palette.border));

        if show_open_tabs_menu {
            let open_tabs_trigger = div()
                .id("workspace-open-tabs-menu")
                .size(px(SESSION_TAB_ACTION_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(rgb(palette.border))
                .bg(if open_tabs_menu {
                    self.shell_surface_color(palette.hover)
                } else {
                    rgba(0x00000000)
                })
                .text_color(rgb(palette.text_muted))
                .cursor_pointer()
                .hover(move |this| this.bg(shell_hover_bg).text_color(rgb(palette.text)))
                .child(
                    svg()
                        .size(px(16.))
                        .flex_none()
                        .path("icons/chevron-down.svg")
                        .text_color(rgb(palette.text_muted)),
                )
                .tooltip(move |window, cx| {
                    nyaterm_ui::NyaTooltip::new(open_tabs_label.clone()).build(window, cx)
                });
            let open_tabs_popover = NyaPopover::new(
                "workspace-open-tabs-popover",
                open_tabs_trigger,
                self.render_open_tabs_menu(cx),
            )
            .anchor(Anchor::TopRight)
            .appearance(false)
            .open(open_tabs_menu)
            .on_open_change(cx.listener(|this, open, _, cx| {
                if *open {
                    if !this.shell.open_tabs_menu_is_open() {
                        this.toggle_open_tabs_menu(cx);
                    }
                } else {
                    this.close_open_tabs_menu(cx);
                }
            }));
            session_actions = session_actions.child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(open_tabs_popover),
            );
        }

        let new_session_trigger = div()
            .id("workspace-new-session-menu")
            .size(px(SESSION_TAB_ACTION_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .border_r_1()
            .border_color(rgb(palette.border))
            .bg(if new_session_menu {
                self.shell_surface_color(palette.hover)
            } else {
                rgba(0x00000000)
            })
            .text_color(rgb(palette.text_muted))
            .cursor_pointer()
            .hover(move |this| this.bg(shell_hover_bg).text_color(rgb(palette.text)))
            .child(
                svg()
                    .size(px(16.))
                    .flex_none()
                    .path("icons/conn/add.svg")
                    .text_color(rgb(palette.text_muted)),
            )
            .tooltip(move |window, cx| {
                nyaterm_ui::NyaTooltip::new(new_session_label.clone()).build(window, cx)
            });
        let new_session_popover = NyaPopover::new(
            "workspace-new-session-popover",
            new_session_trigger,
            self.render_new_session_menu("main", cx),
        )
        .placement(NyaPopoverPlacement::Bottom)
        .align(NyaPopoverAlign::End)
        .offset(px(4.))
        .appearance(false)
        .overlay_closable(!new_session_has_submenu)
        .open(new_session_menu)
        .on_open_change(cx.listener(|this, open, _, cx| {
            let anchor = NewSessionMenuAnchor::MainTabStrip;
            if *open {
                this.open_new_session_menu(anchor, cx);
            } else if this.shell.new_session_menu_is_open_at(&anchor) {
                this.close_new_session_menu(cx);
            }
        }));
        session_actions = session_actions.child(
            div()
                .h_full()
                .flex()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .child(new_session_popover),
        );

        let tracked_app = cx.weak_entity();
        let tab_scroll_for_layout = tab_scroll.clone();
        let tab_strip_viewport = div()
            .h_full()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .on_children_prepainted(move |_, _, cx| {
                let viewport_width = tab_scroll_for_layout.bounds().size.width;
                if viewport_width <= px(0.) {
                    return;
                }
                let has_overflow = tab_strip_overflows(
                    f32::from(tab_scroll_for_layout.max_offset().x),
                    show_open_tabs_menu,
                    session_count > 1,
                );
                let Some(app) = tracked_app.upgrade() else {
                    return;
                };
                if app
                    .read(cx)
                    .shell
                    .session_tab_strip_layout_is_current(has_overflow, viewport_width)
                {
                    return;
                }
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        if this
                            .shell
                            .update_session_tab_strip_layout(has_overflow, viewport_width)
                        {
                            cx.notify();
                        }
                    });
                });
            })
            .child(tabs);

        div()
            .h(px(36.)) // Tauri TabBar: h-9
            .flex()
            .items_center()
            .bg(self.shell_surface_color(palette.surface))
            .child(tab_strip_viewport)
            .child(session_actions)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SessionTabOrderKey, TransientSessionTab, pending_tab_insert_index, tab_drop_insert_after,
        tab_scroll_x, tab_strip_overflows, transient_tab_precedes_session,
    };

    fn merged_tab_order(
        sessions: &[(&str, SessionTabOrderKey)],
        transient: &[(&str, SessionTabOrderKey)],
    ) -> Vec<String> {
        let mut transient_tabs = transient
            .iter()
            .map(|(name, (index, sequence))| {
                (
                    *index,
                    *sequence,
                    String::new(),
                    String::new(),
                    (*name).to_string(),
                    None,
                )
            })
            .collect::<Vec<TransientSessionTab>>();
        transient_tabs.sort_by_key(|tab| (tab.0, tab.1));

        let mut merged = Vec::new();
        let mut transient_cursor = 0;
        for (name, session_key) in sessions {
            while transient_cursor < transient_tabs.len()
                && transient_tab_precedes_session(&transient_tabs[transient_cursor], *session_key)
            {
                merged.push(transient_tabs[transient_cursor].4.clone());
                transient_cursor += 1;
            }
            merged.push((*name).to_string());
        }
        merged.extend(
            transient_tabs[transient_cursor..]
                .iter()
                .map(|tab| tab.4.clone()),
        );
        merged
    }

    #[test]
    fn pending_tab_position_matches_tauri_insertion_rules() {
        assert_eq!(pending_tab_insert_index(3, None, None), 3);
        assert_eq!(pending_tab_insert_index(3, Some(0), None), 1);
        assert_eq!(pending_tab_insert_index(3, Some(0), Some(2)), 2);
        assert_eq!(pending_tab_insert_index(3, None, Some(99)), 3);
    }

    #[test]
    fn tab_drop_uses_source_and_target_order_for_insertion_side() {
        assert_eq!(tab_drop_insert_after(2, 1), Some(false));
        assert_eq!(tab_drop_insert_after(1, 1), None);
        assert_eq!(tab_drop_insert_after(1, 2), Some(true));
    }

    #[test]
    fn tab_wheel_uses_dominant_axis_and_clamps_range() {
        assert_eq!(tab_scroll_x(-40., 120., 2., -30.), -70.);
        assert_eq!(tab_scroll_x(-40., 120., -50., 2.), -90.);
        assert_eq!(tab_scroll_x(-110., 120., 0., -40.), -120.);
        assert_eq!(tab_scroll_x(-10., 120., 0., 40.), 0.);
        assert_eq!(tab_scroll_x(-10., 0., 0., -40.), 0.);
    }

    #[test]
    fn overflow_detection_ignores_tolerance_and_the_rendered_control_width() {
        assert!(!tab_strip_overflows(2., false, false));
        assert!(tab_strip_overflows(2.1, false, false));

        assert!(!tab_strip_overflows(38., true, false));
        assert!(tab_strip_overflows(38.1, true, false));

        assert!(!tab_strip_overflows(66., true, true));
        assert!(tab_strip_overflows(66.1, true, true));
    }

    #[test]
    fn concurrent_session_tabs_merge_by_reserved_submission_order() {
        assert_eq!(
            merged_tab_order(&[("C", (2, 2))], &[("A", (0, 0)), ("B", (1, 1))]),
            ["A", "B", "C"]
        );
        assert_eq!(
            merged_tab_order(&[("B", (1, 1)), ("C", (2, 2))], &[("A", (0, 0))]),
            ["A", "B", "C"]
        );
        assert_eq!(
            merged_tab_order(&[("X", (0, u64::MAX)), ("B", (2, 1))], &[("A", (1, 0))],),
            ["X", "A", "B"]
        );
    }
}

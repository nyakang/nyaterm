use rust_i18n::t;
use std::time::Instant;

use super::tab_motion::{TabMotionElement, TabStripViewportElement};
use crate::features::shell::session_tabs::{SessionStripTab, SessionStripTabId};

use gpui::{
    Anchor, ClickEvent, Context, FontWeight, IntoElement, MouseButton, ScrollDelta,
    ScrollWheelEvent, SharedString, div, point, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaCheckbox, NyaHoverCard, NyaPopover, NyaPopoverAlign, NyaPopoverPlacement};

use crate::features::NyaTermApp;
use crate::features::icons::resolve_connection_icon;
use crate::features::shell::{
    NewSessionMenuAnchor, SessionTabDragPreview, SessionTabTooltip, WorkspaceTabDragPayload,
};
use crate::features::view_widgets::{connection_spinner, themed_icon};

use super::super::super::view_helpers::session_kind_icon_path;

const SESSION_TAB_MIN_WIDTH: f32 = 118.;
const SESSION_TAB_MAX_WIDTH: f32 = 236.;
const SESSION_TAB_ACTION_SIZE: f32 = 36.;
const SESSION_TAB_END_DROP_TARGET_MIN_WIDTH: f32 = 28.;
const TAB_STRIP_OVERFLOW_TOLERANCE: f32 = 2.;

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

/// A view boundary keeps animation frames inside the strip. The application
/// remains the owner of sessions; this view only adapts its presentation.
struct SessionTabStrip {
    app: gpui::WeakEntity<NyaTermApp>,
    group: Option<String>,
}

impl gpui::Render for SessionTabStrip {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view_id = cx.entity_id();
        self.app
            .update(cx, |app, cx| {
                app.render_session_tab_strip(self.group.clone(), view_id, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
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
        let action_target = self
            .session
            .start_request_target(&request_id)
            .expect("visible start has a placement");
        let middle_target = action_target.clone();
        let right_target = action_target.clone();
        let close_request_id = request_id.clone();
        let spinner_id = SharedString::from(format!("pending-session-spinner-{request_id}"));
        div()
            .id(SharedString::from(format!(
                "pending-session-tab-{request_id}"
            )))
            .debug_selector(|| format!("pending-session-tab-{request_id}"))
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
                        .debug_selector(|| format!("pending-session-tab-active-{request_id}"))
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
                    .debug_selector(|| format!("pending-session-tab-close-{close_request_id}"))
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .tooltip(|window, cx| {
                        nyaterm_ui::NyaTooltip::new(t!("common.close").to_string())
                            .build(window, cx)
                    })
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
            .on_click(cx.listener(move |this, event, window, cx| {
                this.handle_tab_target_click(action_target.clone(), event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event, window, cx| {
                    this.handle_tab_target_mouse_down(middle_target.clone(), event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.handle_tab_target_mouse_down(right_target.clone(), event, window, cx);
                }),
            )
    }

    fn failed_session_tab(
        &mut self,
        tab: SessionStripTab,
        tab_number: usize,
        active: bool,
        group: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let SessionStripTab {
            id: SessionStripTabId::Start(sequence),
            request_id: Some(request_id),
            name: failed_name,
            error: Some(failed_error),
        } = tab
        else {
            unreachable!("failed tab requires a failed connection attempt");
        };
        let palette = self.theme_palette();
        let action_target = self
            .session
            .start_request_target(&request_id)
            .expect("visible start has a placement");
        let middle_target = action_target.clone();
        let right_target = action_target.clone();
        let close_request_id = request_id.clone();
        let cancelled = self.session.start_request_cancelled(&request_id);
        let accent = if cancelled {
            palette.text_muted
        } else {
            palette.danger
        };
        let key = SessionStripTabId::Start(sequence).layout_key();
        let motion = self.shell.tab_strip_motion(group.clone());
        let width = motion.borrow().width(&key).unwrap_or(188.);
        let source = self.session.tab_drag_source_is(&key);
        let local_preview = motion.borrow().preview(&key).is_some();
        let payload = WorkspaceTabDragPayload {
            source_workspace_id: self.workspace_id,
            root_tab_id: key,
            source_revision: self.workspace_revision(),
            tab_id: SessionStripTabId::Start(sequence),
            order_index: tab_number - 1,
            source_group_id: group.clone(),
            display_name: failed_name.clone(),
            kind_icon: "icons/session/disconnect.svg",
            preview_background: palette.surface,
            preview_border: palette.border,
            preview_text: accent,
            preview_text_muted: palette.text_muted,
            preview_accent: accent,
        };
        let app = cx.weak_entity();
        let drop_request_id = request_id.clone();
        div()
            .id(SharedString::from(format!(
                "failed-session-tab-{request_id}"
            )))
            .debug_selector(|| format!("failed-session-tab-{request_id}"))
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
                rgba((accent << 8) | 0x24)
            } else {
                rgba((accent << 8) | 0x12)
            })
            .cursor_move()
            .when(source, |this| {
                this.opacity(if local_preview { 0. } else { 0.55 })
            })
            .on_drag(payload, move |payload, position, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.begin_session_tab_drag(payload, position, window, cx)
                });
                cx.new(|_| SessionTabDragPreview::new(payload.clone(), position).with_width(width))
            })
            .on_drop(
                cx.listener(move |app, payload: &WorkspaceTabDragPayload, _, cx| {
                    app.drop_on_session_start_tab(payload, &group, &drop_request_id, cx);
                }),
            )
            .hover(|this| this.bg(rgba((accent << 8) | 0x24)))
            .when(active, |this| {
                this.child(
                    div()
                        .debug_selector(|| format!("failed-session-tab-active-{request_id}"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(accent)),
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
                            .text_color(rgb(accent)),
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
                    .text_color(rgb(accent))
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
                    .debug_selector(|| format!("failed-session-tab-close-{close_request_id}"))
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .tooltip(|window, cx| {
                        nyaterm_ui::NyaTooltip::new(t!("common.close").to_string())
                            .build(window, cx)
                    })
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
                    .hover(|this| this.bg(rgb(palette.border)).text_color(rgb(accent)))
                    .child(
                        svg()
                            .size(px(13.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted))
                            .group_hover(
                                SharedString::from(format!(
                                    "failed-session-tab-close-group-{close_request_id}"
                                )),
                                |this| this.text_color(rgb(accent)),
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
            .on_click(cx.listener(move |this, event, window, cx| {
                this.handle_tab_target_click(action_target.clone(), event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event, window, cx| {
                    this.handle_tab_target_mouse_down(middle_target.clone(), event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.handle_tab_target_mouse_down(right_target.clone(), event, window, cx);
                }),
            )
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

    #[inline(never)]
    fn session_strip_tab(
        &mut self,
        session: nyaterm_transport::SessionInfo,
        tab_index: usize,
        tab_number: usize,
        group_active: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let display_name = self.session.display_name_by_info(&session);
        let session_id = session.id.clone();
        let close_session_id = session.id.clone();
        let tab_group_name = SharedString::from(format!("session-tab-group-{session_id}"));
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
        let motion = self
            .shell
            .tab_strip_motion(self.shell.terminal_group_for_tab(&session.id));
        let has_local_preview = motion.borrow().preview(&session.id).is_some();
        let preview_width = motion.borrow().width(&session.id).unwrap_or(188.);
        let custom_color = self.session.tab_color(&session.id);
        // Active when any leaf under this tab root is focused.
        let is_active = group_active == Some(session.id.as_str());
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
        let sync_paused = if self.sync_input.broadcast_to_all() {
            leaf_ids
                .iter()
                .any(|id| !self.sync_input.session_receives_input(id))
        } else {
            leaf_ids
                .iter()
                .any(|id| self.sync_input.session_is_paused_in_active_group(id))
        };
        let show_sync_indicator = self.sync_input.broadcast_to_all() || sync_group.is_some();
        let sync_checkbox = (self.sync_input.input_selection_active()
            && !matches!(
                session.kind,
                nyaterm_transport::SessionKind::Rdp | nyaterm_transport::SessionKind::Vnc
            ))
        .then(|| {
            let checked = leaf_ids
                .iter()
                .any(|id| self.sync_input.session_receives_input(id));
            (checked, leaf_ids.clone())
        });
        let sync_indicator_color = sync_group.map(|group| group.color).unwrap_or(palette.link);
        let sync_indicator_label = show_sync_indicator.then(|| {
            let scope = sync_group
                .map(|group| group.name.clone())
                .unwrap_or_else(|| t!("menu.broadcastToAll").to_string());
            if sync_paused {
                format!("{scope} · {}", t!("syncGroup.paused"))
            } else {
                scope
            }
        });
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
        // Build both the strip and its drag preview from the same resolved
        // icon, including uploaded images and full-color connection assets.
        let tab_icon = move || {
            if let Some(image) = custom_icon.as_ref() {
                gpui::img(image.clone()).size(px(14.)).into_any_element()
            } else if let Some(icon) = saved_icon {
                themed_icon(palette, icon, is_active, 14.)
            } else {
                svg()
                    .size(px(14.))
                    .path(kind_icon)
                    .text_color(accent)
                    .into_any_element()
            }
        };
        let drag_icon = tab_icon.clone();
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
        let drop_workspace_id = self.workspace_id;
        let target_group = self.shell.terminal_group_for_tab(&session.id);
        let hover_target_group = target_group.clone();
        let drag_payload = WorkspaceTabDragPayload {
            source_workspace_id: self.workspace_id,
            root_tab_id: session.id.clone(),
            source_revision: self.workspace_revision(),
            tab_id: SessionStripTabId::Workspace(crate::models::WorkspaceTabId::Session(
                session.id.clone(),
            )),
            order_index: tab_index,
            source_group_id: target_group.clone(),
            display_name: display_name.clone(),
            kind_icon,
            preview_background: palette.surface,
            preview_border: palette.border,
            preview_text: palette.text,
            preview_text_muted: palette.text_muted,
            preview_accent: accent_color,
        };
        let drag_source_app = cx.entity().downgrade();
        let hover_card = self.session_tab_hover_card(&session.id, palette);
        let hover_card_id = format!("session-tab-hover-card-{session_id}");
        let tab = div()
            .id(SharedString::from(format!("session-tab-{session_id}")))
            .debug_selector(|| format!("session-tab-{session_id}"))
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
            .when(is_drag_source, |this| {
                this.opacity(if has_local_preview { 0. } else { 0.55 })
            })
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .cursor_move()
            .on_drag(drag_payload, move |payload, position, window, cx| {
                let _ = drag_source_app.update(cx, |app, cx| {
                    app.begin_session_tab_drag(payload, position, window, cx);
                });
                cx.new(|_| {
                    SessionTabDragPreview::new(payload.clone(), position)
                        .with_width(preview_width)
                        .with_icon(drag_icon.clone())
                })
            })
            .drag_over::<WorkspaceTabDragPayload>(move |this, payload, _, _| {
                if payload.source_workspace_id == drop_workspace_id
                    && payload.source_group_id == hover_target_group
                {
                    return this;
                }
                if payload.source_workspace_id != drop_workspace_id {
                    return this
                        .bg(drag_target_bg)
                        .border_l_2()
                        .border_color(rgb(palette.focus_ring));
                }
                let Some(insert_after) = (if payload.source_group_id != hover_target_group {
                    Some(false)
                } else {
                    tab_drop_insert_after(payload.order_index, tab_index)
                }) else {
                    return this;
                };
                if payload.tab_id.layout_key() == drag_over_target_session_id {
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
                move |this, event: &gpui::DragMoveEvent<WorkspaceTabDragPayload>, _, cx| {
                    let payload = event.drag(cx);
                    this.update_session_tab_drag(payload.tab_id.layout_key(), cx);
                },
            ))
            .on_drop(
                cx.listener(move |this, payload: &WorkspaceTabDragPayload, _, cx| {
                    if payload.tab_id.is_start()
                        && (payload.source_workspace_id != this.workspace_id
                            || payload.source_group_id != target_group)
                    {
                        this.clear_session_tab_drag(cx);
                        return;
                    }
                    this.accept_session_tab_drop(payload, cx);
                    if this.commit_session_tab_preview(payload, &target_group, cx) {
                        return;
                    }
                    if payload.source_workspace_id != this.workspace_id {
                        this.request_tab_tree_move(
                            payload,
                            nyaterm_core::MoveTabPlacement::BeforeTab(
                                drop_target_session_id.clone(),
                            ),
                            cx,
                        );
                        return;
                    }
                    let Some(insert_after) = (if payload.source_group_id != target_group {
                        Some(false)
                    } else {
                        tab_drop_insert_after(payload.order_index, tab_index)
                    }) else {
                        this.clear_session_tab_drag(cx);
                        return;
                    };
                    this.reorder_session_relative(
                        payload.tab_id.layout_key(),
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
                        .debug_selector(|| format!("session-tab-active-{session_id}"))
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
                    .child(tab_icon()),
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
            .when_some(sync_indicator_label, |this, label| {
                this.child(
                    div()
                        .id(SharedString::from(format!(
                            "session-tab-sync-{close_session_id}"
                        )))
                        .tooltip(move |window, cx| {
                            nyaterm_ui::NyaTooltip::new(label.clone()).build(window, cx)
                        })
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
            .when_some(sync_checkbox, |this, (checked, session_ids)| {
                this.child(
                    div()
                        .id(SharedString::from(format!(
                            "session-tab-sync-receive-{close_session_id}"
                        )))
                        .flex_none()
                        .tooltip(|window, cx| {
                            nyaterm_ui::NyaTooltip::new(t!("syncGroup.receiveInput").to_string())
                                .build(window, cx)
                        })
                        .child(
                            NyaCheckbox::new(SharedString::from(format!(
                                "session-tab-sync-receive-box-{close_session_id}"
                            )))
                            .checked(checked)
                            .on_click(cx.listener(
                                move |this, receive: &bool, _, cx| {
                                    cx.stop_propagation();
                                    this.set_sessions_receive_sync_input(
                                        &session_ids,
                                        *receive,
                                        cx,
                                    );
                                },
                            )),
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
                    .tooltip(move |window, cx| {
                        let label = if is_locked {
                            t!("tabCtx.lockedCloseBlocked")
                        } else {
                            t!("common.close")
                        };
                        nyaterm_ui::NyaTooltip::new(label.to_string()).build(window, cx)
                    })
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
                        this.handle_session_tab_mouse_down(session_id.clone(), event, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let session_id = session.id.clone();
                    move |this, event, window, cx| {
                        this.handle_session_tab_mouse_down(session_id.clone(), event, window, cx);
                    }
                }),
            );
        NyaHoverCard::new(hover_card_id, tab, hover_card)
            .anchor(Anchor::TopLeft)
            .into_any_element()
    }

    pub(in crate::features) fn session_tab_strip(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.session_tab_strip_for_group(None, cx)
    }

    #[inline(never)]
    pub(in crate::features) fn session_tab_strip_for_group(
        &mut self,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let groups = self
            .shell
            .terminal_window_tree()
            .map(|root| root.leaf_ids())
            .unwrap_or_default();
        self.shell.prune_tab_strips(&groups);
        if let Some(view) = self.shell.tab_strip_view(&group_id) {
            return view.into_any_element();
        }
        let app = cx.weak_entity();
        let group = group_id.clone();
        let view: gpui::AnyView = cx.new(|_| SessionTabStrip { app, group }).into();
        self.shell.set_tab_strip_view(group_id, view.clone());
        view.into_any_element()
    }

    fn render_session_tab_strip(
        &mut self,
        group_id: Option<String>,
        strip_view_id: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let shell_hover_bg = self.shell_surface_color(palette.hover);
        let (tab_keys, _) = group_id
            .as_deref()
            .and_then(|group| self.shell.terminal_group_tabs(group))
            .unwrap_or_else(|| (self.workspace_tab_ids(), self.active_workspace_tab()));
        let selected = self.selected_session_strip_tab(&group_id);
        let group_active = selected.as_ref().and_then(|id| match id {
            SessionStripTabId::Workspace(id) => Some(id.layout_key()),
            SessionStripTabId::Start(_) => None,
        });
        let session_count = tab_keys.len();
        let tab_scroll = group_id
            .as_ref()
            .map(|id| self.shell.terminal_group_scroll(id))
            .unwrap_or_else(|| self.shell.session_tab_strip_scroll().clone());
        let strip_tabs = self.session_strip_tabs(&group_id);
        if self.shell.session_tab_scroll_into_view_pending()
            && (group_id.is_none() || self.current_terminal_group() == group_id)
        {
            if let Some(index) = strip_tabs
                .iter()
                .position(|tab| selected.as_ref() == Some(&tab.id))
            {
                tab_scroll.scroll_to_item(index);
            }
            self.shell.consume_session_tab_scroll_into_view();
        }
        let motion = self.shell.tab_strip_motion(group_id.clone());
        let motion_for_drag = motion.clone();
        let scroll_for_drag = tab_scroll.clone();
        let drag_group = group_id.clone();
        let drag_workspace = self.workspace_id;
        let mut motion_order = Vec::new();
        let tab_scroll_for_wheel = tab_scroll.clone();
        let mut tabs = div()
            .id(SharedString::from(format!(
                "session-tab-strip-scroll-{}",
                group_id.as_deref().unwrap_or("main")
            )))
            .h_full()
            .w_full()
            .flex()
            .items_center()
            // Keep tab overflow scrollable without painting a scrollbar over the tabs;
            // the wheel handler and open-tabs menu provide overflow navigation.
            .overflow_x_scroll()
            .overflow_y_hidden()
            .track_scroll(&tab_scroll)
            .on_drag_move(
                move |event: &gpui::DragMoveEvent<WorkspaceTabDragPayload>, _, cx| {
                    let payload = event.drag(cx);
                    let mut motion = motion_for_drag.borrow_mut();
                    let local = payload.source_workspace_id == drag_workspace
                        && payload.source_group_id == drag_group;
                    if local && event.bounds.contains(&event.event.position) {
                        motion.set_pointer(Some(f32::from(event.event.position.x)));
                        motion.update(
                            f32::from(event.event.position.x - scroll_for_drag.offset().x),
                            Instant::now(),
                        );
                    } else {
                        motion.suspend(Instant::now());
                    }
                    if local {
                        cx.notify(strip_view_id);
                    }
                },
            )
            .on_drop(cx.listener({
                let group = group_id.clone();
                move |this, payload: &WorkspaceTabDragPayload, _, cx| {
                    this.drop_session_tab_on_strip(payload, &group, cx);
                }
            }))
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

        let motion_sessions = strip_tabs.iter().map(|tab| tab.snapshot_key()).collect();
        for (index, tab) in strip_tabs.into_iter().enumerate() {
            let key = tab.id.layout_key();
            let active = selected.as_ref() == Some(&tab.id);
            let child = match tab.id {
                SessionStripTabId::Start(_) => {
                    if tab.error.is_some() {
                        self.failed_session_tab(tab, index + 1, active, group_id.clone(), cx)
                            .into_any_element()
                    } else {
                        let request_id = tab.request_id.expect("start tab must have a request");
                        self.pending_session_tab(request_id, tab.name, index + 1, active, cx)
                            .into_any_element()
                    }
                }
                SessionStripTabId::Workspace(crate::models::WorkspaceTabId::Editor(id)) => {
                    self.editor_strip_tab(id, index, index + 1, group_active.as_deref(), cx)
                }
                SessionStripTabId::Workspace(crate::models::WorkspaceTabId::FileManager(id)) => {
                    self.file_manager_strip_tab(id, index, index + 1, group_active.as_deref(), cx)
                }
                SessionStripTabId::Workspace(crate::models::WorkspaceTabId::Settings) => {
                    self.settings_strip_tab(index, index + 1, group_active.as_deref(), cx)
                }
                SessionStripTabId::Workspace(crate::models::WorkspaceTabId::Session(id)) => {
                    let Some(session) = self.session.session_info(&id) else {
                        continue;
                    };
                    self.session_strip_tab(session, index, index + 1, group_active.as_deref(), cx)
                }
            };
            motion_order.push(key.clone());
            tabs = tabs.child(TabMotionElement {
                id: key,
                motion: motion.clone(),
                scroll: tab_scroll.clone(),
                child,
            });
        }
        motion.borrow_mut().configure(motion_order, motion_sessions);

        let end_drop_group = group_id.clone();
        let hover_end_group = group_id.clone();
        if session_count > 0 {
            let drop_workspace_id = self.workspace_id;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!(
                        "session-tab-drop-end-{}",
                        group_id.as_deref().unwrap_or("main")
                    )))
                    .h_full()
                    .min_w(px(SESSION_TAB_END_DROP_TARGET_MIN_WIDTH))
                    .flex_1()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .hover(move |this| this.bg(shell_hover_bg))
                    .drag_over::<WorkspaceTabDragPayload>(move |this, payload, _, _| {
                        if payload.source_workspace_id == drop_workspace_id
                            && payload.source_group_id == hover_end_group
                        {
                            this
                        } else {
                            this.bg(shell_hover_bg)
                                .border_l_2()
                                .border_color(rgb(palette.focus_ring))
                        }
                    })
                    .on_drag_move(cx.listener(
                        |this, event: &gpui::DragMoveEvent<WorkspaceTabDragPayload>, _, cx| {
                            this.update_session_tab_drag(event.drag(cx).tab_id.layout_key(), cx);
                        },
                    ))
                    .on_drop(
                        cx.listener(move |this, payload: &WorkspaceTabDragPayload, _, cx| {
                            this.drop_session_tab_on_strip(payload, &end_drop_group, cx);
                        }),
                    ),
            );
        } else {
            let empty_drop_group = group_id.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!(
                        "session-tab-drop-empty-{}",
                        group_id.as_deref().unwrap_or("main")
                    )))
                    .h_full()
                    .flex_1()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .drag_over::<WorkspaceTabDragPayload>(move |this, _, _, _| {
                        this.bg(shell_hover_bg)
                            .border_l_2()
                            .border_color(rgb(palette.focus_ring))
                    })
                    .on_drop(
                        cx.listener(move |this, payload: &WorkspaceTabDragPayload, _, cx| {
                            this.drop_session_tab_on_strip(payload, &empty_drop_group, cx);
                        }),
                    ),
            );
        }

        // Tauri TabBar trailing chrome: optional open-tabs overflow menu + new session menu.
        let open_tabs_menu = self.shell.open_tabs_menu_is_open()
            && (group_id.is_none() || self.current_terminal_group() == group_id);
        let new_session_anchor = group_id
            .clone()
            .map(NewSessionMenuAnchor::TerminalLeaf)
            .unwrap_or(NewSessionMenuAnchor::MainTabStrip);
        let new_session_menu = self.shell.new_session_menu_is_open_at(&new_session_anchor);
        let new_session_has_submenu = self.shell.new_session_all_sessions_is_open();
        let open_tabs_label = t!("terminal.openTabs").to_string();
        let new_session_label = t!("terminal.newSession").to_string();
        let tab_strip_has_overflow = self.shell.session_tab_strip_has_overflow();
        let show_open_tabs_menu = if group_id.is_some() {
            session_count > 1 || open_tabs_menu
        } else {
            tab_strip_has_overflow || open_tabs_menu
        };

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
                .id(SharedString::from(format!(
                    "workspace-open-tabs-menu-{}",
                    group_id.as_deref().unwrap_or("main")
                )))
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
            let overflow_group = group_id.clone();
            let open_tabs_popover = NyaPopover::new(
                SharedString::from(format!(
                    "workspace-open-tabs-popover-{}",
                    group_id.as_deref().unwrap_or("main")
                )),
                open_tabs_trigger,
                if open_tabs_menu {
                    self.render_open_tabs_menu_for_group(group_id.as_deref(), cx)
                        .into_any_element()
                } else {
                    div().into_any_element()
                },
            )
            .anchor(Anchor::TopRight)
            .appearance(false)
            .open(open_tabs_menu)
            .on_open_change(cx.listener(move |this, open, window, cx| {
                if *open {
                    if let Some(group) = &overflow_group
                        && let Some((_, Some(tab))) = this.shell.terminal_group_tabs(group)
                    {
                        this.select_session(tab, cx);
                    }
                    if !this.shell.open_tabs_menu_is_open() {
                        this.toggle_open_tabs_menu(cx);
                    }
                } else if overflow_group.is_none()
                    || this.current_terminal_group() == overflow_group
                {
                    this.close_open_tabs_menu(cx);
                    this.focus_active_workspace_surface(window, cx);
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
            .id(SharedString::from(format!(
                "workspace-new-session-menu-{}",
                group_id.as_deref().unwrap_or("main")
            )))
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
            SharedString::from(format!(
                "workspace-new-session-popover-{}",
                group_id.as_deref().unwrap_or("main")
            )),
            new_session_trigger,
            if new_session_menu {
                self.render_new_session_menu(group_id.as_deref().unwrap_or("main"), cx)
                    .into_any_element()
            } else {
                div().into_any_element()
            },
        )
        .placement(NyaPopoverPlacement::Bottom)
        .align(NyaPopoverAlign::End)
        .offset(px(4.))
        .appearance(false)
        .overlay_closable(!new_session_has_submenu)
        .open(new_session_menu)
        .on_open_change(cx.listener(move |this, open, window, cx| {
            let anchor = new_session_anchor.clone();
            if *open {
                this.open_new_session_menu(anchor, cx);
            } else if this.shell.new_session_menu_is_open_at(&anchor) {
                this.close_new_session_menu(cx);
                this.focus_active_workspace_surface(window, cx);
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

        let measure_flat_strip = group_id.is_none();
        let tracked_app = cx.weak_entity();
        let tab_scroll_for_layout = tab_scroll.clone();
        let tab_strip_viewport = div()
            .h_full()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .on_children_prepainted(move |_, _, cx| {
                if !measure_flat_strip {
                    return;
                }
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
            .child(TabStripViewportElement {
                child: tabs.into_any_element(),
                motion,
                scroll: tab_scroll,
            });

        div()
            .h(px(36.)) // Tauri TabBar: h-9
            .flex()
            .items_center()
            .bg(self.shell_surface_color(palette.surface))
            .child(tab_strip_viewport)
            .child(session_actions)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{tab_drop_insert_after, tab_scroll_x, tab_strip_overflows};

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
}

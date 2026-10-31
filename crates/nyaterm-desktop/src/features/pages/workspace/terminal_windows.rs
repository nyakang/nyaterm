use crate::features::NyaTermApp;
use crate::features::shell::WorkspaceTabDragPayload;
use crate::models::{TabDockEdge, TabDockZone, TerminalWindowNode, WorkspaceSplitDirection};
use crate::theme::ThemePalette;
use gpui::{
    Context, FontWeight, IntoElement, SharedString, canvas, div, prelude::*, px, relative, rgb,
    rgba,
};
use rust_i18n::t;

impl NyaTermApp {
    // Keep recursion separate from chrome construction: in unoptimized Windows
    // builds a fluent tab builder can otherwise consume hundreds of KiB per level.
    #[inline(never)]
    pub(super) fn render_terminal_window_tree(
        &mut self,
        node: TerminalWindowNode,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match node {
            TerminalWindowNode::Leaf {
                id,
                active_tab_id,
                tab_ids,
            } => {
                let active = active_tab_id.or_else(|| tab_ids.first().cloned());
                self.render_terminal_group(id, active, cx)
            }
            TerminalWindowNode::Split {
                id,
                direction,
                ratio_percent,
                first,
                second,
            } => {
                let first = self.render_terminal_window_tree(*first, cx);
                let second = self.render_terminal_window_tree(*second, cx);
                self.render_terminal_group_split(id, direction, ratio_percent, first, second, cx)
            }
        }
    }

    #[inline(never)]
    fn render_terminal_group_split(
        &mut self,
        id: String,
        direction: WorkspaceSplitDirection,
        ratio: u8,
        first: gpui::AnyElement,
        second: gpui::AnyElement,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let app = cx.weak_entity();
        let measure_id = id.clone();
        let measurement = canvas(
            move |bounds, _, cx| {
                let app = app.clone();
                let id = measure_id.clone();
                cx.defer(move |cx| {
                    let _ = app.update(cx, |this, _| {
                        this.shell.record_terminal_split_bounds(id, bounds)
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let divider = self.workspace_split_resize_handle(id.clone(), direction, cx);
        div()
            .id(SharedString::from(format!("tw-split-{id}")))
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .relative()
            .when(direction == WorkspaceSplitDirection::Horizontal, |this| {
                this.flex_col()
            })
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .flex_grow(ratio as f32)
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(first),
            )
            .child(divider)
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .flex_grow((100 - ratio) as f32)
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(second),
            )
            .child(measurement)
            .into_any_element()
    }

    #[inline(never)]
    fn render_terminal_group(
        &mut self,
        id: String,
        active: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let strip = self.session_tab_strip_for_group(Some(id.clone()), cx);
        let content = self.terminal_group_content(&id, active, cx);
        let app = cx.weak_entity();
        let measure_id = id.clone();
        let measurement = canvas(
            move |bounds, _, cx| {
                let app = app.clone();
                let id = measure_id.clone();
                cx.defer(move |cx| {
                    let _ = app.update(cx, |this, _| {
                        this.shell.record_terminal_split_bounds(id, bounds)
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .child(measurement)
            .id(SharedString::from(format!("tw-leaf-{id}")))
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(strip)
            .child(content)
            .into_any_element()
    }

    #[inline(never)]
    fn terminal_group_content(
        &mut self,
        group: &str,
        active: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pending_here = self.current_terminal_group().as_deref() == Some(group);
        let showing_request = pending_here
            && (self.session.start_has_active_pending() || self.session.start_has_active_failed());
        let content = if pending_here && self.session.start_has_active_pending() {
            self.pending_workspace_state().into_any_element()
        } else if pending_here && self.session.start_has_active_failed() {
            self.failed_workspace_state(cx).into_any_element()
        } else if let Some(active) = &active {
            match crate::models::WorkspaceTabId::from_layout_key(active) {
                crate::models::WorkspaceTabId::Session(id) => {
                    self.workspace_tab_session_content(id, cx)
                }
                crate::models::WorkspaceTabId::Editor(id) => self.workspace_editor_content(&id, cx),
                crate::models::WorkspaceTabId::FileManager(id) => {
                    self.workspace_file_manager_content(&id, cx)
                }
                crate::models::WorkspaceTabId::Settings => {
                    self.settings_view(cx).into_any_element()
                }
            }
        } else {
            self.empty_workspace_state(cx).into_any_element()
        };
        let group_id = group.to_string();
        let move_id = group_id.clone();
        let drop_id = group_id.clone();
        let zone = self.shell.terminal_window_drop_for_leaf(group);
        let palette = self.theme_palette();
        let clicked_editor = active.clone().filter(|key| {
            !crate::models::WorkspaceTabId::from_layout_key(key).is_terminal_session()
        });
        div()
            .id(SharedString::from(format!("tw-content-{group}")))
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if !showing_request
                    && let Some(key) = &clicked_editor
                    && this.active_workspace_tab().as_ref() != Some(key)
                {
                    this.activate_workspace_tab(key, cx);
                }
            }))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .can_drop(|drag, _, _| drag.is::<WorkspaceTabDragPayload>())
            .on_drag_move(cx.listener(
                move |this, event: &gpui::DragMoveEvent<WorkspaceTabDragPayload>, _, cx| {
                    if event.drag(cx).tab_id.is_start() {
                        return;
                    }
                    let b = event.bounds;
                    let p = event.event.position;
                    // GPUI dispatches drag moves to every group, including those
                    // outside the pointer. Only clear this group's old target so
                    // a later sibling cannot erase the hovered group's preview.
                    if !b.contains(&p) {
                        if this.shell.clear_terminal_window_drop_for_leaf(&move_id) {
                            cx.notify();
                        }
                        return;
                    }
                    let mut zone = TabDockZone::detect(
                        f32::from(p.x - b.origin.x),
                        f32::from(p.y - b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height),
                    );
                    if let TabDockZone::Edge(edge) = zone
                        && !this.terminal_group_can_split(&move_id, edge.direction())
                    {
                        zone = TabDockZone::Center;
                    }
                    this.set_terminal_window_drop(move_id.clone(), zone, cx);
                },
            ))
            .on_drop(
                cx.listener(move |this, payload: &WorkspaceTabDragPayload, _, cx| {
                    if payload.tab_id.is_start() {
                        this.clear_session_tab_drag(cx);
                        return;
                    }
                    this.accept_session_tab_drop(payload, cx);
                    let zone = this
                        .shell
                        .terminal_window_drop_for_leaf(&drop_id)
                        .unwrap_or(TabDockZone::Center);
                    if payload.source_workspace_id == this.workspace_id {
                        this.dock_tab_on_terminal_window_leaf(
                            payload.tab_id.layout_key(),
                            drop_id.clone(),
                            zone,
                            cx,
                        );
                    } else {
                        let edge = match zone {
                            TabDockZone::Center => None,
                            TabDockZone::Edge(edge) => Some(match edge {
                                crate::models::TabDockEdge::Left => {
                                    nyaterm_core::MoveTabDockEdge::Left
                                }
                                crate::models::TabDockEdge::Right => {
                                    nyaterm_core::MoveTabDockEdge::Right
                                }
                                crate::models::TabDockEdge::Top => {
                                    nyaterm_core::MoveTabDockEdge::Top
                                }
                                crate::models::TabDockEdge::Bottom => {
                                    nyaterm_core::MoveTabDockEdge::Bottom
                                }
                            }),
                        };
                        this.request_tab_tree_move(
                            payload,
                            nyaterm_core::MoveTabPlacement::TerminalLeaf {
                                leaf_id: drop_id.clone(),
                                edge,
                            },
                            cx,
                        );
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !showing_request
                    && let Some(active) = &active
                    && let crate::models::WorkspaceTabId::Session(tab_session) =
                        crate::models::WorkspaceTabId::from_layout_key(active)
                {
                    // A click that landed inside a registry input (the tmux
                    // command box lives in this group's strip) must keep its
                    // caret, and a pane-tree leaf the canvas just activated
                    // keeps its session — refocusing the tab root would undo
                    // both before they take effect.
                    let leaf_focused = this.session.active_id().is_some_and(|id| {
                        this.shell
                            .workspace_pane_root(&tab_session)
                            .is_some_and(|root| root.contains_session(id))
                    });
                    if !leaf_focused && !this.text_input_focused(window, cx) {
                        this.activate_workspace_pane(active.clone(), cx);
                        this.focus_terminal_session(active, window, cx);
                    }
                }
            }))
            .child(content)
            .when_some(zone, |this, zone| {
                this.child(self.tab_dock_drop_overlay(zone, palette))
            })
            .into_any_element()
    }
}

impl NyaTermApp {
    fn tab_dock_drop_overlay(&self, zone: TabDockZone, palette: ThemePalette) -> impl IntoElement {
        let label = match zone {
            TabDockZone::Center => t!("tabActions.dockMerge"),
            TabDockZone::Edge(TabDockEdge::Left) => t!("tabActions.dockLeft"),
            TabDockZone::Edge(TabDockEdge::Right) => t!("tabActions.dockRight"),
            TabDockZone::Edge(TabDockEdge::Top) => t!("tabActions.dockTop"),
            TabDockZone::Edge(TabDockEdge::Bottom) => t!("tabActions.dockBottom"),
        };
        let accent = rgb(palette.link);
        let mut zone_box = div()
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_2()
            .border_color(accent)
            .bg(rgba((palette.link << 8) | 0x28));
        zone_box = match zone {
            TabDockZone::Center => zone_box.inset_2(),
            TabDockZone::Edge(TabDockEdge::Left) => {
                zone_box.top_2().bottom_2().left_2().w(relative(0.5))
            }
            TabDockZone::Edge(TabDockEdge::Right) => {
                zone_box.top_2().bottom_2().right_2().w(relative(0.5))
            }
            TabDockZone::Edge(TabDockEdge::Top) => {
                zone_box.left_2().right_2().top_2().h(relative(0.5))
            }
            TabDockZone::Edge(TabDockEdge::Bottom) => {
                zone_box.left_2().right_2().bottom_2().h(relative(0.5))
            }
        };
        div()
            .debug_selector(|| "tab-dock-drop-overlay".into())
            .absolute()
            .inset_0()
            .child(
                zone_box.child(
                    div()
                        .rounded_sm()
                        .border_1()
                        .border_color(accent)
                        .bg(self.shell_surface_color(palette.surface))
                        .px_3()
                        .py_1()
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(label),
                ),
            )
    }
}

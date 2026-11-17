//! The panel owns one drag session. Immutable snapshots remain the catalog authority.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{Bounds, Context, Pixels, Point, ScrollHandle, px};
use nyaterm_core::connection_tree::{
    Destination, DropProjection, MoveRequest, Node, NodeId, NodeKind, VisibleNode, plan_move,
    project_drop,
};

use super::list::ConnectionListRow;
use super::panel::ConnectionPanel;
use crate::features::connections::state::list_model::{
    apply_mixed_order, connection_sections, flatten_connection_rows,
};
use crate::features::connections::{ConnectionDragKind, ConnectionDragPayload};
use crate::models::ConnectionSortMode;

const ROW: f32 = 34.;

#[derive(Default)]
pub(super) struct TreeDragState {
    active: Option<Session>,
    pub busy: bool,
    ticking: bool,
    pending_opened: HashSet<String>,
    pending_parent: Option<String>,
    pending_rows: Option<Vec<ConnectionListRow>>,
    shifts: HashMap<NodeId, f32>,
    shifted_at: Option<Instant>,
}

struct Session {
    source: NodeId,
    expected: Vec<Node>,
    excluded: HashSet<NodeId>,
    start_x: Pixels,
    initial_depth: usize,
    pointer: Point<Pixels>,
    bounds: Bounds<Pixels>,
    scroll: ScrollHandle,
    projection: Option<DropProjection>,
    hover: Option<(String, Instant)>,
    opened: HashSet<String>,
    reorder: bool,
}

#[derive(Clone)]
pub(super) enum DragRow {
    Item(ConnectionListRow),
    Slot { depth: usize, label: String },
    Root,
}

impl TreeDragState {
    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    pub fn has_preview(&self) -> bool {
        self.active() || self.pending_rows.is_some()
    }
}

impl ConnectionPanel {
    pub(in crate::features) fn drag_preview_label(&self) -> Option<String> {
        self.tree_drag
            .active
            .as_ref()
            .and_then(|drag| drag.projection.as_ref())
            .map(|_| self.drop_label())
    }

    pub(super) fn root_receives(&self) -> bool {
        self.tree_drag
            .active
            .as_ref()
            .and_then(|drag| drag.projection.as_ref())
            .is_some_and(|p| p.inside && p.destination.parent.is_none())
    }
    #[cfg(test)]
    pub(super) fn tree_projection(&self) -> Option<&DropProjection> {
        self.tree_drag
            .active
            .as_ref()
            .and_then(|drag| drag.projection.as_ref())
    }

    #[cfg(test)]
    pub(super) fn tree_scroll_offset(&self) -> Option<Point<Pixels>> {
        self.tree_drag
            .active
            .as_ref()
            .map(|drag| drag.scroll.offset())
    }

    pub(super) fn drag_row_shift(&self, row: &ConnectionListRow) -> f32 {
        let t = self
            .tree_drag
            .shifted_at
            .map_or(1., |at| (at.elapsed().as_secs_f32() / 0.12).min(1.));
        row.tree_key()
            .and_then(|key| self.tree_drag.shifts.get(&key))
            .copied()
            .unwrap_or(0.)
            * (1. - t).powi(3)
    }

    pub(in crate::features) fn finish_tree_move(&mut self, success: bool, cx: &mut Context<Self>) {
        self.tree_drag.busy = false;
        self.tree_drag.pending_rows = None;
        let opened = std::mem::take(&mut self.tree_drag.pending_opened);
        let mut keep = HashSet::new();
        if success && let Some(snapshot) = self.snapshot() {
            let mut parent = self.tree_drag.pending_parent.as_deref();
            while let Some(id) = parent {
                if !keep.insert(id.to_owned()) {
                    break;
                }
                parent = snapshot
                    .tree_nodes
                    .iter()
                    .find(|n| n.key.kind == NodeKind::Group && n.key.id == id)
                    .and_then(|n| n.parent.as_deref());
            }
        }
        self.tree_drag.pending_parent = None;
        if !opened.is_empty() {
            self.with_app(cx, |app, cx| {
                for id in opened {
                    if !keep.contains(&id)
                        && app.connection_state.list_expanded_group_ids().contains(&id)
                    {
                        app.connection_state.toggle_list_group_expanded(id);
                    }
                }
                cx.notify();
            });
        }
        cx.notify();
    }
    fn drag_items(&self) -> Vec<ConnectionListRow> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let Some(drag) = &self.tree_drag.active else {
            return snapshot.rows.to_vec();
        };
        snapshot
            .rows
            .iter()
            .filter(|row| {
                row.tree_key()
                    .is_some_and(|key| !drag.excluded.contains(&key))
            })
            .cloned()
            .collect()
    }

    pub(super) fn drag_rows(&self) -> Vec<DragRow> {
        if let Some(rows) = &self.tree_drag.pending_rows {
            return rows.iter().cloned().map(DragRow::Item).collect();
        }
        let mut rows: Vec<_> = self.drag_items().into_iter().map(DragRow::Item).collect();
        if let Some(drag) = &self.tree_drag.active {
            if drag.reorder
                && let Some(projection) = &drag.projection
            {
                rows.insert(
                    projection.index.min(rows.len()),
                    DragRow::Slot {
                        depth: projection.depth,
                        label: self.drop_label(),
                    },
                );
            }
            rows.push(DragRow::Root);
        }
        rows
    }

    pub(super) fn drop_label(&self) -> String {
        let Some(drag) = &self.tree_drag.active else {
            return String::new();
        };
        let mut parent = drag
            .projection
            .as_ref()
            .and_then(|p| p.destination.parent.as_deref());
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        while let Some(id) = parent {
            if !seen.insert(id) {
                break;
            }
            let Some(node) = drag
                .expected
                .iter()
                .find(|n| n.key.kind == NodeKind::Group && n.key.id == id)
            else {
                break;
            };
            names.push(node.name.as_str());
            parent = node.parent.as_deref();
        }
        names.reverse();
        if names.is_empty() {
            rust_i18n::t!("savedConnections.dropRoot").to_string()
        } else {
            names.join(" / ")
        }
    }

    pub(super) fn drag_receives(&self, row: &ConnectionListRow) -> bool {
        let Some(drag) = &self.tree_drag.active else {
            return false;
        };
        let Some(parent) = drag
            .projection
            .as_ref()
            .and_then(|p| p.destination.parent.as_deref())
        else {
            return false;
        };
        let Some(key) = row.tree_key() else {
            return false;
        };
        let mut key = Some(key);
        let mut seen = HashSet::new();
        while let Some(current) = key {
            if !seen.insert(current.clone()) {
                break;
            }
            if current.kind == NodeKind::Group && current.id == parent {
                return true;
            }
            key = drag
                .expected
                .iter()
                .find(|n| n.key == current)
                .and_then(|n| n.parent.as_ref())
                .map(|id| NodeId {
                    kind: NodeKind::Group,
                    id: id.clone(),
                });
        }
        false
    }

    pub(super) fn arm_tree_drag(
        &mut self,
        payload: &ConnectionDragPayload,
        pointer: Point<Pixels>,
        bounds: Bounds<Pixels>,
        scroll: ScrollHandle,
        cx: &mut Context<Self>,
    ) {
        if self.tree_drag.busy {
            return;
        }
        if self.tree_drag.active.is_none() {
            if !bounds.contains(&pointer) {
                return;
            }
            let Some(snapshot) = self.snapshot() else {
                return;
            };
            if snapshot.group_editor.is_some() {
                return;
            }
            let source = NodeId {
                kind: match payload.kind {
                    ConnectionDragKind::Connection => NodeKind::Connection,
                    ConnectionDragKind::Group => NodeKind::Group,
                },
                id: payload.id.clone(),
            };
            let Some(source_row) = snapshot
                .rows
                .iter()
                .find(|r| r.tree_key().as_ref() == Some(&source))
            else {
                return;
            };
            let Some(source_node) = snapshot.tree_nodes.iter().find(|n| n.key == source) else {
                return;
            };
            let siblings = nyaterm_core::connection_tree::children(
                &snapshot.tree_nodes,
                source_node.parent.as_deref(),
            );
            let next = siblings
                .iter()
                .position(|n| n.key == source)
                .and_then(|i| siblings.get(i + 1))
                .map(|n| n.key.clone());
            let initial = DropProjection {
                destination: Destination {
                    parent: source_node.parent.clone(),
                    before: next,
                },
                index: snapshot
                    .rows
                    .iter()
                    .filter_map(ConnectionListRow::tree_key)
                    .take_while(|key| key != &source)
                    .count(),
                depth: source_row.depth(),
                inside: false,
            };
            let mut excluded = HashSet::from([source.clone()]);
            loop {
                let before = excluded.len();
                for node in &snapshot.tree_nodes {
                    if node.parent.as_ref().is_some_and(|id| {
                        excluded.contains(&NodeId {
                            kind: NodeKind::Group,
                            id: id.clone(),
                        })
                    }) {
                        excluded.insert(node.key.clone());
                    }
                }
                if excluded.len() == before {
                    break;
                }
            }
            self.tree_drag.active = Some(Session {
                source,
                expected: snapshot.tree_nodes.clone(),
                excluded,
                start_x: pointer.x,
                initial_depth: source_row.depth(),
                pointer,
                bounds,
                scroll: scroll.clone(),
                projection: (snapshot.search_is_empty
                    && snapshot.sort_mode == ConnectionSortMode::Default)
                    .then_some(initial),
                hover: None,
                opened: HashSet::new(),
                reorder: snapshot.search_is_empty
                    && snapshot.sort_mode == ConnectionSortMode::Default,
            });
        }
        self.start_tree_drag_clock(cx);
        cx.notify();
    }

    pub(super) fn move_tree_drag(
        &mut self,
        payload: &ConnectionDragPayload,
        pointer: Point<Pixels>,
        bounds: Bounds<Pixels>,
        scroll: ScrollHandle,
        cx: &mut Context<Self>,
    ) {
        if self.tree_drag.busy {
            return;
        }
        if self.tree_drag.active.is_none() {
            self.arm_tree_drag(payload, pointer, bounds, scroll.clone(), cx);
        }
        if let Some(drag) = &mut self.tree_drag.active {
            drag.pointer = pointer;
            drag.bounds = bounds;
            drag.scroll = scroll;
        }
        self.project_tree_drag();
        self.start_tree_drag_clock(cx);
        cx.notify();
    }

    fn project_tree_drag(&mut self) {
        let old_positions: HashMap<_, _> = self
            .drag_rows()
            .iter()
            .enumerate()
            .filter_map(|(index, row)| match row {
                DragRow::Item(row) => row
                    .tree_key()
                    .map(|key| (key, index as f32 * ROW + self.drag_row_shift(row))),
                _ => None,
            })
            .collect();
        let rows = self.drag_items();
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let current = snapshot.tree_nodes.clone();
        let reorder = snapshot.search_is_empty && snapshot.sort_mode == ConnectionSortMode::Default;
        let editing = snapshot.group_editor.is_some();
        let Some(drag) = &mut self.tree_drag.active else {
            return;
        };
        if current != drag.expected
            || reorder != drag.reorder
            || editing
            || !drag.bounds.contains(&drag.pointer)
        {
            drag.projection = None;
            drag.hover = None;
            return;
        }
        let visible: Vec<_> = rows
            .iter()
            .filter_map(|r| {
                r.tree_key().map(|key| VisibleNode {
                    key,
                    depth: r.depth(),
                })
            })
            .collect();
        let y = f32::from(drag.pointer.y - drag.bounds.top() - drag.scroll.offset().y) / ROW;
        let desired = (drag.initial_depth as i32
            + (f32::from(drag.pointer.x - drag.start_x) / 16.).round() as i32)
            .max(0) as usize;
        let root_index = visible.len() + usize::from(drag.reorder && drag.projection.is_some());
        let projection = if y >= root_index as f32 {
            Some(DropProjection {
                destination: Destination {
                    parent: None,
                    before: None,
                },
                index: visible.len(),
                depth: 0,
                inside: true,
            })
        } else {
            project_drop(
                &drag.expected,
                &visible,
                y,
                desired,
                drag.projection.as_ref(),
                drag.reorder,
            )
        };
        let previous = drag.projection.clone();
        drag.projection = projection.filter(|p| {
            if previous.as_ref() == Some(p) {
                return true;
            }
            let request = MoveRequest {
                source: drag.source.clone(),
                destination: p.destination.clone(),
                expected: drag.expected.clone(),
            };
            plan_move(&drag.expected, &request).is_ok()
        });
        let hover = drag
            .projection
            .as_ref()
            .filter(|p| p.inside)
            .and_then(|p| p.destination.parent.clone());
        if drag.hover.as_ref().map(|h| &h.0) != hover.as_ref() {
            drag.hover = hover.map(|id| (id, Instant::now()));
        }
        if previous != drag.projection {
            self.tree_drag.shifts = self
                .drag_rows()
                .iter()
                .enumerate()
                .filter_map(|(index, row)| match row {
                    DragRow::Item(row) => row.tree_key().and_then(|key| {
                        old_positions
                            .get(&key)
                            .map(|old| (key, old - index as f32 * ROW))
                    }),
                    _ => None,
                })
                .collect();
            self.tree_drag.shifted_at = Some(Instant::now());
        }
    }

    fn start_tree_drag_clock(&mut self, cx: &mut Context<Self>) {
        if self.tree_drag.ticking {
            return;
        }
        self.tree_drag.ticking = true;
        cx.spawn(async move |panel, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(keep) = panel.update(cx, |panel, cx| {
                    if !cx.has_active_drag() {
                        panel.cancel_tree_drag(cx);
                    }
                    if panel.tree_drag.active.is_none() {
                        panel.tree_drag.ticking = false;
                        return false;
                    }
                    panel.tick_tree_drag(cx);
                    true
                }) else {
                    break;
                };
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn tick_tree_drag(&mut self, cx: &mut Context<Self>) {
        let expanded = self
            .snapshot()
            .map(|s| s.expanded_groups.clone())
            .unwrap_or_default();
        let Some(drag) = &mut self.tree_drag.active else {
            return;
        };
        let mut open = None;
        if let Some((id, started)) = &drag.hover
            && started.elapsed() >= Duration::from_millis(450)
            && !expanded.contains(id)
        {
            drag.opened.insert(id.clone());
            open = Some(id.clone());
        }
        if drag.bounds.contains(&drag.pointer) {
            let from_top = f32::from(drag.pointer.y - drag.bounds.top());
            let from_bottom = f32::from(drag.bounds.bottom() - drag.pointer.y);
            let speed = if from_top < 24. {
                (24. - from_top) / 24.
            } else if from_bottom < 24. {
                -(24. - from_bottom) / 24.
            } else {
                0.
            };
            if speed != 0. {
                let mut offset = drag.scroll.offset();
                offset.y = (offset.y + px(speed * 400. * 0.016))
                    .clamp(-drag.scroll.max_offset().y, px(0.));
                drag.scroll.set_offset(offset);
            }
        }
        if let Some(id) = open {
            self.with_app(cx, |app, cx| {
                app.connection_state.expand_list_group(id);
                cx.notify();
            });
        }
        self.project_tree_drag();
        cx.notify();
    }

    pub(super) fn cancel_tree_drag(&mut self, cx: &mut Context<Self>) {
        self.tree_drag.shifts.clear();
        if let Some(drag) = self.tree_drag.active.take() {
            self.with_app(cx, |app, cx| {
                for id in drag.opened {
                    if app.connection_state.list_expanded_group_ids().contains(&id) {
                        app.connection_state.toggle_list_group_expanded(id);
                    }
                }
                cx.notify();
            });
            cx.notify();
        }
    }

    pub(super) fn release_tree_drag(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.tree_drag.active {
            drag.pointer = pointer;
        }
        self.project_tree_drag();
        let Some(drag) = &self.tree_drag.active else {
            return;
        };
        let Some(projection) = &drag.projection else {
            self.cancel_tree_drag(cx);
            return;
        };
        let request = MoveRequest {
            source: drag.source.clone(),
            destination: projection.destination.clone(),
            expected: drag.expected.clone(),
        };
        let Ok(changes) = plan_move(&request.expected, &request) else {
            self.cancel_tree_drag(cx);
            return;
        };
        if changes.is_empty() {
            self.cancel_tree_drag(cx);
            return;
        }
        // Project through the normal list builder so subtree depths, empty groups,
        // search and name sorting agree with the eventual persisted snapshot.
        let snapshot = self.snapshot().expect("drag requires a snapshot");
        let mut nodes = request.expected.clone();
        for change in changes {
            if let Some(node) = nodes.iter_mut().find(|node| node.key == change.key) {
                *node = change;
            }
        }
        let groups: Vec<_> = nodes
            .iter()
            .filter(|n| n.key.kind == NodeKind::Group)
            .map(|n| nyaterm_core::Group {
                id: n.key.id.clone(),
                name: n.name.clone(),
                parent_id: n.parent.clone(),
                sort_order: n.sort_order,
                tree_order: n.tree_order,
                ..Default::default()
            })
            .collect();
        let connections: Vec<_> = nodes
            .iter()
            .filter(|n| n.key.kind == NodeKind::Connection)
            .filter_map(|n| {
                let mut connection = snapshot.connection(&n.key.id)?.clone();
                connection.group_id = n.parent.clone();
                connection.sort_order = n.sort_order;
                connection.tree_order = n.tree_order;
                Some(connection)
            })
            .collect();
        let query = snapshot.search_field.read(cx).value(cx);
        let sections = connection_sections(&connections, &groups, &query, snapshot.sort_mode);
        let mut expanded = snapshot.expanded_groups.as_ref().clone();
        if let Some(parent) = &request.destination.parent {
            expanded.insert(parent.clone());
        }
        let rows = flatten_connection_rows(&sections, &expanded, None);
        self.tree_drag.pending_rows = Some(if drag.reorder {
            apply_mixed_order(rows, &nodes)
        } else {
            rows
        });
        self.tree_drag.pending_parent = request.destination.parent.clone();
        self.tree_drag.shifts.clear();
        if let Some(drag) = self.tree_drag.active.take() {
            self.tree_drag.pending_opened = drag.opened;
        }
        self.tree_drag.busy = true;
        let panel = cx.weak_entity();
        self.with_app(cx, |app, cx| {
            app.submit_connection_tree_move(request, panel, cx)
        });
        cx.notify();
    }
}

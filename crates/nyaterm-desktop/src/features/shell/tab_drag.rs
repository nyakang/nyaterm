use gpui::{
    AnyElement, Context, IntoElement as _, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, Point, PointerCancelEvent, Styled as _, Window, canvas,
};

use super::tab_mouse::WorkspaceTabDragPayload;
use crate::app_shell::tab_drag::{TabDragSource, release_at, screen_bounds};
use crate::features::NyaTermApp;

#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features) fn drop_session_tab_on_strip(
        &mut self,
        payload: &WorkspaceTabDragPayload,
        group: &Option<String>,
        cx: &mut Context<Self>,
    ) {
        if payload.tab_id.is_start()
            && (payload.source_workspace_id != self.workspace_id
                || &payload.source_group_id != group)
        {
            self.clear_session_tab_drag(cx);
            return;
        }
        self.accept_session_tab_drop(payload, cx);
        if self.commit_session_tab_preview(payload, group, cx) {
            return;
        }
        if payload.source_workspace_id == self.workspace_id {
            if let Some(group) = group {
                self.dock_tab_on_terminal_window_leaf(
                    payload.tab_id.layout_key(),
                    group.clone(),
                    crate::models::TabDockZone::Center,
                    cx,
                );
            } else {
                self.reorder_session_to_end(payload.tab_id.layout_key(), cx);
            }
        } else {
            self.request_tab_tree_move(
                payload,
                group
                    .clone()
                    .map(|leaf_id| nyaterm_core::MoveTabPlacement::TerminalLeaf {
                        leaf_id,
                        edge: None,
                    })
                    .unwrap_or(nyaterm_core::MoveTabPlacement::Append),
                cx,
            );
        }
    }

    pub(in crate::features) fn commit_session_tab_preview(
        &mut self,
        payload: &WorkspaceTabDragPayload,
        group: &Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if payload.source_workspace_id != self.workspace_id || &payload.source_group_id != group {
            return false;
        }
        let motion = self.shell.tab_strip_motion(group.clone());
        let preview = motion
            .borrow()
            .preview(&payload.tab_id.layout_key())
            .map(<[String]>::to_vec);
        let Some(preview) = preview else {
            // Membership changed or measurement was incomplete: a stale
            // snapshot must not be converted into a different reorder.
            self.clear_session_tab_drag(cx);
            return true;
        };
        let tabs = self.session_strip_tabs(group);
        let live = tabs
            .iter()
            .map(|tab| tab.id.layout_key())
            .collect::<Vec<_>>();
        let snapshot = tabs
            .iter()
            .map(|tab| tab.snapshot_key())
            .collect::<Vec<_>>();
        if !motion.borrow().matches_sessions(&snapshot) {
            self.clear_session_tab_drag(cx);
            return true;
        }
        let Some(_index) = preview
            .iter()
            .position(|id| id == &payload.tab_id.layout_key())
        else {
            self.clear_session_tab_drag(cx);
            return true;
        };
        if preview.len() != live.len() {
            self.clear_session_tab_drag(cx);
            return true;
        }
        if preview == live {
            self.clear_session_tab_drag(cx);
            return true;
        }
        motion.borrow_mut().commit(std::time::Instant::now());
        self.commit_session_strip_order(group, &preview, &payload.tab_id.layout_key(), cx);
        true
    }

    pub(in crate::features) fn session_tab_drag_listener_layer(
        &self,
        cx: &Context<Self>,
    ) -> AnyElement {
        let app = cx.entity().downgrade();
        // Window-level listeners must be registered during paint, not render.
        // The canvas has no hitbox and contributes no pixels or flow layout.
        canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.install_session_tab_drag_listeners(window, cx)
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
        .into_any_element()
    }

    pub(in crate::features) fn begin_session_tab_drag(
        &mut self,
        payload: &WorkspaceTabDragPayload,
        grab_offset: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.shell
            .tab_strip_motion(payload.source_group_id.clone())
            .borrow_mut()
            .begin(&payload.tab_id.layout_key(), f32::from(grab_offset.x));
        if !matches!(
            payload.tab_id,
            super::session_tabs::SessionStripTabId::Workspace(
                crate::models::WorkspaceTabId::Session(_)
            )
        ) {
            self.update_session_tab_drag(payload.tab_id.layout_key(), cx);
            return;
        }
        // The new workspace has one strip tab. Include the strip's offset in
        // the window so its first tab, rather than the window corner, follows
        // the grab point (including the sidebar and title-bar space).
        let grab_offset = self.shell.session_tab_strip_scroll().bounds().origin + grab_offset;
        if let Some(controller) = &self.desktop_controller {
            let _ = controller.update(cx, |controller, _| {
                controller.update_tab_drag(|drag| {
                    drag.begin(TabDragSource {
                        workspace_id: payload.source_workspace_id,
                        root_tab_id: payload.root_tab_id.clone(),
                        revision: payload.source_revision,
                        grab_offset,
                        screen_bounds: screen_bounds(window),
                        normal_size: drag.normal_size(payload.source_workspace_id),
                    });
                });
            });
        }
        self.update_session_tab_drag(payload.tab_id.layout_key(), cx);
    }

    pub(in crate::features) fn accept_session_tab_drop(
        &self,
        payload: &WorkspaceTabDragPayload,
        cx: &mut Context<Self>,
    ) {
        if payload.tab_id.is_start() {
            return;
        }
        if let Some(controller) = &self.desktop_controller {
            let _ = controller.update(cx, |controller, _| {
                controller.update_tab_drag(|drag| {
                    drag.accept(
                        payload.source_workspace_id,
                        &payload.root_tab_id,
                        payload.source_revision,
                    );
                });
            });
        }
    }

    pub(in crate::features) fn cancel_internal_drag_on_escape(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "escape" || !cx.has_internal_drag() {
            return false;
        }
        cx.stop_active_drag(window);
        self.cancel_pointer_interactions(window, cx);
        cx.refresh_windows();
        cx.stop_propagation();
        true
    }

    pub(in crate::features) fn install_session_tab_drag_listeners(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        window.on_mouse_event(move |_: &PointerCancelEvent, phase, window, cx| {
            if phase.capture() {
                let _ = app.update(cx, |app, cx| app.cancel_pointer_interactions(window, cx));
            }
        });
        let Some(controller) = self.desktop_controller.clone() else {
            return;
        };
        let workspace_id = self.workspace_id;
        if !window.is_maximized() && !window.is_fullscreen() {
            let _ = controller.update(cx, |controller, _| {
                controller.remember_tab_window_size(workspace_id, window.viewport_size());
            });
        }
        let release_controller = controller.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if !phase.capture() || event.button != MouseButton::Left || !cx.has_active_drag() {
                return;
            }
            let id = release_controller.update(cx, |controller, _| {
                let source = controller.tab_drag_source()?;
                let release = release_at(source, workspace_id, window, event.position);
                controller.update_tab_drag(|drag| drag.released(release))
            });
            if let Ok(Some(id)) = id {
                let controller = release_controller.clone();
                // Drop handlers run in the bubble phase. GPUI then clears its
                // generic drag before this callback evaluates our candidate.
                cx.defer(move |cx| {
                    let _ = controller.update(cx, |controller, cx| {
                        controller.finish_tab_drag_release(id, cx)
                    });
                });
            }
        });
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
            if phase.capture() {
                // An interrupted drag must not turn a subsequent, unrelated
                // drag (files, connections, selection) into a tab detach.
                let _ = controller.update(cx, |controller, _| {
                    controller.update_tab_drag(|drag| drag.cancel())
                });
            }
        });
    }
}

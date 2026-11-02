use gpui::{Context, IntoElement, div, prelude::*};
use rust_i18n::t;

use super::super::NyaTermApp;
use crate::models::WorkspacePaneNode;

#[cfg(test)]
mod group_tests;
mod panes;
#[cfg(test)]
mod tab_docking_tests;
mod terminal_windows;

impl NyaTermApp {
    pub(in crate::features) fn workspace_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // Match the Tauri shell: tab strip sits directly above the terminal surface.
        // Do not reconcile/prune layout here — paint must stay pure. Session
        // register/close/idle already keep terminal_windows coherent.
        let multi_leaf = self.shell.terminal_window_tree_is_some();
        let has_connect_failure = self.session.start_has_failed()
            || (self.shell.last_connect_failure_name().is_some()
                && self.shell.last_connect_failure_error().is_some());
        let show_tab_strip = !multi_leaf
            && (self.ordered_tab_session_count() > 0
                || self.session.start_has_pending()
                || has_connect_failure);
        let app = cx.weak_entity();
        let measurement = gpui::canvas(
            move |bounds, _, cx| {
                let app = app.clone();
                cx.defer(move |cx| {
                    let _ = app.update(cx, |this, _| {
                        this.shell.record_terminal_workspace_bounds(bounds)
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let mut workspace = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(self.shell_transparent_color(palette.bg));
        if self.shell.pane_focus_mode()
            && self
                .session
                .active_id()
                .is_some_and(|id| !self.shell.session_tab_is_hidden(id))
        {
            workspace = workspace.child(
                div()
                    .h(gpui::px(30.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_3()
                    .child(
                        div()
                            .id("exit-pane-focus")
                            .cursor_pointer()
                            .text_xs()
                            .child(t!("settings.shortcutLabels.togglePaneFocus"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shell.set_pane_focus_mode(false);
                                this.sync_terminal_frame_snapshot_priority();
                                cx.notify();
                            })),
                    ),
            );
            return workspace.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .child(measurement)
                    .child(self.workspace_terminal_area(cx)),
            );
        }
        if show_tab_strip {
            workspace = workspace.child(self.session_tab_strip(cx));
        }

        workspace
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .child(measurement)
                    .child(self.workspace_terminal_area(cx)),
            )
            .child(self.bottom_panel_resize_handle(cx))
            .child(self.bottom_panel_view(cx))
    }

    fn workspace_terminal_area(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let palette = self.theme_palette();
        if !self.shell.pane_focus_mode() {
            // Multi-leaf tab windows (Tauri TabWindowsWorkspace) take precedence over
            // single-tree pane splits when active.
            if let Some(window_root) = self.shell.terminal_window_tree() {
                return div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .bg(self.shell_transparent_color(palette.bg))
                    .child(self.render_terminal_window_tree(window_root, cx))
                    .into_any_element();
            }
        }
        if self.session.start_has_active_pending() {
            return self.pending_workspace_state().into_any_element();
        }
        if self.session.start_has_active_failed() {
            return self.failed_workspace_state(cx).into_any_element();
        }
        if self
            .session
            .active_id()
            .is_none_or(|id| self.shell.session_tab_is_hidden(id))
        {
            if self.session.start_has_pending() {
                return self.pending_workspace_state().into_any_element();
            }
            if self.session.start_has_failed()
                || (self.shell.last_connect_failure_name().is_some()
                    && self.shell.last_connect_failure_error().is_some())
            {
                return self.failed_workspace_state(cx).into_any_element();
            }
            return self.empty_workspace_state(cx).into_any_element();
        }
        if self.shell.pane_focus_mode() {
            return self.render_workspace_pane_node(
                WorkspacePaneNode::leaf(self.session.active_id_owned().unwrap()),
                cx,
            );
        }
        let active_id = self.session.active_id_owned().unwrap();
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(self.shell_transparent_color(palette.bg))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.workspace_tab_session_content(active_id, cx)),
            )
            .into_any_element()
    }
}

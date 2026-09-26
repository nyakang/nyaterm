use gpui::{Context, IntoElement, div, prelude::*};
use rust_i18n::t;

use super::super::NyaTermApp;
use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};

mod panes;
mod terminal_windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaneBorderEdges {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

impl PaneBorderEdges {
    const ALL: Self = Self {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };

    fn split(self, direction: WorkspaceSplitDirection) -> (Self, Self) {
        let mut first = self;
        let mut second = self;
        match direction {
            WorkspaceSplitDirection::Horizontal => {
                first.bottom = false;
                second.top = false;
            }
            WorkspaceSplitDirection::Vertical => {
                first.right = false;
                second.left = false;
            }
        }
        (first, second)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn workspace_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // Match the Tauri shell: tab strip sits directly above the terminal surface.
        // Do not reconcile/prune layout here — paint must stay pure. Session
        // register/close/idle already keep terminal_windows coherent.
        let multi_leaf = self.terminal_windows_is_multi_leaf();
        let has_connect_failure = self.session.start_has_failed()
            || (self.shell.last_connect_failure_name().is_some()
                && self.shell.last_connect_failure_error().is_some());
        let show_tab_strip = !multi_leaf
            && (self.ordered_tab_session_count() > 0
                || self.session.start_has_pending()
                || has_connect_failure);
        let mut workspace = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(self.shell_transparent_color(palette.bg));
        if self.shell.pane_focus_mode() && self.session.active_id().is_some() {
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
                                cx.notify();
                            })),
                    ),
            );
            return workspace.child(self.workspace_terminal_area(cx));
        }
        if show_tab_strip {
            workspace = workspace.child(self.session_tab_strip(cx));
        }

        workspace
            .child(self.workspace_terminal_area(cx))
            .child(self.bottom_panel_resize_handle(cx))
            .child(self.bottom_panel_view(cx))
    }

    fn workspace_terminal_area(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let palette = self.theme_palette();
        if self.session.start_has_active_pending() {
            return self.pending_workspace_state().into_any_element();
        }
        if self.session.start_has_active_failed() {
            return self.failed_workspace_state().into_any_element();
        }
        if self.session.active_id().is_none() {
            if self.session.start_has_pending() {
                return self.pending_workspace_state().into_any_element();
            }
            if self.session.start_has_failed()
                || (self.shell.last_connect_failure_name().is_some()
                    && self.shell.last_connect_failure_error().is_some())
            {
                return self.failed_workspace_state().into_any_element();
            }
            return self.empty_workspace_state(cx).into_any_element();
        }
        if self.shell.pane_focus_mode() {
            return self.render_workspace_pane_node(
                WorkspacePaneNode::leaf(self.session.active_id_owned().unwrap()),
                false,
                PaneBorderEdges::ALL,
                cx,
            );
        }
        // Multi-leaf tab windows (Tauri TabWindowsWorkspace) take precedence over
        // single-tree pane splits when active.
        if let Some(window_root) = self.terminal.multi_leaf_terminal_window_tree() {
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .bg(self.shell_transparent_color(palette.bg))
                .child(self.render_terminal_window_tree(window_root, PaneBorderEdges::ALL, cx))
                .into_any_element();
        }
        let root =
            self.shell.workspace_split().cloned().unwrap_or_else(|| {
                WorkspacePaneNode::leaf(self.session.active_id_owned().unwrap())
            });

        let show_chrome = root.is_split();
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(self.shell_transparent_color(palette.bg))
            .child(self.render_workspace_pane_node(root, show_chrome, PaneBorderEdges::ALL, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::PaneBorderEdges;
    use crate::models::WorkspaceSplitDirection;

    #[test]
    fn vertical_split_only_suppresses_the_shared_edges() {
        let (first, second) = PaneBorderEdges::ALL.split(WorkspaceSplitDirection::Vertical);

        assert_eq!(
            first,
            PaneBorderEdges {
                top: true,
                right: false,
                bottom: true,
                left: true,
            }
        );
        assert_eq!(
            second,
            PaneBorderEdges {
                top: true,
                right: true,
                bottom: true,
                left: false,
            }
        );
    }

    #[test]
    fn nested_split_preserves_already_suppressed_outer_edges() {
        let (left, _) = PaneBorderEdges::ALL.split(WorkspaceSplitDirection::Vertical);
        let (top, bottom) = left.split(WorkspaceSplitDirection::Horizontal);

        assert!(!top.right);
        assert!(!bottom.right);
        assert!(!top.bottom);
        assert!(!bottom.top);
        assert!(top.top && top.left);
        assert!(bottom.bottom && bottom.left);
    }
}

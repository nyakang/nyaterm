use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, relative, rgb, svg,
};

use super::super::super::NyaTermApp;
use crate::features::formatting::short_id;
use crate::features::view_widgets::connection_spinner;
use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};
use crate::widgets::small_button;
use nyaterm_core::truncate_preview;

impl NyaTermApp {
    fn workspace_reconnect_pending_state(&self, session_id: &str) -> impl IntoElement {
        let palette = self.theme_palette();
        let name = self
            .session
            .display_name(session_id)
            .unwrap_or_else(|| short_id(session_id).to_string());
        let detail = t!("savedConnections.connecting", name = name);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(self.shell_terminal_surface_color(self.theme_palette().ui_terminal_bg))
            .child(connection_spinner(
                SharedString::from(format!("reconnect-spinner-{session_id}")),
                rgb(palette.primary).into(),
                32.,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .max_w(px(320.))
                    .px_4()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("tabCtx.reconnecting")),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(detail),
                    ),
            )
    }

    fn workspace_reconnect_failed_state(
        &mut self,
        session_id: String,
        error: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let reconnect_session_id = session_id.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(self.shell_terminal_surface_color(self.theme_palette().ui_terminal_bg))
            .child(
                svg()
                    .size(px(32.))
                    .path("icons/session/disconnect.svg")
                    .text_color(rgb(palette.danger)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_6()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text))
                            .child(t!("terminal.connectionFailed")),
                    )
                    .child(
                        div()
                            .max_w(px(320.))
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(truncate_preview(&error, 180)),
                    ),
            )
            .child(small_button(
                palette,
                format!("workspace-reconnect-{session_id}"),
                t!("tabCtx.reconnect"),
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.reconnect_session(reconnect_session_id.clone(), window, cx);
                }),
            ))
    }

    pub(super) fn workspace_session_content(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let reconnect_pending = self.session.start_reconnect_is_pending(&session_id);
        if reconnect_pending {
            return self
                .workspace_reconnect_pending_state(&session_id)
                .into_any_element();
        }
        if let Some(error) = self
            .session
            .start_reconnect_failure(&session_id)
            .map(str::to_string)
        {
            return self
                .workspace_reconnect_failed_state(session_id, error, cx)
                .into_any_element();
        }
        if self.remote_desktop.is_session(&session_id) {
            return self.remote_desktop_view(session_id, cx);
        }
        self.terminal_canvas_for(session_id, cx).into_any_element()
    }

    /// Content for one workspace tab: the owning root's pane tree when the
    /// session has a `pane_roots` projection (manual split or tmux control),
    /// otherwise the plain session view. tmux hosts also get their window
    /// strip under the tree. Both the workspace page and the terminal-window
    /// group renderer go through here so the projection cannot diverge.
    pub(super) fn workspace_tab_session_content(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tab_root = self.tab_root_for_session(&session_id);
        let Some(root) = self.shell.workspace_pane_root(&tab_root).cloned() else {
            return self.workspace_session_content(session_id, cx);
        };
        let tree = self.render_workspace_pane_node(root, cx);
        let mut area = div().flex_1().min_h_0().min_w_0().flex().flex_col();
        area = area.child(div().flex_1().min_h_0().min_w_0().child(tree));
        if self.tmux.is_control_session(&tab_root) {
            area = area.child(self.tmux_window_strip(&tab_root, cx));
        }
        area.into_any_element()
    }

    pub(super) fn render_workspace_pane_node(
        &mut self,
        node: WorkspacePaneNode,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match node {
            WorkspacePaneNode::Leaf { session_id } => {
                let content = self.workspace_session_content(session_id.clone(), cx);
                let focus_id = session_id.clone();
                let pane = div()
                    .id(SharedString::from(format!("workspace-leaf-{session_id}")))
                    .size_full()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_workspace_pane(focus_id.clone(), cx);
                        if this.remote_desktop.is_session(&focus_id) {
                            window.focus(this.remote_desktop.focus(), cx);
                        } else {
                            window.focus(this.terminal.input_focus(), cx);
                        }
                        cx.notify();
                    }));
                pane.child(div().flex_1().min_h_0().overflow_hidden().child(content))
                    .into_any_element()
            }
            WorkspacePaneNode::Split {
                id,
                direction,
                ratio_percent,
                first,
                second,
            } => {
                let first_el = self.render_workspace_pane_node(*first, cx);
                let second_el = self.render_workspace_pane_node(*second, cx);
                let divider = self.workspace_split_resize_handle(id.clone(), direction, cx);
                let primary_basis =
                    relative(WorkspacePaneNode::primary_weight(ratio_percent) / 100.);
                let secondary_basis =
                    relative(WorkspacePaneNode::secondary_weight(ratio_percent) / 100.);

                match direction {
                    WorkspaceSplitDirection::Horizontal => div()
                        .id(SharedString::from(format!("workspace-split-{id}")))
                        .size_full()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_none()
                                .flex_basis(primary_basis)
                                .min_h(px(80.))
                                .overflow_hidden()
                                .child(first_el),
                        )
                        .child(divider)
                        .child(
                            div()
                                .flex_none()
                                .flex_basis(secondary_basis)
                                .min_h(px(80.))
                                .overflow_hidden()
                                .child(second_el),
                        )
                        .into_any_element(),
                    WorkspaceSplitDirection::Vertical => div()
                        .id(SharedString::from(format!("workspace-split-{id}")))
                        .size_full()
                        .min_h_0()
                        .min_w_0()
                        .flex()
                        .child(
                            div()
                                .flex_none()
                                .flex_basis(primary_basis)
                                .min_w(px(120.))
                                .overflow_hidden()
                                .child(first_el),
                        )
                        .child(divider)
                        .child(
                            div()
                                .flex_none()
                                .flex_basis(secondary_basis)
                                .min_w(px(120.))
                                .overflow_hidden()
                                .child(second_el),
                        )
                        .into_any_element(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _, Render,
        RenderImage, Styled as _, TestAppContext, VisualTestContext, Window, div, rgb,
    };
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::{NyaTermApp, test_support::app_with_visible_local_session};

    #[derive(Clone, Copy, Debug)]
    enum Stage {
        Empty,
        Pending,
        Failed,
        Terminal,
        ReconnectPending,
        ReconnectFailed,
    }

    struct WorkspaceBackgroundFixture {
        app: Entity<NyaTermApp>,
        stage: Stage,
    }

    impl Render for WorkspaceBackgroundFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let stage = self.stage;
            let content = self.app.update(cx, |app, cx| match stage {
                Stage::Empty => app.empty_workspace_state(cx),
                Stage::Pending => app.pending_workspace_state().into_any_element(),
                Stage::Failed => app.failed_workspace_state(cx).into_any_element(),
                Stage::Terminal => app
                    .terminal_canvas_for("s0".to_string(), cx)
                    .into_any_element(),
                Stage::ReconnectPending => app
                    .workspace_reconnect_pending_state("s0")
                    .into_any_element(),
                Stage::ReconnectFailed => app
                    .workspace_reconnect_failed_state("s0".to_string(), String::new(), cx)
                    .into_any_element(),
            });
            div().size_full().flex().child(content)
        }
    }

    #[test]
    fn connecting_and_reconnecting_preserve_the_painted_workbench_background() {
        let root = TestTempDir::new("nyaterm-workspace-background");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "s0");
        let (terminal_color, expected_alpha) = cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.settings.set_background_content_opacity(35);
            let path = "wallpaper.png".to_string();
            app.shell.request_wallpaper(Some(path.clone()));
            app.shell.cache_wallpaper(
                path,
                Arc::new(RenderImage::new(vec![image::Frame::new(
                    image::RgbaImage::new(1, 1),
                )])),
                1,
                1,
            );
            let color = app.terminal_theme_palette().terminal_bg;
            let alpha = app.shell_surface_color(color).a;
            // The existing workbench paints this tint twice. Keep its appearance.
            (Hsla::from(rgb(color)), 1.0 - (1.0 - alpha).powi(2))
        });
        let fixture_app = app.clone();
        let (fixture, vcx) = cx.add_window_view(move |_, _| WorkspaceBackgroundFixture {
            app: fixture_app,
            stage: Stage::Empty,
        });
        let vcx: &mut VisualTestContext = vcx;
        for stage in [
            Stage::Empty,
            Stage::Pending,
            Stage::Failed,
            Stage::Terminal,
            Stage::ReconnectPending,
            Stage::ReconnectFailed,
        ] {
            vcx.update(|window, cx| {
                fixture.update(cx, |fixture, cx| {
                    fixture.stage = stage;
                    cx.notify();
                });
                _ = window.draw(cx);
                let mut transparency = 1.0;
                for quad in window.painted_quads() {
                    if quad.bounds.size.width.0 > 400.0
                        && quad.bounds.size.height.0 > 300.0
                        && let Some(color) = quad.background.as_solid()
                        && color.h == terminal_color.h
                        && color.s == terminal_color.s
                        && color.l == terminal_color.l
                    {
                        transparency *= 1.0 - color.a;
                    }
                }
                assert!(
                    (1.0 - transparency - expected_alpha).abs() < 0.0001,
                    "{stage:?} must preserve the workbench shading"
                );
            });
        }
    }
}

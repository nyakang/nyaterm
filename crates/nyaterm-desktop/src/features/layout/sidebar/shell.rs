use gpui::{Context, IntoElement, div, prelude::*, px, rgb};

use crate::features::NyaTermApp;
use crate::features::pages::remote::{HostToolKind, RemoteMonitorKind};
use crate::models::{NavItem, PanelSide};

/// Layout for a cached panel subtree.
///
/// `Entity::cached` skips rendering the contents to measure them, so it needs a definite
/// size and takes it from here instead. `size_full` is correct because the parent already
/// constrains the panel: `single_side_panel` puts the body in
/// `div().flex_1().min_h_0().overflow_hidden()`.
pub(in crate::features) fn cached_panel_style() -> gpui::StyleRefinement {
    gpui::StyleRefinement::default().size_full()
}

impl NyaTermApp {
    pub(in crate::features) fn sidebar(
        &mut self,
        draw_shared_edge: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut width = self.shell.left_panel_width().clamp(160., 720.);
        if !cfg!(target_os = "macos") && self.shell.viewport_size().0 < 1024. {
            width = width.min((self.shell.viewport_size().0 - 80.).max(120.));
        }
        let palette = self.theme_palette();
        div()
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .when(draw_shared_edge, |this| this.border_r_1())
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .child(self.side_panel_stack(PanelSide::Left, window, cx))
    }

    pub(in crate::features) fn left_panel_body(
        &mut self,
        panel: NavItem,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.panel_body(panel, window, cx)
    }

    pub(in crate::features) fn panel_body(
        &mut self,
        panel: NavItem,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match panel {
            NavItem::Plugins => self.plugins.panel.clone().into_any_element(),
            NavItem::Transfers => self
                .transfer_panel
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Notes => self
                .notes_panel
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Tunnels => self.tunnels_view(cx).into_any_element(),
            NavItem::SecurityAuth => self.security_auth_panel(cx).into_any_element(),
            NavItem::SyncBackupHistory => self
                .sync_backup_history_panel(window, cx)
                .into_any_element(),
            NavItem::Connections => self
                .connection_panel
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::AiAssistant => self
                .ai_panel
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::ActiveSessions => {
                let model = self.active_sessions_panel_model();
                self.active_sessions_panel(model, cx).into_any_element()
            }
            NavItem::CommandHistory => self.command_history_panel(cx).into_any_element(),
            NavItem::Stats => self
                .remote_panels
                .entity(RemoteMonitorKind::Stats)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::GpuMonitor => self
                .remote_panels
                .entity(RemoteMonitorKind::Gpu)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::AscendNpuMonitor => self
                .remote_panels
                .entity(RemoteMonitorKind::Npu)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Processes => self
                .remote_panels
                .entity(RemoteMonitorKind::Processes)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Docker => self
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Ports => self
                .host_tool_panels
                .entity(HostToolKind::Ports)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Services => self
                .host_tool_panels
                .entity(HostToolKind::Services)
                .clone()
                .cached(cached_panel_style())
                .into_any_element(),
            NavItem::Recording => {
                let model = self.recording_sessions_panel_model();
                self.recording_panel(model, cx).into_any_element()
            }
            NavItem::Workspace | NavItem::Settings => {
                self.left_workspace_summary(cx).into_any_element()
            }
        }
    }
}

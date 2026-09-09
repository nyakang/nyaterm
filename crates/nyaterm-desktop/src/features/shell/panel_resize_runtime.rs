use gpui::{
    Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, deferred,
    prelude::FluentBuilder as _,
};

use crate::features::{
    NyaTermApp, settings::UiLayoutSettingsUpdate, shell::state::RESIZE_HANDLE_HOVER_DELAY,
    view_widgets::horizontal_resize_handle_visual, view_widgets::vertical_resize_handle_visual,
};
use crate::models::{
    BottomPanelMode, NavItem, PanelResizeSide, PanelSide, panel_collapsed_from_persistence,
};

const QUICK_CMD_HEIGHT_MIN: f32 = 36.;
const SERIAL_SEND_HEIGHT_MIN: f32 = 60.;
const BOTTOM_PANEL_HEIGHT_MAX: f32 = 520.;
impl NyaTermApp {
    pub(in crate::features) fn update_resize_handle_hover(
        &mut self,
        id: SharedString,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            if self.shell.leave_resize_handle_hover(&id) {
                cx.notify();
            }
            return;
        }
        let Some(generation) = self.shell.begin_resize_handle_hover(id.clone()) else {
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(RESIZE_HANDLE_HOVER_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.shell.activate_resize_handle_hover(&id, generation) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn activate_resize_handle_immediately(
        &mut self,
        id: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.shell.activate_resize_handle_immediately(id);
        cx.notify();
    }

    pub(in crate::features) fn set_bottom_panel_mode(&mut self, mode: BottomPanelMode) {
        self.shell.bottom_panel.mode = mode;
        self.persist_ui_layout();
    }

    pub(in crate::features) fn start_panel_resize(
        &mut self,
        side: PanelResizeSide,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.shell.panels.start_resize(side, event.position.x);
        self.shell.set_status(match side {
            PanelResizeSide::Left => "resizing left panel".to_string(),
            PanelResizeSide::Right => "resizing right panel".to_string(),
        });
        cx.notify();
    }

    pub(in crate::features) fn update_panel_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some((side, width)) = self.shell.panels.update_resize(event.position.x) else {
            return;
        };
        if side == PanelResizeSide::Right {
            // The process table drops columns as it narrows, and its sort key has to
            // follow. Mid-drag rather than on release, which is when render used to do
            // it.
            self.reconcile_remote_process_sort_columns();
            // Flush boundary: the width is part of the snapshot key, and it changes here
            // without any pane revision moving.
            self.flush_remote_panel_snapshots(cx);
        }
        match side {
            PanelResizeSide::Left => {
                self.shell
                    .set_status(format!("left panel: {:.0}px", width.round()));
            }
            PanelResizeSide::Right => {
                // Right handle sits on the left edge of the right panel: drag left grows width.
                self.shell
                    .set_status(format!("right panel: {:.0}px", width.round()));
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn finish_panel_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.panels.finish_resize() {
            self.persist_panel_widths();
            self.shell.set_status(format!(
                "panel sizes L{:.0}/R{:.0}",
                self.shell.panels.left_width.round(),
                self.shell.panels.right_width.round()
            ));
            cx.notify();
        }
    }

    fn persist_panel_widths(&mut self) {
        self.persist_ui_layout();
    }

    pub(in crate::features) fn apply_ui_layout_from_settings(&mut self) {
        self.shell.panels.left_width = self.settings.summary().ui_left_panel_width as f32;
        self.shell.panels.right_width = self.settings.summary().ui_right_panel_width as f32;
        self.reconcile_remote_process_sort_columns();
        self.transfer
            .set_panel_height(self.settings.summary().ui_transfer_height as f32);
        self.shell.bottom_panel.quick_commands_height =
            self.settings.summary().ui_quick_cmd_height as f32;
        self.shell.bottom_panel.command_send_height =
            self.settings.summary().ui_serial_send_height as f32;
        self.apply_activity_layout_from_settings();
        self.shell.panels.active_left = self
            .settings
            .summary()
            .ui_active_left_panel
            .as_deref()
            .and_then(NavItem::from_persistence_id)
            .filter(|item| self.panel_side_for_item(*item) == Some(PanelSide::Left));
        self.shell.panels.active_right = self
            .settings
            .summary()
            .ui_active_right_panel
            .as_deref()
            .and_then(NavItem::from_persistence_id)
            .filter(|item| self.panel_side_for_item(*item) == Some(PanelSide::Right));
        self.shell.panels.left_collapsed = panel_collapsed_from_persistence(
            self.settings.summary().ui_left_panel_collapsed,
            self.settings.summary().ui_panel_multi_open,
            self.shell.panels.active_left.is_some(),
            !self.settings.summary().ui_left_open_panels.is_empty(),
        );
        self.shell.panels.right_collapsed = panel_collapsed_from_persistence(
            self.settings.summary().ui_right_panel_collapsed,
            self.settings.summary().ui_panel_multi_open,
            self.shell.panels.active_right.is_some(),
            !self.settings.summary().ui_right_open_panels.is_empty(),
        );
        self.apply_panel_stack_from_settings();
        if !self.settings.summary().has_master_password {
            self.security.unlock_without_master_password();
        }
    }

    pub(in crate::features) fn persist_ui_layout(&mut self) {
        let update = UiLayoutSettingsUpdate {
            left_panel_width: self.shell.panels.left_width.round().clamp(160., 720.) as u32,
            right_panel_width: self.shell.panels.right_width.round().clamp(200., 720.) as u32,
            transfer_height: self.transfer.panel_height().round().clamp(60., 600.) as u32,
            quick_command_height: self
                .shell
                .bottom_panel
                .quick_commands_height
                .round()
                .clamp(QUICK_CMD_HEIGHT_MIN, BOTTOM_PANEL_HEIGHT_MAX)
                as u32,
            quick_command_visible: self.shell.bottom_panel.mode == BottomPanelMode::QuickCommands,
            serial_send_height: self
                .shell
                .bottom_panel
                .command_send_height
                .round()
                .clamp(SERIAL_SEND_HEIGHT_MIN, BOTTOM_PANEL_HEIGHT_MAX)
                as u32,
            serial_send_visible: self.shell.bottom_panel.mode == BottomPanelMode::CommandSend,
            active_left_panel: self
                .shell
                .panels
                .active_left
                .map(|item| item.persistence_id().to_string()),
            active_right_panel: self
                .shell
                .panels
                .active_right
                .map(|item| item.persistence_id().to_string()),
            left_panel_collapsed: self.shell.panels.left_collapsed,
            right_panel_collapsed: self.shell.panels.right_collapsed,
            saved_connections_sort_mode: self
                .connection_state
                .list_sort_mode()
                .persistence_id()
                .to_string(),
            saved_connections_expanded_group_ids: self
                .connection_state
                .list_expanded_group_ids()
                .iter()
                .cloned()
                .collect(),
            start_workspace_mode: self.start_workspace.mode().as_str().to_string(),
            asset_sort_key: self
                .start_workspace
                .sort()
                .map(|sort| sort.key.as_str().to_string()),
            asset_sort_direction: self
                .start_workspace
                .sort()
                .map(|sort| sort.direction.as_str().to_string()),
            activity_bar_left_top: self.shell.chrome.activity_bar_layout.left_top.clone(),
            activity_bar_left_bottom: self.shell.chrome.activity_bar_layout.left_bottom.clone(),
            activity_bar_right_top: self.shell.chrome.activity_bar_layout.right_top.clone(),
            activity_bar_right_bottom: self.shell.chrome.activity_bar_layout.right_bottom.clone(),
            activity_bar_show_labels: self.shell.chrome.activity_bar_layout.show_labels,
            activity_bar_hidden_items: self.shell.chrome.activity_bar_layout.hidden_items.clone(),
            panel_multi_open: self.shell.panels.multi_open,
            panel_open_mode: self.shell.panels.open_mode.as_setting().to_string(),
            left_open_panels: self.shell.panels.left_open.clone(),
            right_open_panels: self.shell.panels.right_open.clone(),
            panel_stack_sizes: self
                .shell
                .panels
                .stack_sizes
                .iter()
                .filter_map(|(key, value)| {
                    let scaled = (*value * 1000.).round();
                    (scaled.is_finite() && scaled > 0.).then(|| (key.clone(), scaled as u32))
                })
                .collect(),
        };
        self.settings.apply_ui_layout(update);
        self.shell.mark_ui_layout_persist_pending();
    }

    pub(in crate::features) fn panel_resize_handle(
        &self,
        side: PanelResizeSide,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let id = SharedString::from(format!(
            "panel-resize-{}",
            match side {
                PanelResizeSide::Left => "left",
                PanelResizeSide::Right => "right",
            }
        ));
        let hover_id = id.clone();
        let drag_id = id.clone();
        deferred(
            vertical_resize_handle_visual(
                palette,
                self.shell
                    .panels
                    .resize
                    .is_some_and(|resize| resize.side == side),
                self.shell.resize_handle_is_highlighted(&id),
            )
            .id(id.clone())
            .cursor_col_resize()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.update_resize_handle_hover(hover_id.clone(), *hovered, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.activate_resize_handle_immediately(drag_id.clone(), cx);
                    this.start_panel_resize(side, event, cx);
                }),
            ),
        )
    }
}

impl NyaTermApp {
    pub(in crate::features) fn start_transfer_height_resize(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.transfer.start_panel_height_resize(event.position.y);
        self.shell.set_status("resizing transfer queue".to_string());
        cx.notify();
    }

    pub(in crate::features) fn update_transfer_height_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(height) = self.transfer.update_panel_height_resize(event.position.y) else {
            return;
        };
        self.shell
            .set_status(format!("transfer queue: {:.0}px", height.round()));
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn finish_transfer_height_resize(&mut self, cx: &mut Context<Self>) {
        if self.transfer.finish_panel_height_resize() {
            self.persist_ui_layout();
            self.shell.set_status(format!(
                "transfer queue {:.0}px",
                self.transfer.panel_height().round()
            ));
            self.defer_transfer_panel_snapshot_flush(cx);
            cx.notify();
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn start_bottom_panel_resize(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.bottom_panel.start_resize(event.position.y) {
            return;
        }
        self.shell.set_status("resizing bottom panel".to_string());
        cx.notify();
    }

    pub(in crate::features) fn update_bottom_panel_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(next) = self.shell.bottom_panel.update_resize(event.position.y) else {
            return;
        };
        self.shell
            .set_status(format!("bottom panel: {:.0}px", next.round()));
        cx.notify();
    }

    pub(in crate::features) fn finish_bottom_panel_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.bottom_panel.finish_resize() {
            self.persist_ui_layout();
            self.shell.set_status("bottom panel size saved".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn bottom_panel_resize_handle(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let id = SharedString::from("bottom-panel-resize");
        let hover_id = id.clone();
        let drag_id = id.clone();
        deferred(
            horizontal_resize_handle_visual(
                palette,
                self.shell.bottom_panel.resize.is_some(),
                self.shell.resize_handle_is_highlighted(&id),
            )
            .id(id.clone())
            .cursor_row_resize()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.update_resize_handle_hover(hover_id.clone(), *hovered, cx);
            }))
            .when(
                self.shell.bottom_panel.mode == BottomPanelMode::Hidden,
                |this| this.h_0().mt_0().mb_0(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.activate_resize_handle_immediately(drag_id.clone(), cx);
                    this.start_bottom_panel_resize(event, cx);
                }),
            ),
        )
    }
}

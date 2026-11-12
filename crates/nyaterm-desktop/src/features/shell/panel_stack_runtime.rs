use rust_i18n::t;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, deferred, div, point,
    prelude::FluentBuilder as _, px, rgb, size, svg,
};
use nyaterm_core::truncate_preview;

use crate::features::NyaTermApp;
use crate::features::pages::remote::HostToolKind;
use crate::features::view_widgets::{
    conversation_panel_header_with_actions, panel_header_with_actions,
};
use crate::models::{
    ActivityBarZone, NavItem, NetworkTab, PanelOpenMode, PanelSide, RightFocus, SecurityAuthTab,
    SettingsTab,
};
use crate::theme::ThemePalette;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDialogWindowExt as _, NyaTooltip, nya_popup_is_active,
};

use super::state::{FloatingPressSource, PANEL_STACK_MIN_HEIGHT};

type PanelStackMeasurements = Rc<RefCell<Vec<(String, f32)>>>;

const EXCLUSIVE_PANEL_IDS: &[&str] = &["aiAssistant"];
const NON_PANEL_IDS: &[&str] = &["settings", "lock", "quickCmdBar", "serialSend"];

/// Fixed chrome sizes the floating containers are positioned against: the
/// title bar sits above the workspace surface, and the activity rail is inset.
const FLOATING_PANEL_TITLE_BAR_HEIGHT: f32 = 40.;
const FLOATING_PANEL_ACTIVITY_BAR_WIDTH: f32 = 40.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidePanelStackRenderMode {
    Overlay(NavItem),
    Stack,
}

fn side_panel_stack_render_mode(overlay: Option<NavItem>) -> SidePanelStackRenderMode {
    overlay
        .map(SidePanelStackRenderMode::Overlay)
        .unwrap_or(SidePanelStackRenderMode::Stack)
}

impl NyaTermApp {
    pub(in crate::features) fn transfer_browser_is_visible(&self) -> bool {
        [PanelSide::Left, PanelSide::Right].into_iter().any(|side| {
            if self.shell.panel_is_floating() {
                return self.shell.panels.floating_panel(side) == Some(NavItem::Transfers);
            }
            let viewport_width = self.shell.viewport_size().0;
            if !cfg!(target_os = "macos") && viewport_width > 0. {
                let (overlay_threshold, drawer_open) = match side {
                    PanelSide::Left => (1024., self.shell.mobile_left_panel_open()),
                    PanelSide::Right => (768., self.shell.mobile_right_panel_open()),
                };
                if viewport_width < overlay_threshold && !drawer_open {
                    return false;
                }
            }
            let collapsed = match side {
                PanelSide::Left => self.shell.panels.left_collapsed,
                PanelSide::Right => self.shell.panels.right_collapsed,
            };
            !collapsed
                && if self.shell.panels.multi_open {
                    self.side_overlay_panel(side) == Some(NavItem::Transfers)
                        || (self.side_overlay_panel(side).is_none()
                            && self
                                .side_open_panel_ids(side)
                                .iter()
                                .any(|id| id == NavItem::Transfers.persistence_id()))
                } else {
                    (match side {
                        PanelSide::Left => self.current_left_panel(),
                        PanelSide::Right => self.current_right_panel(),
                    }) == Some(NavItem::Transfers)
                }
        })
    }

    pub(in crate::features) fn is_exclusive_panel_id(id: &str) -> bool {
        EXCLUSIVE_PANEL_IDS.contains(&id)
    }

    pub(in crate::features) fn is_stackable_panel_id(id: &str) -> bool {
        !NON_PANEL_IDS.contains(&id) && !Self::is_exclusive_panel_id(id)
    }

    pub(in crate::features) fn toggle_panel_multi_open(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        let next = !self.shell.panels.multi_open;
        self.shell.panels.multi_open = next;
        if !self.shell.panels.open_mode.is_floating() {
            if next {
                if self.shell.panels.left_open.is_empty()
                    && let Some(panel) = self.shell.panels.active_left
                {
                    let id = panel.persistence_id().to_string();
                    if Self::is_stackable_panel_id(&id) {
                        self.shell.panels.left_open.push(id);
                    }
                }
                if self.shell.panels.right_open.is_empty()
                    && let Some(panel) = self.shell.panels.active_right
                {
                    let id = panel.persistence_id().to_string();
                    if Self::is_stackable_panel_id(&id) {
                        self.shell.panels.right_open.push(id);
                    }
                }
            } else {
                if self.shell.panels.active_left.is_none() {
                    self.shell.panels.active_left = self
                        .shell
                        .panels
                        .left_open
                        .first()
                        .and_then(|id| NavItem::from_persistence_id(id));
                }
                if self.shell.panels.active_right.is_none() {
                    self.shell.panels.active_right = self
                        .shell
                        .panels
                        .right_open
                        .first()
                        .and_then(|id| NavItem::from_persistence_id(id));
                }
                self.shell.panels.left_open.clear();
                self.shell.panels.right_open.clear();
            }
        }
        self.shell.set_status(if next {
            "multi-open panels enabled"
        } else {
            "multi-open panels disabled"
        });
        self.persist_ui_layout();
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
        cx.notify();
    }

    /// Switch between persisted docked and floating presentation. Floating
    /// selections are transient and docked open state is intentionally cleared
    /// on the boundary, matching main's state model.
    pub(in crate::features) fn set_panel_open_mode(
        &mut self,
        mode: PanelOpenMode,
        cx: &mut Context<Self>,
    ) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.open_mode == mode {
            return;
        }
        self.shell.panels.set_open_mode(mode);
        self.shell.panels.clear_floating();
        if mode.is_floating() {
            self.shell.panels.left_open.clear();
            self.shell.panels.right_open.clear();
            self.shell.panels.active_left = None;
            self.shell.panels.active_right = None;
            self.shell.panels.left_collapsed = true;
            self.shell.panels.right_collapsed = true;
        }
        self.shell.set_status(if mode.is_floating() {
            "floating panels enabled"
        } else {
            "docked panels enabled"
        });
        self.persist_ui_layout();
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_floating_panel(
        &mut self,
        item: NavItem,
        side: PanelSide,
        cx: &mut Context<Self>,
    ) {
        let browser_was_visible = self.transfer_browser_is_visible();
        let next = (self.shell.panels.floating_panel(side) != Some(item)).then_some(item);
        self.shell.panels.set_floating_panel(side, next);
        self.shell.set_status(if next.is_some() {
            format!("{} floating panel opened", item.persistence_id())
        } else {
            format!("{} floating panel closed", item.persistence_id())
        });
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
        cx.notify();
    }

    pub(in crate::features) fn close_floating_panel(
        &mut self,
        side: PanelSide,
        cx: &mut Context<Self>,
    ) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.floating_panel(side).is_some() {
            self.shell.panels.set_floating_panel(side, None);
            self.transfer_browser_visibility_changed(browser_was_visible, cx);
            self.sync_host_tools(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn close_last_floating_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(side) = self.shell.panels.last_floating_side else {
            return false;
        };
        self.close_floating_panel(side, cx);
        true
    }

    /// Window-coordinate rectangle of an open floating overlay.
    ///
    /// Derived from the same fixed offsets the render code uses
    /// (`top_0/left(px(40.))` inside the surface that starts below the 40px
    /// title bar), so the input path never reads layout.
    fn floating_panel_window_bounds(&self, side: PanelSide) -> Option<Bounds<Pixels>> {
        if !self.shell.panel_is_floating() {
            return None;
        }
        self.shell.panels.floating_panel(side)?;
        let (viewport_width, viewport_height) = self.shell.viewport_size();
        if viewport_width <= 0. || viewport_height <= 0. {
            return None;
        }
        let panel_width = match side {
            PanelSide::Left => self.shell.left_panel_width(),
            PanelSide::Right => self.shell.right_panel_width(),
        };
        let left = match side {
            PanelSide::Left => FLOATING_PANEL_ACTIVITY_BAR_WIDTH,
            PanelSide::Right => viewport_width - FLOATING_PANEL_ACTIVITY_BAR_WIDTH - panel_width,
        };
        let height = (viewport_height - FLOATING_PANEL_TITLE_BAR_HEIGHT).max(0.);
        Some(Bounds::new(
            point(px(left), px(FLOATING_PANEL_TITLE_BAR_HEIGHT)),
            size(px(panel_width), px(height)),
        ))
    }

    fn position_inside_floating_panel(&self, position: Point<Pixels>) -> bool {
        [PanelSide::Left, PanelSide::Right]
            .into_iter()
            .filter_map(|side| self.floating_panel_window_bounds(side))
            .any(|bounds| bounds.contains(&position))
    }

    /// Popups get the first dismissal click, including menus painted beyond
    /// the panel bounds. Read their existing owners before mouse-down can
    /// dismiss them or move focus back to the workspace.
    pub(in crate::features) fn floating_panel_dismissal_is_blocked(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        window.has_active_prompt()
            || window.has_active_nya_dialog(cx)
            || nya_popup_is_active(window, cx)
            || self.ai.transient_menus_are_open()
            || self.remote_ops.docker_menus_open()
            || self.shell.open_tabs_menu_is_open()
            || self.shell.new_session_menu_is_open()
            || self.shell.header_status_menu_is_open()
            || self.shell.activity_bar_context_menu().is_some()
            || self.session.dialog_tab_actions_target().is_some()
            || self.session.dialog_color_picker_is_open()
            || self.terminal.overlay_visibility().action_link_menu
            || self.transfer.transfer_job_menu().is_some()
            || self.transfer.browser_view().favorites_menu.is_some()
            || self.transfer.browser_view().path_menu.is_some()
            || self.transfer.browser_view().upload_menu.is_some()
    }

    pub(in crate::features) fn capture_floating_panel_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            self.shell.panels.clear_floating_outside_press();
            return;
        }
        // Native observation precedes capture even for ordinary GPUI targets.
        // Reuse its decision made before menus were dismissed.
        if !self.shell.panels.handoff_native_floating_press() {
            self.begin_floating_panel_press(event.position, FloatingPressSource::Gpui, window, cx);
        }
    }

    fn begin_floating_panel_press(
        &mut self,
        position: Point<Pixels>,
        source: FloatingPressSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let eligible = !self.floating_panel_dismissal_is_blocked(window, cx)
            && self.shell.panel_is_floating()
            && !self.position_inside_floating_panel(position)
            && [PanelSide::Left, PanelSide::Right]
                .into_iter()
                .any(|side| self.floating_panel_window_bounds(side).is_some());
        self.shell
            .panels
            .begin_floating_press(position, eligible, source)
    }

    /// A native child sends no release. If GPUI capture takes over this press,
    /// only its release may complete the gesture instead.
    pub(in crate::features) fn defer_native_floating_panel_outside_press(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.begin_floating_panel_press(
            position,
            FloatingPressSource::NativePending,
            window,
            cx,
        );
        let app = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |this, cx| {
                this.finish_floating_panel_outside_click(
                    id,
                    FloatingPressSource::NativePending,
                    position,
                    window,
                    cx,
                );
            });
        });
    }

    /// Terminal pointer handlers can stop bubbling before the root's menu
    /// dismissal runs. Preserve popup priority while closing AI's own menus
    /// from their own outside handlers, using the menu's bounds rather than
    /// the panel's. Component popups and dialogs handle their own dismissal.
    pub(in crate::features) fn dismiss_floating_ai_menu_on_outside_press(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.panel_is_floating()
            || !self.ai.transient_menus_are_open()
            || window.has_active_prompt()
            || window.has_active_nya_dialog(cx)
            || nya_popup_is_active(window, cx)
        {
            return;
        }
        self.shell.panels.clear_floating_outside_press();
        if self.ai.history_is_open() {
            self.close_ai_history(window, cx);
        }
        self.ai.close_transient_menus();
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    /// Capture runs before the activity icon's click. Defer until that click
    /// has invalidated the press if it opened, closed or replaced a panel.
    pub(in crate::features) fn defer_floating_panel_outside_click(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.shell.panels.floating_press_id() else {
            return;
        };
        let app = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |this, cx| {
                this.finish_floating_panel_outside_click(
                    id,
                    FloatingPressSource::Gpui,
                    position,
                    window,
                    cx,
                );
            });
        });
    }

    fn finish_floating_panel_outside_click(
        &mut self,
        id: u64,
        source: FloatingPressSource,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(press) = self.shell.panels.take_floating_outside_press(id, source) else {
            return;
        };
        if self.floating_panel_dismissal_is_blocked(window, cx)
            || !self.shell.panel_is_floating()
            || press.panels
                != [
                    self.shell.panels.floating_left,
                    self.shell.panels.floating_right,
                ]
            || self.position_inside_floating_panel(position)
            || !press.is_click_at(position)
        {
            return;
        }
        let browser_was_visible = self.transfer_browser_is_visible();
        self.shell.panels.clear_floating();
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
        cx.notify();
    }

    pub(in crate::features) fn side_open_panel_ids(&self, side: PanelSide) -> Vec<String> {
        if !self.shell.panels.multi_open {
            let active = match side {
                PanelSide::Left => self.shell.panels.active_left,
                PanelSide::Right => self.shell.panels.active_right,
            };
            return active
                .map(|item| item.persistence_id().to_string())
                .into_iter()
                .collect();
        }

        let open = match side {
            PanelSide::Left => &self.shell.panels.left_open,
            PanelSide::Right => &self.shell.panels.right_open,
        };
        if open.is_empty() {
            return Vec::new();
        }
        let open_set: HashSet<_> = open.iter().cloned().collect();
        let zones = match side {
            PanelSide::Left => [ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom],
            PanelSide::Right => [ActivityBarZone::RightTop, ActivityBarZone::RightBottom],
        };
        let mut ordered = Vec::new();
        for zone in zones {
            for id in self.shell.chrome.activity_bar_layout.zone(zone) {
                if open_set.contains(id) && Self::is_stackable_panel_id(id) {
                    ordered.push(id.clone());
                }
            }
        }
        ordered
    }

    /// Whether `item`'s panel is on screen right now.
    ///
    /// Mirrors the branches of `side_panel_stack`, which is the thing that decides what
    /// actually gets built: an exclusive overlay takes its whole side; an empty open
    /// list falls back to the current panel and then to the side's first entry; and
    /// otherwise a single-entry list or single-panel mode shows the first id while a
    /// multi-open stack shows all of them.
    ///
    pub(in crate::features) fn panel_is_rendered(&self, item: NavItem) -> bool {
        let Some(side) = self.panel_side_for_item(item) else {
            return false;
        };
        let activity_side = match side {
            PanelSide::Left => crate::features::runtime_jobs::ActivitySide::Left,
            PanelSide::Right => crate::features::runtime_jobs::ActivitySide::Right,
        };
        if self.shell.panels.open_mode.is_floating() {
            return self.shell.panels.floating_panel(side) == Some(item);
        }
        if !self.activity_side_has_items(activity_side) {
            return false;
        }
        let (threshold, drawer_open) = match side {
            PanelSide::Left => (1024., self.shell.mobile_left_panel_open()),
            PanelSide::Right => (768., self.shell.mobile_right_panel_open()),
        };
        if !cfg!(target_os = "macos") && self.shell.viewport_size().0 < threshold && !drawer_open {
            return false;
        }
        let side_open = match side {
            PanelSide::Left => self.left_side_open(),
            PanelSide::Right => self.right_side_open(),
        };
        if !side_open {
            return false;
        }
        // `side_open_panel_ids` does not check collapse, so the test above is what
        // stops a collapsed side reporting its remembered active panel as visible.
        if let Some(overlay) = self.side_overlay_panel(side) {
            return overlay == item;
        }
        let open_ids = self.side_open_panel_ids(side);
        if open_ids.is_empty() {
            let fallback = match side {
                PanelSide::Left => self.current_left_panel(),
                PanelSide::Right => self.current_right_panel(),
            }
            .or_else(|| {
                self.shell
                    .chrome
                    .activity_bar_layout
                    .first_panel_on_side(side)
            });
            return fallback == Some(item);
        }
        if !self.shell.panels.multi_open || open_ids.len() == 1 {
            return open_ids
                .first()
                .and_then(|id| NavItem::from_persistence_id(id))
                == Some(item);
        }
        open_ids.iter().any(|id| id == item.persistence_id())
    }

    pub(in crate::features) fn side_overlay_panel(&self, side: PanelSide) -> Option<NavItem> {
        if !self.shell.panels.multi_open {
            return None;
        }
        let active = match side {
            PanelSide::Left => self.shell.panels.active_left,
            PanelSide::Right => self.shell.panels.active_right,
        }?;
        let id = active.persistence_id();
        Self::is_exclusive_panel_id(id).then_some(active)
    }

    pub(in crate::features) fn panel_stack_weight(&self, panel_id: &str) -> f32 {
        self.shell.panels.stack_weight(panel_id)
    }

    pub(in crate::features) fn panel_side_for_item(&self, item: NavItem) -> Option<PanelSide> {
        self.shell
            .chrome
            .activity_bar_layout
            .side_for_entry(item.persistence_id())
            .or_else(|| item.is_left_panel().then_some(PanelSide::Left))
            .or_else(|| item.is_right_panel().then_some(PanelSide::Right))
    }

    pub(in crate::features) fn open_or_toggle_panel(
        &mut self,
        item: NavItem,
        cx: &mut Context<Self>,
    ) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if item == NavItem::Settings || item.opens_settings() {
            self.open_page(NavItem::Settings, cx);
            return;
        }
        if self.shell.panel_is_floating()
            && let Some(side) = self.panel_side_for_item(item)
        {
            self.toggle_floating_panel(item, side, cx);
            return;
        }
        if !self.shell.panels.multi_open {
            self.open_panel(item, cx);
            return;
        }

        let id = item.persistence_id().to_string();
        let Some(side) = self.panel_side_for_item(item) else {
            self.open_panel(item, cx);
            return;
        };

        self.shell.navigation.selected_nav = item;
        if item == NavItem::Recording {
            self.shell.panels.right_focus = RightFocus::Recording;
        } else {
            self.shell.panels.right_focus = RightFocus::Default;
        }

        if Self::is_exclusive_panel_id(&id) {
            let active = match side {
                PanelSide::Left => self.shell.panels.active_left,
                PanelSide::Right => self.shell.panels.active_right,
            };
            if active == Some(item) {
                // Dismiss exclusive overlay to stack.
                let fallback = self
                    .side_open_panel_ids(side)
                    .into_iter()
                    .find_map(|open_id| NavItem::from_persistence_id(&open_id));
                match side {
                    PanelSide::Left => {
                        self.shell.panels.active_left = fallback;
                        self.shell.panels.left_collapsed = fallback.is_none();
                    }
                    PanelSide::Right => {
                        self.shell.panels.active_right = fallback;
                        self.shell.panels.right_collapsed = fallback.is_none();
                    }
                }
                self.shell.set_status(format!("{} closed", item.label()));
            } else {
                match side {
                    PanelSide::Left => {
                        self.shell.panels.active_left = Some(item);
                        self.shell.panels.left_collapsed = false;
                    }
                    PanelSide::Right => {
                        self.shell.panels.active_right = Some(item);
                        self.shell.panels.right_collapsed = false;
                    }
                }
                self.shell.set_status(format!("{} opened", item.label()));
            }
            self.persist_ui_layout();
            self.transfer_browser_visibility_changed(browser_was_visible, cx);
            self.sync_host_tools(cx);
            cx.notify();
            return;
        }

        let open_list = match side {
            PanelSide::Left => &mut self.shell.panels.left_open,
            PanelSide::Right => &mut self.shell.panels.right_open,
        };
        let is_open = open_list.iter().any(|value| value == &id);
        let active = match side {
            PanelSide::Left => self.shell.panels.active_left,
            PanelSide::Right => self.shell.panels.active_right,
        };

        // If exclusive overlay is showing and stacked panel already open, reveal stack.
        if is_open
            && active
                .map(|item| Self::is_exclusive_panel_id(item.persistence_id()))
                .unwrap_or(false)
        {
            match side {
                PanelSide::Left => {
                    self.shell.panels.active_left = Some(item);
                    self.shell.panels.left_collapsed = false;
                }
                PanelSide::Right => {
                    self.shell.panels.active_right = Some(item);
                    self.shell.panels.right_collapsed = false;
                }
            }
            self.shell.set_status(format!("{} focused", item.label()));
            self.persist_ui_layout();
            self.transfer_browser_visibility_changed(browser_was_visible, cx);
            self.sync_host_tools(cx);
            cx.notify();
            return;
        }

        if is_open {
            open_list.retain(|value| value != &id);
            let next_active = if open_list.is_empty() {
                None
            } else if active
                .map(|item| item.persistence_id() == id)
                .unwrap_or(false)
            {
                open_list
                    .first()
                    .and_then(|value| NavItem::from_persistence_id(value))
            } else {
                active.filter(|item| open_list.iter().any(|value| value == item.persistence_id()))
            };
            match side {
                PanelSide::Left => {
                    self.shell.panels.active_left = next_active;
                    self.shell.panels.left_collapsed =
                        next_active.is_none() && self.shell.panels.left_open.is_empty();
                }
                PanelSide::Right => {
                    self.shell.panels.active_right = next_active;
                    self.shell.panels.right_collapsed =
                        next_active.is_none() && self.shell.panels.right_open.is_empty();
                }
            }
            self.shell.set_status(format!("{} closed", item.label()));
        } else {
            open_list.push(id);
            match side {
                PanelSide::Left => {
                    self.shell.panels.active_left = Some(item);
                    self.shell.panels.left_collapsed = false;
                }
                PanelSide::Right => {
                    self.shell.panels.active_right = Some(item);
                    self.shell.panels.right_collapsed = false;
                }
            }
            self.shell.set_status(format!("{} opened", item.label()));
        }
        self.persist_ui_layout();
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
        cx.notify();
        // Publish the new panel visibility and browser state.
        self.flush_transfer_panel_snapshot(cx);
    }

    pub(in crate::features) fn ensure_panel_in_stack(
        &mut self,
        item: NavItem,
        cx: &mut Context<Self>,
    ) {
        let browser_was_visible = self.transfer_browser_is_visible();
        self.shell.navigation.selected_nav = item;
        if !self.shell.panels.multi_open {
            self.ensure_panel_open(item, cx);
            return;
        }
        let id = item.persistence_id().to_string();
        match self.panel_side_for_item(item) {
            Some(PanelSide::Left) => {
                self.shell.panels.left_collapsed = false;
                if Self::is_exclusive_panel_id(&id) {
                    self.shell.panels.active_left = Some(item);
                } else {
                    if !self.shell.panels.left_open.iter().any(|value| value == &id) {
                        self.shell.panels.left_open.push(id);
                    }
                    self.shell.panels.active_left = Some(item);
                }
            }
            Some(PanelSide::Right) => {
                self.shell.panels.right_collapsed = false;
                self.shell.panels.right_focus = if item == NavItem::Recording {
                    RightFocus::Recording
                } else {
                    RightFocus::Default
                };
                if Self::is_exclusive_panel_id(&id) {
                    self.shell.panels.active_right = Some(item);
                } else {
                    if !self
                        .shell
                        .panels
                        .right_open
                        .iter()
                        .any(|value| value == &id)
                    {
                        self.shell.panels.right_open.push(id);
                    }
                    self.shell.panels.active_right = Some(item);
                }
            }
            None => {}
        }
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.sync_host_tools(cx);
    }

    pub(in crate::features) fn start_panel_stack_resize(
        &mut self,
        side: PanelSide,
        above_id: String,
        below_id: String,
        event: &MouseDownEvent,
        panel_heights: &[(String, f32)],
        cx: &mut Context<Self>,
    ) {
        if !self.shell.panels.start_stack_resize(
            side,
            above_id,
            below_id,
            event.position.y,
            panel_heights,
        ) {
            return;
        }
        self.shell.set_status("resizing panel stack".to_string());
        cx.notify();
    }

    pub(in crate::features) fn update_panel_stack_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if self.shell.panels.update_stack_resize(event.position.y) {
            cx.notify();
        }
    }

    pub(in crate::features) fn finish_panel_stack_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.panels.finish_stack_resize() {
            self.persist_ui_layout();
            self.shell.set_status("panel stack sizes saved".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn floating_side_panel(
        &mut self,
        side: PanelSide,
        panel: NavItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let close_button = (panel != NavItem::Plugins).then(|| {
            header_svg_icon_button(
                self.theme_palette(),
                match side {
                    PanelSide::Left => "floating-left-panel-close",
                    PanelSide::Right => "floating-right-panel-close",
                },
                "icons/window/close.svg",
                t!("common.close"),
                true,
                cx.listener(move |this, _, _, cx| {
                    this.close_floating_panel(side, cx);
                }),
            )
            .into_any_element()
        });
        self.single_side_panel(side, panel, close_button, window, cx)
    }

    pub(in crate::features) fn side_panel_stack(
        &mut self,
        side: PanelSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if let SidePanelStackRenderMode::Overlay(overlay) =
            side_panel_stack_render_mode(self.side_overlay_panel(side))
        {
            return div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(self.single_side_panel(side, overlay, None, window, cx));
        }

        let open_ids = self.side_open_panel_ids(side);
        if open_ids.is_empty() {
            let fallback = match side {
                PanelSide::Left => self.current_left_panel(),
                PanelSide::Right => self.current_right_panel(),
            }
            .or_else(|| {
                self.shell
                    .chrome
                    .activity_bar_layout
                    .first_panel_on_side(side)
            })
            .unwrap_or(NavItem::Workspace);
            self.single_side_panel(side, fallback, None, window, cx)
        } else if open_ids.len() == 1 || !self.shell.panels.multi_open {
            let panel = open_ids
                .first()
                .and_then(|id| NavItem::from_persistence_id(id))
                .or_else(|| {
                    self.shell
                        .chrome
                        .activity_bar_layout
                        .first_panel_on_side(side)
                })
                .unwrap_or(NavItem::Workspace);
            self.single_side_panel(side, panel, None, window, cx)
        } else {
            let weights: Vec<f32> = open_ids
                .iter()
                .map(|id| self.panel_stack_weight(id))
                .collect();
            let total: f64 = weights.iter().map(|weight| f64::from(*weight)).sum();
            let count = open_ids.len();
            let measurements: PanelStackMeasurements = Rc::new(RefCell::new(
                open_ids.iter().map(|id| (id.clone(), 0.)).collect(),
            ));
            let mut stack = div().size_full().flex().flex_col().min_h_0();
            for (index, panel_id) in open_ids.iter().enumerate() {
                let panel = NavItem::from_persistence_id(panel_id).unwrap_or(NavItem::Transfers);
                // Distribute only the space left by dividers. Keep total grow
                // above one even when all persisted weights are tiny.
                let grow = (f64::from(weights[index]) / total * count as f64) as f32;
                let panel_measurements = measurements.clone();
                let measurement = canvas(
                    move |bounds, _, _| {
                        panel_measurements.borrow_mut()[index].1 = f32::from(bounds.size.height);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0();
                let title = panel
                    .i18n_key()
                    .map(|key| t!(key))
                    .unwrap_or_else(|| Cow::Borrowed(panel.panel_title()));
                let actions = self.side_panel_header_actions(panel, cx);
                let palette = self.theme_palette();
                let (meta, body) = self.side_panel_content(side, panel, window, cx);
                stack = stack.child(
                    div()
                        .relative()
                        .flex_shrink(1.)
                        .flex_basis(px(0.))
                        .flex_grow(grow)
                        .min_h(px(PANEL_STACK_MIN_HEIGHT))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(measurement)
                        .child(self.side_panel_header(panel, title, meta, palette, actions))
                        .child(div().flex_1().min_h_0().overflow_hidden().child(body)),
                );
                if index + 1 < count {
                    let above = panel_id.clone();
                    let below = open_ids[index + 1].clone();
                    stack = stack.child(self.panel_stack_resize_handle(
                        side,
                        above,
                        below,
                        measurements.clone(),
                        cx,
                    ));
                }
            }
            stack
        }
    }

    fn single_side_panel(
        &mut self,
        side: PanelSide,
        panel: NavItem,
        close_button: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let title = panel
            .i18n_key()
            .map(|key| t!(key))
            .unwrap_or_else(|| Cow::Borrowed(panel.panel_title()));
        let actions = self.side_panel_header_actions(panel, cx);
        // Floating controls share the header's layout instead of covering its
        // metadata or feature actions from an absolutely positioned overlay.
        let actions = match close_button {
            None => actions,
            Some(close_button) => Some(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .max_w_full()
                    .children(actions)
                    .child(close_button)
                    .into_any_element(),
            ),
        };
        let palette = self.theme_palette();
        let (meta, body) = self.side_panel_content(side, panel, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.side_panel_header(panel, title, meta, palette, actions))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }

    fn side_panel_header(
        &self,
        panel: NavItem,
        title: impl Into<SharedString>,
        meta: SharedString,
        palette: ThemePalette,
        actions: Option<AnyElement>,
    ) -> AnyElement {
        let background = self.shell_transparent_color(palette.section_header);
        if panel == NavItem::AiAssistant {
            conversation_panel_header_with_actions(
                self.ai_header_presentation().conversation_title,
                palette,
                background,
                actions,
            )
            .into_any_element()
        } else {
            panel_header_with_actions(title, meta, palette, background, actions).into_any_element()
        }
    }

    fn side_panel_content(
        &mut self,
        side: PanelSide,
        panel: NavItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (SharedString, AnyElement) {
        match panel {
            NavItem::ActiveSessions => {
                let model = self.active_sessions_panel_model();
                let meta = SharedString::from(model.count_label());
                let body = self.active_sessions_panel(model, cx).into_any_element();
                (meta, body)
            }
            NavItem::Recording => {
                let model = self.recording_sessions_panel_model();
                let meta = SharedString::from(model.count_label());
                let body = self.recording_panel(model, cx).into_any_element();
                (meta, body)
            }
            _ => {
                let meta = self.side_panel_meta(side, panel);
                let body = match side {
                    PanelSide::Left => self.left_panel_body(panel, window, cx),
                    PanelSide::Right => self.right_panel_body(panel, window, cx),
                };
                (meta, body)
            }
        }
    }

    /// Header metadata displayed beside the panel title.
    fn side_panel_meta(&self, _side: PanelSide, panel: NavItem) -> SharedString {
        match panel {
            NavItem::Connections => SharedString::from(""),
            NavItem::AiAssistant => SharedString::from(""),
            NavItem::ActiveSessions => SharedString::from(""),
            NavItem::Transfers => SharedString::from(""),
            NavItem::Processes => self
                .session
                .active_has_inspectable_host()
                .then(|| self.remote_ops.loaded_process_count())
                .flatten()
                .map(|count| SharedString::from(count.to_string()))
                .unwrap_or_else(|| SharedString::from("")),
            NavItem::GpuMonitor => self
                .remote_ops
                .gpu_presentation()
                .data
                .filter(|overview| overview.available)
                .map(|overview| {
                    SharedString::from(format!(
                        "{} {} · {} {}",
                        t!("gpuMonitor.driver"),
                        panel_meta_version_or_dash(&overview.driver_version),
                        t!("gpuMonitor.cuda"),
                        panel_meta_version_or_dash(&overview.cuda_version)
                    ))
                })
                .unwrap_or_else(|| SharedString::from("")),
            NavItem::AscendNpuMonitor => self
                .remote_ops
                .npu_presentation()
                .data
                .filter(|overview| overview.available)
                .map(|overview| {
                    SharedString::from(format!(
                        "{} {} · {} {}",
                        t!("ascendNpuMonitor.driver"),
                        panel_meta_version_or_dash(&overview.driver_version),
                        t!("ascendNpuMonitor.cann"),
                        panel_meta_version_or_dash(&overview.cann_version)
                    ))
                })
                .unwrap_or_else(|| SharedString::from("")),
            NavItem::Docker => {
                if !self.session.active_has_inspectable_host() {
                    return SharedString::from("");
                }
                let Some(version) = self.remote_ops.docker_engine_version() else {
                    return SharedString::from("");
                };
                SharedString::from(format!("Engine {}", truncate_preview(&version, 24)))
            }
            NavItem::Recording => SharedString::from(""),
            NavItem::SyncBackupHistory => SharedString::from(""),
            _ => SharedString::from(""),
        }
    }

    fn side_panel_header_actions(
        &mut self,
        panel: NavItem,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let count = match panel {
            NavItem::Connections if !self.connection_state.connections().is_empty() => {
                Some(self.connection_state.connections().len())
            }
            NavItem::Tunnels => Some(match self.connection_state.network_active_tab() {
                NetworkTab::Tunnels => self.tunnel_state.tunnels().len(),
                NetworkTab::Proxies => self.tunnel_state.proxies().len(),
            }),
            NavItem::SecurityAuth => Some(match self.security.auth_tab() {
                SecurityAuthTab::Keys => self.security.ssh_keys().len(),
                SecurityAuthTab::Passwords => self.security.passwords().len(),
                SecurityAuthTab::Credentials => self.security.credentials().len(),
                SecurityAuthTab::Otp => self.security.otp_entries().len(),
                SecurityAuthTab::KnownHosts => self.security.known_hosts().len(),
            }),
            _ => None,
        };
        if let Some(count) = count {
            return Some(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(self.theme_palette().text_dimmed))
                    .child(count.to_string())
                    .into_any_element(),
            );
        }
        match panel {
            NavItem::Transfers => {
                let can_browse = self.session.active_id().is_some_and(|session_id| {
                    self.session
                        .file_browser_backend_support_for_session(session_id)
                        .is_some()
                        && !self.session.is_disconnected(session_id)
                });
                can_browse.then(|| {
                    let palette = self.theme_palette();
                    let sudo = self.session.active_id().and_then(|session| {
                        let status = self.file_sudo_status(session);
                        (self.file_sudo_available(session) || status.is_sudo()).then(|| {
                            let session = session.to_owned();
                            NyaButton::new("transfer-sudo", "")
                                .icon("icons/shield.svg")
                                .variant(NyaButtonVariant::Ghost)
                                .small()
                                .compact()
                                .height(px(24.))
                                .text_color(rgb(if status.is_sudo() {
                                    palette.warning
                                } else {
                                    palette.text_muted
                                }))
                                .tooltip(crate::features::transfers::privilege_runtime::sudo_label(
                                    status,
                                ))
                                .disabled(status.is_busy())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_file_sudo(session.clone(), window, cx);
                                }))
                        })
                    });
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .children(sudo)
                        .child(header_svg_icon_button(
                            palette,
                            "transfer-browser-open-file-manager",
                            "icons/file-manager.svg",
                            t!("fileManager.openWorkspace"),
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.open_file_manager_tab(None, cx);
                            }),
                        ))
                        .into_any_element()
                })
            }
            NavItem::AiAssistant => {
                let palette = self.theme_palette();
                let ai_running = self.ai.chat_or_agent_is_running();
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        // Header triggers are part of their open popover. Keep
                        // their mouse-down away from the root outside-click
                        // handler so a second click can toggle the menu closed.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(header_svg_icon_button(
                            palette,
                            "ai-header-history-toggle",
                            "icons/ai/history.svg",
                            t!("ai.history"),
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.toggle_ai_history(window, cx);
                            }),
                        ))
                        .child(header_svg_icon_button(
                            palette,
                            "ai-header-open-settings",
                            "icons/ai/settings.svg",
                            t!("ai.settings"),
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.ai.close_transient_menus();
                                this.shell.navigation.settings.active_tab = SettingsTab::AiGeneral;
                                this.open_page(NavItem::Settings, cx);
                            }),
                        ))
                        .child(header_svg_icon_button(
                            palette,
                            "ai-header-new-chat",
                            "icons/ai/new.svg",
                            t!("ai.newChat"),
                            !ai_running,
                            cx.listener(|this, _, _, cx| {
                                this.start_new_ai_chat(cx);
                            }),
                        ))
                        .into_any_element(),
                )
            }
            NavItem::Ports | NavItem::Services => {
                let palette = self.theme_palette();
                let kind = if panel == NavItem::Ports {
                    HostToolKind::Ports
                } else {
                    HostToolKind::Services
                };
                Some(
                    header_svg_icon_button(
                        palette,
                        "host-tools-header-refresh",
                        "icons/fe/refresh.svg",
                        t!("common.refresh"),
                        self.host_tool_panels.entity(kind).read(cx).can_refresh(),
                        cx.listener(move |this, _, _window, cx| {
                            this.refresh_host_tool(kind, cx);
                        }),
                    )
                    .into_any_element(),
                )
            }
            NavItem::Stats => {
                let palette = self.theme_palette();
                let manual_refreshing = self.remote_ops.stats_manual_refreshing();
                let can_refresh = self.session.active_has_inspectable_host()
                    && !self.remote_ops.stats_is_pending();
                Some(
                    header_svg_icon_button(
                        palette,
                        "stats-header-refresh",
                        if manual_refreshing {
                            "icons/conn/spinner-arc.svg"
                        } else {
                            "icons/fe/refresh.svg"
                        },
                        t!("resourceMonitor.refresh"),
                        can_refresh,
                        cx.listener(|this, _, _window, cx| {
                            this.refresh_stats(cx);
                            this.defer_remote_panel_snapshot_flush(cx);
                        }),
                    )
                    .into_any_element(),
                )
            }
            NavItem::GpuMonitor => {
                let palette = self.theme_palette();
                let can_refresh = self.session.active_has_inspectable_host()
                    && !self.local_host_tools_unsupported()
                    && !self.remote_ops.gpu_is_pending();
                Some(
                    header_svg_icon_button(
                        palette,
                        "gpu-header-refresh",
                        "icons/fe/refresh.svg",
                        t!("resourceMonitor.refresh"),
                        can_refresh,
                        cx.listener(|this, _, _window, cx| {
                            this.refresh_gpu(cx);
                            this.defer_remote_panel_snapshot_flush(cx);
                        }),
                    )
                    .into_any_element(),
                )
            }
            NavItem::AscendNpuMonitor => {
                let palette = self.theme_palette();
                let can_refresh = self.session.active_has_inspectable_host()
                    && !self.local_host_tools_unsupported()
                    && !self.remote_ops.npu_is_pending();
                Some(
                    header_svg_icon_button(
                        palette,
                        "npu-header-refresh",
                        "icons/fe/refresh.svg",
                        t!("resourceMonitor.refresh"),
                        can_refresh,
                        cx.listener(|this, _, _window, cx| {
                            this.refresh_npu(cx);
                            this.defer_remote_panel_snapshot_flush(cx);
                        }),
                    )
                    .into_any_element(),
                )
            }
            NavItem::Processes => {
                let palette = self.theme_palette();
                let can_refresh = self.session.active_has_inspectable_host()
                    && !self.remote_ops.process_is_pending();
                Some(
                    header_svg_icon_button(
                        palette,
                        "process-header-refresh",
                        "icons/fe/refresh.svg",
                        t!("common.refresh"),
                        can_refresh,
                        cx.listener(|this, _, _window, cx| {
                            this.refresh_processes(cx);
                            this.defer_remote_panel_snapshot_flush(cx);
                        }),
                    )
                    .into_any_element(),
                )
            }
            NavItem::Docker => {
                let palette = self.theme_palette();
                let can_refresh = self.session.active_has_inspectable_host()
                    && !self.local_host_tools_unsupported()
                    && !self.remote_ops.docker_is_pending();
                let can_prune = can_refresh && self.remote_ops.docker_can_prune();
                let more_label = t!("dockerManager.moreActions").to_string();
                let prune_label = t!("dockerManager.prune");
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(header_svg_icon_button(
                            palette,
                            "docker-header-refresh",
                            "icons/fe/refresh.svg",
                            t!("common.refresh"),
                            can_refresh,
                            cx.listener(|this, _, _window, cx| {
                                this.refresh_docker(cx);
                                this.defer_remote_panel_snapshot_flush(cx);
                            }),
                        ))
                        .child(
                            div()
                                .relative()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(header_svg_icon_button(
                                    palette,
                                    "docker-header-more",
                                    "icons/session/more.svg",
                                    more_label,
                                    can_prune,
                                    cx.listener(|this, _, _, cx| {
                                        this.remote_ops.toggle_docker_header_menu();
                                        this.defer_remote_panel_snapshot_flush(cx);
                                        cx.notify();
                                    }),
                                ))
                                .when(self.remote_ops.docker_header_menu_open(), |this| {
                                    this.child(
                                        div()
                                            .id("docker-header-more-menu")
                                            .absolute()
                                            .top(px(30.))
                                            .right_0()
                                            .w(px(160.))
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(palette.border))
                                            .bg(self.shell_surface_color(palette.surface))
                                            .shadow_lg()
                                            .py_1()
                                            .child(
                                                div()
                                                    .id("docker-header-prune")
                                                    .h(px(30.))
                                                    .px_3()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(palette.danger))
                                                    .cursor_pointer()
                                                    .hover(|this| this.bg(rgb(palette.hover)))
                                                    .child(
                                                        svg()
                                                            .size(px(14.))
                                                            .flex_none()
                                                            .path("icons/fe/delete.svg")
                                                            .text_color(rgb(palette.danger)),
                                                    )
                                                    .child(prune_label)
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.remote_ops.close_docker_menus();
                                                            this.prune_docker_system(window, cx);
                                                            this.defer_remote_panel_snapshot_flush(
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            ),
                                    )
                                }),
                        )
                        .into_any_element(),
                )
            }
            NavItem::SyncBackupHistory => {
                let palette = self.theme_palette();
                let enabled = self.cloud_sync.settings().enabled && !self.cloud_sync.job_running();
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(header_svg_icon_button(
                            palette,
                            "sync-history-push-now",
                            "icons/fe/upload.svg",
                            t!("settings.syncPushNow"),
                            enabled,
                            cx.listener(|this, _, window, cx| {
                                this.prompt_provider_cloud_sync_push(window, cx);
                            }),
                        ))
                        .child(header_svg_icon_button(
                            palette,
                            "sync-history-pull-now",
                            "icons/fe/download.svg",
                            t!("settings.syncPullNow"),
                            enabled,
                            cx.listener(|this, _, window, cx| {
                                this.prompt_provider_cloud_sync_pull(window, cx);
                            }),
                        ))
                        .child(header_svg_icon_button(
                            palette,
                            "sync-history-header-refresh",
                            "icons/fe/refresh.svg",
                            t!("resourceMonitor.refresh"),
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.queue_cloud_sync_history_refresh(None, cx);
                                this.shell
                                    .set_status("refreshing cloud sync history".to_string());
                                cx.notify();
                            }),
                        ))
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    fn panel_stack_resize_handle(
        &self,
        side: PanelSide,
        above_id: String,
        below_id: String,
        measurements: PanelStackMeasurements,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let above = above_id.clone();
        let below = below_id.clone();
        let id = SharedString::from(format!(
            "panel-stack-resize-{}-{}-{}",
            match side {
                PanelSide::Left => "left",
                PanelSide::Right => "right",
            },
            above_id,
            below_id
        ));
        let hover_id = id.clone();
        let drag_id = id.clone();
        deferred(
            crate::features::view_widgets::horizontal_resize_handle_visual(
                palette,
                self.shell
                    .panels
                    .stack_resize
                    .as_ref()
                    .is_some_and(|resize| {
                        resize.side == side
                            && resize.above_id == above_id
                            && resize.below_id == below_id
                    }),
                self.shell.resize_handle_is_highlighted(&id),
            )
            .id(id.clone())
            .debug_selector(move || id.to_string())
            .cursor_row_resize()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.update_resize_handle_hover(hover_id.clone(), *hovered, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.activate_resize_handle_immediately(drag_id.clone(), cx);
                    this.start_panel_stack_resize(
                        side,
                        above.clone(),
                        below.clone(),
                        event,
                        &measurements.borrow(),
                        cx,
                    );
                }),
            ),
        )
    }
}

fn panel_meta_version_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        truncate_preview(value, 24)
    }
}

fn header_svg_icon_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<String>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    header_svg_icon_button_with_color(palette, id, icon_path, tooltip, None, enabled, on_click)
}

fn header_svg_icon_button_with_color(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<String>,
    icon_color: Option<u32>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip = tooltip.into();
    let id = id.into();
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(if enabled {
            icon_color.unwrap_or(palette.text_muted)
        } else {
            palette.text_dimmed
        }))
        .when(enabled, |this| {
            this.cursor_pointer().hover(|this| {
                this.bg(rgb(palette.surface_elevated))
                    .text_color(rgb(palette.text))
            })
        })
        .when(!enabled, |this| this.opacity(0.45))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(if enabled {
                    icon_color.unwrap_or(palette.text_muted)
                } else {
                    palette.text_dimmed
                })),
        )
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

#[cfg(test)]
mod tests {
    mod floating;

    use gpui::{
        Context, Entity, InteractiveElement as _, IntoElement, Modifiers, MouseButton,
        MouseMoveEvent, ParentElement as _, Render, Styled as _, TestAppContext, Window, div,
        point, px,
    };

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{NavItem, PanelOpenMode, PanelSide, SettingsTab};
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    use super::{SidePanelStackRenderMode, side_panel_stack_render_mode};

    struct FloatingPanelFixture {
        app: Entity<NyaTermApp>,
        width: f32,
        side: PanelSide,
    }

    impl Render for FloatingPanelFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let panel = self.app.update(cx, |app, cx| {
                app.floating_side_panel(self.side, NavItem::AiAssistant, window, cx)
                    .into_any_element()
            });
            div().size_full().child(
                div()
                    .debug_selector(|| "floating-panel-fixture".into())
                    .w(px(self.width))
                    .h_full()
                    .child(panel),
            )
        }
    }

    struct PanelStackFixture {
        app: Entity<NyaTermApp>,
        side: PanelSide,
        height: f32,
    }

    struct AiPanelHeaderFixture {
        app: Entity<NyaTermApp>,
        side: PanelSide,
        width: f32,
    }

    impl Render for AiPanelHeaderFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let panel = self.app.update(cx, |app, cx| {
                app.single_side_panel(self.side, NavItem::AiAssistant, None, window, cx)
            });
            div().size_full().child(
                div()
                    .debug_selector(|| "ai-panel-header-fixture".into())
                    .w(px(self.width))
                    .h_full()
                    .child(panel),
            )
        }
    }

    #[test]
    fn ai_panel_titles_reserve_space_for_clickable_actions_at_narrow_widths() {
        let test_dir = TestConfigDir::new("nyaterm-ai-panel-header");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, test_dir.path(), "header-test");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            cx.observe(&app, |_, _, cx| cx.notify()).detach();
            AiPanelHeaderFixture {
                app: app.clone(),
                side: PanelSide::Right,
                width: 320.,
            }
        });

        for side in [PanelSide::Left, PanelSide::Right] {
            for width in [160., 200., 320., 600.] {
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        app.sync_ai_active_scope(cx);
                        app.ai.start_new_chat();
                        app.ai.begin_chat_request(
                            "MixedCase 中文 👩🏽‍💻 long conversation title ".repeat(40),
                            nyaterm_core::AiMode::Ask,
                            None,
                        );
                        app.ai.cancel_chat_and_agent();
                    });
                    fixture.update(cx, |fixture, cx| {
                        fixture.width = width;
                        fixture.side = side;
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));

                let panel = cx.debug_bounds("ai-panel-header-fixture").unwrap();
                let title = cx.debug_bounds("ai-conversation-title").unwrap();
                assert!(title.left() >= panel.left());
                let mut previous_right = title.right();
                for id in [
                    "ai-header-history-toggle",
                    "ai-header-open-settings",
                    "ai-header-new-chat",
                ] {
                    let action = cx.debug_bounds(id).unwrap();
                    assert!(
                        previous_right <= action.left(),
                        "title and actions must not overlap"
                    );
                    assert!(action.right() <= panel.right());
                    previous_right = action.right();
                }

                let history = cx.debug_bounds("ai-header-history-toggle").unwrap();
                cx.simulate_click(history.center(), Modifiers::default());
                cx.update(|_, cx| assert!(app.read(cx).ai.history_is_open()));
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));
                cx.simulate_click(history.center(), Modifiers::default());
                cx.update(|_, cx| assert!(!app.read(cx).ai.history_is_open()));
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));

                let settings = cx.debug_bounds("ai-header-open-settings").unwrap();
                cx.simulate_click(settings.center(), Modifiers::default());
                cx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx).shell.navigation.settings.active_tab,
                        SettingsTab::AiGeneral
                    );
                });
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));

                let new_chat = cx.debug_bounds("ai-header-new-chat").unwrap();
                cx.simulate_click(new_chat.center(), Modifiers::default());
                cx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx).ai_header_presentation().conversation_title,
                        rust_i18n::t!("ai.newChat")
                    );
                });
            }
        }
    }

    impl Render for PanelStackFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let panel = self.app.update(cx, |app, cx| {
                app.side_panel_stack(self.side, window, cx)
                    .into_any_element()
            });
            div()
                .size_full()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.app.update(cx, |app, cx| {
                        if event.dragging() {
                            app.update_panel_stack_resize(event, cx);
                        }
                    });
                    cx.notify();
                }))
                .child(div().w(px(320.)).h(px(self.height)).child(panel))
        }
    }

    #[test]
    fn panel_stack_resize_tracks_real_layout_with_cached_connections_and_monitor() {
        let test_dir = TestConfigDir::new("nyaterm-panel-stack-resize");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, test_dir.path(), "resize-test");
        app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            let ids = vec![
                "savedConnections".to_string(),
                "resourceMonitor".to_string(),
            ];
            app.shell.panels.multi_open = true;
            app.shell.panels.active_left = Some(NavItem::Connections);
            app.shell.panels.active_right = Some(NavItem::Connections);
            app.shell.panels.left_open = ids.clone();
            app.shell.panels.right_open = ids.clone();
            app.shell.chrome.activity_bar_layout.left_top = ids.clone();
            app.shell.chrome.activity_bar_layout.right_top = ids;
            app.shell.chrome.activity_bar_layout.left_bottom.clear();
            app.shell.chrome.activity_bar_layout.right_bottom.clear();
        });
        let (fixture, cx) = cx.add_window_view(|_, _| PanelStackFixture {
            app: app.clone(),
            side: PanelSide::Left,
            height: 301.,
        });

        for (side, divider_id) in [
            (
                PanelSide::Left,
                "panel-stack-resize-left-savedConnections-resourceMonitor",
            ),
            (
                PanelSide::Right,
                "panel-stack-resize-right-savedConnections-resourceMonitor",
            ),
        ] {
            for (height, above_weight, below_weight) in [
                (301., 0.02, 0.03),
                (901., 0.02, 0.03),
                (301., 0.001, 1.),
                (97., 1., 1.),
            ] {
                cx.update(|_, cx| {
                    app.update(cx, |app, _| {
                        app.shell
                            .panels
                            .stack_sizes
                            .insert("savedConnections".into(), above_weight);
                        app.shell
                            .panels
                            .stack_sizes
                            .insert("resourceMonitor".into(), below_weight);
                    });
                    fixture.update(cx, |fixture, cx| {
                        fixture.side = side;
                        fixture.height = height;
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));
                let divider = cx.debug_bounds(divider_id).unwrap();
                let start = divider.center();
                cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
                cx.update(|window, cx| {
                    let state = app.read(cx).shell.panels.stack_resize.as_ref().unwrap();
                    // The divider occupies one layout pixel (5px hitbox minus margins).
                    assert!(
                        (state.pair_height - f64::from(height - 1.)).abs() < 1.,
                        "{side:?} {height}px: measured pair height {}",
                        state.pair_height,
                    );
                    fixture.update(cx, |_, cx| cx.notify());
                    _ = window.draw(cx);
                });
                let after_press = cx.debug_bounds(divider_id).unwrap().center();
                assert!(
                    (f32::from(after_press.y - start.y)).abs() < 1.,
                    "{side:?} {height}px: divider jumped on press",
                );

                let delta = if height > 97. { 30. } else { 0. };
                let target = point(start.x, start.y + px(30.));
                cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
                cx.update(|window, cx| _ = window.draw(cx));
                let after_move = cx.debug_bounds(divider_id).unwrap().center();
                assert!(
                    (f32::from(after_move.y - start.y) - delta).abs() < 1.,
                    "{side:?} {height}px: divider moved {}px for a 30px drag",
                    f32::from(after_move.y - start.y),
                );
                cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
                cx.update(|_, cx| {
                    app.update(cx, |app, _| {
                        assert!(app.shell.panels.finish_stack_resize());
                    });
                });
            }
        }
    }

    #[test]
    fn floating_panel_close_stays_clickable_without_covering_actions_at_narrow_widths() {
        let test_dir = TestConfigDir::new("nyaterm-floating-panel-header");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, test_dir.path(), "header-test");
        app.update(&mut cx, |app, cx| {
            app.sync_component_theme(cx);
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            app.sync_ai_active_scope(cx);
            app.ai.begin_chat_request(
                "MixedCase 中文 👩🏽‍💻 long conversation title ".repeat(40),
                nyaterm_core::AiMode::Ask,
                None,
            );
            app.ai.cancel_chat_and_agent();
        });
        let (fixture, cx) = cx.add_window_view(|_, _| FloatingPanelFixture {
            app: app.clone(),
            width: 320.,
            side: PanelSide::Left,
        });

        for (side, close_id) in [
            (PanelSide::Left, "floating-left-panel-close"),
            (PanelSide::Right, "floating-right-panel-close"),
        ] {
            for width in [160., 200., 320., 600.] {
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        app.toggle_floating_panel(NavItem::AiAssistant, side, cx);
                    });
                    fixture.update(cx, |fixture, cx| {
                        fixture.width = width;
                        fixture.side = side;
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    _ = window.draw(cx);
                });

                let panel = cx.debug_bounds("floating-panel-fixture").unwrap();
                let close = cx.debug_bounds(close_id).unwrap();
                let title = cx.debug_bounds("ai-conversation-title").unwrap();
                assert!(title.left() >= panel.left() && title.right() <= close.left());
                assert!(close.left() >= panel.left() && close.right() <= panel.right());
                assert!(
                    cx.debug_bounds("ai-header-execution-mode-toggle").is_none(),
                    "execution mode lives in the composer, not the header"
                );
                for id in [
                    "ai-header-history-toggle",
                    "ai-header-open-settings",
                    "ai-header-new-chat",
                ] {
                    let action = cx.debug_bounds(id).unwrap();
                    assert!(
                        action.left() >= panel.left() && action.right() <= panel.right(),
                        "{id} must stay inside a {width}px panel"
                    );
                    assert!(title.right() <= action.left(), "title must not cover {id}");
                    assert!(
                        action.right() <= close.left()
                            || close.right() <= action.left()
                            || action.bottom() <= close.top()
                            || close.bottom() <= action.top(),
                        "{id} must not overlap close at {width}px"
                    );
                }
                cx.simulate_click(close.center(), Modifiers::default());
                cx.update(|_, cx| {
                    assert_eq!(app.read(cx).shell.floating_panel(side), None);
                });
            }
        }
    }

    #[test]
    fn exclusive_panel_overlay_takes_precedence_over_stack_construction() {
        assert_eq!(
            side_panel_stack_render_mode(Some(NavItem::AiAssistant)),
            SidePanelStackRenderMode::Overlay(NavItem::AiAssistant)
        );
        assert_eq!(
            side_panel_stack_render_mode(None),
            SidePanelStackRenderMode::Stack
        );
    }

    #[test]
    fn floating_panel_visibility_is_independent_of_hidden_activity_rail() {
        let test_dir = TestConfigDir::new("nyaterm-floating-monitor-demand");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, test_dir.path(), "monitor-test");
        app.update(&mut cx, |app, cx| {
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            let ids = app
                .shell
                .chrome
                .activity_bar_layout
                .right_top
                .iter()
                .chain(&app.shell.chrome.activity_bar_layout.right_bottom)
                .cloned()
                .collect::<Vec<_>>();
            for id in ids {
                app.shell.chrome.activity_bar_layout.hide_entry(&id);
            }
            assert!(
                !app.activity_side_has_items(crate::features::runtime_jobs::ActivitySide::Right)
            );
            app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
            assert!(app.panel_is_rendered(NavItem::GpuMonitor));
            app.close_floating_panel(PanelSide::Right, cx);
            assert!(!app.panel_is_rendered(NavItem::GpuMonitor));
        });
    }
}

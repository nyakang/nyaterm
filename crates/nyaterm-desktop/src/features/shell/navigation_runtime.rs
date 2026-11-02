use gpui::Context;

use crate::features::NyaTermApp;
use crate::models::{NavItem, PanelSide, RightFocus};

impl NyaTermApp {
    pub(in crate::features) fn open_page(&mut self, item: NavItem, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        self.hide_secret_inputs(cx);
        if item == NavItem::Settings || item.opens_settings() {
            if let Some(controller) = self
                .desktop_controller
                .as_ref()
                .and_then(|controller| controller.upgrade())
            {
                let workspace_id = self.workspace_id;
                controller.update(cx, |controller, cx| {
                    controller.activate_or_claim_settings(workspace_id, cx)
                });
            }
            self.begin_settings_draft(cx);
            // Inputs are built where a tab is revealed, never in its render.
            let tab = self.shell.navigation.settings.active_tab;
            self.ensure_settings_tab_inputs(tab, cx);
            if let Some(group) = self
                .shell
                .navigation
                .settings
                .active_tab
                .expandable_group_id()
            {
                self.shell
                    .navigation
                    .settings
                    .expanded_groups
                    .insert(group.to_string());
            }
            // Settings is an ordinary workspace tab. Ending the draft (apply
            // and close, cancel, or the tab close button) removes the tab;
            // every session keeps running exactly as before it was opened.
            self.shell.navigation.selected_nav = NavItem::Settings;
            self.transfer_browser_visibility_changed(browser_was_visible, cx);
            self.shell.set_status("settings opened".to_string());
            self.open_settings_workspace_tab(cx);
            return;
        }

        self.open_panel(item, cx);
    }

    pub(in crate::features) fn open_panel(&mut self, item: NavItem, cx: &mut Context<Self>) {
        if item == NavItem::Transfers {
            self.note_file_manager_manual_sidebar_open();
        }
        let browser_was_visible = self.transfer_browser_is_visible();
        self.hide_secret_inputs(cx);
        if item == NavItem::Settings || item.opens_settings() {
            self.open_page(NavItem::Settings, cx);
            return;
        }

        if item == NavItem::Notes {
            self.load_notes_if_needed(cx);
        }

        if self.shell.panel_is_floating()
            && let Some(side) = self.panel_side_for_item(item)
        {
            self.toggle_floating_panel(item, side, cx);
            return;
        }

        if self.shell.panels.multi_open && self.panel_side_for_item(item).is_some() {
            self.open_or_toggle_panel(item, cx);
            return;
        }

        self.shell.navigation.selected_nav = item;
        self.shell.panels.right_focus = if item == NavItem::Recording {
            RightFocus::Recording
        } else {
            RightFocus::Default
        };

        match self.panel_side_for_item(item) {
            Some(PanelSide::Left) => {
                let already_open = self.shell.panels.active_left == Some(item)
                    && !self.shell.panels.left_collapsed;
                if already_open {
                    self.shell.panels.left_collapsed = true;
                    self.shell.panels.active_left = None;
                    self.shell.set_status(format!("{} closed", item.label()));
                } else {
                    self.shell.panels.active_left = Some(item);
                    self.shell.panels.left_collapsed = false;
                    self.shell.set_status(format!("{} opened", item.label()));
                }
            }
            Some(PanelSide::Right) => {
                let already_open = self.shell.panels.active_right == Some(item)
                    && !self.shell.panels.right_collapsed;
                if already_open {
                    self.shell.panels.right_collapsed = true;
                    self.shell.panels.active_right = None;
                    self.shell.panels.right_focus = RightFocus::Default;
                    self.shell.set_status(format!("{} closed", item.label()));
                } else {
                    self.shell.panels.active_right = Some(item);
                    self.shell.panels.right_collapsed = false;
                    self.shell.set_status(format!("{} opened", item.label()));
                }
            }
            None => {
                self.shell.panels.left_collapsed = false;
                self.shell.panels.right_collapsed = false;
            }
        }

        self.persist_ui_layout();
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        cx.notify();
        // Publish the new panel visibility and browser state.
        self.flush_transfer_panel_snapshot(cx);
    }

    pub(in crate::features) fn ensure_panel_open(&mut self, item: NavItem, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panel_is_floating()
            && let Some(side) = self.panel_side_for_item(item)
        {
            self.shell.panels.set_floating_panel(side, Some(item));
            self.transfer_browser_visibility_changed(browser_was_visible, cx);
            return;
        }
        if self.shell.panels.multi_open && self.panel_side_for_item(item).is_some() {
            self.ensure_panel_in_stack(item, cx);
            return;
        }
        self.shell.navigation.selected_nav = item;
        match self.panel_side_for_item(item) {
            Some(PanelSide::Left) => {
                self.shell.panels.active_left = Some(item);
                self.shell.panels.left_collapsed = false;
            }
            Some(PanelSide::Right) => {
                self.shell.panels.active_right = Some(item);
                self.shell.panels.right_collapsed = false;
                self.shell.panels.right_focus = if item == NavItem::Recording {
                    RightFocus::Recording
                } else {
                    RightFocus::Default
                };
            }
            None => {}
        }
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
    }

    pub(in crate::features) fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.multi_open {
            if self.left_side_open() {
                self.shell.panels.left_open.clear();
                self.shell.panels.active_left = None;
                self.shell.panels.left_collapsed = true;
                self.shell.set_status("left sidebar collapsed".to_string());
            } else if let Some(panel) = self
                .shell
                .chrome
                .activity_bar_layout
                .first_panel_on_side(PanelSide::Left)
            {
                self.shell.panels.active_left = Some(panel);
                self.shell.panels.left_open.clear();
                if Self::is_stackable_panel_id(panel.persistence_id()) {
                    self.shell
                        .panels
                        .left_open
                        .push(panel.persistence_id().to_string());
                }
                self.shell.panels.left_collapsed = false;
                self.shell.set_status("left sidebar expanded".to_string());
            }
        } else if self.shell.panels.left_collapsed || self.shell.panels.active_left.is_none() {
            self.shell.panels.active_left = self
                .shell
                .chrome
                .activity_bar_layout
                .first_panel_on_side(PanelSide::Left);
            self.shell.panels.left_collapsed = false;
            self.shell.set_status("left sidebar expanded".to_string());
        } else {
            self.shell.panels.active_left = None;
            self.shell.panels.left_collapsed = true;
            self.shell.set_status("left sidebar collapsed".to_string());
        }
        self.persist_ui_layout();
        self.ensure_visible_notes_loaded(cx);
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_right_inspector(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.multi_open {
            if self.right_side_open() {
                self.shell.panels.right_open.clear();
                self.shell.panels.active_right = None;
                self.shell.panels.right_collapsed = true;
                self.shell.set_status("right sidebar collapsed".to_string());
            } else if let Some(panel) = self
                .shell
                .chrome
                .activity_bar_layout
                .first_panel_on_side(PanelSide::Right)
            {
                self.shell.panels.active_right = Some(panel);
                self.shell.panels.right_open.clear();
                if Self::is_stackable_panel_id(panel.persistence_id()) {
                    self.shell
                        .panels
                        .right_open
                        .push(panel.persistence_id().to_string());
                }
                self.shell.panels.right_collapsed = false;
                self.shell.set_status("right sidebar expanded".to_string());
            }
        } else if self.shell.panels.right_collapsed || self.shell.panels.active_right.is_none() {
            self.shell.panels.active_right = self
                .shell
                .chrome
                .activity_bar_layout
                .first_panel_on_side(PanelSide::Right);
            self.shell.panels.right_collapsed = false;
            self.shell.set_status("right sidebar expanded".to_string());
        } else {
            self.shell.panels.active_right = None;
            self.shell.panels.right_collapsed = true;
            self.shell.set_status("right sidebar collapsed".to_string());
        }
        self.persist_ui_layout();
        self.ensure_visible_notes_loaded(cx);
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_mobile_left_drawer(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.mobile_left_open {
            self.shell.panels.mobile_left_open = false;
        } else {
            if self.shell.panels.active_left.is_none() {
                self.shell.panels.active_left = self
                    .shell
                    .chrome
                    .activity_bar_layout
                    .first_panel_on_side(PanelSide::Left);
            }
            self.shell.panels.left_collapsed = false;
            self.shell.panels.mobile_left_open = true;
        }
        self.ensure_visible_notes_loaded(cx);
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_mobile_right_drawer(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        if self.shell.panels.mobile_right_open {
            self.shell.panels.mobile_right_open = false;
        } else {
            if self.shell.panels.active_right.is_none() {
                self.shell.panels.active_right = self
                    .shell
                    .chrome
                    .activity_bar_layout
                    .first_panel_on_side(PanelSide::Right);
            }
            self.shell.panels.right_collapsed = false;
            self.shell.panels.mobile_right_open = true;
        }
        self.ensure_visible_notes_loaded(cx);
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        cx.notify();
    }

    pub(in crate::features) fn current_left_panel(&self) -> Option<NavItem> {
        if self.shell.panels.left_collapsed {
            return None;
        }
        if self.shell.panels.multi_open {
            if self.side_overlay_panel(PanelSide::Left).is_some()
                || !self.side_open_panel_ids(PanelSide::Left).is_empty()
            {
                return self
                    .side_overlay_panel(PanelSide::Left)
                    .or(self.shell.panels.active_left)
                    .or_else(|| {
                        self.side_open_panel_ids(PanelSide::Left)
                            .first()
                            .and_then(|id| NavItem::from_persistence_id(id))
                    });
            }
            return None;
        }
        self.shell.panels.active_left
    }

    pub(in crate::features) fn current_right_panel(&self) -> Option<NavItem> {
        if self.shell.panels.right_collapsed {
            return None;
        }
        if self.shell.panels.multi_open {
            if self.side_overlay_panel(PanelSide::Right).is_some()
                || !self.side_open_panel_ids(PanelSide::Right).is_empty()
            {
                return self
                    .side_overlay_panel(PanelSide::Right)
                    .or(self.shell.panels.active_right)
                    .or_else(|| {
                        self.side_open_panel_ids(PanelSide::Right)
                            .first()
                            .and_then(|id| NavItem::from_persistence_id(id))
                    })
                    .or(if self.shell.panels.right_focus == RightFocus::Recording {
                        Some(NavItem::Recording)
                    } else {
                        None
                    });
            }
            return if self.shell.panels.right_focus == RightFocus::Recording {
                Some(NavItem::Recording)
            } else {
                None
            };
        }
        self.shell.panels.active_right.or(
            if self.shell.panels.right_focus == RightFocus::Recording {
                Some(NavItem::Recording)
            } else {
                None
            },
        )
    }

    pub(in crate::features) fn left_side_open(&self) -> bool {
        self.current_left_panel().is_some()
            || (self.shell.panels.multi_open
                && !self.side_open_panel_ids(PanelSide::Left).is_empty())
    }

    pub(in crate::features) fn right_side_open(&self) -> bool {
        self.current_right_panel().is_some()
            || (self.shell.panels.multi_open
                && !self.side_open_panel_ids(PanelSide::Right).is_empty())
    }
}

use gpui::{
    Context, IntoElement, MouseDownEvent, ParentElement as _, Render, Styled as _, Window, div, px,
    rgb, rgba,
};
use nyaterm_core::truncate_preview;
use rust_i18n::t;

use crate::features::{NyaTermApp, runtime_jobs::ActivitySide, view_widgets::mono_icon};
use crate::models::{
    ActivityBarContextMenuState, ActivityBarContextTarget, ActivityBarEntry,
    ActivityBarLayoutState, ActivityBarZone, BottomPanelMode, NavItem, PanelSide,
};

#[derive(Clone, Debug)]
pub(in crate::features) struct ActivityBarDragPayload {
    pub entry_id: String,
    pub label: String,
    /// The dragged entry's own icon, so the preview reads as the thing being moved.
    pub icon_path: &'static str,
}

pub(in crate::features) struct ActivityBarDragPreview {
    payload: ActivityBarDragPayload,
    position: gpui::Point<gpui::Pixels>,
}

impl ActivityBarDragPreview {
    pub(in crate::features) fn new(
        payload: ActivityBarDragPayload,
        position: gpui::Point<gpui::Pixels>,
    ) -> Self {
        Self { payload, position }
    }
}

impl Render for ActivityBarDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(72.))
            .pt(self.position.y - px(18.))
            .child(
                div()
                    .w(px(144.))
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0x334155))
                    .bg(rgba(0x151b24dd))
                    .shadow_lg()
                    .child(mono_icon(self.payload.icon_path, rgb(0x93c5fd).into(), 14.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(rgb(0xc9d1d9))
                            .child(truncate_preview(&self.payload.label, 16)),
                    ),
            )
    }
}

impl NyaTermApp {
    pub(in crate::features) fn activity_side_has_items(&self, side: ActivitySide) -> bool {
        let zones = match side {
            ActivitySide::Left => [ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom],
            ActivitySide::Right => [ActivityBarZone::RightTop, ActivityBarZone::RightBottom],
        };
        zones
            .into_iter()
            .any(|zone| !self.activity_entries_for_zone(zone).is_empty())
    }

    pub(in crate::features) fn activity_entries_for_zone(
        &self,
        zone: ActivityBarZone,
    ) -> Vec<ActivityBarEntry> {
        self.shell
            .chrome
            .activity_bar_layout
            .zone(zone)
            .iter()
            .filter(|id| !self.shell.chrome.activity_bar_layout.is_hidden(id))
            .filter_map(|id| ActivityBarEntry::from_persistence_id(id))
            .collect()
    }

    pub(in crate::features) fn normalize_activity_bar_layout(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for zone in ActivityBarZone::all() {
            let mut next = Vec::new();
            for raw_id in self
                .shell
                .chrome
                .activity_bar_layout
                .zone(zone)
                .iter()
                .cloned()
            {
                let id = if raw_id == "keyManagement" {
                    "securityAuth".to_string()
                } else {
                    raw_id
                };
                if id == "fileTransfer" {
                    continue;
                }
                if seen.insert(id.clone()) {
                    next.push(id);
                }
            }
            *self.shell.chrome.activity_bar_layout.zone_mut(zone) = next;
        }
        // Restore newly introduced schema ids beside their nearest default
        // anchor without disturbing the user's existing custom order or side.
        let defaults = ActivityBarLayoutState::default();
        for zone in ActivityBarZone::all() {
            for (default_index, id) in defaults.zone(zone).iter().enumerate() {
                if seen.contains(id) {
                    continue;
                }
                let target = self.shell.chrome.activity_bar_layout.zone(zone);
                let insert_at = defaults.zone(zone)[..default_index]
                    .iter()
                    .rev()
                    .find_map(|anchor| {
                        target
                            .iter()
                            .position(|entry| entry == anchor)
                            .map(|i| i + 1)
                    })
                    .or_else(|| {
                        defaults.zone(zone)[default_index + 1..]
                            .iter()
                            .find_map(|anchor| target.iter().position(|entry| entry == anchor))
                    })
                    .unwrap_or(target.len());
                self.shell
                    .chrome
                    .activity_bar_layout
                    .zone_mut(zone)
                    .insert(insert_at, id.clone());
                seen.insert(id.clone());
            }
        }
        // Hidden state is independent of zone placement, but retired, missing,
        // aliased and duplicate ids must not accumulate across upgrades.
        let mut hidden_seen = std::collections::HashSet::new();
        self.shell.chrome.activity_bar_layout.hidden_items = self
            .shell
            .chrome
            .activity_bar_layout
            .hidden_items
            .drain(..)
            .map(|id| {
                if id == "keyManagement" {
                    "securityAuth".to_string()
                } else {
                    id
                }
            })
            .filter(|id| seen.contains(id) && hidden_seen.insert(id.clone()))
            .collect();
    }

    pub(in crate::features) fn toggle_activity_bar_labels(&mut self, cx: &mut Context<Self>) {
        self.shell.chrome.activity_bar_layout.show_labels =
            !self.shell.chrome.activity_bar_layout.show_labels;
        self.shell
            .set_status(if self.shell.chrome.activity_bar_layout.show_labels {
                "activity labels shown".to_string()
            } else {
                "activity labels hidden".to_string()
            });
        self.persist_ui_layout();
        cx.notify();
    }

    pub(in crate::features) fn open_activity_bar_context_menu(
        &mut self,
        entry_id: String,
        zone: ActivityBarZone,
        index: usize,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.shell.chrome.activity_bar_context_menu = Some(ActivityBarContextMenuState {
            target: ActivityBarContextTarget::Entry {
                entry_id,
                zone,
                index,
            },
            x: event.position.x,
            y: event.position.y,
            move_submenu_open: false,
        });
        cx.notify();
    }

    /// Open the rail-level context menu (right-click on empty activity-bar space).
    pub(in crate::features) fn open_activity_bar_side_context_menu(
        &mut self,
        side: PanelSide,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.shell.chrome.activity_bar_context_menu = Some(ActivityBarContextMenuState {
            target: ActivityBarContextTarget::Bar { side },
            x: event.position.x,
            y: event.position.y,
            move_submenu_open: false,
        });
        cx.notify();
    }

    /// Hide an activity-bar entry from the rail, clearing any open panel it owns.
    pub(in crate::features) fn hide_activity_entry(
        &mut self,
        entry_id: String,
        cx: &mut Context<Self>,
    ) {
        let side = self
            .shell
            .chrome
            .activity_bar_layout
            .side_for_entry(&entry_id);
        if !self.shell.chrome.activity_bar_layout.hide_entry(&entry_id) {
            self.shell.chrome.activity_bar_context_menu = None;
            cx.notify();
            return;
        }
        if let Some(side) = side {
            self.clear_activity_entry_from_side(&entry_id, side);
        }
        self.shell.chrome.activity_bar_context_menu = None;
        self.shell.set_status(format!("{entry_id} hidden"));
        self.persist_ui_layout();
        cx.notify();
    }

    /// Unhide a previously hidden activity-bar entry.
    pub(in crate::features) fn show_activity_entry(
        &mut self,
        entry_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.shell.chrome.activity_bar_layout.show_entry(&entry_id) {
            self.shell.chrome.activity_bar_context_menu = None;
            cx.notify();
            return;
        }
        self.shell.chrome.activity_bar_context_menu = None;
        self.shell.set_status(format!("{entry_id} shown"));
        self.persist_ui_layout();
        cx.notify();
    }

    /// Reset the activity bar rail to its shipped default layout.
    pub(in crate::features) fn reset_activity_bar_layout(&mut self, cx: &mut Context<Self>) {
        self.shell.chrome.activity_bar_layout.reset_to_default();
        self.normalize_activity_bar_layout();
        // Any open panel may reference an entry that just moved sides; collapse
        // both sides to a clean state and let the user reopen from the rail.
        self.shell.panels.left_open.clear();
        self.shell.panels.right_open.clear();
        self.shell.panels.active_left = None;
        self.shell.panels.active_right = None;
        self.shell.panels.clear_floating();
        self.shell.panels.left_collapsed = true;
        self.shell.panels.right_collapsed = true;
        self.shell.chrome.activity_bar_context_menu = None;
        self.shell.set_status("activity bar reset".to_string());
        self.persist_ui_layout();
        cx.notify();
    }

    /// Prompt for confirmation before resetting the activity bar layout.
    pub(in crate::features) fn confirm_reset_activity_bar_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = t!("activityBar.resetLayoutTitle").to_string();
        let message = t!("activityBar.resetLayoutDescription").to_string();
        let action = t!("activityBar.resetLayout").to_string();
        self.open_confirm_dialog(
            (title, message, action, true, |this, _window, cx| {
                this.reset_activity_bar_layout(cx);
                true
            }),
            window,
            cx,
        );
    }

    pub(in crate::features) fn open_activity_bar_move_submenu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.shell.chrome.activity_bar_context_menu.as_mut() else {
            return;
        };
        if !menu.move_submenu_open {
            menu.move_submenu_open = true;
            cx.notify();
        }
    }

    pub(in crate::features) fn close_activity_bar_context_menu(&mut self, cx: &mut Context<Self>) {
        self.shell.chrome.activity_bar_context_menu = None;
        cx.notify();
    }

    pub(in crate::features) fn move_activity_entry(
        &mut self,
        entry_id: String,
        target_zone: ActivityBarZone,
        target_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some((source_zone, source_index)) =
            self.shell.chrome.activity_bar_layout.find_entry(&entry_id)
        else {
            self.shell.set_status("activity item not found".to_string());
            cx.notify();
            return;
        };

        if source_zone == target_zone {
            let mut visible_ids = self
                .activity_entries_for_zone(source_zone)
                .into_iter()
                .map(|entry| entry.persistence_id().to_string())
                .collect::<Vec<_>>();
            let Some(source_visible_index) = visible_ids.iter().position(|id| id == &entry_id)
            else {
                self.shell.chrome.activity_bar_context_menu = None;
                cx.notify();
                return;
            };
            let moved = visible_ids.remove(source_visible_index);
            let mut insert_at = target_index.unwrap_or(visible_ids.len());
            if source_visible_index < insert_at {
                insert_at = insert_at.saturating_sub(1);
            }
            insert_at = insert_at.min(visible_ids.len());
            visible_ids.insert(insert_at, moved);
            let visible_set = visible_ids
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>();
            self.shell.chrome.activity_bar_layout.merge_visible_reorder(
                source_zone,
                &visible_ids,
                |id| visible_set.contains(id),
            );
        } else {
            let removed = self
                .shell
                .chrome
                .activity_bar_layout
                .zone_mut(source_zone)
                .remove(source_index);
            let visible_target_ids = self
                .activity_entries_for_zone(target_zone)
                .into_iter()
                .map(|entry| entry.persistence_id().to_string())
                .collect::<Vec<_>>();
            let insert_at = target_index
                .and_then(|index| visible_target_ids.get(index))
                .and_then(|target_id| {
                    self.shell
                        .chrome
                        .activity_bar_layout
                        .zone(target_zone)
                        .iter()
                        .position(|id| id == target_id)
                })
                .unwrap_or_else(|| {
                    self.shell
                        .chrome
                        .activity_bar_layout
                        .zone(target_zone)
                        .len()
                });
            self.shell
                .chrome
                .activity_bar_layout
                .zone_mut(target_zone)
                .insert(insert_at, removed);
        }

        // Moving an open panel across sides closes its docked source. A
        // transient floating selection follows the item and replaces any panel
        // already floating on the destination side.
        let source_side = Self::activity_zone_side(source_zone);
        let target_side = Self::activity_zone_side(target_zone);
        if source_side != target_side {
            let moved_floating = self
                .shell
                .panels
                .floating_panel(source_side)
                .filter(|item| item.persistence_id() == entry_id);
            self.clear_activity_entry_from_side(&entry_id, source_side);
            if let Some(item) = moved_floating {
                self.shell
                    .panels
                    .set_floating_panel(target_side, Some(item));
            }
        }

        self.shell.chrome.activity_bar_context_menu = None;
        self.shell.set_status(format!(
            "moved {} to {}",
            entry_id,
            target_zone.label().to_lowercase()
        ));
        self.persist_ui_layout();
        cx.notify();
    }

    fn activity_zone_side(zone: ActivityBarZone) -> PanelSide {
        match zone {
            ActivityBarZone::LeftTop | ActivityBarZone::LeftBottom => PanelSide::Left,
            ActivityBarZone::RightTop | ActivityBarZone::RightBottom => PanelSide::Right,
        }
    }

    fn clear_activity_entry_from_side(&mut self, entry_id: &str, side: PanelSide) {
        if self
            .shell
            .panels
            .floating_panel(side)
            .is_some_and(|item| item.persistence_id() == entry_id)
        {
            self.shell.panels.set_floating_panel(side, None);
        }
        match side {
            PanelSide::Left => {
                self.shell.panels.left_open.retain(|id| id != entry_id);
                if self
                    .shell
                    .panels
                    .active_left
                    .is_some_and(|item| item.persistence_id() == entry_id)
                {
                    self.shell.panels.active_left = None;
                }
                if self.shell.panels.left_open.is_empty() && self.shell.panels.active_left.is_none()
                {
                    self.shell.panels.left_collapsed = true;
                }
            }
            PanelSide::Right => {
                self.shell.panels.right_open.retain(|id| id != entry_id);
                if self
                    .shell
                    .panels
                    .active_right
                    .is_some_and(|item| item.persistence_id() == entry_id)
                {
                    self.shell.panels.active_right = None;
                }
                if self.shell.panels.right_open.is_empty()
                    && self.shell.panels.active_right.is_none()
                {
                    self.shell.panels.right_collapsed = true;
                }
            }
        }
    }

    pub(in crate::features) fn activate_activity_entry(
        &mut self,
        entry: ActivityBarEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match entry {
            ActivityBarEntry::Panel(NavItem::Settings) => self.open_page(NavItem::Settings, cx),
            ActivityBarEntry::Panel(item) => {
                let side = self.panel_side_for_item(item);
                if self.shell.panel_is_floating() {
                    if let Some(side) = side {
                        self.toggle_floating_panel(item, side, cx);
                    }
                } else {
                    self.open_panel(item, cx);
                    if !cfg!(target_os = "macos") {
                        match side {
                            Some(PanelSide::Left) if self.shell.viewport.size.0 < 1024. => {
                                self.shell.panels.mobile_left_open = true;
                            }
                            Some(PanelSide::Right) if self.shell.viewport.size.0 < 768. => {
                                self.shell.panels.mobile_right_open = true;
                            }
                            _ => {}
                        }
                    }
                    cx.notify();
                }
                if item == NavItem::Plugins && !self.panel_entry_selected(item) {
                    self.focus_active_workspace_surface(window, cx);
                }
            }
            ActivityBarEntry::QuickCommands => {
                let mode = if self.shell.bottom_panel.mode == BottomPanelMode::QuickCommands {
                    BottomPanelMode::Hidden
                } else {
                    BottomPanelMode::QuickCommands
                };
                self.set_bottom_panel_mode(mode);
                cx.notify();
            }
            ActivityBarEntry::CommandSend => {
                let mode = if self.shell.bottom_panel.mode == BottomPanelMode::CommandSend {
                    BottomPanelMode::Hidden
                } else {
                    BottomPanelMode::CommandSend
                };
                self.set_bottom_panel_mode(mode);
                cx.notify();
            }
            ActivityBarEntry::Recording => {
                let item = NavItem::Recording;
                if let Some(side) = self.panel_side_for_item(item) {
                    if self.shell.panel_is_floating() {
                        self.toggle_floating_panel(item, side, cx);
                    } else {
                        self.open_panel(item, cx);
                        cx.notify();
                    }
                }
            }
            ActivityBarEntry::Lock => self.lock_app(window, cx),
        }
    }

    pub(in crate::features) fn activity_entry_selected(&self, entry: ActivityBarEntry) -> bool {
        match entry {
            ActivityBarEntry::Panel(NavItem::Settings) => self.settings_tab_is_open(),
            ActivityBarEntry::Panel(item) => self.panel_entry_selected(item),
            ActivityBarEntry::QuickCommands => {
                self.shell.bottom_panel.mode == BottomPanelMode::QuickCommands
            }
            ActivityBarEntry::CommandSend => {
                self.shell.bottom_panel.mode == BottomPanelMode::CommandSend
            }
            ActivityBarEntry::Recording => {
                self.panel_entry_selected(NavItem::Recording) || self.recording.active_count() > 0
            }
            ActivityBarEntry::Lock => self.security.screen_locked(),
        }
    }

    fn panel_entry_selected(&self, item: NavItem) -> bool {
        let Some(side) = self.panel_side_for_item(item) else {
            return false;
        };
        if self.shell.panel_is_floating() {
            return self.shell.floating_panel(side) == Some(item);
        }
        if self.shell.panels.multi_open {
            let id = item.persistence_id();
            self.side_open_panel_ids(side).iter().any(|open| open == id)
                || self.side_overlay_panel(side) == Some(item)
                || match side {
                    PanelSide::Left => self.shell.panels.active_left == Some(item),
                    PanelSide::Right => self.shell.panels.active_right == Some(item),
                }
        } else {
            match side {
                PanelSide::Left => self.current_left_panel() == Some(item),
                PanelSide::Right => self.current_right_panel() == Some(item),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::{ActivityBarZone, NavItem, PanelOpenMode, PanelSide};
    use crate::test_support::TestConfigDir;

    fn test_app(cx: &mut TestAppContext, root: &Path) -> gpui::Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    #[test]
    fn hiding_an_active_panel_clears_its_side_and_removes_it_from_the_rail() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            // Open the left file explorer, then hide it from the rail.
            app.open_panel(NavItem::Transfers, cx);
            assert_eq!(app.current_left_panel(), Some(NavItem::Transfers));

            app.hide_activity_entry("fileExplorer".to_string(), cx);

            assert!(app.shell.activity_bar_layout().is_hidden("fileExplorer"));
            assert_eq!(app.shell.active_left_panel(), None);
            assert!(app.current_left_panel().is_none());
            // The hidden entry is no longer offered on the rail.
            assert!(
                !app.activity_side_has_items(crate::features::runtime_jobs::ActivitySide::Left)
                    || app
                        .shell
                        .activity_bar_layout()
                        .first_panel_on_side(PanelSide::Left)
                        != Some(NavItem::Transfers)
            );

            // Showing it again restores it to the rail.
            app.show_activity_entry("fileExplorer".to_string(), cx);
            assert!(!app.shell.activity_bar_layout().is_hidden("fileExplorer"));
        });
    }

    #[test]
    fn floating_panels_are_transient_per_side_and_independent_from_multi_open() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_panel(NavItem::Transfers, cx);
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            assert_eq!(app.shell.active_left_panel(), None);
            assert_eq!(app.shell.floating_panel(PanelSide::Left), None);
            assert!(!app.shell.panel_multi_open());

            app.open_panel(NavItem::Transfers, cx);
            app.open_panel(NavItem::Connections, cx);
            assert_eq!(
                app.shell.floating_panel(PanelSide::Left),
                Some(NavItem::Transfers)
            );
            assert_eq!(
                app.shell.floating_panel(PanelSide::Right),
                Some(NavItem::Connections)
            );

            app.toggle_floating_panel(NavItem::Tunnels, PanelSide::Left, cx);
            assert_eq!(
                app.shell.floating_panel(PanelSide::Left),
                Some(NavItem::Tunnels)
            );
            assert_eq!(
                app.shell.floating_panel(PanelSide::Right),
                Some(NavItem::Connections)
            );

            app.toggle_panel_multi_open(cx);
            assert!(app.shell.panel_multi_open());
            assert_eq!(
                app.shell.floating_panel(PanelSide::Left),
                Some(NavItem::Tunnels)
            );

            app.hide_activity_entry("network".to_string(), cx);
            assert_eq!(app.shell.floating_panel(PanelSide::Left), None);
            app.set_panel_open_mode(PanelOpenMode::Docked, cx);
            assert_eq!(app.shell.floating_panel(PanelSide::Right), None);
            assert!(app.shell.panel_multi_open());
        });
    }

    #[test]
    fn plugin_panel_follows_placement_and_toggles_in_docked_floating_and_multi_open_modes() {
        let test_dir = TestConfigDir::new("nyaterm-plugins-panel-navigation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            let panel = app.plugins.panel.entity_id();
            app.open_panel(NavItem::Plugins, cx);
            assert_eq!(app.current_left_panel(), Some(NavItem::Plugins));
            assert!(app.panel_entry_selected(NavItem::Plugins));
            app.open_panel(NavItem::Plugins, cx);
            assert_eq!(app.current_left_panel(), None);

            app.move_activity_entry("plugins".into(), ActivityBarZone::RightBottom, None, cx);
            app.open_panel(NavItem::Plugins, cx);
            assert_eq!(app.current_right_panel(), Some(NavItem::Plugins));
            assert_eq!(app.plugins.panel.entity_id(), panel);
            app.hide_activity_entry("plugins".into(), cx);
            assert_eq!(app.current_right_panel(), None);
            assert!(app.shell.activity_bar_layout().is_hidden("plugins"));
            app.show_activity_entry("plugins".into(), cx);

            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            for (zone, side) in [
                (ActivityBarZone::RightBottom, PanelSide::Right),
                (ActivityBarZone::LeftBottom, PanelSide::Left),
            ] {
                app.move_activity_entry("plugins".into(), zone, None, cx);
                app.open_panel(NavItem::Plugins, cx);
                assert_eq!(app.shell.floating_panel(side), Some(NavItem::Plugins));
                app.close_floating_panel(side, cx);
                assert_eq!(app.shell.floating_panel(side), None);
            }

            app.set_panel_open_mode(PanelOpenMode::Docked, cx);
            app.toggle_panel_multi_open(cx);
            app.open_panel(NavItem::Notes, cx);
            app.open_panel(NavItem::Plugins, cx);
            assert!(
                app.side_open_panel_ids(PanelSide::Left)
                    .contains(&"plugins".to_string())
            );
            assert!(
                app.side_open_panel_ids(PanelSide::Left)
                    .contains(&"notes".to_string())
            );
            app.open_panel(NavItem::Plugins, cx);
            assert!(
                !app.side_open_panel_ids(PanelSide::Left)
                    .contains(&"plugins".to_string())
            );
            assert!(
                app.side_open_panel_ids(PanelSide::Left)
                    .contains(&"notes".to_string())
            );
            assert_eq!(app.plugins.panel.entity_id(), panel);
        });
    }

    #[test]
    fn startup_completes_legacy_plugin_layout_and_preserves_saved_right_placement() {
        for moved in [false, true] {
            let test_dir = TestConfigDir::new("nyaterm-plugin-startup-layout");
            {
                let store = nyaterm_store::ConnectionStore::open(test_dir.path().join("config"))
                    .expect("store");
                let mut settings = store.load_app_settings_summary().expect("settings");
                settings.ui_activity_bar_left_bottom =
                    vec!["futureEntry".into(), "settings".into()];
                if moved {
                    settings.ui_activity_bar_right_bottom.push("plugins".into());
                    settings.ui_activity_bar_hidden_items.push("plugins".into());
                }
                store
                    .save_ui_layout_settings(&settings)
                    .expect("save layout");
            }
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = test_app(&mut cx, test_dir.path());
            cx.read(|cx| {
                let app = app.read(cx);
                let layout = app.shell.activity_bar_layout();
                if moved {
                    assert_eq!(layout.left_bottom, ["futureEntry", "settings"]);
                    assert_eq!(layout.side_for_entry("plugins"), Some(PanelSide::Right));
                    assert!(layout.is_hidden("plugins"));
                } else {
                    assert_eq!(layout.left_bottom, ["futureEntry", "plugins", "settings"]);
                    assert_eq!(layout.side_for_entry("plugins"), Some(PanelSide::Left));
                }
                assert_eq!(
                    app.settings.summary().ui_activity_bar_left_bottom,
                    layout.left_bottom
                );
            });
        }
    }

    #[test]
    fn normalization_migrates_aliases_and_inserts_notes_at_its_anchor() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _cx| {
            app.shell.chrome.activity_bar_layout.left_top = vec![
                "fileExplorer".to_string(),
                "fileTransfer".to_string(),
                "keyManagement".to_string(),
                "network".to_string(),
            ];
            app.shell.chrome.activity_bar_layout.hidden_items = vec![
                "keyManagement".to_string(),
                "securityAuth".to_string(),
                "fileTransfer".to_string(),
            ];
            app.normalize_activity_bar_layout();

            let left = &app.shell.activity_bar_layout().left_top;
            let files = left.iter().position(|id| id == "fileExplorer").unwrap();
            assert_eq!(left.get(files + 1).map(String::as_str), Some("notes"));
            assert!(!left.iter().any(|id| id == "fileTransfer"));
            assert!(!left.iter().any(|id| id == "keyManagement"));
            assert_eq!(
                app.shell.activity_bar_layout().hidden_items,
                vec!["securityAuth".to_string()]
            );
            assert!(
                app.activity_entries_for_zone(crate::models::ActivityBarZone::LeftTop)
                    .iter()
                    .any(|entry| entry.persistence_id() == "notes")
            );
        });
    }

    #[test]
    fn hiding_an_entry_closes_its_floating_panel_and_can_be_reversed() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.show_activity_entry("gpuMonitor".into(), cx);
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            app.open_panel(NavItem::GpuMonitor, cx);
            assert_eq!(
                app.shell.floating_panel(PanelSide::Right),
                Some(NavItem::GpuMonitor)
            );

            app.hide_activity_entry("gpuMonitor".into(), cx);
            assert_eq!(app.shell.floating_panel(PanelSide::Right), None);
            assert!(app.shell.activity_bar_layout().is_hidden("gpuMonitor"));
            app.show_activity_entry("gpuMonitor".into(), cx);
            assert!(!app.shell.activity_bar_layout().is_hidden("gpuMonitor"));
            assert_eq!(app.shell.floating_panel(PanelSide::Right), None);
        });
    }

    #[test]
    fn drop_indices_map_to_visible_insertion_slots_across_both_sides() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.shell.chrome.activity_bar_layout.left_top = vec![
                "fileExplorer".to_string(),
                "network".to_string(),
                "securityAuth".to_string(),
            ];
            app.shell.chrome.activity_bar_layout.right_top =
                vec!["savedConnections".to_string(), "activeSessions".to_string()];
            app.shell.chrome.activity_bar_layout.hidden_items.clear();

            app.move_activity_entry("network".to_string(), ActivityBarZone::LeftTop, Some(0), cx);
            assert_eq!(
                app.shell.activity_bar_layout().left_top,
                ["network", "fileExplorer", "securityAuth"]
            );

            app.move_activity_entry("network".to_string(), ActivityBarZone::LeftTop, Some(3), cx);
            assert_eq!(
                app.shell.activity_bar_layout().left_top,
                ["fileExplorer", "securityAuth", "network"]
            );

            app.move_activity_entry(
                "network".to_string(),
                ActivityBarZone::RightTop,
                Some(1),
                cx,
            );
            assert_eq!(
                app.shell.activity_bar_layout().left_top,
                ["fileExplorer", "securityAuth"]
            );
            assert_eq!(
                app.shell.activity_bar_layout().right_top,
                ["savedConnections", "network", "activeSessions"]
            );
        });
    }

    #[test]
    fn reset_activity_bar_layout_restores_defaults_and_collapses_sides() {
        let test_dir = TestConfigDir::new("nyaterm-activity-bar");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.hide_activity_entry("aiAssistant".to_string(), cx);
            app.open_panel(NavItem::Transfers, cx);
            assert!(app.shell.activity_bar_layout().has_hidden_entries());

            app.reset_activity_bar_layout(cx);

            assert_eq!(
                app.shell.activity_bar_layout().hidden_items,
                nyaterm_core::models::settings::panel_visibility::default_hidden_items()
            );
            assert_eq!(app.shell.active_left_panel(), None);
            assert_eq!(app.shell.active_right_panel(), None);
        });
    }
}

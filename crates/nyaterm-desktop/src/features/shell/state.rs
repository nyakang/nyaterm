//! Authoritative transient state for the application shell.
//!
//! Shell rendering remains on `NyaTermApp` views. This state owns interaction
//! lifecycles that span those views so the composition root does not retain
//! independently mutable mirrors.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Entity, Pixels, RenderImage, ScrollHandle, SharedString};
use nyaterm_ui::{ChildWindowSlot, NyaAppMenuBar, NyaWindowHandle};

use super::super::app_state::SettingsDraftSnapshot;
use super::runtime_state::ShellRuntimeState;
use crate::models::{
    ActivityBarContextMenuState, ActivityBarLayoutState, BottomPanelMode, BottomPanelResizeState,
    HeaderStatusMode, HeaderStatusState, MainMode, NavItem, PanelOpenMode, PanelResizeSide,
    PanelResizeState, PanelSide, PanelStackResizeState, RightFocus, SettingsTab, WorkspacePaneNode,
    WorkspaceSplitResizeState, WorkspaceSplitState,
};

pub(in crate::features) const RESIZE_HANDLE_HOVER_DELAY: Duration = Duration::from_millis(250);

/// The tab-strip trigger that owns the currently open new-session menu.
///
/// This is transient chrome state. Keeping the anchor here guarantees that a
/// multi-leaf workspace never opens the same controlled popover in every leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) enum NewSessionMenuAnchor {
    MainTabStrip,
    TerminalLeaf(String),
}

pub(in crate::features) struct ShellFeatureState {
    pub(in crate::features) system_tray: Option<super::tray::SystemTray>,
    /// Application-wide transient status shown by shell chrome and terminal overlays.
    status: String,
    /// GPUI event-pump, repaint, and shell-persistence scheduling bookkeeping.
    pub(super) runtime: ShellRuntimeState,
    pub(super) bottom_panel: ShellBottomPanelState,
    pub(super) viewport: ShellViewportState,
    pub(super) navigation: ShellNavigationState,
    pub(super) panels: ShellPanelState,
    pub(super) chrome: ShellChromeState,
    pub(super) workspace: ShellWorkspaceState,
    pub(super) diagnostics: ShellDiagnosticState,
    resize_handle_hover: ResizeHandleHoverState,
    /// The window `NyaTermApp` itself renders in.
    ///
    /// GPUI discards the handle `cx.open_window` returns at startup, and
    /// `cx.active_window()` is whatever the platform focused, which is the wrong
    /// answer when a child window opens from a background event. Child windows
    /// need this one to pick the display and the placement to open on.
    main_window: Option<NyaWindowHandle>,
}

#[derive(Default)]
pub(in crate::features) struct ResizeHandleHoverState {
    pending_id: Option<SharedString>,
    active_id: Option<SharedString>,
    generation: u64,
}

#[derive(Default)]
pub(super) struct ShellDiagnosticState {
    last_log_at: HashMap<&'static str, Instant>,
}

pub(in crate::features) struct ShellFeatureInit {
    pub status: String,
    pub bottom_panel_mode: BottomPanelMode,
    pub quick_commands_height: f32,
    pub command_send_height: f32,
    pub active_left_panel: Option<NavItem>,
    pub active_right_panel: Option<NavItem>,
    pub left_open_panels: Vec<String>,
    pub right_open_panels: Vec<String>,
    pub panel_stack_sizes: HashMap<String, f32>,
    pub panel_open_mode: PanelOpenMode,
    pub panel_multi_open: bool,
    pub left_sidebar_collapsed: bool,
    pub right_inspector_collapsed: bool,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
    pub activity_bar_layout: ActivityBarLayoutState,
}

pub(super) struct ShellBottomPanelState {
    pub(super) mode: BottomPanelMode,
    pub(super) quick_commands_height: f32,
    pub(super) command_send_height: f32,
    pub(super) resize: Option<BottomPanelResizeState>,
}

/// Window geometry and viewport-derived caches.
pub(super) struct ShellViewportState {
    pub(super) size: (f32, f32),
    pub(super) wallpaper: WallpaperCache,
    pub(super) last_change_at: Option<Instant>,
    pub(super) title_drag_active_until: Option<Instant>,
}

#[derive(Clone)]
pub(in crate::features) struct WallpaperAsset {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

impl WallpaperAsset {
    pub(in crate::features) fn image(&self) -> &Arc<RenderImage> {
        &self.image
    }

    pub(in crate::features) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[derive(Default)]
pub(super) struct WallpaperCache {
    requested_path: Option<String>,
    asset: Option<WallpaperAsset>,
}

/// Top-level page navigation and the settings-page/window lifecycle.
pub(super) struct ShellNavigationState {
    pub(super) selected_nav: NavItem,
    pub(super) main_mode: MainMode,
    pub(super) settings: ShellSettingsNavigationState,
}

pub(super) struct ShellSettingsNavigationState {
    pub(super) active_tab: SettingsTab,
    pub(super) expanded_groups: HashSet<String>,
    pub(super) draft_snapshot: Option<SettingsDraftSnapshot>,
    pub(super) window: ChildWindowSlot,
    pub(super) previous_left_collapsed: Option<bool>,
    pub(super) previous_right_collapsed: Option<bool>,
}

/// Side-panel selection, stack layout and resize interaction state.
pub(super) struct ShellPanelState {
    pub(super) active_left: Option<NavItem>,
    pub(super) active_right: Option<NavItem>,
    pub(super) left_open: Vec<String>,
    pub(super) right_open: Vec<String>,
    pub(super) stack_sizes: HashMap<String, f32>,
    /// Docked versus floating presentation. Multi-open is independent.
    pub(super) open_mode: PanelOpenMode,
    pub(super) multi_open: bool,
    /// Floating selections are transient and deliberately never persisted.
    pub(super) floating_left: Option<NavItem>,
    pub(super) floating_right: Option<NavItem>,
    pub(super) last_floating_side: Option<PanelSide>,
    pub(super) right_focus: RightFocus,
    pub(super) left_collapsed: bool,
    pub(super) right_collapsed: bool,
    pub(super) mobile_left_open: bool,
    pub(super) mobile_right_open: bool,
    pub(super) left_width: f32,
    pub(super) right_width: f32,
    pub(super) resize: Option<PanelResizeState>,
    pub(super) stack_resize: Option<PanelStackResizeState>,
}

/// Activity bar, title menus, tab-strip menus and connection-failure chrome.
pub(super) struct ShellChromeState {
    pub(super) title_menu_bar: Option<Entity<NyaAppMenuBar>>,
    pub(super) activity_bar_layout: ActivityBarLayoutState,
    pub(super) activity_bar_context_menu: Option<ActivityBarContextMenuState>,
    pub(super) header_status: HeaderStatusState,
    pub(super) open_tabs_menu_open: bool,
    pub(super) new_session_menu_anchor: Option<NewSessionMenuAnchor>,
    pub(super) new_session_all_sessions_open: bool,
    pub(super) new_session_group_menu_path: Vec<String>,
    pub(super) session_tab_strip_scroll: ScrollHandle,
    pub(super) session_tab_scroll_into_view_pending: bool,
    pub(super) session_tab_strip_has_overflow: bool,
    pub(super) session_tab_strip_viewport_width: Option<Pixels>,
    pub(super) last_connect_failure_name: Option<String>,
    pub(super) last_connect_failure_error: Option<String>,
}

/// Global and per-tab pane trees for the workspace surface.
pub(super) struct ShellWorkspaceState {
    pub(super) split: Option<WorkspaceSplitState>,
    pub(super) split_resize: Option<WorkspaceSplitResizeState>,
    pub(super) pane_roots: HashMap<String, WorkspacePaneNode>,
    pub(super) tab_owner: HashMap<String, String>,
    pub(super) focused_terminal_leaf_id: Option<String>,
    pub(super) pane_layout_restored: bool,
}

impl ShellFeatureState {
    pub(in crate::features) fn new(init: ShellFeatureInit) -> Self {
        Self {
            status: init.status,
            system_tray: None,
            runtime: ShellRuntimeState::default(),
            bottom_panel: ShellBottomPanelState {
                mode: init.bottom_panel_mode,
                quick_commands_height: init.quick_commands_height,
                command_send_height: init.command_send_height,
                resize: None,
            },
            viewport: ShellViewportState {
                size: (1280., 800.),
                wallpaper: WallpaperCache::default(),
                last_change_at: None,
                title_drag_active_until: None,
            },
            navigation: ShellNavigationState {
                selected_nav: NavItem::Workspace,
                main_mode: MainMode::Workspace,
                settings: ShellSettingsNavigationState {
                    active_tab: SettingsTab::General,
                    expanded_groups: HashSet::from(["workspace".to_string()]),
                    draft_snapshot: None,
                    window: ChildWindowSlot::default(),
                    previous_left_collapsed: None,
                    previous_right_collapsed: None,
                },
            },
            panels: ShellPanelState {
                active_left: init.active_left_panel,
                active_right: init.active_right_panel,
                left_open: init.left_open_panels,
                right_open: init.right_open_panels,
                stack_sizes: init.panel_stack_sizes,
                open_mode: init.panel_open_mode,
                multi_open: init.panel_multi_open,
                floating_left: None,
                floating_right: None,
                last_floating_side: None,
                right_focus: RightFocus::Default,
                left_collapsed: init.left_sidebar_collapsed,
                right_collapsed: init.right_inspector_collapsed,
                mobile_left_open: false,
                mobile_right_open: false,
                left_width: init.left_panel_width,
                right_width: init.right_panel_width,
                resize: None,
                stack_resize: None,
            },
            chrome: ShellChromeState {
                title_menu_bar: None,
                activity_bar_layout: init.activity_bar_layout,
                activity_bar_context_menu: None,
                header_status: HeaderStatusState::default(),
                open_tabs_menu_open: false,
                new_session_menu_anchor: None,
                new_session_all_sessions_open: false,
                new_session_group_menu_path: Vec::new(),
                session_tab_strip_scroll: ScrollHandle::new(),
                session_tab_scroll_into_view_pending: false,
                session_tab_strip_has_overflow: false,
                session_tab_strip_viewport_width: None,
                last_connect_failure_name: None,
                last_connect_failure_error: None,
            },
            workspace: ShellWorkspaceState {
                split: None,
                split_resize: None,
                pane_roots: HashMap::new(),
                tab_owner: HashMap::new(),
                focused_terminal_leaf_id: None,
                pane_layout_restored: false,
            },
            diagnostics: ShellDiagnosticState::default(),
            resize_handle_hover: ResizeHandleHoverState::default(),
            main_window: None,
        }
    }

    pub(in crate::features) fn begin_resize_handle_hover(
        &mut self,
        id: SharedString,
    ) -> Option<u64> {
        self.resize_handle_hover.begin(id)
    }

    pub(in crate::features) fn activate_resize_handle_hover(
        &mut self,
        id: &SharedString,
        generation: u64,
    ) -> bool {
        self.resize_handle_hover.activate(id, generation)
    }

    pub(in crate::features) fn activate_resize_handle_immediately(&mut self, id: SharedString) {
        self.resize_handle_hover.activate_immediately(id);
    }

    pub(in crate::features) fn leave_resize_handle_hover(&mut self, id: &SharedString) -> bool {
        self.resize_handle_hover.leave(id)
    }

    pub(in crate::features) fn resize_handle_is_highlighted(&self, id: &SharedString) -> bool {
        self.resize_handle_hover.is_highlighted(id)
    }

    pub(in crate::features) fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn set_title_menu_bar(&mut self, menu_bar: Entity<NyaAppMenuBar>) {
        self.chrome.title_menu_bar = Some(menu_bar);
    }

    pub(in crate::features) fn title_menu_bar(&self) -> Option<Entity<NyaAppMenuBar>> {
        self.chrome.title_menu_bar.clone()
    }

    pub(in crate::features) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(in crate::features) fn bottom_panel_mode(&self) -> BottomPanelMode {
        self.bottom_panel.mode
    }

    pub(in crate::features) fn quick_commands_height(&self) -> f32 {
        self.bottom_panel.quick_commands_height
    }

    pub(in crate::features) fn command_send_height(&self) -> f32 {
        self.bottom_panel.command_send_height
    }

    pub(in crate::features) fn viewport_size(&self) -> (f32, f32) {
        self.viewport.size
    }

    pub(in crate::features) fn wallpaper_asset(&self) -> Option<&WallpaperAsset> {
        self.viewport.wallpaper.asset.as_ref()
    }

    pub(in crate::features) fn request_wallpaper(&mut self, path: Option<String>) -> bool {
        if self.viewport.wallpaper.requested_path == path {
            return false;
        }
        self.viewport.wallpaper.requested_path = path;
        self.viewport.wallpaper.asset = None;
        true
    }

    pub(in crate::features) fn wallpaper_is_requested(&self, path: &str) -> bool {
        self.viewport.wallpaper.requested_path.as_deref() == Some(path)
    }

    pub(in crate::features) fn cache_wallpaper(
        &mut self,
        path: String,
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
    ) -> bool {
        if self.viewport.wallpaper.requested_path.as_deref() != Some(path.as_str()) {
            return false;
        }
        self.viewport.wallpaper.asset = Some(WallpaperAsset {
            image,
            width,
            height,
        });
        true
    }

    pub(in crate::features) fn selected_nav(&self) -> NavItem {
        self.navigation.selected_nav
    }

    pub(in crate::features) fn main_mode(&self) -> MainMode {
        self.navigation.main_mode
    }

    pub(in crate::features) fn select_nav(&mut self, nav: NavItem) {
        self.navigation.selected_nav = nav;
    }

    pub(in crate::features) fn show_workspace(&mut self) {
        self.navigation.selected_nav = NavItem::Workspace;
        self.navigation.main_mode = MainMode::Workspace;
    }

    pub(in crate::features) fn show_page(&mut self, nav: NavItem) {
        self.navigation.selected_nav = nav;
        self.navigation.main_mode = MainMode::Page;
    }

    pub(in crate::features) fn is_settings_page(&self) -> bool {
        self.navigation.main_mode == MainMode::Page
            && self.navigation.selected_nav == NavItem::Settings
    }

    pub(in crate::features) fn settings_active_tab(&self) -> SettingsTab {
        self.navigation.settings.active_tab
    }

    pub(in crate::features) fn set_settings_active_tab(&mut self, tab: SettingsTab) {
        self.navigation.settings.active_tab = tab;
    }

    pub(in crate::features) fn settings_group_is_expanded(&self, group: &str) -> bool {
        self.navigation.settings.expanded_groups.contains(group)
    }

    pub(in crate::features) fn toggle_settings_group(&mut self, group: String) -> bool {
        if !self
            .navigation
            .settings
            .expanded_groups
            .insert(group.clone())
        {
            self.navigation.settings.expanded_groups.remove(&group);
            false
        } else {
            true
        }
    }

    pub(in crate::features) fn settings_draft_snapshot(&self) -> Option<&SettingsDraftSnapshot> {
        self.navigation.settings.draft_snapshot.as_ref()
    }

    pub(in crate::features) fn has_settings_draft(&self) -> bool {
        self.navigation.settings.draft_snapshot.is_some()
    }

    pub(in crate::features) fn set_settings_draft_snapshot(
        &mut self,
        snapshot: SettingsDraftSnapshot,
    ) {
        self.navigation.settings.draft_snapshot = Some(snapshot);
    }

    pub(in crate::features) fn take_settings_draft_snapshot(
        &mut self,
    ) -> Option<SettingsDraftSnapshot> {
        self.navigation.settings.draft_snapshot.take()
    }

    pub(in crate::features) fn clear_settings_draft_snapshot(&mut self) -> bool {
        self.navigation.settings.draft_snapshot.take().is_some()
    }

    pub(in crate::features) fn settings_window(&self) -> Option<NyaWindowHandle> {
        self.navigation.settings.window.handle()
    }

    pub(in crate::features) fn settings_window_open_pending(&self) -> bool {
        self.navigation.settings.window.is_pending()
    }

    pub(in crate::features) fn settings_window_is_open_or_pending(&self) -> bool {
        self.navigation.settings.window.is_open_or_pending()
    }

    pub(in crate::features) fn settings_window_slot(&mut self) -> &mut ChildWindowSlot {
        &mut self.navigation.settings.window
    }

    pub(in crate::features) fn begin_settings_window_open(&mut self) -> bool {
        self.navigation.settings.window.begin_open()
    }

    pub(in crate::features) fn complete_settings_window_open(&mut self, handle: NyaWindowHandle) {
        self.navigation.settings.window.finish_open(handle);
        self.navigation.settings.previous_left_collapsed = None;
        self.navigation.settings.previous_right_collapsed = None;
    }

    pub(in crate::features) fn clear_settings_window(&mut self) {
        self.navigation.settings.window.clear();
    }

    pub(in crate::features) fn cancel_settings_window_open(&mut self) {
        self.navigation.settings.window.cancel_open();
    }

    pub(in crate::features) fn fail_settings_window_open(&mut self) {
        self.navigation.settings.window.fail_open();
        self.show_page(NavItem::Settings);
        self.panels.left_collapsed = true;
        self.panels.right_collapsed = true;
    }

    /// Clears settings-window ownership and restores the embedded-page panel state.
    pub(in crate::features) fn finish_settings_navigation(&mut self) -> bool {
        self.clear_settings_window();
        if self.is_settings_page() {
            self.navigation.main_mode = MainMode::Workspace;
            self.panels.left_collapsed = self
                .navigation
                .settings
                .previous_left_collapsed
                .take()
                .unwrap_or_else(|| self.panels.active_left.is_none());
            self.panels.right_collapsed = self
                .navigation
                .settings
                .previous_right_collapsed
                .take()
                .unwrap_or_else(|| self.panels.active_right.is_none());
            true
        } else {
            self.navigation.settings.previous_left_collapsed = None;
            self.navigation.settings.previous_right_collapsed = None;
            false
        }
    }

    /// The window `NyaTermApp` renders in, once the first frame has run.
    pub(in crate::features) fn main_window(&self) -> Option<NyaWindowHandle> {
        self.main_window
    }

    /// Record the main window on the first frame that can see it.
    ///
    /// Only ever set once: `NyaTermApp` renders in exactly one window, and a
    /// child window's root is a different view type so it cannot land here.
    pub(in crate::features) fn remember_main_window(&mut self, handle: NyaWindowHandle) {
        if self.main_window.is_none() {
            self.main_window = Some(handle);
        }
    }

    pub(in crate::features) fn active_left_panel(&self) -> Option<NavItem> {
        self.panels.active_left
    }
    pub(in crate::features) fn active_right_panel(&self) -> Option<NavItem> {
        self.panels.active_right
    }

    pub(in crate::features) fn left_panel_width(&self) -> f32 {
        self.panels.left_width
    }

    /// Set the stored right-panel width. Test-only: the real writers are the resize
    /// drag and the settings load.
    #[cfg(test)]
    pub(in crate::features) fn set_right_panel_width_for_test(&mut self, width: f32) {
        self.panels.right_width = width;
    }

    pub(in crate::features) fn right_panel_width(&self) -> f32 {
        self.panels.right_width
    }

    pub(in crate::features) fn panel_resize_active(&self) -> bool {
        self.panels.resize.is_some() || self.panels.stack_resize.is_some()
    }

    pub(in crate::features) fn panel_multi_open(&self) -> bool {
        self.panels.multi_open
    }

    pub(in crate::features) fn panel_open_mode(&self) -> PanelOpenMode {
        self.panels.open_mode
    }

    pub(in crate::features) fn panel_is_floating(&self) -> bool {
        self.panels.open_mode.is_floating()
    }

    pub(in crate::features) fn floating_panel(&self, side: PanelSide) -> Option<NavItem> {
        self.panels.floating_panel(side)
    }

    pub(in crate::features) fn mobile_left_panel_open(&self) -> bool {
        self.panels.mobile_left_open
    }

    pub(in crate::features) fn mobile_right_panel_open(&self) -> bool {
        self.panels.mobile_right_open
    }

    pub(in crate::features) fn close_mobile_panels(&mut self) -> bool {
        let changed = self.panels.mobile_left_open || self.panels.mobile_right_open;
        self.panels.mobile_left_open = false;
        self.panels.mobile_right_open = false;
        changed
    }

    pub(in crate::features) fn close_mobile_panel(&mut self, side: PanelSide) -> bool {
        match side {
            PanelSide::Left => std::mem::take(&mut self.panels.mobile_left_open),
            PanelSide::Right => std::mem::take(&mut self.panels.mobile_right_open),
        }
    }

    pub(in crate::features) fn set_right_focus(&mut self, focus: RightFocus) {
        self.panels.right_focus = focus;
    }

    pub(in crate::features) fn activity_bar_layout(&self) -> &ActivityBarLayoutState {
        &self.chrome.activity_bar_layout
    }

    pub(in crate::features) fn activity_bar_context_menu(
        &self,
    ) -> Option<&ActivityBarContextMenuState> {
        self.chrome.activity_bar_context_menu.as_ref()
    }

    pub(in crate::features) fn close_root_menus(&mut self) -> bool {
        let mut changed = self.chrome.close_open_tabs_menu();
        changed |= self.chrome.close_new_session_menu();
        changed
    }

    pub(in crate::features) fn header_status_rendered_minute(&self) -> i64 {
        self.chrome.header_status.rendered_minute
    }

    pub(in crate::features) fn set_header_status_rendered_minute(&mut self, minute: i64) {
        self.chrome.header_status.rendered_minute = minute;
    }

    pub(in crate::features) fn header_status_hardware_page(&self, mode: HeaderStatusMode) -> usize {
        match mode {
            HeaderStatusMode::Gpu => self.chrome.header_status.gpu_page,
            HeaderStatusMode::Npu => self.chrome.header_status.npu_page,
            _ => 0,
        }
    }

    pub(in crate::features) fn set_header_status_hardware_page(
        &mut self,
        mode: HeaderStatusMode,
        page: usize,
    ) {
        match mode {
            HeaderStatusMode::Gpu => self.chrome.header_status.gpu_page = page,
            HeaderStatusMode::Npu => self.chrome.header_status.npu_page = page,
            _ => {}
        }
    }

    pub(in crate::features) fn open_tabs_menu_is_open(&self) -> bool {
        self.chrome.open_tabs_menu_open
    }

    pub(in crate::features) fn prepare_session_switch(&mut self) {
        self.chrome.prepare_session_switch();
    }

    pub(in crate::features) fn toggle_open_tabs_menu(&mut self) {
        self.chrome.toggle_open_tabs_menu();
    }

    pub(in crate::features) fn close_open_tabs_menu(&mut self) -> bool {
        self.chrome.close_open_tabs_menu()
    }

    pub(in crate::features) fn new_session_menu_is_open(&self) -> bool {
        self.chrome.new_session_menu_anchor.is_some()
    }

    pub(in crate::features) fn new_session_menu_is_open_at(
        &self,
        anchor: &NewSessionMenuAnchor,
    ) -> bool {
        self.chrome.new_session_menu_anchor.as_ref() == Some(anchor)
    }

    pub(in crate::features) fn open_new_session_menu(
        &mut self,
        anchor: NewSessionMenuAnchor,
    ) -> bool {
        self.chrome.open_new_session_menu(anchor)
    }

    pub(in crate::features) fn close_new_session_menu(&mut self) -> bool {
        self.chrome.close_new_session_menu()
    }

    pub(in crate::features) fn new_session_all_sessions_is_open(&self) -> bool {
        self.chrome.new_session_all_sessions_open
    }

    pub(in crate::features) fn new_session_group_menu_path(&self) -> &[String] {
        &self.chrome.new_session_group_menu_path
    }

    pub(in crate::features) fn open_new_session_all_sessions(&mut self) -> bool {
        if self.chrome.new_session_all_sessions_open {
            return false;
        }
        self.chrome.new_session_all_sessions_open = true;
        self.chrome.new_session_group_menu_path.clear();
        true
    }

    pub(in crate::features) fn close_new_session_all_sessions(&mut self) -> bool {
        let changed = self.chrome.new_session_all_sessions_open
            || !self.chrome.new_session_group_menu_path.is_empty();
        self.chrome.new_session_all_sessions_open = false;
        self.chrome.new_session_group_menu_path.clear();
        changed
    }

    pub(in crate::features) fn open_new_session_group(
        &mut self,
        group_id: String,
        depth: usize,
    ) -> bool {
        let unchanged = self.chrome.new_session_all_sessions_open
            && self.chrome.new_session_group_menu_path.get(depth) == Some(&group_id)
            && self.chrome.new_session_group_menu_path.len() == depth + 1;
        if unchanged {
            return false;
        }
        self.chrome.new_session_all_sessions_open = true;
        self.chrome.new_session_group_menu_path.truncate(depth);
        self.chrome.new_session_group_menu_path.push(group_id);
        true
    }

    pub(in crate::features) fn truncate_new_session_group_path(&mut self, depth: usize) -> bool {
        if self.chrome.new_session_group_menu_path.len() <= depth {
            return false;
        }
        self.chrome.new_session_group_menu_path.truncate(depth);
        true
    }

    pub(in crate::features) fn session_tab_scroll_into_view_pending(&self) -> bool {
        self.chrome.session_tab_scroll_into_view_pending
    }

    pub(in crate::features) fn consume_session_tab_scroll_into_view(&mut self) -> bool {
        std::mem::take(&mut self.chrome.session_tab_scroll_into_view_pending)
    }

    pub(in crate::features) fn session_tab_strip_scroll(&self) -> &ScrollHandle {
        &self.chrome.session_tab_strip_scroll
    }

    pub(in crate::features) fn session_tab_strip_has_overflow(&self) -> bool {
        self.chrome.session_tab_strip_has_overflow
    }

    pub(in crate::features) fn session_tab_strip_layout_is_current(
        &self,
        has_overflow: bool,
        viewport_width: Pixels,
    ) -> bool {
        self.chrome.session_tab_strip_has_overflow == has_overflow
            && self.chrome.session_tab_strip_viewport_width == Some(viewport_width)
    }

    pub(in crate::features) fn update_session_tab_strip_layout(
        &mut self,
        has_overflow: bool,
        viewport_width: Pixels,
    ) -> bool {
        let width_changed = self.chrome.session_tab_strip_viewport_width != Some(viewport_width);
        let overflow_changed = self.chrome.session_tab_strip_has_overflow != has_overflow;
        self.chrome.session_tab_strip_has_overflow = has_overflow;
        self.chrome.session_tab_strip_viewport_width = Some(viewport_width);
        if width_changed {
            self.chrome.session_tab_scroll_into_view_pending = true;
        }
        width_changed || overflow_changed
    }

    pub(in crate::features) fn request_session_tab_scroll_into_view(&mut self) {
        self.chrome.session_tab_scroll_into_view_pending = true;
    }

    pub(in crate::features) fn last_connect_failure_name(&self) -> Option<&str> {
        self.chrome.last_connect_failure_name.as_deref()
    }

    pub(in crate::features) fn last_connect_failure_error(&self) -> Option<&str> {
        self.chrome.last_connect_failure_error.as_deref()
    }

    pub(in crate::features) fn set_last_connect_failure(&mut self, name: String, error: String) {
        self.chrome.last_connect_failure_name = Some(name);
        self.chrome.last_connect_failure_error = Some(error);
    }

    pub(in crate::features) fn clear_last_connect_failure(&mut self) -> bool {
        self.chrome.last_connect_failure_name.take().is_some()
            | self.chrome.last_connect_failure_error.take().is_some()
    }

    pub(in crate::features) fn workspace_split(&self) -> Option<&WorkspaceSplitState> {
        self.workspace.split.as_ref()
    }

    pub(in crate::features) fn workspace_pane_root(
        &self,
        tab_root: &str,
    ) -> Option<&WorkspacePaneNode> {
        self.workspace.pane_roots.get(tab_root)
    }

    pub(in crate::features) fn workspace_pane_roots(&self) -> &HashMap<String, WorkspacePaneNode> {
        &self.workspace.pane_roots
    }

    pub(in crate::features) fn insert_workspace_pane_root(
        &mut self,
        tab_root: String,
        root: WorkspacePaneNode,
    ) {
        self.workspace.pane_roots.insert(tab_root, root);
        self.workspace.rebuild_tab_owners();
    }

    pub(in crate::features) fn workspace_tab_owner(&self, session_id: &str) -> Option<&str> {
        self.workspace.tab_owner.get(session_id).map(String::as_str)
    }

    pub(in crate::features) fn set_focused_terminal_leaf(&mut self, leaf_id: Option<String>) {
        self.workspace.focused_terminal_leaf_id = leaf_id;
    }

    pub(in crate::features) fn set_workspace_pane_layout_restored(&mut self, restored: bool) {
        self.workspace.pane_layout_restored = restored;
    }

    pub(in crate::features) fn replace_workspace_session_id(&mut self, old_id: &str, new_id: &str) {
        self.workspace.replace_session_id(old_id, new_id);
    }

    pub(in crate::features) fn remove_workspace_session(&mut self, session_id: &str) {
        self.workspace.remove_session(session_id);
    }

    pub(in crate::features) fn rekey_workspace_pane_root(
        &mut self,
        old_root: &str,
        new_root: String,
    ) -> bool {
        self.workspace.rekey_pane_root(old_root, new_root)
    }
}

impl ResizeHandleHoverState {
    pub(in crate::features) fn begin(&mut self, id: SharedString) -> Option<u64> {
        if self.pending_id.as_ref() == Some(&id) || self.active_id.as_ref() == Some(&id) {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending_id = Some(id);
        self.active_id = None;
        Some(self.generation)
    }

    pub(in crate::features) fn activate(&mut self, id: &SharedString, generation: u64) -> bool {
        if self.generation != generation || self.pending_id.as_ref() != Some(id) {
            return false;
        }
        self.pending_id = None;
        self.active_id = Some(id.clone());
        true
    }

    pub(in crate::features) fn activate_immediately(&mut self, id: SharedString) {
        self.generation = self.generation.wrapping_add(1);
        self.pending_id = None;
        self.active_id = Some(id);
    }

    pub(in crate::features) fn leave(&mut self, id: &SharedString) -> bool {
        let pending = self.pending_id.as_ref() == Some(id);
        let active = self.active_id.as_ref() == Some(id);
        if pending {
            self.generation = self.generation.wrapping_add(1);
            self.pending_id = None;
        }
        if active {
            self.active_id = None;
        }
        active
    }

    pub(in crate::features) fn is_highlighted(&self, id: &SharedString) -> bool {
        self.active_id.as_ref() == Some(id)
    }
}

impl ShellDiagnosticState {
    pub(in crate::features) fn should_log(
        &mut self,
        key: &'static str,
        now: Instant,
        throttle: Duration,
    ) -> bool {
        if self.last_log_at.get(key).is_some_and(|last| {
            now.checked_duration_since(*last)
                .is_some_and(|elapsed| elapsed < throttle)
        }) {
            return false;
        }
        self.last_log_at.insert(key, now);
        true
    }
}

impl ShellBottomPanelState {
    const QUICK_COMMANDS_HEIGHT_MIN: f32 = 36.;
    const COMMAND_SEND_HEIGHT_MIN: f32 = 60.;
    const HEIGHT_MAX: f32 = 520.;

    pub(in crate::features) fn start_resize(&mut self, start_y: Pixels) -> bool {
        let start_height = match self.mode {
            BottomPanelMode::QuickCommands => self.quick_commands_height,
            BottomPanelMode::CommandSend => self.command_send_height,
            BottomPanelMode::Hidden => return false,
        };
        self.resize = Some(BottomPanelResizeState {
            mode: self.mode,
            start_y,
            start_height: gpui::px(start_height),
        });
        true
    }

    pub(in crate::features) fn update_resize(&mut self, current_y: Pixels) -> Option<f32> {
        let state = self.resize?;
        let delta = f32::from(current_y - state.start_y);
        let minimum = match state.mode {
            BottomPanelMode::QuickCommands => Self::QUICK_COMMANDS_HEIGHT_MIN,
            BottomPanelMode::CommandSend => Self::COMMAND_SEND_HEIGHT_MIN,
            BottomPanelMode::Hidden => return None,
        };
        let next = (f32::from(state.start_height) - delta).clamp(minimum, Self::HEIGHT_MAX);
        match state.mode {
            BottomPanelMode::QuickCommands => self.quick_commands_height = next,
            BottomPanelMode::CommandSend => self.command_send_height = next,
            BottomPanelMode::Hidden => return None,
        }
        Some(next)
    }

    pub(in crate::features) fn finish_resize(&mut self) -> bool {
        self.resize.take().is_some()
    }
}

impl ShellViewportState {
    pub(in crate::features) fn update_size(&mut self, size: (f32, f32), now: Instant) -> bool {
        if self.size == size {
            return false;
        }
        self.size = size;
        self.last_change_at = Some(now);
        true
    }

    pub(in crate::features) fn mark_title_drag(&mut self, now: Instant, hold: Duration) {
        self.title_drag_active_until = Some(now + hold);
    }

    pub(in crate::features) fn title_drag_active(&self, now: Instant) -> bool {
        self.title_drag_active_until
            .is_some_and(|until| now < until)
    }
}

impl ShellPanelState {
    const LEFT_WIDTH_MIN: f32 = 160.;

    pub(in crate::features) fn set_open_mode(&mut self, mode: PanelOpenMode) {
        self.open_mode = mode;
    }

    pub(in crate::features) fn floating_panel(&self, side: PanelSide) -> Option<NavItem> {
        match side {
            PanelSide::Left => self.floating_left,
            PanelSide::Right => self.floating_right,
        }
    }

    pub(in crate::features) fn set_floating_panel(
        &mut self,
        side: PanelSide,
        panel: Option<NavItem>,
    ) {
        match side {
            PanelSide::Left => self.floating_left = panel,
            PanelSide::Right => self.floating_right = panel,
        }
        if panel.is_some() {
            self.last_floating_side = Some(side);
        } else if self.floating_left.is_none() && self.floating_right.is_none() {
            self.last_floating_side = None;
        }
    }

    pub(in crate::features) fn clear_floating(&mut self) {
        self.floating_left = None;
        self.floating_right = None;
        self.last_floating_side = None;
    }
    const LEFT_WIDTH_MAX: f32 = 720.;
    const RIGHT_WIDTH_MIN: f32 = 200.;
    const RIGHT_WIDTH_MAX: f32 = 720.;

    pub(in crate::features) fn start_resize(&mut self, side: PanelResizeSide, start_x: Pixels) {
        let start_width = match side {
            PanelResizeSide::Left => self.left_width,
            PanelResizeSide::Right => self.right_width,
        };
        self.resize = Some(PanelResizeState {
            side,
            start_x,
            start_width: gpui::px(start_width),
        });
    }

    pub(in crate::features) fn update_resize(
        &mut self,
        current_x: Pixels,
    ) -> Option<(PanelResizeSide, f32)> {
        let state = self.resize?;
        let delta = f32::from(current_x - state.start_x);
        let start = f32::from(state.start_width);
        let width = match state.side {
            PanelResizeSide::Left => {
                self.left_width = (start + delta).clamp(Self::LEFT_WIDTH_MIN, Self::LEFT_WIDTH_MAX);
                self.left_width
            }
            PanelResizeSide::Right => {
                self.right_width =
                    (start - delta).clamp(Self::RIGHT_WIDTH_MIN, Self::RIGHT_WIDTH_MAX);
                self.right_width
            }
        };
        Some((state.side, width))
    }

    pub(in crate::features) fn finish_resize(&mut self) -> bool {
        self.resize.take().is_some()
    }

    pub(in crate::features) fn stack_weight(&self, panel_id: &str) -> f32 {
        self.stack_sizes
            .get(panel_id)
            .copied()
            .filter(|value| value.is_finite() && *value > 0.)
            .unwrap_or(1.)
    }

    pub(in crate::features) fn start_stack_resize(
        &mut self,
        side: PanelSide,
        above_id: String,
        below_id: String,
        start_y: Pixels,
        container_height: f32,
    ) {
        self.stack_resize = Some(PanelStackResizeState {
            side,
            above_weight: self.stack_weight(&above_id),
            below_weight: self.stack_weight(&below_id),
            above_id,
            below_id,
            start_y,
            container_height: container_height.max(1.),
        });
    }

    pub(in crate::features) fn update_stack_resize(&mut self, current_y: Pixels) -> bool {
        let Some(state) = self.stack_resize.as_ref() else {
            return false;
        };
        let delta_px = f32::from(current_y - state.start_y);
        let pair = state.above_weight + state.below_weight;
        if pair <= 0. || state.container_height <= 0. {
            return false;
        }
        let px_per_weight = state.container_height / pair;
        let min_weight = (48. / px_per_weight).min(pair / 2.).max(0.05);
        let next_above =
            (state.above_weight + delta_px / px_per_weight).clamp(min_weight, pair - min_weight);
        let next_below = pair - next_above;
        self.stack_sizes.insert(state.above_id.clone(), next_above);
        self.stack_sizes.insert(state.below_id.clone(), next_below);
        true
    }

    pub(in crate::features) fn finish_stack_resize(&mut self) -> bool {
        self.stack_resize.take().is_some()
    }
}

impl ShellChromeState {
    pub(in crate::features) fn prepare_session_switch(&mut self) {
        self.open_tabs_menu_open = false;
        self.close_new_session_menu();
        self.session_tab_scroll_into_view_pending = true;
    }

    pub(in crate::features) fn toggle_open_tabs_menu(&mut self) {
        self.open_tabs_menu_open = !self.open_tabs_menu_open;
        if self.open_tabs_menu_open {
            self.close_new_session_menu();
        }
    }

    pub(in crate::features) fn close_open_tabs_menu(&mut self) -> bool {
        std::mem::take(&mut self.open_tabs_menu_open)
    }

    pub(in crate::features) fn open_new_session_menu(
        &mut self,
        anchor: NewSessionMenuAnchor,
    ) -> bool {
        if self.new_session_menu_anchor.as_ref() == Some(&anchor) {
            return false;
        }
        self.new_session_menu_anchor = Some(anchor);
        self.open_tabs_menu_open = false;
        self.new_session_all_sessions_open = false;
        self.new_session_group_menu_path.clear();
        true
    }

    pub(in crate::features) fn close_new_session_menu(&mut self) -> bool {
        let changed = self.new_session_menu_anchor.is_some()
            || self.new_session_all_sessions_open
            || !self.new_session_group_menu_path.is_empty();
        self.new_session_menu_anchor = None;
        self.new_session_all_sessions_open = false;
        self.new_session_group_menu_path.clear();
        changed
    }
}

impl ShellWorkspaceState {
    pub(in crate::features) fn rebuild_tab_owners(&mut self) {
        let mut owners = HashMap::new();
        for (tab_root, tree) in &self.pane_roots {
            for leaf in tree.session_ids() {
                owners.insert(leaf, tab_root.clone());
            }
        }
        self.tab_owner = owners;
    }

    pub(in crate::features) fn replace_session_id(&mut self, old_id: &str, new_id: &str) {
        for root in self.pane_roots.values_mut() {
            root.replace_session_id(old_id, new_id);
        }
        if let Some(root) = self.pane_roots.remove(old_id) {
            self.pane_roots.insert(new_id.to_string(), root);
        }
        if let Some(root) = self.split.as_mut() {
            root.replace_session_id(old_id, new_id);
        }
        self.rebuild_tab_owners();
    }

    pub(in crate::features) fn remove_session(&mut self, session_id: &str) {
        self.tab_owner.remove(session_id);
        self.pane_roots.remove(session_id);
    }

    pub(in crate::features) fn rekey_pane_root(
        &mut self,
        old_root: &str,
        new_root: String,
    ) -> bool {
        if old_root == new_root || self.pane_roots.contains_key(&new_root) {
            return false;
        }
        let Some(root) = self.pane_roots.remove(old_root) else {
            return false;
        };
        self.pane_roots.insert(new_root, root);
        self.rebuild_tab_owners();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use gpui::{RenderImage, SharedString, px};

    use super::{NewSessionMenuAnchor, ShellFeatureInit, ShellFeatureState};
    use crate::models::{
        ActivityBarLayoutState, BottomPanelMode, MainMode, NavItem, PanelResizeSide, PanelSide,
        WorkspacePaneNode, WorkspaceSplitDirection,
    };

    fn shell(mode: BottomPanelMode) -> ShellFeatureState {
        ShellFeatureState::new(ShellFeatureInit {
            status: "idle".to_string(),
            bottom_panel_mode: mode,
            quick_commands_height: 120.,
            command_send_height: 180.,
            active_left_panel: None,
            active_right_panel: None,
            left_open_panels: Vec::new(),
            right_open_panels: Vec::new(),
            panel_stack_sizes: HashMap::new(),
            panel_open_mode: crate::models::PanelOpenMode::Docked,
            panel_multi_open: false,
            left_sidebar_collapsed: true,
            right_inspector_collapsed: true,
            left_panel_width: 240.,
            right_panel_width: 320.,
            activity_bar_layout: ActivityBarLayoutState::default(),
        })
    }

    #[test]
    fn bottom_panel_resize_updates_only_the_mode_that_started_the_drag() {
        let mut shell = shell(BottomPanelMode::QuickCommands);

        assert!(shell.bottom_panel.start_resize(px(400.)));
        shell.bottom_panel.mode = BottomPanelMode::CommandSend;
        assert_eq!(shell.bottom_panel.update_resize(px(430.)), Some(90.));
        assert_eq!(shell.bottom_panel.quick_commands_height, 90.);
        assert_eq!(shell.bottom_panel.command_send_height, 180.);
        assert!(shell.bottom_panel.finish_resize());
        assert!(!shell.bottom_panel.finish_resize());
    }

    #[test]
    fn application_status_changes_only_through_owner_operations() {
        let mut shell = shell(BottomPanelMode::Hidden);

        assert_eq!(shell.status(), "idle");
        shell.set_status("connected");
        assert_eq!(shell.status(), "connected");
        shell.set_status(String::new());
        assert!(shell.status().is_empty());
    }

    #[test]
    fn wallpaper_cache_rejects_stale_background_results() {
        let mut shell = shell(BottomPanelMode::Hidden);
        let image = || {
            Arc::new(RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(1, 1),
            )]))
        };

        assert!(shell.request_wallpaper(Some("first.png".to_string())));
        assert!(!shell.request_wallpaper(Some("first.png".to_string())));
        assert!(shell.request_wallpaper(Some("second.png".to_string())));
        assert!(!shell.cache_wallpaper("first.png".to_string(), image(), 10, 20));
        assert!(shell.wallpaper_asset().is_none());

        assert!(shell.cache_wallpaper("second.png".to_string(), image(), 30, 40));
        assert_eq!(
            shell.wallpaper_asset().map(|asset| asset.dimensions()),
            Some((30, 40))
        );
        assert!(shell.request_wallpaper(None));
        assert!(shell.wallpaper_asset().is_none());
    }

    #[test]
    fn resize_handle_hover_ignores_stale_activation_and_reuses_same_dwell() {
        let mut shell = shell(BottomPanelMode::Hidden);
        let first = SharedString::from("first-resize-handle");
        let second = SharedString::from("second-resize-handle");

        let first_generation = shell
            .begin_resize_handle_hover(first.clone())
            .expect("first enter should arm dwell");
        assert!(shell.begin_resize_handle_hover(first.clone()).is_none());
        assert!(!shell.resize_handle_is_highlighted(&first));

        assert!(!shell.leave_resize_handle_hover(&first));
        assert!(!shell.activate_resize_handle_hover(&first, first_generation));

        let reentered_generation = shell
            .begin_resize_handle_hover(first.clone())
            .expect("re-enter should arm a new dwell");
        assert_ne!(reentered_generation, first_generation);
        assert!(shell.activate_resize_handle_hover(&first, reentered_generation));
        assert!(shell.resize_handle_is_highlighted(&first));
        assert!(shell.begin_resize_handle_hover(first.clone()).is_none());

        let second_generation = shell
            .begin_resize_handle_hover(second.clone())
            .expect("another handle should replace the active handle");
        assert!(!shell.resize_handle_is_highlighted(&first));
        assert!(shell.activate_resize_handle_hover(&second, second_generation));
        assert!(shell.leave_resize_handle_hover(&second));
        assert!(!shell.resize_handle_is_highlighted(&second));
    }

    #[test]
    fn resize_handle_mouse_down_activates_immediately_and_invalidates_pending_dwell() {
        let mut shell = shell(BottomPanelMode::Hidden);
        let id = SharedString::from("resize-handle");
        let generation = shell
            .begin_resize_handle_hover(id.clone())
            .expect("enter should arm dwell");

        shell.activate_resize_handle_immediately(id.clone());

        assert!(shell.resize_handle_is_highlighted(&id));
        assert!(!shell.activate_resize_handle_hover(&id, generation));
        assert!(shell.leave_resize_handle_hover(&id));
    }

    #[test]
    fn hidden_bottom_panel_does_not_start_resize() {
        let mut shell = shell(BottomPanelMode::Hidden);

        assert!(!shell.bottom_panel.start_resize(px(400.)));
        assert!(shell.bottom_panel.resize.is_none());
    }

    #[test]
    fn panel_resize_clamps_each_side_and_finishes_once() {
        let mut shell = shell(BottomPanelMode::Hidden);

        shell.panels.start_resize(PanelResizeSide::Left, px(100.));
        assert_eq!(
            shell.panels.update_resize(px(-100.)),
            Some((PanelResizeSide::Left, 160.))
        );
        assert!(shell.panels.finish_resize());
        assert!(!shell.panels.finish_resize());

        shell.panels.start_resize(PanelResizeSide::Right, px(100.));
        assert_eq!(
            shell.panels.update_resize(px(-500.)),
            Some((PanelResizeSide::Right, 720.))
        );
    }

    #[test]
    fn panel_stack_resize_preserves_pair_weight() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.panels.stack_sizes.insert("above".to_string(), 2.);
        shell.panels.stack_sizes.insert("below".to_string(), 1.);
        shell.panels.start_stack_resize(
            PanelSide::Left,
            "above".to_string(),
            "below".to_string(),
            px(100.),
            300.,
        );

        assert!(shell.panels.update_stack_resize(px(150.)));
        let total = shell.panels.stack_sizes["above"] + shell.panels.stack_sizes["below"];
        assert!((total - 3.).abs() < f32::EPSILON);
        assert!(shell.panels.finish_stack_resize());
        assert!(!shell.panels.finish_stack_resize());
    }

    #[test]
    fn chrome_menu_transitions_are_mutually_exclusive() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.chrome.new_session_menu_anchor = Some(NewSessionMenuAnchor::MainTabStrip);
        shell.chrome.new_session_all_sessions_open = true;
        shell
            .chrome
            .new_session_group_menu_path
            .push("group".to_string());

        shell.chrome.toggle_open_tabs_menu();
        assert!(shell.chrome.open_tabs_menu_open);
        assert!(shell.chrome.new_session_menu_anchor.is_none());
        assert!(!shell.chrome.new_session_all_sessions_open);
        assert!(shell.chrome.new_session_group_menu_path.is_empty());

        assert!(
            shell
                .chrome
                .open_new_session_menu(NewSessionMenuAnchor::MainTabStrip)
        );
        assert!(!shell.chrome.open_tabs_menu_open);
        assert_eq!(
            shell.chrome.new_session_menu_anchor,
            Some(NewSessionMenuAnchor::MainTabStrip)
        );

        assert!(
            shell
                .chrome
                .open_new_session_menu(NewSessionMenuAnchor::TerminalLeaf("right".to_string()))
        );
        assert_eq!(
            shell.chrome.new_session_menu_anchor,
            Some(NewSessionMenuAnchor::TerminalLeaf("right".to_string()))
        );
        assert!(
            !shell
                .chrome
                .open_new_session_menu(NewSessionMenuAnchor::TerminalLeaf("right".to_string()))
        );
    }

    #[test]
    fn root_menu_close_clears_every_owned_menu_branch() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.chrome.open_tabs_menu_open = true;
        shell.chrome.new_session_menu_anchor =
            Some(NewSessionMenuAnchor::TerminalLeaf("leaf".to_string()));
        shell.chrome.new_session_all_sessions_open = true;
        shell
            .chrome
            .new_session_group_menu_path
            .push("group".to_string());

        assert!(shell.close_root_menus());
        assert!(!shell.open_tabs_menu_is_open());
        assert!(!shell.new_session_menu_is_open());
        assert!(!shell.new_session_all_sessions_is_open());
        assert!(shell.new_session_group_menu_path().is_empty());
        assert!(!shell.close_root_menus());
    }

    #[test]
    fn tab_strip_layout_tracks_overflow_and_reveals_active_tab_after_resize() {
        let mut shell = shell(BottomPanelMode::Hidden);

        assert!(shell.update_session_tab_strip_layout(false, px(640.)));
        assert!(shell.session_tab_scroll_into_view_pending());
        assert!(shell.consume_session_tab_scroll_into_view());
        assert!(!shell.session_tab_scroll_into_view_pending());
        assert!(shell.session_tab_strip_layout_is_current(false, px(640.)));

        assert!(shell.update_session_tab_strip_layout(true, px(640.)));
        assert!(shell.session_tab_strip_has_overflow());
        assert!(!shell.session_tab_scroll_into_view_pending());

        assert!(shell.update_session_tab_strip_layout(true, px(480.)));
        assert!(shell.session_tab_scroll_into_view_pending());
        assert!(!shell.session_tab_strip_layout_is_current(true, px(640.)));
    }

    #[test]
    fn connection_failure_cleanup_always_clears_name_and_error() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.set_last_connect_failure("host".to_string(), "denied".to_string());

        assert!(shell.clear_last_connect_failure());
        assert_eq!(shell.last_connect_failure_name(), None);
        assert_eq!(shell.last_connect_failure_error(), None);
        assert!(!shell.clear_last_connect_failure());
    }

    #[test]
    fn finishing_embedded_settings_restores_navigation_and_panel_state() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.show_page(NavItem::Settings);
        shell.navigation.settings.previous_left_collapsed = Some(false);
        shell.navigation.settings.previous_right_collapsed = Some(true);
        shell.panels.left_collapsed = true;
        shell.panels.right_collapsed = false;

        assert!(shell.finish_settings_navigation());
        assert_eq!(shell.main_mode(), MainMode::Workspace);
        assert!(!shell.panels.left_collapsed);
        assert!(shell.panels.right_collapsed);
        assert_eq!(shell.navigation.settings.previous_left_collapsed, None);
        assert_eq!(shell.navigation.settings.previous_right_collapsed, None);
    }

    #[test]
    fn viewport_tracks_only_real_geometry_changes_and_title_drag_deadline() {
        let mut shell = shell(BottomPanelMode::Hidden);
        let now = Instant::now();
        assert!(!shell.viewport.update_size((1280., 800.), now));
        assert!(shell.viewport.update_size((1024., 768.), now));
        assert_eq!(shell.viewport.last_change_at, Some(now));

        shell
            .viewport
            .mark_title_drag(now, Duration::from_millis(10));
        assert!(shell.viewport.title_drag_active(now));
        assert!(
            !shell
                .viewport
                .title_drag_active(now + Duration::from_millis(10))
        );
    }

    #[test]
    fn diagnostic_throttle_is_keyed_and_advances_after_interval() {
        let mut shell = shell(BottomPanelMode::Hidden);
        let now = Instant::now();
        let throttle = Duration::from_secs(2);

        assert!(shell.diagnostics.should_log("session", now, throttle));
        assert!(
            !shell
                .diagnostics
                .should_log("session", now + Duration::from_secs(1), throttle)
        );
        assert!(shell.diagnostics.should_log("frame", now, throttle));
        assert!(
            shell
                .diagnostics
                .should_log("session", now + Duration::from_secs(2), throttle)
        );
        assert!(
            shell
                .diagnostics
                .should_log("clock", now + Duration::from_secs(2), throttle)
        );
        assert!(shell.diagnostics.should_log("clock", now, throttle));
    }

    #[test]
    fn workspace_rebuilds_and_renames_tab_ownership() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.workspace.pane_roots.insert(
            "root".to_string(),
            WorkspacePaneNode::Split {
                id: "split".to_string(),
                direction: WorkspaceSplitDirection::Vertical,
                ratio_percent: 50,
                first: Box::new(WorkspacePaneNode::leaf("root".to_string())),
                second: Box::new(WorkspacePaneNode::leaf("leaf".to_string())),
            },
        );
        shell.workspace.rebuild_tab_owners();
        assert_eq!(shell.workspace.tab_owner["leaf"], "root");

        shell.workspace.replace_session_id("root", "renamed");
        assert!(shell.workspace.pane_roots.contains_key("renamed"));
        assert_eq!(shell.workspace.tab_owner["leaf"], "renamed");
        assert_eq!(shell.workspace.tab_owner["renamed"], "renamed");
    }

    #[test]
    fn workspace_rekeys_split_root_before_closing_the_root_pane() {
        let mut shell = shell(BottomPanelMode::Hidden);
        shell.workspace.pane_roots.insert(
            "root".to_string(),
            WorkspacePaneNode::Split {
                id: "split".to_string(),
                direction: WorkspaceSplitDirection::Horizontal,
                ratio_percent: 50,
                first: Box::new(WorkspacePaneNode::leaf("root".to_string())),
                second: Box::new(WorkspacePaneNode::leaf("survivor".to_string())),
            },
        );
        shell.workspace.rebuild_tab_owners();

        assert!(
            shell
                .workspace
                .rekey_pane_root("root", "survivor".to_string())
        );
        shell.workspace.remove_session("root");

        assert!(!shell.workspace.pane_roots.contains_key("root"));
        assert!(shell.workspace.pane_roots.contains_key("survivor"));
        assert_eq!(shell.workspace.tab_owner["survivor"], "survivor");
    }

    #[test]
    fn docked_floating_mode_does_not_derive_multi_open() {
        use crate::models::PanelOpenMode;
        let mut shell = shell(BottomPanelMode::Hidden);

        assert_eq!(shell.panel_open_mode(), PanelOpenMode::Docked);
        assert!(!shell.panel_multi_open());
        assert!(!shell.panel_is_floating());

        shell.panels.multi_open = true;
        shell.panels.set_open_mode(PanelOpenMode::Floating);
        assert_eq!(shell.panel_open_mode(), PanelOpenMode::Floating);
        assert!(shell.panel_multi_open());
        assert!(shell.panel_is_floating());

        shell.panels.set_open_mode(PanelOpenMode::Docked);
        assert!(shell.panel_multi_open());
        assert!(!shell.panel_is_floating());
    }
}

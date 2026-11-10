use gpui::Pixels;
use std::hash::Hash;

/// Whether side panels are docked in the workspace flow or float above it.
/// Docked single/multi-open behavior remains an independent preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PanelOpenMode {
    #[default]
    Docked,
    Floating,
}

impl PanelOpenMode {
    pub(crate) fn from_setting(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("floating") {
            Self::Floating
        } else {
            Self::Docked
        }
    }

    pub(crate) fn as_setting(self) -> &'static str {
        match self {
            Self::Docked => "docked",
            Self::Floating => "floating",
        }
    }

    pub(crate) fn is_floating(self) -> bool {
        self == Self::Floating
    }
}

/// The activity-bar element a right-click context menu was opened against.
///
/// A menu opened on an entry can hide/move/reset that entry; a menu opened on
/// empty rail space (`Bar`) offers only rail-wide actions (show hidden, labels,
/// reset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActivityBarContextTarget {
    Entry {
        entry_id: String,
        zone: ActivityBarZone,
        index: usize,
    },
    Bar {
        side: PanelSide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NavItem {
    Workspace,
    Connections,
    Tunnels,
    Stats,
    GpuMonitor,
    AscendNpuMonitor,
    Processes,
    Docker,
    Ports,
    Services,
    Transfers,
    Notes,
    Settings,
    Plugins,
    AiAssistant,
    ActiveSessions,
    CommandHistory,
    SecurityAuth,
    SyncBackupHistory,
    Recording,
}

impl NavItem {
    pub(crate) fn i18n_key(self) -> Option<&'static str> {
        Some(match self {
            NavItem::Connections => "panel.savedConnections",
            NavItem::Tunnels => "panel.network",
            NavItem::Stats => "panel.resourceMonitor",
            NavItem::GpuMonitor => "panel.gpuMonitor",
            NavItem::AscendNpuMonitor => "panel.npuMonitor",
            NavItem::Processes => "panel.processManager",
            NavItem::Docker => "panel.dockerManager",
            NavItem::Ports => "hostTools.portsTitle",
            NavItem::Services => "hostTools.servicesTitle",
            NavItem::Transfers => "panel.fileExplorer",
            NavItem::Notes => "panel.notes",
            NavItem::Settings => "settings.title",
            NavItem::Plugins => "plugins.title",
            NavItem::AiAssistant => "ai.title",
            NavItem::ActiveSessions => "panel.activeSessions",
            NavItem::CommandHistory => "panel.commandHistory",
            NavItem::SecurityAuth => "securityAuth.title",
            NavItem::SyncBackupHistory => "panel.syncBackupHistory",
            NavItem::Recording => "recording.panelTitle",
            NavItem::Workspace => return None,
        })
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            NavItem::Workspace => "Workspace",
            NavItem::Connections => "Saved Connections",
            NavItem::Tunnels => "Network",
            NavItem::Stats => "Resource Monitor",
            NavItem::GpuMonitor => "GPU Monitor",
            NavItem::AscendNpuMonitor => "Ascend NPU Monitor",
            NavItem::Processes => "Process Manager",
            NavItem::Docker => "Docker",
            NavItem::Ports => "Ports",
            NavItem::Services => "Services",
            NavItem::Transfers => "File Explorer",
            NavItem::Notes => "Notes",
            NavItem::Settings => "Settings",
            NavItem::Plugins => "Plugins",
            NavItem::AiAssistant => "AI Assistant",
            NavItem::ActiveSessions => "Active Sessions",
            NavItem::CommandHistory => "Command History",
            NavItem::SecurityAuth => "Security / Auth",
            NavItem::SyncBackupHistory => "Sync / Backup",
            NavItem::Recording => "Recording",
        }
    }

    /// Compact panel title used in side PanelHeader (Tauri panel.* keys).
    pub(crate) fn panel_title(self) -> &'static str {
        match self {
            NavItem::Transfers => "Files",
            NavItem::Notes => "Notes",
            NavItem::Tunnels => "Network",
            NavItem::Connections => "Connections",
            NavItem::AiAssistant => "AI Assistant",
            NavItem::ActiveSessions => "Sessions",
            NavItem::CommandHistory => "History",
            NavItem::Stats => "Resources",
            NavItem::GpuMonitor => "GPU",
            NavItem::AscendNpuMonitor => "NPU",
            NavItem::Processes => "Processes",
            NavItem::Docker => "Docker",
            NavItem::Ports => "Ports",
            NavItem::Services => "Services",
            NavItem::SyncBackupHistory => "Cloud Sync",
            NavItem::SecurityAuth => "Security",
            NavItem::Recording => "Recording",
            NavItem::Settings => "Settings",
            NavItem::Plugins => "Plugins",
            NavItem::Workspace => "Workspace",
        }
    }

    /// Compact monochrome glyph used as text fallback for the activity bar.
    /// Bundled SVG path for activity-bar / toolbar icons.
    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            NavItem::Transfers => "icons/files.svg",
            NavItem::Notes => "icons/notes.svg",
            NavItem::Tunnels => "icons/network.svg",
            NavItem::SecurityAuth => "icons/auth.svg",
            NavItem::SyncBackupHistory => "icons/sync.svg",
            NavItem::Settings => "icons/settings.svg",
            NavItem::Plugins => "icons/extensions.svg",
            NavItem::Connections => "icons/connections.svg",
            NavItem::AiAssistant => "icons/ai.svg",
            NavItem::ActiveSessions => "icons/sessions.svg",
            NavItem::CommandHistory => "icons/history.svg",
            NavItem::Stats => "icons/resources.svg",
            NavItem::GpuMonitor => "icons/gpu.svg",
            NavItem::AscendNpuMonitor => "icons/npu.svg",
            NavItem::Processes => "icons/processes.svg",
            NavItem::Docker => "icons/docker.svg",
            NavItem::Ports => "icons/ports.svg",
            NavItem::Services => "icons/services.svg",
            NavItem::Recording => "icons/record.svg",
            NavItem::Workspace => "icons/workspace.svg",
        }
    }

    pub(crate) fn is_left_panel(self) -> bool {
        matches!(
            self,
            NavItem::Transfers
                | NavItem::Notes
                | NavItem::Tunnels
                | NavItem::SecurityAuth
                | NavItem::SyncBackupHistory
                | NavItem::Plugins
        )
    }

    pub(crate) fn is_right_panel(self) -> bool {
        matches!(
            self,
            NavItem::Connections
                | NavItem::AiAssistant
                | NavItem::ActiveSessions
                | NavItem::CommandHistory
                | NavItem::Stats
                | NavItem::GpuMonitor
                | NavItem::AscendNpuMonitor
                | NavItem::Processes
                | NavItem::Docker
                | NavItem::Ports
                | NavItem::Services
                | NavItem::Recording
                | NavItem::Workspace
        )
    }

    pub(crate) fn opens_settings(self) -> bool {
        matches!(self, NavItem::Settings)
    }

    /// Stable id compatible with Tauri `UiConfig` panel ids.
    pub(crate) fn persistence_id(self) -> &'static str {
        match self {
            NavItem::Workspace => "workspace",
            NavItem::Connections => "savedConnections",
            NavItem::Tunnels => "network",
            NavItem::Stats => "resourceMonitor",
            NavItem::GpuMonitor => "gpuMonitor",
            NavItem::AscendNpuMonitor => "ascendNpuMonitor",
            NavItem::Processes => "processManager",
            NavItem::Docker => "dockerManager",
            NavItem::Ports => "portsManager",
            NavItem::Services => "servicesManager",
            NavItem::Transfers => "fileExplorer",
            NavItem::Notes => "notes",
            NavItem::Settings => "settings",
            NavItem::Plugins => "plugins",
            NavItem::AiAssistant => "aiAssistant",
            NavItem::ActiveSessions => "activeSessions",
            NavItem::CommandHistory => "commandHistory",
            NavItem::SecurityAuth => "securityAuth",
            NavItem::SyncBackupHistory => "syncBackupHistory",
            NavItem::Recording => "recording",
        }
    }

    pub(crate) fn from_persistence_id(id: &str) -> Option<Self> {
        match id.trim() {
            "workspace" => Some(NavItem::Workspace),
            "connections" | "savedConnections" => Some(NavItem::Connections),
            "network" | "tunnels" => Some(NavItem::Tunnels),
            "stats" | "resourceMonitor" => Some(NavItem::Stats),
            "gpu" | "gpuMonitor" => Some(NavItem::GpuMonitor),
            "npu" | "ascendNpuMonitor" => Some(NavItem::AscendNpuMonitor),
            "processes" | "processManager" => Some(NavItem::Processes),
            "docker" | "dockerManager" => Some(NavItem::Docker),
            "ports" | "portsManager" => Some(NavItem::Ports),
            "services" | "servicesManager" => Some(NavItem::Services),
            "fileExplorer" | "fileTransfer" | "transfers" => Some(NavItem::Transfers),
            "notes" => Some(NavItem::Notes),
            "settings" => Some(NavItem::Settings),
            "plugins" => Some(NavItem::Plugins),
            "aiAssistant" | "ai" => Some(NavItem::AiAssistant),
            "activeSessions" => Some(NavItem::ActiveSessions),
            "commandHistory" => Some(NavItem::CommandHistory),
            "securityAuth" | "security" => Some(NavItem::SecurityAuth),
            "syncBackupHistory" | "syncBackup" => Some(NavItem::SyncBackupHistory),
            "recording" => Some(NavItem::Recording),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityBarZone {
    LeftTop,
    LeftBottom,
    RightTop,
    RightBottom,
}

impl ActivityBarZone {
    pub(crate) fn i18n_key(self) -> &'static str {
        match self {
            Self::LeftTop => "activityBar.leftTop",
            Self::LeftBottom => "activityBar.leftBottom",
            Self::RightTop => "activityBar.rightTop",
            Self::RightBottom => "activityBar.rightBottom",
        }
    }

    pub(crate) fn persistence_key(self) -> &'static str {
        match self {
            Self::LeftTop => "left_top",
            Self::LeftBottom => "left_bottom",
            Self::RightTop => "right_top",
            Self::RightBottom => "right_bottom",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LeftTop => "Left Top",
            Self::LeftBottom => "Left Bottom",
            Self::RightTop => "Right Top",
            Self::RightBottom => "Right Bottom",
        }
    }

    pub(crate) fn all() -> [Self; 4] {
        [
            Self::LeftTop,
            Self::LeftBottom,
            Self::RightTop,
            Self::RightBottom,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityBarEntry {
    Panel(NavItem),
    QuickCommands,
    CommandSend,
    Recording,
    Lock,
}

impl ActivityBarEntry {
    pub(crate) fn i18n_key(self) -> Option<&'static str> {
        match self {
            Self::Panel(item) => item.i18n_key(),
            Self::QuickCommands => Some("panel.quickCommands"),
            Self::CommandSend => Some("panel.serialSend"),
            Self::Recording => Some("recording.panelTitle"),
            Self::Lock => Some("statusBar.lock"),
        }
    }

    pub(crate) fn persistence_id(self) -> &'static str {
        match self {
            Self::Panel(item) => item.persistence_id(),
            Self::QuickCommands => "quickCmdBar",
            Self::CommandSend => "serialSend",
            Self::Recording => "recording",
            Self::Lock => "lock",
        }
    }

    pub(crate) fn from_persistence_id(id: &str) -> Option<Self> {
        match id.trim() {
            "quickCmdBar" => Some(Self::QuickCommands),
            "serialSend" => Some(Self::CommandSend),
            "recording" => Some(Self::Recording),
            "lock" => Some(Self::Lock),
            other => NavItem::from_persistence_id(other).map(Self::Panel),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Panel(item) => item.label(),
            Self::QuickCommands => "Quick Commands",
            Self::CommandSend => "Command Send",
            Self::Recording => "Recording",
            Self::Lock => "Lock",
        }
    }

    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::Panel(item) => item.icon_path(),
            Self::QuickCommands => "icons/commands.svg",
            Self::CommandSend => "icons/send.svg",
            Self::Recording => "icons/record.svg",
            Self::Lock => "icons/lock.svg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityBarLayoutState {
    pub(crate) left_top: Vec<String>,
    pub(crate) left_bottom: Vec<String>,
    pub(crate) right_top: Vec<String>,
    pub(crate) right_bottom: Vec<String>,
    /// Entry ids the user has hidden from the rail. Hidden entries keep their
    /// zone position so unhiding restores their place, but they are skipped
    /// when the rail is rendered. Mirrors Tauri `ui.activity_bar_layout.hidden_items`.
    pub(crate) hidden_items: Vec<String>,
    pub(crate) show_labels: bool,
}

impl Default for ActivityBarLayoutState {
    fn default() -> Self {
        let mut layout = Self {
            left_top: vec![
                "fileExplorer".to_string(),
                "notes".to_string(),
                "network".to_string(),
                "securityAuth".to_string(),
            ],
            left_bottom: vec![
                "syncBackupHistory".to_string(),
                "plugins".to_string(),
                "settings".to_string(),
            ],
            right_top: vec![
                "savedConnections".to_string(),
                "aiAssistant".to_string(),
                "activeSessions".to_string(),
                "commandHistory".to_string(),
                "resourceMonitor".to_string(),
                "gpuMonitor".to_string(),
                "ascendNpuMonitor".to_string(),
                "processManager".to_string(),
                "dockerManager".to_string(),
            ],
            right_bottom: vec![
                "quickCmdBar".to_string(),
                "serialSend".to_string(),
                "recording".to_string(),
                "lock".to_string(),
            ],
            hidden_items: nyaterm_core::models::settings::panel_visibility::default_hidden_items(),
            show_labels: false,
        };
        layout.ensure_host_tool_entries();
        layout
    }
}

impl ActivityBarLayoutState {
    /// Use the same compatibility projection for startup and optimistic
    /// settings comparisons; a missing optional slot is not an external edit.
    pub(crate) fn complete_settings_layout(settings: &mut nyaterm_core::AppSettingsSummary) {
        let mut layout = Self {
            left_top: settings.ui_activity_bar_left_top.clone(),
            left_bottom: settings.ui_activity_bar_left_bottom.clone(),
            right_top: settings.ui_activity_bar_right_top.clone(),
            right_bottom: settings.ui_activity_bar_right_bottom.clone(),
            hidden_items: settings.ui_activity_bar_hidden_items.clone(),
            show_labels: settings.ui_activity_bar_show_labels,
        };
        layout.ensure_plugin_entry();
        layout.ensure_remote_monitor_entries();
        layout.ensure_host_tool_entries();
        settings.ui_activity_bar_left_top = layout.left_top;
        settings.ui_activity_bar_left_bottom = layout.left_bottom;
        settings.ui_activity_bar_right_top = layout.right_top;
        settings.ui_activity_bar_right_bottom = layout.right_bottom;
        settings.ui_activity_bar_hidden_items = layout.hidden_items;
        for open in [
            &mut settings.ui_left_open_panels,
            &mut settings.ui_right_open_panels,
        ] {
            for id in open {
                match id.as_str() {
                    "ports" => *id = "portsManager".into(),
                    "services" => *id = "servicesManager".into(),
                    _ => {}
                }
            }
        }
    }

    /// Optional tools need slots for customization, but introducing a missing
    /// slot must never opt a user into showing it. Existing slots are choices.
    pub(crate) fn ensure_host_tool_entries(&mut self) {
        for zone in ActivityBarZone::all() {
            for id in self.zone_mut(zone) {
                match id.as_str() {
                    "ports" => *id = "portsManager".into(),
                    "services" => *id = "servicesManager".into(),
                    _ => {}
                }
            }
        }
        for id in &mut self.hidden_items {
            match id.as_str() {
                "ports" => *id = "portsManager".into(),
                "services" => *id = "servicesManager".into(),
                _ => {}
            }
        }
        for (id, anchor) in [
            ("portsManager", "dockerManager"),
            ("servicesManager", "portsManager"),
        ] {
            if self.find_entry(id).is_some() {
                continue;
            }
            let insert_at = self
                .right_top
                .iter()
                .position(|entry| entry == anchor)
                .map(|index| index + 1)
                .unwrap_or(self.right_top.len());
            self.right_top.insert(insert_at, id.into());
            self.hide_entry(id);
        }
    }

    /// Older store defaults omitted accelerator slots entirely. Restore only
    /// those missing slots, retaining unknown ids and all saved visibility.
    pub(crate) fn ensure_remote_monitor_entries(&mut self) {
        for (id, anchor) in [
            ("gpuMonitor", "resourceMonitor"),
            ("ascendNpuMonitor", "gpuMonitor"),
        ] {
            if self.find_entry(id).is_some() {
                continue;
            }
            let insert_at = self
                .right_top
                .iter()
                .position(|entry| entry == anchor)
                .map(|index| index + 1)
                .or_else(|| {
                    self.right_top
                        .iter()
                        .position(|entry| entry == "processManager")
                })
                .unwrap_or(self.right_top.len());
            self.right_top.insert(insert_at, id.into());
        }
    }

    /// Add the new entry before restoring panels, without changing saved placement.
    pub(crate) fn ensure_plugin_entry(&mut self) {
        if self.find_entry("plugins").is_some() {
            return;
        }
        let insert_at = self
            .left_bottom
            .iter()
            .position(|id| id == "settings")
            .unwrap_or(self.left_bottom.len());
        self.left_bottom.insert(insert_at, "plugins".to_string());
    }

    pub(crate) fn zone_mut(&mut self, zone: ActivityBarZone) -> &mut Vec<String> {
        match zone {
            ActivityBarZone::LeftTop => &mut self.left_top,
            ActivityBarZone::LeftBottom => &mut self.left_bottom,
            ActivityBarZone::RightTop => &mut self.right_top,
            ActivityBarZone::RightBottom => &mut self.right_bottom,
        }
    }

    pub(crate) fn zone(&self, zone: ActivityBarZone) -> &[String] {
        match zone {
            ActivityBarZone::LeftTop => &self.left_top,
            ActivityBarZone::LeftBottom => &self.left_bottom,
            ActivityBarZone::RightTop => &self.right_top,
            ActivityBarZone::RightBottom => &self.right_bottom,
        }
    }

    pub(crate) fn find_entry(&self, entry_id: &str) -> Option<(ActivityBarZone, usize)> {
        for zone in ActivityBarZone::all() {
            if let Some(index) = self.zone(zone).iter().position(|id| id == entry_id) {
                return Some((zone, index));
            }
        }
        None
    }

    pub(crate) fn side_for_entry(&self, entry_id: &str) -> Option<PanelSide> {
        self.find_entry(entry_id).map(|(zone, _)| match zone {
            ActivityBarZone::LeftTop | ActivityBarZone::LeftBottom => PanelSide::Left,
            ActivityBarZone::RightTop | ActivityBarZone::RightBottom => PanelSide::Right,
        })
    }

    pub(crate) fn first_panel_on_side(&self, side: PanelSide) -> Option<NavItem> {
        let zones = match side {
            PanelSide::Left => [ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom],
            PanelSide::Right => [ActivityBarZone::RightTop, ActivityBarZone::RightBottom],
        };
        zones
            .into_iter()
            .flat_map(|zone| self.zone(zone))
            .filter(|id| !self.is_hidden(id))
            .find_map(|id| NavItem::from_persistence_id(id).filter(|item| !item.opens_settings()))
    }

    pub(crate) fn is_hidden(&self, entry_id: &str) -> bool {
        self.hidden_items.iter().any(|id| id == entry_id)
    }

    /// Hide `entry_id` from the rail. Returns whether the hidden set changed.
    pub(crate) fn hide_entry(&mut self, entry_id: &str) -> bool {
        if self.find_entry(entry_id).is_none() || self.is_hidden(entry_id) {
            return false;
        }
        self.hidden_items.push(entry_id.to_string());
        true
    }

    /// Unhide `entry_id`. Returns whether the hidden set changed.
    pub(crate) fn show_entry(&mut self, entry_id: &str) -> bool {
        let before = self.hidden_items.len();
        self.hidden_items.retain(|id| id != entry_id);
        self.hidden_items.len() != before
    }

    #[cfg(test)]
    pub(crate) fn has_hidden_entries(&self) -> bool {
        !self.hidden_items.is_empty()
    }

    /// Hidden entry ids in rail order, so the "show hidden" menu is stable.
    #[cfg(test)]
    pub(crate) fn hidden_entries_ordered(&self) -> Vec<String> {
        let mut ordered = Vec::new();
        for zone in ActivityBarZone::all() {
            for id in self.zone(zone) {
                if self.is_hidden(id) {
                    ordered.push(id.clone());
                }
            }
        }
        ordered
    }

    /// Hidden entry ids on one side in rail order.
    pub(crate) fn hidden_entries_on_side(&self, side: PanelSide) -> Vec<String> {
        let zones = match side {
            PanelSide::Left => [ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom],
            PanelSide::Right => [ActivityBarZone::RightTop, ActivityBarZone::RightBottom],
        };
        zones
            .into_iter()
            .flat_map(|zone| self.zone(zone))
            .filter(|id| self.is_hidden(id))
            .cloned()
            .collect()
    }

    /// Merge a reorder of visible ids back into their existing visible slots.
    /// Hidden or unavailable ids never move.
    pub(crate) fn merge_visible_reorder<F>(
        &mut self,
        zone: ActivityBarZone,
        ordered_visible_ids: &[String],
        mut is_visible: F,
    ) where
        F: FnMut(&str) -> bool,
    {
        let ordered = ordered_visible_ids
            .iter()
            .cloned()
            .collect::<std::collections::VecDeque<_>>();
        let ordered_set = ordered_visible_ids
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut replacements = ordered;
        let current = self.zone(zone).to_vec();
        let mut merged = current
            .into_iter()
            .map(|id| {
                if ordered_set.contains(id.as_str()) && is_visible(&id) {
                    replacements.pop_front().unwrap_or(id)
                } else {
                    id
                }
            })
            .collect::<Vec<_>>();
        for id in replacements {
            if !merged.contains(&id) {
                merged.push(id);
            }
        }
        *self.zone_mut(zone) = merged;
    }

    /// Reset the rail to the shipped default layout, including the hidden set.
    pub(crate) fn reset_to_default(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActivityBarContextMenuState {
    pub(crate) target: ActivityBarContextTarget,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) move_submenu_open: bool,
}

impl ActivityBarContextMenuState {
    pub(crate) fn entry_id(&self) -> Option<&str> {
        match &self.target {
            ActivityBarContextTarget::Entry { entry_id, .. } => Some(entry_id.as_str()),
            ActivityBarContextTarget::Bar { .. } => None,
        }
    }

    pub(crate) fn entry_zone(&self) -> Option<ActivityBarZone> {
        match &self.target {
            ActivityBarContextTarget::Entry { zone, .. } => Some(*zone),
            ActivityBarContextTarget::Bar { .. } => None,
        }
    }
}

/// Top menubar dropdown (Tauri Header File/View/Terminal/Help).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleMenu {
    File,
    View,
    Terminal,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabActionsSubmenu {
    Color,
    MoveWindow,
    SshAdvanced,
    Ai,
}

impl TitleMenu {
    pub(crate) fn i18n_key(self) -> &'static str {
        match self {
            Self::File => "menu.file",
            Self::View => "menu.view",
            Self::Terminal => "menu.terminal",
            Self::Help => "menu.help",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::View => "View",
            Self::Terminal => "Terminal",
            Self::Help => "Help",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelSide {
    Left,
    Right,
}

pub(crate) fn panel_collapsed_from_persistence(
    configured_collapsed: bool,
    multi_open: bool,
    has_active_panel: bool,
    has_open_stack: bool,
) -> bool {
    configured_collapsed || (!has_active_panel && (!multi_open || !has_open_stack))
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityBarEntry, ActivityBarLayoutState, ActivityBarZone, NavItem, PanelOpenMode,
        PanelSide, panel_collapsed_from_persistence,
    };

    #[test]
    fn plugins_use_a_stable_panel_id_and_default_left_bottom_entry() {
        assert_eq!(NavItem::Plugins.persistence_id(), "plugins");
        assert_eq!(
            NavItem::from_persistence_id("plugins"),
            Some(NavItem::Plugins)
        );
        assert_eq!(
            ActivityBarEntry::from_persistence_id("plugins"),
            Some(ActivityBarEntry::Panel(NavItem::Plugins)),
        );
        assert!(NavItem::Plugins.is_left_panel());
        assert!(!NavItem::Plugins.opens_settings());
        assert_eq!(NavItem::Plugins.i18n_key(), Some("plugins.title"));
        assert_eq!(NavItem::Plugins.icon_path(), "icons/extensions.svg");
        let layout = ActivityBarLayoutState::default();
        assert_eq!(
            layout.left_bottom,
            ["syncBackupHistory", "plugins", "settings"]
        );
    }

    #[test]
    fn legacy_layout_adds_plugins_once_without_reordering_unknown_entries() {
        let mut layout = ActivityBarLayoutState {
            left_bottom: vec![
                "futureEntry".into(),
                "settings".into(),
                "syncBackupHistory".into(),
            ],
            hidden_items: vec!["futureEntry".into()],
            ..ActivityBarLayoutState::default()
        };
        let hidden = layout.hidden_items.clone();
        layout.ensure_plugin_entry();
        assert_eq!(
            layout.left_bottom,
            ["futureEntry", "plugins", "settings", "syncBackupHistory"]
        );
        let once = layout.clone();
        layout.ensure_plugin_entry();
        assert_eq!(layout, once);
        assert_eq!(layout.hidden_items, hidden);

        layout.left_bottom = vec!["futureEntry".into()];
        layout.ensure_plugin_entry();
        assert_eq!(layout.left_bottom, ["futureEntry", "plugins"]);
    }

    #[test]
    fn saved_plugin_placement_and_hidden_state_survive_layout_completion() {
        for zone in ActivityBarZone::all() {
            let mut layout = ActivityBarLayoutState::default();
            layout.left_bottom.retain(|id| id != "plugins");
            layout.zone_mut(zone).insert(0, "plugins".into());
            layout.hidden_items.push("plugins".into());
            let saved = layout.clone();
            layout.ensure_plugin_entry();
            assert_eq!(layout, saved);
        }
    }

    #[test]
    fn retired_migration_panel_is_ignored_when_loading_persisted_layouts() {
        assert_eq!(NavItem::from_persistence_id("migration"), None);
        assert_eq!(ActivityBarEntry::from_persistence_id("migration"), None);
        assert_eq!(
            ActivityBarEntry::from_persistence_id("notes"),
            Some(ActivityBarEntry::Panel(NavItem::Notes))
        );
    }

    #[test]
    fn activity_bar_entry_side_follows_current_layout() {
        let mut layout = ActivityBarLayoutState::default();
        assert_eq!(layout.side_for_entry("fileExplorer"), Some(PanelSide::Left));

        layout.left_top.retain(|id| id != "fileExplorer");
        layout.right_bottom.push("fileExplorer".to_string());

        assert_eq!(
            layout.side_for_entry("fileExplorer"),
            Some(PanelSide::Right)
        );
        assert_eq!(
            layout.first_panel_on_side(PanelSide::Right),
            Some(NavItem::Connections)
        );
        assert_eq!(layout.side_for_entry("missing"), None);
    }

    #[test]
    fn gpu_and_npu_panels_keep_tauri_activity_ids() {
        let layout = ActivityBarLayoutState::default();

        assert_eq!(
            NavItem::from_persistence_id("gpuMonitor"),
            Some(NavItem::GpuMonitor)
        );
        assert_eq!(
            NavItem::from_persistence_id("ascendNpuMonitor"),
            Some(NavItem::AscendNpuMonitor)
        );
        assert_eq!(
            ActivityBarEntry::from_persistence_id("gpuMonitor"),
            Some(ActivityBarEntry::Panel(NavItem::GpuMonitor))
        );
        assert_eq!(
            ActivityBarEntry::from_persistence_id("ascendNpuMonitor"),
            Some(ActivityBarEntry::Panel(NavItem::AscendNpuMonitor))
        );
        assert_eq!(layout.side_for_entry("gpuMonitor"), Some(PanelSide::Right));
        assert_eq!(
            layout.side_for_entry("ascendNpuMonitor"),
            Some(PanelSide::Right)
        );
        assert_eq!(
            layout
                .right_top
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "savedConnections",
                "aiAssistant",
                "activeSessions",
                "commandHistory",
                "resourceMonitor",
                "gpuMonitor",
                "ascendNpuMonitor",
                "processManager",
                "dockerManager",
                "portsManager",
                "servicesManager",
            ]
        );
    }

    #[test]
    fn ports_and_services_panels_have_stable_ids_and_sit_on_the_right() {
        assert_eq!(NavItem::Ports.persistence_id(), "portsManager");
        assert_eq!(NavItem::Services.persistence_id(), "servicesManager");
        assert_eq!(
            NavItem::from_persistence_id("servicesManager"),
            Some(NavItem::Services)
        );
        assert!(NavItem::Ports.is_right_panel() && !NavItem::Ports.is_left_panel());
        assert!(NavItem::Services.is_right_panel() && !NavItem::Services.is_left_panel());
        assert_eq!(
            ActivityBarLayoutState::default().side_for_entry("portsManager"),
            Some(PanelSide::Right)
        );
    }

    #[test]
    fn persisted_null_panel_closes_only_an_empty_side() {
        assert!(panel_collapsed_from_persistence(false, false, false, false));
        assert!(!panel_collapsed_from_persistence(false, false, true, false));
        assert!(!panel_collapsed_from_persistence(false, true, false, true));
        assert!(panel_collapsed_from_persistence(false, true, false, false));
        assert!(panel_collapsed_from_persistence(true, true, true, true));
    }

    #[test]
    fn panel_open_mode_parses_tauri_docked_and_floating_values() {
        assert_eq!(PanelOpenMode::from_setting("docked"), PanelOpenMode::Docked);
        assert_eq!(PanelOpenMode::from_setting("multi"), PanelOpenMode::Docked);
        assert_eq!(
            PanelOpenMode::from_setting("floating"),
            PanelOpenMode::Floating
        );
        assert_eq!(
            PanelOpenMode::from_setting("nonsense"),
            PanelOpenMode::Docked
        );
        assert_eq!(PanelOpenMode::Docked.as_setting(), "docked");
        assert_eq!(PanelOpenMode::Floating.as_setting(), "floating");
        assert!(!PanelOpenMode::Docked.is_floating());
        assert!(PanelOpenMode::Floating.is_floating());
    }

    #[test]
    fn hiding_an_entry_keeps_its_zone_slot_and_skips_it_from_first_panel() {
        let mut layout = ActivityBarLayoutState::default();
        assert!(!layout.is_hidden("fileExplorer"));
        assert_eq!(
            layout.first_panel_on_side(PanelSide::Left),
            Some(NavItem::Transfers)
        );

        assert!(layout.hide_entry("fileExplorer"));
        assert!(layout.is_hidden("fileExplorer"));
        // Slot preserved in the zone even while hidden.
        assert!(layout.left_top.iter().any(|id| id == "fileExplorer"));
        // Hiding is idempotent.
        assert!(!layout.hide_entry("fileExplorer"));
        // First visible panel now skips the hidden entry.
        assert_eq!(
            layout.first_panel_on_side(PanelSide::Left),
            Some(NavItem::Notes)
        );
        assert_eq!(
            layout.hidden_entries_ordered(),
            vec![
                "fileExplorer".to_string(),
                "gpuMonitor".to_string(),
                "ascendNpuMonitor".to_string(),
                "portsManager".to_string(),
                "servicesManager".to_string()
            ]
        );

        // Unknown ids cannot be hidden.
        assert!(!layout.hide_entry("does-not-exist"));

        assert!(layout.show_entry("fileExplorer"));
        assert!(!layout.is_hidden("fileExplorer"));
        assert!(!layout.show_entry("fileExplorer"));
        assert_eq!(
            layout.first_panel_on_side(PanelSide::Left),
            Some(NavItem::Transfers)
        );
    }

    #[test]
    fn missing_monitor_slots_are_restored_without_moving_existing_or_unknown_entries() {
        let mut layout = ActivityBarLayoutState {
            left_top: vec!["gpuMonitor".into(), "future".into()],
            right_top: vec!["resourceMonitor".into(), "processManager".into()],
            hidden_items: vec!["futureHidden".into(), "gpuMonitor".into()],
            ..ActivityBarLayoutState::default()
        };
        layout.ensure_remote_monitor_entries();
        assert_eq!(layout.left_top, ["gpuMonitor", "future"]);
        assert_eq!(
            layout.right_top,
            ["resourceMonitor", "ascendNpuMonitor", "processManager"]
        );
        assert_eq!(layout.hidden_items, ["futureHidden", "gpuMonitor"]);
        let before = layout.clone();
        layout.ensure_remote_monitor_entries();
        assert_eq!(layout, before);
    }

    #[test]
    fn visible_reorder_preserves_hidden_and_unavailable_slots() {
        let mut layout = ActivityBarLayoutState {
            left_top: ["a", "hidden", "unavailable", "c", "d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..ActivityBarLayoutState::default()
        };
        layout.hidden_items = vec!["hidden".to_string()];
        layout.merge_visible_reorder(
            ActivityBarZone::LeftTop,
            &["d".to_string(), "a".to_string(), "c".to_string()],
            |id| id != "hidden" && id != "unavailable",
        );
        assert_eq!(
            layout.left_top,
            ["d", "hidden", "unavailable", "a", "c"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn reset_restores_default_layout_including_hidden_set() {
        let mut layout = ActivityBarLayoutState::default();
        layout.hide_entry("aiAssistant");
        layout.left_top.retain(|id| id != "network");
        layout.right_bottom.push("network".to_string());
        assert!(layout.has_hidden_entries());

        layout.reset_to_default();
        assert_eq!(layout, ActivityBarLayoutState::default());
        assert!(layout.has_hidden_entries());
        assert_eq!(
            layout.hidden_items,
            ActivityBarLayoutState::default().hidden_items
        );
    }

    #[test]
    fn optional_host_tools_are_hidden_on_first_registration_and_keep_saved_choices() {
        let mut layout = ActivityBarLayoutState {
            left_top: vec!["futureEntry".into()],
            left_bottom: Vec::new(),
            right_top: vec!["dockerManager".into(), "after".into()],
            right_bottom: Vec::new(),
            hidden_items: vec!["futureEntry".into()],
            show_labels: true,
        };
        layout.ensure_host_tool_entries();
        assert_eq!(
            layout.right_top,
            ["dockerManager", "portsManager", "servicesManager", "after"]
        );
        assert_eq!(
            layout.hidden_items,
            ["futureEntry", "portsManager", "servicesManager"]
        );
        assert!(layout.show_entry("portsManager"));
        layout.right_top.retain(|id| id != "portsManager");
        layout.left_top.insert(0, "portsManager".into());
        let saved = layout.clone();
        layout.ensure_host_tool_entries();
        assert_eq!(layout, saved);
        assert!(!layout.is_hidden("portsManager"));
        assert!(layout.is_hidden("servicesManager"));
    }

    #[test]
    fn optional_host_tool_aliases_keep_placement_and_visibility_without_duplicates() {
        let mut layout = ActivityBarLayoutState {
            left_top: vec!["ports".into(), "futureEntry".into()],
            left_bottom: Vec::new(),
            right_top: Vec::new(),
            right_bottom: vec!["services".into()],
            hidden_items: vec!["services".into(), "futureEntry".into()],
            show_labels: false,
        };
        layout.ensure_host_tool_entries();
        assert_eq!(layout.left_top, ["portsManager", "futureEntry"]);
        assert_eq!(layout.right_bottom, ["servicesManager"]);
        assert!(layout.right_top.is_empty());
        assert_eq!(layout.hidden_items, ["servicesManager", "futureEntry"]);
        let saved = layout.clone();
        layout.ensure_host_tool_entries();
        assert_eq!(layout, saved);
    }

    #[test]
    fn an_empty_layout_offers_host_tools_as_hidden_customization_entries() {
        let mut layout = ActivityBarLayoutState {
            left_top: Vec::new(),
            left_bottom: Vec::new(),
            right_top: Vec::new(),
            right_bottom: Vec::new(),
            hidden_items: Vec::new(),
            show_labels: false,
        };
        layout.ensure_host_tool_entries();
        assert_eq!(layout.right_top, ["portsManager", "servicesManager"]);
        assert_eq!(layout.hidden_items, layout.right_top);
        assert_eq!(layout.first_panel_on_side(PanelSide::Right), None);
    }
}

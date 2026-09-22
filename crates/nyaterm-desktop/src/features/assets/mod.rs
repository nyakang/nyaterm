use std::collections::HashSet;

use gpui::{AppContext as _, Context, Entity, ScrollHandle, Subscription, UniformListScrollHandle};
use nyaterm_core::{
    AssetAcceleratorSnapshot, AssetAcceleratorType, AssetDiskSnapshot, AssetDisplayLabels,
    AssetFilterKey, AssetMonitoringCache, AssetRecord, AssetSortDirection, AssetSortKey,
    AssetSortState, AssetStatsSnapshot, AssetViewMode, ConnectionType, Group,
    RecordAssetMonitoringPatch, SavedConnection, StartWorkspaceMode,
    build_asset_patch_from_accelerator_snapshot, build_asset_patch_from_stats_snapshot,
    build_asset_records, build_group_options, connection_matches_filters,
    normalize_asset_sort_state, parse_start_workspace_mode, records_in_group, sort_asset_records,
};
use nyaterm_transport::{RemoteGpuOverview, RemoteNpuOverview, RemoteStats};
use nyaterm_ui::{NyaInputEvent, NyaInputState, NyaSelectEvent, NyaSelectOption, NyaSelectState};

use crate::features::NyaTermApp;

pub(in crate::features) const ASSET_TABLE_ROW_HEIGHT: f32 = 56.;
// The Tauri table uses an 88px action column. GPUI scrollbars overlay their
// viewport, so keep another 16px clear on the right for the vertical track.
pub(in crate::features) const ASSET_TABLE_ACTIONS_WIDTH: f32 = 104.;
pub(in crate::features) const ASSET_CARD_ROW_HEIGHT: f32 = 198.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum AssetColumn {
    Name,
    Address,
    ConnectionTime,
    Cpu,
    Memory,
    Storage,
    Accelerators,
}

impl AssetColumn {
    pub const ALL: [Self; 7] = [
        Self::Name,
        Self::Address,
        Self::ConnectionTime,
        Self::Cpu,
        Self::Memory,
        Self::Storage,
        Self::Accelerators,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::Address => 1,
            Self::ConnectionTime => 2,
            Self::Cpu => 3,
            Self::Memory => 4,
            Self::Storage => 5,
            Self::Accelerators => 6,
        }
    }

    pub fn sort_key(self) -> AssetSortKey {
        match self {
            Self::Name => AssetSortKey::Name,
            Self::Address => AssetSortKey::Address,
            Self::ConnectionTime => AssetSortKey::ConnectionTime,
            Self::Cpu => AssetSortKey::Cpu,
            Self::Memory => AssetSortKey::Memory,
            Self::Storage => AssetSortKey::Storage,
            Self::Accelerators => AssetSortKey::Accelerators,
        }
    }
}

#[derive(Clone, Copy)]
struct AssetColumnResize {
    column: AssetColumn,
    pointer_x: f32,
    width: f32,
}

pub(in crate::features) struct StartWorkspaceFeatureState {
    mode: StartWorkspaceMode,
    search_field: Entity<NyaInputState>,
    search: String,
    _search_subscription: Subscription,
    group_select: Entity<NyaSelectState>,
    _group_subscription: Subscription,
    filters: HashSet<AssetFilterKey>,
    selected_group_id: Option<String>,
    view_mode: AssetViewMode,
    sort: Option<AssetSortState>,
    column_widths: [f32; 7],
    column_resize: Option<AssetColumnResize>,
    table_horizontal_scroll: ScrollHandle,
    list_scroll: UniformListScrollHandle,
    card_scroll: UniformListScrollHandle,
    card_columns: usize,
    monitoring: AssetMonitoringCache,
}

impl StartWorkspaceFeatureState {
    pub fn new(
        groups: &[Group],
        settings: &nyaterm_core::AppSettingsSummary,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let search_field = cx.new(|cx| {
            NyaInputState::new(cx, String::new())
                .placeholder(rust_i18n::t!("assets.searchPlaceholder"))
        });
        let search_subscription = cx.subscribe(
            &search_field,
            |app: &mut NyaTermApp, _, event: &NyaInputEvent, cx| {
                if let NyaInputEvent::Changed(value) | NyaInputEvent::Submitted(value) = event {
                    app.start_workspace.search = value.clone();
                    cx.notify();
                }
            },
        );

        let mut options = vec![NyaSelectOption::new("", rust_i18n::t!("assets.title"))];
        options.extend(
            build_group_options(groups, &rust_i18n::t!("assets.title"))
                .into_iter()
                .map(|option| {
                    NyaSelectOption::new(option.group.id, option.path)
                        .search_text(option.search_text)
                }),
        );
        let group_select = cx.new(|cx| {
            NyaSelectState::new(cx, options, Some(String::new()))
                .placeholder(rust_i18n::t!("assets.groupPickerPlaceholder"))
                .search_placeholder(rust_i18n::t!("assets.groupPickerPlaceholder"))
                .searchable(true)
        });
        let group_subscription = cx.subscribe(
            &group_select,
            |app: &mut NyaTermApp, _, event: &NyaSelectEvent, cx| {
                let NyaSelectEvent::Changed(value) = event;
                app.start_workspace.selected_group_id =
                    value.clone().filter(|value| !value.is_empty());
                cx.notify();
            },
        );

        Self {
            mode: parse_start_workspace_mode(&settings.ui_start_workspace_mode),
            search_field,
            search: String::new(),
            _search_subscription: search_subscription,
            group_select,
            _group_subscription: group_subscription,
            filters: HashSet::new(),
            selected_group_id: None,
            view_mode: AssetViewMode::List,
            sort: normalize_asset_sort_state(
                settings.ui_asset_sort_key.as_deref(),
                settings.ui_asset_sort_direction.as_deref(),
            ),
            column_widths: [280., 150., 170., 110., 110., 120., 190.],
            column_resize: None,
            table_horizontal_scroll: ScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            card_scroll: UniformListScrollHandle::new(),
            card_columns: 3,
            monitoring: AssetMonitoringCache::new(),
        }
    }

    pub fn mode(&self) -> StartWorkspaceMode {
        self.mode
    }
    pub fn set_mode(&mut self, mode: StartWorkspaceMode) -> bool {
        if self.mode == mode {
            false
        } else {
            self.mode = mode;
            true
        }
    }
    pub fn search_field(&self) -> Entity<NyaInputState> {
        self.search_field.clone()
    }
    pub fn group_select(&self) -> Entity<NyaSelectState> {
        self.group_select.clone()
    }
    pub fn sync_group_options(&mut self, groups: &[Group], cx: &mut Context<NyaTermApp>) {
        if self
            .selected_group_id
            .as_ref()
            .is_some_and(|selected| !groups.iter().any(|group| &group.id == selected))
        {
            self.selected_group_id = None;
        }
        let mut options = vec![NyaSelectOption::new("", rust_i18n::t!("assets.title"))];
        options.extend(
            build_group_options(groups, &rust_i18n::t!("assets.title"))
                .into_iter()
                .map(|option| {
                    NyaSelectOption::new(option.group.id, option.path)
                        .search_text(option.search_text)
                }),
        );
        let selected = self.selected_group_id.clone().unwrap_or_default();
        self.group_select.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_selected_value(Some(selected), cx);
        });
    }
    pub fn selected_group_id(&self) -> Option<&str> {
        self.selected_group_id.as_deref()
    }
    pub fn filters(&self) -> &HashSet<AssetFilterKey> {
        &self.filters
    }
    pub fn toggle_filter(&mut self, filter: AssetFilterKey) {
        if !self.filters.remove(&filter) {
            self.filters.insert(filter);
        }
    }
    pub fn clear_filters(&mut self) {
        self.filters.clear();
    }
    pub fn view_mode(&self) -> AssetViewMode {
        self.view_mode
    }
    pub fn set_view_mode(&mut self, mode: AssetViewMode) {
        self.view_mode = mode;
    }
    pub fn sort(&self) -> Option<AssetSortState> {
        self.sort
    }
    pub fn cycle_sort(&mut self, key: AssetSortKey) {
        self.sort = Some(match self.sort {
            Some(current) if current.key == key => AssetSortState {
                key,
                direction: if current.direction == AssetSortDirection::Asc {
                    AssetSortDirection::Desc
                } else {
                    AssetSortDirection::Asc
                },
            },
            _ => AssetSortState {
                key,
                direction: AssetSortDirection::Asc,
            },
        });
    }
    pub fn column_width(&self, column: AssetColumn) -> f32 {
        self.column_widths[column.index()]
    }
    pub fn table_width(&self) -> f32 {
        self.column_widths.iter().sum::<f32>() + ASSET_TABLE_ACTIONS_WIDTH
    }
    pub fn begin_column_resize(&mut self, column: AssetColumn, pointer_x: f32) {
        self.column_resize = Some(AssetColumnResize {
            column,
            pointer_x,
            width: self.column_width(column),
        });
    }
    pub fn update_column_resize(&mut self, pointer_x: f32) -> bool {
        let Some(resize) = self.column_resize else {
            return false;
        };
        let minimum = match resize.column {
            AssetColumn::Name => 180.,
            AssetColumn::Address => 120.,
            AssetColumn::ConnectionTime => 140.,
            AssetColumn::Cpu => 80.,
            AssetColumn::Memory => 88.,
            AssetColumn::Storage => 96.,
            AssetColumn::Accelerators => 140.,
        };
        let width = (resize.width + pointer_x - resize.pointer_x)
            .max(minimum)
            .round();
        let slot = &mut self.column_widths[resize.column.index()];
        if (*slot - width).abs() < f32::EPSILON {
            false
        } else {
            *slot = width;
            true
        }
    }
    pub fn finish_column_resize(&mut self) -> bool {
        self.column_resize.take().is_some()
    }
    pub fn table_horizontal_scroll(&self) -> &ScrollHandle {
        &self.table_horizontal_scroll
    }
    pub fn list_scroll(&self) -> &UniformListScrollHandle {
        &self.list_scroll
    }
    pub fn card_scroll(&self) -> &UniformListScrollHandle {
        &self.card_scroll
    }
    pub fn card_columns(&self) -> usize {
        self.card_columns
    }
    pub fn set_card_columns(&mut self, columns: usize) -> bool {
        let columns = columns.clamp(1, 3);
        if self.card_columns == columns {
            false
        } else {
            self.card_columns = columns;
            true
        }
    }
    pub fn monitoring_mut(&mut self) -> &mut AssetMonitoringCache {
        &mut self.monitoring
    }

    pub fn records(
        &self,
        connections: &[SavedConnection],
        groups: &[Group],
        labels: &AssetDisplayLabels,
        root_label: &str,
    ) -> Vec<AssetRecord> {
        let records = build_asset_records(connections, groups, root_label);
        let mut records = records_in_group(
            records,
            connections,
            groups,
            self.selected_group_id.as_deref(),
        );
        let query = self.search.trim().to_lowercase();
        let filters = self.filters.iter().cloned().collect::<Vec<_>>();
        records.retain(|record| {
            (query.is_empty() || record.search_text.contains(&query))
                && connection_matches_filters(&record.connection, &filters)
        });
        sort_asset_records(&mut records, self.sort, labels);
        records
    }
}

impl NyaTermApp {
    fn monitored_ssh_connection_id(&self, session_id: &str) -> Option<String> {
        let connection_id = self
            .session
            .metadata(session_id)?
            .source_connection_id
            .as_deref()?
            .trim();
        if connection_id.is_empty() {
            return None;
        }
        self.connection_state
            .connections()
            .iter()
            .find(|connection| {
                connection.id == connection_id
                    && matches!(connection.config, ConnectionType::Ssh { .. })
            })
            .map(|connection| connection.id.clone())
    }

    pub(in crate::features) fn cache_asset_stats_snapshot(
        &mut self,
        session_id: &str,
        stats: &RemoteStats,
    ) -> bool {
        let Some(connection_id) = self.monitored_ssh_connection_id(session_id) else {
            return false;
        };
        let snapshot = AssetStatsSnapshot {
            hostname: stats.system.hostname.clone(),
            os: stats.system.os.clone(),
            arch: stats.system.arch.clone(),
            cpu_model: stats.cpu.model.clone(),
            cpu_cores: stats.cpu.cores,
            memory_total_bytes: stats.memory.used.saturating_add(stats.memory.available),
            disks: stats
                .disks
                .iter()
                .map(|disk| AssetDiskSnapshot {
                    device: disk.device.clone(),
                    mount: disk.mount.clone(),
                    total_bytes: disk.total,
                })
                .collect(),
        };
        self.start_workspace
            .monitoring_mut()
            .record(RecordAssetMonitoringPatch {
                source_session_id: session_id,
                target_session_id: session_id,
                connection_id: &connection_id,
                patch: build_asset_patch_from_stats_snapshot(&snapshot),
            })
    }

    pub(in crate::features) fn cache_asset_gpu_snapshot(
        &mut self,
        session_id: &str,
        overview: &RemoteGpuOverview,
    ) -> bool {
        let Some(connection_id) = self.monitored_ssh_connection_id(session_id) else {
            return false;
        };
        let devices = overview
            .gpus
            .iter()
            .map(|gpu| AssetAcceleratorSnapshot {
                vendor: "NVIDIA".to_string(),
                model: gpu.name.clone(),
                memory_total_mb: gpu.memory_total_mb,
            })
            .collect::<Vec<_>>();
        self.start_workspace
            .monitoring_mut()
            .record(RecordAssetMonitoringPatch {
                source_session_id: session_id,
                target_session_id: session_id,
                connection_id: &connection_id,
                patch: build_asset_patch_from_accelerator_snapshot(
                    AssetAcceleratorType::Gpu,
                    "NVIDIA",
                    overview.available,
                    &devices,
                ),
            })
    }

    pub(in crate::features) fn cache_asset_npu_snapshot(
        &mut self,
        session_id: &str,
        overview: &RemoteNpuOverview,
    ) -> bool {
        let Some(connection_id) = self.monitored_ssh_connection_id(session_id) else {
            return false;
        };
        let devices = overview
            .npus
            .iter()
            .map(|npu| AssetAcceleratorSnapshot {
                vendor: "Huawei".to_string(),
                model: npu.name.clone(),
                memory_total_mb: npu.hbm_total_mb.unwrap_or(npu.memory_total_mb),
            })
            .collect::<Vec<_>>();
        self.start_workspace
            .monitoring_mut()
            .record(RecordAssetMonitoringPatch {
                source_session_id: session_id,
                target_session_id: session_id,
                connection_id: &connection_id,
                patch: build_asset_patch_from_accelerator_snapshot(
                    AssetAcceleratorType::Npu,
                    "Huawei",
                    overview.available,
                    &devices,
                ),
            })
    }
}

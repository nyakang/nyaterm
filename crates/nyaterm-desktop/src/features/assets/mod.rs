use crate::features::NyaTermApp;
use gpui::{
    AppContext as _, Context, Entity, ScrollHandle, SharedString, Subscription,
    UniformListScrollHandle,
};
use nyaterm_core::{
    AssetAcceleratorSnapshot, AssetAcceleratorType, AssetDiskSnapshot, AssetDisplayLabels,
    AssetFilterKey, AssetMonitoringCache, AssetRecord, AssetSortDirection, AssetSortKey,
    AssetSortState, AssetStatsSnapshot, AssetViewMode, ConnectionType, Group,
    RecordAssetMonitoringPatch, SavedConnection, StartWorkspaceMode,
    build_asset_patch_from_accelerator_snapshot, build_asset_patch_from_stats_snapshot,
    build_asset_records, build_group_options, connection_matches_filters,
    normalize_asset_sort_state, parse_start_workspace_mode, records_in_group, sort_asset_records,
};
use nyaterm_remote_ops::ascend_npu::RemoteNpuOverview;
use nyaterm_remote_ops::gpu::RemoteGpuOverview;
use nyaterm_remote_ops::stats::RemoteStats;
use nyaterm_ui::{
    ChildWindowSlot, NyaInputEvent, NyaInputState, NyaSelectEvent, NyaSelectOption, NyaSelectState,
    NyaWindowHandle,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod batch_edit;

#[cfg(test)]
pub(in crate::features) use batch_edit::AssetBatchSaveSummary;
pub(in crate::features) use batch_edit::{
    AssetBatchEditDraft, BatchEditMode, BatchFieldId, FieldBasis,
};

pub(in crate::features) const ASSET_TABLE_ROW_HEIGHT: f32 = 56.;
// The Tauri table uses an 88px action column. GPUI scrollbars overlay their
// viewport, so keep another 16px clear on the right for the vertical track.
pub(in crate::features) const ASSET_TABLE_ACTIONS_WIDTH: f32 = 104.;
pub(in crate::features) const ASSET_SELECT_COLUMN_WIDTH: f32 = 40.;
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
    context_connection_id: Option<String>,
    sort: Option<AssetSortState>,
    column_widths: [f32; 7],
    column_resize: Option<AssetColumnResize>,
    table_horizontal_scroll: ScrollHandle,
    list_scroll: UniformListScrollHandle,
    card_scroll: UniformListScrollHandle,
    card_columns: usize,
    monitoring: AssetMonitoringCache,
    record_source_key: Option<((u64, u64), String)>,
    source_records: Vec<AssetRecord>,
    record_view_key: Option<AssetRecordViewKey>,
    visible_records: Arc<[AssetRecord]>,
    group_options_key: Option<(Vec<Group>, String)>,
    group_options_revision: Option<(u64, String)>,
    asset_selection: HashSet<String>,
    asset_selection_anchor: Option<String>,
    asset_hovered_id: Option<String>,
    asset_batch: Box<AssetBatchEditDraft>,
}

#[derive(Clone, PartialEq, Eq)]
struct AssetRecordViewKey {
    search: String,
    filters: HashSet<AssetFilterKey>,
    group: Option<String>,
    sort: Option<AssetSortState>,
    labels: AssetDisplayLabels,
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
            context_connection_id: None,
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
            record_source_key: None,
            source_records: Vec::new(),
            record_view_key: None,
            visible_records: Arc::from([]),
            group_options_key: None,
            group_options_revision: None,
            asset_selection: HashSet::new(),
            asset_selection_anchor: None,
            asset_hovered_id: None,
            asset_batch: Box::default(),
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
    pub fn context_connection_id(&self) -> Option<&str> {
        self.context_connection_id.as_deref()
    }

    pub fn set_context_connection_id(&mut self, connection_id: Option<String>) {
        self.context_connection_id = connection_id;
    }

    pub fn is_asset_selected(&self, connection_id: &str) -> bool {
        self.asset_selection.contains(connection_id)
    }

    pub fn selected_asset_count(&self) -> usize {
        self.asset_selection.len()
    }

    pub fn toggle_asset_selection(&mut self, connection_id: &str) -> bool {
        if self.asset_selection.remove(connection_id) {
            if self.asset_selection_anchor.as_deref() == Some(connection_id) {
                self.asset_selection_anchor = None;
            }
            true
        } else {
            self.asset_selection.insert(connection_id.to_string());
            self.asset_selection_anchor = Some(connection_id.to_string());
            true
        }
    }

    pub fn select_asset_only(&mut self, connection_id: &str) -> bool {
        if self.asset_selection.len() == 1
            && self.asset_selection.contains(connection_id)
            && self.asset_selection_anchor.as_deref() == Some(connection_id)
        {
            return false;
        }
        self.asset_selection.clear();
        self.asset_selection.insert(connection_id.to_string());
        self.asset_selection_anchor = Some(connection_id.to_string());
        true
    }

    /// Shift range selection over the currently visible order.
    pub fn select_asset_range(
        &mut self,
        records: &[AssetRecord],
        connection_id: &str,
        additive: bool,
    ) -> bool {
        let anchor = self
            .asset_selection_anchor
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        let next: HashSet<String> = if additive {
            self.asset_selection.clone()
        } else {
            HashSet::new()
        };
        self.asset_selection = next;

        if let (Some(start), Some(end)) = (
            records
                .iter()
                .position(|record| record.connection.id == anchor),
            records
                .iter()
                .position(|record| record.connection.id == connection_id),
        ) {
            let (low, high) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            for record in &records[low..=high] {
                self.asset_selection.insert(record.connection.id.clone());
            }
        } else {
            self.asset_selection.insert(connection_id.to_string());
            self.asset_selection_anchor = Some(connection_id.to_string());
        }
        true
    }

    pub fn select_all_assets(&mut self, records: &[AssetRecord]) -> bool {
        let mut changed = false;
        for record in records {
            if self.asset_selection.insert(record.connection.id.clone()) {
                changed = true;
            }
        }
        if let Some(record) = records.first() {
            self.asset_selection_anchor = Some(record.connection.id.clone());
        }
        changed
    }

    pub fn deselect_visible_assets(&mut self, records: &[AssetRecord]) -> bool {
        let before = self.asset_selection.len();
        for record in records {
            self.asset_selection.remove(&record.connection.id);
        }
        before != self.asset_selection.len()
    }
    pub fn visible_selected_count(&self, records: &[AssetRecord]) -> usize {
        records
            .iter()
            .filter(|record| self.is_asset_selected(&record.connection.id))
            .count()
    }
    pub fn hidden_selected_count(&self, records: &[AssetRecord]) -> usize {
        self.selected_asset_count()
            .saturating_sub(self.visible_selected_count(records))
    }
    pub fn is_asset_hovered(&self, id: &str) -> bool {
        self.asset_hovered_id.as_deref() == Some(id)
    }
    pub fn set_asset_hovered(&mut self, id: &str, hovered: bool) -> bool {
        if hovered {
            if self.is_asset_hovered(id) {
                return false;
            }
            self.asset_hovered_id = Some(id.into());
        } else {
            if !self.is_asset_hovered(id) {
                return false;
            }
            self.asset_hovered_id = None;
        }
        true
    }
    pub fn clear_asset_selection(&mut self) -> bool {
        if self.asset_selection.is_empty() && self.asset_selection_anchor.is_none() {
            false
        } else {
            self.asset_selection.clear();
            self.asset_selection_anchor = None;
            true
        }
    }

    pub fn all_assets_selected(&self, records: &[AssetRecord]) -> bool {
        !records.is_empty()
            && records
                .iter()
                .all(|record| self.asset_selection.contains(&record.connection.id))
    }

    /// The assets that the context menu of CONNECTION_ID acts on: the whole
    /// selection when the clicked asset belongs to it, as in the connection
    /// panel, otherwise the clicked asset alone.
    pub fn asset_menu_target_ids(&self, connection_id: &str) -> Vec<String> {
        if self.asset_selection.len() > 1 && self.asset_selection.contains(connection_id) {
            self.selected_asset_ids()
        } else {
            vec![connection_id.to_string()]
        }
    }

    pub fn selected_asset_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.asset_selection.iter().cloned().collect();
        ids.sort();
        ids
    }

    pub fn asset_batch(&self) -> &AssetBatchEditDraft {
        &self.asset_batch
    }

    pub fn asset_batch_mut(&mut self) -> &mut AssetBatchEditDraft {
        &mut self.asset_batch
    }

    pub fn asset_batch_is_open(&self) -> bool {
        self.asset_batch.is_open()
    }

    pub fn start_asset_batch_edit(
        &mut self,
        ids: Vec<String>,
        basis: HashMap<BatchFieldId, FieldBasis>,
        values: HashMap<BatchFieldId, String>,
        bool_values: HashMap<BatchFieldId, bool>,
    ) {
        self.asset_batch.start(ids, basis, values, bool_values);
    }

    pub fn clear_asset_batch_edit(&mut self) {
        self.asset_batch.clear();
    }

    pub fn asset_batch_ids(&self) -> &[String] {
        self.asset_batch.ids()
    }

    pub fn asset_batch_saving(&self) -> bool {
        self.asset_batch.saving()
    }

    pub fn set_asset_batch_saving(&mut self, saving: bool) {
        self.asset_batch.set_saving(saving);
    }

    pub fn set_asset_batch_error(&mut self, error: Option<SharedString>) {
        self.asset_batch.set_error(error);
    }

    pub fn set_asset_batch_value(&mut self, field: BatchFieldId, value: &str) {
        self.asset_batch.set_value(field, value.to_string());
    }

    pub fn set_asset_batch_bool(&mut self, field: BatchFieldId, value: bool) {
        self.asset_batch.set_bool_value(field, value);
    }

    pub fn asset_batch_window_handle(&self) -> Option<NyaWindowHandle> {
        self.asset_batch.window().handle()
    }

    pub fn asset_batch_window_slot(&mut self) -> &mut ChildWindowSlot {
        self.asset_batch.window_mut()
    }

    pub fn asset_batch_window_open_pending(&self) -> bool {
        self.asset_batch.window().is_pending()
    }

    pub fn begin_asset_batch_window_open(&mut self) -> bool {
        self.asset_batch.window_mut().begin_open()
    }

    pub fn cancel_asset_batch_window_open(&mut self) {
        self.asset_batch.window_mut().cancel_open();
    }

    pub fn finish_asset_batch_window_open(&mut self, handle: NyaWindowHandle) {
        self.asset_batch.window_mut().finish_open(handle);
    }

    pub fn clear_asset_batch_window(&mut self) {
        self.asset_batch.window_mut().fail_open();
    }

    /// Drops ids that no longer exist so stale selection can never reach the write path.
    fn prune_asset_selection(&mut self) -> bool {
        let known = self
            .source_records
            .iter()
            .map(|record| record.connection.id.as_str())
            .collect::<HashSet<_>>();
        let before = self.asset_selection.len();
        self.asset_selection
            .retain(|connection_id| known.contains(connection_id.as_str()));
        if self
            .asset_selection_anchor
            .as_deref()
            .is_some_and(|anchor| !known.contains(anchor))
        {
            self.asset_selection_anchor = None;
        }
        self.asset_selection.len() != before
    }

    /// The records currently on screen, in visible order, for range selection.
    pub fn visible_records(&self) -> &[AssetRecord] {
        &self.visible_records
    }

    pub fn search_field(&self) -> Entity<NyaInputState> {
        self.search_field.clone()
    }
    pub fn group_select(&self) -> Entity<NyaSelectState> {
        self.group_select.clone()
    }
    pub fn sync_group_options(&mut self, groups: &[Group], cx: &mut Context<NyaTermApp>) {
        self.group_options_revision = None;
        let root_label = rust_i18n::t!("assets.title").to_string();
        if self
            .selected_group_id
            .as_ref()
            .is_some_and(|selected| !groups.iter().any(|group| &group.id == selected))
        {
            self.selected_group_id = None;
        }
        if self
            .group_options_key
            .as_ref()
            .is_some_and(|(cached, label)| cached == groups && label == &root_label)
        {
            self.sync_selected_group(cx);
            return;
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
        self.group_options_key = Some((groups.to_vec(), root_label));
    }

    pub fn sync_group_options_for_catalog(
        &mut self,
        revision: u64,
        groups: &[Group],
        cx: &mut Context<NyaTermApp>,
    ) {
        let key = (revision, rust_i18n::t!("assets.title").to_string());
        if self.group_options_revision.as_ref() == Some(&key) {
            self.sync_selected_group(cx);
            return;
        }
        self.sync_group_options(groups, cx);
        self.group_options_revision = Some(key);
    }

    fn sync_selected_group(&self, cx: &mut Context<NyaTermApp>) {
        self.group_select.update(cx, |select, cx| {
            select.set_selected_value(Some(self.selected_group_id.clone().unwrap_or_default()), cx);
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
        self.column_widths.iter().sum::<f32>()
            + ASSET_SELECT_COLUMN_WIDTH
            + ASSET_TABLE_ACTIONS_WIDTH
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
        &mut self,
        revisions: (u64, u64),
        connections: &[SavedConnection],
        groups: &[Group],
        labels: &AssetDisplayLabels,
        root_label: &str,
    ) -> Arc<[AssetRecord]> {
        let source_key = (revisions, root_label.to_string());
        if self.record_source_key.as_ref() != Some(&source_key) {
            self.source_records = build_asset_records(connections, groups, root_label);
            self.record_source_key = Some(source_key);
            self.record_view_key = None;
            self.prune_asset_selection();
        }
        let view_key = AssetRecordViewKey {
            search: self.search.trim().to_lowercase(),
            filters: self.filters.clone(),
            group: self.selected_group_id.clone(),
            sort: self.sort,
            labels: labels.clone(),
        };
        if self.record_view_key.as_ref() == Some(&view_key) {
            return self.visible_records.clone();
        }
        let records = self.source_records.clone();
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
        self.visible_records = records.into();
        self.record_view_key = Some(view_key);
        self.visible_records.clone()
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

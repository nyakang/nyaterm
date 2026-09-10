use rust_i18n::t;
use std::borrow::Cow;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Pixels, SharedString, Subscription,
};
use nyaterm_core::{AppSettingsSummary, ConnectionType, Group, SavedConnection};
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};

use super::catalog::ConnectionCatalogState;
use super::connection_runtime::ConnectionEditorToggle;
use super::interaction::{ConnectionDropPosition, ConnectionDropTarget};
use crate::features::NyaTermApp;
use crate::features::pages::connections::list::{
    ConnectionListRow, ConnectionSection, connection_sections, flatten_connection_rows,
    widest_connection_row,
};
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorRdpTab, ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab,
    ConnectionEditorState, ConnectionEditorTelnetTab, ConnectionGroupEditorMode,
    ConnectionGroupEditorState, ConnectionImportSource, ConnectionKindTab,
    ConnectionListContextTarget, ConnectionSortMode, NetworkGroupEditorState,
    NetworkMovePickerState, NetworkProxyEditorField, NetworkProxyEditorState, NetworkTab,
    NetworkTunnelEditorField, NetworkTunnelEditorState,
};
use nyaterm_ui::{
    NyaInputEvent, NyaInputState, NyaNumberInputEvent, NyaNumberInputOptions, NyaNumberInputState,
};

mod editor_logic;
mod list_logic;
mod network_logic;

use self::editor_logic::{
    ConnectionEditorPlaceholder, add_connection_editor_agent_endpoint,
    advance_connection_editor_focus, apply_connection_editor_shell_path,
    apply_connection_editor_working_dir, clear_connection_editor_runtime_state,
    commit_connection_editor_new_group, connection_editor_inline_panel_draft, editor_field_seeds,
    forwarding_endpoint_field_seeds, insert_connection_editor_description_newline,
    move_connection_editor_agent_endpoint, move_connection_editor_ssh_algorithm,
    remove_connection_editor_agent_endpoint, select_connection_editor_agent_endpoint,
    select_saved_connection_after_editor_save, set_connection_editor_advanced_tab,
    set_connection_editor_agent_endpoint_type, set_connection_editor_error,
    set_connection_editor_field_text, set_connection_editor_forwarding_endpoint_field,
    set_connection_editor_icon, set_connection_editor_icon_auto_detect, set_connection_editor_kind,
    set_connection_editor_password_source, set_connection_editor_rdp_tab,
    set_connection_editor_select_value, set_connection_editor_ssh_algorithm_enabled,
    set_connection_editor_ssh_algorithm_tab, set_connection_editor_telnet_tab,
    set_connection_group_editor_error, toggle_connection_editor_agent_allowlist_fingerprint,
    toggle_connection_editor_flag,
};
use self::list_logic::{
    AppliedSearchExpansion, clear_connection_list_runtime_state, clear_selected_connection_ids,
    connection_drop_position_for_target, cycle_connection_sort_mode,
    remove_connection_list_references, remove_group_list_references,
    retain_loaded_connection_references, retain_loaded_group_list_references,
    saved_connections_in_group_tree_for_list_state, select_connection_ids,
    selected_connections_for_list_state, set_connection_drop_target_if_changed,
    set_connection_group_hover, sync_connection_search_expansion,
    visible_connection_ids_for_list_state,
};
use self::network_logic::{
    clear_network_proxy_editor, clear_network_tunnel_editor, remove_network_group_references,
    remove_network_item_references, set_network_group_editor_error, set_network_group_editor_name,
    set_network_proxy_editor_error, set_network_proxy_editor_field, set_network_proxy_group,
    set_network_proxy_protocol, set_network_tunnel_bind_localhost, set_network_tunnel_connection,
    set_network_tunnel_editor_error, set_network_tunnel_editor_field, set_network_tunnel_group,
    set_network_tunnel_type, toggle_network_move_picker_state, toggle_network_tunnel_auto_open,
};

pub(in crate::features) struct ConnectionFeatureState {
    pub(in crate::features) custom_icons: super::custom_icons::CustomIconState,
    catalog: ConnectionCatalogState,
    list: ConnectionListState,
    list_model: ConnectionListModelCache,
    import: ConnectionImportState,
    editor: ConnectionEditorFeatureState,
    group_editor: ConnectionGroupEditorFeatureState,
    network: NetworkFeatureState,
}

#[derive(Clone, Copy, Default, PartialEq)]
pub(in crate::features) struct ConnectionListModelStats {
    pub cache_hit: bool,
    pub connection_count: usize,
    pub group_count: usize,
    pub flat_row_count: usize,
    pub sections_ms: f64,
    pub flatten_ms: f64,
    pub widest_ms: f64,
}

#[derive(Clone)]
pub(in crate::features) struct ConnectionListModelSnapshot {
    /// Shared, because a cache hit hands this straight back to a caller and the
    /// panel then holds it for as long as the snapshot lives. As a `Vec` that was
    /// a deep copy of every row on every read.
    pub rows: Arc<[ConnectionListRow]>,
    pub widest_row: Option<usize>,
    pub stats: ConnectionListModelStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConnectionListSectionKey {
    connections_revision: u64,
    groups_revision: u64,
    search_revision: u64,
    sort_revision: u64,
    query: String,
    sort_mode: ConnectionSortMode,
}

/// The flat-row cache key, also the backbone of the panel snapshot key.
///
/// Complete by construction for what it covers: the catalog keeps its vectors
/// private behind two mutators that both bump, and the four list revisions are
/// bumped by the only methods that can change what they stand for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::features) struct ConnectionListRowsKey {
    section_key: ConnectionListSectionKey,
    expanded_groups_revision: u64,
    group_editor_revision: u64,
}

#[derive(Default)]
struct ConnectionListModelCache {
    section_key: Option<ConnectionListSectionKey>,
    sections: Vec<ConnectionSection>,
    rows_key: Option<ConnectionListRowsKey>,
    snapshot: Option<ConnectionListModelSnapshot>,
    pending_sections_ms: f64,
}

pub(in crate::features) struct ConnectionFeatureFocus {
    /// Placeholder for the filter box, resolved by the caller so this struct
    /// stays free of the i18n lookup.
    pub filter_placeholder: SharedString,
    pub editor: FocusHandle,
}

struct ConnectionListState {
    /// The editable field. It owns the caret, selection and composition; this
    /// struct only caches what it last reported so filtering stays synchronous.
    search_field: Entity<NyaInputState>,
    search_draft: String,
    /// Kept alive for as long as the field is, so edits keep arriving.
    _search_subscription: Subscription,
    sort_mode: ConnectionSortMode,
    /// Row the arrow keys are currently on while filtering. Distinct from the
    /// selection: walking results must not clobber a multi-select.
    keyboard_active_connection_id: Option<String>,
    drop_target: Option<ConnectionDropTarget>,
    hovered_group_id: Option<String>,
    expanded_group_ids: HashSet<String>,
    /// Expansion to restore once the filter box empties again.
    search_expanded_base: Option<HashSet<String>>,
    /// Query and matching-group set the auto-expand has already been applied for.
    search_applied: Option<AppliedSearchExpansion>,
    /// Behind an `Arc` so the panel snapshot's change signal is the pointer.
    ///
    /// Every mutation below goes through `Arc::make_mut`, and the snapshot always
    /// holds a clone, so the refcount at mutation time is never 1 and `make_mut`
    /// is obliged to allocate. `Arc::ptr_eq` is therefore an O(1) signal that no
    /// mutator has to remember to maintain -- the representation carries it. It
    /// errs toward a spurious rebuild (a no-op write still moves the pointer),
    /// never toward a missed one.
    selected_ids: Arc<HashSet<String>>,
    last_selected_id: Option<String>,
    /// What the last right-click landed on, read by the list's single context
    /// menu when it builds its items.
    context_target: ConnectionListContextTarget,
    search_revision: u64,
    sort_revision: u64,
    expanded_groups_revision: u64,
}

struct ConnectionImportState {
    import_path_prompt: Option<ConnectionImportSource>,
}

struct ConnectionEditorFeatureState {
    draft: Option<ConnectionEditorState>,
    /// One editable field per text input, built when the editor opens.
    ///
    /// The draft above stays the source of truth for saving; these own the
    /// caret, selection and composition, and write back through their
    /// subscriptions. Keeping them out of `ConnectionEditorState` keeps that
    /// model a plain value the runtime can clone.
    fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
    number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
    field_subscriptions: Vec<Subscription>,
    number_field_subscriptions: Vec<Subscription>,
    forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
    forwarding_endpoint_field_subscriptions: Vec<Subscription>,
    window: ChildWindowSlot,
    focus: FocusHandle,
    icon_picker_open: bool,
    group_select_open: bool,
    agent_identity_picker_open: bool,
    agent_preview_generation: u64,
    group_select_trigger_bounds: Option<Bounds<Pixels>>,
}

struct ConnectionGroupEditorFeatureState {
    draft: Option<ConnectionGroupEditorState>,
    /// The folder-name input, built with the draft it mirrors.
    field: Option<Entity<NyaInputState>>,
    field_subscription: Option<Subscription>,
    revision: u64,
}

struct NetworkFeatureState {
    tab: NetworkTab,
    group_editor: Option<NetworkGroupEditorState>,
    move_picker: Option<NetworkMovePickerState>,
    expanded_sections: HashSet<String>,
    tunnel_editor: Option<NetworkTunnelEditorState>,
    proxy_editor: Option<NetworkProxyEditorState>,
}

impl ConnectionFeatureState {
    pub fn new(
        connections: Vec<SavedConnection>,
        groups: Vec<Group>,
        settings: &AppSettingsSummary,
        focus: ConnectionFeatureFocus,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let filter_placeholder = focus.filter_placeholder;
        let search_field =
            cx.new(|cx| NyaInputState::new(cx, String::new()).placeholder(filter_placeholder));
        // The field owns the text; the panel only needs to know when it changed.
        let search_subscription = cx.subscribe(
            &search_field,
            |app: &mut NyaTermApp, _, event: &NyaInputEvent, cx| match event {
                NyaInputEvent::Changed(text) | NyaInputEvent::Submitted(text) => {
                    app.connection_state.set_list_search_text(text.clone());
                    app.sync_connection_keyboard_active(cx);
                    app.defer_connection_panel_snapshot_flush(cx);
                    cx.notify();
                }
                NyaInputEvent::Blurred(_) => {}
            },
        );
        Self {
            custom_icons: Default::default(),
            catalog: ConnectionCatalogState::new(connections, groups),
            list: ConnectionListState {
                search_field,
                search_draft: String::new(),
                _search_subscription: search_subscription,
                sort_mode: ConnectionSortMode::from_setting(
                    &settings.ui_saved_connections_sort_mode,
                ),
                keyboard_active_connection_id: None,
                drop_target: None,
                hovered_group_id: None,
                expanded_group_ids: settings
                    .ui_saved_connections_expanded_group_ids
                    .iter()
                    .cloned()
                    .collect(),
                search_expanded_base: None,
                search_applied: None,
                selected_ids: Arc::new(HashSet::new()),
                last_selected_id: None,
                context_target: ConnectionListContextTarget::default(),
                search_revision: 0,
                sort_revision: 0,
                expanded_groups_revision: 0,
            },
            list_model: ConnectionListModelCache::default(),
            import: ConnectionImportState {
                import_path_prompt: None,
            },
            editor: ConnectionEditorFeatureState {
                draft: None,
                fields: HashMap::new(),
                number_fields: HashMap::new(),
                field_subscriptions: Vec::new(),
                number_field_subscriptions: Vec::new(),
                forwarding_endpoint_fields: HashMap::new(),
                forwarding_endpoint_field_subscriptions: Vec::new(),
                window: ChildWindowSlot::default(),
                focus: focus.editor,
                icon_picker_open: false,
                group_select_open: false,
                agent_identity_picker_open: false,
                agent_preview_generation: 0,
                group_select_trigger_bounds: None,
            },
            group_editor: ConnectionGroupEditorFeatureState {
                draft: None,
                field: None,
                field_subscription: None,
                revision: 0,
            },
            network: NetworkFeatureState {
                tab: NetworkTab::Tunnels,
                group_editor: None,
                move_picker: None,
                expanded_sections: HashSet::new(),
                tunnel_editor: None,
                proxy_editor: None,
            },
        }
    }

    pub fn connections(&self) -> &[SavedConnection] {
        self.catalog.connections()
    }

    pub fn groups(&self) -> &[Group] {
        self.catalog.groups()
    }

    /// The current flat-row key, for the panel snapshot.
    ///
    /// Cheap: revisions and the filter text, no model build. Read it *after*
    /// `connection_list_model`, which is what settles the search expansion the
    /// key reads.
    pub(in crate::features) fn list_rows_key(&self) -> ConnectionListRowsKey {
        ConnectionListRowsKey {
            section_key: ConnectionListSectionKey {
                connections_revision: self.catalog.connections_revision(),
                groups_revision: self.catalog.groups_revision(),
                search_revision: self.list.search_revision,
                sort_revision: self.list.sort_revision,
                query: self.list.search_query(),
                sort_mode: self.list.sort_mode(),
            },
            expanded_groups_revision: self.list.expanded_groups_revision,
            group_editor_revision: self.group_editor.revision,
        }
    }

    pub(in crate::features) fn list_selection(&self) -> Arc<HashSet<String>> {
        self.list.selected_ids.clone()
    }

    /// The expanded set as the snapshot holds it.
    ///
    /// A fresh `Arc` each time: this set is small and only read through the
    /// snapshot, and it is already covered by `expanded_groups_revision`, so it
    /// needs no pointer identity of its own.
    pub(in crate::features) fn list_expanded_groups_arc(&self) -> Arc<HashSet<String>> {
        Arc::new(self.list.expanded_group_ids.clone())
    }

    pub(in crate::features) fn list_hovered_group_id(&self) -> Option<String> {
        self.list.hovered_group_id.clone()
    }

    pub(in crate::features) fn list_drop_target(&self) -> Option<ConnectionDropTarget> {
        self.list.drop_target.clone()
    }

    pub fn connection_by_id(&self, connection_id: &str) -> Option<&SavedConnection> {
        self.catalog
            .connections()
            .iter()
            .find(|connection| connection.id == connection_id)
    }

    pub fn serial_ports(&self) -> &[String] {
        self.catalog.serial_ports()
    }

    pub fn replace_loaded(&mut self, connections: Vec<SavedConnection>, groups: Vec<Group>) {
        self.catalog.replace_loaded(connections, groups);
        self.retain_list_references_for_catalog();
    }

    pub fn replace_serial_ports(&mut self, serial_ports: Vec<String>) {
        self.catalog.replace_serial_ports(serial_ports);
    }

    pub fn update_connection(&mut self, updated: SavedConnection) -> bool {
        self.catalog.update_connection(updated)
    }

    pub(in crate::features) fn connection_list_model(&mut self) -> ConnectionListModelSnapshot {
        let query = self.list.search_query();
        let section_key = ConnectionListSectionKey {
            connections_revision: self.catalog.connections_revision(),
            groups_revision: self.catalog.groups_revision(),
            search_revision: self.list.search_revision,
            sort_revision: self.list.sort_revision,
            query: query.clone(),
            sort_mode: self.list.sort_mode(),
        };
        if self.list_model.section_key.as_ref() != Some(&section_key) {
            let started_at = Instant::now();
            self.list_model.sections = connection_sections(
                self.catalog.connections(),
                self.catalog.groups(),
                &query,
                self.list.sort_mode(),
            );
            self.list_model.section_key = Some(section_key.clone());
            self.list_model.snapshot = None;
            self.list_model.rows_key = None;
            self.list_model
                .remember_sections_duration(duration_ms(started_at.elapsed()));
        }

        if self.list.sync_search_expansion(
            &query,
            self.list_model
                .sections
                .iter()
                .filter_map(|section| section.group_id.clone()),
        ) {
            self.list.bump_expanded_groups_revision();
        }

        let rows_key = ConnectionListRowsKey {
            section_key,
            expanded_groups_revision: self.list.expanded_groups_revision,
            group_editor_revision: self.group_editor.revision,
        };
        if self.list_model.rows_key.as_ref() == Some(&rows_key)
            && let Some(snapshot) = self.list_model.snapshot.as_ref()
        {
            let mut snapshot = snapshot.clone();
            snapshot.stats.cache_hit = true;
            snapshot.stats.sections_ms = 0.0;
            snapshot.stats.flatten_ms = 0.0;
            snapshot.stats.widest_ms = 0.0;
            return snapshot;
        }

        let flatten_started_at = Instant::now();
        let rows = flatten_connection_rows(
            &self.list_model.sections,
            self.list.expanded_group_ids(),
            self.group_editor.draft.as_ref(),
        );
        let flatten_ms = duration_ms(flatten_started_at.elapsed());
        let widest_started_at = Instant::now();
        let widest_row = widest_connection_row(&rows, self.catalog.connections());
        let widest_ms = duration_ms(widest_started_at.elapsed());
        let stats = ConnectionListModelStats {
            cache_hit: false,
            connection_count: self.catalog.connections().len(),
            group_count: self.catalog.groups().len(),
            flat_row_count: rows.len(),
            sections_ms: self.list_model.take_sections_duration(),
            flatten_ms,
            widest_ms,
        };
        let snapshot = ConnectionListModelSnapshot {
            rows: rows.into(),
            widest_row,
            stats,
        };
        self.list_model.rows_key = Some(rows_key);
        self.list_model.snapshot = Some(snapshot.clone());
        snapshot
    }

    pub fn connections_reordered_into_group(
        &self,
        source_ids: &[String],
        group_id: &Option<String>,
    ) -> Vec<SavedConnection> {
        self.catalog
            .connections_reordered_into_group(source_ids, group_id)
    }

    pub fn group_is_descendant(&self, candidate_id: &str, ancestor_id: &str) -> bool {
        self.catalog.group_is_descendant(candidate_id, ancestor_id)
    }

    fn retain_list_references_for_catalog(&mut self) {
        let connection_ids = self
            .catalog
            .connections()
            .iter()
            .map(|connection| connection.id.clone())
            .collect::<HashSet<_>>();
        let group_ids = self
            .catalog
            .groups()
            .iter()
            .map(|group| group.id.clone())
            .collect::<HashSet<_>>();
        self.list
            .retain_loaded_references(&connection_ids, &group_ids);
    }

    pub fn list_search_is_empty(&self) -> bool {
        self.list.search_is_empty()
    }

    pub fn list_search_field(&self) -> Entity<NyaInputState> {
        self.list.search_field()
    }

    pub fn set_list_search_text(&mut self, text: String) {
        self.list.set_search_text(text);
    }

    pub fn list_sort_mode(&self) -> ConnectionSortMode {
        self.list.sort_mode()
    }

    pub fn list_has_selection(&self) -> bool {
        self.list.has_selection()
    }

    pub fn list_contains_selected_id(&self, connection_id: &str) -> bool {
        self.list.contains_selected_id(connection_id)
    }

    pub fn selected_connections(&self) -> Vec<SavedConnection> {
        selected_connections_for_list_state(self.catalog.connections(), &self.list.selected_ids)
    }

    pub fn saved_connections_in_group_tree(&self, group_id: &str) -> Vec<SavedConnection> {
        saved_connections_in_group_tree_for_list_state(
            self.catalog.connections(),
            self.catalog.groups(),
            group_id,
        )
    }

    pub fn visible_connection_ids(&self) -> Vec<String> {
        visible_connection_ids_for_list_state(
            self.catalog.connections(),
            self.catalog.groups(),
            &self.list.search_query(),
            self.list.sort_mode,
            &self.list.expanded_group_ids,
        )
    }

    pub fn list_expanded_group_ids(&self) -> &HashSet<String> {
        self.list.expanded_group_ids()
    }

    pub fn select_list_connection(
        &mut self,
        connection_id: String,
        visible_ids: &[String],
        additive: bool,
        range: bool,
    ) -> usize {
        self.list
            .select_connection(connection_id, visible_ids, additive, range)
    }

    pub fn clear_list_selection(&mut self) {
        self.list.clear_selection();
    }

    pub fn cycle_list_sort_mode(&mut self) -> ConnectionSortMode {
        self.list.cycle_sort_mode()
    }

    pub fn set_list_group_hover(&mut self, group_id: String, hovered: bool) -> bool {
        self.list.set_group_hover(group_id, hovered)
    }

    pub fn list_keyboard_active_connection_id(&self) -> Option<&str> {
        self.list.keyboard_active_connection_id()
    }

    pub fn set_list_keyboard_active_connection_id(&mut self, connection_id: Option<String>) {
        self.list.set_keyboard_active_connection_id(connection_id);
    }

    pub fn prepare_list_connection_context_menu(&mut self, connection_id: String) {
        self.list.context_target = ConnectionListContextTarget::Connection(connection_id.clone());
        self.list.select_for_context_menu(connection_id);
    }

    pub fn prepare_list_group_context_menu(&mut self, group_id: String) {
        self.list.context_target = ConnectionListContextTarget::Group(group_id);
    }

    /// Aim the list's context menu at the list itself.
    ///
    /// Rows and group headers re-aim it from their own capture-phase handlers,
    /// which run after this one, so a right-click that misses every row is the
    /// case that keeps this value.
    pub fn prepare_list_context_menu(&mut self) {
        self.list.context_target = ConnectionListContextTarget::List;
    }

    pub fn list_context_target(&self) -> &ConnectionListContextTarget {
        &self.list.context_target
    }

    pub fn toggle_list_group_expanded(&mut self, group_id: String) -> bool {
        self.list.toggle_group_expanded(group_id)
    }

    pub fn expand_list_group(&mut self, group_id: String) {
        self.list.expand_group(group_id);
    }

    pub fn expand_all_catalog_groups(&mut self) {
        let group_ids = self
            .catalog
            .groups()
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        self.list.expand_groups(group_ids);
    }

    pub fn set_list_drop_target_if_changed(&mut self, target: ConnectionDropTarget) -> bool {
        self.list.set_drop_target_if_changed(target)
    }

    pub fn list_drop_position_for_target(
        &self,
        target_id: &str,
        fallback: ConnectionDropPosition,
    ) -> ConnectionDropPosition {
        self.list.drop_position_for_target(target_id, fallback)
    }

    pub fn clear_list_drop_target(&mut self) {
        self.list.clear_drop_target();
    }

    pub fn clear_list_runtime_state(&mut self) {
        self.list.clear_runtime_state();
    }

    pub fn remove_list_connection_references(&mut self, connection_id: &str) {
        self.list.remove_connection_references(connection_id);
    }

    pub fn remove_list_group_references(&mut self, group_id: &str) {
        self.list.remove_group_references(group_id);
    }

    /// The editor's fields, for handing to the render sections.
    pub fn editor_fields(&self) -> &HashMap<ConnectionEditorField, Entity<NyaInputState>> {
        &self.editor.fields
    }

    pub fn editor_number_fields(
        &self,
    ) -> &HashMap<ConnectionEditorField, Entity<NyaNumberInputState>> {
        &self.editor.number_fields
    }

    pub fn editor_forwarding_endpoint_fields(
        &self,
    ) -> &HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>> {
        &self.editor.forwarding_endpoint_fields
    }

    /// Build a field per input and wire each back into the draft.
    ///
    /// Called when the editor opens, so the entities live exactly as long as the
    /// draft they mirror and never leak between edits.
    pub fn build_editor_fields(&mut self, cx: &mut Context<NyaTermApp>) {
        self.editor.fields.clear();
        self.editor.number_fields.clear();
        self.editor.field_subscriptions.clear();
        self.editor.number_field_subscriptions.clear();
        self.editor.forwarding_endpoint_fields.clear();
        self.editor.forwarding_endpoint_field_subscriptions.clear();
        let Some(draft) = self.editor.draft.as_ref() else {
            return;
        };
        for (field, value, masked, placeholder) in editor_field_seeds(draft) {
            let placeholder = match placeholder {
                ConnectionEditorPlaceholder::Empty => Cow::Borrowed(""),
                ConnectionEditorPlaceholder::I18n(key) => t!(key),
                ConnectionEditorPlaceholder::Literal(value) => Cow::Borrowed(value),
            };
            if let Some(options) = connection_editor_number_options(field) {
                let entity = cx.new(|cx| {
                    NyaNumberInputState::new(cx, value, options).placeholder(placeholder)
                });
                let subscription = cx.subscribe(
                    &entity,
                    move |app: &mut NyaTermApp, _, event, cx| match event {
                        NyaNumberInputEvent::Changed(text)
                        | NyaNumberInputEvent::Submitted(text) => {
                            app.apply_connection_editor_field_text(field, text.clone(), cx);
                        }
                        NyaNumberInputEvent::Stepped(_) => {}
                    },
                );
                self.editor.number_fields.insert(field, entity);
                self.editor.number_field_subscriptions.push(subscription);
                continue;
            }
            let entity = cx.new(|cx| {
                let input = NyaInputState::new(cx, value)
                    .masked(masked)
                    .placeholder(placeholder);
                if matches!(
                    field,
                    ConnectionEditorField::Description | ConnectionEditorField::PostLoginCommand
                ) {
                    input.multi_line(Some(4))
                } else {
                    input
                }
            });
            let subscription =
                cx.subscribe(
                    &entity,
                    move |app: &mut NyaTermApp, _, event, cx| match event {
                        NyaInputEvent::Changed(text) | NyaInputEvent::Submitted(text) => {
                            app.apply_connection_editor_field_text(field, text.clone(), cx);
                        }
                        NyaInputEvent::Blurred(_) => {}
                    },
                );
            self.editor.fields.insert(field, entity);
            self.editor.field_subscriptions.push(subscription);
        }
        self.ensure_editor_forwarding_endpoint_fields(cx);
    }

    /// Keep one independent input entity for every endpoint value.
    ///
    /// The endpoint list is editable at runtime, so these entities are created
    /// lazily and refreshed from the draft whenever the editor renders.
    pub fn ensure_editor_forwarding_endpoint_fields(&mut self, cx: &mut Context<NyaTermApp>) {
        let seeds = self
            .editor
            .draft
            .as_ref()
            .map(forwarding_endpoint_field_seeds)
            .unwrap_or_default();
        for (index, field, value, placeholder) in seeds {
            let key = (index, field);
            if let Some(entity) = self.editor.forwarding_endpoint_fields.get(&key) {
                if entity.read(cx).value(cx) != value {
                    entity.update(cx, |entity, cx| entity.set_content(&value, cx));
                }
                continue;
            }
            let seed = value.clone();
            let entity = cx.new(|cx| NyaInputState::new(cx, seed).placeholder(placeholder));
            let subscription =
                cx.subscribe(
                    &entity,
                    move |app: &mut NyaTermApp, _, event, cx| match event {
                        NyaInputEvent::Changed(text) | NyaInputEvent::Submitted(text) => {
                            app.set_connection_editor_agent_endpoint_field(
                                index,
                                field,
                                text.clone(),
                                cx,
                            );
                        }
                        NyaInputEvent::Blurred(_) => {}
                    },
                );
            self.editor.forwarding_endpoint_fields.insert(key, entity);
            self.editor
                .forwarding_endpoint_field_subscriptions
                .push(subscription);
        }
    }

    pub fn rebuild_editor_forwarding_endpoint_fields(&mut self, cx: &mut Context<NyaTermApp>) {
        self.editor.forwarding_endpoint_fields.clear();
        self.editor.forwarding_endpoint_field_subscriptions.clear();
        self.ensure_editor_forwarding_endpoint_fields(cx);
    }

    /// Push a value the runtime changed back down into its field.
    ///
    /// Edits normally flow field → draft; this is the other direction, for the
    /// cases where the runtime consumes what was typed — committing a new folder
    /// empties the box it was typed into.
    pub fn reset_editor_field(&mut self, field: ConnectionEditorField, text: &str, cx: &mut App) {
        if let Some(entity) = self.editor.fields.get(&field) {
            entity.update(cx, |entity, cx| entity.set_content(text, cx));
        }
        if let Some(entity) = self.editor.number_fields.get(&field) {
            entity.update(cx, |entity, cx| entity.set_content(text, cx));
        }
    }

    /// Push every draft value back into its field.
    ///
    /// For the changes the runtime makes on the draft's behalf — switching the
    /// connection kind rewrites the port — where the boxes would otherwise keep
    /// showing what the draft no longer says. `set_content` is a no-op when the
    /// text already matches, so this cannot disturb what is being typed.
    pub fn sync_editor_fields_from_draft(&mut self, cx: &mut App) {
        let Some(draft) = self.editor.draft.as_ref() else {
            return;
        };
        for (field, value, _, _) in editor_field_seeds(draft) {
            if let Some(entity) = self.editor.fields.get(&field) {
                entity.update(cx, |entity, cx| entity.set_content(&value, cx));
            }
            if let Some(entity) = self.editor.number_fields.get(&field) {
                entity.update(cx, |entity, cx| entity.set_content(&value, cx));
            }
        }
    }

    pub fn set_editor_field_text(&mut self, field: ConnectionEditorField, text: String) {
        if let Some(draft) = self.editor.draft.as_mut() {
            set_connection_editor_field_text(draft, field, text);
        }
    }

    pub fn set_editor_agent_endpoint_field(
        &mut self,
        index: usize,
        field: ConnectionEditorField,
        text: String,
    ) -> bool {
        set_connection_editor_forwarding_endpoint_field(&mut self.editor.draft, index, field, text)
    }

    pub fn begin_editor(&mut self, draft: ConnectionEditorState) {
        self.editor.begin_edit(draft);
    }

    pub fn active_editor_draft(&self) -> Option<ConnectionEditorState> {
        self.editor.active_draft()
    }

    pub fn begin_editor_agent_preview(&mut self) -> Option<(u64, ConnectionEditorState)> {
        let editor = self.editor.draft.as_mut()?;
        self.editor.agent_preview_generation = self.editor.agent_preview_generation.wrapping_add(1);
        editor.agent_preview_loading = true;
        Some((self.editor.agent_preview_generation, editor.clone()))
    }

    pub fn confirm_editor_agent_allow_all(&mut self) {
        if let Some(editor) = self.editor.draft.as_mut() {
            editor.agent_allow_all_confirmed = true;
        }
    }

    pub fn set_editor_agent_preview(
        &mut self,
        generation: u64,
        preview: nyaterm_transport::SshAgentIdentityPreviewResponse,
    ) -> bool {
        if generation != self.editor.agent_preview_generation {
            return false;
        }
        if let Some(editor) = self.editor.draft.as_mut() {
            editor.agent_preview = Some(preview);
            editor.agent_preview_loading = false;
            return true;
        }
        false
    }

    pub fn editor_icon_picker_is_open(&self) -> bool {
        self.editor.icon_picker_is_open()
    }

    pub fn editor_group_select_is_open(&self) -> bool {
        self.editor.group_select_is_open()
    }

    pub fn editor_agent_identity_picker_is_open(&self) -> bool {
        self.editor.agent_identity_picker_is_open()
    }

    pub fn editor_group_select_trigger_bounds(&self) -> Option<Bounds<Pixels>> {
        self.editor.group_select_trigger_bounds()
    }

    pub fn editor_description_is_focused(&self) -> bool {
        self.editor.description_is_focused()
    }

    pub fn editor_new_group_field_is_focused(&self, cx: &App) -> bool {
        self.editor.new_group_field_is_focused(cx)
    }

    pub fn inline_editor_panel_draft(&self) -> Option<ConnectionEditorState> {
        self.editor.inline_panel_draft()
    }

    pub fn editor_is_editing_saved_connection(&self) -> bool {
        self.editor.is_editing_saved_connection()
    }

    pub fn editor_has_draft(&self) -> bool {
        self.editor.has_draft()
    }

    pub fn editor_focus_handle(&self) -> FocusHandle {
        self.editor.focus_handle()
    }

    pub(in crate::features) fn editor_window_handle(&self) -> Option<NyaWindowHandle> {
        self.editor.window_handle()
    }

    pub fn editor_has_window(&self) -> bool {
        self.editor.has_window()
    }

    pub fn editor_window_open_pending(&self) -> bool {
        self.editor.window_open_pending()
    }

    pub(in crate::features) fn editor_window_is_open_or_pending(&self) -> bool {
        self.editor.window_is_open_or_pending()
    }

    pub(in crate::features) fn editor_window_slot(&mut self) -> &mut ChildWindowSlot {
        self.editor.window_slot()
    }

    pub fn close_editor_icon_picker(&mut self) {
        self.editor.close_icon_picker();
    }

    pub fn set_editor_icon_picker_open(&mut self, open: bool) -> bool {
        self.editor.set_icon_picker_open(open)
    }

    pub fn set_editor_agent_identity_picker_open(&mut self, open: bool) -> bool {
        let changed = self.editor.set_agent_identity_picker_open(open);
        if changed && !open {
            self.editor.agent_preview_generation =
                self.editor.agent_preview_generation.wrapping_add(1);
            if let Some(editor) = self.editor.draft.as_mut() {
                editor.agent_preview_loading = false;
            }
        }
        changed
    }

    pub fn close_editor_group_select(&mut self) {
        self.editor.close_group_select();
    }

    pub fn set_editor_group_select_trigger_bounds(&mut self, bounds: Bounds<Pixels>) -> bool {
        self.editor.set_group_select_trigger_bounds(bounds)
    }

    pub fn toggle_editor_group_select(&mut self) {
        self.editor.toggle_group_select();
    }

    pub fn set_editor_icon(&mut self, icon: Option<&str>) -> bool {
        self.editor.set_icon(icon)
    }

    pub fn set_editor_icon_auto_detect(&mut self, enabled: bool) -> bool {
        self.editor.set_icon_auto_detect(enabled)
    }

    pub fn set_editor_select_value(
        &mut self,
        select: ConnectionEditorSelect,
        value: Option<String>,
    ) -> bool {
        self.editor.set_select_value(select, value)
    }

    pub fn set_editor_password_source(&mut self, source: ConnectionEditorPasswordSource) -> bool {
        self.editor.set_password_source(source)
    }

    pub fn set_editor_advanced_tab(&mut self, tab: ConnectionEditorAdvancedTab) -> bool {
        self.editor.set_advanced_tab(tab)
    }

    pub fn set_editor_ssh_algorithm_tab(&mut self, tab: ConnectionEditorSshAlgorithmTab) -> bool {
        self.editor.set_ssh_algorithm_tab(tab)
    }

    pub fn set_editor_ssh_algorithm_enabled(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        enabled: bool,
    ) -> bool {
        self.editor.set_ssh_algorithm_enabled(tab, id, enabled)
    }

    pub fn move_editor_ssh_algorithm(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        direction: i8,
    ) -> bool {
        self.editor.move_ssh_algorithm(tab, id, direction)
    }

    pub fn add_editor_agent_endpoint(&mut self) -> bool {
        self.editor.add_editor_agent_endpoint()
    }

    pub fn remove_editor_agent_endpoint(&mut self, index: usize) -> bool {
        self.editor.remove_editor_agent_endpoint(index)
    }

    pub fn select_editor_agent_endpoint(&mut self, index: usize) -> bool {
        self.editor.select_editor_agent_endpoint(index)
    }

    pub fn set_editor_agent_endpoint_type(&mut self, index: usize, value: &str) -> bool {
        self.editor.set_editor_agent_endpoint_type(index, value)
    }

    pub fn move_editor_agent_endpoint(&mut self, index: usize, direction: i8) -> bool {
        self.editor.move_editor_agent_endpoint(index, direction)
    }

    pub fn toggle_editor_agent_allowlist_fingerprint(&mut self, fingerprint: &str) -> bool {
        self.editor
            .toggle_editor_agent_allowlist_fingerprint(fingerprint)
    }

    pub fn set_editor_telnet_tab(&mut self, tab: ConnectionEditorTelnetTab) -> bool {
        self.editor.set_telnet_tab(tab)
    }

    pub fn set_editor_rdp_tab(&mut self, tab: ConnectionEditorRdpTab) -> bool {
        self.editor.set_rdp_tab(tab)
    }

    pub fn set_editor_kind(&mut self, kind: ConnectionKindTab) -> bool {
        self.editor.set_kind(kind)
    }

    pub fn commit_editor_new_group(&mut self, required_message: String) -> bool {
        self.editor.commit_new_group(required_message)
    }

    pub fn toggle_editor_flag(&mut self, flag: ConnectionEditorToggle) -> bool {
        self.editor.toggle_flag(flag)
    }

    pub fn insert_editor_description_newline(&mut self) -> bool {
        self.editor.insert_description_newline()
    }

    pub fn advance_editor_focus(&mut self) -> bool {
        self.editor.advance_focus()
    }

    pub fn set_editor_error(&mut self, error: String) -> bool {
        self.editor.set_error(error)
    }

    pub fn apply_editor_shell_path(&mut self, shell_path: String) -> bool {
        self.editor.apply_shell_path(shell_path)
    }

    pub fn apply_editor_working_dir(&mut self, working_dir: String) -> bool {
        self.editor.apply_working_dir(working_dir)
    }

    pub fn close_editor(&mut self) {
        self.editor.close();
    }

    /// Claim the right to open the editor window; false when one already exists.
    pub fn begin_editor_window_open(&mut self) -> bool {
        self.editor.begin_window_open()
    }

    pub fn clear_editor_window_pending(&mut self) {
        self.editor.clear_window_pending();
    }

    pub(in crate::features::connections) fn attach_editor_window(
        &mut self,
        window: NyaWindowHandle,
    ) {
        self.editor.attach_window(window);
    }

    pub fn clear_editor_window(&mut self) {
        self.editor.clear_window();
    }

    pub fn group_editor_field(&self) -> Option<Entity<NyaInputState>> {
        self.group_editor.field.clone()
    }

    pub fn build_group_editor_field(
        &mut self,
        placeholder: SharedString,
        cx: &mut Context<NyaTermApp>,
    ) {
        let Some(draft) = self.group_editor.draft.as_ref() else {
            self.clear_group_editor_field();
            return;
        };
        let entity =
            cx.new(|cx| NyaInputState::new(cx, draft.name.clone()).placeholder(placeholder));
        let subscription =
            cx.subscribe(&entity, |app: &mut NyaTermApp, _, event, cx| match event {
                NyaInputEvent::Changed(text) => {
                    app.connection_state.set_group_editor_name(text.clone());
                    app.defer_connection_panel_snapshot_flush(cx);
                    cx.notify();
                }
                NyaInputEvent::Submitted(text) => {
                    app.connection_state.set_group_editor_name(text.clone());
                    app.save_connection_group_editor(cx);
                    app.defer_connection_panel_snapshot_flush(cx);
                }
                NyaInputEvent::Blurred(text) => {
                    app.connection_state.set_group_editor_name(text.clone());
                    app.finish_connection_group_editor_from_blur(cx);
                    app.defer_connection_panel_snapshot_flush(cx);
                }
            });
        self.group_editor.field = Some(entity);
        self.group_editor.field_subscription = Some(subscription);
    }

    pub fn clear_group_editor_field(&mut self) {
        self.group_editor.field = None;
        self.group_editor.field_subscription = None;
    }

    pub fn set_group_editor_name(&mut self, name: String) {
        if let Some(draft) = self.group_editor.draft.as_mut() {
            draft.name = name;
            draft.error = None;
            self.group_editor.bump_revision();
        }
    }

    pub fn active_group_editor_draft(&self) -> Option<ConnectionGroupEditorState> {
        self.group_editor.active_draft()
    }

    pub fn begin_create_group_editor(&mut self, parent_id: Option<String>) {
        if let Some(parent_id) = parent_id.as_ref() {
            self.expand_list_group(parent_id.clone());
        }
        self.group_editor.begin_edit(ConnectionGroupEditorState {
            mode: ConnectionGroupEditorMode::Create,
            id: None,
            name: String::new(),
            parent_id,
            error: None,
        });
        self.group_editor.bump_revision();
    }

    pub fn begin_rename_group_editor(
        &mut self,
        id: String,
        name: String,
        parent_id: Option<String>,
    ) {
        self.group_editor.begin_edit(ConnectionGroupEditorState {
            mode: ConnectionGroupEditorMode::Rename,
            id: Some(id),
            name,
            parent_id,
            error: None,
        });
        self.group_editor.bump_revision();
    }

    pub fn set_group_editor_error(&mut self, error: String) -> bool {
        let changed = self.group_editor.set_error(error);
        if changed {
            self.group_editor.bump_revision();
        }
        changed
    }

    pub fn close_group_editor(&mut self) {
        self.group_editor.close();
        self.group_editor.bump_revision();
    }

    pub fn network_active_tab(&self) -> NetworkTab {
        self.network.active_tab()
    }

    pub fn network_tab_is(&self, tab: NetworkTab) -> bool {
        self.network.tab_is(tab)
    }

    pub fn network_section_is_expanded(&self, section_key: &str) -> bool {
        self.network.section_is_expanded(section_key)
    }

    pub fn network_move_picker_is_open(&self, tab: NetworkTab, id: &str) -> bool {
        self.network.move_picker_is_open(tab, id)
    }

    pub fn active_network_group_editor(&self) -> Option<NetworkGroupEditorState> {
        self.network.active_group_editor()
    }

    pub fn active_network_tunnel_editor(&self) -> Option<NetworkTunnelEditorState> {
        self.network.active_tunnel_editor()
    }

    pub fn active_network_proxy_editor(&self) -> Option<NetworkProxyEditorState> {
        self.network.active_proxy_editor()
    }

    pub fn set_network_tab(&mut self, tab: NetworkTab) {
        self.network.set_tab(tab);
    }

    pub fn toggle_network_section(&mut self, section_key: String) -> bool {
        self.network.toggle_section(section_key)
    }

    pub fn toggle_network_move_picker(&mut self, tab: NetworkTab, id: String) -> bool {
        self.network.toggle_move_picker(tab, id)
    }

    pub fn close_network_move_picker(&mut self) {
        self.network.close_move_picker();
    }

    pub fn begin_network_group_edit(&mut self, draft: NetworkGroupEditorState) {
        self.network.begin_group_edit(draft);
    }

    pub fn set_network_group_editor_name(&mut self, text: String) -> bool {
        self.network.set_group_editor_name(text)
    }

    pub fn set_network_group_editor_error(&mut self, error: String) -> bool {
        self.network.set_group_editor_error(error)
    }

    pub fn close_network_group_editor(&mut self) {
        self.network.close_group_editor();
    }

    pub fn begin_network_tunnel_edit(&mut self, draft: NetworkTunnelEditorState) {
        self.network.begin_tunnel_edit(draft);
    }

    pub fn close_network_tunnel_editor(&mut self) {
        self.network.close_tunnel_editor();
    }

    pub fn set_network_tunnel_editor_field(
        &mut self,
        field: NetworkTunnelEditorField,
        text: String,
    ) -> bool {
        self.network.set_tunnel_editor_field(field, text)
    }

    pub fn set_network_tunnel_type(&mut self, tunnel_type: &str) -> Option<String> {
        self.network.set_tunnel_type(tunnel_type)
    }

    pub fn set_network_tunnel_connection(&mut self, connection_id: Option<String>) -> bool {
        let connection_id = connection_id.filter(|id| {
            self.catalog.connections().iter().any(|connection| {
                connection.id == *id && matches!(&connection.config, ConnectionType::Ssh { .. })
            })
        });
        self.network.set_tunnel_connection(connection_id)
    }

    pub fn set_network_tunnel_group(&mut self, group_id: Option<String>) -> bool {
        self.network.set_tunnel_group(group_id)
    }

    pub fn set_network_tunnel_bind_localhost(&mut self, bind_localhost: bool) -> bool {
        self.network.set_tunnel_bind_localhost(bind_localhost)
    }

    pub fn toggle_network_tunnel_auto_open(&mut self) -> Option<bool> {
        self.network.toggle_tunnel_auto_open()
    }

    pub fn set_network_tunnel_editor_error(&mut self, error: String) -> bool {
        self.network.set_tunnel_editor_error(error)
    }

    pub fn begin_network_proxy_edit(&mut self, draft: NetworkProxyEditorState) {
        self.network.begin_proxy_edit(draft);
    }

    pub fn close_network_proxy_editor(&mut self) {
        self.network.close_proxy_editor();
    }

    pub fn set_network_proxy_editor_field(
        &mut self,
        field: NetworkProxyEditorField,
        text: String,
    ) -> bool {
        self.network.set_proxy_editor_field(field, text)
    }

    pub fn set_network_proxy_protocol(&mut self, protocol: &str) -> Option<String> {
        self.network.set_proxy_protocol(protocol)
    }

    pub fn set_network_proxy_group(&mut self, group_id: Option<String>) -> bool {
        self.network.set_proxy_group(group_id)
    }

    pub fn set_network_proxy_editor_error(&mut self, error: String) -> bool {
        self.network.set_proxy_editor_error(error)
    }

    pub fn remove_network_item_references(&mut self, tab: NetworkTab, id: &str) {
        self.network.remove_item_references(tab, id);
    }

    pub fn remove_network_group_references(
        &mut self,
        tab: NetworkTab,
        group_id: &str,
        deleted_item_ids: &[String],
    ) {
        self.network
            .remove_group_references(tab, group_id, deleted_item_ids);
    }

    pub fn clear_editor_fields(&mut self) {
        self.editor.fields.clear();
        self.editor.field_subscriptions.clear();
        self.editor.forwarding_endpoint_fields.clear();
        self.editor.forwarding_endpoint_field_subscriptions.clear();
    }

    pub fn finish_editor_save(&mut self, connection_id: String, group_id: Option<String>) {
        self.editor.agent_identity_picker_open = false;
        clear_connection_editor_runtime_state(
            &mut self.editor.draft,
            &mut self.editor.icon_picker_open,
            &mut self.editor.group_select_open,
            &mut self.editor.window,
        );
        self.editor.group_select_trigger_bounds = None;
        let expanded_before = self.list.expanded_group_ids.clone();
        select_saved_connection_after_editor_save(
            Arc::make_mut(&mut self.list.selected_ids),
            &mut self.list.last_selected_id,
            &mut self.list.expanded_group_ids,
            connection_id,
            group_id,
        );
        if self.list.expanded_group_ids != expanded_before {
            self.list.bump_expanded_groups_revision();
        }
    }

    pub fn import_path_prompt_active(&self) -> bool {
        self.import.path_prompt_active()
    }

    pub fn begin_import_path_prompt(&mut self, source: ConnectionImportSource) {
        self.import.begin_path_prompt(source);
    }

    pub fn finish_import_path_prompt(&mut self) {
        self.import.finish_path_prompt();
    }
}

impl ConnectionListState {
    pub fn search_query(&self) -> String {
        self.search_draft.trim().to_ascii_lowercase()
    }

    pub fn search_is_empty(&self) -> bool {
        self.search_draft.is_empty()
    }

    pub fn search_field(&self) -> Entity<NyaInputState> {
        self.search_field.clone()
    }

    /// Cache what the field just reported. Filtering runs on every keystroke and
    /// from paths without an `App`, so it reads this rather than the entity.
    pub fn set_search_text(&mut self, text: String) {
        if self.search_draft != text {
            self.search_draft = text;
            self.search_revision = self.search_revision.wrapping_add(1);
        }
    }

    pub fn sort_mode(&self) -> ConnectionSortMode {
        self.sort_mode
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_ids.is_empty()
    }

    pub fn contains_selected_id(&self, connection_id: &str) -> bool {
        self.selected_ids.contains(connection_id)
    }

    pub fn expanded_group_ids(&self) -> &HashSet<String> {
        &self.expanded_group_ids
    }

    pub fn select_connection(
        &mut self,
        connection_id: String,
        visible_ids: &[String],
        additive: bool,
        range: bool,
    ) -> usize {
        select_connection_ids(
            Arc::make_mut(&mut self.selected_ids),
            &mut self.last_selected_id,
            connection_id,
            visible_ids,
            additive,
            range,
        )
    }

    pub fn select_only(&mut self, connection_id: String) {
        let selected = Arc::make_mut(&mut self.selected_ids);
        selected.clear();
        selected.insert(connection_id.clone());
        self.last_selected_id = Some(connection_id);
    }

    pub fn clear_selection(&mut self) {
        clear_selected_connection_ids(
            Arc::make_mut(&mut self.selected_ids),
            &mut self.last_selected_id,
        );
    }

    pub fn cycle_sort_mode(&mut self) -> ConnectionSortMode {
        let sort_mode = cycle_connection_sort_mode(&mut self.sort_mode);
        self.sort_revision = self.sort_revision.wrapping_add(1);
        sort_mode
    }

    pub fn set_group_hover(&mut self, group_id: String, hovered: bool) -> bool {
        set_connection_group_hover(&mut self.hovered_group_id, group_id, hovered)
    }

    pub fn keyboard_active_connection_id(&self) -> Option<&str> {
        self.keyboard_active_connection_id.as_deref()
    }

    pub fn set_keyboard_active_connection_id(&mut self, connection_id: Option<String>) {
        self.keyboard_active_connection_id = connection_id;
    }

    pub fn select_for_context_menu(&mut self, connection_id: String) {
        if !self.selected_ids.contains(&connection_id) {
            self.select_only(connection_id);
        }
    }

    pub fn toggle_group_expanded(&mut self, group_id: String) -> bool {
        if self.expanded_group_ids.remove(&group_id) {
            self.bump_expanded_groups_revision();
            return false;
        }
        self.expanded_group_ids.insert(group_id);
        self.bump_expanded_groups_revision();
        true
    }

    pub fn expand_group(&mut self, group_id: String) {
        if self.expanded_group_ids.insert(group_id) {
            self.bump_expanded_groups_revision();
        }
    }

    pub fn expand_groups(&mut self, group_ids: impl IntoIterator<Item = String>) {
        let before = self.expanded_group_ids.len();
        self.expanded_group_ids.extend(group_ids);
        if self.expanded_group_ids.len() != before {
            self.bump_expanded_groups_revision();
        }
    }

    pub fn sync_search_expansion(
        &mut self,
        query: &str,
        matching_group_ids: impl IntoIterator<Item = String>,
    ) -> bool {
        sync_connection_search_expansion(
            &mut self.expanded_group_ids,
            &mut self.search_expanded_base,
            &mut self.search_applied,
            query,
            matching_group_ids,
        )
    }

    pub fn set_drop_target_if_changed(&mut self, target: ConnectionDropTarget) -> bool {
        set_connection_drop_target_if_changed(&mut self.drop_target, target)
    }

    pub fn drop_position_for_target(
        &self,
        target_id: &str,
        fallback: ConnectionDropPosition,
    ) -> ConnectionDropPosition {
        connection_drop_position_for_target(&self.drop_target, target_id, fallback)
    }

    pub fn clear_drop_target(&mut self) {
        self.drop_target = None;
    }

    pub fn clear_runtime_state(&mut self) {
        self.search_expanded_base = None;
        self.search_applied = None;
        self.keyboard_active_connection_id = None;
        let expanded_before = self.expanded_group_ids.clone();
        clear_connection_list_runtime_state(
            Arc::make_mut(&mut self.selected_ids),
            &mut self.last_selected_id,
            &mut self.expanded_group_ids,
            &mut self.drop_target,
            &mut self.hovered_group_id,
        );
        if self.expanded_group_ids != expanded_before {
            self.bump_expanded_groups_revision();
        }
    }

    pub fn remove_connection_references(&mut self, connection_id: &str) {
        remove_connection_list_references(
            Arc::make_mut(&mut self.selected_ids),
            &mut self.last_selected_id,
            &mut self.drop_target,
            connection_id,
        );
    }

    pub fn remove_group_references(&mut self, group_id: &str) {
        let expanded_before = self.expanded_group_ids.clone();
        remove_group_list_references(
            &mut self.expanded_group_ids,
            &mut self.hovered_group_id,
            &mut self.drop_target,
            group_id,
        );
        if self.expanded_group_ids != expanded_before {
            self.bump_expanded_groups_revision();
        }
    }

    pub fn retain_loaded_references(
        &mut self,
        connection_ids: &HashSet<String>,
        group_ids: &HashSet<String>,
    ) {
        if let Some(base) = self.search_expanded_base.as_mut() {
            base.retain(|id| group_ids.contains(id));
        }
        retain_loaded_connection_references(
            Arc::make_mut(&mut self.selected_ids),
            &mut self.last_selected_id,
            &mut self.drop_target,
            connection_ids,
        );
        let expanded_before = self.expanded_group_ids.clone();
        retain_loaded_group_list_references(
            &mut self.expanded_group_ids,
            &mut self.hovered_group_id,
            &mut self.drop_target,
            group_ids,
        );
        if self.expanded_group_ids != expanded_before {
            self.bump_expanded_groups_revision();
        }
    }

    fn bump_expanded_groups_revision(&mut self) {
        self.expanded_groups_revision = self.expanded_groups_revision.wrapping_add(1);
    }
}

impl ConnectionListModelCache {
    fn remember_sections_duration(&mut self, duration_ms: f64) {
        self.pending_sections_ms = duration_ms;
    }

    fn take_sections_duration(&mut self) -> f64 {
        std::mem::take(&mut self.pending_sections_ms)
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl ConnectionImportState {
    pub fn path_prompt_active(&self) -> bool {
        self.import_path_prompt.is_some()
    }

    pub fn begin_path_prompt(&mut self, source: ConnectionImportSource) {
        self.import_path_prompt = Some(source);
    }

    pub fn finish_path_prompt(&mut self) {
        self.import_path_prompt = None;
    }
}

impl ConnectionEditorFeatureState {
    pub fn begin_edit(&mut self, draft: ConnectionEditorState) {
        self.icon_picker_open = false;
        self.group_select_open = false;
        self.agent_identity_picker_open = false;
        self.agent_preview_generation = self.agent_preview_generation.wrapping_add(1);
        self.group_select_trigger_bounds = None;
        self.draft = Some(draft);
    }

    pub fn active_draft(&self) -> Option<ConnectionEditorState> {
        self.draft.clone()
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn icon_picker_is_open(&self) -> bool {
        self.icon_picker_open
    }

    pub fn group_select_is_open(&self) -> bool {
        self.group_select_open
    }

    pub fn agent_identity_picker_is_open(&self) -> bool {
        self.agent_identity_picker_open
    }

    pub fn group_select_trigger_bounds(&self) -> Option<Bounds<Pixels>> {
        self.group_select_trigger_bounds
    }

    pub fn description_is_focused(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|editor| editor.focused_field == ConnectionEditorField::Description)
    }

    pub fn new_group_field_is_focused(&self, cx: &App) -> bool {
        self.fields
            .get(&ConnectionEditorField::NewGroupName)
            .is_some_and(|field| field.read(cx).has_focus())
    }

    pub fn inline_panel_draft(&self) -> Option<ConnectionEditorState> {
        connection_editor_inline_panel_draft(
            &self.draft,
            self.has_window(),
            self.window_open_pending(),
        )
    }

    pub fn is_editing_saved_connection(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|editor| editor.id.is_some())
    }

    pub fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub fn window_handle(&self) -> Option<NyaWindowHandle> {
        self.window.handle()
    }

    pub fn has_window(&self) -> bool {
        self.window.is_open()
    }

    pub fn window_open_pending(&self) -> bool {
        self.window.is_pending()
    }

    pub fn window_slot(&mut self) -> &mut ChildWindowSlot {
        &mut self.window
    }

    pub fn window_is_open_or_pending(&self) -> bool {
        self.window.is_open_or_pending()
    }

    pub fn close_icon_picker(&mut self) {
        self.icon_picker_open = false;
    }

    pub fn set_icon_picker_open(&mut self, open: bool) -> bool {
        if self.icon_picker_open == open {
            return false;
        }
        self.close_icon_picker();
        if open {
            self.close_group_select();
        }
        self.icon_picker_open = open;
        true
    }

    pub fn set_agent_identity_picker_open(&mut self, open: bool) -> bool {
        if self.agent_identity_picker_open == open {
            return false;
        }
        self.agent_identity_picker_open = open;
        true
    }

    pub fn close_group_select(&mut self) {
        self.group_select_open = false;
    }

    pub fn set_group_select_trigger_bounds(&mut self, bounds: Bounds<Pixels>) -> bool {
        if self.group_select_trigger_bounds == Some(bounds) {
            return false;
        }
        self.group_select_trigger_bounds = Some(bounds);
        true
    }

    pub fn toggle_group_select(&mut self) {
        let opening = !self.group_select_open;
        self.close_icon_picker();
        self.close_group_select();
        self.group_select_open = opening;
    }

    pub fn set_icon(&mut self, icon: Option<&str>) -> bool {
        self.close_icon_picker();
        set_connection_editor_icon(&mut self.draft, icon)
    }

    pub fn set_icon_auto_detect(&mut self, enabled: bool) -> bool {
        set_connection_editor_icon_auto_detect(&mut self.draft, enabled)
    }

    pub fn set_select_value(
        &mut self,
        select: ConnectionEditorSelect,
        value: Option<String>,
    ) -> bool {
        let changed = set_connection_editor_select_value(&mut self.draft, select, value);
        if changed {
            self.close_icon_picker();
            if select == ConnectionEditorSelect::Group {
                self.close_group_select();
            }
        }
        changed
    }

    pub fn set_password_source(&mut self, source: ConnectionEditorPasswordSource) -> bool {
        let changed = set_connection_editor_password_source(&mut self.draft, source);
        if changed {
            self.close_icon_picker();
        }
        changed
    }

    pub fn set_advanced_tab(&mut self, tab: ConnectionEditorAdvancedTab) -> bool {
        let changed = set_connection_editor_advanced_tab(&mut self.draft, tab);
        if changed {
            self.close_icon_picker();
        }
        changed
    }

    pub fn set_ssh_algorithm_tab(&mut self, tab: ConnectionEditorSshAlgorithmTab) -> bool {
        set_connection_editor_ssh_algorithm_tab(&mut self.draft, tab)
    }

    pub fn set_ssh_algorithm_enabled(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        enabled: bool,
    ) -> bool {
        set_connection_editor_ssh_algorithm_enabled(&mut self.draft, tab, id, enabled)
    }

    pub fn move_ssh_algorithm(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        direction: i8,
    ) -> bool {
        move_connection_editor_ssh_algorithm(&mut self.draft, tab, id, direction)
    }

    pub fn add_editor_agent_endpoint(&mut self) -> bool {
        add_connection_editor_agent_endpoint(&mut self.draft)
    }

    pub fn remove_editor_agent_endpoint(&mut self, index: usize) -> bool {
        remove_connection_editor_agent_endpoint(&mut self.draft, index)
    }

    pub fn select_editor_agent_endpoint(&mut self, index: usize) -> bool {
        select_connection_editor_agent_endpoint(&mut self.draft, index)
    }

    pub fn set_editor_agent_endpoint_type(&mut self, index: usize, value: &str) -> bool {
        set_connection_editor_agent_endpoint_type(&mut self.draft, index, value)
    }

    pub fn move_editor_agent_endpoint(&mut self, index: usize, direction: i8) -> bool {
        move_connection_editor_agent_endpoint(&mut self.draft, index, direction)
    }

    pub fn toggle_editor_agent_allowlist_fingerprint(&mut self, fingerprint: &str) -> bool {
        toggle_connection_editor_agent_allowlist_fingerprint(&mut self.draft, fingerprint)
    }

    pub fn set_telnet_tab(&mut self, tab: ConnectionEditorTelnetTab) -> bool {
        let changed = set_connection_editor_telnet_tab(&mut self.draft, tab);
        if changed {
            self.close_icon_picker();
        }
        changed
    }

    pub fn set_rdp_tab(&mut self, tab: ConnectionEditorRdpTab) -> bool {
        let changed = set_connection_editor_rdp_tab(&mut self.draft, tab);
        if changed {
            self.close_icon_picker();
        }
        changed
    }

    pub fn set_kind(&mut self, kind: ConnectionKindTab) -> bool {
        self.close_icon_picker();
        self.close_group_select();
        set_connection_editor_kind(&mut self.draft, kind)
    }

    pub fn commit_new_group(&mut self, required_message: String) -> bool {
        let changed = commit_connection_editor_new_group(&mut self.draft, required_message);
        if changed {
            self.close_icon_picker();
            self.close_group_select();
        }
        changed
    }

    pub fn toggle_flag(&mut self, flag: ConnectionEditorToggle) -> bool {
        let changed = toggle_connection_editor_flag(&mut self.draft, flag);
        if changed && flag == ConnectionEditorToggle::Advanced {
            self.close_icon_picker();
        }
        changed
    }

    pub fn insert_description_newline(&mut self) -> bool {
        insert_connection_editor_description_newline(&mut self.draft)
    }

    pub fn advance_focus(&mut self) -> bool {
        advance_connection_editor_focus(&mut self.draft)
    }

    pub fn set_error(&mut self, error: String) -> bool {
        set_connection_editor_error(&mut self.draft, error)
    }

    pub fn apply_shell_path(&mut self, shell_path: String) -> bool {
        apply_connection_editor_shell_path(&mut self.draft, shell_path)
    }

    pub fn apply_working_dir(&mut self, working_dir: String) -> bool {
        apply_connection_editor_working_dir(&mut self.draft, working_dir)
    }

    pub fn close(&mut self) {
        self.agent_identity_picker_open = false;
        self.agent_preview_generation = self.agent_preview_generation.wrapping_add(1);
        clear_connection_editor_runtime_state(
            &mut self.draft,
            &mut self.icon_picker_open,
            &mut self.group_select_open,
            &mut self.window,
        );
        self.group_select_trigger_bounds = None;
    }

    /// Claim the right to open the editor window; false when one already exists.
    pub fn begin_window_open(&mut self) -> bool {
        self.window.begin_open()
    }

    pub fn clear_window_pending(&mut self) {
        self.window.cancel_open();
    }

    pub fn attach_window(&mut self, window: NyaWindowHandle) {
        self.window.finish_open(window);
    }

    pub fn clear_window(&mut self) {
        self.window.clear();
    }
}

impl ConnectionGroupEditorFeatureState {
    pub fn active_draft(&self) -> Option<ConnectionGroupEditorState> {
        self.draft.clone()
    }

    pub fn begin_edit(&mut self, draft: ConnectionGroupEditorState) {
        self.draft = Some(draft);
    }

    pub fn set_error(&mut self, error: String) -> bool {
        set_connection_group_editor_error(&mut self.draft, error)
    }

    pub fn close(&mut self) {
        self.draft = None;
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn connection_editor_number_options(field: ConnectionEditorField) -> Option<NyaNumberInputOptions> {
    match field {
        ConnectionEditorField::Port => Some(
            NyaNumberInputOptions::default()
                .range(1.0, 65_535.0)
                .step(1.0),
        ),
        ConnectionEditorField::PostLoginDelay => Some(
            NyaNumberInputOptions::default()
                .range(0.0, 60_000.0)
                .step(1.0),
        ),
        ConnectionEditorField::TelnetAutoLoginTimeout => Some(
            NyaNumberInputOptions::default()
                .range(100.0, 600_000.0)
                .step(100.0),
        ),
        ConnectionEditorField::TelnetAutoLoginMaxRetries => {
            Some(NyaNumberInputOptions::default().range(0.0, 10.0).step(1.0))
        }
        ConnectionEditorField::RdpDisplayWidth => Some(
            NyaNumberInputOptions::default()
                .range(640.0, 7680.0)
                .step(1.0),
        ),
        ConnectionEditorField::RdpDisplayHeight => Some(
            NyaNumberInputOptions::default()
                .range(480.0, 4320.0)
                .step(1.0),
        ),
        ConnectionEditorField::RdpReconnectAttempts => {
            Some(NyaNumberInputOptions::default().range(0.0, 20.0).step(1.0))
        }
        ConnectionEditorField::BaudRate => Some(
            NyaNumberInputOptions::default()
                .range(50.0, 4_000_000.0)
                .step(1.0),
        ),
        _ => None,
    }
}

impl NetworkFeatureState {
    pub fn active_tab(&self) -> NetworkTab {
        self.tab
    }

    pub fn tab_is(&self, tab: NetworkTab) -> bool {
        self.tab == tab
    }

    pub fn section_is_expanded(&self, section_key: &str) -> bool {
        self.expanded_sections.contains(section_key)
    }

    pub fn move_picker_is_open(&self, tab: NetworkTab, id: &str) -> bool {
        self.move_picker
            .as_ref()
            .is_some_and(|picker| picker.tab == tab && picker.id == id)
    }

    pub fn active_group_editor(&self) -> Option<NetworkGroupEditorState> {
        self.group_editor.clone()
    }

    pub fn active_tunnel_editor(&self) -> Option<NetworkTunnelEditorState> {
        self.tunnel_editor.clone()
    }

    pub fn active_proxy_editor(&self) -> Option<NetworkProxyEditorState> {
        self.proxy_editor.clone()
    }

    pub fn set_tab(&mut self, tab: NetworkTab) {
        self.tab = tab;
        self.move_picker = None;
    }

    pub fn toggle_section(&mut self, section_key: String) -> bool {
        if self.expanded_sections.remove(&section_key) {
            self.move_picker = None;
            return false;
        }
        self.expanded_sections.insert(section_key);
        true
    }

    pub fn toggle_move_picker(&mut self, tab: NetworkTab, id: String) -> bool {
        toggle_network_move_picker_state(&mut self.move_picker, tab, id)
    }

    pub fn close_move_picker(&mut self) {
        self.move_picker = None;
    }

    pub fn begin_group_edit(&mut self, draft: NetworkGroupEditorState) {
        self.group_editor = Some(draft);
    }

    /// Write the group draft's name, clearing any stale validation.
    pub fn set_group_editor_name(&mut self, text: String) -> bool {
        set_network_group_editor_name(&mut self.group_editor, text)
    }

    pub fn set_group_editor_error(&mut self, error: String) -> bool {
        set_network_group_editor_error(&mut self.group_editor, error)
    }

    pub fn close_group_editor(&mut self) {
        self.group_editor = None;
    }

    pub fn begin_tunnel_edit(&mut self, draft: NetworkTunnelEditorState) {
        self.tab = NetworkTab::Tunnels;
        self.tunnel_editor = Some(draft);
    }

    pub fn close_tunnel_editor(&mut self) {
        clear_network_tunnel_editor(&mut self.tunnel_editor);
    }

    /// Write one field of the tunnel draft, clearing any stale validation.
    pub fn set_tunnel_editor_field(
        &mut self,
        field: NetworkTunnelEditorField,
        text: String,
    ) -> bool {
        set_network_tunnel_editor_field(&mut self.tunnel_editor, field, text)
    }

    pub fn set_tunnel_type(&mut self, tunnel_type: &str) -> Option<String> {
        set_network_tunnel_type(&mut self.tunnel_editor, tunnel_type)
    }

    pub fn set_tunnel_connection(&mut self, connection_id: Option<String>) -> bool {
        set_network_tunnel_connection(&mut self.tunnel_editor, connection_id)
    }

    pub fn set_tunnel_group(&mut self, group_id: Option<String>) -> bool {
        set_network_tunnel_group(&mut self.tunnel_editor, group_id)
    }

    pub fn set_tunnel_bind_localhost(&mut self, bind_localhost: bool) -> bool {
        set_network_tunnel_bind_localhost(&mut self.tunnel_editor, bind_localhost)
    }

    pub fn toggle_tunnel_auto_open(&mut self) -> Option<bool> {
        toggle_network_tunnel_auto_open(&mut self.tunnel_editor)
    }

    pub fn set_tunnel_editor_error(&mut self, error: String) -> bool {
        set_network_tunnel_editor_error(&mut self.tunnel_editor, error)
    }

    pub fn begin_proxy_edit(&mut self, draft: NetworkProxyEditorState) {
        self.tab = NetworkTab::Proxies;
        self.proxy_editor = Some(draft);
    }

    pub fn close_proxy_editor(&mut self) {
        clear_network_proxy_editor(&mut self.proxy_editor);
    }

    /// Write one field of the proxy draft, clearing any stale validation.
    pub fn set_proxy_editor_field(&mut self, field: NetworkProxyEditorField, text: String) -> bool {
        set_network_proxy_editor_field(&mut self.proxy_editor, field, text)
    }

    pub fn set_proxy_protocol(&mut self, protocol: &str) -> Option<String> {
        set_network_proxy_protocol(&mut self.proxy_editor, protocol)
    }

    pub fn set_proxy_group(&mut self, group_id: Option<String>) -> bool {
        set_network_proxy_group(&mut self.proxy_editor, group_id)
    }

    pub fn set_proxy_editor_error(&mut self, error: String) -> bool {
        set_network_proxy_editor_error(&mut self.proxy_editor, error)
    }

    pub fn remove_item_references(&mut self, tab: NetworkTab, id: &str) {
        remove_network_item_references(
            &mut self.move_picker,
            &mut self.tunnel_editor,
            &mut self.proxy_editor,
            tab,
            id,
        );
    }

    pub fn remove_group_references(
        &mut self,
        tab: NetworkTab,
        group_id: &str,
        deleted_item_ids: &[String],
    ) {
        remove_network_group_references(
            &mut self.group_editor,
            &mut self.expanded_sections,
            tab,
            group_id,
        );
        for item_id in deleted_item_ids {
            self.remove_item_references(tab, item_id);
        }
    }
}

#[cfg(test)]
mod tests;

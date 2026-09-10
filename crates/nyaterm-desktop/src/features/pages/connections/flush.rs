use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::Context;

use crate::features::NyaTermApp;
use crate::features::perf::record_gpui_perf_sample;

use super::panel::{ConnectionChrome, ConnectionListKey, ConnectionListSnapshot};

impl NyaTermApp {
    /// Queue a snapshot rebuild after the current GPUI entity leases are released.
    ///
    /// Input subscriptions and panel listeners can both mutate connection state while
    /// another entity is leased. Deferring the rebuild keeps those boundaries safe and
    /// still publishes the snapshot before the next paint.
    pub(in crate::features) fn defer_connection_panel_snapshot_flush(
        &self,
        cx: &mut Context<Self>,
    ) {
        self.defer_app_update(cx, |app, cx| {
            app.flush_connection_panel_snapshot(cx);
        });
    }

    fn connection_chrome(&self) -> ConnectionChrome {
        let palette = self.theme_palette();
        ConnectionChrome {
            transparent_surface: self.shell_transparent_color(palette.surface),
            transparent_section_header: self.shell_transparent_color(palette.section_header),
            palette,
        }
    }

    /// Rebuild the connections panel snapshot if anything it draws from moved.
    ///
    /// Not called from any render. The panel used to re-enter the app from its own
    /// `render`, which made the root paint the reconciliation pump; this runs at
    /// the boundaries that actually change something instead -- panel interactions,
    /// connection-input subscriptions, and menu actions that open the group editor
    /// (through their deferred boundary helpers), plus every store reply (through
    /// `submit_store_request`, after the whole handler body has run).
    ///
    /// The derived model is reconciled first and unconditionally. It is memoised,
    /// so a no-op costs a key comparison, and it is what settles the search
    /// expansion -- which the panel key then reads. Computing the key first would
    /// read an expansion that this call is about to change.
    pub(in crate::features) fn flush_connection_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        let started_at = Instant::now();
        let model = self.connection_state.connection_list_model();
        let stats = model.stats;

        // Instrumentation lives here rather than in the panel, and not merely to
        // keep the app out of its render. These samples measure building the model;
        // the panel may now legitimately not render at all for many flushes, so a
        // sample taken there would quietly stop recording and misreport.
        let perf = self.gpui_perf_context(stats.flat_row_count, Some(stats.cache_hit));
        record_gpui_perf_sample(
            "connection_sections",
            Duration::from_secs_f64(stats.sections_ms / 1000.0),
            perf,
        );
        record_gpui_perf_sample(
            "flatten_connection_rows",
            Duration::from_secs_f64(stats.flatten_ms / 1000.0),
            perf,
        );
        record_gpui_perf_sample(
            "widest_connection_row",
            Duration::from_secs_f64(stats.widest_ms / 1000.0),
            perf,
        );

        let key = ConnectionListKey::new(
            self.connection_state.list_rows_key(),
            self.connection_state.list_selection(),
            self.connection_state
                .list_keyboard_active_connection_id()
                .map(str::to_string),
            self.connection_state.list_hovered_group_id(),
            self.connection_state.list_drop_target(),
            self.connection_chrome(),
        );

        let panel = self.connection_panel.clone();
        if panel.read(cx).snapshot_key() == Some(&key)
            && panel
                .read(cx)
                .icon_images_are_current(&self.connection_state.custom_icons.images)
        {
            return;
        }

        let snapshot = self.build_connection_list_snapshot(model, key);
        panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
        record_gpui_perf_sample("connection_snapshot", started_at.elapsed(), perf);
    }

    fn build_connection_list_snapshot(
        &mut self,
        model: crate::features::connections::ConnectionListModelSnapshot,
        key: ConnectionListKey,
    ) -> ConnectionListSnapshot {
        let connections = self.connection_state.connections();
        let connections_by_id: HashMap<String, _> = connections
            .iter()
            .map(|connection| (connection.id.clone(), connection.clone()))
            .collect();
        // A folder is worth showing before anything is filed under it, so the empty
        // state waits until there are no folders either. Otherwise a freshly created
        // folder is swallowed by "no saved connections".
        let store_is_empty = connections.is_empty()
            && self.connection_state.groups().is_empty()
            && self.connection_state.active_group_editor_draft().is_none();

        ConnectionListSnapshot {
            custom_icons: self.connection_state.custom_icons.images.clone(),
            chrome: key.chrome(),
            rows: model.rows,
            widest_row: model.widest_row,
            connections_by_id: Arc::new(connections_by_id),
            selection: self.connection_state.list_selection(),
            expanded_groups: self.connection_state.list_expanded_groups_arc(),
            keyboard_active: self
                .connection_state
                .list_keyboard_active_connection_id()
                .map(str::to_string),
            hovered_group: self.connection_state.list_hovered_group_id(),
            drop_target: self.connection_state.list_drop_target(),
            group_editor: self.connection_state.active_group_editor_draft(),
            group_editor_field: self.connection_state.group_editor_field(),
            search_field: self.connection_state.list_search_field(),
            search_is_empty: self.connection_state.list_search_is_empty(),
            sort_mode: self.connection_state.list_sort_mode(),
            store_is_empty,
            key,
        }
    }
}

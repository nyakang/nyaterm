use gpui::Context;
use nyaterm_store::{StoreDomain, store_request};

use crate::features::shell::TerminalWindowDockResult;
use crate::features::{NyaTermApp, formatting::short_id};
use crate::models::{NavItem, SmartSplitMode, TabDockZone};

impl NyaTermApp {
    /// Ensure every live session appears in the multi-leaf layout once it is enabled.
    pub(in crate::features) fn reconcile_terminal_windows(&mut self) {
        let mut live_ids = self
            .ordered_tab_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        live_ids.extend(
            self.transfer
                .editor_documents()
                .iter()
                .filter(|document| {
                    document.display == nyaterm_core::InternalEditorDisplay::Workspace
                })
                .map(|document| {
                    crate::models::WorkspaceTabId::Editor(document.id.clone()).layout_key()
                }),
        );
        live_ids.extend(
            self.file_manager
                .workspaces()
                .map(|(id, _)| crate::models::WorkspaceTabId::FileManager(id.clone()).layout_key()),
        );
        // The settings tab has no session or document behind it; the draft is
        // what keeps it alive.
        if self.shell.has_settings_draft() {
            live_ids.push(crate::models::WorkspaceTabId::settings_key());
        }
        let preferred = self.current_terminal_group();
        let active = self.active_workspace_tab();
        self.shell
            .reconcile_terminal_windows(&live_ids, preferred.as_deref(), active.as_deref());
        self.normalize_legacy_terminal_groups();
    }

    pub(in crate::features) fn ensure_terminal_windows_root(&mut self) {
        let mut tab_ids = self
            .ordered_tab_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        tab_ids.extend(
            self.transfer
                .editor_documents()
                .iter()
                .filter(|document| {
                    document.display == nyaterm_core::InternalEditorDisplay::Workspace
                })
                .map(|document| {
                    crate::models::WorkspaceTabId::Editor(document.id.clone()).layout_key()
                }),
        );
        tab_ids.extend(
            self.file_manager
                .workspaces()
                .map(|(id, _)| crate::models::WorkspaceTabId::FileManager(id.clone()).layout_key()),
        );
        if self.shell.has_settings_draft() {
            tab_ids.push(crate::models::WorkspaceTabId::settings_key());
        }
        let active = self.active_workspace_tab();
        self.shell.ensure_terminal_windows_root(tab_ids, active);
    }

    pub(in crate::features) fn terminal_windows_is_multi_leaf(&self) -> bool {
        self.shell.terminal_windows_is_multi_leaf()
    }

    pub(in crate::features) fn sync_terminal_windows_active_tab(&mut self, session_id: &str) {
        // Multi-leaf tab ids are tab roots; map secondary pane focus to its strip tab.
        let tab_id = self.tab_root_for_session(session_id);
        self.shell.workspace.focused_group = self.shell.sync_terminal_windows_active_tab(&tab_id);
    }

    pub(in crate::features) fn set_terminal_window_drop(
        &mut self,
        leaf_id: String,
        zone: TabDockZone,
        cx: &mut Context<Self>,
    ) {
        if self.shell.set_terminal_window_drop(leaf_id, zone) {
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_terminal_window_drop(&mut self, cx: &mut Context<Self>) {
        if self.shell.clear_terminal_window_drop() {
            cx.notify();
        }
    }

    pub(in crate::features) fn dock_tab_on_terminal_window_leaf(
        &mut self,
        tab_id: String,
        target_leaf_id: String,
        zone: TabDockZone,
        cx: &mut Context<Self>,
    ) {
        self.ensure_terminal_windows_root();
        if let TabDockZone::Edge(edge) = zone
            && !self.terminal_group_can_split(&target_leaf_id, edge.direction())
        {
            self.shell
                .set_status("not enough space to split this group".to_string());
            cx.notify();
            return;
        }
        let focused_leaf_id =
            match self
                .shell
                .dock_tab_on_terminal_window_leaf(&tab_id, &target_leaf_id, zone)
            {
                TerminalWindowDockResult::MissingTree => {
                    cx.notify();
                    return;
                }
                TerminalWindowDockResult::UnknownTab => {
                    self.shell
                        .set_status(format!("unknown tab {}", short_id(&tab_id)));
                    cx.notify();
                    return;
                }
                TerminalWindowDockResult::NoEffect => {
                    self.shell.set_status("tab dock had no effect".to_string());
                    cx.notify();
                    return;
                }
                TerminalWindowDockResult::Docked { focused_leaf_id } => focused_leaf_id,
            };
        let _ = focused_leaf_id;
        self.activate_workspace_tab(&tab_id, cx);
        self.shell.navigation.selected_nav = NavItem::Workspace;
        let zone_label = match zone {
            TabDockZone::Center => "merged into leaf".to_string(),
            TabDockZone::Edge(edge) => format!("split to {}", edge.label()),
        };
        self.shell
            .set_status(format!("docked tab {} ({})", short_id(&tab_id), zone_label));
        self.persist_terminal_window_layout();
        cx.notify();
    }

    /// Apply Tauri smart-split / tile layout: each open tab becomes its own multi-leaf window.
    pub(in crate::features) fn apply_smart_split(
        &mut self,
        mode: SmartSplitMode,
        cx: &mut Context<Self>,
    ) {
        if !self.normalize_legacy_terminal_groups() && !self.shell.workspace.pane_roots.is_empty() {
            return;
        }
        let tab_ids = self.workspace_tab_ids();
        if tab_ids.len() < 2 {
            self.shell.set_status("no tabs to tile".to_string());
            cx.notify();
            return;
        }
        let active = self.active_workspace_tab();
        let size = self
            .shell
            .terminal_workspace_size()
            .unwrap_or(self.shell.viewport_size());
        let Some(focused_leaf_id) =
            self.shell
                .tile_terminal_groups(&tab_ids, mode, size, active.as_deref())
        else {
            self.shell
                .set_status("unable to build tile layout".to_string());
            cx.notify();
            return;
        };
        // Clear global pane splits so multi-leaf rendering takes precedence cleanly.
        self.shell.workspace.split = None;
        self.shell.workspace.split_resize = None;
        self.sync_terminal_frame_snapshot_priority();
        self.shell.workspace.focused_group = focused_leaf_id;
        self.shell.navigation.selected_nav = NavItem::Workspace;
        self.shell
            .set_status(format!("applied {}", mode.label().to_ascii_lowercase()));
        self.persist_terminal_window_layout();
        // Global pane layout is obsolete while multi-leaf is active.
        self.persist_workspace_pane_layout();
        cx.notify();
    }

    pub(in crate::features) fn persist_terminal_window_layout(&mut self) {
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        if !self.settings.summary().startup_restore
            || !self.settings.summary().startup_restore_window_layout
        {
            return;
        }
        // Defer disk write — layout changes must not open redb on the UI hot path.
        self.shell.mark_window_layout_persist_dirty();
    }

    fn restore_session_window_layout(
        &mut self,
        layout: &nyaterm_core::RestorableTerminalWindowNode,
        ordered: &[String],
        active: Option<&str>,
    ) -> Option<Option<String>> {
        let tree = self.shell.terminal_window_tree();
        let result = self
            .shell
            .restore_terminal_window_layout(layout, ordered, active);
        if self
            .transfer
            .editor_documents()
            .iter()
            .any(|doc| doc.display == nyaterm_core::InternalEditorDisplay::Workspace)
        {
            self.shell.restore_terminal_window_tree(tree);
            return None;
        }
        result
    }

    pub(in crate::features) fn try_restore_terminal_window_layout(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.shell.terminal_windows_restore_is_complete()
            || self.shell.terminal_windows_restore_is_in_flight()
        {
            return;
        }
        if !self.settings.summary().startup_restore
            || !self.settings.summary().startup_restore_window_layout
        {
            self.shell.complete_terminal_windows_restore();
            return;
        }
        // Do not open the config DB during connect/register; wait for idle.
        if self.session.start_has_pending()
            || self.runtime_output_pressure_active()
            || !self.shell.workspace_layout_restore_is_settled()
            || !self.session.restore_is_complete()
        {
            return;
        }
        let ordered = self
            .ordered_tab_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        // Wait until startup restore has created sessions so tab indexes can map
        // correctly. Once startup restore is complete, an empty session list
        // means there is nothing to restore; mark this done so the runtime can
        // enter the quiet cadence.
        if ordered.is_empty() && self.transfer.editor_documents().is_empty() {
            if self.session.restore_is_complete() {
                self.shell.complete_terminal_windows_restore();
            }
            return;
        }
        let active = self.active_workspace_tab();
        let loaded = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.take_loaded_terminal_window_layout());
        if let Some(layout) = loaded {
            self.shell.complete_terminal_windows_restore();
            if let Some(layout) = layout
                && let Some(focused_leaf_id) =
                    self.restore_session_window_layout(&layout, &ordered, active.as_deref())
            {
                if self.shell.workspace.pane_roots.is_empty()
                    && let Some(group) = focused_leaf_id
                    && let Some((_, Some(tab))) = self.shell.terminal_group_tabs(&group)
                {
                    self.select_session(tab, cx);
                }
                self.shell
                    .set_status("restored multi-leaf window layout".to_string());
            }
            self.normalize_legacy_terminal_groups();
            cx.notify();
            return;
        }
        self.shell.begin_terminal_windows_restore();
        let restore_revision = self.workspace_revision;
        self.submit_store_request(
            0,
            store_request(StoreDomain::Sessions, |store| {
                store.load_terminal_window_layout()
            }),
            move |this, event, cx| {
                this.shell.complete_terminal_windows_restore();
                if this.workspace_revision != restore_revision {
                    match &event.outcome {
                        Ok(Some(layout))
                            if crate::models::TerminalWindowNode::restore_layout(
                                layout, &ordered,
                            )
                            .is_none() =>
                        {
                            this.shell.preserve_unread_terminal_layout()
                        }
                        Err(_) => this.shell.preserve_unread_terminal_layout(),
                        _ => {}
                    }
                    this.normalize_legacy_terminal_groups();
                    return;
                }
                match event.outcome {
                    Ok(Some(layout)) => {
                        if let Some(focused_leaf_id) =
                            this.restore_session_window_layout(&layout, &ordered, active.as_deref())
                        {
                            if this.shell.workspace.pane_roots.is_empty()
                                && let Some(group) = focused_leaf_id
                                && let Some((_, Some(tab))) = this.shell.terminal_group_tabs(&group)
                            {
                                this.select_session(tab, cx);
                            }
                            this.shell
                                .set_status("restored multi-leaf window layout".to_string());
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        this.shell.preserve_unread_terminal_layout();
                        this.shell
                            .set_status(format!("failed to restore terminal layout: {error}"));
                    }
                }
                this.normalize_legacy_terminal_groups();
                cx.notify();
            },
            cx,
        );
    }
}

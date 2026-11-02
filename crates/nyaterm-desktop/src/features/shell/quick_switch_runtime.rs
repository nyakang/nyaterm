use rust_i18n::t;

use std::collections::HashMap;

use gpui::{AppContext as _, Context, Focusable as _, Window};
use nyaterm_core::fuzzy_search_items;
use nyaterm_ui::NyaCommandState;

use crate::entities::{OverlayStore, QuickSwitchState};
use crate::features::{NyaTermApp, formatting::session_kind_label, formatting::short_id};
use crate::models::QuickSwitchItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceSurfaceFocusTarget {
    Terminal,
    RemoteDesktop,
}

fn workspace_surface_focus_target(
    active_session_id: Option<&str>,
    is_remote_desktop: impl FnOnce(&str) -> bool,
) -> WorkspaceSurfaceFocusTarget {
    if active_session_id.is_some_and(is_remote_desktop) {
        WorkspaceSurfaceFocusTarget::RemoteDesktop
    } else {
        WorkspaceSurfaceFocusTarget::Terminal
    }
}

impl NyaTermApp {
    pub(in crate::features) fn quick_switch_state(
        &self,
        cx: &mut Context<Self>,
    ) -> QuickSwitchState {
        self.stores
            .overlays
            .read_with(cx, |store, _| store.quick_switch().clone())
    }

    pub(in crate::features) fn quick_switch_open(&self, cx: &mut Context<Self>) -> bool {
        self.quick_switch_state(cx).is_open()
    }

    pub(in crate::features) fn update_quick_switch_state(
        &mut self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut OverlayStore) -> bool,
    ) -> bool {
        let changed = self.stores.overlays.update(cx, |store, cx| {
            let changed = update(store);
            if changed {
                cx.notify();
            }
            changed
        });
        if changed {
            cx.notify();
        }
        changed
    }

    pub(in crate::features) fn quick_switch_command_state(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<NyaCommandState>> {
        self.quick_switch_state(cx).command_state()
    }

    pub(in crate::features) fn open_quick_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command_state = cx.new(|cx| NyaCommandState::new(window, cx));
        self.update_quick_switch_state(cx, |store| store.open_quick_switch(command_state.clone()));
        command_state.update(cx, |state, cx| state.focus(window, cx));
        self.shell.set_status("quick switch opened".to_string());
        cx.notify();
    }

    pub(in crate::features) fn close_quick_switch(&mut self, cx: &mut Context<Self>) {
        self.update_quick_switch_state(cx, |store| store.close_quick_switch());
        self.shell.set_status("quick switch closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn dismiss_quick_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_quick_switch(cx);
        self.focus_active_workspace_surface(window, cx);
    }

    pub(in crate::features) fn focus_active_workspace_surface(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.active_workspace_tab() {
            match crate::models::WorkspaceTabId::from_layout_key(&key) {
                crate::models::WorkspaceTabId::Editor(id) => {
                    if let Some(view) = self.transfer.workspace_editor_view(&id) {
                        view.update(cx, |view, cx| view.focus(window, cx));
                        return;
                    }
                }
                crate::models::WorkspaceTabId::FileManager(id) => {
                    if let Some(view) = self.file_manager.workspace(&id) {
                        window.focus(&view.read(cx).focus_handle(cx), cx);
                        return;
                    }
                }
                // The settings surface owns its own focus targets.
                crate::models::WorkspaceTabId::Settings => return,
                crate::models::WorkspaceTabId::Session(_) => {}
            }
        }
        match workspace_surface_focus_target(self.session.active_id(), |session_id| {
            self.remote_desktop.is_session(session_id)
        }) {
            WorkspaceSurfaceFocusTarget::RemoteDesktop => {
                window.focus(self.remote_desktop.focus(), cx)
            }
            WorkspaceSurfaceFocusTarget::Terminal => window.focus(self.terminal.input_focus(), cx),
        }
    }

    pub(in crate::features) fn quick_switch_items(&self) -> Vec<QuickSwitchItem> {
        let sessions = self.ordered_tab_sessions();
        let active_tab = self
            .selected_session_strip_tab(&self.current_terminal_group())
            .map(|id| id.layout_key());
        let mut session_items = Vec::new();
        for session in &sessions {
            let title = self.session.display_name_by_info(session);
            let active = active_tab.as_deref() == Some(session.id.as_str());
            let mut subtitle = format!(
                "{} - {}",
                session_kind_label(session.kind),
                short_id(&session.id)
            );
            if let Some(path) = session.working_dir.as_ref() {
                subtitle.push_str(" - ");
                subtitle.push_str(&path.display().to_string());
            }
            session_items.push(QuickSwitchItem::Session {
                id: session.id.clone(),
                title,
                subtitle,
                active,
            });
        }

        for key in self.workspace_tab_ids() {
            match crate::models::WorkspaceTabId::from_layout_key(&key) {
                crate::models::WorkspaceTabId::Editor(id) => {
                    if let Some(document) = self.transfer.editor_tab_snapshot(&id) {
                        session_items.push(QuickSwitchItem::Session {
                            active: active_tab.as_deref() == Some(&key),
                            id: key,
                            title: document.name,
                            subtitle: document.remote_path,
                        });
                    }
                }
                crate::models::WorkspaceTabId::FileManager(id) => {
                    session_items.push(QuickSwitchItem::Session {
                        active: active_tab.as_deref() == Some(&key),
                        id: key,
                        title: t!("fileManager.tabTitle").to_string(),
                        subtitle: id,
                    });
                }
                crate::models::WorkspaceTabId::Settings => {
                    session_items.push(QuickSwitchItem::Session {
                        active: active_tab.as_deref() == Some(&key),
                        id: key,
                        title: t!("settings.title").to_string(),
                        subtitle: t!("settings.title").to_string(),
                    });
                }
                crate::models::WorkspaceTabId::Session(_) => {}
            }
        }
        let order = self.workspace_tab_ids();
        session_items.sort_by_key(|item| {
            order
                .iter()
                .position(|key| item.id() == *key)
                .unwrap_or(usize::MAX)
        });
        let mut transient_items = self
            .session
            .start_pending_entries()
            .filter(|(_, pending)| pending.reconnect_session_id.is_none())
            .map(|(request_id, pending)| {
                let insert_index = quick_switch_transient_insert_index(
                    sessions.len(),
                    pending.insert_index,
                    pending.after_session_id.as_deref().and_then(|after_id| {
                        sessions.iter().position(|session| session.id == after_id)
                    }),
                );
                let title = pending
                    .custom_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&pending.connection_name)
                    .to_string();
                (
                    insert_index,
                    pending.requested_at,
                    pending.connection_name.clone(),
                    QuickSwitchItem::Pending {
                        request_id: request_id.clone(),
                        title,
                        subtitle: format!(
                            "{} - {}",
                            t!("sessionQuickSwitcher.connecting"),
                            session_kind_label(pending.kind)
                        ),
                        active: self.session.start_request_is_active(request_id),
                        failed: false,
                        search_detail: None,
                    },
                )
            })
            .chain(
                self.session
                    .start_failed_entries()
                    .map(|(request_id, failed)| {
                        let pending = &failed.pending;
                        let insert_index = quick_switch_transient_insert_index(
                            sessions.len(),
                            pending.insert_index,
                            pending.after_session_id.as_deref().and_then(|after_id| {
                                sessions.iter().position(|session| session.id == after_id)
                            }),
                        );
                        let title = pending
                            .custom_name
                            .as_deref()
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .unwrap_or(&pending.connection_name)
                            .to_string();
                        (
                            insert_index,
                            pending.requested_at,
                            pending.connection_name.clone(),
                            QuickSwitchItem::Pending {
                                request_id: request_id.clone(),
                                title,
                                subtitle: format!(
                                    "{} - {}",
                                    if self.session.start_request_cancelled(request_id) {
                                        t!("terminal.connectionCancelled")
                                    } else {
                                        t!("terminal.connectionFailed")
                                    },
                                    session_kind_label(pending.kind)
                                ),
                                active: self.session.start_request_is_active(request_id),
                                failed: true,
                                search_detail: Some(failed.error.clone()),
                            },
                        )
                    }),
            )
            .collect::<Vec<_>>();
        transient_items.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(left.2.cmp(&right.2))
        });
        for (offset, (insert_index, _, _, item)) in transient_items.into_iter().enumerate() {
            session_items.insert((insert_index + offset).min(session_items.len()), item);
        }

        let mut items = session_items;

        let mut connections = self.connection_state.connections().to_vec();
        connections.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then(left.id.cmp(&right.id))
        });
        for connection in connections {
            items.push(QuickSwitchItem::Connection {
                title: connection.name.clone(),
                subtitle: format!("{} - {}", connection.kind_label(), connection.endpoint()),
                connection: Box::new(connection),
            });
        }
        items
    }

    pub(in crate::features) fn filtered_quick_switch_items(
        &self,
        query: &str,
    ) -> Vec<QuickSwitchItem> {
        filter_quick_switch_items(self.quick_switch_items(), query)
    }

    pub(in crate::features) fn select_quick_switch_item(
        &mut self,
        item: QuickSwitchItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command_focus = window.focused(cx);
        self.close_quick_switch(cx);
        self.mark_user_activity();
        match item {
            QuickSwitchItem::Session { id, .. } => {
                self.select_session(id, cx);
            }
            QuickSwitchItem::Connection { connection, .. } => {
                self.start_saved_connection(*connection, window, cx);
            }
            QuickSwitchItem::Pending {
                request_id, failed, ..
            } => {
                if failed {
                    self.select_failed_session_start(request_id, cx);
                } else {
                    self.select_pending_session_start(request_id, cx);
                }
                cx.notify();
            }
        }
        if window.focused(cx) == command_focus {
            self.focus_active_workspace_surface(window, cx);
        }
    }
}

const QUICK_SWITCH_RESULT_LIMIT: usize = 50;

fn filter_quick_switch_items(items: Vec<QuickSwitchItem>, query: &str) -> Vec<QuickSwitchItem> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return items.into_iter().take(QUICK_SWITCH_RESULT_LIMIT).collect();
    }

    let candidates = items
        .iter()
        .map(|item| (item.search_text(), item.id()))
        .collect::<Vec<_>>();
    let candidate_refs = candidates
        .iter()
        .map(|(display, id)| (display.as_str(), id.as_str()))
        .collect::<Vec<_>>();
    let matches = fuzzy_search_items(
        &candidate_refs,
        &query,
        "sessionQuickSwitcher",
        QUICK_SWITCH_RESULT_LIMIT,
        None,
        None,
    );
    let mut items_by_id = items
        .into_iter()
        .map(|item| (item.id(), item))
        .collect::<HashMap<_, _>>();
    matches
        .into_iter()
        .filter_map(|matched| items_by_id.remove(&matched.command))
        .collect()
}

fn quick_switch_transient_insert_index(
    session_count: usize,
    insert_index: Option<usize>,
    after_position: Option<usize>,
) -> usize {
    insert_index
        .or_else(|| after_position.map(|position| position + 1))
        .unwrap_or(session_count)
        .min(session_count)
}

#[cfg(test)]
mod tests {
    use super::{
        WorkspaceSurfaceFocusTarget, filter_quick_switch_items,
        quick_switch_transient_insert_index, workspace_surface_focus_target,
    };
    use crate::models::QuickSwitchItem;

    fn session_item(
        index: usize,
        title: impl Into<String>,
        subtitle: impl Into<String>,
    ) -> QuickSwitchItem {
        QuickSwitchItem::Session {
            id: format!("session-{index}"),
            title: title.into(),
            subtitle: subtitle.into(),
            active: false,
        }
    }

    #[test]
    fn empty_query_keeps_catalog_order_and_limits_results_to_fifty() {
        let items = (0..60)
            .map(|index| session_item(index, format!("Session {index}"), "Local"))
            .collect();

        let filtered = filter_quick_switch_items(items, "   ");

        assert_eq!(filtered.len(), 50);
        assert_eq!(
            filtered.first().map(QuickSwitchItem::title),
            Some("Session 0")
        );
        assert_eq!(
            filtered.last().map(QuickSwitchItem::title),
            Some("Session 49")
        );
    }

    #[test]
    fn non_empty_query_uses_search_text_and_limits_fuzzy_results() {
        let mut items = (0..60)
            .map(|index| session_item(index, format!("Server {index}"), "needle target"))
            .collect::<Vec<_>>();
        items.push(session_item(99, "Unrelated", "nothing here"));

        let filtered = filter_quick_switch_items(items, "needle");

        assert_eq!(filtered.len(), 50);
        assert!(
            filtered
                .iter()
                .all(|item| item.subtitle() == "needle target")
        );
    }

    #[test]
    fn transient_items_follow_workspace_tab_insertion_rules() {
        assert_eq!(quick_switch_transient_insert_index(3, None, None), 3);
        assert_eq!(quick_switch_transient_insert_index(3, None, Some(0)), 1);
        assert_eq!(quick_switch_transient_insert_index(3, Some(2), Some(0)), 2);
        assert_eq!(quick_switch_transient_insert_index(3, Some(99), None), 3);
    }

    #[test]
    fn workspace_focus_targets_terminal_or_remote_desktop_by_active_session() {
        assert_eq!(
            workspace_surface_focus_target(None, |_| true),
            WorkspaceSurfaceFocusTarget::Terminal
        );
        assert_eq!(
            workspace_surface_focus_target(Some("ssh"), |_| false),
            WorkspaceSurfaceFocusTarget::Terminal
        );
        assert_eq!(
            workspace_surface_focus_target(Some("rdp-or-vnc"), |_| true),
            WorkspaceSurfaceFocusTarget::RemoteDesktop
        );
    }
}

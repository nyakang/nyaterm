use rust_i18n::t;

use gpui::{Context, KeyDownEvent, Window};
use nyaterm_core::{SavedConnection, SessionsConfig};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;

#[derive(Debug, PartialEq)]
enum SelectedConnectionsDeleteTarget {
    None,
    Single(String),
    Multiple(Vec<SavedConnection>),
}

fn selected_connections_delete_target(
    selected: Vec<SavedConnection>,
) -> SelectedConnectionsDeleteTarget {
    match selected.len() {
        0 => SelectedConnectionsDeleteTarget::None,
        1 => SelectedConnectionsDeleteTarget::Single(selected[0].id.clone()),
        _ => SelectedConnectionsDeleteTarget::Multiple(selected),
    }
}

impl NyaTermApp {
    pub(in crate::features) fn open_connections_clear_all_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell
            .set_status("confirm clearing all saved connections".to_string());
        self.open_confirm_dialog(
            (
                t!("savedConnections.clearAll").to_string(),
                t!("savedConnections.clearAllConfirm").to_string(),
                t!("savedConnections.clearAll").to_string(),
                true,
                |app, _, cx| {
                    app.confirm_connections_clear_all(cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn confirm_connections_clear_all(&mut self, cx: &mut Context<Self>) {
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, |store| {
                store.replace_sessions(&SessionsConfig::default())?;
                store.load_sessions()
            }),
            |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.connection_state.clear_list_runtime_state();
                    this.apply_loaded_sessions(sessions, cx);
                    this.shell
                        .set_status(t!("savedConnections.clearAllSuccess").to_string());
                    this.settings
                        .update_store_status(this.shell.status().to_string(), true);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("clear saved connections failed: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn open_connection_delete_confirm(
        &mut self,
        connection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self
            .connection_state
            .connections()
            .iter()
            .find(|connection| connection.id == connection_id)
        else {
            self.shell
                .set_status("connection is no longer available".to_string());
            cx.notify();
            return;
        };
        let label = connection.name.clone();
        self.shell
            .set_status("confirm connection delete".to_string());
        self.open_confirm_dialog(
            (
                t!("savedConnections.delete").to_string(),
                t!("savedConnections.deleteConfirm", name = label).to_string(),
                t!("savedConnections.delete").to_string(),
                true,
                move |app, _, cx| {
                    app.confirm_connection_delete(connection_id.clone(), label.clone(), cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    fn open_selected_connections_delete_confirm(
        &mut self,
        selected: Vec<nyaterm_core::SavedConnection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = selected.len();
        self.shell
            .set_status("confirm selected connections delete".to_string());
        self.open_confirm_dialog(
            (
                t!("savedConnections.deleteSelected").to_string(),
                t!("savedConnections.deleteSelectedConfirm", count = count).to_string(),
                t!("savedConnections.delete").to_string(),
                true,
                move |app, _, cx| {
                    app.confirm_selected_connections_delete(selected.clone(), cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    fn confirm_connection_delete(
        &mut self,
        connection_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let persisted_id = connection_id.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                store.delete_connection(&persisted_id)?;
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.connection_state
                        .remove_list_connection_references(&connection_id);
                    this.apply_loaded_sessions(sessions, cx);
                    this.shell.set_status(format!("deleted connection {label}"));
                    cx.notify();
                }
                Err(error) => {
                    this.shell
                        .set_status(format!("delete connection failed: {error}"));
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn open_connection_group_delete_confirm(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .connection_state
            .groups()
            .iter()
            .find(|group| group.id == group_id)
        else {
            self.shell
                .set_status("connection group is no longer available".to_string());
            cx.notify();
            return;
        };
        let connection_count = self
            .connection_state
            .connections()
            .iter()
            .filter(|connection| connection.group_id.as_deref() == Some(group_id.as_str()))
            .count();
        let child_group_count = self
            .connection_state
            .groups()
            .iter()
            .filter(|child| child.parent_id.as_deref() == Some(group_id.as_str()))
            .count();
        let label = group.name.clone();
        self.shell
            .set_status("confirm connection group delete".to_string());
        let message = t!(
            "savedConnections.deleteFolderConfirm",
            name = label,
            count = connection_count,
            childCount = child_group_count
        )
        .to_string();
        self.open_confirm_dialog(
            (
                t!("savedConnections.deleteFolder").to_string(),
                message,
                t!("savedConnections.deleteFolder").to_string(),
                true,
                move |app, _, cx| {
                    app.confirm_connection_group_delete(group_id.clone(), label.clone(), cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    fn confirm_connection_group_delete(
        &mut self,
        group_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let persisted_id = group_id.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                store.delete_group(&persisted_id)?;
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.connection_state
                        .remove_list_group_references(&group_id);
                    this.apply_loaded_sessions(sessions, cx);
                    this.shell
                        .set_status(format!("deleted connection group {label}"));
                    cx.notify();
                }
                Err(error) => {
                    this.shell
                        .set_status(format!("delete connection group failed: {error}"));
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn toggle_connection_group_expanded(
        &mut self,
        group_id: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.toggle_list_group_expanded(group_id);
        self.persist_ui_layout();
        cx.notify();
    }

    pub(in crate::features) fn cycle_connection_sort_mode(&mut self, cx: &mut Context<Self>) {
        let sort_mode = self.connection_state.cycle_list_sort_mode();
        self.settings
            .set_saved_connections_sort_mode(sort_mode.persistence_id().to_string());
        self.persist_ui_layout();
        self.shell
            .set_status(format!("connections sorted by {}", sort_mode.label()));
        cx.notify();
    }

    /// Move the keyboard-active row through the filtered results, wrapping around.
    ///
    /// Returns whether the key was consumed, so the caller does not also feed it
    /// to the text field.
    fn step_connection_keyboard_active(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let visible = self.connection_state.visible_connection_ids();
        if visible.is_empty() {
            return false;
        }
        let current = self
            .connection_state
            .list_keyboard_active_connection_id()
            .and_then(|id| visible.iter().position(|candidate| candidate == id));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1) % visible.len(),
            (Some(index), false) => (index + visible.len() - 1) % visible.len(),
            (None, true) => 0,
            (None, false) => visible.len() - 1,
        };
        self.connection_state
            .set_list_keyboard_active_connection_id(Some(visible[next].clone()));
        cx.notify();
        true
    }

    /// Open the keyboard-active row, or the first result when nothing is active.
    fn open_connection_keyboard_active(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let visible = self.connection_state.visible_connection_ids();
        let Some(target) = self
            .connection_state
            .list_keyboard_active_connection_id()
            .filter(|id| visible.iter().any(|candidate| candidate == id))
            .map(ToOwned::to_owned)
            .or_else(|| visible.first().cloned())
        else {
            return false;
        };
        let Some(connection) = self
            .connection_state
            .connections()
            .iter()
            .find(|connection| connection.id == target)
            .cloned()
        else {
            return false;
        };
        self.start_saved_connection(connection, window, cx);
        true
    }

    /// Drop the keyboard-active row once the filter no longer shows it.
    pub(in crate::features) fn sync_connection_keyboard_active(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self
            .connection_state
            .list_keyboard_active_connection_id()
            .map(ToOwned::to_owned)
        else {
            return;
        };
        if !self
            .connection_state
            .visible_connection_ids()
            .iter()
            .any(|candidate| candidate == &active)
        {
            self.connection_state
                .set_list_keyboard_active_connection_id(None);
            cx.notify();
        }
    }

    /// Keys the filter field deliberately leaves alone.
    ///
    /// The field consumes its own editing keys, so anything arriving here is a
    /// list gesture: walk the filtered results, open one, or clear the filter.
    pub(in crate::features) fn handle_connection_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.alt
            || keystroke.modifiers.function
            || keystroke.modifiers.platform
            || keystroke.modifiers.control
        {
            return;
        }

        match keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.clear_connection_search(window, cx);
            }
            "up" | "down" if !self.connection_state.list_search_is_empty() => {
                if self.step_connection_keyboard_active(keystroke.key == "down", cx) {
                    cx.stop_propagation();
                }
            }
            "enter"
                if !self.connection_state.list_search_is_empty()
                    && self.open_connection_keyboard_active(window, cx) =>
            {
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    pub(in crate::features) fn clear_connection_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = self.connection_state.list_search_field();
        field.update(cx, |field, cx| field.set_content("", cx));
        self.connection_state.set_list_search_text(String::new());
        window.focus(&field.read(cx).focus_handle(), cx);
        self.shell
            .set_status("connection search cleared".to_string());
        self.sync_connection_keyboard_active(cx);
        cx.notify();
    }

    pub(in crate::features) fn delete_selected_connections(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match selected_connections_delete_target(self.connection_state.selected_connections()) {
            SelectedConnectionsDeleteTarget::None => {
                self.shell
                    .set_status("select saved connections before deleting".to_string());
                cx.notify();
            }
            SelectedConnectionsDeleteTarget::Single(connection_id) => {
                self.open_connection_delete_confirm(connection_id, window, cx);
            }
            SelectedConnectionsDeleteTarget::Multiple(selected) => {
                self.open_selected_connections_delete_confirm(selected, window, cx);
            }
        }
    }

    fn confirm_selected_connections_delete(
        &mut self,
        selected: Vec<nyaterm_core::SavedConnection>,
        cx: &mut Context<Self>,
    ) {
        let persisted = selected.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                for connection in &persisted {
                    store.delete_connection(&connection.id)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    for connection in &selected {
                        this.connection_state
                            .remove_list_connection_references(&connection.id);
                    }
                    this.apply_loaded_sessions(sessions, cx);
                    this.shell
                        .set_status(format!("deleted {} connection(s)", selected.len()));
                    cx.notify();
                }
                Err(error) => {
                    this.shell
                        .set_status(format!("delete selected connections failed: {error}"));
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn rename_connection(
        &mut self,
        connection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_connection_editor(Some(connection_id), None, false, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection};

    use super::{SelectedConnectionsDeleteTarget, selected_connections_delete_target};

    fn saved_connection(id: &str) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            id: id.to_string(),
            name: id.to_string(),
            config: ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    #[test]
    fn selected_connections_delete_target_requires_multi_delete_confirmation() {
        assert_eq!(
            selected_connections_delete_target(Vec::new()),
            SelectedConnectionsDeleteTarget::None
        );
        assert_eq!(
            selected_connections_delete_target(vec![saved_connection("one")]),
            SelectedConnectionsDeleteTarget::Single("one".to_string())
        );

        let target = selected_connections_delete_target(vec![
            saved_connection("one"),
            saved_connection("two"),
        ]);
        let SelectedConnectionsDeleteTarget::Multiple(selected) = target else {
            panic!("multi-selection must use the multi-delete confirmation path");
        };
        assert_eq!(
            selected
                .iter()
                .map(|connection| connection.id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
    }
}

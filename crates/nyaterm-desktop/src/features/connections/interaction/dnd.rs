use gpui::Context;
use nyaterm_core::{Group, SavedConnection};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn move_connection_before(
        &mut self,
        source_id: String,
        target_id: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        if source_id == target_id {
            return;
        }
        let Some(source) = self
            .connection_state
            .connections()
            .iter()
            .find(|c| c.id == source_id)
            .cloned()
        else {
            self.shell
                .set_status("drag source connection missing".to_string());
            cx.notify();
            return;
        };
        let Some(target) = self
            .connection_state
            .connections()
            .iter()
            .find(|c| c.id == target_id)
            .cloned()
        else {
            self.shell
                .set_status("drop target connection missing".to_string());
            cx.notify();
            return;
        };

        let parent = target.group_id.clone();
        let mut siblings = self
            .connection_state
            .connections()
            .iter()
            .filter(|c| c.group_id == parent && c.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.sort_by(|a, b| {
            a.sort_order.cmp(&b.sort_order).then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
        });
        let target_idx = siblings
            .iter()
            .position(|c| c.id == target_id)
            .unwrap_or(siblings.len());
        let mut moved = source;
        moved.group_id = parent;
        siblings.insert(target_idx, moved);

        self.submit_connection_order_persistence(
            siblings,
            "reorder connection",
            |this, _| this.shell.set_status("connection reordered".to_string()),
            cx,
        );
        self.connection_state.clear_list_drop_target();
        cx.notify();
    }

    pub(in crate::features) fn move_connection_after(
        &mut self,
        source_id: String,
        target_id: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        if source_id == target_id {
            return;
        }
        let Some(source) = self
            .connection_state
            .connections()
            .iter()
            .find(|c| c.id == source_id)
            .cloned()
        else {
            self.shell
                .set_status("drag source connection missing".to_string());
            cx.notify();
            return;
        };
        let Some(target) = self
            .connection_state
            .connections()
            .iter()
            .find(|c| c.id == target_id)
            .cloned()
        else {
            self.shell
                .set_status("drop target connection missing".to_string());
            cx.notify();
            return;
        };

        let parent = target.group_id.clone();
        let mut siblings = self
            .connection_state
            .connections()
            .iter()
            .filter(|c| c.group_id == parent && c.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.sort_by(|a, b| {
            a.sort_order.cmp(&b.sort_order).then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
        });
        let target_idx = siblings
            .iter()
            .position(|c| c.id == target_id)
            .map(|idx| idx + 1)
            .unwrap_or(siblings.len());
        let mut moved = source;
        moved.group_id = parent;
        siblings.insert(target_idx.min(siblings.len()), moved);

        self.submit_connection_order_persistence(
            siblings,
            "reorder connection",
            |this, _| this.shell.set_status("connection reordered".to_string()),
            cx,
        );
        self.connection_state.clear_list_drop_target();
        cx.notify();
    }

    pub(in crate::features) fn move_connection_into_group(
        &mut self,
        source_id: String,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        let Some(source) = self
            .connection_state
            .connections()
            .iter()
            .find(|c| c.id == source_id)
            .cloned()
        else {
            self.shell
                .set_status("drag source connection missing".to_string());
            cx.notify();
            return;
        };
        if source.group_id == group_id {
            // already there: append to end of group order
        }
        let mut siblings = self
            .connection_state
            .connections()
            .iter()
            .filter(|c| c.group_id == group_id && c.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.sort_by_key(|connection| connection.sort_order);
        let mut moved = source;
        moved.group_id = group_id.clone();
        siblings.push(moved);

        self.submit_connection_order_persistence(
            siblings,
            "move connection",
            move |this, _| {
                if let Some(gid) = group_id {
                    this.connection_state.expand_list_group(gid);
                }
                this.shell.set_status("connection moved".to_string());
            },
            cx,
        );
        cx.notify();
    }

    /// Reparent several connections in one write.
    ///
    /// Looping [`Self::move_connection_into_group`] would re-read the list, persist
    /// a fresh order and refresh the store once per connection; the old UI sent a
    /// single reorder. One ordered write also means the list cannot be observed
    /// half-moved.
    pub(in crate::features) fn move_connections_into_group(
        &mut self,
        source_ids: Vec<String>,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        let moving = self
            .connection_state
            .connections()
            .iter()
            .filter(|connection| source_ids.contains(&connection.id))
            .cloned()
            .collect::<Vec<_>>();
        if moving.is_empty() {
            self.shell.set_status("no connections to move".to_string());
            cx.notify();
            return;
        }

        let moved_count = moving.len();
        let ordered = self
            .connection_state
            .connections_reordered_into_group(&source_ids, &group_id);

        self.submit_connection_order_persistence(
            ordered,
            "move connections",
            move |this, _| {
                if let Some(group_id) = group_id {
                    this.connection_state.expand_list_group(group_id);
                }
                this.shell
                    .set_status(format!("moved {moved_count} connection(s)"));
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn move_group_before(
        &mut self,
        source_id: String,
        target_id: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        if source_id == target_id {
            return;
        }
        let Some(source) = self
            .connection_state
            .groups()
            .iter()
            .find(|g| g.id == source_id)
            .cloned()
        else {
            return;
        };
        let Some(target) = self
            .connection_state
            .groups()
            .iter()
            .find(|g| g.id == target_id)
            .cloned()
        else {
            return;
        };
        // Only reorder among same parent for now.
        let parent = target.parent_id.clone();
        let mut siblings = self
            .connection_state
            .groups()
            .iter()
            .filter(|g| g.parent_id == parent && g.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.sort_by_key(|group| group.sort_order);
        let target_idx = siblings
            .iter()
            .position(|g| g.id == target_id)
            .unwrap_or(siblings.len());
        let mut moved = source;
        moved.parent_id = parent;
        siblings.insert(target_idx, moved);
        self.submit_group_order_persistence(
            siblings,
            "reorder group",
            |this, _| this.shell.set_status("group reordered".to_string()),
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn move_group_into_group(
        &mut self,
        source_id: String,
        parent_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.clear_list_drop_target();
        if parent_id.as_deref() == Some(source_id.as_str()) {
            self.shell
                .set_status("cannot nest group into itself".to_string());
            cx.notify();
            return;
        }
        // Prevent cycles: parent cannot be descendant of source.
        if let Some(pid) = parent_id.as_ref()
            && self.connection_state.group_is_descendant(pid, &source_id)
        {
            self.shell
                .set_status("cannot create group cycle".to_string());
            cx.notify();
            return;
        }
        let Some(source) = self
            .connection_state
            .groups()
            .iter()
            .find(|g| g.id == source_id)
            .cloned()
        else {
            return;
        };
        let mut siblings = self
            .connection_state
            .groups()
            .iter()
            .filter(|g| g.parent_id == parent_id && g.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        siblings.sort_by_key(|group| group.sort_order);
        let mut moved = source;
        moved.parent_id = parent_id;
        siblings.push(moved);
        self.submit_group_order_persistence(
            siblings,
            "move group",
            |this, _| this.shell.set_status("group moved".to_string()),
            cx,
        );
        cx.notify();
    }

    fn submit_connection_order_persistence(
        &mut self,
        ordered: Vec<SavedConnection>,
        error_action: &'static str,
        on_success: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                for (index, connection) in ordered.iter().enumerate() {
                    let mut updated = connection.clone();
                    updated.sort_order = index as i32;
                    store.save_connection(&updated)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.apply_loaded_sessions(sessions, cx);
                    on_success(this, cx);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("{error_action} failed: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                    cx.notify();
                }
            },
            cx,
        );
    }

    fn submit_group_order_persistence(
        &mut self,
        ordered: Vec<Group>,
        error_action: &'static str,
        on_success: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                for (index, group) in ordered.iter().enumerate() {
                    let mut updated = group.clone();
                    updated.sort_order = index as i32;
                    store.save_group(&updated)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.apply_loaded_sessions(sessions, cx);
                    on_success(this, cx);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("{error_action} failed: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                    cx.notify();
                }
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection};

    use crate::features::connections::catalog::ConnectionCatalogState;

    fn connection(id: &str, group_id: Option<&str>, sort_order: i32) -> SavedConnection {
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
            group_id: group_id.map(ToOwned::to_owned),
            description: None,
            sort_order,
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

    fn ids(connections: &[SavedConnection]) -> Vec<&str> {
        connections.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn moved_connections_land_after_the_group_in_list_order() {
        let connections = vec![
            connection("a", None, 0),
            connection("target-1", Some("target"), 1),
            connection("b", None, 2),
            connection("target-0", Some("target"), 0),
        ];

        let catalog = ConnectionCatalogState::new(connections, Vec::new());
        let ordered = catalog.connections_reordered_into_group(
            // Deliberately reversed: the selection order must not leak through.
            &["b".to_string(), "a".to_string()],
            &Some("target".to_string()),
        );

        assert_eq!(ids(&ordered), vec!["target-0", "target-1", "a", "b"]);
        assert!(
            ordered
                .iter()
                .all(|c| c.group_id.as_deref() == Some("target"))
        );
    }

    #[test]
    fn moving_to_ungrouped_clears_the_group_and_skips_the_movers() {
        let connections = vec![
            connection("root", None, 0),
            connection("grouped", Some("g"), 0),
        ];

        let catalog = ConnectionCatalogState::new(connections, Vec::new());
        let ordered = catalog.connections_reordered_into_group(&["grouped".to_string()], &None);

        assert_eq!(ids(&ordered), vec!["root", "grouped"]);
        assert!(ordered.iter().all(|c| c.group_id.is_none()));
    }

    #[test]
    fn a_connection_already_in_the_target_is_not_duplicated() {
        let connections = vec![
            connection("stay", Some("g"), 0),
            connection("move", Some("g"), 1),
        ];

        let catalog = ConnectionCatalogState::new(connections, Vec::new());
        let ordered =
            catalog.connections_reordered_into_group(&["move".to_string()], &Some("g".to_string()));

        assert_eq!(ids(&ordered), vec!["stay", "move"]);
    }
}

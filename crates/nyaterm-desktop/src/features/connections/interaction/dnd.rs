use gpui::Context;
use nyaterm_store::StoreDomain;

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn submit_connection_tree_move(
        &mut self,
        request: nyaterm_core::connection_tree::MoveRequest,
        panel: gpui::WeakEntity<crate::features::pages::connections::panel::ConnectionPanel>,
        cx: &mut Context<Self>,
    ) {
        let parent = request.destination.parent.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
                store.move_connection_tree(&request)?;
                store.load_sessions()
            }),
            move |this, event, cx| {
                let success = event.outcome.is_ok();
                match event.outcome {
                    Ok(sessions) => {
                        this.apply_loaded_sessions(sessions, cx);
                        if let Some(parent) = parent {
                            this.connection_state.expand_list_group(parent);
                        }
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("move connection tree failed: {error}"));
                    }
                }
                this.defer_connection_panel_snapshot_flush(cx);
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |panel, cx| panel.finish_tree_move(success, cx));
                });
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn move_connections_into_group(
        &mut self,
        source_ids: Vec<String>,
        parent: Option<String>,
        cx: &mut Context<Self>,
    ) {
        use nyaterm_core::connection_tree::{
            Destination, MoveRequest, NodeKind, catalog, plan_move,
        };
        let mut nodes = catalog(
            self.connection_state.groups(),
            self.connection_state.connections(),
        );
        let sources: Vec<_> = nodes
            .iter()
            .filter(|n| n.key.kind == NodeKind::Connection && source_ids.contains(&n.key.id))
            .map(|n| n.key.clone())
            .collect();
        let mut requests = Vec::new();
        for source in sources {
            let request = MoveRequest {
                source,
                destination: Destination {
                    parent: parent.clone(),
                    before: None,
                },
                expected: nodes.clone(),
            };
            let Ok(changes) = plan_move(&nodes, &request) else {
                return;
            };
            for change in changes {
                if let Some(node) = nodes.iter_mut().find(|n| n.key == change.key) {
                    *node = change;
                }
            }
            requests.push(request);
        }
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
                store.move_connection_trees(&requests)?;
                store.load_sessions()
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(sessions) => {
                        this.apply_loaded_sessions(sessions, cx);
                        if let Some(parent) = parent {
                            this.connection_state.expand_list_group(parent);
                        }
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("move connections failed: {error}")),
                }
                cx.notify();
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
            tags: Vec::new(),
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
            tree_order: None,
            sort_order,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_compatibility_mode: false,
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

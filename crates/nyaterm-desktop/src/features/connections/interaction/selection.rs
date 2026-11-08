use gpui::Context;
use nyaterm_core::{SavedConnection, uuid};
use nyaterm_store::StoreDomain;

use crate::features::NyaTermApp;

impl NyaTermApp {
    /// Tauri connection selection: plain click replaces, Ctrl/Cmd toggles, Shift ranges.
    pub(in crate::features) fn select_connection(
        &mut self,
        connection_id: String,
        additive: bool,
        range: bool,
        cx: &mut Context<Self>,
    ) {
        let visible_ids = self.connection_state.visible_connection_ids();
        let count = self.connection_state.select_list_connection(
            connection_id,
            &visible_ids,
            additive,
            range,
        );
        self.shell.set_status(if count == 0 {
            "connection selection cleared".to_string()
        } else {
            format!("selected {count} connection(s)")
        });
        cx.notify();
    }

    pub(in crate::features) fn clear_selected_connections(&mut self, cx: &mut Context<Self>) {
        self.connection_state.clear_list_selection();
        self.shell
            .set_status("connection selection cleared".to_string());
        cx.notify();
    }

    pub(in crate::features) fn copy_selected_connections(&mut self, cx: &mut Context<Self>) {
        let selected = self.connection_state.selected_connections();
        if selected.is_empty() {
            self.shell
                .set_status("select saved connections before copying".to_string());
            cx.notify();
            return;
        }

        self.submit_connection_copies(selected, cx);
    }

    pub(in crate::features) fn submit_connection_copies(
        &mut self,
        connections: Vec<SavedConnection>,
        cx: &mut Context<Self>,
    ) {
        self.submit_connection_copies_with_selection(connections, true, cx);
    }

    pub(in crate::features) fn submit_connection_copies_preserving_selection(
        &mut self,
        connections: Vec<SavedConnection>,
        cx: &mut Context<Self>,
    ) {
        self.submit_connection_copies_with_selection(connections, false, cx);
    }

    fn submit_connection_copies_with_selection(
        &mut self,
        connections: Vec<SavedConnection>,
        clear_selection: bool,
        cx: &mut Context<Self>,
    ) {
        let count = connections.len();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
                for connection in &connections {
                    let copy = duplicate_saved_connection(connection);
                    store.save_connection(&copy)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.apply_loaded_sessions(sessions, cx);
                    if clear_selection {
                        this.connection_state.clear_list_selection();
                    }
                    this.shell
                        .set_status(format!("copied {count} saved connection(s)"));
                    this.settings
                        .update_store_status("saved connections copied", true);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("copy saved connections failed: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }
}

fn duplicate_saved_connection(connection: &SavedConnection) -> SavedConnection {
    let mut copy = connection.clone();
    copy.id = uuid();
    copy.name = format!("{} (copy)", connection.name);
    copy.created_at_ms = None;
    copy.updated_at_ms = None;
    copy.last_used_at_ms = None;
    copy
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{
        AiExecutionProfile, ConnectionAuth, ConnectionType, SavedConnection,
        models::credentials::ConnectionPasswordSource,
    };

    use super::duplicate_saved_connection;

    fn connection_with_auth(auth: ConnectionAuth) -> SavedConnection {
        SavedConnection {
            id: "source-connection".to_string(),
            name: "Source".to_string(),
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
            tags: Vec::new(),
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(auth),
            network: None,
            post_login: None,
            recording: None,
            ssh_algorithms: None,
            ssh_compatibility_mode: false,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            asset: None,
            created_at_ms: Some(1),
            updated_at_ms: Some(2),
            last_used_at_ms: Some(3),
            extensions: Default::default(),
        }
    }

    #[test]
    fn duplicate_connection_preserves_direct_password() {
        let source = connection_with_auth(ConnectionAuth {
            mode: "password".to_string(),
            password_source: Some(ConnectionPasswordSource::Connection),
            password: Some("secret".to_string().into()),
            ..ConnectionAuth::default()
        });

        let copy = duplicate_saved_connection(&source);

        assert_ne!(copy.id, source.id);
        assert_eq!(copy.name, "Source (copy)");
        assert_eq!(copy.auth, source.auth);
        assert_eq!(copy.created_at_ms, None);
        assert_eq!(copy.updated_at_ms, None);
        assert_eq!(copy.last_used_at_ms, None);
    }

    #[test]
    fn duplicate_connection_preserves_saved_account_references() {
        let source = connection_with_auth(ConnectionAuth {
            mode: "password".to_string(),
            account_id: Some("account-1".to_string()),
            password_id: Some("legacy-password-1".to_string()),
            password_source: Some(ConnectionPasswordSource::Account),
            ..ConnectionAuth::default()
        });

        let copy = duplicate_saved_connection(&source);

        assert_eq!(copy.auth, source.auth);
    }

    #[test]
    fn duplicate_connection_preserves_locked_password() {
        let source = connection_with_auth(ConnectionAuth {
            mode: "password".to_string(),
            password_source: Some(ConnectionPasswordSource::Connection),
            password: Some("locked-ciphertext".to_string().into()),
            has_password: true,
            ..ConnectionAuth::default()
        });

        let copy = duplicate_saved_connection(&source);

        assert_eq!(copy.auth, source.auth);
    }
}

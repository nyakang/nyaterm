use gpui::Context;
use nyaterm_core::{SavedConnection, uuid};
use nyaterm_store::{StoreDomain, store_request};

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
        let count = connections.len();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                for connection in &connections {
                    let mut copy = connection.clone();
                    copy.id = uuid();
                    copy.name = format!("{} (copy)", connection.name);
                    copy.created_at_ms = None;
                    copy.updated_at_ms = None;
                    copy.last_used_at_ms = None;
                    if let Some(auth) = copy.auth.as_mut() {
                        auth.password = None;
                        auth.password_id = None;
                        auth.has_password = false;
                    }
                    store.save_connection(&copy)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    this.apply_loaded_sessions(sessions, cx);
                    this.connection_state.clear_list_selection();
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

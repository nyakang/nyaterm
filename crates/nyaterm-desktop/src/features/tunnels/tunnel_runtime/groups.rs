use rust_i18n::t;

use gpui::{Context, Window};
use nyaterm_store::StoreDomain;

use crate::features::NyaTermApp;
use crate::models::{NetworkGroupEditorState, NetworkTab};

impl NyaTermApp {
    pub(in crate::features) fn open_network_group_editor(
        &mut self,
        tab: NetworkTab,
        group_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing = group_id.is_some();
        let name = match (tab, group_id.as_deref()) {
            (NetworkTab::Tunnels, Some(id)) => self
                .tunnel_state
                .tunnel_groups()
                .iter()
                .find(|group| group.id == id)
                .map(|group| group.name.clone()),
            (NetworkTab::Proxies, Some(id)) => self
                .tunnel_state
                .proxy_groups()
                .iter()
                .find(|group| group.id == id)
                .map(|group| group.name.clone()),
            (_, None) => Some(String::new()),
        };
        let Some(name) = name else {
            self.shell
                .set_status("network group is no longer available".to_string());
            cx.notify();
            return;
        };

        self.connection_state
            .begin_network_group_edit(NetworkGroupEditorState {
                tab,
                id: group_id,
                name,
                error: None,
            });
        // The box owns its text, so it has to be dropped for the next group to
        // seed from its own name.
        self.forget_text_inputs("network.group-editor.");
        self.shell
            .set_status("network group editor opened".to_string());
        cx.notify();
        self.open_form_dialog(
            (
                if editing {
                    t!("network.renameGroup").to_string()
                } else {
                    t!("network.newGroup").to_string()
                },
                420.,
                t!("common.save").to_string(),
                |app, _, cx| app.network_group_editor_dialog_content(cx),
                |app, _, cx| {
                    app.save_network_group_editor(cx);
                    let saved = app.connection_state.active_network_group_editor().is_none();
                    if saved {
                        app.forget_text_inputs("network.group-editor.");
                    }
                    saved
                },
                |app, cx| app.close_network_group_editor(cx),
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn close_network_group_editor(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_network_group_editor();
        self.forget_text_inputs("network.group-editor.");
        self.shell
            .set_status("network group editor closed".to_string());
        cx.notify();
    }

    /// Apply an edit from the group dialog's name box.
    pub(in crate::features) fn apply_network_group_editor_name(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_network_group_editor_name(text) {
            cx.notify();
        }
    }

    pub(in crate::features) fn save_network_group_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.connection_state.active_network_group_editor() else {
            self.shell
                .set_status("no network group editor is active".to_string());
            cx.notify();
            return;
        };
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            self.connection_state
                .set_network_group_editor_error("Group name is required".to_string());
            cx.notify();
            return;
        }

        match editor.tab {
            NetworkTab::Tunnels => self.save_tunnel_group(editor.id, name, cx),
            NetworkTab::Proxies => self.save_proxy_group(editor.id, name, cx),
        }
    }

    pub(in crate::features) fn save_tunnel_group(
        &mut self,
        group_id: Option<String>,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(groups) = self
            .tunnel_state
            .tunnel_groups_with_upsert(group_id.as_deref(), name.clone())
        else {
            self.shell
                .set_status("tunnel group is no longer available".to_string());
            cx.notify();
            return;
        };

        let persisted = groups.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_tunnel_groups(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_tunnel_groups(groups);
                        this.connection_state.close_network_group_editor();
                        this.shell
                            .set_status(format!("tunnel group '{name}' saved"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to save tunnel group: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn save_proxy_group(
        &mut self,
        group_id: Option<String>,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(groups) = self
            .tunnel_state
            .proxy_groups_with_upsert(group_id.as_deref(), name.clone())
        else {
            self.shell
                .set_status("proxy group is no longer available".to_string());
            cx.notify();
            return;
        };

        let persisted = groups.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_proxy_groups(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_proxy_groups(groups);
                        this.connection_state.close_network_group_editor();
                        this.shell.set_status(format!("proxy group '{name}' saved"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to save proxy group: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn open_network_group_delete_confirm(
        &mut self,
        tab: NetworkTab,
        id: String,
        label: String,
        item_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell
            .set_status("network group delete confirmation opened".to_string());
        let description = t!(
            "network.deleteGroupConfirm",
            name = label,
            count = item_count
        )
        .to_string();
        self.open_confirm_dialog(
            (
                t!("network.deleteGroup").to_string(),
                description,
                t!("common.delete").to_string(),
                true,
                move |app, _, cx| {
                    match tab {
                        NetworkTab::Tunnels => {
                            app.delete_tunnel_group(id.clone(), label.clone(), cx)
                        }
                        NetworkTab::Proxies => {
                            app.delete_proxy_group(id.clone(), label.clone(), cx)
                        }
                    }
                    true
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn delete_tunnel_group(
        &mut self,
        group_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.tunnel_state.cancel_group_batch(&group_id);
        let removal = self.tunnel_state.without_tunnel_group(&group_id);
        let groups = removal.groups().to_vec();
        let tunnels = removal.tunnels().to_vec();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_tunnel_groups(&groups)?;
                store.replace_tunnels(&tunnels)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        let deleted_tunnel_ids =
                            this.tunnel_state.commit_tunnel_group_removal(removal);
                        this.tunnel_state.cancel_group_batch(&group_id);
                        // Configuration is gone, so the forwards must go too.
                        // Close in the background; a tunnel still opening is
                        // closed by `apply_tunnel_event` once its open lands.
                        let close_failures = deleted_tunnel_ids
                            .iter()
                            .filter(|id| {
                                this.tunnel_state.is_open(id) && !this.tunnel_state.is_pending(id)
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                            .into_iter()
                            .filter_map(|id| this.submit_tunnel_close(id).err())
                            .collect::<Vec<_>>();
                        this.connection_state.remove_network_group_references(
                            NetworkTab::Tunnels,
                            &group_id,
                            &deleted_tunnel_ids,
                        );
                        if close_failures.is_empty() {
                            this.shell
                                .set_status(format!("tunnel group '{label}' deleted"));
                        } else {
                            this.shell.set_status(format!(
                                "tunnel group '{label}' deleted; failed to close tunnels: {}",
                                close_failures.join("; ")
                            ));
                        }
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to delete tunnel group: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn delete_proxy_group(
        &mut self,
        group_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let removal = self.tunnel_state.without_proxy_group(&group_id);
        let groups = removal.groups().to_vec();
        let proxies = removal.proxies().to_vec();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_proxy_groups(&groups)?;
                store.replace_proxies(&proxies)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        let deleted_proxy_ids =
                            this.tunnel_state.commit_proxy_group_removal(removal);
                        this.connection_state.remove_network_group_references(
                            NetworkTab::Proxies,
                            &group_id,
                            &deleted_proxy_ids,
                        );
                        this.shell
                            .set_status(format!("proxy group '{label}' deleted"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to delete proxy group: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }
}

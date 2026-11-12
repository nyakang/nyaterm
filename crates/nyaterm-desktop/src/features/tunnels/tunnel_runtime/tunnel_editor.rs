use rust_i18n::t;

use gpui::{Context, Window};
use nyaterm_core::{TunnelConfig, uuid};
use nyaterm_store::StoreDomain;

use super::helpers::{network_section_key, parse_port};
use crate::features::NyaTermApp;
use crate::features::view_widgets::dialogs::FormSaveCompletion;
use crate::models::{NetworkTab, NetworkTunnelEditorField, NetworkTunnelEditorState};

impl NyaTermApp {
    pub(in crate::features) fn set_network_tab(&mut self, tab: NetworkTab, cx: &mut Context<Self>) {
        self.connection_state.set_network_tab(tab);
        self.shell
            .set_status(format!("network tab set to {}", tab.label()));
        cx.notify();
    }

    pub(in crate::features) fn toggle_network_section(
        &mut self,
        tab: NetworkTab,
        section_id: String,
        cx: &mut Context<Self>,
    ) {
        let key = network_section_key(tab, &section_id);
        if self.connection_state.toggle_network_section(key) {
            self.shell
                .set_status(format!("expanded {} group", tab.label()));
        } else {
            self.shell
                .set_status(format!("collapsed {} group", tab.label()));
        }
        cx.notify();
    }

    pub(in crate::features) fn open_network_tunnel_editor(
        &mut self,
        tunnel_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing = tunnel_id.is_some();
        let tunnel = match tunnel_id.as_deref() {
            Some(id) => self
                .tunnel_state
                .tunnels()
                .iter()
                .find(|tunnel| tunnel.id == id)
                .cloned(),
            None => Some(TunnelConfig::default()),
        };
        let Some(tunnel) = tunnel else {
            self.shell
                .set_status("tunnel profile is no longer available".to_string());
            cx.notify();
            return;
        };

        self.connection_state
            .begin_network_tunnel_edit(NetworkTunnelEditorState {
                id: tunnel_id,
                is_open: tunnel.is_open,
                name: tunnel.name,
                tunnel_type: match tunnel.tunnel_type.as_str() {
                    "remote" | "dynamic" => tunnel.tunnel_type,
                    _ => "local".to_string(),
                },
                connection_id: tunnel.connection_id,
                listen_port: if tunnel.listen_port == 0 {
                    String::new()
                } else {
                    tunnel.listen_port.to_string()
                },
                target_host: if tunnel.target_host.trim().is_empty() {
                    "127.0.0.1".to_string()
                } else {
                    tunnel.target_host
                },
                target_port: if tunnel.target_port == 0 {
                    String::new()
                } else {
                    tunnel.target_port.to_string()
                },
                auto_open: tunnel.auto_open,
                bind_localhost: tunnel.bind_localhost,
                group_id: tunnel.group_id,
                focused_field: NetworkTunnelEditorField::Name,
                error: None,
            });
        // The dialog's boxes own their text, so they have to be dropped for the
        // next tunnel to seed from its own values.
        self.forget_text_inputs("network.tunnel-editor.");
        self.shell.set_status("tunnel editor opened".to_string());
        cx.notify();
        self.open_async_form_dialog(
            (
                if editing {
                    t!("network.editTunnel").to_string()
                } else {
                    t!("network.newTunnel").to_string()
                },
                640.,
                t!("common.save").to_string(),
                |app, _, cx| app.network_tunnel_editor_dialog_content(cx),
                |app, completion, cx| app.save_network_tunnel_editor(completion, cx),
                |app, cx| app.close_network_tunnel_editor(cx),
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn close_network_tunnel_editor(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_network_tunnel_editor();
        self.forget_text_inputs("network.tunnel-editor.");
        self.shell.set_status("tunnel editor closed".to_string());
        cx.notify();
    }

    /// Apply an edit from one of the tunnel dialog's inputs.
    pub(in crate::features) fn apply_network_tunnel_editor_input(
        &mut self,
        field: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let field = match field {
            "name" => NetworkTunnelEditorField::Name,
            "listen-port" => NetworkTunnelEditorField::ListenPort,
            "target-host" => NetworkTunnelEditorField::TargetHost,
            "target-port" => NetworkTunnelEditorField::TargetPort,
            _ => return,
        };
        if self
            .connection_state
            .set_network_tunnel_editor_field(field, text)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_network_tunnel_type(
        &mut self,
        tunnel_type: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(tunnel_type) = self.connection_state.set_network_tunnel_type(tunnel_type) {
            self.shell
                .set_status(format!("tunnel type set to {tunnel_type}"));
        }
        cx.notify();
    }

    pub(in crate::features) fn set_network_tunnel_connection(
        &mut self,
        connection_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_network_tunnel_connection(connection_id)
        {
            self.shell
                .set_status("tunnel SSH connection changed".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn set_network_tunnel_group(
        &mut self,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let group_id = group_id.filter(|id| {
            self.tunnel_state
                .tunnel_groups()
                .iter()
                .any(|group| group.id == *id)
        });
        if self.connection_state.set_network_tunnel_group(group_id) {
            self.shell.set_status("tunnel group changed".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn set_network_tunnel_bind_localhost(
        &mut self,
        bind_localhost: bool,
        cx: &mut Context<Self>,
    ) {
        self.connection_state
            .set_network_tunnel_bind_localhost(bind_localhost);
        cx.notify();
    }

    pub(in crate::features) fn toggle_network_tunnel_auto_open(&mut self, cx: &mut Context<Self>) {
        if let Some(auto_open) = self.connection_state.toggle_network_tunnel_auto_open() {
            self.shell.set_status(
                if auto_open {
                    "tunnel auto-open enabled"
                } else {
                    "tunnel auto-open disabled"
                }
                .to_string(),
            );
        }
        cx.notify();
    }

    pub(in crate::features) fn save_network_tunnel_editor(
        &mut self,
        completion: FormSaveCompletion,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.connection_state.active_network_tunnel_editor() else {
            self.shell
                .set_status("no tunnel editor is active".to_string());
            cx.notify();
            return false;
        };

        let name = editor.name.trim().to_string();
        if name.is_empty() {
            self.set_network_tunnel_editor_error("Tunnel name is required", cx);
            return false;
        }
        let Some(connection_id) = editor.connection_id.clone() else {
            self.set_network_tunnel_editor_error("SSH connection is required", cx);
            return false;
        };
        if !self
            .connection_state
            .connections()
            .iter()
            .any(|connection| connection.id == connection_id)
        {
            self.set_network_tunnel_editor_error("Selected SSH connection is missing", cx);
            return false;
        }
        let Some(listen_port) = parse_port(&editor.listen_port) else {
            self.set_network_tunnel_editor_error("Listen port must be 1-65535", cx);
            return false;
        };
        let is_dynamic = editor.is_dynamic();
        let target_host = if is_dynamic {
            "127.0.0.1".to_string()
        } else {
            let value = editor.target_host.trim().to_string();
            if value.is_empty() {
                self.set_network_tunnel_editor_error("Target host is required", cx);
                return false;
            }
            value
        };
        let target_port = if is_dynamic {
            0
        } else {
            let Some(port) = parse_port(&editor.target_port) else {
                self.set_network_tunnel_editor_error("Target port must be 1-65535", cx);
                return false;
            };
            port
        };
        let group_id = editor
            .group_id
            .filter(|id| self.tunnel_state.has_tunnel_group(id));

        let id = editor.id.clone().unwrap_or_else(uuid);
        let tunnel = TunnelConfig {
            id: id.clone(),
            name: name.clone(),
            tunnel_type: editor.tunnel_type,
            connection_id: Some(connection_id),
            listen_port,
            target_host,
            target_port,
            is_open: editor.is_open,
            auto_open: editor.auto_open,
            bind_localhost: editor.bind_localhost,
            group_id,
        };
        let next_tunnels = self.tunnel_state.tunnels_with_upsert(tunnel);

        let persisted = next_tunnels.clone();
        let submitted = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_tunnels(&persisted)
            }),
            move |this, event, cx| {
                let saved = event.outcome.is_ok();
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_tunnels(next_tunnels);
                        this.connection_state.close_network_tunnel_editor();
                        this.forget_text_inputs("network.tunnel-editor.");
                        this.shell.set_status(format!("tunnel '{name}' saved"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to save tunnel: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                        this.connection_state
                            .set_network_tunnel_editor_error(this.shell.status().to_string());
                    }
                }
                completion(saved, cx);
                cx.notify();
            },
            cx,
        );
        if !submitted {
            self.connection_state
                .set_network_tunnel_editor_error(self.shell.status().to_string());
        }
        submitted
    }

    pub(in crate::features) fn set_network_tunnel_editor_error(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let error = error.into();
        self.connection_state
            .set_network_tunnel_editor_error(error.clone());
        self.shell.set_status(error);
        cx.notify();
    }
}

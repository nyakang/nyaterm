use rust_i18n::t;

use gpui::{Context, Window};
use nyaterm_core::{ProxyConfig, uuid};
use nyaterm_store::StoreDomain;

use super::helpers::parse_port;
use crate::features::NyaTermApp;
use crate::features::view_widgets::dialogs::FormSaveCompletion;
use crate::models::{NetworkProxyEditorField, NetworkProxyEditorState};

impl NyaTermApp {
    pub(in crate::features) fn open_network_proxy_editor(
        &mut self,
        proxy_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing = proxy_id.is_some();
        let proxy = match proxy_id.as_deref() {
            Some(id) => self
                .tunnel_state
                .proxies()
                .iter()
                .find(|proxy| proxy.id == id)
                .cloned(),
            None => Some(ProxyConfig::default()),
        };
        let Some(proxy) = proxy else {
            self.shell
                .set_status("proxy profile is no longer available".to_string());
            cx.notify();
            return;
        };

        self.connection_state
            .begin_network_proxy_edit(NetworkProxyEditorState {
                id: proxy_id,
                name: proxy.name,
                protocol: match proxy.protocol.as_str() {
                    "http" | "proxycommand" => proxy.protocol,
                    _ => "socks5".to_string(),
                },
                host: if proxy.host.trim().is_empty() {
                    "127.0.0.1".to_string()
                } else {
                    proxy.host
                },
                port: if proxy.port == 0 {
                    String::new()
                } else {
                    proxy.port.to_string()
                },
                command: proxy.command.unwrap_or_default(),
                username: proxy.username.unwrap_or_default(),
                password: nyaterm_core::SecretString::default(),
                existing_password: proxy.password,
                password_id: proxy.password_id,
                group_id: proxy.group_id,
                focused_field: NetworkProxyEditorField::Name,
                error: None,
            });
        // The dialog's boxes own their text, so they have to be dropped for
        // the next proxy to seed from its own values.
        self.forget_text_inputs("network.proxy-editor.");
        self.shell.set_status("proxy editor opened".to_string());
        cx.notify();
        self.open_async_form_dialog(
            (
                if editing {
                    t!("network.editProxy").to_string()
                } else {
                    t!("network.newProxy").to_string()
                },
                520.,
                t!("common.save").to_string(),
                |app, _, cx| app.network_proxy_editor_dialog_content(cx),
                |app, completion, cx| app.save_network_proxy_editor(completion, cx),
                |app, cx| app.close_network_proxy_editor(cx),
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn close_network_proxy_editor(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_network_proxy_editor();
        self.forget_text_inputs("network.proxy-editor.");
        self.shell.set_status("proxy editor closed".to_string());
        cx.notify();
    }

    /// Apply an edit from one of the proxy dialog's inputs.
    pub(in crate::features) fn apply_network_proxy_editor_input(
        &mut self,
        field: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let field = match field {
            "name" => NetworkProxyEditorField::Name,
            "host" => NetworkProxyEditorField::Host,
            "port" => NetworkProxyEditorField::Port,
            "command" => NetworkProxyEditorField::Command,
            "username" => NetworkProxyEditorField::Username,
            "password" => NetworkProxyEditorField::Password,
            _ => return,
        };
        if self
            .connection_state
            .set_network_proxy_editor_field(field, text)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_network_proxy_protocol(
        &mut self,
        protocol: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(protocol) = self.connection_state.set_network_proxy_protocol(protocol) {
            self.shell
                .set_status(format!("proxy protocol set to {protocol}"));
        }
        cx.notify();
    }

    pub(in crate::features) fn set_network_proxy_group(
        &mut self,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let group_id = group_id.filter(|id| {
            self.tunnel_state
                .proxy_groups()
                .iter()
                .any(|group| group.id == *id)
        });
        if self.connection_state.set_network_proxy_group(group_id) {
            self.shell.set_status("proxy group changed".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn save_network_proxy_editor(
        &mut self,
        completion: FormSaveCompletion,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.connection_state.active_network_proxy_editor() else {
            self.shell
                .set_status("no proxy editor is active".to_string());
            cx.notify();
            return false;
        };

        let name = editor.name.trim().to_string();
        if name.is_empty() {
            self.set_network_proxy_editor_error("Proxy name is required", cx);
            return false;
        }
        let is_command = editor.is_proxy_command();
        let command = if is_command {
            let command = editor.command.trim().to_string();
            if command.is_empty() {
                self.set_network_proxy_editor_error("ProxyCommand is required", cx);
                return false;
            }
            Some(command)
        } else {
            None
        };
        let host = if is_command {
            editor.host.trim().to_string()
        } else {
            let host = editor.host.trim().to_string();
            if host.is_empty() {
                self.set_network_proxy_editor_error("Proxy host is required", cx);
                return false;
            }
            host
        };
        let port = if is_command {
            editor.port.trim().parse::<u16>().unwrap_or(0)
        } else {
            let Some(port) = parse_port(&editor.port) else {
                self.set_network_proxy_editor_error("Proxy port must be 1-65535", cx);
                return false;
            };
            port
        };
        let username = if editor.username.trim().is_empty() {
            None
        } else {
            Some(editor.username.trim().to_string())
        };
        let password = if editor.password.is_empty() {
            editor.existing_password
        } else {
            Some(editor.password)
        };
        let password_id = if password.is_some() {
            None
        } else {
            editor.password_id
        };
        let group_id = editor
            .group_id
            .filter(|id| self.tunnel_state.has_proxy_group(id));

        let id = editor.id.clone().unwrap_or_else(uuid);
        let proxy = ProxyConfig {
            id: id.clone(),
            name: name.clone(),
            protocol: editor.protocol,
            host,
            port,
            command,
            username,
            password,
            password_id,
            group_id,
        };
        let next_proxies = self.tunnel_state.proxies_with_upsert(proxy);

        let persisted = next_proxies.clone();
        let submitted = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Tunnels, move |store| {
                store.replace_proxies(&persisted)
            }),
            move |this, event, cx| {
                let saved = event.outcome.is_ok();
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_proxies(next_proxies);
                        this.connection_state.close_network_proxy_editor();
                        this.forget_text_inputs("network.proxy-editor.");
                        this.shell.set_status(format!("proxy '{name}' saved"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to save proxy: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                        this.connection_state
                            .set_network_proxy_editor_error(this.shell.status().to_string());
                    }
                }
                completion(saved, cx);
                cx.notify();
            },
            cx,
        );
        if !submitted {
            self.connection_state
                .set_network_proxy_editor_error(self.shell.status().to_string());
        }
        submitted
    }

    pub(in crate::features) fn set_network_proxy_editor_error(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let error = error.into();
        self.connection_state
            .set_network_proxy_editor_error(error.clone());
        self.shell.set_status(error);
        cx.notify();
    }
}

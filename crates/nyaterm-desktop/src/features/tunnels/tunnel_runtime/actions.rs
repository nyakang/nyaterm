use rust_i18n::t;

use futures::StreamExt as _;
use gpui::{Context, Window};
use nyaterm_core::TunnelConfig;
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_transport::{SshTunnelConfig, SshTunnelMode};

use super::helpers::network_group_label;
use crate::features::{
    NyaTermApp, formatting::tunnel_mode, formatting::tunnel_name, runtime_jobs::TunnelJobOutput,
    runtime_jobs::TunnelJobResult,
};
use crate::models::NetworkTab;

impl NyaTermApp {
    pub(in crate::features) fn open_network_move_picker(
        &mut self,
        tab: NetworkTab,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.toggle_network_move_picker(tab, id) {
            self.shell
                .set_status(format!("choose {} group", tab.label()));
        } else {
            self.shell
                .set_status("network move menu closed".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn move_tunnel_to_group(
        &mut self,
        tunnel_id: String,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let group_id = group_id.filter(|id| self.tunnel_state.has_tunnel_group(id));
        let label = network_group_label(group_id.as_deref(), self.tunnel_state.tunnel_groups());
        self.move_tunnel_to_group_internal(tunnel_id, group_id, label, cx);
    }

    pub(in crate::features) fn move_tunnel_to_group_internal(
        &mut self,
        tunnel_id: String,
        group_id: Option<String>,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(next_tunnels) = self
            .tunnel_state
            .tunnels_moved_to_group(&tunnel_id, group_id)
        else {
            self.shell
                .set_status("tunnel profile is no longer available".to_string());
            cx.notify();
            return;
        };
        let persisted = next_tunnels.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Tunnels, move |store| {
                store.replace_tunnels(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_tunnels(next_tunnels);
                        this.connection_state.close_network_move_picker();
                        this.shell.set_status(format!("tunnel moved to {label}"));
                        this.settings
                            .update_store_status("tunnel group saved", true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to move tunnel: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn move_proxy_to_group(
        &mut self,
        proxy_id: String,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let group_id = group_id.filter(|id| self.tunnel_state.has_proxy_group(id));
        let label = network_group_label(group_id.as_deref(), self.tunnel_state.proxy_groups());
        self.move_proxy_to_group_internal(proxy_id, group_id, label, cx);
    }

    pub(in crate::features) fn move_proxy_to_group_internal(
        &mut self,
        proxy_id: String,
        group_id: Option<String>,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(next_proxies) = self
            .tunnel_state
            .proxies_moved_to_group(&proxy_id, group_id)
        else {
            self.shell
                .set_status("proxy profile is no longer available".to_string());
            cx.notify();
            return;
        };
        let persisted = next_proxies.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Tunnels, move |store| {
                store.replace_proxies(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_proxies(next_proxies);
                        this.connection_state.close_network_move_picker();
                        this.shell.set_status(format!("proxy moved to {label}"));
                        this.settings.update_store_status("proxy group saved", true);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to move proxy: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn open_network_delete_confirm(
        &mut self,
        tab: NetworkTab,
        id: String,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell
            .set_status("network delete confirmation opened".to_string());
        let type_label = match tab {
            NetworkTab::Tunnels => t!("network.tunnelConfig"),
            NetworkTab::Proxies => t!("network.proxyConfig"),
        };
        self.open_confirm_dialog(
            (
                format!("{} {type_label}", t!("common.delete")),
                t!("common.deletingConfirm", name = label).to_string(),
                t!("common.delete").to_string(),
                true,
                move |app, _, cx| {
                    match tab {
                        NetworkTab::Tunnels => {
                            app.delete_tunnel_profile(id.clone(), label.clone(), cx)
                        }
                        NetworkTab::Proxies => {
                            app.delete_proxy_profile(id.clone(), label.clone(), cx)
                        }
                    }
                    true
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn delete_tunnel_profile(
        &mut self,
        tunnel_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        if self.tunnel_state.is_open(&tunnel_id)
            && let Err(error) = self.tunnel_state.close_now(&tunnel_id)
        {
            self.shell
                .set_status(format!("failed to close tunnel before delete: {error}"));
            cx.notify();
            return;
        }

        let (next_tunnels, deleted) = self.tunnel_state.tunnels_without(&tunnel_id);
        let persisted = next_tunnels.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Tunnels, move |store| {
                store.replace_tunnels(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_tunnels(next_tunnels);
                        this.tunnel_state.finish_job(&tunnel_id);
                        this.connection_state
                            .remove_network_item_references(NetworkTab::Tunnels, &tunnel_id);
                        this.shell.set_status(if deleted {
                            format!("tunnel '{label}' deleted")
                        } else {
                            format!("tunnel '{label}' was already deleted")
                        });
                        this.settings
                            .update_store_status(this.shell.status().to_string(), deleted);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to delete tunnel: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn delete_proxy_profile(
        &mut self,
        proxy_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let (next_proxies, deleted) = self.tunnel_state.proxies_without(&proxy_id);
        let persisted = next_proxies.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Tunnels, move |store| {
                store.replace_proxies(&persisted)
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.tunnel_state.commit_proxies(next_proxies);
                        this.connection_state
                            .remove_network_item_references(NetworkTab::Proxies, &proxy_id);
                        this.shell.set_status(if deleted {
                            format!("proxy '{label}' deleted")
                        } else {
                            format!("proxy '{label}' was already deleted")
                        });
                        this.settings
                            .update_store_status(this.shell.status().to_string(), deleted);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("failed to delete proxy: {error}"));
                        this.settings
                            .update_store_status(this.shell.status().to_string(), false);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn start_tunnel_job(
        &mut self,
        tunnel: TunnelConfig,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_tunnel_without_window(tunnel, cx);
    }

    pub(in crate::features) fn start_auto_tunnels_for_connection(
        &mut self,
        connection_id: &str,
        cx: &mut Context<Self>,
    ) {
        let tunnels = self
            .tunnel_state
            .tunnels()
            .iter()
            .filter(|tunnel| {
                tunnel.auto_open && tunnel.connection_id.as_deref() == Some(connection_id)
            })
            .filter(|tunnel| {
                !self.tunnel_state.is_open(&tunnel.id) && !self.tunnel_state.is_pending(&tunnel.id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for tunnel in tunnels {
            self.start_tunnel_without_window(tunnel, cx);
        }
    }

    fn start_tunnel_without_window(&mut self, tunnel: TunnelConfig, cx: &mut Context<Self>) {
        if self.tunnel_state.is_pending(&tunnel.id) {
            self.shell.set_status(format!(
                "tunnel {} is already pending",
                tunnel_name(&tunnel)
            ));
            cx.notify();
            return;
        }
        if self.tunnel_state.is_open(&tunnel.id) {
            self.shell
                .set_status(format!("tunnel {} is already open", tunnel_name(&tunnel)));
            cx.notify();
            return;
        }

        let Some(connection_id) = tunnel.connection_id.as_deref() else {
            self.shell.set_status(format!(
                "tunnel {} has no SSH connection",
                tunnel_name(&tunnel)
            ));
            cx.notify();
            return;
        };
        let Some(connection) = self
            .connection_state
            .connections()
            .iter()
            .find(|connection| connection.id == connection_id)
            .cloned()
        else {
            self.shell.set_status(format!(
                "tunnel {} references missing connection {}",
                tunnel_name(&tunnel),
                connection_id
            ));
            cx.notify();
            return;
        };
        let mode = match tunnel_mode(&tunnel) {
            Some(mode) => mode,
            None => {
                self.shell.set_status(format!(
                    "tunnel {} mode '{}' is not native yet",
                    tunnel_name(&tunnel),
                    tunnel.tunnel_type
                ));
                cx.notify();
                return;
            }
        };
        let build_context = self.ssh_session_config_build_context();
        if !self.tunnel_state.begin_job(tunnel.id.clone()) {
            self.shell.set_status(format!(
                "tunnel {} is already pending",
                tunnel_name(&tunnel)
            ));
            cx.notify();
            return;
        }
        self.shell
            .set_status(format!("opening tunnel {}", tunnel_name(&tunnel)));
        let tunnel_manager = self.tunnel_state.manager_for_job();
        let tunnel_tx = self.tunnel_state.job_sender();
        let rejected_tx = tunnel_tx.clone();
        let rejected_tunnel_id = tunnel.id.clone();
        if let Err(error) = self.blocking_jobs.submit_detached("tunnel-open", move |_| {
            let result = build_context
                .build_config(&connection)
                .and_then(|ssh_config| {
                    let config = SshTunnelConfig {
                        id: tunnel.id.clone(),
                        ssh_config,
                        mode,
                        bind_host: if tunnel.bind_localhost {
                            "127.0.0.1".to_string()
                        } else {
                            "0.0.0.0".to_string()
                        },
                        listen_port: tunnel.listen_port,
                        target_host: matches!(mode, SshTunnelMode::Local | SshTunnelMode::Remote)
                            .then_some(tunnel.target_host.clone()),
                        target_port: matches!(mode, SshTunnelMode::Local | SshTunnelMode::Remote)
                            .then_some(tunnel.target_port),
                    };

                    tunnel_manager
                        .open(config)
                        .map(TunnelJobOutput::Opened)
                        .map_err(|error| error.to_string())
                });
            let _ = tunnel_tx.unbounded_send(TunnelJobResult {
                tunnel_id: tunnel.id,
                result,
            });
        }) {
            let _ = rejected_tx.unbounded_send(TunnelJobResult {
                tunnel_id: rejected_tunnel_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn close_tunnel_job(
        &mut self,
        tunnel_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.tunnel_state.is_pending(&tunnel_id) {
            self.shell
                .set_status(format!("tunnel {tunnel_id} is already pending"));
            cx.notify();
            return;
        }
        if !self.tunnel_state.is_open(&tunnel_id) {
            self.shell
                .set_status(format!("tunnel {tunnel_id} is not open"));
            cx.notify();
            return;
        }

        if !self.tunnel_state.begin_job(tunnel_id.clone()) {
            self.shell
                .set_status(format!("tunnel {tunnel_id} is already pending"));
            cx.notify();
            return;
        }
        self.shell.set_status(format!("closing tunnel {tunnel_id}"));
        let tunnel_manager = self.tunnel_state.manager_for_job();
        let tunnel_tx = self.tunnel_state.job_sender();
        let rejected_tx = tunnel_tx.clone();
        let rejected_tunnel_id = tunnel_id.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("tunnel-close", move |_| {
                let result = tunnel_manager
                    .close(&tunnel_id)
                    .map(|_| TunnelJobOutput::Closed)
                    .map_err(|error| error.to_string());
                let _ = tunnel_tx.unbounded_send(TunnelJobResult { tunnel_id, result });
            })
        {
            let _ = rejected_tx.unbounded_send(TunnelJobResult {
                tunnel_id: rejected_tunnel_id,
                result: Err(error.to_string()),
            });
        }
        cx.notify();
    }

    /// Deliver tunnel open/close results as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `try_recv_job`, which meant a result waited for the next tick and forced
    /// `runtime_quiet_tick_allowed` to carry a `tunnel_state` term to keep that
    /// wait short.
    pub(in crate::features) fn start_tunnel_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.tunnel_state.take_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.apply_tunnel_event(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_tunnel_event(&mut self, event: TunnelJobResult, cx: &mut Context<Self>) {
        self.tunnel_state.finish_job(&event.tunnel_id);
        match event.result {
            Ok(TunnelJobOutput::Opened(info)) => {
                self.shell.set_status(format!(
                    "tunnel {} open on {}:{}",
                    event.tunnel_id, info.bind_host, info.listen_port
                ));
            }
            Ok(TunnelJobOutput::Closed) => {
                self.shell
                    .set_status(format!("tunnel {} closed", event.tunnel_id));
            }
            Err(error) => {
                self.notify_background_operation(
                    "tunnel-failed",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    format!("{}: {error}", event.tunnel_id),
                    cx,
                );
                self.shell
                    .set_status(format!("tunnel {} failed: {error}", event.tunnel_id));
            }
        }
    }
}

//! Fill in a saved connection's icon from what the remote host reports.
//!
//! Reuses remote-stats snapshots, requesting a bounded probe when no monitor
//! consumes them. Only ever fills in a blank or refreshes a previous
//! auto-detection — a hand-picked icon clears the flag and is never touched.

use crate::features::{NyaTermApp, icons::infer_connection_icon_key_from_remote_system};
use gpui::Context;
use nyaterm_core::ConnectionType;
use nyaterm_remote_ops::stats::SystemInfo;
use nyaterm_store::StoreDomain;

impl NyaTermApp {
    pub(in crate::features) fn connection_icon_probe_needed(&self) -> bool {
        if !self
            .session
            .active_ssh_config()
            .is_some_and(|config| config.remote_stats_enabled())
        {
            return false;
        }
        let Some(session_id) = self.session.active_id() else {
            return false;
        };
        let Some(metadata) = self.session.metadata(session_id) else {
            return false;
        };
        if metadata.disconnected || !self.remote_ops.icon_probe_needed(session_id) {
            return false;
        }
        let Some(connection_id) = metadata.source_connection_id.as_deref() else {
            return false;
        };
        self.connection_state
            .connections()
            .iter()
            .any(|connection| {
                connection.id == connection_id
                    && matches!(connection.config, ConnectionType::Ssh { .. })
                    && connection.ssh_profile != nyaterm_core::SshProfile::NetworkDevice
                    && connection.icon_auto_detect_enabled()
            })
    }

    /// A bounded consumer of the existing stats service, sharing its in-flight
    /// request with visible monitoring without keeping periodic collection alive.
    pub(in crate::features) fn refresh_connection_icon_if_due(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.connection_icon_probe_needed() {
            return false;
        }
        let Some(session_id) = self.session.active_id().map(str::to_owned) else {
            return false;
        };
        if let Some(stats) = self.remote_ops.take_icon_probe_snapshot(&session_id) {
            self.apply_auto_detected_connection_icon(&session_id, &stats.system, cx);
            self.sync_remote_panel_demand(cx);
            return true;
        }
        if self.remote_ops.stats_is_pending() || !self.remote_ops.icon_probe_due(&session_id) {
            return false;
        }
        self.refresh_stats_auto(cx);
        true
    }

    pub(in crate::features) fn apply_auto_detected_connection_icon(
        &mut self,
        session_id: &str,
        system: &SystemInfo,
        cx: &mut Context<Self>,
    ) {
        let Some(connection_id) = self
            .session
            .metadata(session_id)
            .and_then(|metadata| metadata.source_connection_id.as_deref())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
        else {
            return;
        };

        let Some(connection) = self
            .connection_state
            .connections()
            .iter()
            .find(|connection| connection.id == connection_id)
        else {
            return;
        };

        // Only SSH sessions report a remote system worth reading.
        if !matches!(connection.config, ConnectionType::Ssh { .. })
            || connection.ssh_profile == nyaterm_core::SshProfile::NetworkDevice
            || !connection.icon_auto_detect_enabled()
        {
            return;
        }

        let Some(icon_key) = infer_connection_icon_key_from_remote_system(&system.os, &system.arch)
        else {
            return;
        };
        if connection.icon.as_deref() == Some(icon_key) {
            return;
        }

        let icon_key = icon_key.to_string();
        let persisted_icon = icon_key.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_request(StoreDomain::Connections, move |store| {
                if !store.update_auto_detected_connection_icon(&connection_id, &persisted_icon)? {
                    return Ok(None);
                }
                store.get_connection(&connection_id)
            }),
            move |this, event, cx| match event.outcome {
                Ok(Some(connection)) => {
                    this.shell
                        .set_status(format!("detected {icon_key} icon for {}", connection.name));
                    this.connection_state.update_connection(connection);
                    cx.notify();
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        target: "nyaterm::connections",
                        category = error.category(),
                        "failed to persist auto-detected connection icon"
                    );
                }
            },
            cx,
        );
    }
}

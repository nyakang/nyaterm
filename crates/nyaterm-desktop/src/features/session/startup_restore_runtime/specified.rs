use gpui::{Context, Window};
use nyaterm_core::RestorableOpenTab;
use nyaterm_core::connection_tree::catalog;
use nyaterm_core::models::connection::ConnectionType;
use nyaterm_core::models::startup::{plan_startup, supports_automatic_start};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(super) fn start_specified_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let connections = self.connection_state.connections();
        let nodes = catalog(self.connection_state.groups(), connections);
        let plan = plan_startup(&self.settings.summary().startup_targets, &nodes);
        let mut skipped = plan.unavailable_targets.len();
        let mut tabs = Vec::new();
        for id in plan.connection_ids {
            let Some(connection) = connections.iter().find(|connection| connection.id == id) else {
                continue;
            };
            if !supports_automatic_start(&connection.config) {
                skipped += 1;
                continue;
            }
            let session_type = match &connection.config {
                ConnectionType::Ssh { .. } => "SSH",
                ConnectionType::LocalTerminal { .. } => "Local",
                ConnectionType::Telnet { .. } => "Telnet",
                ConnectionType::Serial { .. } => "Serial",
                ConnectionType::Rdp { .. } => "RDP",
                ConnectionType::Vnc { .. } => "VNC",
                ConnectionType::Adb { .. } => "adb_shell",
                ConnectionType::Ftp { .. } => "ftp",
            };
            tabs.push(RestorableOpenTab::with_leaf_root(
                connection.name.clone(),
                session_type,
                Some(id),
                None,
                None,
            ));
        }
        self.stores.startup_restore.update(cx, |store, _| {
            store.begin_specified(skipped);
            store.set_queue(tabs);
            store.request_focus_handoff();
        });
        // A preset contains connection references only. Old pane indexes cannot
        // describe it, even if the user changes the setting while jobs are running.
        self.shell.complete_terminal_windows_restore();
        self.shell.set_workspace_pane_layout_restored(true);
        self.pump_startup_restore_queue(window, cx);
    }
}

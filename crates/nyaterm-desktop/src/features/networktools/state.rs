//! Authoritative state for the built-in network tools panel.
//!
//! The transport registry owns listeners and file IO. This state owns the saved
//! settings, each service's configuration draft and child window, the bounded
//! event channel and the presentation-only caches (transfers, shared log tail
//! and per-transfer toast throttling).
//!
//! Every mutable map here is keyed by [`NetworkToolKind`], so adding a service
//! never shares a draft, window slot, error or transfer row with another one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, sync_channel};

use gpui::Task;
use nyaterm_core::{NetworkToolsSettings, TftpSettings};
use nyaterm_transport::networktools::{
    NetworkToolEvent, NetworkToolKind, NetworkToolRegistry, NetworkToolStatus, TransferSnapshot,
};
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};

use super::common::form::BindOption;

const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Editable snapshot of one service, keyed by that service in the draft maps.
#[derive(Clone)]
pub(in crate::features) enum NetworkToolDraft {
    Tftp(TftpSettings),
}

/// One service's configuration window plus whether an unlock must reopen it.
#[derive(Default)]
struct ConfigWindowEntry {
    window: ChildWindowSlot,
    reopen_on_unlock: bool,
}

pub(in crate::features) struct NetworkToolsFeatureState {
    settings: NetworkToolsSettings,
    /// Per-service editable snapshot, populated when its window opens.
    drafts: HashMap<NetworkToolKind, NetworkToolDraft>,
    /// Saved snapshot used to restore the draft when the window is cancelled.
    draft_backups: HashMap<NetworkToolKind, NetworkToolDraft>,
    config_windows: HashMap<NetworkToolKind, ConfigWindowEntry>,
    pub(in crate::features) registry: NetworkToolRegistry,
    events: Option<Receiver<NetworkToolEvent>>,
    pub(in crate::features) pump: Option<Task<()>>,
    transfers: HashMap<NetworkToolKind, Vec<TransferSnapshot>>,
    /// Per-service presentation cache of recent lines from that service's own
    /// log file. Read on demand; writing, rotation and retention stay in the
    /// shared tracing subscriber.
    logs: HashMap<NetworkToolKind, Vec<String>>,
    /// Enumerated once when a configuration window opens; rendering only reads
    /// this snapshot and never touches the network stack.
    bind_options: HashMap<NetworkToolKind, Vec<BindOption>>,
    /// Last directory existence/readability probe, with the exact path it was
    /// taken for so an edited path does not inherit a stale error.
    root_dir_probes: HashMap<NetworkToolKind, (String, bool)>,
    status_messages: HashMap<NetworkToolKind, String>,
    /// The application log directory. Only read on demand to show recent lines;
    /// writing, level filtering, sharding and retention stay in the shared
    /// tracing subscriber.
    log_dir: PathBuf,
}

impl NetworkToolsFeatureState {
    pub(in crate::features) fn new(settings: NetworkToolsSettings, log_dir: PathBuf) -> Self {
        let (sender, receiver) = sync_channel(EVENT_CHANNEL_CAPACITY);
        Self {
            settings,
            drafts: HashMap::new(),
            draft_backups: HashMap::new(),
            config_windows: HashMap::new(),
            registry: NetworkToolRegistry::new(sender),
            events: Some(receiver),
            pump: None,
            transfers: HashMap::new(),
            logs: HashMap::new(),
            bind_options: HashMap::new(),
            root_dir_probes: HashMap::new(),
            status_messages: HashMap::new(),
            log_dir,
        }
    }

    pub(in crate::features) fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub(in crate::features) fn is_running(&self, kind: NetworkToolKind) -> bool {
        self.registry.is_running(kind)
    }

    pub(in crate::features) fn status(&self, kind: NetworkToolKind) -> NetworkToolStatus {
        self.registry.status(kind)
    }

    pub(in crate::features) fn settings(&self) -> &NetworkToolsSettings {
        &self.settings
    }

    pub(in crate::features) fn status_message(&self, kind: NetworkToolKind) -> Option<&str> {
        self.status_messages.get(&kind).map(String::as_str)
    }

    pub(in crate::features) fn set_status_message(
        &mut self,
        kind: NetworkToolKind,
        message: Option<String>,
    ) {
        match message {
            Some(message) => {
                self.status_messages.insert(kind, message);
            }
            None => {
                self.status_messages.remove(&kind);
            }
        }
    }

    /// Whether a configuration session exists for this service. A session is
    /// created when the open is requested and removed when it ends, so it also
    /// covers the gap before the OS window appears.
    pub(in crate::features) fn is_config_open(&self, kind: NetworkToolKind) -> bool {
        self.config_windows.contains_key(&kind)
    }

    pub(in crate::features) fn begin_window_open(&mut self, kind: NetworkToolKind) -> bool {
        self.config_windows
            .entry(kind)
            .or_default()
            .window
            .begin_open()
    }

    pub(in crate::features) fn config_window_handle(
        &self,
        kind: NetworkToolKind,
    ) -> Option<NyaWindowHandle> {
        self.config_windows
            .get(&kind)
            .and_then(|entry| entry.window.handle())
    }

    pub(in crate::features) fn config_window_is_pending(&self, kind: NetworkToolKind) -> bool {
        self.config_windows
            .get(&kind)
            .is_some_and(|entry| entry.window.is_pending())
    }

    pub(in crate::features) fn config_window_slot(
        &mut self,
        kind: NetworkToolKind,
    ) -> Option<&mut ChildWindowSlot> {
        self.config_windows
            .get_mut(&kind)
            .map(|entry| &mut entry.window)
    }

    pub(in crate::features) fn attach_config_window(
        &mut self,
        kind: NetworkToolKind,
        handle: NyaWindowHandle,
    ) {
        if let Some(entry) = self.config_windows.get_mut(&kind) {
            entry.window.finish_open(handle);
            entry.reopen_on_unlock = false;
        }
    }

    pub(in crate::features) fn fail_config_window(&mut self, kind: NetworkToolKind) {
        if let Some(mut entry) = self.config_windows.remove(&kind) {
            entry.window.fail_open();
        }
    }

    /// End the session for one service and hand back its window, if any. Other
    /// services' sessions are untouched.
    pub(in crate::features) fn end_config_session(
        &mut self,
        kind: NetworkToolKind,
    ) -> Option<NyaWindowHandle> {
        self.config_windows
            .remove(&kind)
            .and_then(|entry| entry.window.handle())
    }

    /// Close every service's window for the lock screen, keeping the drafts and
    /// the intent to reopen. Returns the windows that were actually open.
    pub(in crate::features) fn suspend_config_windows(&mut self) -> Vec<NyaWindowHandle> {
        let mut handles = Vec::new();
        for entry in self.config_windows.values_mut() {
            if !entry.window.is_open_or_pending() {
                continue;
            }
            entry.reopen_on_unlock = true;
            entry.window.cancel_open();
            if let Some(handle) = entry.window.handle() {
                entry.window.clear();
                handles.push(handle);
            }
        }
        handles
    }

    /// Consume the services whose windows must come back after unlocking.
    pub(in crate::features) fn take_reopen_kinds(&mut self) -> Vec<NetworkToolKind> {
        self.config_windows
            .iter_mut()
            .filter_map(|(kind, entry)| {
                std::mem::take(&mut entry.reopen_on_unlock).then_some(*kind)
            })
            .collect()
    }

    /// Snapshot the saved settings into the draft before the window edits them.
    pub(in crate::features) fn begin_config_edit(&mut self, kind: NetworkToolKind) {
        match kind {
            NetworkToolKind::Tftp => {
                let mut draft = self.settings.tftp.clone();
                self.draft_backups
                    .insert(kind, NetworkToolDraft::Tftp(self.settings.tftp.clone()));
                // An unset directory shows the platform default so the field is
                // never blank; Cancel still restores the saved (possibly empty)
                // value.
                if draft.root_dir.trim().is_empty()
                    && let Some(directory) = super::default_networktools_share_dir()
                {
                    draft.root_dir = directory.display().to_string();
                }
                self.root_dir_probes.insert(
                    kind,
                    (
                        draft.root_dir.clone(),
                        std::fs::read_dir(&draft.root_dir).is_ok(),
                    ),
                );
                self.bind_options.insert(
                    kind,
                    super::common::form::bind_options(
                        &nyaterm_remote_ops::local_network::local_interface_addresses(),
                        &draft.bind_address,
                    ),
                );
                self.drafts.insert(kind, NetworkToolDraft::Tftp(draft));
            }
        }
    }

    pub(in crate::features) fn discard_config_edits(&mut self, kind: NetworkToolKind) {
        if let Some(backup) = self.draft_backups.remove(&kind) {
            self.drafts.insert(kind, backup);
        }
    }

    pub(in crate::features) fn commit_config_edits(&mut self, kind: NetworkToolKind) {
        if let Some(NetworkToolDraft::Tftp(draft)) = self.drafts.get(&kind) {
            self.settings.tftp = draft.clone();
        }
        self.draft_backups.remove(&kind);
    }

    /// The TFTP draft. Falls back to the saved settings when no window has
    /// opened yet, so a caller never sees a fabricated default.
    pub(in crate::features) fn tftp_draft(&self) -> TftpSettings {
        match self.drafts.get(&NetworkToolKind::Tftp) {
            Some(NetworkToolDraft::Tftp(draft)) => draft.clone(),
            None => self.settings.tftp.clone(),
        }
    }

    pub(in crate::features) fn set_tftp_draft(&mut self, draft: TftpSettings) {
        self.drafts
            .insert(NetworkToolKind::Tftp, NetworkToolDraft::Tftp(draft));
    }

    /// Mutable TFTP draft for the form's switches. Creates the entry from the
    /// saved settings if the window has not populated it yet.
    pub(in crate::features) fn tftp_draft_mut(&mut self) -> &mut TftpSettings {
        let kind = NetworkToolKind::Tftp;
        if !self.drafts.contains_key(&kind) {
            let draft = self.settings.tftp.clone();
            self.drafts.insert(kind, NetworkToolDraft::Tftp(draft));
        }
        match self.drafts.get_mut(&kind) {
            Some(NetworkToolDraft::Tftp(draft)) => draft,
            None => unreachable!("the TFTP draft was just inserted"),
        }
    }

    pub(in crate::features) fn take_events(&self) -> Vec<NetworkToolEvent> {
        let Some(receiver) = self.events.as_ref() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    pub(in crate::features) fn set_log_lines(&mut self, kind: NetworkToolKind, lines: Vec<String>) {
        self.logs.insert(kind, lines);
    }

    pub(in crate::features) fn log_lines(&self, kind: NetworkToolKind) -> &[String] {
        self.logs.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(in crate::features) fn bind_options(&self, kind: NetworkToolKind) -> &[BindOption] {
        self.bind_options
            .get(&kind)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// `Some(usable)` only when the probe was taken for this exact path.
    pub(in crate::features) fn root_dir_probe(
        &self,
        kind: NetworkToolKind,
        path: &str,
    ) -> Option<bool> {
        self.root_dir_probes
            .get(&kind)
            .filter(|(probed, _)| probed == path)
            .map(|(_, usable)| *usable)
    }

    pub(in crate::features) fn set_root_dir_probe(
        &mut self,
        kind: NetworkToolKind,
        path: String,
        usable: bool,
    ) {
        self.root_dir_probes.insert(kind, (path, usable));
    }

    pub(in crate::features) fn transfers(&self, kind: NetworkToolKind) -> &[TransferSnapshot] {
        self.transfers.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(in crate::features) fn push_transfer(
        &mut self,
        kind: NetworkToolKind,
        snapshot: TransferSnapshot,
    ) {
        self.transfers.entry(kind).or_default().push(snapshot);
    }

    pub(in crate::features) fn update_transfer(
        &mut self,
        kind: NetworkToolKind,
        id: u64,
        done: u64,
        total: Option<u64>,
    ) {
        if let Some(snapshot) = self
            .transfers
            .get_mut(&kind)
            .and_then(|transfers| transfers.iter_mut().find(|item| item.id == id))
        {
            snapshot.done = done;
            snapshot.total = total;
        }
    }

    pub(in crate::features) fn remove_transfer(&mut self, kind: NetworkToolKind, id: u64) {
        if let Some(transfers) = self.transfers.get_mut(&kind) {
            transfers.retain(|item| item.id != id);
        }
    }

    pub(in crate::features) fn clear_transfers(&mut self, kind: NetworkToolKind) {
        self.transfers.remove(&kind);
    }
}

impl Drop for NetworkToolsFeatureState {
    fn drop(&mut self) {
        self.registry.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{NetworkToolsSettings, TftpSettings};
    use nyaterm_transport::networktools::{NetworkToolKind, TransferDirection, TransferSnapshot};

    use super::NetworkToolsFeatureState;

    fn state() -> NetworkToolsFeatureState {
        NetworkToolsFeatureState::new(NetworkToolsSettings::default(), std::path::PathBuf::new())
    }

    #[test]
    fn window_claim_is_refused_while_open_or_pending() {
        let kind = NetworkToolKind::Tftp;
        let mut state = state();
        assert!(!state.is_config_open(kind));
        assert!(state.begin_window_open(kind));
        assert!(state.is_config_open(kind));
        assert!(!state.begin_window_open(kind));
        state.suspend_config_windows();
        assert!(state.begin_window_open(kind));
        assert_eq!(state.take_reopen_kinds(), vec![kind]);
    }

    #[test]
    fn ending_a_session_drops_its_own_window_claim() {
        let mut state = state();
        let kind = NetworkToolKind::Tftp;
        assert!(state.begin_window_open(kind));
        state.begin_config_edit(kind);
        assert_eq!(state.end_config_session(kind), None);
        assert!(!state.is_config_open(kind));
    }

    #[test]
    fn cancel_restores_the_saved_draft() {
        let kind = NetworkToolKind::Tftp;
        let mut state = state();
        state.begin_config_edit(kind);
        state.set_tftp_draft(TftpSettings {
            port: 1069,
            root_dir: "/tmp/share".to_string(),
            ..TftpSettings::default()
        });
        state.discard_config_edits(kind);
        assert_eq!(state.tftp_draft(), state.settings.tftp);
    }

    #[test]
    fn commit_persists_the_draft() {
        let kind = NetworkToolKind::Tftp;
        let mut state = state();
        state.begin_config_edit(kind);
        state.set_tftp_draft(TftpSettings {
            port: 1069,
            root_dir: "/tmp/share".to_string(),
            ..TftpSettings::default()
        });
        state.commit_config_edits(kind);
        assert_eq!(state.settings.tftp.port, 1069);
        assert_eq!(state.settings.tftp.root_dir, "/tmp/share");
    }

    #[test]
    fn transfers_and_errors_stay_with_their_service() {
        let kind = NetworkToolKind::Tftp;
        let mut state = state();
        state.push_transfer(
            kind,
            TransferSnapshot::new(
                1,
                TransferDirection::Download,
                "firmware.bin".to_string(),
                "10.0.0.2:69".to_string(),
                Some(1024),
            ),
        );
        state.set_status_message(kind, Some("port in use".to_string()));
        assert_eq!(state.transfers(kind).len(), 1);
        assert_eq!(state.status_message(kind), Some("port in use"));

        state.clear_transfers(kind);
        state.set_status_message(kind, None);
        assert!(state.transfers(kind).is_empty());
        assert_eq!(state.status_message(kind), None);
    }

    #[test]
    fn suspend_marks_every_open_window_for_reopen() {
        let kind = NetworkToolKind::Tftp;
        let mut state = state();
        assert!(state.begin_window_open(kind));
        assert!(state.suspend_config_windows().is_empty());
        assert_eq!(state.take_reopen_kinds(), vec![kind]);
        assert!(state.take_reopen_kinds().is_empty());
    }

    #[test]
    fn shared_log_tail_replaces_any_private_buffer() {
        let mut state = state();
        state.set_log_lines(
            NetworkToolKind::Tftp,
            vec!["line 1".to_string(), "line 2".to_string()],
        );
        assert_eq!(state.log_lines(NetworkToolKind::Tftp), ["line 1", "line 2"]);
        state.set_log_lines(NetworkToolKind::Tftp, Vec::new());
        assert!(state.log_lines(NetworkToolKind::Tftp).is_empty());
    }
}

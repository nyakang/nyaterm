use gpui::Context;
use nyaterm_core::{WorkspaceId, WorkspaceRestoreState};
use nyaterm_store::{BootstrapSnapshot, StoreDomain};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SharedStateDomain {
    Settings,
    Connections,
    Security,
    Tunnels,
    Commands,
    Ai,
    Translation,
    CloudSync,
    All,
}

impl SharedStateDomain {
    pub(crate) fn from_store_domain(domain: StoreDomain) -> Option<Self> {
        match domain {
            StoreDomain::Settings => Some(Self::Settings),
            StoreDomain::Connections => Some(Self::Connections),
            StoreDomain::Security => Some(Self::Security),
            StoreDomain::Tunnels => Some(Self::Tunnels),
            StoreDomain::Commands => Some(Self::Commands),
            StoreDomain::Ai => Some(Self::Ai),
            StoreDomain::CloudSync => Some(Self::CloudSync),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SharedStateEvent {
    pub(crate) domain: SharedStateDomain,
    pub(crate) revision: u64,
    pub(crate) changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SettingsDraftRevisions {
    pub(crate) settings: u64,
    pub(crate) ai: u64,
    pub(crate) translation: u64,
    pub(crate) cloud_sync: u64,
}

pub(crate) enum GlobalStateMutation {
    UpdateSettings(Box<nyaterm_core::AppSettingsSummary>),
    ReplaceSnapshot {
        snapshot: Box<BootstrapSnapshot>,
        domain: SharedStateDomain,
    },
}

#[derive(Clone)]
pub(crate) struct WorkspaceInitSnapshot {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) state: Option<WorkspaceRestoreState>,
}

pub(crate) struct ProcessStateStore {
    snapshot: BootstrapSnapshot,
    revision: u64,
    settings_revisions: SettingsDraftRevisions,
}

impl ProcessStateStore {
    pub(crate) fn new(snapshot: BootstrapSnapshot) -> Self {
        Self {
            snapshot,
            revision: 0,
            settings_revisions: SettingsDraftRevisions::default(),
        }
    }

    pub(crate) fn snapshot(&self) -> &BootstrapSnapshot {
        &self.snapshot
    }

    pub(crate) fn workspace_init(&self, workspace_id: WorkspaceId) -> WorkspaceInitSnapshot {
        WorkspaceInitSnapshot {
            workspace_id,
            state: self
                .snapshot
                .workspace_restore
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .cloned(),
        }
    }

    pub(crate) fn settings_draft_revisions(&self) -> SettingsDraftRevisions {
        self.settings_revisions
    }

    pub(crate) fn mutate(
        &mut self,
        mutation: GlobalStateMutation,
        cx: &mut Context<Self>,
    ) -> SharedStateEvent {
        let event = self.apply_mutation(mutation);
        if event.changed {
            cx.notify();
        }
        event
    }

    fn apply_mutation(&mut self, mutation: GlobalStateMutation) -> SharedStateEvent {
        let mut changed = true;
        let domain = match mutation {
            GlobalStateMutation::UpdateSettings(settings) => {
                let local = &self.snapshot.settings;
                let mut settings = *settings;
                preserve_workspace_local_settings(local, &mut settings);
                changed = self.snapshot.settings != settings;
                if changed {
                    self.snapshot.settings = settings;
                }
                SharedStateDomain::Settings
            }
            GlobalStateMutation::ReplaceSnapshot {
                mut snapshot,
                domain,
            } => {
                if domain == SharedStateDomain::Settings {
                    let mut normalized = snapshot.settings.clone();
                    let local = &self.snapshot.settings;
                    preserve_workspace_local_settings(local, &mut normalized);
                    changed = normalized != self.snapshot.settings
                        || snapshot.keyword_highlights != self.snapshot.keyword_highlights
                        || snapshot.default_session_settings
                            != self.snapshot.default_session_settings;
                    snapshot.settings = normalized;
                }
                self.snapshot = *snapshot;
                domain
            }
        };
        if !changed {
            return SharedStateEvent {
                domain,
                revision: self.revision,
                changed: false,
            };
        }
        self.revision = self.revision.saturating_add(1);
        match domain {
            SharedStateDomain::Settings => {
                self.settings_revisions.settings =
                    self.settings_revisions.settings.saturating_add(1);
            }
            SharedStateDomain::Ai => {
                self.settings_revisions.ai = self.settings_revisions.ai.saturating_add(1);
            }
            SharedStateDomain::Translation => {
                self.settings_revisions.translation =
                    self.settings_revisions.translation.saturating_add(1);
            }
            SharedStateDomain::CloudSync => {
                self.settings_revisions.cloud_sync =
                    self.settings_revisions.cloud_sync.saturating_add(1);
            }
            SharedStateDomain::All => {
                self.settings_revisions.settings =
                    self.settings_revisions.settings.saturating_add(1);
                self.settings_revisions.ai = self.settings_revisions.ai.saturating_add(1);
                self.settings_revisions.translation =
                    self.settings_revisions.translation.saturating_add(1);
                self.settings_revisions.cloud_sync =
                    self.settings_revisions.cloud_sync.saturating_add(1);
            }
            SharedStateDomain::Connections
            | SharedStateDomain::Security
            | SharedStateDomain::Tunnels
            | SharedStateDomain::Commands => {}
        }
        SharedStateEvent {
            domain,
            revision: self.revision,
            changed: true,
        }
    }
}

fn preserve_workspace_local_settings(
    local: &nyaterm_core::AppSettingsSummary,
    incoming: &mut nyaterm_core::AppSettingsSummary,
) {
    incoming.ui_left_panel_width = local.ui_left_panel_width;
    incoming.ui_right_panel_width = local.ui_right_panel_width;
    incoming.ui_transfer_height = local.ui_transfer_height;
    incoming.ui_quick_cmd_height = local.ui_quick_cmd_height;
    incoming.ui_quick_cmd_visible = local.ui_quick_cmd_visible;
    incoming.ui_serial_send_height = local.ui_serial_send_height;
    incoming.ui_serial_send_visible = local.ui_serial_send_visible;
    incoming.ui_active_left_panel = local.ui_active_left_panel.clone();
    incoming.ui_active_right_panel = local.ui_active_right_panel.clone();
    incoming.ui_left_panel_collapsed = local.ui_left_panel_collapsed;
    incoming.ui_right_panel_collapsed = local.ui_right_panel_collapsed;
    incoming.ui_panel_multi_open = local.ui_panel_multi_open;
    incoming.ui_panel_open_mode = local.ui_panel_open_mode.clone();
    incoming.ui_left_open_panels = local.ui_left_open_panels.clone();
    incoming.ui_right_open_panels = local.ui_right_open_panels.clone();
    incoming.ui_panel_stack_sizes = local.ui_panel_stack_sizes.clone();
}

#[cfg(test)]
mod tests {
    use nyaterm_store::{LoadBootstrap, StoreConfig, StoreRuntime};

    use super::{
        GlobalStateMutation, ProcessStateStore, SettingsDraftRevisions, SharedStateDomain,
    };
    use crate::test_support::TestConfigDir;

    fn process_state() -> (TestConfigDir, ProcessStateStore) {
        let root = TestConfigDir::new("nyaterm-process-state");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: root.path().join("config"),
            portable_key_path: None,
        })
        .expect("spawn store");
        let snapshot = runtime
            .blocking_client()
            .request(0, LoadBootstrap)
            .expect("receive bootstrap")
            .outcome
            .expect("load bootstrap");
        (root, ProcessStateStore::new(snapshot))
    }

    #[test]
    fn settings_mutation_preserves_workspace_local_projection() {
        let (_root, mut state) = process_state();
        state.snapshot.settings.ui_left_panel_width = 731;
        state.snapshot.settings.ui_transfer_height = 287;
        state.snapshot.settings.ui_quick_cmd_visible = false;
        state.snapshot.settings.ui_serial_send_visible = true;
        state.snapshot.settings.ui_panel_multi_open = true;
        state.snapshot.settings.ui_panel_open_mode = "floating".to_string();
        state.snapshot.settings.ui_left_open_panels = vec!["notes".to_string()];
        state
            .snapshot
            .settings
            .ui_panel_stack_sizes
            .insert("left:notes".to_string(), 700);
        state.snapshot.settings.ui_active_left_panel = Some("sessions".to_string());
        let mut settings = state.snapshot.settings.clone();
        settings.language = "ja".to_string();
        settings.ui_left_panel_width = 999;
        settings.ui_transfer_height = 444;
        settings.ui_quick_cmd_visible = true;
        settings.ui_serial_send_visible = false;
        settings.ui_panel_multi_open = false;
        settings.ui_panel_open_mode = "docked".to_string();
        settings.ui_left_open_panels.clear();
        settings.ui_panel_stack_sizes.clear();
        settings.ui_active_left_panel = Some("notes".to_string());

        let event = state.apply_mutation(GlobalStateMutation::UpdateSettings(Box::new(settings)));

        assert_eq!(event.domain, SharedStateDomain::Settings);
        assert_eq!(state.snapshot.settings.language, "ja");
        assert_eq!(state.snapshot.settings.ui_left_panel_width, 731);
        assert_eq!(state.snapshot.settings.ui_transfer_height, 287);
        assert!(!state.snapshot.settings.ui_quick_cmd_visible);
        assert!(state.snapshot.settings.ui_serial_send_visible);
        assert!(state.snapshot.settings.ui_panel_multi_open);
        assert_eq!(state.snapshot.settings.ui_panel_open_mode, "floating");
        assert_eq!(state.snapshot.settings.ui_left_open_panels, ["notes"]);
        assert_eq!(
            state.snapshot.settings.ui_panel_stack_sizes["left:notes"],
            700
        );
        assert_eq!(
            state.snapshot.settings.ui_active_left_panel.as_deref(),
            Some("sessions")
        );
        assert_eq!(
            state.settings_draft_revisions(),
            SettingsDraftRevisions {
                settings: 1,
                ..SettingsDraftRevisions::default()
            }
        );
    }

    #[test]
    fn settings_snapshot_refresh_applies_normalized_workspace_local_projection() {
        let (_root, mut state) = process_state();
        state.snapshot.settings.ui_left_panel_width = 731;
        state.snapshot.settings.ui_quick_cmd_visible = false;
        state.snapshot.settings.ui_serial_send_visible = true;
        state.snapshot.settings.ui_left_open_panels = vec!["notes".to_string()];
        let mut snapshot = state.snapshot.clone();
        snapshot.settings.language = "ja".to_string();
        snapshot.settings.ui_left_panel_width = 999;
        snapshot.settings.ui_quick_cmd_visible = true;
        snapshot.settings.ui_serial_send_visible = false;
        snapshot.settings.ui_left_open_panels.clear();

        let event = state.apply_mutation(GlobalStateMutation::ReplaceSnapshot {
            snapshot: Box::new(snapshot),
            domain: SharedStateDomain::Settings,
        });

        assert!(event.changed);
        assert_eq!(state.snapshot.settings.language, "ja");
        assert_eq!(state.snapshot.settings.ui_left_panel_width, 731);
        assert!(!state.snapshot.settings.ui_quick_cmd_visible);
        assert!(state.snapshot.settings.ui_serial_send_visible);
        assert_eq!(state.snapshot.settings.ui_left_open_panels, ["notes"]);
    }

    #[test]
    fn full_refresh_advances_every_settings_domain_revision() {
        let (_root, mut state) = process_state();
        let snapshot = state.snapshot.clone();

        let event = state.apply_mutation(GlobalStateMutation::ReplaceSnapshot {
            snapshot: Box::new(snapshot),
            domain: SharedStateDomain::All,
        });

        assert_eq!(event.revision, 1);
        assert_eq!(
            state.settings_draft_revisions(),
            SettingsDraftRevisions {
                settings: 1,
                ai: 1,
                translation: 1,
                cloud_sync: 1,
            }
        );
    }

    #[test]
    fn unchanged_settings_refresh_does_not_invalidate_a_draft() {
        let (_root, mut state) = process_state();
        let snapshot = state.snapshot.clone();

        let event = state.apply_mutation(GlobalStateMutation::ReplaceSnapshot {
            snapshot: Box::new(snapshot),
            domain: SharedStateDomain::Settings,
        });

        assert!(!event.changed);
        assert_eq!(event.revision, 0);
        assert_eq!(
            state.settings_draft_revisions(),
            SettingsDraftRevisions::default()
        );
    }
}

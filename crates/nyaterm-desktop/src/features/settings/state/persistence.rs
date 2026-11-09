//! settings persistence state and transitions.

use super::{SettingsFeatureState, StoreStatusView};
use crate::features::settings::catalog::StoreStatus;
use nyaterm_core::{AppSettingsSummary, KeywordHighlightConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::features) enum SettingsPersistenceDomain {
    Diagnostics,
    General,
    Interaction,
    ScreenLock,
    HostKey,
    Recording,
    Transfer,
    Terminal,
    QuickCommands,
    Appearance,
    UiLayout,
    Keybindings,
    FileExplorer,
}

pub(super) struct SettingsPersistenceSlot {
    pub(super) latest_generation: u64,
    pub(super) in_flight_generation: Option<u64>,
    pub(super) pending: Option<AppSettingsSummary>,
    pub(super) dirty: bool,
}

pub(in crate::features) struct SettingsPersistenceCompletion {
    pub apply_result: bool,
    pub report_result: bool,
    pub next: Option<(u64, AppSettingsSummary)>,
}

impl SettingsFeatureState {
    pub(in crate::features) fn queue_persistence(
        &mut self,
        domain: SettingsPersistenceDomain,
    ) -> Option<(u64, AppSettingsSummary)> {
        let slot = self
            .persistence
            .entry(domain)
            .or_insert_with(|| SettingsPersistenceSlot {
                latest_generation: 0,
                in_flight_generation: None,
                pending: None,
                dirty: false,
            });
        slot.latest_generation = slot.latest_generation.saturating_add(1);
        slot.dirty = true;
        let snapshot = self.summary.clone();
        if slot.in_flight_generation.is_some() {
            slot.pending = Some(snapshot);
            None
        } else {
            slot.in_flight_generation = Some(slot.latest_generation);
            Some((slot.latest_generation, snapshot))
        }
    }
    pub(in crate::features) fn finish_persistence(
        &mut self,
        domain: SettingsPersistenceDomain,
        generation: u64,
        succeeded: bool,
    ) -> SettingsPersistenceCompletion {
        let Some(slot) = self.persistence.get_mut(&domain) else {
            return SettingsPersistenceCompletion {
                apply_result: false,
                report_result: false,
                next: None,
            };
        };
        if slot.in_flight_generation != Some(generation) {
            return SettingsPersistenceCompletion {
                apply_result: false,
                report_result: false,
                next: None,
            };
        }
        slot.in_flight_generation = None;
        let next = slot.pending.take().map(|snapshot| {
            let generation = slot.latest_generation;
            slot.in_flight_generation = Some(generation);
            (generation, snapshot)
        });
        let report_result = generation == slot.latest_generation && next.is_none();
        let apply_result = succeeded && report_result;
        if apply_result {
            slot.dirty = false;
        }
        SettingsPersistenceCompletion {
            apply_result,
            report_result,
            next,
        }
    }
    /// Mark a domain as owing a durable write without submitting one.
    ///
    /// Used by the shutdown path to fold in a write that a debounce window had not
    /// got to yet: `dirty_persistence_domains` is what decides the shutdown batch,
    /// and only `queue_persistence` would otherwise have set this.
    pub(in crate::features) fn mark_persistence_dirty(
        &mut self,
        domain: SettingsPersistenceDomain,
    ) {
        self.persistence
            .entry(domain)
            .or_insert_with(|| SettingsPersistenceSlot {
                latest_generation: 0,
                in_flight_generation: None,
                pending: None,
                dirty: false,
            })
            .dirty = true;
    }
    pub(in crate::features) fn clear_draft_dirty_domains(&mut self) {
        self.draft_dirty_domains.clear();
    }
    pub(in crate::features) fn draft_dirty_domains(&self) -> Vec<SettingsPersistenceDomain> {
        const DOMAINS: [SettingsPersistenceDomain; 13] = [
            SettingsPersistenceDomain::Diagnostics,
            SettingsPersistenceDomain::General,
            SettingsPersistenceDomain::Interaction,
            SettingsPersistenceDomain::ScreenLock,
            SettingsPersistenceDomain::HostKey,
            SettingsPersistenceDomain::Recording,
            SettingsPersistenceDomain::Transfer,
            SettingsPersistenceDomain::Terminal,
            SettingsPersistenceDomain::QuickCommands,
            SettingsPersistenceDomain::Appearance,
            SettingsPersistenceDomain::UiLayout,
            SettingsPersistenceDomain::Keybindings,
            SettingsPersistenceDomain::FileExplorer,
        ];
        DOMAINS
            .into_iter()
            .filter(|domain| self.draft_dirty_domains.contains(domain))
            .collect()
    }
    pub(in crate::features) fn dirty_persistence_domains(&self) -> Vec<SettingsPersistenceDomain> {
        const DOMAINS: [SettingsPersistenceDomain; 13] = [
            SettingsPersistenceDomain::Diagnostics,
            SettingsPersistenceDomain::General,
            SettingsPersistenceDomain::Interaction,
            SettingsPersistenceDomain::ScreenLock,
            SettingsPersistenceDomain::HostKey,
            SettingsPersistenceDomain::Recording,
            SettingsPersistenceDomain::Transfer,
            SettingsPersistenceDomain::Terminal,
            SettingsPersistenceDomain::QuickCommands,
            SettingsPersistenceDomain::Appearance,
            SettingsPersistenceDomain::UiLayout,
            SettingsPersistenceDomain::Keybindings,
            SettingsPersistenceDomain::FileExplorer,
        ];
        DOMAINS
            .into_iter()
            .filter(|domain| self.persistence.get(domain).is_some_and(|slot| slot.dirty))
            .collect()
    }
    pub(in crate::features) fn keyword_persistence_dirty(&self) -> bool {
        self.keyword_persistence_dirty
    }
    pub(in crate::features) fn mark_keyword_persistence_dirty(&mut self) {
        self.keyword_persistence_dirty = true;
    }
    pub(in crate::features) fn finish_keyword_persistence(
        &mut self,
        saved: &KeywordHighlightConfig,
    ) {
        if &self.keyword_config == saved {
            self.keyword_persistence_dirty = false;
        }
    }
    pub(in crate::features) fn store_status(&self) -> StoreStatusView<'_> {
        StoreStatusView {
            path: &self.store_status.path,
            message: &self.store_status.message,
            ready: self.store_status.ready,
        }
    }
    pub(in crate::features) fn set_store_message(&mut self, message: impl Into<String>) {
        self.store_status.message = message.into();
    }
    pub(in crate::features) fn update_store_status(
        &mut self,
        message: impl Into<String>,
        ready: bool,
    ) {
        self.store_status.message = message.into();
        self.store_status.ready = ready;
    }
    pub(in crate::features) fn replace_store_status(
        &mut self,
        path: String,
        message: String,
        ready: bool,
    ) {
        self.store_status = StoreStatus {
            path,
            message,
            ready,
        };
    }
}

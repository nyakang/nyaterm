//! Cloud sync provider adapters and cloud sync runtime.

mod auto_sync;
mod cloud_sync_provider;
mod cloud_sync_runtime;
mod cloud_sync_v4;
pub(crate) mod io_runtime;
pub(crate) mod operation;
mod state;
pub(in crate::features) use cloud_sync_v4::PreviewSummary as V4PreviewSummary;

pub(crate) use auto_sync::{AutoSyncRequest, AutoSyncResult, AutoSyncTrigger, run_auto_sync};
pub(in crate::features) use cloud_sync_provider::{
    cleanup_provider_snapshots, pull_provider_snapshot, push_provider_snapshot,
    recover_provider_snapshot, test_provider_connection,
};
pub(in crate::features) use state::{CloudSyncFeatureState, CloudSyncLiveState, SyncHistoryFilter};

/// The sync protocol that can actually run for the configured provider.
///
/// Providers that have not passed V4 capability validation (GitHub Gist,
/// Gitee snippets) keep working through the legacy path even when a V4
/// preference was persisted before the provider changed.
pub(crate) fn effective_protocol_version(settings: &nyaterm_core::CloudSyncSettings) -> u32 {
    nyaterm_sync::protocol::ProviderCapabilities::effective_protocol_version(
        &settings.provider,
        settings.protocol_version,
    )
}

#[cfg(test)]
mod legacy_compat_tests;

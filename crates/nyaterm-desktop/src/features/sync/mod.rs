//! Cloud sync provider adapters and cloud sync runtime.

mod auto_sync;
mod cloud_sync_provider;
mod cloud_sync_runtime;
mod cloud_sync_v4;
pub(crate) mod io_runtime;
pub(crate) mod operation;
mod state;
pub(in crate::features) use cloud_sync_v4::PreviewSummary as V4PreviewSummary;

pub(crate) use auto_sync::{AutoSyncResult, AutoSyncTrigger, run_auto_sync};
pub(in crate::features) use cloud_sync_provider::{
    cleanup_provider_snapshots, pull_provider_snapshot, push_provider_snapshot,
    recover_provider_snapshot, test_provider_connection,
};
pub(in crate::features) use state::{CloudSyncFeatureState, CloudSyncLiveState, SyncHistoryFilter};

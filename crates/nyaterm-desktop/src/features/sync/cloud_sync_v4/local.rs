//! Read-only change detection before constructing a remote adapter.

use nyaterm_core::sync_schema::SyncScope;
use nyaterm_core::{CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions};
use nyaterm_store::sync_v4::LocalView;
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use nyaterm_sync::engine::{digest, prepare};

use super::{destination, invalid, store_error};

pub(super) fn load(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
) -> Result<LocalView, CloudSyncError> {
    let destination = destination(settings, options);
    let password = options.master_password.clone();
    let scope = settings.v4_scope.clone();
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.load_v4_view(&destination, password.expose_secret(), &scope)
        })
        .map_err(store_error)
}

pub(super) fn needs_remote(view: &LocalView, scope: &SyncScope) -> Result<bool, CloudSyncError> {
    let local = &view.record;
    let Some(replica) = &local.replica else {
        return Ok(true);
    };
    if !local.migration_approved
        || local.scope != *scope
        || local.pending.is_some()
        || !local.deferred.is_empty()
        || local.published_document.as_deref() != Some(digest(&replica.document).as_str())
        || !view.projection.edited.is_empty()
    {
        return Ok(true);
    }
    // Reuse capture/deletion/completeness rules, never the database generation:
    // that counter also protects device-local writes against concurrent Apply.
    let plan = prepare(
        local.writer,
        Some(replica),
        &local.effective,
        view.projection.clone(),
        &[],
        scope.clone(),
    )
    .map_err(|_| invalid())?;
    Ok(plan.replica != *replica)
}

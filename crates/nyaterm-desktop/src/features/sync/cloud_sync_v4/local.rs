//! Read-only change detection before constructing a remote adapter.

use nyaterm_core::sync_schema::{FIXED_CLOUD_SYNC_SCOPE, SyncScope, fixed_cloud_sync_field};
use nyaterm_core::{CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions};
use nyaterm_store::sync_v4::LocalView;
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use nyaterm_sync::engine::{digest, prepare};

use super::{destination, invalid, legacy_destination, store_error};

pub(super) fn load(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
) -> Result<LocalView, CloudSyncError> {
    let destination = destination(settings, options);
    let password = options.master_password.clone();
    let scope = FIXED_CLOUD_SYNC_SCOPE;
    let legacy_destination = legacy_destination(settings, options);
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            let mut view = store.load_v4_view(&destination, password.expose_secret(), &scope)?;
            if view.record_hash.is_none()
                && let Some((old, _)) =
                    store.load_v4_replica(&legacy_destination, password.expose_secret())?
                && let Some(snapshot) = old.replica
            {
                let mut replica =
                    nyaterm_sync::engine::SyncReplica::load(&snapshot, view.record.writer)
                        .map_err(|_| nyaterm_store::StorageError::CloudSyncLocalChanged)?;
                view.record.replica = Some(
                    replica
                        .fixed_snapshot()
                        .map_err(|_| nyaterm_store::StorageError::CloudSyncLocalChanged)?,
                );
                view.record.effective = old
                    .effective
                    .into_iter()
                    .filter(|(key, _)| {
                        nyaterm_core::sync_schema::SyncFieldId::from_key(key)
                            .is_some_and(|field| fixed_cloud_sync_field(&field))
                    })
                    .collect();
            }
            Ok(view)
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

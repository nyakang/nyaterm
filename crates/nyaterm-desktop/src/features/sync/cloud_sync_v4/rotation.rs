use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::{
    AppRuntime, CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions, SecretString,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use uuid::Uuid;

use super::{destination, invalid, preview, publish_bytes, store_error};

pub(in crate::features) fn rotate_master_password(
    store: &StoreBlockingClient,
    runtime: &AppRuntime,
    next_password: SecretString,
    control: &CloudSyncOperationControl,
) -> Result<Option<CloudSyncSettings>, CloudSyncError> {
    let _operation = crate::features::sync::operation::lock_cloud_sync_operation(control)?;
    if !store
        .request_fn(StoreDomain::CloudSync, |store| store.has_v4_replicas())
        .map_err(store_error)?
    {
        return Ok(None);
    }
    let (settings, state, password) = store
        .request_fn(StoreDomain::CloudSync, |store| {
            Ok((
                store.load_cloud_sync_settings()?,
                store.load_cloud_sync_state()?,
                store.load_cloud_sync_master_password()?,
            ))
        })
        .map_err(store_error)?;
    if password
        .as_ref()
        .is_some_and(|password| password.expose_secret() == next_password.expose_secret())
    {
        return Ok(None);
    }
    let options = LocalCloudSyncOptions {
        config_dir: runtime.config_dir().into(),
        portable_key_path: runtime.portable_key_path().map(ToOwned::to_owned),
        remote_dir: runtime.config_dir().join("cloud-sync-local"),
        remote_root: settings.remote_root.clone(),
        device_id: state.device_id,
        device_name: Some(settings.device_name.clone()),
        app_version: env!("CARGO_PKG_VERSION").into(),
        master_password: password.ok_or_else(invalid)?,
        enabled: true,
    };
    rotate_with_options(store, &settings, &options, next_password, control).map(Some)
}

pub(super) fn rotate_with_options(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    next_password: SecretString,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncSettings, CloudSyncError> {
    if settings.protocol_version != 4 {
        return Err(invalid());
    }
    let source = destination(settings, options);
    let cached = {
        let source = source.clone();
        let password = next_password.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.load_v4_password_rotation(&source, password.expose_secret())
            })
            .map_err(store_error)?
    };
    let staged = if let Some(staged) = cached {
        staged
    } else {
        let prepared = preview(store, settings, options, control)?;
        if prepared.migration {
            return Err(CloudSyncError::Remote(
                "Complete the Cloud Sync V3 migration preview before rotating the master password"
                    .into(),
            ));
        }
        let root = nyaterm_core::cloud_sync::remote_path(
            &options.remote_root,
            &format!("rotation-{}", Uuid::new_v4()),
        );
        let mut target_options = options.clone();
        target_options.remote_root = root.clone();
        let mut target_settings = settings.clone();
        target_settings.remote_root = root.clone();
        let target = destination(&target_settings, &target_options);
        let password = next_password.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.stage_v4_password_rotation(
                    prepared.apply,
                    password.expose_secret(),
                    &target,
                    &root,
                )
            })
            .map_err(store_error)?
    };
    let mut target_options = options.clone();
    target_options.remote_root = staged.remote_root.clone();
    target_options.master_password = next_password;
    let mut target_settings = settings.clone();
    target_settings.remote_root = staged.remote_root.clone();
    control.begin_commit()?;
    super::super::cloud_sync_provider::with_remote(
        store,
        &target_settings,
        &target_options,
        "v4_password_rotation",
        |remote, _| publish_bytes(&control.remote(remote), &target_options, &staged.pending),
    )?;
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.commit_verified_v4_password_rotation(&staged.ticket)?;
            store.load_cloud_sync_settings()
        })
        .map_err(store_error)
}

use crate::http::cloud_sync::{
    NativeAliyunDriveRemote, NativeGoogleDriveRemote, NativeOneDriveRemote, NativeS3Remote,
    NativeSnippetHttpClient, NativeWebdavRemote,
};
use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::cloud_sync::{self, CloudSyncRemote};
use nyaterm_core::{
    CloudSyncError, CloudSyncResult, CloudSyncSettings, CloudSyncState, GiteeSnippetHttpBackend,
    GithubGistHttpBackend, LocalCloudSyncOptions, RemoteSyncPointer, SnippetRemote,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};

macro_rules! with_provider_remote {
    ($store:expr, $settings:ident, $options:expr, $label:expr, $remote:ident, $caches:ident, $body:block) => {{
        let fresh = $store
            .request_fn(StoreDomain::CloudSync, |store| {
                store.load_cloud_sync_settings()
            })
            .map_err(|_| CloudSyncError::CredentialPersistence)?;
        let mut expected = $settings.clone();
        let mut pending = expected.clone();
        nyaterm_core::cloud_sync::credentials::rebase_rotated_tokens(
            &mut expected,
            &mut pending,
            &fresh,
        );
        if expected != fresh {
            return Err(CloudSyncError::CredentialsChanged);
        }
        let $settings = &fresh;
        let mut io_scope = super::io_runtime::IoScope::new($label);
        let io = super::io_runtime::CloudSyncIoRuntime::shared();
        let $caches = io.caches($store, $settings, $options)?;

        let token_store = $store.clone();
        let token_sink: crate::http::cloud_sync::CloudTokenSink =
            std::sync::Arc::new(move |update| {
                let result = token_store
                    .request_fn(StoreDomain::CloudSync, move |store| {
                        store.update_cloud_credentials(&update)
                    })
                    .map_err(|_| CloudSyncError::CredentialPersistence)?;
                match result {
                    nyaterm_core::cloud_sync::credentials::CredentialUpdateResult::Applied => {
                        Ok(())
                    }
                    nyaterm_core::cloud_sync::credentials::CredentialUpdateResult::Superseded => {
                        Err(CloudSyncError::CredentialsChanged)
                    }
                }
            });
        let result = (|| {
            let native: Box<dyn CloudSyncRemote> = match $settings.provider.as_str() {
                "webdav" => Box::new(NativeWebdavRemote::new(&$settings.webdav)?),
                "s3" => Box::new(NativeS3Remote::new_with_stats(
                    &$settings.s3,
                    io_scope.stats.clone(),
                )?),
                "google_drive" => Box::new(NativeGoogleDriveRemote::new_with_token_sink(
                    &$settings.google_drive,
                    token_sink.clone(),
                )?),
                "onedrive" => Box::new(NativeOneDriveRemote::new_with_token_sink(
                    &$settings.onedrive,
                    token_sink.clone(),
                )?),
                "aliyun_drive" => Box::new(NativeAliyunDriveRemote::new_with_token_sink(
                    &$settings.aliyun_drive,
                    token_sink.clone(),
                )?),
                "gitee_snippet" => Box::new(SnippetRemote::new(
                    "gitee_snippet",
                    GiteeSnippetHttpBackend::new(
                        &$settings.gitee_snippet,
                        NativeSnippetHttpClient::new()?,
                    )?,
                )),
                "github_gist" => Box::new(SnippetRemote::new(
                    "github_gist",
                    GithubGistHttpBackend::new(
                        &$settings.github_gist,
                        NativeSnippetHttpClient::new()?,
                    )?,
                )),
                provider => {
                    return Err(CloudSyncError::Remote(format!(
                        "native cloud provider '{provider}' is not wired yet"
                    )));
                }
            };
            let observed = super::io_runtime::ObservedRemote {
                inner: native.as_ref(),
                stats: io_scope.stats.clone(),
                heads_checked: std::cell::Cell::new(false),
            };
            let $remote = nyaterm_core::cloud_sync::metadata_cache::CachedMetadataRemote::new(
                &observed,
                &$caches.metadata,
                &$settings.remote_root,
            );
            let run = || $body;
            let result = run();
            if result.is_ok() && matches!($label, "push" | "pull" | "resolve" | "recover") {
                $caches
                    .pending_remote
                    .store(false, std::sync::atomic::Ordering::Release);
            }
            if (result.is_ok() || matches!(&result, Err(CloudSyncError::Conflict(_))))
                && observed.heads_checked.get()
                && !matches!($label, "cleanup" | "connection_test")
            {
                io.checked();
            }
            if result.is_err()
                && ($label != "check" || !matches!(&result, Err(CloudSyncError::Conflict(_))))
            {
                *$caches
                    .validation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Default::default();
                $caches
                    .pending_remote
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            result
        })();
        io_scope.finish(&result);
        result
    }};
}

pub(super) fn with_remote<T>(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    label: &'static str,
    operation: impl FnOnce(
        &dyn CloudSyncRemote,
        &super::io_runtime::IoCaches,
    ) -> Result<T, CloudSyncError>,
) -> Result<T, CloudSyncError> {
    if settings.protocol_version != 3 && !label.starts_with("v4_") && label != "connection_test" {
        return Err(CloudSyncError::Remote(
            "Legacy sync writing is disabled for this protocol".into(),
        ));
    }
    if settings.provider == "local_directory" {
        let remote = nyaterm_core::LocalDirectoryRemote::new(options.remote_dir.clone());
        let caches = super::io_runtime::CloudSyncIoRuntime::shared().caches(
            local_store,
            settings,
            Some(options),
        )?;
        return operation(&remote, &caches);
    }
    with_provider_remote!(
        local_store,
        settings,
        Some(options),
        label,
        remote,
        caches,
        { operation(&remote, &caches) }
    )
}

pub(crate) fn check_provider_snapshot(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    control: &CloudSyncOperationControl,
) -> Result<cloud_sync::CloudSyncCheck, CloudSyncError> {
    control.checkpoint()?;
    with_remote(local_store, settings, options, "check", |remote, caches| {
        let mut cache = caches
            .validation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = cloud_sync::check_snapshot_with_remote_cached(
            &control.local_store(local_store),
            options,
            &control.remote(remote),
            state,
            settings.auto_pull_remote_changes,
            &mut cache,
        );
        use std::sync::atomic::Ordering;
        match &result {
            Ok(check) => caches.pending_remote.store(
                matches!(
                    check.decision,
                    nyaterm_core::CloudRemoteCheckDecision::AutoPull
                        | nyaterm_core::CloudRemoteCheckDecision::RemoteAvailable
                        | nyaterm_core::CloudRemoteCheckDecision::Conflict
                ),
                Ordering::Release,
            ),
            Err(_) => caches.pending_remote.store(true, Ordering::Release),
        }
        result
    })
}

pub(crate) fn push_checked_provider_snapshot(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    checked: cloud_sync::CloudSyncCheck,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    with_remote(store, settings, options, "push", |remote, _| {
        checked.push(store, options, remote, control)
    })
}

pub(crate) fn pull_checked_provider_snapshot(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    checked: cloud_sync::CloudSyncCheck,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    with_remote(store, settings, options, "pull", |remote, _| {
        checked.pull(store, options, remote, before_apply, control)
    })
}

pub(in crate::features) fn test_provider_connection(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    control: &CloudSyncOperationControl,
) -> Result<(), CloudSyncError> {
    control.checkpoint()?;
    with_provider_remote!(
        local_store,
        settings,
        None,
        "connection_test",
        remote,
        caches,
        {
            cloud_sync::load_sync_pointer_from_remote(
                &control.local_store(local_store),
                &control.remote(&remote),
                &settings.remote_root,
            )?;
            control.checkpoint()
        }
    )
}

pub(crate) fn push_provider_snapshot(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    with_remote(local_store, settings, options, "push", |remote, _| {
        cloud_sync::push_snapshot_with_remote_controlled(
            local_store,
            options,
            remote,
            state,
            force,
            control,
        )
    })
}

pub(crate) fn pull_provider_snapshot(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    pull_provider_snapshot_guarded(
        local_store,
        settings,
        options,
        state,
        force,
        &|| Ok(()),
        control,
    )
}

pub(crate) fn pull_provider_snapshot_guarded(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    with_remote(local_store, settings, options, "pull", |remote, _| {
        cloud_sync::pull_snapshot_with_remote_controlled(
            local_store,
            options,
            remote,
            state,
            force,
            before_apply,
            control,
        )
    })
}

pub(in crate::features) fn recover_provider_snapshot(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    with_remote(local_store, settings, options, "recover", |remote, _| {
        cloud_sync::recover_current_snapshot_with_remote_controlled(
            local_store,
            options,
            remote,
            control,
        )
    })
}

pub(in crate::features) fn cleanup_provider_snapshots(
    local_store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    latest: Option<&RemoteSyncPointer>,
    control: &CloudSyncOperationControl,
) -> Result<(), CloudSyncError> {
    control.checkpoint()?;
    with_remote(local_store, settings, options, "cleanup", |remote, _| {
        cloud_sync::cleanup_sync_snapshots_with_remote_controlled(
            local_store,
            options,
            remote,
            latest,
            control,
        )
    })
}

pub(in crate::features) fn resolve_provider_conflict(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    request: &nyaterm_core::cloud_sync::conflict::CloudResolutionRequest,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    with_remote(store, settings, options, "resolve", |remote, _| {
        nyaterm_core::cloud_sync::conflict::resolve_with_remote(
            store,
            options,
            remote,
            state,
            request,
            &|| Ok(()),
            control,
        )
    })
}

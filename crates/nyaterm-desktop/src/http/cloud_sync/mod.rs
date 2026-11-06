//! Compatibility facade for native cloud-storage adapters.
pub(crate) use nyaterm_cloud_storage::auth::run_github_gist_device_flow;
pub(crate) use nyaterm_cloud_storage::{CloudTokenSink, OpenDalDriveRemote, OpenDalRemote};
pub use nyaterm_cloud_storage::{
    NativeAliyunDriveRemote, NativeGoogleDriveRemote, NativeOneDriveRemote, NativeS3Remote,
    NativeSnippetHttpClient, NativeWebdavRemote,
};

pub(crate) mod metrics {
    pub(crate) use nyaterm_cloud_storage::metrics::{begin_scope, end_scope};
    #[cfg(test)]
    pub(crate) use nyaterm_cloud_storage::test_support::with_test_drive_http;
}

#[cfg(test)]
pub(crate) use nyaterm_cloud_storage::test_support::{CloudStorageFixture, DriveFixture};

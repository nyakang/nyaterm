//! UI-independent native cloud-storage adapters.

mod aliyun;
pub mod auth;
mod google_drive;
mod helpers;
mod listing;
pub mod metrics;
mod onedrive;
mod opendal;
mod s3;
mod snippet;
mod webdav;

pub use aliyun::NativeAliyunDriveRemote;
pub use google_drive::NativeGoogleDriveRemote;
pub use onedrive::NativeOneDriveRemote;
pub use opendal::{OpenDalDriveRemote, OpenDalRemote};
pub use s3::NativeS3Remote;
pub use snippet::NativeSnippetHttpClient;
pub use webdav::NativeWebdavRemote;

#[cfg(test)]
mod tests;

pub type CloudTokenSink = std::sync::Arc<
    dyn Fn(
            nyaterm_core::cloud_sync::credentials::CloudCredentialUpdate,
        ) -> Result<(), nyaterm_core::CloudSyncError>
        + Send
        + Sync,
>;

mod thread;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

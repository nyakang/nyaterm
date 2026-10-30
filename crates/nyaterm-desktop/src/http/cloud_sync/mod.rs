mod aliyun;
mod github_gist_auth;
mod google_drive;
mod helpers;
mod listing;
pub(crate) mod metrics;
mod onedrive;
mod s3;
mod snippet;
mod webdav;

pub use aliyun::NativeAliyunDriveRemote;
pub(crate) use github_gist_auth::run_github_gist_device_flow;
pub use google_drive::NativeGoogleDriveRemote;
pub use onedrive::NativeOneDriveRemote;
pub use s3::NativeS3Remote;
pub use snippet::NativeSnippetHttpClient;
pub use webdav::NativeWebdavRemote;

#[cfg(test)]
mod tests;

pub(crate) type CloudTokenSink = std::sync::Arc<
    dyn Fn(
            nyaterm_core::cloud_sync::credentials::CloudCredentialUpdate,
        ) -> Result<(), nyaterm_core::CloudSyncError>
        + Send
        + Sync,
>;

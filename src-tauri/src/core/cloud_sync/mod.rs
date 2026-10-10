pub mod crypto;
mod gc;
mod github_gist_auth;
mod history_log;
mod manager;
mod migration;
mod operator;
mod protocol;
mod remote;
mod snapshot_decode_helper;

// Serialize tests that temporarily replace the process-wide master password.
#[cfg(test)]
static MASTER_PASSWORD_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use github_gist_auth::{
    GithubGistDeviceFlowPoll, GithubGistDeviceFlowStart, begin_github_gist_device_flow,
    cancel_github_gist_device_flow, poll_github_gist_device_flow,
};
pub use manager::{CloudSyncManager, notify_config_changed};
pub use snapshot_decode_helper::run_helper_if_requested as run_snapshot_decode_helper_if_requested;

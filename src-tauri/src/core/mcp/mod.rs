mod approval;
mod discovery;
mod host;
#[cfg(windows)]
mod windows_acl;
#[cfg(all(test, windows))]
pub(crate) use windows_acl::assert_current_user_only;
#[cfg(windows)]
pub(crate) use windows_acl::set_current_user_only;

pub use approval::ApprovalDecision;
pub use host::{EphemeralMcpCredential, McpClientConfigs, McpManager, McpRuntimeStatus};

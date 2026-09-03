mod approval;
mod discovery;
mod host;
#[cfg(windows)]
mod windows_acl;

pub use approval::ApprovalDecision;
pub use host::{EphemeralMcpCredential, McpClientConfigs, McpManager, McpRuntimeStatus};

mod catalog;
mod output_store;
mod policy;
mod recent_output;
mod scope;
pub(crate) mod sftp;
mod terminal;

pub use catalog::{CapabilityAccess, capability_for_tool};
pub use output_store::OutputStore;
pub use policy::{PolicyDecision, RiskAssessment, assess_command_risk, decide_policy};
pub use recent_output::RecentOutputStore;
pub use scope::{McpScope, McpScopeSnapshot};
pub use terminal::{
    TerminalExecuteRequest, TerminalExecutionPresentation, execute_terminal_command,
};

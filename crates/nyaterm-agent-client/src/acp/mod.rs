//! ACP v1 transport. Desktop owns jobs, terminal authorization, and credential lifetime.
pub mod discovery;
pub mod policy;
mod runtime;

pub use runtime::{AgentEvent, AgentOperation, AgentRunResult, run};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Agent configuration is invalid: {0}")]
    Configuration(&'static str),
    #[error("ACP adapter was not found; install it or select its executable")]
    Missing,
    #[error("This adapter or engine version has not passed the strict execution policy")]
    UnsupportedVersion,
    #[error(
        "ACP process-tree cleanup has only been validated on Windows; this platform is not enabled yet"
    )]
    UnsupportedPlatform,
    #[error("This Agent strategy has not passed the strict execution policy")]
    UnsupportedStrategy,
    #[error(
        "Codex ACP is not enabled for strict terminal execution: inherited MCP configuration has not passed acceptance; existing history is preserved"
    )]
    PolicyUnavailable,
    #[error("Agent startup or transport failed")]
    Transport,
    #[error("Agent protocol request failed ({0})")]
    Protocol(&'static str),
    #[error("The external session could not be restored; its saved history is unchanged")]
    Resume,
    #[error("AI request cancelled")]
    Cancelled,
    #[error("Agent request timed out")]
    Timeout,
    #[error("Agent reported a refusal or incomplete response")]
    Incomplete,
}

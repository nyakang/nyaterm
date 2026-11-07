//! Native agent control state and adapters around the shared capability contract.

mod decision;
mod registry;
mod run;
pub mod transcript;

pub use decision::{
    AgentBatchCall, AgentDecision, AgentResponseError, AgentToolBatch, AgentTurnRecord,
};
pub use registry::{AgentRequestPolicy, AgentTool, AgentToolCall, AgentToolRegistry};
pub use run::{
    AgentPlan, AgentPlanTask, AgentQuestion, AgentRequestContext, AgentRun, AgentRunStatus,
    AgentTaskStatus, AgentToolResult, AgentVerification, AgentVerificationStatus,
};

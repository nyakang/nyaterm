//! Transient presentation contracts; none of these types change saved AI history.

use nyaterm_core::ai::AiAgentStepStatus;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiResponsePhase {
    #[default]
    Ended,
    Waiting,
    Thinking,
    ToolArguments,
    Responding,
}

impl AiResponsePhase {
    pub fn shows_thinking(self) -> bool {
        matches!(self, Self::Waiting | Self::Thinking)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiAgentStepKind {
    Planning,
    ToolProgress,
    Command,
    Observation,
    FinalAnswer,
    Diagnostic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiCommandPhase {
    Suggested,
    Preparing,
    NeedsApproval,
    Running,
    Completed,
    Observed,
    Failed,
    Rejected,
    Cancelled,
    HistoryUnknown,
}

impl AiCommandPhase {
    pub fn from_step(step: &AiAgentStepView) -> Self {
        match step.status {
            AiAgentStepStatus::NeedsApproval => Self::NeedsApproval,
            AiAgentStepStatus::Running if step.kind == AiAgentStepKind::Command => Self::Running,
            AiAgentStepStatus::Running | AiAgentStepStatus::Planning | AiAgentStepStatus::Tool => {
                Self::Preparing
            }
            AiAgentStepStatus::Completed if step.exit_code.is_some_and(|code| code != 0) => {
                Self::Failed
            }
            AiAgentStepStatus::Completed if step.exit_code.is_none() => Self::Observed,
            AiAgentStepStatus::Completed => Self::Completed,
            AiAgentStepStatus::Failed => Self::Failed,
            AiAgentStepStatus::Rejected => Self::Rejected,
            AiAgentStepStatus::Cancelled => Self::Cancelled,
        }
    }

    pub fn offers_approval(self) -> bool {
        self == Self::NeedsApproval
    }
    pub fn offers_run(self) -> bool {
        self == Self::Suggested
    }
    pub fn offers_reuse(self) -> bool {
        !matches!(self, Self::NeedsApproval | Self::Preparing | Self::Running)
    }
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Suggested => "ai.commandSuggested",
            Self::Preparing => "ai.commandPreparing",
            Self::NeedsApproval => "ai.commandNeedsApproval",
            Self::Running => "ai.commandRunning",
            Self::Completed => "ai.commandCompleted",
            Self::Observed => "ai.commandObserved",
            Self::Failed => "ai.commandFailed",
            Self::Rejected => "ai.commandRejected",
            Self::Cancelled => "ai.commandCancelled",
            Self::HistoryUnknown => "ai.commandHistoryUnknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiAgentStepView {
    pub kind: AiAgentStepKind,
    pub source_message_id: Option<String>,
    pub command_card_id: Option<String>,
    pub exit_code: Option<i32>,
    pub step_index: u16,
    pub status: AiAgentStepStatus,
    pub title: String,
    /// Short summary line (Tauri duration / status meta).
    pub detail: String,
    /// Collapsible thought text (Tauri AgentStepView.thought).
    pub thought: Option<String>,
    /// Shell command body when the step is execute_command-like.
    pub command: Option<String>,
    /// Observation / terminal output snippet.
    pub observation: Option<String>,
}

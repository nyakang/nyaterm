//! Agent Activity summary projected from one execution group.
//!
//! The summary is derived on demand from the immutable panel snapshot; it owns
//! no state, so it can never disagree with the rows it summarizes.

use std::collections::HashSet;

use nyaterm_core::ai::harness::{AgentRunStatus, AgentTaskStatus};

use crate::features::ai::panel::AiPanelSnapshot;
use crate::features::ai::panel::execution::AiExecutionGroup;
use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
use crate::features::runtime_jobs::AiAgentStepStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiActivityItemState {
    Pending,
    Running,
    NeedsApproval,
    Completed,
    Failed,
    Skipped,
}

impl AiActivityItemState {
    fn finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }

    fn from_command_phase(phase: AiCommandPhase) -> Self {
        match phase {
            AiCommandPhase::Suggested => Self::Pending,
            AiCommandPhase::Preparing | AiCommandPhase::Running => Self::Running,
            AiCommandPhase::NeedsApproval => Self::NeedsApproval,
            AiCommandPhase::Completed | AiCommandPhase::Observed => Self::Completed,
            AiCommandPhase::Failed => Self::Failed,
            AiCommandPhase::Rejected
            | AiCommandPhase::Cancelled
            | AiCommandPhase::HistoryUnknown => Self::Skipped,
        }
    }

    fn from_step_status(status: AiAgentStepStatus) -> Self {
        match status {
            AiAgentStepStatus::Planning | AiAgentStepStatus::Running | AiAgentStepStatus::Tool => {
                Self::Running
            }
            AiAgentStepStatus::NeedsApproval => Self::NeedsApproval,
            AiAgentStepStatus::Completed => Self::Completed,
            AiAgentStepStatus::Failed => Self::Failed,
            AiAgentStepStatus::Rejected | AiAgentStepStatus::Cancelled => Self::Skipped,
        }
    }

    fn from_task_status(status: AgentTaskStatus) -> Self {
        match status {
            AgentTaskStatus::Pending => Self::Pending,
            AgentTaskStatus::InProgress => Self::Running,
            AgentTaskStatus::Completed => Self::Completed,
            AgentTaskStatus::Blocked => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AiActivityItem {
    pub label: String,
    pub state: AiActivityItemState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiActivityStatus {
    Running,
    NeedsApproval,
    WaitingUser,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AiActivitySummary {
    pub status: AiActivityStatus,
    pub completed: usize,
    pub total: usize,
    pub current_label: Option<String>,
    pub items: Vec<AiActivityItem>,
    /// True when the items come from the native agent plan rather than from
    /// the tool calls themselves.
    pub from_plan: bool,
}

impl AiActivitySummary {
    pub(super) fn project(snapshot: &AiPanelSnapshot, group: &AiExecutionGroup) -> Self {
        let live = group.is_live(snapshot);
        let plan = (group.messages.end == snapshot.messages.len())
            .then_some(snapshot.native_run.as_ref())
            .flatten()
            .filter(|view| !view.plan.tasks.is_empty());
        let items = match plan {
            Some(view) => view
                .plan
                .tasks
                .iter()
                .map(|task| AiActivityItem {
                    label: task.description.clone(),
                    state: AiActivityItemState::from_task_status(task.status),
                })
                .collect(),
            None => Self::tool_items(snapshot, group),
        };
        let mut summary = Self::from_items(&items, live);
        summary.from_plan = plan.is_some();
        if group.messages.end == snapshot.messages.len() {
            if snapshot
                .native_run
                .as_ref()
                .is_some_and(|view| view.status == AgentRunStatus::WaitingForUser)
            {
                summary.status = AiActivityStatus::WaitingUser;
            } else if !snapshot.native_approvals.is_empty() {
                // Native approvals belong to the newest request even before a tool card exists.
                summary.status = AiActivityStatus::NeedsApproval;
            }
        }
        summary
    }

    fn tool_items(snapshot: &AiPanelSnapshot, group: &AiExecutionGroup) -> Vec<AiActivityItem> {
        let mut seen = HashSet::new();
        let mut items: Vec<AiActivityItem> = snapshot.messages[group.messages.clone()]
            .iter()
            .flat_map(|message| &message.command_cards)
            .filter(|card| seen.insert(card.id.as_str()))
            .map(|card| AiActivityItem {
                label: if card.title.trim().is_empty() {
                    first_line(&card.command)
                } else {
                    card.title.trim().to_string()
                },
                state: AiActivityItemState::from_command_phase(snapshot.command_phase(card)),
            })
            .collect();
        items.extend(
            group
                .steps
                .iter()
                .map(|index| &snapshot.agent_steps[*index].step)
                .filter(|step| {
                    step.command_card_id.is_none()
                        && matches!(
                            step.kind,
                            AiAgentStepKind::ToolProgress
                                | AiAgentStepKind::Diagnostic
                                | AiAgentStepKind::Command
                                | AiAgentStepKind::Observation
                        )
                })
                .map(|step| AiActivityItem {
                    label: if step.title.trim().is_empty() {
                        step.command.as_deref().map(first_line).unwrap_or_default()
                    } else {
                        step.title.trim().to_string()
                    },
                    state: AiActivityItemState::from_step_status(step.status),
                }),
        );
        items
    }

    pub(super) fn from_items(items: &[AiActivityItem], live: bool) -> Self {
        let completed = items.iter().filter(|item| item.state.finished()).count();
        let waiting = items
            .iter()
            .any(|item| item.state == AiActivityItemState::NeedsApproval);
        let status = if waiting {
            AiActivityStatus::NeedsApproval
        } else if live {
            AiActivityStatus::Running
        } else if items
            .iter()
            .any(|item| item.state == AiActivityItemState::Failed)
        {
            AiActivityStatus::Failed
        } else {
            AiActivityStatus::Completed
        };
        let current_label = items
            .iter()
            .find(|item| {
                matches!(
                    item.state,
                    AiActivityItemState::NeedsApproval | AiActivityItemState::Running
                )
            })
            .or_else(|| {
                live.then(|| {
                    items
                        .iter()
                        .find(|item| item.state == AiActivityItemState::Pending)
                })
                .flatten()
            })
            .map(|item| item.label.clone())
            .filter(|label| !label.is_empty());
        Self {
            status,
            completed,
            total: items.len(),
            current_label,
            items: items.to_vec(),
            from_plan: false,
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{AiActivityItem, AiActivityItemState, AiActivityStatus, AiActivitySummary};

    fn item(label: &str, state: AiActivityItemState) -> AiActivityItem {
        AiActivityItem {
            label: label.into(),
            state,
        }
    }

    #[test]
    fn live_activity_counts_finished_items_and_names_the_running_one() {
        let summary = AiActivitySummary::from_items(
            &[
                item("check disk", AiActivityItemState::Completed),
                item("restart nginx", AiActivityItemState::Running),
                item("verify", AiActivityItemState::Pending),
            ],
            true,
        );
        assert_eq!(summary.status, AiActivityStatus::Running);
        assert_eq!((summary.completed, summary.total), (1, 3));
        assert_eq!(summary.current_label.as_deref(), Some("restart nginx"));
    }

    #[test]
    fn approval_wait_outranks_running_and_failed_items() {
        let summary = AiActivitySummary::from_items(
            &[
                item("probe", AiActivityItemState::Failed),
                item("rm -rf /tmp/cache", AiActivityItemState::NeedsApproval),
            ],
            true,
        );
        assert_eq!(summary.status, AiActivityStatus::NeedsApproval);
        assert_eq!(summary.current_label.as_deref(), Some("rm -rf /tmp/cache"));
    }

    #[test]
    fn finished_activity_reports_failure_without_a_current_label() {
        let summary = AiActivitySummary::from_items(
            &[
                item("probe", AiActivityItemState::Failed),
                item("skipped", AiActivityItemState::Skipped),
                item("later", AiActivityItemState::Pending),
            ],
            false,
        );
        assert_eq!(summary.status, AiActivityStatus::Failed);
        assert_eq!((summary.completed, summary.total), (2, 3));
        assert_eq!(summary.current_label, None);
    }

    #[test]
    fn empty_finished_activity_is_completed() {
        let summary = AiActivitySummary::from_items(&[], false);
        assert_eq!(summary.status, AiActivityStatus::Completed);
        assert_eq!(summary.total, 0);
    }

    #[test]
    fn live_activity_keeps_pending_items_out_of_the_finished_count() {
        let summary = AiActivitySummary::from_items(
            &[
                item("check disk", AiActivityItemState::Completed),
                item("read logs", AiActivityItemState::Pending),
            ],
            true,
        );
        assert_eq!(summary.status, AiActivityStatus::Running);
        assert_eq!(summary.items.len(), 2);
        assert_eq!(summary.current_label.as_deref(), Some("read logs"));
    }
}

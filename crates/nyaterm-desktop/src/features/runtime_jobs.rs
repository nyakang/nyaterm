use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use futures::channel::mpsc::{UnboundedReceiver, unbounded};

use nyaterm_core::{
    AiCommandCard, AiMode, AiModelDiscovery, CommandHistoryEntry, CommandObservation,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use nyaterm_transport::{
    DockerComposeService, DockerContainerDetails, RemoteDockerOverview, RemoteGpuOverview,
    RemoteNpuOverview, RemoteProcess, RemoteStats, SessionInfo, SessionKind, SshMultiplexHandle,
    SshSessionConfig, SshSessionStartHandle, SshTunnelInfo,
};

use crate::models::SessionLaunchConfig;

pub(in crate::features) struct SessionStartResult {
    pub(in crate::features) request_id: String,
    pub(in crate::features) connection_name: String,
    pub(in crate::features) kind: SessionKind,
    pub(in crate::features) worker_started_at: Instant,
    pub(in crate::features) worker_finished_at: Instant,
    pub(in crate::features) result: Result<SessionStartSuccess, String>,
}

pub(in crate::features) struct SessionStartSuccess {
    pub(in crate::features) session_info: SessionInfo,
    pub(in crate::features) multiplex_handle: Option<SshMultiplexHandle>,
    pub(in crate::features) ssh_start_handle: Option<SshSessionStartHandle>,
    pub(in crate::features) launch_config: Option<SessionLaunchConfig>,
}

#[derive(Debug)]
pub(in crate::features) struct TunnelJobResult {
    pub(in crate::features) tunnel_id: String,
    pub(in crate::features) result: Result<TunnelJobOutput, String>,
}

#[derive(Debug)]
pub(in crate::features) enum TunnelJobOutput {
    Opened(SshTunnelInfo),
    Closed,
}

#[derive(Debug)]
pub(in crate::features) struct ProcessJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<ProcessJobOutput, String>,
}

#[derive(Debug)]
pub(in crate::features) struct StatsJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<RemoteStats, String>,
}

#[derive(Debug)]
pub(in crate::features) struct GpuJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<RemoteGpuOverview, String>,
}

#[derive(Debug)]
pub(in crate::features) struct NpuJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<RemoteNpuOverview, String>,
}

#[derive(Debug)]
pub(in crate::features) enum CommandPersistenceRequest {
    AppendHistory(Vec<String>),
    IncrementQuickCommand(String),
}

#[derive(Debug)]
pub(in crate::features) enum CommandPersistenceResult {
    History(Result<Vec<CommandHistoryEntry>, String>),
    QuickCommandUseCount {
        command_id: String,
        result: Result<(), String>,
    },
}

#[derive(Debug)]
pub(in crate::features) struct DockerJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<DockerJobOutput, String>,
}

#[derive(Debug)]
pub(in crate::features) struct AiDiscoveryJobResult {
    pub(in crate::features) profile_id: String,
    pub(in crate::features) result: Result<Vec<AiModelDiscovery>, String>,
}

#[derive(Debug)]
pub(in crate::features) struct AiChatJobResult {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) session_id: String,
    pub(in crate::features) result: Result<AiChatJobOutput, String>,
}

#[derive(Debug)]
pub(in crate::features) enum AiChatWorkerEvent {
    Delta {
        job_id: u64,
        session_id: String,
        text_delta: String,
        reasoning_delta: Option<String>,
    },
    AgentToolCallDelta {
        job_id: u64,
        session_id: String,
        tool_name: Option<String>,
        arguments_delta_len: usize,
    },
    AgentBackgroundFinished {
        job_id: u64,
        state: AiAgentLoopState,
        result: Result<CommandObservation, String>,
    },
    Finished(AiChatJobResult),
}

#[derive(Debug)]
pub(in crate::features) struct AiChatJobOutput {
    pub(in crate::features) mode: AiMode,
    pub(in crate::features) text: String,
    pub(in crate::features) reasoning: Option<String>,
    pub(in crate::features) command_cards: Vec<AiCommandCard>,
    pub(in crate::features) auto_execute_first: bool,
    pub(in crate::features) approval_note: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::features) struct AiAgentLoopState {
    pub(in crate::features) ai_session_id: String,
    pub(in crate::features) terminal_session_id: String,
    pub(in crate::features) task_prompt: String,
    pub(in crate::features) command: String,
    pub(in crate::features) marker_id: Option<String>,
    pub(in crate::features) background_job_id: Option<u64>,
    pub(in crate::features) step_index: u16,
    pub(in crate::features) max_steps: u16,
    pub(in crate::features) output_start_len: usize,
    pub(in crate::features) started_at: Instant,
    pub(in crate::features) min_wait_until: Instant,
    pub(in crate::features) timeout_at: Instant,
    pub(in crate::features) last_seen_len: usize,
    pub(in crate::features) stable_since: Instant,
}

#[derive(Debug, Clone)]
pub(in crate::features) struct AiAgentStepView {
    pub(in crate::features) step_index: u16,
    pub(in crate::features) status: AiAgentStepStatus,
    pub(in crate::features) title: String,
    /// Short summary line (Tauri duration / status meta).
    pub(in crate::features) detail: String,
    /// Collapsible thought text (Tauri AgentStepView.thought).
    pub(in crate::features) thought: Option<String>,
    /// Shell command body when the step is execute_command-like.
    pub(in crate::features) command: Option<String>,
    /// Observation / terminal output snippet.
    pub(in crate::features) observation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum AiAgentStepStatus {
    Planning,
    Tool,
    NeedsApproval,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone)]
pub(in crate::features) enum AiAgentBackgroundTarget {
    Ssh(Box<SshSessionConfig>),
    Local { working_dir: Option<PathBuf> },
}

#[derive(Debug)]
pub(in crate::features) enum ProcessJobOutput {
    Listed(Vec<RemoteProcess>),
    Signalled {
        pid: u32,
        signal: String,
        processes: Vec<RemoteProcess>,
    },
    Reniced {
        pid: u32,
        nice: i32,
        processes: Vec<RemoteProcess>,
    },
}

#[derive(Debug)]
pub(in crate::features) enum DockerJobOutput {
    Overview(RemoteDockerOverview),
    Details {
        container_id: String,
        details: DockerContainerDetails,
    },
    ComposeServices {
        key: String,
        project_name: String,
        services: Vec<DockerComposeService>,
    },
    ComposeServiceAction {
        key: String,
        service_name: String,
        action: String,
        overview: RemoteDockerOverview,
        services: Vec<DockerComposeService>,
    },
    ComposeProjectAction {
        key: String,
        project_name: String,
        action: String,
        overview: RemoteDockerOverview,
        services: Option<Vec<DockerComposeService>>,
        service_error: Option<String>,
    },
    RefreshedAfterAction {
        label: String,
        overview: RemoteDockerOverview,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum ActivitySide {
    Left,
    Right,
}

pub(in crate::features) fn spawn_command_persistence_worker(
    store: StoreBlockingClient,
) -> (
    mpsc::Sender<CommandPersistenceRequest>,
    UnboundedReceiver<CommandPersistenceResult>,
) {
    // The request side stays a blocking channel: the worker thread parks on
    // `recv`. Only the result side is read from the GPUI thread.
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = unbounded();
    let _ = std::thread::Builder::new()
        .name("nyaterm-command-persistence".to_string())
        .spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let result = match request {
                    CommandPersistenceRequest::AppendHistory(commands) => {
                        CommandPersistenceResult::History(
                            store
                                .request_fn(StoreDomain::Commands, move |database| {
                                    for command in commands {
                                        database.append_command_history(&command)?;
                                    }
                                    database.list_command_history(64)
                                })
                                .map_err(|error| error.to_string()),
                        )
                    }
                    CommandPersistenceRequest::IncrementQuickCommand(command_id) => {
                        let persisted_id = command_id.clone();
                        let result = store
                            .request_fn(StoreDomain::Commands, move |database| {
                                database.increment_quick_command_use_count(&persisted_id)
                            })
                            .map_err(|error| error.to_string());
                        CommandPersistenceResult::QuickCommandUseCount { command_id, result }
                    }
                };
                if result_tx.unbounded_send(result).is_err() {
                    break;
                }
            }
        });
    (request_tx, result_rx)
}

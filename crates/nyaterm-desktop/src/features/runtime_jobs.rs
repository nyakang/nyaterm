use std::sync::Arc;
use std::time::Instant;

use futures::channel::mpsc::UnboundedSender;

use nyaterm_core::{AiCommandCard, AiMode, AiModelDiscovery, CommandHistoryEntry};
use nyaterm_transport::{
    DockerComposeProject, DockerComposeService, DockerContainerDetails, RemoteDockerOverview,
    RemoteGpuOverview, RemoteNpuOverview, RemoteProcess, RemoteStats, SessionInfo, SessionKind,
    SessionManager, SshTunnelInfo,
};

use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;
use crate::blocking_jobs::{BlockingJobScheduler, JobRejected, JobTask};
use crate::models::SessionLaunchConfig;

pub(in crate::features) async fn await_blocking_job<T>(
    task: Result<JobTask<T>, JobRejected>,
) -> Result<T, String> {
    match task {
        Ok(task) => task.await.map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

pub(in crate::features) async fn await_blocking_result<T, E: std::fmt::Display>(
    task: Result<JobTask<Result<T, E>>, JobRejected>,
) -> Result<T, String> {
    await_blocking_job(task)
        .await
        .and_then(|result| result.map_err(|error| error.to_string()))
}

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
    pub(in crate::features) multiplex_handle: Option<SshConnectionLease>,
    pub(in crate::features) launch_config: Option<SessionLaunchConfig>,
    pub(in crate::features) cleanup: Option<PendingSshSession>,
}

/// The worker's session is not owned by a window until its result is accepted.
/// Dropping a cancelled or undeliverable result closes only that channel.
pub(in crate::features) struct PendingSshSession {
    manager: Arc<SessionManager>,
    session_id: Option<String>,
    connection: SshConnectionLease,
}

impl SessionStartSuccess {
    pub(in crate::features) fn ssh(
        manager: Arc<SessionManager>,
        session_info: SessionInfo,
        connection: SshConnectionLease,
        launch_config: Option<SessionLaunchConfig>,
    ) -> Self {
        let cleanup = PendingSshSession {
            manager,
            session_id: Some(session_info.id.clone()),
            connection: connection.clone(),
        };
        Self {
            session_info,
            multiplex_handle: Some(connection),
            launch_config,
            cleanup: Some(cleanup),
        }
    }

    pub(in crate::features) fn accept(&mut self) {
        if let Some(cleanup) = self.cleanup.as_mut() {
            cleanup.session_id = None;
        }
    }
}

impl Drop for PendingSshSession {
    fn drop(&mut self) {
        if let Some(session_id) = self.session_id.take() {
            let manager = Arc::clone(&self.manager);
            let connection = self.connection.clone();
            self.connection.enqueue_cleanup(move || {
                let _ = manager.close(&session_id);
                drop(connection);
            });
        }
    }
}

pub(in crate::features) fn submit_session_start_job(
    scheduler: &BlockingJobScheduler,
    name: &'static str,
    request_id: String,
    connection_name: String,
    kind: SessionKind,
    tx: UnboundedSender<SessionStartResult>,
    run: impl FnOnce() -> Result<SessionStartSuccess, String> + Send + 'static,
) {
    let rejected_tx = tx.clone();
    let rejected_request_id = request_id.clone();
    let rejected_connection_name = connection_name.clone();
    if let Err(error) = scheduler.submit_detached(name, move |_| {
        let worker_started_at = Instant::now();
        let result = run();
        let worker_finished_at = Instant::now();
        let _ = tx.unbounded_send(SessionStartResult {
            request_id,
            connection_name,
            kind,
            worker_started_at,
            worker_finished_at,
            result,
        });
    }) {
        let now = Instant::now();
        let _ = rejected_tx.unbounded_send(SessionStartResult {
            request_id: rejected_request_id,
            connection_name: rejected_connection_name,
            kind,
            worker_started_at: now,
            worker_finished_at: now,
            result: Err(error.to_string()),
        });
    }
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
    pub(in crate::features) users_revision: u64,
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
    AgentProtocolFallback {
        job_id: u64,
        session_id: String,
    },
    Finished(Box<AiChatJobResult>),
}

impl AiChatWorkerEvent {
    pub(in crate::features) fn session_id(&self) -> &str {
        match self {
            Self::Delta { session_id, .. }
            | Self::AgentToolCallDelta { session_id, .. }
            | Self::AgentProtocolFallback { session_id, .. } => session_id,
            Self::Finished(result) => &result.session_id,
        }
    }
}

#[derive(Debug)]
pub(in crate::features) struct AiChatJobOutput {
    pub(in crate::features) native_call: Option<nyaterm_core::ai::harness::AgentToolCall>,
    pub(in crate::features) mode: AiMode,
    pub(in crate::features) text: String,
    pub(in crate::features) reasoning: Option<String>,
    pub(in crate::features) command_cards: Vec<AiCommandCard>,
    pub(in crate::features) auto_execute_first: bool,
    pub(in crate::features) approval_note: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::features) struct AiAgentLoopState {
    pub(in crate::features) execution_id: String,
    pub(in crate::features) command_card_id: Option<String>,
    pub(in crate::features) ai_session_id: String,
    pub(in crate::features) terminal_session_id: String,
    pub(in crate::features) available_targets: Vec<nyaterm_core::AiTerminalTarget>,
    pub(in crate::features) default_target_session_id: Option<String>,
    pub(in crate::features) command: String,
    pub(in crate::features) step_index: u16,
    pub(in crate::features) max_steps: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct AiAgentStepView {
    pub(in crate::features) kind: crate::features::ai::presentation::AiAgentStepKind,
    pub(in crate::features) source_message_id: Option<String>,
    pub(in crate::features) command_card_id: Option<String>,
    pub(in crate::features) exit_code: Option<i32>,
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

pub(in crate::features) use nyaterm_core::ai::AiAgentStepStatus;

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
    Overview {
        overview: RemoteDockerOverview,
        resource: Option<Result<DockerResource, String>>,
    },
    Resource(DockerResource),
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

#[derive(Debug)]
pub(in crate::features) enum DockerResource {
    Images(Vec<nyaterm_transport::DockerImage>),
    Volumes(Vec<nyaterm_transport::DockerVolume>),
    Networks(Vec<nyaterm_transport::DockerNetwork>),
    Compose(Vec<DockerComposeProject>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum ActivitySide {
    Left,
    Right,
}

#[cfg(test)]
mod session_start_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    use futures::channel::mpsc::unbounded;
    use nyaterm_transport::{LocalSessionConfig, SessionInfo, SessionKind, SessionManager};

    use super::{SessionStartResult, SessionStartSuccess};
    use crate::app_shell::session_hub::ssh_connections::SshConnectionPool;

    fn live_channel(manager: &SessionManager) -> SessionInfo {
        manager
            .create_local_session(LocalSessionConfig {
                name: "pending-ssh-channel-cleanup".into(),
                shell_path: Some(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into()),
                ..LocalSessionConfig::default()
            })
            .expect("create channel for cleanup test")
    }

    #[test]
    fn discarded_reused_start_closes_its_channel_and_preserves_other_connection_leases() {
        let manager = Arc::new(SessionManager::new());
        let first = live_channel(&manager);
        let pending = live_channel(&manager);
        let pool = SshConnectionPool::default();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&disconnected);
        let connection = pool.register_for_test("shared", move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let result =
            SessionStartSuccess::ssh(Arc::clone(&manager), pending, connection.clone(), None);
        drop(result);
        pool.finish_disconnects();
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, first.id);
        assert_eq!(disconnected.load(Ordering::SeqCst), 0);
        manager.close(&first.id).unwrap();
        drop(connection);
        pool.finish_disconnects();
        assert_eq!(disconnected.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn undeliverable_fresh_start_closes_channel_and_disconnects_unowned_connection() {
        let manager = Arc::new(SessionManager::new());
        let pending = live_channel(&manager);
        let pool = SshConnectionPool::default();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&disconnected);
        let connection = pool.register_for_test("fresh", move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let result = SessionStartSuccess::ssh(Arc::clone(&manager), pending, connection, None);
        let (sender, receiver) = unbounded();
        drop(receiver);
        let now = Instant::now();
        assert!(
            sender
                .unbounded_send(SessionStartResult {
                    request_id: "request".into(),
                    connection_name: "test".into(),
                    kind: SessionKind::Ssh,
                    worker_started_at: now,
                    worker_finished_at: now,
                    result: Ok(result),
                })
                .is_err()
        );
        pool.finish_disconnects();
        assert!(manager.list_sessions().unwrap().is_empty());
        assert_eq!(disconnected.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn accepted_start_relinquishes_pending_cleanup_to_session_owner() {
        let manager = Arc::new(SessionManager::new());
        let pending = live_channel(&manager);
        let id = pending.id.clone();
        let pool = SshConnectionPool::default();
        let connection = pool.register_for_test("accepted", || {});
        let mut result =
            SessionStartSuccess::ssh(Arc::clone(&manager), pending, connection.clone(), None);
        result.accept();
        drop(result);
        pool.finish_disconnects();
        assert_eq!(manager.list_sessions().unwrap()[0].id, id);
        manager.close(&id).unwrap();
        drop(connection);
        pool.finish_disconnects();
    }
}

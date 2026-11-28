//! ACP account inspection runs on one owned worker. UI consumes generation-tagged snapshots.
use super::helper_resolver::resolve_mcp_helper;
use crate::thread_owner::spawn_joinable;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_agent_client::acp::{AgentError, AgentEvent, AgentOperation};
use nyaterm_core::ai::external_agents::ExternalAgentProfile;
use nyaterm_store::StoreBlockingClient;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum AcpProfileProblem {
    Missing,
    Version,
    Platform,
    Strategy,
    Configuration,
    Preparation,
    Policy,
    Timeout,
    Startup,
    Protocol,
    Cancelled,
}

impl From<AgentError> for AcpProfileProblem {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Missing => Self::Missing,
            AgentError::UnsupportedVersion => Self::Version,
            AgentError::UnsupportedPlatform => Self::Platform,
            AgentError::UnsupportedStrategy => Self::Strategy,
            AgentError::Configuration(_) => Self::Configuration,
            AgentError::PolicyUnavailable => Self::Policy,
            AgentError::Timeout => Self::Timeout,
            AgentError::Transport => Self::Startup,
            AgentError::Cancelled => Self::Cancelled,
            AgentError::Protocol(_) | AgentError::Resume | AgentError::Incomplete => Self::Protocol,
        }
    }
}

impl AcpProfileProblem {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Missing => "ai.acp.missingAdapter",
            Self::Version => "ai.acp.versionMismatch",
            Self::Platform => "ai.acp.platformUnavailable",
            Self::Strategy => "ai.acp.strategyUnavailable",
            Self::Configuration => "ai.acp.invalidProfile",
            Self::Preparation => "ai.acp.preparationFailed",
            Self::Policy => "ai.acp.policyUnavailable",
            Self::Timeout => "ai.acp.checkTimedOut",
            Self::Startup => "ai.acp.startupFailed",
            Self::Protocol => "ai.acp.protocolFailed",
            Self::Cancelled => "ai.acp.notChecked",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct AcpProfileStatus {
    pub authenticated: Option<bool>,
    pub auth_methods: Vec<(String, String)>,
    pub logout: bool,
    pub problem: Option<AcpProfileProblem>,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct AgentManagementView {
    pub profiles: BTreeMap<String, AcpProfileStatus>,
    pub mcp_helper_path: Option<PathBuf>,
    pub pending: bool,
    pub active_profile: Option<String>,
    pub helper_checked: bool,
    pub error: Option<String>,
}
pub(in crate::features) enum AgentCommand {
    Acp {
        profile: Box<ExternalAgentProfile>,
        operation: AgentOperation,
    },
    Refresh(Vec<ExternalAgentProfile>),
}
pub(in crate::features) struct AgentJobEvent {
    generation: u64,
    profiles: BTreeMap<String, AcpProfileStatus>,
    helper: Option<PathBuf>,
    complete: bool,
    active_profile: Option<String>,
}
type CommandEnvelope = (u64, AgentCommand, Arc<AtomicBool>);
pub(in crate::features) struct AgentManagementState {
    view: AgentManagementView,
    generation: u64,
    commands: Option<mpsc::SyncSender<CommandEnvelope>>,
    events: Option<UnboundedReceiver<AgentJobEvent>>,
    worker: Option<JoinHandle<()>>,
    cleanup: Option<crate::thread_owner::BackgroundCleanup>,
    active_request: Arc<AtomicBool>,
}
impl AgentManagementState {
    pub fn new() -> Self {
        Self {
            view: Default::default(),
            generation: 0,
            commands: None,
            events: None,
            worker: None,
            cleanup: None,
            active_request: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn view(&self) -> &AgentManagementView {
        &self.view
    }
    pub fn set_error(&mut self, error: String) {
        self.view.error = Some(error);
    }
    pub fn submit(
        &mut self,
        command: AgentCommand,
        store: StoreBlockingClient,
        executor: gpui::BackgroundExecutor,
    ) -> bool {
        self.cleanup
            .get_or_insert_with(|| crate::thread_owner::BackgroundCleanup::new(executor));
        // Never queue a second account operation behind an interactive authentication.
        if self.view.pending {
            return false;
        }
        self.active_request.store(true, Ordering::Release);
        self.active_request = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
        if self.commands.is_none() {
            let (tx, rx) = mpsc::sync_channel(1);
            let (events, incoming) = unbounded();
            match spawn_joinable("nyaterm-acp-settings", move || {
                run_worker(rx, events, store)
            }) {
                Ok(worker) => {
                    self.commands = Some(tx);
                    self.events = Some(incoming);
                    self.worker = Some(worker);
                }
                Err(_) => {
                    self.set_error(rust_i18n::t!("ai.acp.startupFailed").into());
                    return false;
                }
            }
        }
        self.view.error = None;
        self.view.active_profile = match &command {
            AgentCommand::Acp { profile, .. } => Some(profile.id.clone()),
            AgentCommand::Refresh(_) => None,
        };
        self.view.pending = self.commands.as_ref().is_some_and(|tx| {
            tx.try_send((self.generation, command, self.active_request.clone()))
                .is_ok()
        });
        if !self.view.pending {
            self.set_error(rust_i18n::t!("ai.acp.startupFailed").into());
        }
        self.view.pending
    }
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<AgentJobEvent>> {
        self.events.take()
    }
    pub fn cancel_pending(&mut self) {
        self.active_request.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.view.pending = false;
        self.view.active_profile = None;
        self.view.error = None;
    }
    pub fn invalidate(&mut self) {
        self.cancel_pending();
        self.view.profiles.clear();
    }
    pub fn invalidate_profile(&mut self, id: &str) {
        self.cancel_pending();
        self.view.profiles.remove(id);
    }
    pub fn apply_job(&mut self, event: AgentJobEvent) -> Option<String> {
        if event.generation == self.generation {
            self.view.pending = !event.complete;
            self.view.active_profile = event.active_profile;
            self.view.profiles.extend(event.profiles);
            self.view.mcp_helper_path = event.helper;
            self.view.helper_checked = true;
        }
        None
    }
    pub(in crate::features) fn begin_shutdown(&mut self) -> Option<JoinHandle<()>> {
        self.active_request.store(true, Ordering::Release);
        self.commands.take();
        self.worker.take()
    }
}
impl Drop for AgentManagementState {
    fn drop(&mut self) {
        // Normal application shutdown transfers this handle to its background reaper.
        if let Some(worker) = self.begin_shutdown() {
            self.cleanup
                .as_ref()
                .expect("worker has background cleanup")
                .submit(move || {
                    let _ = worker.join();
                });
        }
    }
}
fn inspect(
    store: &StoreBlockingClient,
    profile: &ExternalAgentProfile,
    operation: AgentOperation,
    cancel: &AtomicBool,
) -> AcpProfileStatus {
    let mut status = AcpProfileStatus::default();
    let profile = match super::acp_runtime::prepare_profile(store, profile.clone()) {
        Ok(profile) => profile,
        Err(_) => {
            status.problem = Some(AcpProfileProblem::Preparation);
            return status;
        }
    };
    let refresh = !matches!(operation, AgentOperation::Inspect);
    let mut sink = |event| {
        match event {
            AgentEvent::Capabilities {
                auth_methods,
                logout,
            } => {
                status.auth_methods = auth_methods;
                status.logout = logout;
            }
            AgentEvent::Authentication { authenticated } => status.authenticated = authenticated,
            _ => {}
        }
        Ok(())
    };
    let result = nyaterm_agent_client::acp::run(
        &profile,
        operation,
        None,
        Default::default(),
        cancel,
        Duration::from_secs(60),
        &mut sink,
        &mut || {},
    );
    let result = if result.is_ok() && refresh {
        nyaterm_agent_client::acp::run(
            &profile,
            AgentOperation::Inspect,
            None,
            Default::default(),
            cancel,
            Duration::from_secs(20),
            &mut sink,
            &mut || {},
        )
    } else {
        result
    };
    status.problem = result.err().map(AcpProfileProblem::from);
    status
}
fn run_worker(
    commands: mpsc::Receiver<CommandEnvelope>,
    events: UnboundedSender<AgentJobEvent>,
    store: StoreBlockingClient,
) {
    while let Ok((generation, command, cancel)) = commands.recv() {
        if cancel.load(Ordering::Acquire) {
            continue;
        }
        let helper = resolve_mcp_helper().ok();
        let operations = match command {
            AgentCommand::Acp { profile, operation } => vec![(*profile, operation)],
            AgentCommand::Refresh(profiles) => profiles
                .into_iter()
                .map(|profile| (profile, AgentOperation::Inspect))
                .collect(),
        };
        for (profile, operation) in operations {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            if events
                .unbounded_send(AgentJobEvent {
                    generation,
                    profiles: BTreeMap::new(),
                    helper: helper.clone(),
                    complete: false,
                    active_profile: Some(profile.id.clone()),
                })
                .is_err()
            {
                return;
            }
            let status = inspect(&store, &profile, operation, &cancel);
            if events
                .unbounded_send(AgentJobEvent {
                    generation,
                    profiles: BTreeMap::from([(profile.id, status)]),
                    helper: helper.clone(),
                    complete: false,
                    active_profile: None,
                })
                .is_err()
            {
                return;
            }
        }
        if events
            .unbounded_send(AgentJobEvent {
                generation,
                profiles: BTreeMap::new(),
                helper,
                complete: true,
                active_profile: None,
            })
            .is_err()
        {
            break;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{AcpProfileStatus, AgentJobEvent, AgentManagementState};
    #[test]
    fn progressive_inspections_keep_completed_profiles_and_reject_cancelled_results() {
        let mut state = AgentManagementState::new();
        state.view.pending = true;
        let generation = state.generation;
        state.apply_job(AgentJobEvent {
            generation,
            profiles: std::collections::BTreeMap::from([(
                "first".into(),
                AcpProfileStatus {
                    authenticated: Some(true),
                    ..Default::default()
                },
            )]),
            helper: Some("fixture-helper".into()),
            complete: false,
            active_profile: Some("second".into()),
        });
        assert!(state.view.pending);
        assert!(state.view.helper_checked);
        assert_eq!(state.view.active_profile.as_deref(), Some("second"));
        state.invalidate_profile("second");
        assert_eq!(state.view.profiles["first"].authenticated, Some(true));
        state.apply_job(AgentJobEvent {
            generation,
            profiles: std::collections::BTreeMap::from([(
                "second".into(),
                AcpProfileStatus::default(),
            )]),
            helper: None,
            complete: true,
            active_profile: None,
        });
        assert!(!state.view.pending);
        assert!(!state.view.profiles.contains_key("second"));
        assert!(state.view.mcp_helper_path.is_some());
    }

    #[test]
    fn helper_is_not_reported_missing_before_the_first_completed_probe() {
        let mut state = AgentManagementState::new();
        assert!(!state.view.helper_checked);
        state.apply_job(AgentJobEvent {
            generation: state.generation,
            profiles: Default::default(),
            helper: None,
            complete: true,
            active_profile: None,
        });
        assert!(state.view.helper_checked);
        assert!(state.view.mcp_helper_path.is_none());
    }

    #[test]
    fn editing_or_deleting_a_profile_cancels_inspection_and_rejects_stale_account_state() {
        let mut state = AgentManagementState::new();
        let generation = state.generation;
        let cancel = state.active_request.clone();
        state.invalidate();
        state.apply_job(AgentJobEvent {
            generation,
            profiles: std::collections::BTreeMap::from([(
                "deleted".into(),
                AcpProfileStatus {
                    authenticated: Some(true),
                    ..Default::default()
                },
            )]),
            helper: None,
            complete: true,
            active_profile: None,
        });
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(state.view().profiles.is_empty());
        assert!(!state.view().pending);
    }
}

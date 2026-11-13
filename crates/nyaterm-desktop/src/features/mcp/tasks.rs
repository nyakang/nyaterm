#[derive(Default)]
pub(in crate::features) struct BackgroundTaskPresentation {
    pub open: bool,
    pub selected: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct BackgroundTasksView {
    pub open: bool,
    pub jobs: Vec<BackgroundJobView>,
    pub selected: Option<BackgroundJobDetails>,
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::features) struct BackgroundJobView {
    pub id: String,
    pub host: String,
    pub command: String,
    pub client: String,
    pub started_at_ms: u64,
    pub status: nyaterm_mcp_protocol::TerminalJobStatus,
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::features) struct BackgroundJobDetails {
    pub id: String,
    pub connection_id: String,
    pub output: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub process_state: nyaterm_mcp_protocol::TerminalProcessState,
    pub stop_state: String,
    pub can_stop: bool,
}

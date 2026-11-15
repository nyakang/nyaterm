use nyaterm_core::completion::types::{
    CompletionHistorySnapshot, CompletionHistoryUpdate, ShellCommandContext,
};
use nyaterm_core::models::quick_commands::CommandHistoryEntry;
use std::sync::{Arc, atomic::AtomicU64};

pub struct HistorySnapshot {
    pub history: CompletionHistorySnapshot,
    pub generation: u64,
}

pub struct HistoryAppend {
    pub before: u64,
    pub after: u64,
    pub update: Option<CompletionHistoryUpdate>,
    pub entries: Vec<CommandHistoryEntry>,
    pub warning: Option<String>,
}

/// Blocking operations called only by the completion worker.
/// Each snapshot/append/delete is one repository request. In particular, generation
/// must be captured before the snapshot in that same request, never read separately.
pub trait CompletionHistoryRepository: Send + Sync {
    /// A shared invalidation counter. Consumers only load it; the repository owns writes.
    fn generation(&self) -> Result<Arc<AtomicU64>, String>;
    fn snapshot(&self) -> Result<HistorySnapshot, String>;
    fn append(
        &self,
        context: ShellCommandContext,
        command: String,
        include_global: bool,
    ) -> Result<HistoryAppend, String>;
    fn delete(&self, command: String) -> Result<Vec<CommandHistoryEntry>, String>;
}

pub enum CompletionEvent {
    History(Result<Vec<CommandHistoryEntry>, String>),
    HistoryMaintenanceWarning(String),
}
impl std::fmt::Debug for CompletionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History(result) => f
                .debug_struct("History")
                .field("succeeded", &result.is_ok())
                .finish_non_exhaustive(),
            Self::HistoryMaintenanceWarning(_) => {
                f.write_str("HistoryMaintenanceWarning([REDACTED])")
            }
        }
    }
}

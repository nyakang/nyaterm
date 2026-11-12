//! UI-independent external Agent CLI clients. Callers own jobs and credentials.
pub mod account;
pub mod claude_code;
pub mod codex;
pub mod executable;
mod external_process;
pub mod probe;

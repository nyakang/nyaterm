//! UI-independent terminal frame processing and transport event coordination.
//! Callers own session/window lifetimes and execute shutdown joins off the UI thread.
pub mod frame;
pub mod session_event_bridge;
pub mod work_policy;

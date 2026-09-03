use std::collections::HashSet;

use crate::core::session::SessionInfo;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub enum McpScope {
    Explicit {
        session_ids: HashSet<String>,
        default_session_id: Option<String>,
    },
    CurrentWindow {
        owner_window_label: String,
    },
    AllSessions,
}

#[derive(Debug, Clone)]
pub struct McpScopeSnapshot {
    pub session_ids: HashSet<String>,
    pub default_session_id: Option<String>,
}

impl McpScope {
    pub fn explicit(
        session_ids: impl IntoIterator<Item = String>,
        default_session_id: Option<String>,
    ) -> Self {
        let session_ids = session_ids.into_iter().collect::<HashSet<_>>();
        let default_session_id = default_session_id.filter(|id| session_ids.contains(id));
        Self::Explicit {
            session_ids,
            default_session_id,
        }
    }

    pub fn current_window(owner_window_label: impl Into<String>) -> Self {
        Self::CurrentWindow {
            owner_window_label: owner_window_label.into(),
        }
    }

    pub fn resolve(&self, sessions: &[SessionInfo]) -> McpScopeSnapshot {
        match self {
            Self::Explicit {
                session_ids,
                default_session_id,
            } => {
                let live_ids = sessions
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<HashSet<_>>();
                let session_ids = session_ids
                    .iter()
                    .filter(|id| live_ids.contains(id.as_str()))
                    .cloned()
                    .collect::<HashSet<_>>();
                let default_session_id = default_session_id
                    .as_ref()
                    .filter(|id| session_ids.contains(*id))
                    .cloned();
                McpScopeSnapshot {
                    session_ids,
                    default_session_id,
                }
            }
            Self::CurrentWindow { owner_window_label } => {
                dynamic_snapshot(sessions.iter().filter(|session| {
                    session.owner_window_label.as_deref() == Some(owner_window_label.as_str())
                }))
            }
            Self::AllSessions => dynamic_snapshot(sessions.iter()),
        }
    }
}

impl McpScopeSnapshot {
    pub fn require(&self, session_id: &str) -> AppResult<()> {
        if self.session_ids.contains(session_id) {
            Ok(())
        } else {
            Err(AppError::Config(
                "Session is not available in the current MCP scope.".to_string(),
            ))
        }
    }

    pub fn resolve_terminal_session(&self, requested: Option<&str>) -> AppResult<String> {
        if let Some(id) = requested.filter(|id| !id.trim().is_empty()) {
            self.require(id)?;
            return Ok(id.to_string());
        }
        self.default_session_id.clone().ok_or_else(|| {
            AppError::Config(
                "No default session is available; provide sessionId explicitly.".to_string(),
            )
        })
    }
}

fn dynamic_snapshot<'a>(sessions: impl Iterator<Item = &'a SessionInfo>) -> McpScopeSnapshot {
    let session_ids = sessions
        .map(|session| session.id.clone())
        .collect::<HashSet<_>>();
    let default_session_id = (session_ids.len() == 1)
        .then(|| session_ids.iter().next().cloned())
        .flatten();
    McpScopeSnapshot {
        session_ids,
        default_session_id,
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AiExecutionProfile;
    use crate::core::session::{DynamicTitleCapabilities, SessionType};

    use super::*;

    fn session(id: &str, owner: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            name: id.into(),
            session_type: SessionType::SSH,
            started_at: String::new(),
            connection_id: None,
            connected: true,
            owner_window_label: Some(owner.into()),
            ai_execution_profile: AiExecutionProfile::Auto,
            injection_active: true,
            dynamic_title_capabilities: DynamicTitleCapabilities::default(),
            remote_file_browser_enabled: true,
            remote_stats_enabled: true,
            ssh_profile: None,
        }
    }

    #[test]
    fn explicit_scope_is_frozen_and_drops_closed_sessions() {
        let scope = McpScope::explicit(["a".into(), "b".into()], Some("a".into()));
        let initial = scope.resolve(&[session("a", "main"), session("b", "main")]);
        assert_eq!(initial.resolve_terminal_session(None).unwrap(), "a");
        assert!(initial.resolve_terminal_session(Some("c")).is_err());

        let changed = scope.resolve(&[session("b", "main"), session("c", "main")]);
        assert_eq!(changed.session_ids, HashSet::from(["b".into()]));
        assert!(changed.default_session_id.is_none());
    }

    #[test]
    fn current_window_scope_resolves_new_sessions_and_excludes_other_windows() {
        let scope = McpScope::current_window("main");
        let initial = scope.resolve(&[session("a", "main"), session("x", "main-2")]);
        assert_eq!(initial.session_ids, HashSet::from(["a".into()]));
        assert_eq!(initial.default_session_id.as_deref(), Some("a"));

        let changed = scope.resolve(&[
            session("a", "main"),
            session("b", "main"),
            session("x", "main-2"),
        ]);
        assert_eq!(changed.session_ids, HashSet::from(["a".into(), "b".into()]));
        assert!(changed.default_session_id.is_none());
    }

    #[test]
    fn all_sessions_scope_resolves_new_sessions() {
        let scope = McpScope::AllSessions;
        assert_eq!(scope.resolve(&[session("a", "main")]).session_ids.len(), 1);
        assert_eq!(
            scope
                .resolve(&[session("a", "main"), session("b", "main-2")])
                .session_ids
                .len(),
            2
        );
    }
}

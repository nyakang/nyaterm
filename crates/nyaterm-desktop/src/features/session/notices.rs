//! Runtime-only, authoritative session notices. No persistence or rendering.
use nyaterm_core::ai::sanitize_ai_diagnostic;
use nyaterm_transport::session_notice::SessionNotice;
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::features) enum SessionNoticeKind {
    Disconnected(String),
    Error(String),
    WriteFailed { context: String, error: String },
    OutputDropped(usize),
    Transport(SessionNotice),
}

impl SessionNoticeKind {
    fn bounded(mut self) -> Self {
        let text = match &mut self {
            Self::Disconnected(text) | Self::Error(text) => Some(text),
            Self::WriteFailed { error, .. } => Some(error),
            Self::Transport(SessionNotice::TmuxCommandFailed { message }) => Some(message),
            Self::Transport(SessionNotice::X11ServerUnavailable { display_target }) => {
                Some(display_target)
            }
            _ => None,
        };
        if let Some(text) = text {
            *text = sanitize_ai_diagnostic(text, 4096);
        }
        self
    }

    pub(in crate::features) fn category(&self) -> &'static str {
        match self {
            Self::Disconnected(_) => "disconnected",
            Self::Error(_) => "session-error",
            Self::WriteFailed { .. } => "write-failed",
            Self::OutputDropped(_) => "output-dropped",
            Self::Transport(notice) => notice.category(),
        }
    }
}

#[derive(Clone)]
pub(in crate::features) struct SessionNoticeEntry {
    pub kind: SessionNoticeKind,
    pub count: usize,
    dismissed: bool,
}

#[derive(Default)]
pub(in crate::features) struct SessionNotices {
    entries: Vec<SessionNoticeEntry>,
    pub expanded: bool,
}

impl SessionNotices {
    pub(in crate::features) fn visible(
        &self,
    ) -> impl DoubleEndedIterator<Item = &SessionNoticeEntry> {
        self.entries.iter().filter(|entry| !entry.dismissed)
    }

    pub(in crate::features) fn contains(&self, category: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind.category() == category)
    }

    fn push(&mut self, kind: SessionNoticeKind) -> bool {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.kind.category() == kind.category())
        {
            let entry = &mut self.entries[index];
            // Loss reports describe the same discontinuity category; accumulate
            // bytes without undoing the user's dismissal on every output burst.
            if let (
                SessionNoticeKind::OutputDropped(total),
                SessionNoticeKind::OutputDropped(bytes),
            ) = (&mut entry.kind, &kind)
            {
                *total = total.saturating_add(*bytes);
                entry.count = entry.count.saturating_add(1);
                return !entry.dismissed;
            }
            if entry.kind == kind {
                entry.count = entry.count.saturating_add(1);
                return !entry.dismissed;
            }
            self.entries.remove(index);
        }
        // One entry per typed category bounds growth even for repeating errors.
        self.entries.push(SessionNoticeEntry {
            kind,
            count: 1,
            dismissed: false,
        });
        true
    }

    pub(in crate::features) fn dismiss(&mut self, category: &str) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.kind.category() == category)
        {
            entry.dismissed = true;
        }
        if self.visible().next().is_none() {
            self.expanded = false;
        }
    }
}

#[derive(Default)]
pub(in crate::features) struct SessionNoticeState {
    sessions: HashMap<String, SessionNotices>,
}

impl SessionNoticeState {
    pub(in crate::features) fn push(&mut self, session_id: &str, kind: SessionNoticeKind) -> bool {
        self.sessions
            .entry(session_id.to_owned())
            .or_default()
            .push(kind.bounded())
    }

    pub(in crate::features) fn get(&self, session_id: &str) -> Option<&SessionNotices> {
        self.sessions.get(session_id)
    }

    pub(in crate::features) fn dismiss(&mut self, session_id: &str, category: &str) {
        if let Some(notices) = self.sessions.get_mut(session_id) {
            notices.dismiss(category);
        }
    }

    pub(in crate::features) fn toggle_expanded(&mut self, session_id: &str) {
        if let Some(notices) = self.sessions.get_mut(session_id) {
            notices.expanded = !notices.expanded;
        }
    }

    pub(in crate::features) fn take(&mut self, session_id: &str) -> Option<SessionNotices> {
        self.sessions.remove(session_id)
    }

    pub(in crate::features) fn insert(&mut self, session_id: String, notices: SessionNotices) {
        self.sessions.insert(session_id, notices);
    }

    pub(in crate::features) fn clear(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionNoticeKind, SessionNoticeState};

    #[test]
    fn dismissed_repeats_stay_closed_but_changed_errors_reappear() {
        let mut state = SessionNoticeState::default();
        state.push("s1", SessionNoticeKind::Error("closed".into()));
        state.dismiss("s1", "session-error");
        assert!(!state.push("s1", SessionNoticeKind::Error("closed".into())));
        assert_eq!(state.get("s1").unwrap().visible().count(), 0);
        state.push("s1", SessionNoticeKind::Error("unavailable".into()));
        assert_eq!(state.get("s1").unwrap().visible().count(), 1);
        assert_eq!(state.get("s1").unwrap().visible().next().unwrap().count, 1);
    }

    #[test]
    fn dropped_bytes_accumulate_without_reopening_dismissed_notice() {
        let mut state = SessionNoticeState::default();
        state.push("s1", SessionNoticeKind::OutputDropped(10));
        state.push("s1", SessionNoticeKind::OutputDropped(20));
        let entry = state.get("s1").unwrap().visible().next().unwrap();
        assert!(matches!(entry.kind, SessionNoticeKind::OutputDropped(30)));
        assert_eq!(entry.count, 2);
        state.dismiss("s1", "output-dropped");
        assert!(!state.push("s1", SessionNoticeKind::OutputDropped(40)));
    }

    #[test]
    fn transfer_preserves_details_and_dismissals_without_mirroring_state() {
        let mut source = SessionNoticeState::default();
        source.push("s1", SessionNoticeKind::Error("closed".into()));
        source.push("s2", SessionNoticeKind::OutputDropped(5));
        source.dismiss("s1", "session-error");
        let bundle = source.take("s1").unwrap();
        let mut target = SessionNoticeState::default();
        target.insert("s1".into(), bundle);
        assert!(source.get("s1").is_none());
        assert_eq!(source.get("s2").unwrap().visible().count(), 1);
        assert!(!target.push("s1", SessionNoticeKind::Error("closed".into())));
    }

    #[test]
    fn clearing_old_connection_preserves_new_connection_warnings() {
        let mut state = SessionNoticeState::default();
        state.push("old", SessionNoticeKind::Disconnected("closed".into()));
        state.push("new", SessionNoticeKind::Error("warning".into()));
        state.clear("old");
        assert!(state.get("old").is_none());
        assert_eq!(state.get("new").unwrap().visible().count(), 1);
    }
}

use std::path::PathBuf;

use crate::models::ConnectionImportSource;

use super::ConnectionFeatureState;

#[derive(Default)]
enum ImportPhase {
    #[default]
    Idle,
    Selecting,
    Running,
    AwaitingWindtermPassword(PathBuf),
}

#[derive(Default)]
pub(super) struct ConnectionImportState {
    phase: ImportPhase,
}

impl ConnectionImportState {
    pub fn path_prompt_active(&self) -> bool {
        !matches!(self.phase, ImportPhase::Idle)
    }

    pub fn begin_path_prompt(&mut self, _source: ConnectionImportSource) {
        if !self.path_prompt_active() {
            self.phase = ImportPhase::Selecting;
        }
    }

    pub fn finish_path_prompt(&mut self) {
        self.phase = ImportPhase::Idle;
    }

    fn begin_running(&mut self) {
        self.phase = ImportPhase::Running;
    }

    fn await_password(&mut self, path: PathBuf) {
        self.phase = ImportPhase::AwaitingWindtermPassword(path);
    }

    fn take_password_path(&mut self) -> Option<PathBuf> {
        if !matches!(self.phase, ImportPhase::AwaitingWindtermPassword(_)) {
            return None;
        }
        match std::mem::replace(&mut self.phase, ImportPhase::Running) {
            ImportPhase::AwaitingWindtermPassword(path) => Some(path),
            _ => unreachable!(),
        }
    }

    fn awaiting_password(&self) -> bool {
        matches!(self.phase, ImportPhase::AwaitingWindtermPassword(_))
    }
}

impl ConnectionFeatureState {
    pub(in crate::features) fn begin_session_import_job(&mut self) {
        self.import.begin_running();
    }

    pub(in crate::features) fn await_windterm_password(&mut self, path: PathBuf) {
        self.import.await_password(path);
    }

    pub(in crate::features) fn take_windterm_password_path(&mut self) -> Option<PathBuf> {
        self.import.take_password_path()
    }

    pub(in crate::features) fn awaiting_windterm_password(&self) -> bool {
        self.import.awaiting_password()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::models::ConnectionImportSource;

    use super::{ConnectionImportState, ImportPhase};

    #[test]
    fn selecting_running_and_waiting_for_password_all_block_another_import() {
        let mut state = ConnectionImportState::default();
        assert!(!state.path_prompt_active());
        state.begin_path_prompt(ConnectionImportSource::WindTerm);
        state.begin_path_prompt(ConnectionImportSource::Xshell);
        assert!(matches!(state.phase, ImportPhase::Selecting));
        state.begin_running();
        assert!(state.path_prompt_active());
        state.await_password(PathBuf::from("user.sessions"));
        assert!(state.path_prompt_active());
        assert_eq!(
            state.take_password_path(),
            Some(PathBuf::from("user.sessions"))
        );
        assert!(state.path_prompt_active());
        assert!(state.take_password_path().is_none());
        state.finish_path_prompt();
        assert!(!state.path_prompt_active());
    }

    #[test]
    fn cancelling_a_password_prompt_discards_path_and_allows_a_new_import() {
        let mut state = ConnectionImportState::default();
        state.await_password(PathBuf::from("cancelled.sessions"));
        state.finish_path_prompt();
        assert!(!state.awaiting_password());
        assert!(state.take_password_path().is_none());
        state.begin_path_prompt(ConnectionImportSource::WindTerm);
        assert!(state.path_prompt_active());
    }
}

use std::fmt;
use std::path::PathBuf;

use nyaterm_core::{
    SavedPassword, SecretString, WindtermAccountAction, WindtermAccountDecision,
    WindtermImportReport, WindtermOneKeyConfigState, WindtermPreview, WindtermUnlinkedReason,
};

use crate::models::ConnectionImportSource;

use super::ConnectionFeatureState;

#[derive(Default)]
enum ImportPhase {
    #[default]
    Idle,
    Selecting,
    Running,
    AwaitingWindtermPassword(PathBuf),
    AwaitingWindtermAccounts,
}

/// A reusable existing account offered when a OneKey name already exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct WindtermReuseCandidate {
    pub id: String,
    pub name: String,
    pub username: String,
}

/// Editable account row for the WindTerm account confirmation dialog.
#[derive(Clone)]
pub(in crate::features) struct WindtermAccountDraft {
    pub one_key_uuid: String,
    pub name: String,
    pub session_count: usize,
    pub username: String,
    pub password: SecretString,
    pub password_available: bool,
    pub password_readable: bool,
    pub skip: bool,
    pub reuse_candidates: Vec<WindtermReuseCandidate>,
    pub reuse_account_id: Option<String>,
}

impl fmt::Debug for WindtermAccountDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindtermAccountDraft")
            .field("one_key_uuid", &self.one_key_uuid)
            .field("name", &self.name)
            .field("session_count", &self.session_count)
            .field("username", &self.username)
            .field("password_available", &self.password_available)
            .field("password_readable", &self.password_readable)
            .field("skip", &self.skip)
            .field("reuse_candidates", &self.reuse_candidates)
            .field("reuse_account_id", &self.reuse_account_id)
            .finish()
    }
}

impl WindtermAccountDraft {
    pub(in crate::features) fn reuse_candidate(&self) -> Option<&WindtermReuseCandidate> {
        self.reuse_account_id.as_deref().and_then(|id| {
            self.reuse_candidates
                .iter()
                .find(|candidate| candidate.id == id)
        })
    }

    fn decision(&self) -> WindtermAccountDecision {
        let action = if self.skip {
            WindtermAccountAction::Skip
        } else if let Some(candidate) = self.reuse_candidate() {
            WindtermAccountAction::Reuse {
                account_id: candidate.id.clone(),
                username: candidate.username.clone(),
            }
        } else {
            WindtermAccountAction::Create {
                username: self.username.trim().to_owned(),
                password: (!self.password.is_empty()).then(|| self.password.clone()),
            }
        };
        WindtermAccountDecision {
            one_key_uuid: self.one_key_uuid.clone(),
            action,
        }
    }
}

/// Presentation-neutral summary shown in the connections panel after any
/// import. WindTerm fills the OneKey specific fields; other sources leave them
/// empty so the same banner can present success/failure without fabricating
/// OneKey information.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct ConnectionImportReportView {
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub missing_one_key: Vec<String>,
    pub skipped_account_sessions: Vec<String>,
    pub unsupported_one_key_sessions: Vec<String>,
    pub one_key_state: Option<WindtermOneKeyConfigState>,
    pub expanded: bool,
}

impl ConnectionImportReportView {
    pub(in crate::features) fn from_windterm(report: &WindtermImportReport) -> Self {
        let mut skipped_account_sessions = Vec::new();
        let mut unsupported_one_key_sessions = Vec::new();
        for unlinked in &report.unlinked_sessions {
            match unlinked.reason {
                WindtermUnlinkedReason::SkippedByUser => {
                    skipped_account_sessions.push(unlinked.session_name.clone());
                }
                WindtermUnlinkedReason::UnsupportedType => {
                    unsupported_one_key_sessions.push(unlinked.session_name.clone());
                }
                WindtermUnlinkedReason::MissingDefinition => {}
            }
        }
        Self {
            imported: report.imported_sessions,
            skipped: report.skipped_sessions,
            failed: report.credential_warning_sessions,
            missing_one_key: report
                .missing_one_keys
                .iter()
                .map(|missing| missing.session_name.clone())
                .collect(),
            skipped_account_sessions,
            unsupported_one_key_sessions,
            one_key_state: Some(report.one_key_state),
            expanded: false,
        }
    }

    pub(in crate::features) fn from_basic(imported: usize, failed: usize) -> Self {
        Self {
            imported,
            failed,
            ..Self::default()
        }
    }

    pub(in crate::features) fn has_details(&self) -> bool {
        !self.missing_one_key.is_empty()
            || !self.skipped_account_sessions.is_empty()
            || !self.unsupported_one_key_sessions.is_empty()
    }

    #[cfg(test)]
    pub(in crate::features) fn result_count(&self) -> usize {
        self.imported + self.skipped + self.failed
    }
}

pub(in crate::features) struct WindtermAccountsContext {
    pub preview: WindtermPreview,
    pub path: PathBuf,
    pub master_password: Option<SecretString>,
    pub drafts: Vec<WindtermAccountDraft>,
}

impl WindtermAccountsContext {
    pub(in crate::features) fn decisions(&self) -> Vec<WindtermAccountDecision> {
        self.drafts
            .iter()
            .map(WindtermAccountDraft::decision)
            .collect()
    }
}

#[derive(Default)]
pub(super) struct ConnectionImportState {
    phase: ImportPhase,
    /// Parsed WindTerm sessions/accounts held until the user confirms the
    /// inferred accounts. Cleared together with the phase so secrets do not
    /// outlive the dialog.
    windterm_preview: Option<WindtermPreview>,
    windterm_path: Option<PathBuf>,
    windterm_master_password: Option<SecretString>,
    windterm_drafts: Vec<WindtermAccountDraft>,
    import_report: Option<ConnectionImportReportView>,
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
        self.windterm_preview = None;
        self.windterm_path = None;
        self.windterm_master_password = None;
        self.windterm_drafts.clear();
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

    fn await_accounts(
        &mut self,
        preview: WindtermPreview,
        path: PathBuf,
        master_password: Option<SecretString>,
        existing_passwords: &[SavedPassword],
    ) {
        self.windterm_drafts = build_drafts(&preview, existing_passwords);
        self.windterm_preview = Some(preview);
        self.windterm_path = Some(path);
        self.windterm_master_password = master_password;
        self.phase = ImportPhase::AwaitingWindtermAccounts;
    }

    fn take_accounts_context(&mut self) -> Option<WindtermAccountsContext> {
        if !self.awaiting_accounts() {
            return None;
        }
        self.phase = ImportPhase::Running;
        Some(WindtermAccountsContext {
            preview: self.windterm_preview.take()?,
            path: self.windterm_path.take()?,
            master_password: self.windterm_master_password.take(),
            drafts: std::mem::take(&mut self.windterm_drafts),
        })
    }

    fn awaiting_accounts(&self) -> bool {
        matches!(self.phase, ImportPhase::AwaitingWindtermAccounts)
    }

    fn accounts_preview(&self) -> Option<&WindtermPreview> {
        self.windterm_preview.as_ref()
    }

    fn drafts(&self) -> &[WindtermAccountDraft] {
        &self.windterm_drafts
    }

    fn draft_mut(&mut self, uuid: &str) -> Option<&mut WindtermAccountDraft> {
        self.windterm_drafts
            .iter_mut()
            .find(|draft| draft.one_key_uuid == uuid)
    }

    #[cfg(test)]
    fn decisions(&self) -> Vec<WindtermAccountDecision> {
        self.windterm_drafts
            .iter()
            .map(WindtermAccountDraft::decision)
            .collect()
    }

    fn update_username(&mut self, uuid: &str, username: String) -> bool {
        let Some(draft) = self.draft_mut(uuid) else {
            return false;
        };
        draft.username = username;
        true
    }

    fn update_password(&mut self, uuid: &str, password: String) -> bool {
        let Some(draft) = self.draft_mut(uuid) else {
            return false;
        };
        draft.password = SecretString::from(password);
        true
    }

    fn set_skip(&mut self, uuid: &str, skip: bool) -> bool {
        let Some(draft) = self.draft_mut(uuid) else {
            return false;
        };
        draft.skip = skip;
        if skip {
            draft.reuse_account_id = None;
        }
        true
    }

    fn set_reuse(&mut self, uuid: &str, account_id: Option<String>) -> bool {
        let Some(draft) = self.draft_mut(uuid) else {
            return false;
        };
        draft.reuse_account_id = account_id;
        if draft.reuse_account_id.is_some() {
            draft.skip = false;
        }
        true
    }

    fn set_import_report(&mut self, report: ConnectionImportReportView) {
        self.import_report = Some(report);
    }
}

impl ConnectionFeatureState {
    pub(in crate::features) fn begin_session_import_job(&mut self) {
        self.import.begin_running();
        self.import.import_report = None;
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

    pub(in crate::features) fn await_windterm_accounts(
        &mut self,
        preview: WindtermPreview,
        path: PathBuf,
        master_password: Option<SecretString>,
        existing_passwords: &[SavedPassword],
    ) {
        self.import
            .await_accounts(preview, path, master_password, existing_passwords);
    }

    pub(in crate::features) fn take_windterm_accounts_context(
        &mut self,
    ) -> Option<WindtermAccountsContext> {
        self.import.take_accounts_context()
    }

    pub(in crate::features) fn awaiting_windterm_accounts(&self) -> bool {
        self.import.awaiting_accounts()
    }

    pub(in crate::features) fn windterm_accounts_preview(&self) -> Option<&WindtermPreview> {
        self.import.accounts_preview()
    }

    pub(in crate::features) fn windterm_account_drafts(&self) -> &[WindtermAccountDraft] {
        self.import.drafts()
    }

    pub(in crate::features) fn update_windterm_account_username(
        &mut self,
        uuid: &str,
        username: String,
    ) -> bool {
        self.import.update_username(uuid, username)
    }

    pub(in crate::features) fn update_windterm_account_password(
        &mut self,
        uuid: &str,
        password: String,
    ) -> bool {
        self.import.update_password(uuid, password)
    }

    pub(in crate::features) fn set_windterm_account_skip(
        &mut self,
        uuid: &str,
        skip: bool,
    ) -> bool {
        self.import.set_skip(uuid, skip)
    }

    pub(in crate::features) fn set_windterm_account_reuse(
        &mut self,
        uuid: &str,
        account_id: Option<String>,
    ) -> bool {
        self.import.set_reuse(uuid, account_id)
    }

    pub(in crate::features) fn set_import_report(&mut self, report: ConnectionImportReportView) {
        self.import.set_import_report(report);
    }

    pub(in crate::features) fn import_report(&self) -> Option<&ConnectionImportReportView> {
        self.import.import_report.as_ref()
    }

    pub(in crate::features) fn toggle_import_report_expanded(&mut self) {
        if let Some(report) = self.import.import_report.as_mut() {
            report.expanded = !report.expanded;
        }
    }

    pub(in crate::features) fn dismiss_import_report(&mut self) {
        self.import.import_report = None;
    }
}

pub(super) fn build_drafts(
    preview: &WindtermPreview,
    existing_passwords: &[SavedPassword],
) -> Vec<WindtermAccountDraft> {
    preview
        .accounts
        .iter()
        .map(|account| {
            let reuse_candidates = existing_passwords
                .iter()
                .filter(|existing| {
                    existing
                        .name
                        .trim()
                        .eq_ignore_ascii_case(account.name.trim())
                })
                .map(|existing| WindtermReuseCandidate {
                    id: existing.id.clone(),
                    name: existing.name.clone(),
                    username: existing.username.clone(),
                })
                .collect();
            WindtermAccountDraft {
                one_key_uuid: account.one_key_uuid.clone(),
                name: account.name.clone(),
                session_count: account.session_count,
                username: account.username.clone(),
                password: account.password.clone().unwrap_or_default(),
                password_available: account.password_available,
                password_readable: account.password_readable,
                skip: false,
                reuse_candidates,
                reuse_account_id: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use nyaterm_core::{
        PreparedSessionImport, SavedPassword, WindtermAccountAction, WindtermAccountPreview,
        WindtermImportReport, WindtermMissingOneKey, WindtermOneKeyConfigState, WindtermPreview,
        WindtermUnlinkedReason, WindtermUnlinkedSession,
    };

    use crate::models::ConnectionImportSource;

    use super::{ConnectionImportReportView, ConnectionImportState, ImportPhase, build_drafts};

    fn preview_with_accounts() -> WindtermPreview {
        WindtermPreview {
            prepared: PreparedSessionImport {
                warnings: Vec::new(),
                custom_icons: Vec::new(),
                groups: Vec::new(),
                passwords: Vec::new(),
                ssh_keys: Vec::new(),
                connections: Vec::new(),
            },
            accounts: vec![WindtermAccountPreview {
                id: "account-1".into(),
                one_key_uuid: "one-key-1".into(),
                name: "jump-host".into(),
                username: "deploy".into(),
                password: Some("prefilled".into()),
                password_available: true,
                password_readable: true,
                session_count: 2,
            }],
            one_key_state: WindtermOneKeyConfigState::Readable,
            missing_one_keys: Vec::new(),
            unlinked_sessions: Vec::new(),
            unsupported_one_key_count: 0,
            invalid_one_key_count: 0,
            duplicate_one_key_uuid_count: 0,
            duplicate_account_name_count: 0,
            unreadable_account_count: 0,
            skipped_session_count: 0,
            total_session_count: 2,
        }
    }

    fn existing_password() -> SavedPassword {
        SavedPassword {
            id: "existing-1".into(),
            sort_order: 0,
            name: "jump-host".into(),
            username: "existing".into(),
            password: Some("old".into()),
            has_password: false,
        }
    }

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
    fn waiting_for_account_confirmation_blocks_imports_and_releases_context_once() {
        let mut state = ConnectionImportState::default();
        state.await_accounts(
            preview_with_accounts(),
            PathBuf::from("user.sessions"),
            None,
            &[existing_password()],
        );
        assert!(state.path_prompt_active());
        assert!(state.accounts_preview().is_some());
        assert_eq!(state.drafts().len(), 1);
        let taken = state.take_accounts_context().unwrap();
        assert_eq!(taken.preview.accounts.len(), 1);
        assert_eq!(taken.path, PathBuf::from("user.sessions"));
        assert!(state.accounts_preview().is_none());
        assert!(state.take_accounts_context().is_none());
        state.finish_path_prompt();
        assert!(!state.path_prompt_active());
    }

    #[test]
    fn drafts_prefill_credentials_and_offer_same_name_accounts() {
        let preview = preview_with_accounts();
        let drafts = build_drafts(&preview, &[existing_password()]);
        assert_eq!(drafts.len(), 1);
        let draft = &drafts[0];
        assert_eq!(draft.username, "deploy");
        assert_eq!(draft.password, "prefilled");
        assert_eq!(draft.session_count, 2);
        assert_eq!(draft.reuse_candidates.len(), 1);
        assert_eq!(draft.reuse_candidates[0].id, "existing-1");
        assert!(draft.reuse_account_id.is_none());
        assert!(!format!("{draft:?}").contains("prefilled"));
    }

    #[test]
    fn draft_decisions_cover_create_skip_and_reuse() {
        let preview = preview_with_accounts();
        let mut state = ConnectionImportState::default();
        state.await_accounts(
            preview,
            PathBuf::from("user.sessions"),
            None,
            &[existing_password()],
        );

        assert!(state.update_password("one-key-1", "new".into()));
        let create = &state.decisions()[0];
        assert!(matches!(
            &create.action,
            WindtermAccountAction::Create { username, password }
                if username == "deploy" && password.as_deref() == Some("new")
        ));

        assert!(state.set_skip("one-key-1", true));
        assert!(matches!(
            state.decisions()[0].action,
            WindtermAccountAction::Skip
        ));

        assert!(state.set_reuse("one-key-1", Some("existing-1".into())));
        assert!(!state.draft_mut("one-key-1").unwrap().skip);
        assert!(matches!(
            &state.decisions()[0].action,
            WindtermAccountAction::Reuse { account_id, username }
                if account_id == "existing-1" && username == "existing"
        ));
    }

    #[test]
    fn report_view_keeps_counts_and_connection_names_consistent() {
        let report = WindtermImportReport {
            imported_sessions: 3,
            skipped_sessions: 1,
            credential_warning_sessions: 2,
            missing_one_keys: vec![WindtermMissingOneKey {
                session_name: "missing-host".into(),
                one_key_uuid: "missing".into(),
            }],
            unlinked_sessions: vec![
                WindtermUnlinkedSession {
                    session_name: "skipped-host".into(),
                    reason: WindtermUnlinkedReason::SkippedByUser,
                },
                WindtermUnlinkedSession {
                    session_name: "unsupported-host".into(),
                    reason: WindtermUnlinkedReason::UnsupportedType,
                },
            ],
            unsupported_one_key_count: 1,
            invalid_one_key_count: 0,
            duplicate_one_key_uuid_count: 0,
            one_key_state: WindtermOneKeyConfigState::Readable,
        };
        let view = ConnectionImportReportView::from_windterm(&report);
        assert_eq!(view.imported, 3);
        assert_eq!(view.skipped, 1);
        assert_eq!(view.failed, 2);
        assert_eq!(view.missing_one_key, vec!["missing-host"]);
        assert_eq!(view.skipped_account_sessions, vec!["skipped-host"]);
        assert_eq!(view.unsupported_one_key_sessions, vec!["unsupported-host"]);
        assert_eq!(view.result_count(), 6);
        assert!(view.has_details());
        assert_eq!(
            view.one_key_state,
            Some(WindtermOneKeyConfigState::Readable)
        );
    }

    #[test]
    fn cancelling_an_account_prompt_discards_preview_path_and_password() {
        let mut state = ConnectionImportState::default();
        state.await_accounts(
            preview_with_accounts(),
            PathBuf::from("user.sessions"),
            Some("master".into()),
            &[],
        );
        state.finish_path_prompt();
        assert!(!state.awaiting_accounts());
        assert!(state.accounts_preview().is_none());
        assert!(state.take_accounts_context().is_none());
        state.begin_path_prompt(ConnectionImportSource::WindTerm);
        assert!(state.path_prompt_active());
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

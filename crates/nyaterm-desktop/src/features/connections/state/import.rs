use std::fmt;
use std::path::{Path, PathBuf};

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
    AwaitingMobaxtermPassword(MobaXtermPasswordContext),
    AwaitingWindtermAccounts,
}

pub(in crate::features) struct MobaXtermPasswordContext {
    pub path: PathBuf,
    pub passwords_path: Option<PathBuf>,
    pub decryption_failed: bool,
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
    pub notice_messages: Vec<String>,
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
            notice_messages: Vec::new(),
            one_key_state: Some(report.one_key_state),
            expanded: false,
        }
    }

    pub(in crate::features) fn from_basic(
        imported: usize,
        failed: usize,
        skipped: usize,
        notice_messages: Vec<String>,
    ) -> Self {
        Self {
            imported,
            failed,
            skipped,
            notice_messages,
            ..Self::default()
        }
    }

    pub(in crate::features) fn has_details(&self) -> bool {
        !self.missing_one_key.is_empty()
            || !self.skipped_account_sessions.is_empty()
            || !self.unsupported_one_key_sessions.is_empty()
            || !self.notice_messages.is_empty()
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

/// The two file slots the MobaXterm import prompt presents in one window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum MobaImportSlot {
    #[default]
    Session,
    Passwords,
}

#[derive(Default)]
struct MobaImportSelection {
    session_path: Option<PathBuf>,
    passwords_path: Option<PathBuf>,
    session_extension_warning: bool,
    passwords_extension_warning: bool,
    drop_hover: Option<MobaImportSlot>,
}

fn moba_extension_matches(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
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
    mobaxterm: MobaImportSelection,
}

impl ConnectionImportState {
    fn await_mobaxterm_password(&mut self, context: MobaXtermPasswordContext) {
        self.phase = ImportPhase::AwaitingMobaxtermPassword(context);
    }

    fn mobaxterm_password_context(&self) -> Option<&MobaXtermPasswordContext> {
        match &self.phase {
            ImportPhase::AwaitingMobaxtermPassword(context) => Some(context),
            _ => None,
        }
    }

    fn take_mobaxterm_password_context(&mut self) -> Option<MobaXtermPasswordContext> {
        self.mobaxterm_password_context()?;
        match std::mem::replace(&mut self.phase, ImportPhase::Running) {
            ImportPhase::AwaitingMobaxtermPassword(context) => Some(context),
            _ => unreachable!(),
        }
    }

    pub fn path_prompt_active(&self) -> bool {
        !matches!(self.phase, ImportPhase::Idle)
    }

    pub fn begin_path_prompt(&mut self, source: ConnectionImportSource) {
        if !self.path_prompt_active() {
            self.phase = ImportPhase::Selecting;
            if source == ConnectionImportSource::MobaXterm {
                self.mobaxterm = MobaImportSelection::default();
            }
        }
    }

    pub fn finish_path_prompt(&mut self) {
        self.phase = ImportPhase::Idle;
        self.windterm_preview = None;
        self.windterm_path = None;
        self.windterm_master_password = None;
        self.windterm_drafts.clear();
        self.mobaxterm = MobaImportSelection::default();
    }

    pub(super) fn mobaxterm_session_path(&self) -> Option<&Path> {
        self.mobaxterm.session_path.as_deref()
    }

    pub(super) fn mobaxterm_passwords_path(&self) -> Option<&Path> {
        self.mobaxterm.passwords_path.as_deref()
    }

    pub(super) fn mobaxterm_import_ready(&self) -> bool {
        self.mobaxterm.session_path.is_some()
    }

    pub(super) fn mobaxterm_passwords_missing(&self) -> bool {
        self.mobaxterm.passwords_path.is_none()
    }

    pub(super) fn mobaxterm_session_extension_warning(&self) -> bool {
        self.mobaxterm.session_extension_warning
    }

    pub(super) fn mobaxterm_passwords_extension_warning(&self) -> bool {
        self.mobaxterm.passwords_extension_warning
    }

    pub(super) fn mobaxterm_drop_hover(&self) -> Option<MobaImportSlot> {
        self.mobaxterm.drop_hover
    }

    pub(super) fn set_mobaxterm_drop_hover(&mut self, slot: Option<MobaImportSlot>) -> bool {
        if self.mobaxterm.drop_hover == slot {
            return false;
        }
        self.mobaxterm.drop_hover = slot;
        true
    }

    pub(super) fn set_mobaxterm_file(&mut self, slot: MobaImportSlot, path: PathBuf) -> bool {
        match slot {
            MobaImportSlot::Session => {
                self.mobaxterm.session_extension_warning =
                    !moba_extension_matches(&path, "mxtsessions")
                        && !moba_extension_matches(&path, "ini");
                self.mobaxterm.session_path = Some(path);
                if self.mobaxterm.drop_hover == Some(MobaImportSlot::Session) {
                    self.mobaxterm.drop_hover = None;
                }
            }
            MobaImportSlot::Passwords => {
                self.mobaxterm.passwords_extension_warning = !moba_extension_matches(&path, "txt");
                self.mobaxterm.passwords_path = Some(path);
                if self.mobaxterm.drop_hover == Some(MobaImportSlot::Passwords) {
                    self.mobaxterm.drop_hover = None;
                }
            }
        }
        true
    }

    pub(super) fn clear_mobaxterm_slot(&mut self, slot: MobaImportSlot) -> bool {
        let hovered = self.mobaxterm.drop_hover == Some(slot);
        if hovered {
            self.mobaxterm.drop_hover = None;
        }
        let changed = match slot {
            MobaImportSlot::Session => {
                let changed = self.mobaxterm.session_path.take().is_some()
                    || self.mobaxterm.session_extension_warning;
                self.mobaxterm.session_extension_warning = false;
                changed
            }
            MobaImportSlot::Passwords => {
                let changed = self.mobaxterm.passwords_path.take().is_some()
                    || self.mobaxterm.passwords_extension_warning;
                self.mobaxterm.passwords_extension_warning = false;
                changed
            }
        };
        changed || hovered
    }

    pub(super) fn apply_mobaxterm_drop(&mut self, slot: MobaImportSlot, paths: &[PathBuf]) -> bool {
        match paths.first() {
            Some(path) => self.set_mobaxterm_file(slot, path.clone()),
            None => false,
        }
    }

    pub(super) fn cancel_mobaxterm_prompt(&mut self) -> bool {
        if !matches!(self.phase, ImportPhase::Selecting) {
            return false;
        }
        self.phase = ImportPhase::Idle;
        self.mobaxterm = MobaImportSelection::default();
        true
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
    pub(in crate::features) fn await_mobaxterm_password(
        &mut self,
        path: PathBuf,
        passwords_path: Option<PathBuf>,
        decryption_failed: bool,
    ) {
        self.import
            .await_mobaxterm_password(MobaXtermPasswordContext {
                path,
                passwords_path,
                decryption_failed,
            });
    }

    pub(in crate::features) fn mobaxterm_password_context(
        &self,
    ) -> Option<&MobaXtermPasswordContext> {
        self.import.mobaxterm_password_context()
    }

    pub(in crate::features) fn take_mobaxterm_password_context(
        &mut self,
    ) -> Option<MobaXtermPasswordContext> {
        self.import.take_mobaxterm_password_context()
    }

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

    use super::{
        ConnectionImportReportView, ConnectionImportState, ImportPhase, MobaImportSlot,
        MobaXtermPasswordContext, build_drafts,
    };

    fn preview_with_accounts() -> WindtermPreview {
        WindtermPreview {
            prepared: PreparedSessionImport {
                warnings: Vec::new(),
                custom_icons: Vec::new(),
                groups: Vec::new(),
                passwords: Vec::new(),
                ssh_keys: Vec::new(),
                connections: Vec::new(),
                notices: Vec::new(),
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

    #[test]
    fn mobaxterm_challenge_retains_both_paths_blocks_other_imports_and_clears_on_cancel() {
        let mut state = ConnectionImportState::default();
        state.await_mobaxterm_password(MobaXtermPasswordContext {
            path: PathBuf::from("MobaXterm.ini"),
            passwords_path: Some(PathBuf::from("synthetic.txt")),
            decryption_failed: true,
        });
        assert!(state.path_prompt_active());
        state.begin_path_prompt(ConnectionImportSource::WindTerm);
        assert!(
            state
                .mobaxterm_password_context()
                .unwrap()
                .decryption_failed
        );
        let context = state.take_mobaxterm_password_context().unwrap();
        assert_eq!(context.path, PathBuf::from("MobaXterm.ini"));
        assert_eq!(context.passwords_path, Some(PathBuf::from("synthetic.txt")));
        assert!(matches!(state.phase, ImportPhase::Running));
        assert!(state.take_mobaxterm_password_context().is_none());
        state.finish_path_prompt();
        assert!(!state.path_prompt_active());
        assert!(state.mobaxterm_password_context().is_none());
    }

    #[test]
    fn mobaxterm_ini_is_an_expected_extension() {
        let mut state = ConnectionImportState::default();
        state.set_mobaxterm_file(MobaImportSlot::Session, PathBuf::from("MobaXterm.INI"));
        assert!(state.mobaxterm_import_ready());
        assert!(!state.mobaxterm_session_extension_warning());
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

    #[test]
    fn mobaxterm_drop_assigns_and_replaces_slots_independently() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        assert!(state.apply_mobaxterm_drop(
            MobaImportSlot::Session,
            &[PathBuf::from("synthetic-a.mxtsessions")]
        ));
        assert!(state.apply_mobaxterm_drop(
            MobaImportSlot::Passwords,
            &[PathBuf::from("synthetic-passwords.txt")]
        ));
        assert!(state.apply_mobaxterm_drop(
            MobaImportSlot::Session,
            &[PathBuf::from("synthetic-b.mxtsessions")]
        ));
        assert_eq!(
            state.mobaxterm_session_path(),
            Some(std::path::Path::new("synthetic-b.mxtsessions"))
        );
        assert_eq!(
            state.mobaxterm_passwords_path(),
            Some(std::path::Path::new("synthetic-passwords.txt"))
        );
    }

    #[test]
    fn mobaxterm_clear_only_clears_requested_slot() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic.mxtsessions"),
        );
        state.set_mobaxterm_file(MobaImportSlot::Passwords, PathBuf::from("synthetic.txt"));
        assert!(state.clear_mobaxterm_slot(MobaImportSlot::Session));
        assert!(state.mobaxterm_session_path().is_none());
        assert_eq!(
            state.mobaxterm_passwords_path(),
            Some(std::path::Path::new("synthetic.txt"))
        );
        assert!(state.clear_mobaxterm_slot(MobaImportSlot::Passwords));
        assert!(state.mobaxterm_passwords_path().is_none());
        assert!(!state.clear_mobaxterm_slot(MobaImportSlot::Passwords));
    }

    #[test]
    fn mobaxterm_import_requires_session_file() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        assert!(!state.mobaxterm_import_ready());
        state.set_mobaxterm_file(MobaImportSlot::Passwords, PathBuf::from("synthetic.txt"));
        assert!(!state.mobaxterm_import_ready());
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic.mxtsessions"),
        );
        assert!(state.mobaxterm_import_ready());
    }

    #[test]
    fn mobaxterm_password_missing_shows_warning() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        assert!(state.mobaxterm_passwords_missing());
        state.set_mobaxterm_file(MobaImportSlot::Passwords, PathBuf::from("synthetic.txt"));
        assert!(!state.mobaxterm_passwords_missing());
    }

    #[test]
    fn mobaxterm_unexpected_extension_is_accepted_with_warning() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic-sessions.txt"),
        );
        assert_eq!(
            state.mobaxterm_session_path(),
            Some(std::path::Path::new("synthetic-sessions.txt"))
        );
        assert!(state.mobaxterm_session_extension_warning());
        assert!(state.mobaxterm_import_ready());

        state.set_mobaxterm_file(
            MobaImportSlot::Passwords,
            PathBuf::from("synthetic-passwords"),
        );
        assert_eq!(
            state.mobaxterm_passwords_path(),
            Some(std::path::Path::new("synthetic-passwords"))
        );
        assert!(state.mobaxterm_passwords_extension_warning());
    }

    #[test]
    fn mobaxterm_cancel_only_resets_selecting_phase() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic.mxtsessions"),
        );
        assert!(state.cancel_mobaxterm_prompt());
        assert!(state.mobaxterm_session_path().is_none());

        state.begin_running();
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic.mxtsessions"),
        );
        assert!(!state.cancel_mobaxterm_prompt());
        assert_eq!(
            state.mobaxterm_session_path(),
            Some(std::path::Path::new("synthetic.mxtsessions"))
        );
    }

    #[test]
    fn mobaxterm_cancel_allows_reopening_import_without_draft() {
        let mut state = ConnectionImportState::default();
        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        state.set_mobaxterm_file(
            MobaImportSlot::Session,
            PathBuf::from("synthetic-a.mxtsessions"),
        );
        state.set_mobaxterm_file(
            MobaImportSlot::Passwords,
            PathBuf::from("synthetic-passwords.txt"),
        );
        state.set_mobaxterm_drop_hover(Some(MobaImportSlot::Passwords));
        state.set_mobaxterm_file(
            MobaImportSlot::Passwords,
            PathBuf::from("synthetic-passwords"),
        );
        assert!(state.mobaxterm_passwords_extension_warning());

        assert!(state.cancel_mobaxterm_prompt());
        assert!(
            !state.path_prompt_active(),
            "取消后必须允许再次打开导入入口"
        );
        assert!(
            state.mobaxterm_session_path().is_none(),
            "取消不得保留会话文件"
        );
        assert!(
            state.mobaxterm_passwords_path().is_none(),
            "取消不得保留密码文件"
        );
        assert!(
            state.mobaxterm_drop_hover().is_none(),
            "取消不得保留拖放悬停"
        );
        assert!(!state.mobaxterm_session_extension_warning());
        assert!(!state.mobaxterm_passwords_extension_warning());

        state.begin_path_prompt(ConnectionImportSource::MobaXterm);
        assert!(state.path_prompt_active(), "取消后再次打开应进入选择阶段");
        assert!(!state.mobaxterm_import_ready(), "重新打开时不得带上旧草稿");
    }
}

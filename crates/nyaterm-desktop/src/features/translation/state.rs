//! Authoritative transient state for translation settings and jobs.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use rust_i18n::t;

use nyaterm_core::{TranslateResult, TranslationSettings};

use crate::models::{TranslateInputField, TranslationDialogState, TranslationSecretDraft};

pub(super) struct TranslateJobResult {
    result: Result<TranslateResult, String>,
}

pub(super) struct TranslateJobRequest {
    tx: UnboundedSender<TranslateJobResult>,
    provider: String,
    target_language_snapshot: String,
    text: String,
    settings: TranslationSettings,
}

impl TranslateJobResult {
    pub(super) fn new(result: Result<TranslateResult, String>) -> Self {
        Self { result }
    }
}

impl TranslateJobRequest {
    pub(super) fn into_parts(
        self,
    ) -> (
        UnboundedSender<TranslateJobResult>,
        String,
        String,
        String,
        TranslationSettings,
    ) {
        (
            self.tx,
            self.provider,
            self.target_language_snapshot,
            self.text,
            self.settings,
        )
    }
}

pub(in crate::features) struct TranslationFeatureState {
    dialog: Option<TranslationDialogState>,
    tx: UnboundedSender<TranslateJobResult>,
    /// Taken once by `NyaTermApp::start_translation_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<TranslateJobResult>>,
    provider: String,
    settings: TranslationSettings,
    secret_draft: TranslationSecretDraft,
    input: String,
    result: Option<TranslateResult>,
    status: String,
    pending: bool,
    focused_field: TranslateInputField,
    persistence_generation: u64,
    persistence_in_flight: Option<u64>,
    persistence_pending: Option<TranslationSettings>,
    persistence_dirty: bool,
}

pub(super) struct TranslationPersistenceCompletion {
    apply_result: bool,
    report_result: bool,
    next: Option<(u64, TranslationSettings)>,
}

impl TranslationPersistenceCompletion {
    pub(super) fn apply_result(&self) -> bool {
        self.apply_result
    }

    pub(super) fn report_result(&self) -> bool {
        self.report_result
    }

    pub(super) fn take_next(&mut self) -> Option<(u64, TranslationSettings)> {
        self.next.take()
    }
}

impl TranslationFeatureState {
    pub(in crate::features) fn new(settings: TranslationSettings) -> Self {
        let (tx, rx) = unbounded();
        Self {
            dialog: None,
            tx,
            rx: Some(rx),
            provider: "google".to_string(),
            settings,
            secret_draft: TranslationSecretDraft::default(),
            input: String::new(),
            result: None,
            status: "Google translation ready".to_string(),
            pending: false,
            focused_field: TranslateInputField::Text,
            persistence_generation: 0,
            persistence_in_flight: None,
            persistence_pending: None,
            persistence_dirty: false,
        }
    }

    pub(super) fn begin_run(&mut self) -> Option<TranslateJobRequest> {
        if self.pending {
            self.status = "translation already running".to_string();
            return None;
        }
        if self.input.trim().is_empty() {
            self.status = "type text before translating".to_string();
            return None;
        }

        self.pending = true;
        self.status = format!("translating with {}", self.provider);
        Some(TranslateJobRequest {
            tx: self.tx.clone(),
            provider: self.provider.clone(),
            target_language_snapshot: self.settings.target_language.clone(),
            text: self.input.clone(),
            settings: self.settings.clone(),
        })
    }

    pub(in crate::features) fn dialog_is_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(in crate::features) fn dialog_snapshot(&self) -> Option<TranslationDialogState> {
        self.dialog.clone()
    }

    pub(in crate::features) fn provider(&self) -> &str {
        &self.provider
    }

    pub(in crate::features) fn settings(&self) -> &TranslationSettings {
        &self.settings
    }

    pub(in crate::features) fn settings_draft_snapshot(
        &self,
    ) -> (TranslationSettings, TranslationSecretDraft) {
        (self.settings.clone(), self.secret_draft.clone())
    }

    pub(in crate::features) fn settings_draft_matches(
        &self,
        settings: &TranslationSettings,
        secret_draft: &TranslationSecretDraft,
    ) -> bool {
        &self.settings == settings && &self.secret_draft == secret_draft
    }

    pub(in crate::features) fn pending_settings(&self) -> TranslationSettings {
        let mut next = self.settings.clone();
        if !self.secret_draft.deepl_api_key.is_empty() {
            next.deepl_api_key = self.secret_draft.deepl_api_key.clone();
        }
        if !self.secret_draft.baidu_app_key.is_empty() {
            next.baidu_app_key = self.secret_draft.baidu_app_key.clone();
        }
        if !self.secret_draft.ali_app_key.is_empty() {
            next.ali_app_key = self.secret_draft.ali_app_key.clone();
        }
        if !self.secret_draft.youdao_app_key.is_empty() {
            next.youdao_app_key = self.secret_draft.youdao_app_key.clone();
        }
        next
    }

    pub(in crate::features) fn result_snapshot(&self) -> Option<TranslateResult> {
        self.result.clone()
    }

    pub(in crate::features) fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn is_pending(&self) -> bool {
        self.pending
    }

    pub(in crate::features) fn mark_result_copied(&mut self, status: String) {
        self.status = status;
    }

    pub(in crate::features) fn replace_settings(
        &mut self,
        settings: TranslationSettings,
        secret_draft: TranslationSecretDraft,
    ) {
        self.settings = settings;
        self.secret_draft = secret_draft;
    }

    pub(in crate::features) fn select_target_language(&mut self, language: &str) {
        self.settings.target_language = language.to_string();
    }

    pub(super) fn settings_staged(&mut self, settings: TranslationSettings) {
        self.replace_settings(settings, TranslationSecretDraft::default());
        self.status = "translation settings staged".to_string();
    }

    pub(super) fn settings_saved(&mut self, settings: TranslationSettings) {
        self.replace_settings(settings, TranslationSecretDraft::default());
        self.status = "translation settings saved".to_string();
    }

    pub(super) fn settings_save_failed(&mut self, error: impl std::fmt::Display) {
        self.status = format!("translation settings save failed: {error}");
    }

    pub(super) fn queue_settings_persistence(&mut self) -> Option<(u64, TranslationSettings)> {
        self.persistence_generation = self.persistence_generation.saturating_add(1);
        self.persistence_dirty = true;
        let snapshot = self.pending_settings();
        if self.persistence_in_flight.is_some() {
            self.persistence_pending = Some(snapshot);
            None
        } else {
            self.persistence_in_flight = Some(self.persistence_generation);
            Some((self.persistence_generation, snapshot))
        }
    }

    pub(super) fn finish_settings_persistence(
        &mut self,
        generation: u64,
        succeeded: bool,
    ) -> TranslationPersistenceCompletion {
        if self.persistence_in_flight != Some(generation) {
            return TranslationPersistenceCompletion {
                apply_result: false,
                report_result: false,
                next: None,
            };
        }
        self.persistence_in_flight = None;
        let next = self.persistence_pending.take().map(|snapshot| {
            let generation = self.persistence_generation;
            self.persistence_in_flight = Some(generation);
            (generation, snapshot)
        });
        let report_result = generation == self.persistence_generation && next.is_none();
        let apply_result = succeeded && report_result;
        if apply_result {
            self.persistence_dirty = false;
        }
        TranslationPersistenceCompletion {
            apply_result,
            report_result,
            next,
        }
    }

    pub(in crate::features) fn settings_persistence_is_dirty(&self) -> bool {
        self.persistence_dirty
    }

    pub(super) fn clear_secret(&mut self, provider: &str) {
        match provider {
            "deepl" => {
                self.settings.deepl_api_key.expose_secret_mut().clear();
                self.secret_draft.deepl_api_key.expose_secret_mut().clear();
            }
            "baidu" => {
                self.settings.baidu_app_key.expose_secret_mut().clear();
                self.secret_draft.baidu_app_key.expose_secret_mut().clear();
            }
            "ali" => {
                self.settings.ali_app_key.expose_secret_mut().clear();
                self.secret_draft.ali_app_key.expose_secret_mut().clear();
            }
            "youdao" => {
                self.settings.youdao_app_key.expose_secret_mut().clear();
                self.secret_draft.youdao_app_key.expose_secret_mut().clear();
            }
            _ => {}
        }
        self.status = t!("settings.translationSecretCleared", provider = provider).to_string();
    }

    pub(super) fn edit_input(&mut self, field: TranslateInputField, text: String) {
        self.focused_field = field;
        *self.input_value_mut() = text;
        self.status = if field.is_settings_field() {
            t!("settings.translationSettingsEdited").to_string()
        } else {
            t!("settings.translationInputEdited").to_string()
        };
    }

    fn input_value_mut(&mut self) -> &mut String {
        match self.focused_field {
            TranslateInputField::TargetLanguage => &mut self.settings.target_language,
            TranslateInputField::Text => &mut self.input,
            TranslateInputField::DeeplApiKey => self.secret_draft.deepl_api_key.expose_secret_mut(),
            TranslateInputField::BaiduAppId => &mut self.settings.baidu_app_id,
            TranslateInputField::BaiduAppKey => self.secret_draft.baidu_app_key.expose_secret_mut(),
            TranslateInputField::AliAppId => &mut self.settings.ali_app_id,
            TranslateInputField::AliAppKey => self.secret_draft.ali_app_key.expose_secret_mut(),
            TranslateInputField::YoudaoAppId => &mut self.settings.youdao_app_id,
            TranslateInputField::YoudaoAppKey => {
                self.secret_draft.youdao_app_key.expose_secret_mut()
            }
        }
    }

    pub(super) fn open_dialog(
        &mut self,
        text: String,
        provider: String,
        provider_label: String,
    ) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() {
            self.status = "no text to translate".to_string();
            return false;
        }
        self.dialog = Some(TranslationDialogState {
            source_text: text.clone(),
            provider: provider.clone(),
            provider_label,
        });
        self.provider = provider;
        self.input = text;
        self.result = None;
        self.status = format!("translating with {}", self.provider);
        true
    }

    pub(super) fn close_dialog(&mut self) -> bool {
        self.dialog.take().is_some()
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<TranslateJobResult>> {
        self.rx.take()
    }

    /// Apply one job result, reporting whether the UI needs a repaint.
    pub(super) fn apply_event(&mut self, event: TranslateJobResult) -> bool {
        if !self.pending {
            // No run is outstanding, so this can only be a late duplicate.
            // Dropping it keeps a stale status off a run that already settled.
            return false;
        }
        self.pending = false;
        match event.result {
            Ok(result) => {
                self.status = format!(
                    "translated {} character(s) from {}",
                    result.original.chars().count(),
                    result.detected_language
                );
                self.result = Some(result);
            }
            Err(error) => {
                self.status = format!("translation failed: {error}");
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{TranslateResult, TranslationSettings};

    use crate::models::TranslateInputField;

    use super::{TranslateJobResult, TranslationFeatureState, TranslationSecretDraft};

    #[test]
    fn translation_state_owns_job_channel_and_loaded_settings() {
        let settings = TranslationSettings {
            target_language: "ja".to_string(),
            ..TranslationSettings::default()
        };

        let mut state = TranslationFeatureState::new(settings.clone());

        assert_eq!(state.settings(), &settings);
        assert!(
            state
                .take_event_receiver()
                .expect("a fresh state still holds its receiver")
                .try_recv()
                .is_err(),
            "the job channel starts empty"
        );
        assert!(!state.is_pending());
        assert!(!state.dialog_is_open());
    }

    #[test]
    fn translation_job_admission_captures_request_and_blocks_overlap() {
        let mut state = TranslationFeatureState::new(TranslationSettings::default());
        assert!(state.begin_run().is_none());
        assert_eq!(state.status(), "type text before translating");

        state.select_target_language("ja");
        assert!(state.open_dialog(
            "hello".to_string(),
            "deepl".to_string(),
            "DeepL".to_string(),
        ));
        let request = state.begin_run().expect("non-empty input should start");
        let (_, provider, target_language, text, _) = request.into_parts();
        assert_eq!(provider, "deepl");
        assert_eq!(target_language, "ja");
        assert_eq!(text, "hello");
        assert!(state.is_pending());
        assert!(state.begin_run().is_none());
        assert_eq!(state.status(), "translation already running");
    }

    #[test]
    fn translation_inputs_and_secrets_transition_inside_owner() {
        let mut state = TranslationFeatureState::new(TranslationSettings::default());
        state.edit_input(TranslateInputField::BaiduAppId, "app-id".to_string());
        assert_eq!(state.settings().baidu_app_id, "app-id");
        assert_eq!(state.status(), "Translation settings edited.");

        state.replace_settings(
            TranslationSettings {
                baidu_app_key: "stored".to_string().into(),
                ..TranslationSettings::default()
            },
            TranslationSecretDraft {
                baidu_app_key: "draft".to_string().into(),
                ..TranslationSecretDraft::default()
            },
        );
        state.clear_secret("baidu");
        assert!(state.settings().baidu_app_key.is_empty());
        assert!(state.settings_draft_snapshot().1.baidu_app_key.is_empty());
        assert_eq!(
            state.status(),
            "baidu translation secret cleared; save to persist."
        );

        state.select_target_language("ko");
        assert_eq!(state.settings().target_language, "ko");

        let restored_draft = TranslationSecretDraft {
            deepl_api_key: "restored".to_string().into(),
            ..TranslationSecretDraft::default()
        };
        state.replace_settings(
            TranslationSettings {
                target_language: "fr".to_string(),
                ..TranslationSettings::default()
            },
            restored_draft,
        );
        let (settings, secret_draft) = state.settings_draft_snapshot();
        assert_eq!(settings.target_language, "fr");
        assert_eq!(secret_draft.deepl_api_key.expose_secret(), "restored");
    }

    #[test]
    fn translation_pending_settings_overlay_only_non_empty_secret_drafts() {
        let settings = TranslationSettings {
            deepl_api_key: "stored-deepl".to_string().into(),
            baidu_app_key: "stored-baidu".to_string().into(),
            ali_app_key: "stored-ali".to_string().into(),
            ..TranslationSettings::default()
        };
        let secret_draft = TranslationSecretDraft {
            baidu_app_key: "draft-baidu".to_string().into(),
            youdao_app_key: "draft-youdao".to_string().into(),
            ..TranslationSecretDraft::default()
        };
        let mut state = TranslationFeatureState::new(TranslationSettings::default());
        state.replace_settings(settings.clone(), secret_draft.clone());

        assert!(state.settings_draft_matches(&settings, &secret_draft));
        let pending = state.pending_settings();
        assert_eq!(pending.deepl_api_key.expose_secret(), "stored-deepl");
        assert_eq!(pending.baidu_app_key.expose_secret(), "draft-baidu");
        assert_eq!(pending.ali_app_key.expose_secret(), "stored-ali");
        assert_eq!(pending.youdao_app_key.expose_secret(), "draft-youdao");

        state.select_target_language("ja");
        assert!(!state.settings_draft_matches(&settings, &secret_draft));
    }

    #[test]
    fn translation_settings_persistence_coalesces_latest_snapshot_and_keeps_failed_dirty() {
        let mut state = TranslationFeatureState::new(TranslationSettings::default());
        let (first_generation, _) = state
            .queue_settings_persistence()
            .expect("first save should start");
        state.select_target_language("ja");
        assert!(state.queue_settings_persistence().is_none());

        let mut first = state.finish_settings_persistence(first_generation, true);
        assert!(!first.apply_result());
        assert!(!first.report_result());
        let (latest_generation, latest) = first.take_next().expect("latest snapshot should follow");
        assert_eq!(latest.target_language, "ja");

        let failed = state.finish_settings_persistence(latest_generation, false);
        assert!(failed.report_result());
        assert!(!failed.apply_result());
        assert!(state.settings_persistence_is_dirty());

        let (retry_generation, _) = state
            .queue_settings_persistence()
            .expect("retry should submit latest snapshot");
        let retried = state.finish_settings_persistence(retry_generation, true);
        assert!(retried.apply_result());
        assert!(!state.settings_persistence_is_dirty());
    }

    #[test]
    fn translation_dialog_and_job_completion_share_one_state_machine() {
        let mut state = TranslationFeatureState::new(TranslationSettings::default());
        assert!(!state.open_dialog("  ".to_string(), "google".to_string(), "Google".to_string()));
        assert!(!state.dialog_is_open());

        assert!(state.open_dialog(
            " hello ".to_string(),
            "google".to_string(),
            "Google".to_string()
        ));
        let mut rx = state
            .take_event_receiver()
            .expect("state should retain its event receiver");
        let request = state.begin_run().expect("dialog input should start");
        request
            .into_parts()
            .0
            .unbounded_send(TranslateJobResult::new(Ok(TranslateResult {
                original: "hello".to_string(),
                translated: "你好".to_string(),
                detected_language: "en".to_string(),
                provider: "google".to_string(),
            })))
            .expect("the drain receiver is still alive");
        let event = rx.try_recv().expect("the job result should be queued");

        assert!(state.apply_event(event));
        assert!(!state.is_pending());
        assert_eq!(state.result_snapshot().unwrap().translated, "你好");
        assert_eq!(state.status(), "translated 5 character(s) from en");
        state.mark_result_copied("copied".to_string());
        assert_eq!(state.status(), "copied");
        assert!(state.close_dialog());
        assert!(!state.close_dialog());
    }
}

use super::command_suggestions::{
    SUGGESTION_OVERLAY_FOOTER_HEIGHT, SUGGESTION_OVERLAY_HEADER_HEIGHT,
    suggestion_overlay_desired_height,
};
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use crate::models::{
    CredentialAutofillAction, CredentialAutofillMatchEvent, CredentialAutofillMatchRequest,
    CredentialAutofillMatchRequestKey, CredentialAutofillTarget, CredentialSuggestionState,
    PendingCredentialAutofill, credential_autofill_action, otp_prompt_target,
};
use crate::models::{credential_autofill_target_display_name, credential_autofill_target_subtitle};
use futures::StreamExt as _;
use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb, rgba,
    svg,
};
use nyaterm_core::credential_autofill::matching::detect_live_prompt_kind as credential_autofill_detect_prompt_kind;
use nyaterm_core::{CredentialPromptKind, TerminalInputState, truncate_preview};
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_terminal::TerminalSnapshot;
use nyaterm_ui::NyaScrollable;
use rust_i18n::t;
use std::time::{SystemTime, UNIX_EPOCH};

const CREDENTIAL_AUTOFILL_INPUT_TAIL_LIMIT: usize = 4096;
const PENDING_PASSWORD_TTL_MS: u64 = 60_000;
const CREDENTIAL_PROMPT_INPUT_TTL_MS: u64 = 120_000;

impl NyaTermApp {
    pub(in crate::features) fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }

    pub(in crate::features) fn dismiss_credential_suggestions(&mut self, cx: &mut Context<Self>) {
        let had_panel = self.terminal.assist.dismiss_credential_suggestions();
        if had_panel {
            cx.notify();
        }
    }

    pub(in crate::features) fn observe_recording_prompt_output(
        &mut self,
        session_id: &str,
        text: &str,
    ) {
        self.recording.observe_prompt(session_id, text, |prompt| {
            matches!(
                credential_autofill_detect_prompt_kind(prompt),
                Some(CredentialPromptKind::Password)
            )
        });
    }

    pub(in crate::features) fn recording_input_is_sensitive(&self, session_id: &str) -> bool {
        // Inspect this session's prompt; active-session suggestion state cannot classify peers.
        if self.recording.has_observed_prompt(session_id) {
            return self.recording.input_is_sensitive(session_id);
        }
        self.terminal
            .view
            .views
            .get(session_id)
            .and_then(|view| view.frame_snapshot.as_deref())
            .and_then(credential_autofill_prompt_text_from_snapshot)
            .is_some_and(|prompt| {
                matches!(
                    credential_autofill_detect_prompt_kind(&prompt),
                    Some(CredentialPromptKind::Password)
                )
            })
    }

    pub(in crate::features) fn is_credential_prompt_input_mode(&self) -> bool {
        let now = Self::now_unix_ms();
        self.terminal.assist.credential_prompt_input_mode(now)
    }

    pub(in crate::features) fn cancel_credential_autofill(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.cancel_credential_autofill() {
            cx.notify();
        }
    }

    fn credential_autofill_enabled(&self) -> bool {
        self.settings
            .summary()
            .interaction_credential_autofill_enabled
    }

    fn show_credential_panel(
        &mut self,
        kind: CredentialPromptKind,
        matches: Vec<CredentialAutofillTarget>,
        prompt_text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.credential_autofill_enabled() || matches.is_empty() {
            return;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        self.dismiss_command_suggestions(cx);
        let selected_index = (matches.len() == 1).then_some(0);
        self.terminal.assist.credential_suggestions = Some(CredentialSuggestionState {
            session_id,
            kind,
            matches,
            prompt_text,
            selected_index,
        });
        cx.notify();
    }

    pub(in crate::features) fn drain_pending_credential_autofill_detection(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.credential_autofill_enabled() {
            let had_panel = self.terminal.assist.credential_suggestions.is_some();
            if had_panel
                || self.terminal.assist.credential_autofill_pending.is_some()
                || self
                    .terminal
                    .assist
                    .credential_autofill_pending_request
                    .is_some()
                || self.terminal.assist.credential_autofill_detection_pending
            {
                self.cancel_credential_autofill(cx);
            }
            return had_panel;
        }
        // Prompt detection also protects manual input, even with an empty vault.
        // Replies arrive on `start_credential_autofill_match_drain`; what is
        // left here is snapshot-driven detection.
        let mut root_overlay_dirty = false;
        let detection_was_pending = self.terminal.assist.credential_autofill_detection_pending;
        let runtime_backlog = CredentialAutofillRuntimeBacklog {
            queued_output_bytes: self.shell.session_event_queued_output_bytes(),
            pending_session_events: self.session.pending_event_count(),
            pending_terminal_frame_events: self.terminal.view.pending_frame_events.len(),
            queued_terminal_frame_events: self.terminal.view.frame_pipeline.queued_event_count(),
            queued_terminal_frame_output_bytes: self
                .terminal
                .view
                .frame_pipeline
                .queued_output_bytes(),
        };
        let match_request_pending = self
            .terminal
            .assist
            .credential_autofill_pending_request
            .is_some();
        if credential_autofill_snapshot_detection_can_run(
            self.session.active_id(),
            runtime_backlog,
            match_request_pending,
        ) {
            root_overlay_dirty |= self.sync_credential_autofill_from_active_snapshot(cx);
        }
        if !credential_autofill_detection_should_run_this_tick(
            detection_was_pending,
            credential_autofill_pending_detection_can_run(
                self.session.active_id(),
                self.terminal.assist.credential_autofill_detection_pending,
                runtime_backlog,
                match_request_pending,
            ),
        ) {
            return root_overlay_dirty;
        }
        self.terminal.assist.credential_autofill_detection_pending = false;
        let _ = self.detect_credential_prompt(cx);
        root_overlay_dirty
    }

    /// Test adapter for the snapshot-driven credential prompt tick.
    #[cfg(test)]
    pub(in crate::features) fn sync_credential_autofill_for_test(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        self.sync_credential_autofill_from_active_snapshot(cx)
    }

    fn sync_credential_autofill_from_active_snapshot(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        let view = self.terminal.view.views.get(&session_id);
        // Missing frames are not evidence that a no-echo prompt ended.
        let Some(row) = view
            .and_then(|view| view.frame_snapshot.as_deref())
            .and_then(credential_autofill_prompt_row_from_snapshot)
        else {
            return false;
        };
        let instance = Some((row.line_id, row.revision));
        let prompt_text = credential_autofill_prompt_text_from_visible(&row.text);
        let dirty = self
            .terminal
            .assist
            .observe_credential_prompt_instance(instance);
        self.sync_credential_autofill_prompt_text(&session_id, prompt_text, cx) || dirty
    }

    fn sync_credential_autofill_prompt_text(
        &mut self,
        session_id: &str,
        prompt_text: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.session.active_id() != Some(session_id) {
            return false;
        }
        let now = Self::now_unix_ms();
        if self
            .terminal
            .assist
            .credential_autofill_pending
            .as_ref()
            .is_some_and(|pending| pending.session_id != session_id || pending.expires_at_ms <= now)
        {
            self.terminal.assist.credential_autofill_pending = None;
        }
        if prompt_text.is_empty() {
            self.terminal.assist.credential_autofill_buffer.clear();
            if self.terminal.assist.credential_autofill_pending.is_none()
                && !self.terminal.assist.credential_autofill_sending
            {
                self.terminal.assist.credential_prompt_input_until_ms = 0;
            }
            return false;
        }
        if self.terminal.assist.credential_autofill_buffer == prompt_text
            && (self.terminal.assist.credential_autofill_detection_pending
                || self.terminal.assist.credential_autofill_pending.is_none())
        {
            return false;
        }
        let detected_prompt_kind = credential_autofill_detect_prompt_kind(&prompt_text);
        if detected_prompt_kind.is_none()
            && self.terminal.assist.credential_autofill_pending.is_none()
        {
            // The visible prompt is no longer a credential prompt, so the
            // no-echo input window must end with the prompt it guarded. The
            // buffer is often already empty here because a prompt-instance
            // change cleared it, so clearing the deadline must not depend on
            // the buffer still holding text: a prompt such as `su`'s child
            // shell would otherwise keep discarding every keystroke for the
            // remainder of the TTL.
            self.terminal.assist.credential_autofill_buffer.clear();
            self.terminal.assist.credential_prompt_input_until_ms = 0;
            return false;
        }

        if self.terminal.assist.credential_autofill_buffer != prompt_text {
            self.terminal.assist.credential_autofill_buffer = prompt_text;
        }
        let mut root_overlay_dirty = false;
        if detected_prompt_kind.is_some() {
            self.terminal.assist.credential_prompt_input_until_ms =
                Self::now_unix_ms().saturating_add(CREDENTIAL_PROMPT_INPUT_TTL_MS);
            // Suppress command suggestions while a credential prompt is live.
            if self.terminal.assist.commands.popup.take().is_some() {
                root_overlay_dirty = true;
            }
            *self.terminal.editing.input_mut() = TerminalInputState::new();
        }

        if self.terminal.assist.credential_suggestions.is_some()
            || self.terminal.assist.credential_autofill_sending
        {
            return root_overlay_dirty;
        }
        if !self.terminal.assist.credential_autofill_detection_pending {
            self.terminal.assist.credential_autofill_detection_pending = true;
        }
        let _ = cx;
        root_overlay_dirty
    }

    pub(in crate::features) fn detect_credential_prompt(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> bool {
        if !self.credential_autofill_enabled()
            || self.session.active_id().is_none()
            || self.terminal.assist.credential_suggestions.is_some()
        {
            return false;
        }

        let now = Self::now_unix_ms();
        let prompt_text = credential_autofill_prompt_text_from_visible(
            &self.terminal.assist.credential_autofill_buffer,
        );
        if prompt_text.is_empty() {
            return false;
        }
        let Some(prompt_kind) = credential_autofill_detect_prompt_kind(&prompt_text) else {
            return false;
        };
        let current_line = prompt_text.trim().to_string();
        let Some(active_session_id) = self.session.active_id_owned() else {
            return false;
        };
        let mut credentials: Vec<CredentialAutofillTarget> = self
            .security
            .credentials()
            .iter()
            .cloned()
            .map(CredentialAutofillTarget::Vault)
            .collect();
        credentials.extend(
            self.security
                .otp_entries()
                .iter()
                .filter_map(otp_prompt_target),
        );
        if credentials.is_empty() {
            return false;
        }

        if let Some(pending) = self.terminal.assist.credential_autofill_pending.clone()
            && pending.expires_at_ms <= now
        {
            self.terminal.assist.credential_autofill_pending = None;
        }

        if let Some(pending) = self.terminal.assist.credential_autofill_pending.clone()
            && pending.session_id != active_session_id
        {
            self.terminal.assist.credential_autofill_pending = None;
        }

        if self.terminal.assist.credential_autofill_pending.is_none()
            && !self
                .terminal
                .assist
                .remember_credential_prompt(prompt_kind, &prompt_text)
        {
            return false;
        }

        self.terminal.assist.credential_autofill_next_request_id = self
            .terminal
            .assist
            .credential_autofill_next_request_id
            .saturating_add(1);
        let key = CredentialAutofillMatchRequestKey {
            request_id: self.terminal.assist.credential_autofill_next_request_id,
            session_id: active_session_id,
            prompt_text,
        };
        self.terminal.assist.credential_autofill_pending_request = Some(key.clone());
        self.terminal
            .assist
            .credential_autofill_match_pipeline
            .request(CredentialAutofillMatchRequest {
                key,
                current_line,
                prompt_kind,
                credentials,
                pending: self.terminal.assist.credential_autofill_pending.clone(),
            });
        true
    }

    /// Deliver credential-autofill match replies as they arrive.
    ///
    /// Started once at window open. The reply queue dedups per (session, prompt)
    /// and drops the oldest under pressure, so it stays a queue and only the wake
    /// signal is a channel; see `models::event_wake`.
    ///
    /// Only the *reply* half moves here. Prompt detection still runs on the
    /// runtime tick, because it scans the terminal frame snapshot and
    /// deliberately holds off while output backlog is high -- that belongs with
    /// the terminal frame work, driven off a frame being applied.
    pub(in crate::features) fn start_credential_autofill_match_drain(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(mut wake_rx) = self
            .terminal
            .assist
            .credential_autofill_match_pipeline
            .take_wake_receiver()
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                // Arm before draining: a reply pushed between the drain and the
                // arm would otherwise go unsignalled and sit until something
                // unrelated woke us.
                let drained = this.update(cx, |this, cx| {
                    this.terminal
                        .assist
                        .credential_autofill_match_pipeline
                        .arm_event_wake();
                    let dirty = this.drain_credential_autofill_match_events(cx);
                    if dirty {
                        cx.notify();
                    }
                    dirty
                });
                match drained {
                    Err(_) => break,
                    // Applying one reply can leave more queued; keep going before
                    // sleeping, rather than waiting for the next signal.
                    Ok(true) => continue,
                    Ok(false) => {}
                }
                if wake_rx.next().await.is_none() {
                    break;
                }
            }
        })
        .detach();
    }

    fn drain_credential_autofill_match_events(&mut self, cx: &mut Context<Self>) -> bool {
        let mut dirty = false;
        while let Some(event) = self
            .terminal
            .assist
            .credential_autofill_match_pipeline
            .try_recv_event()
        {
            dirty |= self.apply_credential_autofill_match_event(event, cx);
        }
        dirty
    }

    fn apply_credential_autofill_match_event(
        &mut self,
        event: CredentialAutofillMatchEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.credential_autofill_enabled()
            || self
                .terminal
                .assist
                .credential_autofill_pending_request
                .as_ref()
                != Some(&event.key)
        {
            return false;
        }
        self.terminal.assist.credential_autofill_pending_request = None;
        if self.session.active_id() != Some(event.key.session_id.as_str()) {
            return false;
        }
        if credential_autofill_prompt_text_from_visible(
            &self.terminal.assist.credential_autofill_buffer,
        ) != event.key.prompt_text
        {
            return false;
        }

        match credential_autofill_action(&event.outcome) {
            CredentialAutofillAction::Send { target, kind } => {
                self.terminal.assist.credential_autofill_pending = None;
                self.terminal.assist.credential_autofill_buffer.clear();
                self.terminal
                    .assist
                    .remember_credential_prompt(kind, &event.key.prompt_text);
                self.send_credential_value(&target, kind, &event.key.session_id, cx);
                true
            }
            CredentialAutofillAction::Suggest {
                kind,
                matches,
                clear_pending,
            } => {
                if clear_pending {
                    self.terminal.assist.credential_autofill_pending = None;
                }
                // Nothing is sent here: a suggestion is an offer, and
                // `select_credential_suggestion` is the single place that fills
                // one, behind a user action.
                self.show_credential_panel(kind, matches, event.key.prompt_text, cx);
                true
            }
            CredentialAutofillAction::None { clear_pending } => {
                if clear_pending {
                    self.terminal.assist.credential_autofill_pending = None;
                }
                false
            }
        }
    }

    fn send_credential_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) -> bool {
        // Credential fills must never enter sync fanout, history, or recording,
        // even when terminal output has not identified a sensitive prompt.
        if !self.send_sensitive_input_to_active_session(bytes, cx) {
            return false;
        }
        self.terminal.assist.credential_prompt_awaiting_output = true;
        true
    }

    fn send_credential_value(
        &mut self,
        target: &CredentialAutofillTarget,
        kind: CredentialPromptKind,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.credential_autofill_enabled() || session_id.is_empty() {
            return;
        }
        if self.session.is_disconnected(session_id) {
            self.shell.set_status(
                "session disconnected - reconnect before filling credentials".to_string(),
            );
            cx.notify();
            return;
        }
        if self.session.active_id() != Some(session_id) {
            self.activate_session_id_with_surface_sync(session_id, cx);
        }
        let generation = self.terminal.assist.credential_autofill_generation;
        let credential_name = credential_autofill_target_display_name(target);
        match (kind, target) {
            (CredentialPromptKind::Username, CredentialAutofillTarget::Vault(credential)) => {
                let mut payload = credential.username.clone();
                payload.push('\r');
                if self.send_credential_input(payload.into_bytes(), cx) {
                    self.shell
                        .set_status(format!("filled username from '{credential_name}'"));
                }
            }
            // The matcher never offers an OTP entry for a username prompt.
            (CredentialPromptKind::Username, CredentialAutofillTarget::Otp(_)) => {}
            (CredentialPromptKind::Password, CredentialAutofillTarget::Vault(credential)) => {
                let credential_id = credential.id.clone();
                let closure_name = credential_name.clone();
                let session_id = session_id.to_string();
                let submitted = self.submit_store_request(
                    0,
                    store_request(StoreDomain::Security, move |store| {
                        store.load_decrypted_credential_by_id(&credential_id)
                    }),
                    move |this, event, cx| {
                        if !this.terminal.assist.credential_fill_is_current(
                            this.credential_autofill_enabled(),
                            generation,
                        ) {
                            return;
                        }
                        if this.session.active_id() != Some(session_id.as_str()) {
                            this.shell.set_status(
                                "credential fill cancelled because the active session changed"
                                    .to_string(),
                            );
                            cx.notify();
                            return;
                        }
                        if this.session.is_disconnected(&session_id) {
                            this.shell.set_status(
                                "session disconnected - reconnect before filling credentials"
                                    .to_string(),
                            );
                            cx.notify();
                            return;
                        }
                        match event.outcome {
                            Ok(Some(entry)) => {
                                let Some(mut password) =
                                    entry.password.filter(|value| !value.is_empty())
                                else {
                                    this.shell.set_status(format!(
                                        "credential '{closure_name}' has no password"
                                    ));
                                    cx.notify();
                                    return;
                                };
                                password.expose_secret_mut().push('\r');
                                if this
                                    .send_credential_input(password.into_secret().into_bytes(), cx)
                                {
                                    this.shell.set_status(format!(
                                        "filled password from '{closure_name}'"
                                    ));
                                }
                            }
                            Ok(None) => this
                                .shell
                                .set_status(format!("credential '{closure_name}' was not found")),
                            Err(error) => this.shell.set_status(format!(
                                "failed to load credential '{closure_name}': {error}"
                            )),
                        }
                        cx.notify();
                    },
                    cx,
                );
                if submitted {
                    self.shell
                        .set_status(format!("loading password from '{credential_name}'"));
                }
            }
            (CredentialPromptKind::Password, CredentialAutofillTarget::Otp(otp)) => {
                // The panel never holds a code: it is generated here, so what
                // gets typed is always from the current step.
                let otp_id = otp.id.clone();
                let closure_name = credential_name.clone();
                let session_id = session_id.to_string();
                let provider = self.session.otp_provider();
                let task = self
                    .blocking_jobs
                    .submit_task("credential-autofill-otp", move |_| {
                        provider.preview_otp_code(&otp_id)
                    });
                self.shell
                    .set_status(format!("generating code from '{credential_name}'"));
                cx.spawn(async move |this, cx| {
                    let result = await_blocking_job(task).await.and_then(|result| result);
                    let _ = this.update(cx, |this, cx| {
                        if !this.terminal.assist.credential_fill_is_current(
                            this.credential_autofill_enabled(),
                            generation,
                        ) {
                            return;
                        }
                        if this.session.active_id() != Some(session_id.as_str()) {
                            this.shell.set_status(
                                "credential fill cancelled because the active session changed"
                                    .to_string(),
                            );
                            cx.notify();
                            return;
                        }
                        if this.session.is_disconnected(&session_id) {
                            this.shell.set_status(
                                "session disconnected - reconnect before filling credentials"
                                    .to_string(),
                            );
                            cx.notify();
                            return;
                        }
                        match result {
                            Ok(Some(preview)) => {
                                let mut payload = preview.code.into_bytes();
                                payload.push(b'\r');
                                if this.send_credential_input(payload, cx) {
                                    this.shell
                                        .set_status(format!("filled code from '{closure_name}'"));
                                }
                            }
                            Ok(None) => this
                                .shell
                                .set_status(format!("OTP entry '{closure_name}' was not found")),
                            Err(error) => this.shell.set_status(format!(
                                "failed to generate code from '{closure_name}': {error}"
                            )),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Fills the highlighted suggestion. Returns false when the panel has no
    /// selection, so the caller can let Enter reach the terminal instead of
    /// submitting an unchosen password.
    pub(in crate::features) fn apply_selected_credential_suggestion(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.credential_autofill_enabled() {
            return false;
        }
        let Some(state) = self.terminal.assist.credential_suggestions.clone() else {
            return false;
        };
        let Some(index) = state.selected_index else {
            return false;
        };
        let Some(target) = state.matches.get(index).cloned() else {
            return false;
        };
        self.select_credential_suggestion(target, cx);
        true
    }

    pub(in crate::features) fn select_credential_suggestion(
        &mut self,
        target: CredentialAutofillTarget,
        cx: &mut Context<Self>,
    ) {
        if !self.credential_autofill_enabled() {
            return;
        }
        let Some(state) = self.terminal.assist.credential_suggestions.clone() else {
            return;
        };
        if self.terminal.assist.credential_autofill_sending {
            return;
        }
        let was_username = state.kind == CredentialPromptKind::Username;
        self.terminal.assist.credential_autofill_sending = true;
        self.send_credential_value(&target, state.kind, &state.session_id, cx);
        if was_username {
            // Only a username suggestion enters the pending password flow, and
            // the matcher never offers an OTP entry for a username prompt.
            if let CredentialAutofillTarget::Vault(credential) = &target {
                self.terminal.assist.credential_autofill_pending =
                    Some(PendingCredentialAutofill {
                        session_id: state.session_id.clone(),
                        credential_id: credential.id.clone(),
                        expires_at_ms: Self::now_unix_ms().saturating_add(PENDING_PASSWORD_TTL_MS),
                    });
            }
        } else {
            self.terminal.assist.credential_autofill_pending = None;
        }
        self.terminal.assist.credential_autofill_sending = false;
        self.terminal.assist.credential_suggestions = None;
        self.terminal
            .assist
            .remember_credential_prompt(state.kind, &state.prompt_text);

        if was_username {
            // Keep buffer so a password prompt that arrived during selection can still be detected.
            if !self.terminal.assist.credential_autofill_buffer.is_empty() {
                self.detect_credential_prompt(cx);
            }
        } else {
            self.terminal.assist.credential_autofill_buffer.clear();
        }
        cx.notify();
    }

    /// Handle credential panel keys. Returns true when the key was consumed.
    pub(in crate::features) fn handle_credential_suggestion_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.terminal.assist.credential_suggestions.as_ref() else {
            return false;
        };
        if state.matches.is_empty() {
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            // A chord such as Ctrl+V is input the panel cannot act on, so it
            // dismisses like any other key. Leaving the panel open let the next
            // Enter submit the highlighted password on top of the pasted one.
            self.dismiss_credential_suggestions(cx);
            return false;
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.dismiss_credential_suggestions(cx);
                true
            }
            "up" => {
                if let Some(state) = self.terminal.assist.credential_suggestions.as_mut() {
                    state.selected_index = credential_suggestion_step_selection(
                        state.selected_index,
                        state.matches.len(),
                        -1,
                    );
                    cx.notify();
                }
                true
            }
            "down" => {
                if let Some(state) = self.terminal.assist.credential_suggestions.as_mut() {
                    state.selected_index = credential_suggestion_step_selection(
                        state.selected_index,
                        state.matches.len(),
                        1,
                    );
                    cx.notify();
                }
                true
            }
            "enter" => {
                if self.apply_selected_credential_suggestion(cx) {
                    true
                } else {
                    self.dismiss_credential_suggestions(cx);
                    false
                }
            }
            _ => {
                // Typing while the panel is open dismisses it (Tauri parity).
                self.dismiss_credential_suggestions(cx);
                false
            }
        }
    }

    pub(in crate::features) fn credential_suggestions_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let Some(state) = self.terminal.assist.credential_suggestions.as_ref() else {
            return div().into_any_element();
        };
        if state.matches.is_empty() {
            return div().into_any_element();
        }

        let Some((cursor_row, cursor_col)) =
            self.suggestion_cursor_cell_for_session(&state.session_id, cx)
        else {
            return div().into_any_element();
        };
        let menu_w = 340.0_f32;
        let menu_h = suggestion_overlay_desired_height(state.matches.len(), 36.0);
        let Some(placement) = self.suggestion_overlay_position_for_session(
            Some(&state.session_id),
            cursor_row,
            cursor_col,
            menu_w,
            menu_h,
        ) else {
            return div().into_any_element();
        };

        let title = t!(match state.kind {
            CredentialPromptKind::Password => "credentialAutofill.passwordTitle",
            CredentialPromptKind::Username => "credentialAutofill.usernameTitle",
        });
        let kind_icon = match state.kind {
            CredentialPromptKind::Password => "icons/auth.svg",
            CredentialPromptKind::Username => "icons/sessions.svg",
        };
        let footer = format!(
            "↑↓ {} · Enter {} · Esc {}",
            t!("credentialAutofill.select"),
            t!("credentialAutofill.fill"),
            t!("credentialAutofill.dismiss")
        );

        let mut list = div()
            .id(SharedString::from("credential-suggestions-list"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();

        for (index, credential) in state.matches.iter().enumerate() {
            let selected = state.selected_index == Some(index);
            list = list.child(
                div()
                    .id(SharedString::from(format!("credential-suggestion-{index}")))
                    .h(px(36.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_l_2()
                    .border_color(rgb(if selected {
                        palette.primary
                    } else {
                        palette.surface
                    }))
                    .bg(rgb(if selected {
                        palette.hover
                    } else {
                        palette.surface
                    }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(state) = this.terminal.assist.credential_suggestions.as_mut() {
                            state.selected_index = Some(index);
                        }
                        let Some(selected) = this
                            .terminal
                            .assist
                            .credential_suggestions
                            .as_ref()
                            .and_then(|state| state.matches.get(index).cloned())
                        else {
                            return;
                        };
                        this.select_credential_suggestion(selected, cx);
                    }))
                    .child(svg().size(px(14.)).flex_none().path(kind_icon).text_color(
                        if selected {
                            rgb(palette.accent)
                        } else {
                            rgb(palette.text_dimmed)
                        },
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(rgb(palette.text))
                                            .child(truncate_preview(
                                                &credential_autofill_target_display_name(
                                                    credential,
                                                ),
                                                36,
                                            )),
                                    )
                                    .when(credential.is_otp(), |row| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .px(px(4.))
                                                .rounded(px(3.))
                                                .bg(rgb(palette.hover))
                                                .text_size(px(9.))
                                                .text_color(rgb(palette.accent))
                                                .child(t!("credentialAutofill.otpBadge")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(truncate_preview(
                                        credential_autofill_target_subtitle(credential),
                                        40,
                                    )),
                            ),
                    ),
            );
        }

        div()
            .id(SharedString::from("credential-suggestions-overlay"))
            .absolute()
            .occlude()
            .left(px(placement.x))
            .top(px(placement.y))
            .w(px(menu_w))
            .h(px(placement.height))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgba((palette.surface << 8) | 0xf2))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_HEADER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(10.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path(kind_icon)
                                    .text_color(rgb(palette.text_dimmed)),
                            )
                            .child(title),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(format!("{}", state.matches.len())),
                    ),
            )
            .child(list)
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_FOOTER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(footer),
            )
            .into_any_element()
    }
}

/// Move the panel highlight by one row, treating `None` as "no selection".
/// Stepping past either end returns to no selection, so an unchosen list keeps
/// requiring an explicit pick. Mirrors the command-suggestion panel.
fn credential_suggestion_step_selection(
    current: Option<usize>,
    len: usize,
    direction: i32,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if direction > 0 {
        return match current {
            None => Some(0),
            Some(index) if index + 1 < len => Some(index + 1),
            Some(_) => None,
        };
    }
    if direction < 0 {
        return match current {
            None => Some(len - 1),
            Some(0) => None,
            Some(index) => Some(index.min(len - 1).saturating_sub(1)),
        };
    }
    current.map(|index| index.min(len - 1))
}

fn credential_autofill_snapshot_detection_can_run(
    active_session_id: Option<&str>,
    backlog: CredentialAutofillRuntimeBacklog,
    match_request_pending: bool,
) -> bool {
    active_session_id.is_some() && backlog.is_empty() && !match_request_pending
}

fn credential_autofill_pending_detection_can_run(
    active_session_id: Option<&str>,
    detection_pending: bool,
    backlog: CredentialAutofillRuntimeBacklog,
    match_request_pending: bool,
) -> bool {
    active_session_id.is_some() && detection_pending && backlog.is_empty() && !match_request_pending
}

#[derive(Clone, Copy, Default)]
struct CredentialAutofillRuntimeBacklog {
    queued_output_bytes: usize,
    pending_session_events: usize,
    pending_terminal_frame_events: usize,
    queued_terminal_frame_events: usize,
    queued_terminal_frame_output_bytes: usize,
}

impl CredentialAutofillRuntimeBacklog {
    fn is_empty(self) -> bool {
        self.queued_output_bytes == 0
            && self.pending_session_events == 0
            && self.pending_terminal_frame_events == 0
            && self.queued_terminal_frame_events == 0
            && self.queued_terminal_frame_output_bytes == 0
    }
}

fn credential_autofill_detection_should_run_this_tick(
    detection_was_pending: bool,
    can_run: bool,
) -> bool {
    detection_was_pending && can_run
}

fn credential_autofill_prompt_text_from_snapshot(snapshot: &TerminalSnapshot) -> Option<String> {
    let line = credential_autofill_prompt_line_from_snapshot(snapshot)?;
    Some(credential_autofill_prompt_text_from_visible(line))
}

fn credential_autofill_prompt_line_from_snapshot(snapshot: &TerminalSnapshot) -> Option<&str> {
    credential_autofill_prompt_row_from_snapshot(snapshot).map(|row| row.text.as_str())
}

fn credential_autofill_prompt_row_from_snapshot(
    snapshot: &TerminalSnapshot,
) -> Option<&nyaterm_terminal::TerminalSnapshotRow> {
    if snapshot.cursor.row != usize::MAX {
        return snapshot.row(snapshot.cursor.row);
    }
    snapshot
        .rows()
        .iter()
        .rev()
        .find(|row| !row.text.trim().is_empty())
        .map(AsRef::as_ref)
}

#[cfg(test)]
fn credential_autofill_prompt_line_from_viewport(
    lines: &[String],
    cursor_row: usize,
) -> Option<&str> {
    if lines.is_empty() {
        return None;
    }
    if cursor_row != usize::MAX {
        return lines.get(cursor_row).map(String::as_str);
    }
    lines
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(String::as_str)
}

fn credential_autofill_visible_tail(text: &str) -> &str {
    if text.len() <= CREDENTIAL_AUTOFILL_INPUT_TAIL_LIMIT {
        return text;
    }
    let mut start = text.len() - CREDENTIAL_AUTOFILL_INPUT_TAIL_LIMIT;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn credential_autofill_prompt_text_from_visible(output: &str) -> String {
    if output
        .chars()
        .last()
        .is_some_and(|ch| ch == '\r' || ch == '\n')
    {
        return String::new();
    }

    let tail = credential_autofill_visible_tail(output);
    let prompt_start = tail.rfind(['\r', '\n']).map(|index| index + 1).unwrap_or(0);
    let prompt = tail[prompt_start..].trim();
    let prompt_len = prompt.chars().count();
    if prompt_len > 500 {
        prompt.chars().skip(prompt_len - 500).collect::<String>()
    } else {
        prompt.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CREDENTIAL_AUTOFILL_INPUT_TAIL_LIMIT, CredentialAutofillRuntimeBacklog,
        credential_autofill_detect_prompt_kind, credential_autofill_detection_should_run_this_tick,
        credential_autofill_pending_detection_can_run,
        credential_autofill_prompt_line_from_viewport,
        credential_autofill_prompt_text_from_visible,
        credential_autofill_snapshot_detection_can_run, credential_autofill_visible_tail,
        credential_suggestion_step_selection,
    };
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::credential_autofill_target_display_name;
    use crate::models::{CredentialAutofillTarget, SessionLaunchConfig};
    use crate::test_support::TestConfigDir;
    use gpui::{AppContext as _, TestAppContext};

    #[test]
    fn credential_window_ends_at_a_real_empty_prompt_but_survives_missing_frames() {
        let dir = TestConfigDir::new("nyaterm-empty-credential-prompt");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            for prompt in ["", "   ", "$ "] {
                app.terminal
                    .seed_session_view("s1".into(), "Password: ".into(), "UTF-8");
                let view = app.terminal.view.views.get_mut("s1").unwrap();
                view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
                app.sync_credential_autofill_from_active_snapshot(cx);
                assert!(app.is_credential_prompt_input_mode());
                app.note_shell_editing_input(b"synthetic-no-echo", cx);
                assert!(app.terminal.editing.predicted_input().value.is_empty());
                app.terminal
                    .view
                    .views
                    .get_mut("s1")
                    .unwrap()
                    .frame_snapshot = None;
                app.sync_credential_autofill_from_active_snapshot(cx);
                assert!(app.is_credential_prompt_input_mode());

                app.terminal.seed_session_view(
                    "s1".into(),
                    format!("Password: \r\n{prompt}"),
                    "UTF-8",
                );
                let view = app.terminal.view.views.get_mut("s1").unwrap();
                view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
                app.sync_credential_autofill_from_active_snapshot(cx);
                assert!(!app.is_credential_prompt_input_mode());
                assert!(app.terminal.assist.credential_autofill_buffer.is_empty());
            }
        });
    }

    #[test]
    fn empty_prompt_preserves_only_current_unexpired_pending_credentials() {
        let dir = TestConfigDir::new("nyaterm-empty-pending-credential");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            for (session, expiry, protected) in [
                ("s1", u64::MAX, true),
                ("s1", 0, false),
                ("other", u64::MAX, false),
            ] {
                app.terminal.assist.credential_prompt_input_until_ms = u64::MAX;
                app.terminal.assist.credential_autofill_pending =
                    Some(crate::models::PendingCredentialAutofill {
                        session_id: session.into(),
                        credential_id: "fixture".into(),
                        expires_at_ms: expiry,
                    });
                app.sync_credential_autofill_prompt_text("s1", String::new(), cx);
                assert_eq!(app.is_credential_prompt_input_mode(), protected);
                assert_eq!(
                    app.terminal.assist.credential_autofill_pending.is_some(),
                    protected
                );
            }
        });
    }
    use nyaterm_core::{
        AiExecutionProfile, ConnectionAuth, ConnectionPasswordSource, ConnectionType,
        CredentialPromptKind, SavedConnection,
    };
    use nyaterm_transport::SshSessionConfig;

    fn backlog(
        queued_output_bytes: usize,
        pending_session_events: usize,
        pending_terminal_frame_events: usize,
        queued_terminal_frame_events: usize,
        queued_terminal_frame_output_bytes: usize,
    ) -> CredentialAutofillRuntimeBacklog {
        CredentialAutofillRuntimeBacklog {
            queued_output_bytes,
            pending_session_events,
            pending_terminal_frame_events,
            queued_terminal_frame_events,
            queued_terminal_frame_output_bytes,
        }
    }

    #[test]
    fn credential_autofill_snapshot_detection_requires_active_session_and_no_pending_match() {
        assert!(credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            None,
            backlog(0, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 0, 0, 0, 0),
            true
        ));
    }

    #[test]
    fn credential_autofill_snapshot_detection_waits_for_all_backlogs() {
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(1, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 1, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 0, 1, 0, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 0, 0, 1, 0),
            false
        ));
        assert!(!credential_autofill_snapshot_detection_can_run(
            Some("active"),
            backlog(0, 0, 0, 0, 1),
            false
        ));
    }

    #[test]
    fn credential_autofill_pending_detection_runs_only_when_idle() {
        assert!(credential_autofill_pending_detection_can_run(
            Some("active"),
            true,
            backlog(0, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            None,
            true,
            backlog(0, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            Some("active"),
            false,
            backlog(0, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            Some("active"),
            true,
            backlog(1, 0, 0, 0, 0),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            Some("active"),
            true,
            backlog(0, 0, 0, 1, 0),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            Some("active"),
            true,
            backlog(0, 0, 0, 0, 1),
            false
        ));
        assert!(!credential_autofill_pending_detection_can_run(
            Some("active"),
            true,
            backlog(0, 0, 0, 0, 0),
            true
        ));
    }

    #[test]
    fn credential_autofill_detection_waits_for_next_tick_after_snapshot_sync() {
        assert!(!credential_autofill_detection_should_run_this_tick(
            false, true
        ));
        assert!(credential_autofill_detection_should_run_this_tick(
            true, true
        ));
        assert!(!credential_autofill_detection_should_run_this_tick(
            true, false
        ));
    }

    #[test]
    fn credential_autofill_prompt_line_uses_cursor_row() {
        let lines = vec![
            "Last login".to_string(),
            "Password:".to_string(),
            "ignored:".to_string(),
        ];

        assert_eq!(
            credential_autofill_prompt_line_from_viewport(&lines, 1),
            Some("Password:")
        );
    }

    #[test]
    fn credential_autofill_prompt_line_falls_back_to_last_nonempty_line() {
        let lines = vec![
            "Last login".to_string(),
            "Password:".to_string(),
            "".to_string(),
        ];

        assert_eq!(
            credential_autofill_prompt_line_from_viewport(&lines, usize::MAX),
            Some("Password:")
        );
    }

    #[test]
    fn credential_autofill_visible_tail_caps_input_on_boundary() {
        let text = format!("{}密码：", "测".repeat(3000));
        let tail = credential_autofill_visible_tail(&text);

        assert!(tail.len() <= CREDENTIAL_AUTOFILL_INPUT_TAIL_LIMIT);
        assert!(tail.is_char_boundary(0));
        assert!(tail.ends_with("密码："));
    }

    #[test]
    fn credential_autofill_prompt_text_reads_visible_last_line() {
        assert_eq!(
            credential_autofill_prompt_text_from_visible("hello\nPassword: "),
            "Password:"
        );
        assert_eq!(
            credential_autofill_prompt_text_from_visible("Password:\n"),
            ""
        );

        let long = format!("{}Password: ", "x".repeat(700));
        let prompt = credential_autofill_prompt_text_from_visible(&long);
        assert_eq!(prompt.chars().count(), 500);
        assert!(prompt.ends_with("Password:"));
    }

    #[test]
    fn credential_autofill_detect_prompt_kind_without_regex() {
        assert_eq!(
            credential_autofill_detect_prompt_kind("Password:"),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            credential_autofill_detect_prompt_kind("login as:"),
            Some(CredentialPromptKind::Username)
        );
        assert_eq!(
            credential_autofill_detect_prompt_kind("密码："),
            Some(CredentialPromptKind::Password)
        );
        assert_eq!(
            credential_autofill_detect_prompt_kind("Password accepted"),
            None
        );
    }

    const SESSION_ID: &str = "credential-autofill-session";

    fn ssh_connection(username: &str, auth: Option<ConnectionAuth>) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            tags: Vec::new(),
            id: "conn-1".to_string(),
            name: "prod".to_string(),
            config: ConnectionType::Ssh {
                host: "host".into(),
                port: 22,
                username: username.to_string(),
                backspace_mode: "del".into(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            tree_order: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth,
            recording: None,
            ssh_algorithms: None,
            allow_short_rsa_host_keys: false,
            ssh_compatibility_mode: false,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn account_password_auth() -> ConnectionAuth {
        ConnectionAuth {
            mode: "password".into(),
            account_id: Some("account-1".into()),
            password_source: Some(ConnectionPasswordSource::Account),
            ..Default::default()
        }
    }

    /// An active SSH session whose login user came from the saved account, not
    /// from the catalog connection: the config says `dev` while the session
    /// really logged in as `root`.
    fn app_with_account_backed_session(
        cx: &mut TestAppContext,
        root: &TestConfigDir,
        connection: SavedConnection,
        login_username: &str,
    ) -> gpui::Entity<NyaTermApp> {
        let app = app_with_visible_local_session(cx, root.path(), SESSION_ID);
        cx.update_entity(&app, |app, _| {
            let mut metadata = app
                .session
                .metadata(SESSION_ID)
                .cloned()
                .expect("fixture session metadata");
            metadata.source_connection_id = Some(connection.id.clone());
            metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
                username: login_username.to_string(),
                ..SshSessionConfig::default()
            }));
            *app.session
                .metadata_mut(SESSION_ID)
                .expect("fixture session metadata") = metadata;
            app.connection_state
                .replace_loaded(vec![connection], Vec::new());
        });
        app
    }

    #[test]
    fn empty_vault_protects_manual_password_input_without_candidates() {
        for saved_password in [true, false] {
            let root = TestConfigDir::new("nyaterm-empty-vault-input-protection");
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let auth = saved_password.then(|| ConnectionAuth {
                mode: "password".into(),
                password_source: Some(ConnectionPasswordSource::Connection),
                password: Some(nyaterm_core::SecretString::from("connection-fixture")),
                ..Default::default()
            });
            let app = app_with_account_backed_session(
                &mut cx,
                &root,
                ssh_connection("root", auth),
                "root",
            );
            cx.update_entity(&app, |app, cx| {
                assert!(app.credential_autofill_enabled());
                assert!(app.security.credentials().is_empty());
                app.terminal.seed_session_view(
                    SESSION_ID.into(),
                    "[sudo] password for root: ".into(),
                    "UTF-8",
                );
                let view = app.terminal.view.views.get_mut(SESSION_ID).unwrap();
                view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
                // Exercise the production scheduler, including its idle fast path.
                app.drain_pending_credential_autofill_detection(cx);
                app.drain_pending_credential_autofill_detection(cx);
                assert!(app.is_credential_prompt_input_mode());
                assert!(app.terminal.assist.credential_suggestions.is_none());
                assert!(
                    app.terminal
                        .assist
                        .credential_autofill_pending_request
                        .is_none()
                );
                app.note_shell_editing_input(b"manual-password-fixture", cx);
                app.note_command_suggestion_input(b"manual-password-fixture", cx);
                assert!(app.terminal.editing.predicted_input().value.is_empty());
                assert!(app.terminal.editing.assist_input().value.is_empty());

                app.terminal.seed_session_view(
                    SESSION_ID.into(),
                    "[sudo] password for root: \r\nroot@host:~# ".into(),
                    "UTF-8",
                );
                let view = app.terminal.view.views.get_mut(SESSION_ID).unwrap();
                view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
                app.drain_pending_credential_autofill_detection(cx);
                assert!(!app.is_credential_prompt_input_mode());
                app.note_shell_editing_input(b"ls", cx);
                app.note_command_suggestion_input(b"ls", cx);
                assert_eq!(app.terminal.editing.predicted_input().value, "ls");
                assert_eq!(app.terminal.editing.assist_input().value, "ls");
            });
        }
    }

    #[test]
    fn credential_suggestion_selection_steps_out_at_the_ends() {
        assert_eq!(credential_suggestion_step_selection(None, 3, 1), Some(0));
        assert_eq!(credential_suggestion_step_selection(Some(0), 3, 1), Some(1));
        assert_eq!(credential_suggestion_step_selection(Some(1), 3, 1), Some(2));
        assert_eq!(credential_suggestion_step_selection(Some(2), 3, 1), None);
        assert_eq!(credential_suggestion_step_selection(None, 3, -1), Some(2));
        assert_eq!(
            credential_suggestion_step_selection(Some(2), 3, -1),
            Some(1)
        );
        assert_eq!(credential_suggestion_step_selection(Some(0), 3, -1), None);
        assert_eq!(credential_suggestion_step_selection(None, 0, 1), None);
        assert_eq!(credential_suggestion_step_selection(None, 0, -1), None);
        assert_eq!(credential_suggestion_step_selection(Some(0), 1, 1), None);
        assert_eq!(credential_suggestion_step_selection(Some(0), 1, -1), None);
    }
    fn test_credential_target(id: &str) -> CredentialAutofillTarget {
        use nyaterm_core::models::credentials::SavedCredential;
        CredentialAutofillTarget::Vault(SavedCredential {
            id: id.into(),
            name: id.into(),
            sort_order: 0,
            username: "user".into(),
            password: None,
            has_password: true,
            username_prompt_regex: None,
            password_prompt_regex: Some("Password:".into()),
            enabled: true,
        })
    }

    #[test]
    fn only_a_unique_match_is_preselected() {
        let root = TestConfigDir::new("nyaterm-credential-selection");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), SESSION_ID);
        cx.update_entity(&app, |app, cx| {
            for (count, expected) in [(1, Some(0)), (2, None)] {
                let targets = (0..count)
                    .map(|index| test_credential_target(&format!("c{index}")))
                    .collect();
                app.show_credential_panel(
                    CredentialPromptKind::Password,
                    targets,
                    "Password:".into(),
                    cx,
                );
                assert_eq!(
                    app.terminal
                        .assist
                        .credential_suggestions
                        .as_ref()
                        .unwrap()
                        .selected_index,
                    expected
                );
                if expected.is_none() {
                    assert!(!app.apply_selected_credential_suggestion(cx));
                }
                app.dismiss_credential_suggestions(cx);
            }
        });
    }

    #[test]
    fn disabling_autofill_cancels_the_password_pending_after_username_selection() {
        let root = TestConfigDir::new("nyaterm-credential-toggle-pending");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), SESSION_ID);
        cx.update_entity(&app, |app, cx| {
            let target = test_credential_target("c1");
            app.show_credential_panel(
                CredentialPromptKind::Username,
                vec![target.clone()],
                "Username:".into(),
                cx,
            );
            app.select_credential_suggestion(target, cx);
            assert!(app.terminal.assist.credential_autofill_pending.is_some());
            let generation = app.terminal.assist.credential_autofill_generation;
            app.toggle_credential_autofill(cx);
            assert!(
                !app.settings
                    .summary()
                    .interaction_credential_autofill_enabled
            );
            assert!(app.terminal.assist.credential_autofill_pending.is_none());
            assert!(app.terminal.assist.credential_suggestions.is_none());
            app.terminal.assist.credential_autofill_buffer = "Password:".into();
            assert!(!app.detect_credential_prompt(cx));
            app.toggle_credential_autofill(cx);
            assert!(
                !app.terminal
                    .assist
                    .credential_fill_is_current(true, generation)
            );
        });
    }

    #[test]
    fn disabled_autofill_rejects_pending_detection_and_late_match_events() {
        use crate::models::{
            CredentialAutofillMatchEvent, CredentialAutofillMatchRequestKey,
            PendingCredentialAutofill,
        };
        use nyaterm_core::credential_autofill::matching::CredentialAutofillMatchOutcome;
        let root = TestConfigDir::new("nyaterm-credential-disabled");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_account_backed_session(
            &mut cx,
            &root,
            ssh_connection("root", Some(account_password_auth())),
            "root",
        );
        cx.update_entity(&app, |app, cx| {
            let key = CredentialAutofillMatchRequestKey {
                request_id: 1,
                session_id: SESSION_ID.into(),
                prompt_text: "Password:".into(),
            };
            app.terminal.assist.credential_autofill_pending = Some(PendingCredentialAutofill {
                session_id: SESSION_ID.into(),
                credential_id: "c1".into(),
                expires_at_ms: u64::MAX,
            });
            app.terminal.assist.credential_autofill_pending_request = Some(key.clone());
            app.terminal.assist.credential_autofill_buffer = key.prompt_text.clone();
            app.terminal.assist.credential_autofill_detection_pending = true;
            let mut settings = app.settings.summary().clone();
            settings.interaction_credential_autofill_enabled = false;
            app.settings.replace_summary(settings);
            assert!(!app.detect_credential_prompt(cx));
            assert!(!app.apply_credential_autofill_match_event(
                CredentialAutofillMatchEvent {
                    key: key.clone(),
                    outcome: CredentialAutofillMatchOutcome::AutoFill {
                        credential: test_credential_target("c1"),
                        kind: CredentialPromptKind::Password
                    },
                },
                cx
            ));
            assert!(!app.drain_pending_credential_autofill_detection(cx));
            assert!(app.terminal.assist.credential_autofill_pending.is_none());
            assert!(
                app.terminal
                    .assist
                    .credential_autofill_pending_request
                    .is_none()
            );
            app.toggle_credential_autofill(cx);
            assert!(!app.apply_credential_autofill_match_event(
                CredentialAutofillMatchEvent {
                    key,
                    outcome: CredentialAutofillMatchOutcome::Suggest {
                        kind: CredentialPromptKind::Password,
                        matches: vec![test_credential_target("c1")],
                        clear_pending: true,
                    },
                },
                cx
            ));
            assert!(app.terminal.assist.credential_suggestions.is_none());
        });
    }
    #[test]
    fn credential_fills_skip_sync_peers_history_and_recording_without_prompt_detection() {
        use nyaterm_core::SecretString;
        use nyaterm_recording::recording::{
            ExistingFileBehavior, RecordingContext, RecordingMode, RecordingProfile,
            RecordingRotationPolicy,
        };
        use nyaterm_store::ConnectionStore;
        use nyaterm_transport::TelnetSessionConfig;
        use std::io::Read;
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let root = TestConfigDir::new("nyaterm-credential-sensitive-wire");
        let store = ConnectionStore::open(root.path().join("config")).unwrap();
        let CredentialAutofillTarget::Vault(mut vault) = test_credential_target("vault") else {
            panic!("test target must be a vault credential");
        };
        vault.username = "fixture-user".into();
        vault.password = Some(SecretString::from("vault-fixture"));
        store.save_credential(vault.clone()).unwrap();
        let connection = ssh_connection(
            "fixture-user",
            Some(ConnectionAuth {
                mode: "password".into(),
                password: Some(SecretString::from("connection-fixture")),
                ..Default::default()
            }),
        );
        store.save_connection(&connection).unwrap();
        drop(store);

        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (manager, ids) = cx.update_entity(&app, |app, cx| {
            app.start_command_persistence_event_drain(cx);
            let manager = app.session.manager_handle();
            let metadata = app.session.metadata("fixture").unwrap().clone();
            let mut ids = Vec::new();
            app.sync_input.create_group();
            for index in 0..2 {
                let config = TelnetSessionConfig {
                    host: "127.0.0.1".into(),
                    port,
                    raw_tcp: true,
                    encoding: "UTF-8".into(),
                    ..Default::default()
                };
                let info = manager.create_telnet_session(config.clone()).unwrap();
                let mut metadata = metadata.clone();
                metadata.launch_config = SessionLaunchConfig::Telnet(config);
                app.session.register_session_metadata(&info.id, metadata);
                app.terminal
                    .seed_session_view(info.id.clone(), "$ ".into(), "UTF-8");
                app.sync_input.toggle_selected_session(info.id.clone());
                app.recording
                    .writer()
                    .start(
                        info.id.clone(),
                        RecordingContext {
                            session_id: info.id.clone(),
                            session_name: "synthetic".into(),
                            connection_id: None,
                            connection_name: None,
                            group_path: None,
                            protocol: "telnet".into(),
                            host: None,
                            port: None,
                            username: None,
                            started_at: time::OffsetDateTime::now_utc(),
                        },
                        RecordingProfile {
                            mode: RecordingMode::Raw,
                            base_path: root.path().into(),
                            path_template: "unused.log".into(),
                            include_timestamps: false,
                            include_io_labels: false,
                            include_session_metadata: false,
                            rotation: RecordingRotationPolicy::Session,
                            existing_file_behavior: ExistingFileBehavior::Unique,
                            include_binary_transfer_payloads: false,
                            include_input: true,
                        },
                        Some(root.path().join(format!("{index}.bin"))),
                        4096,
                    )
                    .unwrap();
                ids.push(info.id);
            }
            app.session.select_active_session(&ids[0]);
            (manager, ids)
        });
        let mut streams = (0..2)
            .map(|_| listener.accept().unwrap().0)
            .collect::<Vec<_>>();
        for stream in &streams {
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
        }
        let vault_target = CredentialAutofillTarget::Vault(vault);
        for (kind, target) in [
            (CredentialPromptKind::Username, &vault_target),
            (CredentialPromptKind::Password, &vault_target),
        ] {
            let expected_status = format!(
                "filled {} from '{}'",
                if kind == CredentialPromptKind::Username {
                    "username"
                } else {
                    "password"
                },
                credential_autofill_target_display_name(target)
            );
            cx.update_entity(&app, |app, cx| {
                assert_eq!(app.sync_peer_session_ids(&ids[0]), vec![ids[1].clone()]);
                assert!(!app.recording_input_is_sensitive(&ids[0]));
                app.terminal.assist.credential_prompt_awaiting_output = false;
                app.shell.set_status(String::new());
                app.send_credential_value(target, kind, &ids[0], cx);
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                cx.run_until_parked();
                if cx.update_entity(&app, |app, _| app.shell.status() == expected_status) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "credential fill callback did not complete"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            cx.update_entity(&app, |app, _| {
                assert!(app.terminal.assist.credential_prompt_awaiting_output);
                app.terminal.assist.observe_credential_prompt_output();
                assert!(!app.terminal.assist.credential_prompt_awaiting_output);
            });
        }
        cx.update_entity(&app, |app, _| {
            for id in &ids {
                assert!(
                    app.session
                        .command_history_for(id)
                        .is_none_or(|history| history.is_empty())
                );
                let completion = app.recording.writer().stop_complete(id.clone()).unwrap();
                assert!(completion.error.is_none());
                assert!(
                    std::fs::read(completion.input_file_path.unwrap())
                        .unwrap()
                        .is_empty()
                );
                app.write_session_protocol_response(id, b"END").unwrap();
            }
            assert!(app.commands.command_history().is_empty());
            assert!(app.terminal.assist.commands.pending_history_entry.is_none());
        });
        let store_client = cx.update_entity(&app, |app, _| app.store_blocking_client());
        assert!(
            store_client
                .request_fn(nyaterm_store::StoreDomain::Commands, |store| store
                    .list_command_history(64))
                .unwrap()
                .is_empty()
        );
        for (stream, expected) in streams.iter_mut().zip([
            b"fixture-user\rvault-fixture\rEND".as_slice(),
            b"END".as_slice(),
        ]) {
            let mut received = vec![0; expected.len()];
            stream.read_exact(&mut received).unwrap();
            assert_eq!(received, expected);
        }
        for id in ids {
            manager.close(&id).unwrap();
        }
    }

    #[test]
    fn open_credential_popup_anchor_tracks_folded_frames_without_resetting_selection() {
        let dir = TestConfigDir::new("nyaterm-credential-fold-anchor");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        cx.update_entity(&app, |app, cx| {
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.screen.resize(80, 24);
            view.screen.advance(b"$ \x1b]133;B\x07run\r\n\x1b]133;C\x07one\r\ntwo\r\n\x1b]133;D;0\x07\x1b]133;A\x07Password: ");
            view.frame_snapshot = Some(std::sync::Arc::new(view.screen.snapshot()));
            let id = view.screen.snapshot().command_blocks[0].id;
            app.terminal.assist.credential_suggestions = Some(crate::models::CredentialSuggestionState {
                session_id: "s1".into(), kind: CredentialPromptKind::Password,
                matches: vec![test_credential_target("fold")], prompt_text: "Password: ".into(), selected_index: Some(0),
            });
            let original = app.terminal.assist.credential_suggestions.clone();
            let before = app.suggestion_cursor_cell_for_session("s1", cx).unwrap();
            for collapsed in [true, false] {
                let view = app.terminal.view.views.get_mut("s1").unwrap();
                view.screen.set_collapsed_commands(if collapsed { [id].into() } else { Default::default() });
                view.frame_snapshot = Some(std::sync::Arc::new(view.screen.snapshot()));
                let after = app.suggestion_cursor_cell_for_session("s1", cx).unwrap();
                assert_eq!(after, (before.0 - usize::from(collapsed), before.1));
                assert_eq!(app.terminal.assist.credential_suggestions, original);
            }
            assert!(app.suggestion_cursor_cell_for_session("other", cx).is_none());
        });
    }
}

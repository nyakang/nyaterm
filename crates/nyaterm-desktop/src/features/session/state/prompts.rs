//! session prompts state and transitions.

use super::SessionFeatureState;
use crate::features::session::auth_runtime::{
    AgentPromptBroker, AgentPromptRequest, CredentialPromptBroker, CredentialPromptRequest,
    CredentialPromptState, HostKeyPromptBroker, HostKeyPromptRequest,
    KeyboardInteractivePromptState, NativeOtpCodePreview, NativeOtpProvider,
};
use crate::features::session::duplicate_prompts::{
    SftpDuplicatePromptBroker, SftpDuplicatePromptState,
};
use crate::models::event_wake::{ANY_INTEREST, EventWake};
use futures::channel::mpsc::UnboundedReceiver;
use gpui::FocusHandle;
use std::sync::Arc;

/// Native authentication and transfer prompts tied to the session runtime.
pub(in crate::features::session) struct SessionPromptState {
    /// One wake shared by all four brokers: activation runs all four steps on
    /// any signal, and each is a cheap no-op when its queue is empty.
    pub(super) wake: EventWake,
    pub(super) wake_rx: Option<UnboundedReceiver<()>>,
    pub(super) duplicate_prompts: Arc<SftpDuplicatePromptBroker>,
    pub(super) active_duplicate_prompt: Option<SftpDuplicatePromptState>,
    pub(super) host_key_prompts: Arc<HostKeyPromptBroker>,
    pub(super) active_host_key_prompt: Option<HostKeyPromptRequest>,
    pub(super) credential_prompts: Arc<CredentialPromptBroker>,
    pub(super) active_credential_prompt: Option<CredentialPromptState>,
    pub(super) active_keyboard_interactive_prompt: Option<KeyboardInteractivePromptState>,
    pub(super) agent_prompts: Arc<AgentPromptBroker>,
    pub(super) active_agent_prompt: Option<AgentPromptRequest>,
    pub(super) credential_prompt_focus_pending: bool,
    /// True while the TOTP step-boundary clock task is alive.
    pub(super) totp_clock_armed: bool,
    pub(super) credential_focus: FocusHandle,
    pub(super) otp_provider: Arc<NativeOtpProvider>,
}

pub(in crate::features) enum PromptResolution<T> {
    Inactive,
    Changed,
    Ready(T),
}

pub(in crate::features) struct PromptInputTarget {
    pub id: String,
    pub seed: String,
    pub echo: bool,
    pub kind: nyaterm_ui::NyaInputKind,
}

impl SessionPromptState {
    pub(super) fn has_blocking_prompt(&self) -> bool {
        self.active_duplicate_prompt.is_some()
            || self.active_host_key_prompt.is_some()
            || self.active_credential_prompt.is_some()
            || self.active_keyboard_interactive_prompt.is_some()
            || self.active_agent_prompt.is_some()
    }

    pub(in crate::features) fn duplicate_broker(&self) -> Arc<SftpDuplicatePromptBroker> {
        Arc::clone(&self.duplicate_prompts)
    }

    pub(in crate::features) fn host_key_broker(&self) -> Arc<HostKeyPromptBroker> {
        Arc::clone(&self.host_key_prompts)
    }

    pub(in crate::features) fn credential_broker(&self) -> Arc<CredentialPromptBroker> {
        Arc::clone(&self.credential_prompts)
    }

    pub(in crate::features) fn agent_broker(&self) -> Arc<AgentPromptBroker> {
        Arc::clone(&self.agent_prompts)
    }

    pub(in crate::features) fn otp_provider(&self) -> Arc<NativeOtpProvider> {
        Arc::clone(&self.otp_provider)
    }

    pub(in crate::features) fn credential_focus(&self) -> &FocusHandle {
        &self.credential_focus
    }

    pub(in crate::features) fn credential_focus_is_pending(&self) -> bool {
        self.credential_prompt_focus_pending
    }

    pub(in crate::features) fn finish_credential_focus(&mut self) {
        self.credential_prompt_focus_pending = false;
    }

    pub(in crate::features) fn active_duplicate(&self) -> Option<&SftpDuplicatePromptState> {
        self.active_duplicate_prompt.as_ref()
    }

    pub(in crate::features) fn active_host_key(&self) -> Option<&HostKeyPromptRequest> {
        self.active_host_key_prompt.as_ref()
    }

    pub(in crate::features) fn active_credential(&self) -> Option<&CredentialPromptState> {
        self.active_credential_prompt.as_ref()
    }

    pub(in crate::features) fn active_keyboard_interactive(
        &self,
    ) -> Option<&KeyboardInteractivePromptState> {
        self.active_keyboard_interactive_prompt.as_ref()
    }

    pub(in crate::features) fn active_agent(&self) -> Option<&AgentPromptRequest> {
        self.active_agent_prompt.as_ref()
    }

    pub(in crate::features) fn has_active_credential(&self) -> bool {
        self.active_credential_prompt.is_some()
    }

    pub(in crate::features) fn has_active_keyboard_interactive(&self) -> bool {
        self.active_keyboard_interactive_prompt.is_some()
    }

    pub(in crate::features) fn has_active_ssh_auth(&self) -> bool {
        self.active_host_key_prompt.is_some()
            || self.active_credential_prompt.is_some()
            || self.active_keyboard_interactive_prompt.is_some()
            || self.active_agent_prompt.is_some()
    }

    /// Install one wake across all four brokers, before any of them can be
    /// handed to a transport thread.
    pub(super) fn new(otp_provider: Arc<NativeOtpProvider>, credential_focus: FocusHandle) -> Self {
        let (wake, wake_rx) = EventWake::new();
        let duplicate_prompts = Arc::new(SftpDuplicatePromptBroker::default());
        let host_key_prompts = Arc::new(HostKeyPromptBroker::default());
        let credential_prompts = Arc::new(CredentialPromptBroker::default());
        let agent_prompts = Arc::new(AgentPromptBroker::default());
        duplicate_prompts.set_wake(wake.clone());
        host_key_prompts.set_wake(wake.clone());
        credential_prompts.set_wake(wake.clone());
        agent_prompts.set_wake(wake.clone());
        Self {
            wake,
            wake_rx: Some(wake_rx),
            duplicate_prompts,
            active_duplicate_prompt: None,
            host_key_prompts,
            active_host_key_prompt: None,
            credential_prompts,
            active_credential_prompt: None,
            active_keyboard_interactive_prompt: None,
            agent_prompts,
            active_agent_prompt: None,
            credential_prompt_focus_pending: false,
            totp_clock_armed: false,
            credential_focus,
            otp_provider,
        }
    }

    pub(in crate::features) fn take_wake_receiver(&mut self) -> Option<UnboundedReceiver<()>> {
        self.wake_rx.take()
    }

    /// Declare interest in the next enqueued prompt. See `models::event_wake`:
    /// this must happen before the consumer checks the queues.
    pub(in crate::features) fn arm_wake(&self) {
        self.wake.arm(ANY_INTEREST);
    }

    /// Freeing the single active slot is what lets the next queued prompt in, and
    /// nothing is enqueued at that moment, so the activation pass has to be woken
    /// explicitly. Every `take_*` below does this.
    pub(in crate::features) fn signal_wake(&self) {
        self.wake.signal(ANY_INTEREST);
    }

    pub(in crate::features) fn take_host_key_resolution(
        &mut self,
        request_id: &str,
    ) -> PromptResolution<HostKeyPromptRequest> {
        let Some(request) = self.active_host_key_prompt.take() else {
            return PromptResolution::Inactive;
        };
        if request.id != request_id {
            self.active_host_key_prompt = Some(request);
            return PromptResolution::Changed;
        }
        self.signal_wake();
        PromptResolution::Ready(request)
    }

    pub(in crate::features) fn take_duplicate_resolution(
        &mut self,
        request_id: &str,
    ) -> PromptResolution<SftpDuplicatePromptState> {
        let Some(prompt) = self.active_duplicate_prompt.take() else {
            return PromptResolution::Inactive;
        };
        if prompt.id != request_id {
            self.active_duplicate_prompt = Some(prompt);
            return PromptResolution::Changed;
        }
        self.signal_wake();
        PromptResolution::Ready(prompt)
    }

    pub(in crate::features) fn active_credential_mut(
        &mut self,
    ) -> Option<&mut CredentialPromptState> {
        self.active_credential_prompt.as_mut()
    }

    pub(in crate::features) fn take_credential(&mut self) -> Option<CredentialPromptState> {
        let state = self.active_credential_prompt.take()?;
        self.credential_prompt_focus_pending = false;
        self.signal_wake();
        Some(state)
    }

    pub(in crate::features) fn take_keyboard_interactive(
        &mut self,
    ) -> Option<KeyboardInteractivePromptState> {
        let state = self.active_keyboard_interactive_prompt.take()?;
        self.credential_prompt_focus_pending = false;
        self.signal_wake();
        Some(state)
    }

    pub(in crate::features) fn take_agent_resolution(
        &mut self,
        request_id: &str,
    ) -> PromptResolution<AgentPromptRequest> {
        let Some(request) = self.active_agent_prompt.take() else {
            return PromptResolution::Inactive;
        };
        if request.id != request_id || request.is_resolved() {
            self.active_agent_prompt = Some(request);
            return PromptResolution::Changed;
        }
        self.signal_wake();
        PromptResolution::Ready(request)
    }

    pub(in crate::features) fn take_agent_changed(&self) -> bool {
        self.agent_prompts.take_changed()
    }

    /// Agent 成功或取消后由 transport 调用 `finish`，此处把已解决请求从
    /// UI 活动槽位移除，再允许队列中的下一个请求展示。
    pub(in crate::features) fn reconcile_agent(&mut self) -> bool {
        if self
            .active_agent_prompt
            .as_ref()
            .is_some_and(AgentPromptRequest::is_resolved)
        {
            self.active_agent_prompt = None;
            return true;
        }
        false
    }

    pub(in crate::features) fn keyboard_interactive_otp_id(&self) -> Option<String> {
        self.active_keyboard_interactive_prompt
            .as_ref()
            .and_then(|state| state.request.otp_id.clone())
    }

    pub(in crate::features) fn keyboard_interactive_otp_code(&self) -> Option<String> {
        self.active_keyboard_interactive_prompt
            .as_ref()
            .and_then(|state| state.otp_code.clone())
    }

    pub(in crate::features) fn apply_keyboard_interactive_otp_result(
        &mut self,
        result: Result<Option<NativeOtpCodePreview>, String>,
        clear_missing_time_step: bool,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        match result {
            Ok(Some(preview)) => {
                state.otp_code = Some(preview.code);
                state.otp_type = Some(preview.otp_type);
                state.otp_period = preview.period;
                state.otp_time_step = preview.time_step;
                state.otp_error = None;
                true
            }
            Ok(None) => {
                state.otp_code = None;
                if clear_missing_time_step {
                    state.otp_time_step = None;
                }
                state.otp_error = Some("OTP entry not found".to_string());
                false
            }
            Err(error) => {
                state.otp_code = None;
                state.otp_time_step = None;
                state.otp_error = Some(error);
                false
            }
        }
    }

    pub(in crate::features) fn send_keyboard_interactive_otp_to_response(
        &mut self,
    ) -> Option<(String, String)> {
        let state = self.active_keyboard_interactive_prompt.as_mut()?;
        let code = state.otp_code.clone()?;
        let response = state.responses.first_mut()?;
        *response = code;
        state.focused_index = 0;
        Some((state.id.clone(), response.clone()))
    }

    pub(in crate::features) fn advance_keyboard_interactive_focus(
        &mut self,
        backwards: bool,
    ) -> Option<PromptInputTarget> {
        let state = self.active_keyboard_interactive_prompt.as_mut()?;
        let prompt_count = state.responses.len();
        if prompt_count == 0 {
            return None;
        }
        state.focused_index = if backwards {
            state
                .focused_index
                .checked_sub(1)
                .unwrap_or(prompt_count - 1)
        } else {
            (state.focused_index + 1) % prompt_count
        };
        let index = state.focused_index;
        Some(PromptInputTarget {
            id: format!("ssh.keyboard-interactive.{}.{index}", state.id),
            seed: state.responses[index].clone(),
            echo: state.request.prompts[index].echo,
            kind: if index == 0 && state.request.otp_id.is_some() {
                nyaterm_ui::NyaInputKind::OneTimeCode
            } else {
                nyaterm_ui::NyaInputKind::Secret
            },
        })
    }

    pub(in crate::features) fn apply_credential_input(
        &mut self,
        prompt_id: &str,
        text: String,
    ) -> bool {
        let Some(state) = self.active_credential_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id
            || state
                .authentication
                .as_ref()
                .is_some_and(|form| form.is_busy())
        {
            return false;
        }
        state.value = text;
        true
    }

    pub(in crate::features) fn apply_keyboard_interactive_input(
        &mut self,
        prompt_id: &str,
        index: usize,
        text: String,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id {
            return false;
        }
        let Some(response) = state.responses.get_mut(index) else {
            return false;
        };
        *response = text;
        state.focused_index = index;
        true
    }

    pub(in crate::features) fn focus_keyboard_interactive_response(
        &mut self,
        prompt_id: &str,
        index: usize,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id || index >= state.responses.len() {
            return false;
        }
        state.focused_index = index;
        true
    }

    pub(in crate::features) fn active_input_target(&self) -> Option<PromptInputTarget> {
        if let Some(state) = self.active_credential_prompt.as_ref() {
            if state
                .authentication
                .as_ref()
                .is_some_and(|form| !form.uses_secret_input())
            {
                return None;
            }
            return Some(PromptInputTarget {
                id: format!("ssh.credential.{}", state.id),
                seed: state.value.clone(),
                echo: state.prompt.echo,
                kind: match state.prompt.kind {
                    nyaterm_transport::SshCredentialPromptKind::Password => {
                        nyaterm_ui::NyaInputKind::Password
                    }
                    _ => nyaterm_ui::NyaInputKind::Secret,
                },
            });
        }
        let state = self.active_keyboard_interactive_prompt.as_ref()?;
        let index = (!state.responses.is_empty())
            .then_some(state.focused_index.min(state.responses.len() - 1))?;
        Some(PromptInputTarget {
            id: format!("ssh.keyboard-interactive.{}.{index}", state.id),
            seed: state.responses[index].clone(),
            echo: state.request.prompts[index].echo,
            kind: if index == 0 && state.request.otp_id.is_some() {
                nyaterm_ui::NyaInputKind::OneTimeCode
            } else {
                nyaterm_ui::NyaInputKind::Secret
            },
        })
    }

    pub(in crate::features) fn clear_cancelled_attempt_prompts(&mut self) -> bool {
        let mut changed = false;
        for id in self.host_key_prompts.take_cancelled() {
            if self
                .active_host_key_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.id == id)
            {
                self.active_host_key_prompt = None;
                changed = true;
            }
        }
        for id in self.credential_prompts.take_cancelled() {
            if self
                .active_credential_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.id == id)
            {
                self.active_credential_prompt = None;
                changed = true;
            }
            if self
                .active_keyboard_interactive_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.id == id)
            {
                self.active_keyboard_interactive_prompt = None;
                changed = true;
            }
        }
        changed
    }

    pub(in crate::features) fn activate_next_host_key(&mut self) -> Option<String> {
        if self.active_host_key_prompt.is_some() || !self.host_key_prompts.has_pending() {
            return None;
        }
        let request = self.host_key_prompts.pop_pending()?;
        let host = request.host_key.host_identifier.clone();
        self.active_host_key_prompt = Some(request);
        Some(host)
    }

    pub(in crate::features) fn activate_next_agent(&mut self) -> Option<String> {
        if self.active_agent_prompt.is_some() || !self.agent_prompts.has_pending() {
            return None;
        }
        let request = self.agent_prompts.pop_pending()?;
        let target = request.target();
        self.active_agent_prompt = Some(request);
        Some(target)
    }

    pub(in crate::features) fn take_next_credential_request(
        &self,
    ) -> Option<CredentialPromptRequest> {
        if self.active_credential_prompt.is_some()
            || self.active_keyboard_interactive_prompt.is_some()
            || !self.credential_prompts.has_pending()
        {
            return None;
        }
        self.credential_prompts.pop_pending()
    }

    pub(in crate::features) fn activate_credential(&mut self, state: CredentialPromptState) {
        self.active_credential_prompt = Some(state);
        self.credential_prompt_focus_pending = true;
    }

    pub(in crate::features) fn activate_keyboard_interactive(
        &mut self,
        state: KeyboardInteractivePromptState,
    ) {
        self.active_keyboard_interactive_prompt = Some(state);
        self.credential_prompt_focus_pending = true;
    }

    /// Seconds until the active TOTP code's next step boundary, if one is showing.
    ///
    /// The preview only changes on that boundary, so this is both the latest moment
    /// that is still correct and the earliest worth waking for. `None` means nothing
    /// is showing a TOTP code and no clock is needed.
    pub(in crate::features) fn keyboard_totp_seconds_to_next_step(&self, now: u64) -> Option<u64> {
        let state = self.active_keyboard_interactive_prompt.as_ref()?;
        if state.otp_type.as_deref() != Some("totp") || state.otp_code.is_none() {
            return None;
        }
        let period = state.otp_period.max(1);
        Some(period - (now % period))
    }

    pub(in crate::features) fn keyboard_totp_refresh_otp_id(&self, now: u64) -> Option<String> {
        let state = self.active_keyboard_interactive_prompt.as_ref()?;
        if state.otp_type.as_deref() != Some("totp") || state.otp_code.is_none() {
            return None;
        }
        let current_step = now / state.otp_period.max(1);
        if state.otp_time_step == Some(current_step) {
            return None;
        }
        state.request.otp_id.clone()
    }

    pub(in crate::features) fn activate_next_duplicate(&mut self) -> Option<String> {
        if self.active_duplicate_prompt.is_some() || !self.duplicate_prompts.has_pending() {
            return None;
        }
        let request = self.duplicate_prompts.pop_pending()?;
        let target = request.request.target_path.clone();
        self.active_duplicate_prompt = Some(SftpDuplicatePromptState {
            id: request.id,
            request: request.request,
            response_tx: request.response_tx,
            upload_batch: request.upload_batch,
        });
        Some(target)
    }
}

impl SessionFeatureState {
    pub(in crate::features) fn prompt_duplicate_broker(&self) -> Arc<SftpDuplicatePromptBroker> {
        self.prompts.duplicate_broker()
    }
    pub(in crate::features) fn prompt_otp_provider(&self) -> Arc<NativeOtpProvider> {
        self.prompts.otp_provider()
    }
    pub(in crate::features) fn prompt_credential_focus(&self) -> &FocusHandle {
        self.prompts.credential_focus()
    }
    pub(in crate::features) fn prompt_credential_focus_is_pending(&self) -> bool {
        self.prompts.credential_focus_is_pending()
    }
    pub(in crate::features) fn prompt_finish_credential_focus(&mut self) {
        self.prompts.finish_credential_focus();
    }
    pub(in crate::features) fn prompt_active_duplicate(&self) -> Option<&SftpDuplicatePromptState> {
        self.prompts.active_duplicate()
    }
    pub(in crate::features) fn prompt_active_host_key(&self) -> Option<&HostKeyPromptRequest> {
        self.prompts.active_host_key()
    }
    pub(in crate::features) fn prompt_active_credential(&self) -> Option<&CredentialPromptState> {
        self.prompts.active_credential()
    }
    pub(in crate::features) fn prompt_restore_credential_focus(&mut self) {
        if self.prompts.has_active_credential() {
            self.prompts.credential_prompt_focus_pending = true;
        }
    }
    #[cfg(test)]
    pub(in crate::features) fn prompt_request_credential_focus_for_test(&mut self) {
        self.prompts.credential_prompt_focus_pending = true;
    }
    pub(in crate::features) fn prompt_totp_clock_is_armed(&self) -> bool {
        self.prompts.totp_clock_armed
    }
    pub(in crate::features) fn set_prompt_totp_clock_armed(&mut self, armed: bool) {
        self.prompts.totp_clock_armed = armed;
    }
    pub(in crate::features) fn prompt_keyboard_totp_seconds_to_next_step(
        &self,
        now: u64,
    ) -> Option<u64> {
        self.prompts.keyboard_totp_seconds_to_next_step(now)
    }
    pub(in crate::features) fn prompt_active_keyboard_interactive(
        &self,
    ) -> Option<&KeyboardInteractivePromptState> {
        self.prompts.active_keyboard_interactive()
    }
    pub(in crate::features) fn prompt_active_agent(&self) -> Option<&AgentPromptRequest> {
        self.prompts.active_agent()
    }
    /// 消费 Agent 请求的后台状态变更通知。
    pub(in crate::features) fn prompt_take_agent_changed(&self) -> bool {
        self.prompts.take_agent_changed()
    }
    /// 清理 transport 已完成的 Agent 请求，并释放当前 UI 槽位。
    pub(in crate::features) fn prompt_reconcile_agent(&mut self) -> bool {
        self.prompts.reconcile_agent()
    }
    pub(in crate::features) fn prompt_has_active_credential(&self) -> bool {
        self.prompts.has_active_credential()
    }
    pub(in crate::features) fn prompt_has_active_keyboard_interactive(&self) -> bool {
        self.prompts.has_active_keyboard_interactive()
    }
    pub(in crate::features) fn prompt_has_active_ssh_auth(&self) -> bool {
        self.prompts.has_active_ssh_auth()
    }
    pub(in crate::features) fn prompt_focus_keyboard_interactive_response(
        &mut self,
        prompt_id: &str,
        index: usize,
    ) -> bool {
        self.prompts
            .focus_keyboard_interactive_response(prompt_id, index)
    }
}

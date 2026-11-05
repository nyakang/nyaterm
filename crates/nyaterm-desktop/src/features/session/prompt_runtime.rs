use super::state::PromptResolution;
use super::{
    CredentialPromptRequest, CredentialPromptState, DuplicatePromptAction, HostKeyPromptChoice,
    KeyboardInteractivePromptState, unix_seconds_now,
};
use crate::features::{
    NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup,
    transfers::duplicate_decision_label,
};
use futures::StreamExt as _;
use gpui::{Context, KeyDownEvent, Window};
use nyaterm_files::sftp_transfer_types::{SftpDuplicateDecision, SftpDuplicateRequest};
use nyaterm_transport::{
    SshAgentPromptAction, SshCredentialPrompt, SshHostKey, SshKeyboardInteractiveRequest,
};
use std::hash::{Hash, Hasher};
use std::time::Duration;

#[derive(Clone, Copy)]
enum KeyboardInteractiveOtpPreview {
    Initial,
    Generate,
    Refresh,
}

impl NyaTermApp {
    pub(in crate::features) fn resolve_agent_prompt(
        &mut self,
        request_id: String,
        action: SshAgentPromptAction,
        cx: &mut Context<Self>,
    ) {
        let request = match self.session.prompts.take_agent_resolution(&request_id) {
            PromptResolution::Inactive => {
                self.shell
                    .set_status("no SSH Agent prompt is active".to_string());
                cx.notify();
                return;
            }
            PromptResolution::Changed => {
                self.shell
                    .set_status("SSH Agent prompt changed before response".to_string());
                cx.notify();
                return;
            }
            PromptResolution::Ready(request) => request,
        };
        let target = request.target();
        let _ = request.response_tx.send(action);
        self.shell.set_status(match action {
            SshAgentPromptAction::Retry => {
                format!("retrying SSH Agent authentication for {target}")
            }
            SshAgentPromptAction::Cancel => {
                format!("cancelled SSH Agent authentication for {target}")
            }
        });
        cx.notify();
    }

    pub(in crate::features) fn resolve_host_key_prompt(
        &mut self,
        request_id: String,
        choice: HostKeyPromptChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = match self.session.prompts.take_host_key_resolution(&request_id) {
            PromptResolution::Inactive => {
                self.shell
                    .set_status("no SSH host key prompt is active".to_string());
                cx.notify();
                return;
            }
            PromptResolution::Changed => {
                self.shell
                    .set_status("SSH host key prompt changed before response".to_string());
                cx.notify();
                return;
            }
            PromptResolution::Ready(request) => request,
        };

        let host = request.host_key.host_identifier.clone();
        let _ = request.response_tx.send(choice);
        self.shell.set_status(match choice {
            HostKeyPromptChoice::Accept => format!("accepted SSH host key for {host}"),
            HostKeyPromptChoice::Reject => format!("rejected SSH host key for {host}"),
        });
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn resolve_duplicate_prompt(
        &mut self,
        request_id: String,
        decision: SftpDuplicateDecision,
        cx: &mut Context<Self>,
    ) {
        self.resolve_duplicate_prompt_action(
            request_id,
            DuplicatePromptAction::Current(decision),
            cx,
        );
    }

    pub(in crate::features) fn resolve_duplicate_prompt_action(
        &mut self,
        request_id: String,
        action: DuplicatePromptAction,
        cx: &mut Context<Self>,
    ) {
        let prompt = match self.session.prompts.take_duplicate_resolution(&request_id) {
            PromptResolution::Inactive => {
                self.shell
                    .set_status("no remote transfer duplicate prompt is active".to_string());
                cx.notify();
                return;
            }
            PromptResolution::Changed => {
                self.shell.set_status(
                    "remote transfer duplicate prompt changed before response".to_string(),
                );
                cx.notify();
                return;
            }
            PromptResolution::Ready(prompt) => prompt,
        };

        let target = prompt.request.target_path.clone();
        let decision = match self
            .session
            .prompt_duplicate_broker()
            .resolve(prompt, action)
        {
            Ok(decision) => decision,
            Err(error) => {
                self.shell.set_status(error);
                cx.notify();
                return;
            }
        };
        self.shell.set_status(format!(
            "remote transfer duplicate decision for {target}: {}",
            duplicate_decision_label(decision)
        ));
        cx.notify();
    }

    pub(in crate::features) fn submit_credential_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .session
            .prompts
            .active_credential()
            .is_some_and(|s| s.authentication.is_some())
        {
            self.submit_runtime_authentication(window, cx);
            return;
        }
        let Some(state) = self.session.prompts.take_credential() else {
            return;
        };
        let host = credential_prompt_target(&state.prompt);
        let _ = state.response_tx.send(Some(state.value));
        self.forget_text_inputs("ssh.credential.");
        self.shell
            .set_status(format!("submitted SSH credential for {host}"));
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn cancel_credential_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.prompts.take_credential() else {
            return;
        };
        let host = credential_prompt_target(&state.prompt);
        let _ = state.response_tx.send(None);
        self.forget_text_inputs("ssh.credential.");
        self.shell
            .set_status(format!("cancelled SSH credential prompt for {host}"));
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn submit_keyboard_interactive_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.prompts.take_keyboard_interactive() else {
            return;
        };
        let target = keyboard_interactive_prompt_target(&state.request);
        let _ = state.response_tx.send(Some(state.responses));
        self.forget_text_inputs("ssh.keyboard-interactive.");
        self.shell
            .set_status(format!("submitted SSH verification for {target}"));
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn cancel_keyboard_interactive_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.prompts.take_keyboard_interactive() else {
            return;
        };
        let target = keyboard_interactive_prompt_target(&state.request);
        let _ = state.response_tx.send(None);
        self.forget_text_inputs("ssh.keyboard-interactive.");
        self.shell
            .set_status(format!("cancelled SSH verification for {target}"));
        self.focus_active_workspace_surface(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn generate_keyboard_interactive_otp_code(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(otp_id) = self.session.prompts.keyboard_interactive_otp_id() else {
            return;
        };
        self.request_keyboard_interactive_otp_preview(
            otp_id,
            KeyboardInteractiveOtpPreview::Generate,
            cx,
        );
    }

    /// Store reads behind an OTP preview wait on the store worker, so they run on the
    /// blocking pool; the result applies only while the same prompt is still active.
    fn request_keyboard_interactive_otp_preview(
        &mut self,
        otp_id: String,
        purpose: KeyboardInteractiveOtpPreview,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt_id) = self
            .session
            .prompt_active_keyboard_interactive()
            .map(|state| state.id.clone())
        else {
            return;
        };
        let provider = self.session.prompts.otp_provider();
        let task = self
            .blocking_jobs
            .submit_task("ssh-prompt-otp-preview", move |_| {
                provider.preview_otp_code(&otp_id)
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this
                    .session
                    .prompt_active_keyboard_interactive()
                    .is_some_and(|state| state.id == prompt_id)
                {
                    return;
                }
                let result = match (purpose, result) {
                    (KeyboardInteractiveOtpPreview::Initial, Ok(Some(preview))) => {
                        Ok(Some(preview))
                    }
                    // An initial preview is best effort; the user can still generate one.
                    (KeyboardInteractiveOtpPreview::Initial, _) => return,
                    (_, result) => result,
                };
                let ready = this.session.prompts.apply_keyboard_interactive_otp_result(
                    result,
                    matches!(purpose, KeyboardInteractiveOtpPreview::Refresh),
                );
                if ready && matches!(purpose, KeyboardInteractiveOtpPreview::Generate) {
                    this.shell.set_status("OTP code ready".to_string());
                }
                this.ensure_keyboard_interactive_totp_clock(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn send_keyboard_interactive_otp_to_input(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some((prompt_id, response)) = self
            .session
            .prompts
            .send_keyboard_interactive_otp_to_response()
        else {
            return;
        };
        let input_id = keyboard_interactive_text_input_id(&prompt_id, 0);
        self.reset_text_input(&input_id, &response, cx);
        self.shell
            .set_status("OTP code sent to verification input".to_string());
        cx.notify();
    }

    pub(in crate::features) fn copy_keyboard_interactive_otp_code(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(code) = self.session.prompts.keyboard_interactive_otp_code() else {
            self.copy_text_with_feedback("copy-auth-otp", String::new(), cx);
            return;
        };
        self.copy_text_with_feedback("copy-auth-otp", code, cx);
        self.shell.set_status("OTP code copied".to_string());
        cx.notify();
    }

    pub(in crate::features) fn handle_credential_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        if !self.session.prompts.has_active_credential() {
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        if self.select_menu_with_prefix_is_focused("ssh-auth-", window, cx) {
            return false;
        }
        match keystroke.key.as_str() {
            "enter" => {
                if self.select_with_prefix_is_focused("ssh-auth-", window, cx) {
                    return false;
                }
                self.submit_credential_prompt(window, cx);
            }
            "escape" => {
                self.cancel_credential_prompt(window, cx);
            }
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn handle_keyboard_interactive_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        if !self.session.prompts.has_active_keyboard_interactive() {
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        match keystroke.key.as_str() {
            "enter" => self.submit_keyboard_interactive_prompt(window, cx),
            "escape" => self.cancel_keyboard_interactive_prompt(window, cx),
            "tab" => {
                let Some(target) = self
                    .session
                    .prompts
                    .advance_keyboard_interactive_focus(keystroke.modifiers.shift)
                else {
                    return false;
                };
                let setup = prompt_input_setup(target.kind, target.echo);
                let field = self.text_input(target.id, &target.seed, setup, cx);
                window.focus(&field.read(cx).focus_handle(), cx);
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn apply_ssh_credential_input(
        &mut self,
        prompt_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.session.prompts.apply_credential_input(prompt_id, text) {
            return;
        }
        self.mark_user_activity();
        cx.notify();
    }

    pub(in crate::features) fn apply_keyboard_interactive_input(
        &mut self,
        field_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some((prompt_id, index)) = parse_keyboard_interactive_text_input_id(field_id) else {
            return;
        };
        if !self
            .session
            .prompts
            .apply_keyboard_interactive_input(prompt_id, index, text)
        {
            return;
        }
        self.mark_user_activity();
        cx.notify();
    }

    pub(in crate::features) fn focus_active_ssh_prompt_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.session.prompts.active_input_target() else {
            let Some(form) = self
                .session
                .prompts
                .active_credential()
                .and_then(|state| state.authentication.as_ref())
            else {
                return false;
            };
            return self.focus_select(form.selection_focus_id(), window, cx);
        };
        let setup = prompt_input_setup(target.kind, target.echo);
        let field = self.text_input(target.id, &target.seed, setup, cx);
        window.focus(&field.read(cx).focus_handle(), cx);
        true
    }

    /// Promote queued auth and transfer prompts into the single active slot.
    ///
    /// Started once at window open. Before this the runtime tick ran the four
    /// activation steps on every control-plane pass.
    ///
    /// Two things wake this, and both are needed. A transport thread enqueuing a
    /// prompt is the obvious one. The other is the user *answering* the active
    /// prompt: that frees the slot the next one needs, and nothing is enqueued at
    /// that moment, so every `SessionPromptState::take_*` signals as it clears the
    /// slot. Without that the second prompt of a pair would never appear.
    pub(in crate::features) fn start_prompt_activation_drain(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut wake_rx) = self.session.prompts.take_wake_receiver() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                // Arm before checking, so a prompt enqueued in between still
                // signals rather than waiting for the next one.
                let Some(app) = this.upgrade() else {
                    break;
                };
                let updated = cx.update(|window, cx| {
                    app.update(cx, |this, cx| {
                        this.session.prompts.arm_wake();
                        let dirty = this.session.prompts.clear_cancelled_attempt_prompts()
                            | this.drain_host_key_prompts()
                            | this.drain_agent_prompts()
                            | this.drain_credential_prompts(cx);
                        let duplicate_activated = this.drain_duplicate_prompts();
                        if duplicate_activated
                            && let Some(prompt) = this.session.prompt_active_duplicate().cloned()
                        {
                            // 冲突会阻塞传输线程，必须在收到唤醒时直接打开全局对话框。
                            // 窗口任务从创建时就持有 Window 上下文，不依赖首次绘制后
                            // 才建立的 Entity-to-Window 映射。
                            this.open_duplicate_prompt_dialog(prompt, window, cx);
                        }
                        let dirty = dirty | duplicate_activated;
                        if dirty {
                            cx.notify();
                        }
                        // A keyboard-interactive TOTP prompt can only reach the screen
                        // through an activation, so this is where its clock starts.
                        this.ensure_keyboard_interactive_totp_clock(cx);
                        dirty
                    });
                });
                // No `continue` on success: only one prompt occupies the slot at
                // a time, so a queue with several waiting needs another pass once
                // this one resolves -- and that pass is driven by the `take_*`
                // signal, not by looping here.
                if updated.is_err() {
                    break;
                }
                if wake_rx.next().await.is_none() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn drain_host_key_prompts(&mut self) -> bool {
        let Some(host) = self.session.prompts.activate_next_host_key() else {
            return false;
        };
        self.shell
            .set_status(format!("SSH host key decision required for {host}"));
        true
    }

    pub(in crate::features) fn drain_agent_prompts(&mut self) -> bool {
        let changed = self.session.prompt_take_agent_changed();
        let reconciled = self.session.prompt_reconcile_agent();
        let Some(target) = self.session.prompts.activate_next_agent() else {
            return changed || reconciled;
        };
        self.shell
            .set_status(format!("SSH Agent approval required for {target}"));
        true
    }

    pub(in crate::features) fn drain_credential_prompts(&mut self, cx: &mut Context<Self>) -> bool {
        let mut completed = false;
        while let Some(CredentialPromptRequest::Authenticated { id, saves, attempt }) =
            self.session.prompts.credential_broker().pop_completed()
        {
            self.save_authenticated_credentials(id, saves, attempt, cx);
            completed = true;
        }
        if let Some(request) = self.session.prompts.take_next_credential_request() {
            match request {
                CredentialPromptRequest::Authentication {
                    id,
                    request,
                    context,
                    response_tx,
                } => {
                    self.forget_text_inputs("ssh.credential.");
                    self.forget_text_inputs("ssh.auth.");
                    self.session
                        .prompts
                        .activate_credential(CredentialPromptState {
                            id,
                            prompt: request.prompt.clone(),
                            response_tx:
                                super::runtime_auth::CredentialResponseSender::Authentication(
                                    response_tx,
                                ),
                            authentication: Some(
                                super::runtime_auth::RuntimeAuthenticationForm::new(
                                    request, context,
                                ),
                            ),
                            value: String::new(),
                        });
                    self.refresh_security_catalog(cx);
                }
                CredentialPromptRequest::Authenticated { id, saves, attempt } => {
                    self.save_authenticated_credentials(id, saves, attempt, cx);
                    // Completion events do not occupy a prompt slot.
                    self.session.prompts.signal_wake();
                }
                CredentialPromptRequest::Secret {
                    id,
                    prompt,
                    response_tx,
                } => {
                    self.forget_text_inputs("ssh.credential.");
                    self.shell.set_status(format!(
                        "SSH credential required for {}",
                        credential_prompt_target(&prompt)
                    ));
                    self.session
                        .prompts
                        .activate_credential(CredentialPromptState {
                            id,
                            prompt,
                            response_tx: response_tx.into(),
                            authentication: None,
                            value: String::new(),
                        });
                }
                CredentialPromptRequest::KeyboardInteractive {
                    id,
                    request,
                    response_tx,
                } => {
                    self.forget_text_inputs("ssh.keyboard-interactive.");
                    self.shell.set_status(format!(
                        "SSH verification required for {}",
                        keyboard_interactive_prompt_target(&request)
                    ));
                    let responses = vec![String::new(); request.prompts.len()];
                    let otp_type = request.otp_id.as_deref().and_then(|otp_id| {
                        self.security
                            .otp_entries()
                            .iter()
                            .find(|entry| entry.id == otp_id)
                            .map(|entry| entry.otp_type.to_ascii_lowercase())
                    });
                    let initial_preview_otp_id = request
                        .otp_id
                        .clone()
                        .filter(|_| otp_type.as_deref() == Some("totp"));
                    self.session.prompts.activate_keyboard_interactive(
                        KeyboardInteractivePromptState {
                            id,
                            request,
                            response_tx,
                            responses,
                            focused_index: 0,
                            otp_code: None,
                            otp_type,
                            otp_period: 0,
                            otp_time_step: None,
                            otp_error: None,
                        },
                    );
                    if let Some(otp_id) = initial_preview_otp_id {
                        self.request_keyboard_interactive_otp_preview(
                            otp_id,
                            KeyboardInteractiveOtpPreview::Initial,
                            cx,
                        );
                    }
                }
            }
            return true;
        }
        completed
    }

    /// Keep the shown TOTP code current while a keyboard-interactive prompt displays
    /// one.
    ///
    /// This was the last thing on the runtime tick's control plane. It is genuinely
    /// time-driven -- the code changes on its own step boundary -- but it only matters
    /// while such a prompt is on screen, which is a state with a definite beginning
    /// and end. Idempotent, and it retires itself when the prompt goes away.
    pub(in crate::features) fn ensure_keyboard_interactive_totp_clock(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.session.prompt_totp_clock_is_armed()
            || self
                .session
                .prompt_keyboard_totp_seconds_to_next_step(unix_seconds_now())
                .is_none()
        {
            return;
        }
        self.session.set_prompt_totp_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(delay)) = this.update(cx, |this, _| {
                    this.session
                        .prompt_keyboard_totp_seconds_to_next_step(unix_seconds_now())
                        .map(Duration::from_secs)
                }) else {
                    let _ = this.update(cx, |this, _| {
                        this.session.set_prompt_totp_clock_armed(false);
                    });
                    break;
                };
                cx.background_executor().timer(delay).await;
                let Ok(still_showing) = this.update(cx, |this, cx| {
                    this.refresh_keyboard_interactive_totp(cx);
                    let showing = this
                        .session
                        .prompt_keyboard_totp_seconds_to_next_step(unix_seconds_now())
                        .is_some();
                    if !showing {
                        this.session.set_prompt_totp_clock_armed(false);
                    }
                    showing
                }) else {
                    break;
                };
                if !still_showing {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn refresh_keyboard_interactive_totp(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(otp_id) = self
            .session
            .prompts
            .keyboard_totp_refresh_otp_id(unix_seconds_now())
        else {
            return;
        };
        self.request_keyboard_interactive_otp_preview(
            otp_id,
            KeyboardInteractiveOtpPreview::Refresh,
            cx,
        );
    }

    pub(in crate::features) fn drain_duplicate_prompts(&mut self) -> bool {
        let Some(target) = self.session.prompts.activate_next_duplicate() else {
            return false;
        };
        self.shell.set_status(format!(
            "remote transfer duplicate decision required for {target}"
        ));
        true
    }
}

pub(in crate::features) fn uuid_like_prompt_id(host_key: &SshHostKey) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    host_key.host_identifier.hash(&mut hasher);
    host_key.key_type.hash(&mut hasher);
    host_key.key_base64.hash(&mut hasher);
    format!("hk-{:016x}", hasher.finish())
}

pub(in crate::features) fn credential_prompt_id(prompt: &SshCredentialPrompt) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    prompt.connection_name.hash(&mut hasher);
    prompt.host.hash(&mut hasher);
    prompt.port.hash(&mut hasher);
    prompt.username.hash(&mut hasher);
    prompt.kind.hash(&mut hasher);
    prompt.reason.hash(&mut hasher);
    prompt.attempt.hash(&mut hasher);
    prompt.prompt_text.hash(&mut hasher);
    prompt.echo.hash(&mut hasher);
    format!("cred-{:016x}", hasher.finish())
}

pub(in crate::features) fn credential_text_input_id(prompt_id: &str) -> String {
    format!("ssh.credential.{prompt_id}")
}

pub(in crate::features) fn keyboard_interactive_text_input_id(
    prompt_id: &str,
    index: usize,
) -> String {
    format!("ssh.keyboard-interactive.{prompt_id}.{index}")
}

fn parse_keyboard_interactive_text_input_id(field_id: &str) -> Option<(&str, usize)> {
    let (prompt_id, index) = field_id.rsplit_once('.')?;
    Some((prompt_id, index.parse().ok()?))
}

pub(in crate::features) fn keyboard_interactive_prompt_id(
    request: &SshKeyboardInteractiveRequest,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.hash(&mut hasher);
    format!("keyboard-interactive-{:016x}", hasher.finish())
}

pub(in crate::features) fn sftp_duplicate_prompt_id(request: &SftpDuplicateRequest) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.direction.hash(&mut hasher);
    request.source_path.hash(&mut hasher);
    request.target_path.hash(&mut hasher);
    request.is_directory.hash(&mut hasher);
    format!("sftp-dup-{:016x}", hasher.finish())
}

pub(in crate::features) fn credential_prompt_target(prompt: &SshCredentialPrompt) -> String {
    format!(
        "{}@{}:{} (attempt {})",
        prompt.username, prompt.host, prompt.port, prompt.attempt
    )
}

pub(in crate::features) fn keyboard_interactive_prompt_target(
    request: &SshKeyboardInteractiveRequest,
) -> String {
    if request.connection_name.trim().is_empty() {
        format!("{}@{}:{}", request.username, request.host, request.port)
    } else {
        request.connection_name.clone()
    }
}

/// Server echo controls initial masking; known OTP prompts retain their semantic type.
pub(in crate::features) fn prompt_input_setup(
    kind: nyaterm_ui::NyaInputKind,
    echo: bool,
) -> TextInputSetup {
    if kind == nyaterm_ui::NyaInputKind::OneTimeCode {
        TextInputSetup::one_time_code(!echo)
    } else {
        TextInputSetup {
            kind,
            masked: !echo,
            ..TextInputSetup::default()
        }
    }
}

#[cfg(test)]
mod text_input_id_tests {
    use super::{keyboard_interactive_text_input_id, parse_keyboard_interactive_text_input_id};

    #[test]
    fn keyboard_interactive_input_ids_keep_prompt_and_index_separate() {
        let id = keyboard_interactive_text_input_id("keyboard-interactive.abc", 3);

        assert_eq!(
            parse_keyboard_interactive_text_input_id(
                id.strip_prefix("ssh.keyboard-interactive.").unwrap()
            ),
            Some(("keyboard-interactive.abc", 3))
        );
    }

    #[test]
    fn keyboard_interactive_input_ids_reject_missing_or_invalid_indexes() {
        assert_eq!(parse_keyboard_interactive_text_input_id("prompt"), None);
        assert_eq!(
            parse_keyboard_interactive_text_input_id("prompt.not-a-number"),
            None
        );
    }
    #[test]
    fn authentication_input_semantics_preserve_server_echo_and_known_otp() {
        use super::prompt_input_setup;
        use nyaterm_ui::NyaInputKind;
        let password = prompt_input_setup(NyaInputKind::Password, false);
        assert!(password.masked);
        assert_eq!(password.kind, NyaInputKind::Password);
        let echoed = prompt_input_setup(NyaInputKind::Secret, true);
        assert!(!echoed.masked);
        assert_eq!(echoed.kind, NyaInputKind::Secret);
        let echoed_password = prompt_input_setup(NyaInputKind::Password, true);
        assert!(!echoed_password.masked);
        assert_eq!(echoed_password.kind, NyaInputKind::Password);
        let otp = prompt_input_setup(NyaInputKind::OneTimeCode, false);
        assert!(otp.masked);
        assert_eq!(otp.kind, NyaInputKind::OneTimeCode);
        assert!(!prompt_input_setup(NyaInputKind::OneTimeCode, true).masked);
    }
}

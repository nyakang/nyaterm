use std::hash::{Hash, Hasher};

use futures::StreamExt as _;
use gpui::{ClipboardItem, Context, KeyDownEvent, Window};
use nyaterm_transport::{
    SftpDuplicateDecision, SftpDuplicateRequest, SshAgentPromptAction, SshCredentialPrompt,
    SshHostKey, SshKeyboardInteractiveRequest,
};

use super::state::PromptResolution;
use super::{
    CredentialPromptRequest, CredentialPromptState, HostKeyPromptChoice,
    KeyboardInteractivePromptState, unix_seconds_now,
};
use crate::features::{
    NyaTermApp, text_inputs::TextInputSetup, transfers::duplicate_decision_label,
};

impl NyaTermApp {
    pub(in crate::features) fn resolve_agent_prompt(
        &mut self,
        action: SshAgentPromptAction,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.session.prompts.take_agent() else {
            return;
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
        cx.notify();
    }

    pub(in crate::features) fn resolve_duplicate_prompt(
        &mut self,
        request_id: String,
        decision: SftpDuplicateDecision,
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
        let _ = prompt.response_tx.send(decision);
        self.shell.set_status(format!(
            "remote transfer duplicate decision for {target}: {}",
            duplicate_decision_label(decision)
        ));
        cx.notify();
    }

    pub(in crate::features) fn submit_credential_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.session.prompts.take_credential() else {
            return;
        };
        let host = credential_prompt_target(&state.prompt);
        let _ = state.response_tx.send(Some(state.value));
        self.forget_text_inputs("ssh.credential.");
        self.shell
            .set_status(format!("submitted SSH credential for {host}"));
        cx.notify();
    }

    pub(in crate::features) fn cancel_credential_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.session.prompts.take_credential() else {
            return;
        };
        let host = credential_prompt_target(&state.prompt);
        let _ = state.response_tx.send(None);
        self.forget_text_inputs("ssh.credential.");
        self.shell
            .set_status(format!("cancelled SSH credential prompt for {host}"));
        cx.notify();
    }

    pub(in crate::features) fn submit_keyboard_interactive_prompt(
        &mut self,
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
        cx.notify();
    }

    pub(in crate::features) fn cancel_keyboard_interactive_prompt(
        &mut self,
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
        cx.notify();
    }

    pub(in crate::features) fn generate_keyboard_interactive_otp_code(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(otp_id) = self.session.prompts.keyboard_interactive_otp_id() else {
            return;
        };
        let result = self
            .session
            .prompts
            .otp_provider()
            .preview_otp_code(&otp_id);
        if self
            .session
            .prompts
            .apply_keyboard_interactive_otp_result(result, false)
        {
            self.shell.set_status("OTP code ready".to_string());
        }
        cx.notify();
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
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.shell.set_status("OTP code copied".to_string());
        cx.notify();
    }

    pub(in crate::features) fn handle_credential_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        if !self.session.prompts.has_active_credential() {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }

        match keystroke.key.as_str() {
            "enter" => {
                self.submit_credential_prompt(cx);
            }
            "escape" => {
                self.cancel_credential_prompt(cx);
            }
            _ => {}
        }
    }

    pub(in crate::features) fn handle_keyboard_interactive_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        if !self.session.prompts.has_active_keyboard_interactive() {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }

        match keystroke.key.as_str() {
            "enter" => self.submit_keyboard_interactive_prompt(cx),
            "escape" => self.cancel_keyboard_interactive_prompt(cx),
            "tab" => {
                let Some(target) = self
                    .session
                    .prompts
                    .advance_keyboard_interactive_focus(keystroke.modifiers.shift)
                else {
                    return;
                };
                let setup = if target.echo {
                    TextInputSetup::default()
                } else {
                    TextInputSetup::masked()
                };
                let field = self.text_input(target.id, &target.seed, setup, cx);
                window.focus(&field.read(cx).focus_handle(), cx);
                cx.notify();
            }
            _ => {}
        }
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
            return false;
        };
        let setup = if target.echo {
            TextInputSetup::default()
        } else {
            TextInputSetup::masked()
        };
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
    pub(in crate::features) fn start_prompt_activation_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut wake_rx) = self.session.prompts.take_wake_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                // Arm before checking, so a prompt enqueued in between still
                // signals rather than waiting for the next one.
                let activated = this.update(cx, |this, cx| {
                    this.session.prompts.arm_wake();
                    let dirty = this.drain_host_key_prompts()
                        | this.drain_agent_prompts()
                        | this.drain_credential_prompts()
                        | this.drain_duplicate_prompts();
                    if dirty {
                        cx.notify();
                    }
                    dirty
                });
                // No `continue` on success: only one prompt occupies the slot at
                // a time, so a queue with several waiting needs another pass once
                // this one resolves -- and that pass is driven by the `take_*`
                // signal, not by looping here.
                if activated.is_err() {
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

    pub(in crate::features) fn drain_credential_prompts(&mut self) -> bool {
        if let Some(request) = self.session.prompts.take_next_credential_request() {
            match request {
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
                            response_tx,
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
                    let otp_preview = if otp_type.as_deref() == Some("totp") {
                        request
                            .otp_id
                            .as_deref()
                            .and_then(|otp_id| {
                                self.session
                                    .prompts
                                    .otp_provider()
                                    .preview_otp_code(otp_id)
                                    .ok()
                            })
                            .flatten()
                    } else {
                        None
                    };
                    let otp_code = otp_preview.as_ref().map(|preview| preview.code.clone());
                    let otp_period = otp_preview
                        .as_ref()
                        .map(|preview| preview.period)
                        .unwrap_or(0);
                    let otp_time_step = otp_preview.as_ref().and_then(|preview| preview.time_step);
                    self.session.prompts.activate_keyboard_interactive(
                        KeyboardInteractivePromptState {
                            id,
                            request,
                            response_tx,
                            responses,
                            focused_index: 0,
                            otp_code,
                            otp_type,
                            otp_period,
                            otp_time_step,
                            otp_error: None,
                        },
                    );
                }
            }
            return true;
        }
        false
    }

    pub(in crate::features) fn refresh_keyboard_interactive_totp(&mut self) -> bool {
        let Some(otp_id) = self
            .session
            .prompts
            .keyboard_totp_refresh_otp_id(unix_seconds_now())
        else {
            return false;
        };
        let result = self
            .session
            .prompts
            .otp_provider()
            .preview_otp_code(&otp_id);
        self.session
            .prompts
            .apply_keyboard_interactive_otp_result(result, true);
        true
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
}

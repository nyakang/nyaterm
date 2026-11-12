use rust_i18n::t;

use gpui::{Context, IntoElement as _, KeyDownEvent, PathPromptOptions, SharedString, Window};
use nyaterm_core::OtpEntry;
use nyaterm_ui::NyaDialogWindowExt as _;
use std::time::Duration;

use crate::features::{NyaTermApp, formatting::compact_id, runtime_jobs::await_blocking_job};
use crate::models::{SecurityAuthTab, SecurityOtpEditorState};
use nyaterm_transport::SessionKind;

use super::jobs::{SecurityStoreLocation, load_security_catalog};

#[derive(Clone, Copy)]
enum SecurityOtpCodeAction {
    Display,
    Copy,
    Send,
}

impl NyaTermApp {
    pub(in crate::features) fn import_security_otp_from_qr(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scanning_status = t!("otpManager.scanningQr").to_string();
        let Some(request_id) = self.security.begin_otp_qr_import(scanning_status) else {
            return;
        };
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(t!("otpManager.selectQrImage"))),
        };
        let receiver = cx.prompt_for_paths(options);
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let result = match selected {
                Some(path) => {
                    let task = scheduler.submit_task("otp-qr-decode", move |_| {
                        decode_security_otp_qr(&path).map(Some)
                    });
                    await_blocking_job(task).await.and_then(|result| result)
                }
                None => Ok(None),
            };
            let mut editor = None;
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_otp_qr_import(request_id) {
                    return;
                }
                match result {
                    Ok(Some(decoded)) => editor = Some(decoded),
                    Ok(None) => {
                        let status = t!("common.cancel").to_string();
                        this.security.set_status(status);
                    }
                    Err(error) => {
                        let status = format!("{}: {error}", t!("otpManager.qrImportFailed"));
                        this.security.set_status(status.clone());
                        this.shell.set_status(status);
                    }
                }
                cx.notify();
            });
            if let Some(editor) = editor {
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.open_security_otp_editor_with_draft(editor, window, cx);
                    });
                });
            }
        })
        .detach();
        cx.notify();
    }
}

fn decode_security_otp_qr(path: &std::path::Path) -> Result<SecurityOtpEditorState, String> {
    let reader =
        image::ImageReader::open(path).map_err(|error| format!("failed to open image: {error}"))?;
    let format = reader.format();
    let payload = nyaterm_preview::qr::decode_first_qr(reader.into_inner(), format)
        .map_err(|error| error.to_string())?;
    security_otp_from_uri(payload.as_str())
}

fn security_otp_from_uri(uri: &str) -> Result<SecurityOtpEditorState, String> {
    if uri.starts_with("otpauth://totp/") {
        let totp = nyaterm_otp::Totp::from_uri(uri)
            .map_err(|error| format!("invalid TOTP URI: {error}"))?;
        Ok(SecurityOtpEditorState {
            id: None,
            otp_type: "totp".to_string(),
            issuer: totp.issuer().to_string(),
            username: totp.label().to_string(),
            secret: totp.secret().into_base32().into(),
            algorithm: totp.alg().to_string(),
            digits: totp.digits().to_string(),
            period: totp.period().to_string(),
            counter: "0".to_string(),
            has_secret: false,
            error: None,
        })
    } else if uri.starts_with("otpauth://hotp/") {
        let hotp = nyaterm_otp::Hotp::from_uri(uri)
            .map_err(|error| format!("invalid HOTP URI: {error}"))?;
        Ok(SecurityOtpEditorState {
            id: None,
            otp_type: "hotp".to_string(),
            issuer: hotp.issuer().to_string(),
            username: hotp.label().to_string(),
            secret: hotp.secret().into_base32().into(),
            algorithm: hotp.alg().to_string(),
            digits: hotp.digits().to_string(),
            period: "30".to_string(),
            counter: hotp.counter().to_string(),
            has_secret: false,
            error: None,
        })
    } else {
        Err("QR image does not contain an otpauth URI".to_string())
    }
}

impl NyaTermApp {
    fn open_security_otp_editor_with_draft(
        &mut self,
        editor: SecurityOtpEditorState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("security.editor.otp-");
        self.security
            .open_otp_editor(editor, "OTP editor opened".to_string());
        window.focus(self.security.otp_editor_focus(), cx);
        self.open_security_otp_dialog(window, cx);
    }

    pub(in crate::features) fn open_security_otp_editor(
        &mut self,
        otp_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("security.editor.otp-");
        let editor = if let Some(otp_id) = otp_id {
            let Some(entry) = self
                .security
                .otp_entries()
                .iter()
                .find(|entry| entry.id == otp_id)
                .cloned()
            else {
                self.security.set_status("OTP entry is no longer available");
                cx.notify();
                return;
            };
            SecurityOtpEditorState {
                id: Some(entry.id),
                otp_type: entry.otp_type,
                issuer: entry.issuer,
                username: entry.username,
                secret: nyaterm_core::SecretString::default(),
                algorithm: entry.algorithm,
                digits: entry.digits.to_string(),
                period: entry.period.to_string(),
                counter: entry.counter.to_string(),
                has_secret: entry.has_secret,
                error: None,
            }
        } else {
            SecurityOtpEditorState {
                id: None,
                otp_type: "totp".to_string(),
                issuer: String::new(),
                username: String::new(),
                secret: nyaterm_core::SecretString::default(),
                algorithm: "SHA1".to_string(),
                digits: "6".to_string(),
                period: "30".to_string(),
                counter: "0".to_string(),
                has_secret: false,
                error: None,
            }
        };
        self.security
            .open_otp_editor(editor, "OTP editor opened".to_string());
        window.focus(self.security.otp_editor_focus(), cx);
        self.open_security_otp_dialog(window, cx);
        if let Some(otp_id) = self
            .security
            .otp_editor()
            .and_then(|editor| editor.id.clone())
        {
            self.load_security_otp_editor_secret(otp_id, cx);
        }
        cx.notify();
    }

    fn open_security_otp_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = if self
            .security
            .otp_editor()
            .is_some_and(|editor| editor.id.is_some())
        {
            t!("otpManager.editTitle")
        } else {
            t!("otpManager.newTitle")
        }
        .to_string();
        self.open_guarded_form_dialog(
            (
                title,
                560.,
                t!("common.save").to_string(),
                |app, _, cx| {
                    app.security
                        .otp_editor()
                        .cloned()
                        .map(|editor| app.security_otp_editor_view(editor, cx).into_any_element())
                        .unwrap_or_else(|| gpui::div().into_any_element())
                },
                |app, window, cx| {
                    app.save_security_otp_editor(window, cx);
                    app.security.otp_editor().is_none()
                },
                |app, cx| app.close_security_otp_editor(cx),
                |app| app.security.editor_busy(),
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn close_security_otp_editor(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        self.forget_text_inputs("security.editor.otp-");
        self.security.close_otp_editor();
        cx.notify();
    }

    pub(in crate::features) fn set_security_otp_type(
        &mut self,
        otp_type: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        if let Some(editor) = self.security.otp_editor_mut() {
            editor.otp_type = otp_type.to_string();
        }
        cx.notify();
    }

    pub(in crate::features) fn cycle_security_otp_algorithm(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        self.security.cycle_otp_algorithm();
        cx.notify();
    }

    pub(in crate::features) fn handle_security_otp_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }
        // The boxes own the text; the editor owns the keys that close or save
        // it, which the boxes leave unconsumed.
        match keystroke.key.as_str() {
            "escape" => {
                if !self.security.editor_busy() {
                    self.close_security_otp_editor(cx);
                }
            }
            "enter" => {
                self.save_security_otp_editor(window, cx);
            }
            _ => {}
        }
    }

    pub(in crate::features) fn save_security_otp_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.security.otp_editor().cloned() else {
            return;
        };
        if editor.id.is_none() && editor.secret.trim().is_empty() {
            if let Some(editor) = self.security.otp_editor_mut() {
                editor.error = Some("OTP secret is required".to_string());
            }
            cx.notify();
            return;
        }
        let digits = editor.digits.trim().parse::<u8>().unwrap_or(6).clamp(4, 10);
        let period = editor.period.trim().parse::<u64>().unwrap_or(30).max(1);
        let counter = editor.counter.trim().parse::<u64>().unwrap_or(0);

        let entry = OtpEntry {
            id: editor.id.clone().unwrap_or_default(),
            otp_type: if editor.otp_type == "hotp" {
                "hotp".to_string()
            } else {
                "totp".to_string()
            },
            issuer: editor.issuer.trim().to_string(),
            username: editor.username.trim().to_string(),
            secret: if editor.secret.trim().is_empty() {
                None
            } else {
                Some(editor.secret.trim().into())
            },
            algorithm: editor.algorithm.clone(),
            digits,
            period,
            counter,
            has_secret: false,
        };
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("otp-save", move |_| {
                let store = location.open()?;
                let id = store
                    .save_otp_entry(entry)
                    .map_err(|error| error.to_string())?;
                let catalog = load_security_catalog(&store)?;
                Ok::<_, String>((id, catalog))
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let mut close = false;
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                match result {
                    Ok((id, catalog)) => {
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        this.security
                            .finish_otp_editor(format!("OTP entry saved ({})", compact_id(&id)));
                        this.shell.set_status("OTP entry saved".to_string());
                        close = true;
                    }
                    Err(error) => {
                        if let Some(editor) = this.security.otp_editor_mut() {
                            editor.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
            if close {
                let _ = cx.update(|window, cx| window.close_nya_dialog(cx));
            }
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn request_delete_security_otp(
        &mut self,
        otp_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self
            .security
            .otp_entries()
            .iter()
            .find(|entry| entry.id == otp_id)
            .map(|entry| {
                if !entry.issuer.trim().is_empty() || !entry.username.trim().is_empty() {
                    format!(
                        "{}{}",
                        entry.issuer,
                        if entry.username.trim().is_empty() {
                            String::new()
                        } else if entry.issuer.trim().is_empty() {
                            entry.username.clone()
                        } else {
                            format!(" ({})", entry.username)
                        }
                    )
                } else {
                    compact_id(&entry.id)
                }
            })
            .unwrap_or_else(|| otp_id.clone());
        self.open_security_delete_dialog(SecurityAuthTab::Otp, otp_id, label, window, cx);
        cx.notify();
    }

    pub(in crate::features) fn generate_security_otp_code(
        &mut self,
        otp_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_security_otp_code(otp_id, SecurityOtpCodeAction::Display, window, cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_otp_code_visibility(
        &mut self,
        otp_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.security.toggle_otp_code_visible(otp_id.clone());
        if visible
            && self
                .security
                .otp_entries()
                .iter()
                .find(|entry| entry.id == otp_id)
                .is_some_and(|entry| entry.otp_type.eq_ignore_ascii_case("totp"))
        {
            self.request_security_otp_code(
                otp_id.clone(),
                SecurityOtpCodeAction::Display,
                window,
                cx,
            );
            self.arm_security_otp_refresh(cx);
        }
        cx.notify();
    }

    fn arm_security_otp_refresh(&mut self, cx: &mut Context<Self>) {
        if self.security.visible_totp_ids().is_empty() || !self.security.arm_otp_refresh() {
            return;
        }
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let work = this
                    .update(cx, |this, _| {
                        let ids = this.security.visible_totp_ids();
                        if ids.is_empty() {
                            this.security.disarm_otp_refresh();
                            return None;
                        }
                        Some((ids, this.session.prompt_otp_provider()))
                    })
                    .ok()
                    .flatten();
                let Some((ids, provider)) = work else {
                    break;
                };
                let task = scheduler.submit_task("otp-visible-refresh", move |_| {
                    ids.into_iter()
                        .filter_map(|otp_id| {
                            provider
                                .preview_otp_code(&otp_id)
                                .ok()
                                .flatten()
                                .map(|preview| (otp_id, preview.code))
                        })
                        .collect::<Vec<_>>()
                });
                let codes = await_blocking_job(task).await.unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    for (otp_id, code) in codes {
                        if this.security.otp_code_visible(&otp_id) {
                            this.security.reveal_otp_code(otp_id, code);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn request_security_otp_code(
        &mut self,
        otp_id: String,
        action: SecurityOtpCodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.security.clear_revealed_otp_code(&otp_id);
        let request_otp_id = otp_id.clone();
        let request_id = self.security.begin_otp_request(otp_id.clone());
        let provider = self.session.prompt_otp_provider();
        let scheduler = self.blocking_jobs.clone();
        let origin_window = Some(window.window_handle());
        cx.spawn_in(window, async move |this, cx| {
            let task =
                scheduler.submit_task("otp-preview", move |_| provider.preview_otp_code(&otp_id));
            let result = match await_blocking_job(task).await {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
            let mut send_code = None;
            let mut refresh_catalog = false;
            let _ = this.update(cx, |this, cx| {
                if !this
                    .security
                    .finish_otp_request(request_id, &request_otp_id)
                {
                    return;
                }
                match result {
                    Ok(Some(preview)) => {
                        let code = preview.code;
                        refresh_catalog = preview.otp_type.eq_ignore_ascii_case("hotp");
                        this.security
                            .reveal_otp_code(request_otp_id.clone(), code.clone());
                        match action {
                            SecurityOtpCodeAction::Display => {
                                this.security.set_status(format!(
                                    "OTP code ready for {}",
                                    compact_id(&request_otp_id)
                                ));
                                this.shell.set_status("OTP code ready".to_string());
                            }
                            SecurityOtpCodeAction::Copy => {
                                this.copy_text_with_feedback_at(
                                    origin_window,
                                    "copy-otp",
                                    code,
                                    cx,
                                );
                                this.security.set_status(format!(
                                    "OTP code copied ({})",
                                    compact_id(&request_otp_id)
                                ));
                                this.shell.set_status("OTP code copied".to_string());
                            }
                            SecurityOtpCodeAction::Send => send_code = Some(code),
                        }
                    }
                    Ok(None) => {
                        this.security.set_status("OTP entry not found");
                        if matches!(action, SecurityOtpCodeAction::Copy) {
                            this.notify_operation_at(
                                origin_window,
                                "copy-otp",
                                nyaterm_ui::notification::NyaNotificationKind::Error,
                                rust_i18n::t!("common.copyFailed").to_string(),
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        this.security.set_status(error);
                        if matches!(action, SecurityOtpCodeAction::Copy) {
                            this.notify_operation_at(
                                origin_window,
                                "copy-otp",
                                nyaterm_ui::notification::NyaNotificationKind::Error,
                                rust_i18n::t!("common.copyFailed").to_string(),
                                cx,
                            );
                        }
                    }
                }
                if refresh_catalog {
                    this.refresh_security_catalog(cx);
                }
                cx.notify();
            });
            if let Some(code) = send_code {
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        if this.send_sensitive_input_to_active_session(code.into_bytes(), cx) {
                            this.security
                                .set_status("OTP code sent to terminal".to_string());
                            this.focus_terminal_input(window, cx);
                        } else {
                            this.security
                                .set_status(t!("otpManager.sendToTerminalFailed").to_string());
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }

    pub(in crate::features) fn security_otp_can_send_to_terminal(&self) -> bool {
        let Some(session_id) = self.session.active_id() else {
            return false;
        };
        !self.session.is_disconnected(session_id)
            && self
                .session
                .session_info(session_id)
                .is_some_and(|session| !matches!(session.kind, SessionKind::Rdp | SessionKind::Vnc))
    }

    pub(in crate::features) fn send_security_otp_to_terminal(
        &mut self,
        otp_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.security_otp_can_send_to_terminal() {
            self.security
                .set_status(t!("otpManager.noActiveTerminal").to_string());
            cx.notify();
            return;
        }
        self.request_security_otp_code(otp_id, SecurityOtpCodeAction::Send, window, cx);
    }

    pub(in crate::features) fn copy_security_otp_code(
        &mut self,
        otp_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = self.security.revealed_otp_code(&otp_id).map(str::to_string)
            && code != "------"
            && !code.trim().is_empty()
        {
            self.copy_text_with_feedback("copy-otp", code, cx);
            self.security
                .set_status(format!("OTP code copied ({})", compact_id(&otp_id)));
            self.shell.set_status("OTP code copied".to_string());
            cx.notify();
            return;
        }
        self.request_security_otp_code(otp_id, SecurityOtpCodeAction::Copy, window, cx);
    }

    fn load_security_otp_editor_secret(&mut self, otp_id: String, cx: &mut Context<Self>) {
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("otp-editor-secret", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_otp_entry_by_id(&otp_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                let Some(editor) = this.security.otp_editor_mut() else {
                    return;
                };
                match result {
                    Ok(Some(entry)) => editor.secret = entry.secret.unwrap_or_default(),
                    Ok(None) => editor.error = Some("OTP entry not found".to_string()),
                    Err(error) => editor.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod qr_tests {
    use super::security_otp_from_uri;
    #[test]
    fn imported_totp_and_hotp_keep_editor_fields() {
        let totp = security_otp_from_uri("otpauth://totp/Fixture:user?secret=JBSWY3DPEHPK3PXP&issuer=Fixture&algorithm=SHA256&digits=8&period=45").unwrap();
        assert_eq!(
            (
                &*totp.otp_type,
                &*totp.issuer,
                &*totp.username,
                &*totp.algorithm,
                &*totp.digits,
                &*totp.period,
                &*totp.counter
            ),
            ("totp", "Fixture", "user", "SHA256", "8", "45", "0")
        );
        let hotp = security_otp_from_uri(
            "otpauth://hotp/Fixture:user?secret=JBSWY3DPEHPK3PXP&issuer=Fixture&counter=7",
        )
        .unwrap();
        assert_eq!(
            (&*hotp.otp_type, &*hotp.period, &*hotp.counter),
            ("hotp", "30", "7")
        );
        assert!(
            security_otp_from_uri("https://example.invalid")
                .err()
                .unwrap()
                .contains("does not contain an otpauth URI")
        );
        assert!(security_otp_from_uri("otpauth://totp/Fixture:user?secret=!").is_err());
    }
}

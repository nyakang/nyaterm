use rust_i18n::t;

use gpui::{AnyWindowHandle, Context, IntoElement as _, ParentElement as _, Window, div};
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};
use crate::models::{NavItem, SecurityAuthTab, SecurityUnlockAction, SettingsTab};

use super::jobs::{SecurityStoreLocation, load_security_catalog};

impl NyaTermApp {
    pub(in crate::features) fn security_secrets_locked(&self) -> bool {
        self.settings.summary().has_master_password && !self.security.secrets_unlocked()
    }

    pub(in crate::features) fn require_security_secrets_unlocked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        pending_action: Option<SecurityUnlockAction>,
    ) -> bool {
        if self.settings.summary().has_master_password && self.security.secrets_unlocked() {
            return true;
        }
        self.security.set_pending_unlock_action(pending_action);
        self.open_security_unlock_prompt(window, cx);
        false
    }

    pub(in crate::features) fn open_security_unlock_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.unlock_prompt_open() || self.security.master_required_prompt_open() {
            return;
        }
        let origin = window.window_handle();
        if !self.settings.summary().has_master_password {
            self.security.show_master_required_prompt();
            let generation = self.security.bind_unlock_window(origin);
            self.forget_text_inputs("security.unlock.password");
            let dialog = self.open_cancellable_form_dialog(
                (
                    t!("settings.masterPasswordRequired").to_string(),
                    384.,
                    t!("settings.security").to_string(),
                    |_, _, _| {
                        div()
                            .child(t!("settings.masterPasswordRequiredDesc"))
                            .into_any_element()
                    },
                    move |app, _, cx| {
                        if app.security.unlock_dialog_matches(generation, origin) {
                            app.open_security_settings_from_prompt(cx);
                        }
                        true
                    },
                    move |app, cx| {
                        if app.security.unlock_dialog_matches(generation, origin) {
                            app.close_security_master_required_prompt(cx);
                        }
                    },
                    |_| false,
                ),
                window,
                cx,
            );
            self.security.bind_unlock_dialog(dialog);
            cx.notify();
            return;
        }
        self.security.show_unlock_prompt();
        let generation = self.security.bind_unlock_window(origin);
        self.forget_text_inputs("security.unlock.password");
        let field = self.text_input("security.unlock.password", "", TextInputSetup::masked(), cx);
        let dialog = self.open_cancellable_form_dialog(
            (
                t!("secretUnlock.unlockTitle").to_string(),
                384.,
                t!("secretUnlock.unlock").to_string(),
                |app, _, cx| app.security_unlock_prompt(cx),
                move |app, window, cx| {
                    if app.security.unlock_dialog_matches(generation, origin) {
                        app.submit_security_unlock(window, cx);
                    }
                    false
                },
                move |app, cx| {
                    if app.security.unlock_dialog_matches(generation, origin) {
                        app.cancel_security_unlock_prompt(cx);
                    }
                },
                |app| app.security.unlock_busy(),
            ),
            window,
            cx,
        );
        self.security.bind_unlock_dialog(dialog);
        window.focus(&field.read(cx).focus_handle(), cx);
        cx.notify();
    }

    pub(in crate::features) fn cancel_security_unlock_prompt(&mut self, cx: &mut Context<Self>) {
        self.security.cancel_unlock_prompt();
        self.session.prompt_restore_credential_focus();
        self.forget_text_inputs("security.unlock.password");
        cx.notify();
    }

    pub(in crate::features) fn close_security_master_required_prompt(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.security.close_master_required_prompt();
        self.session.prompt_restore_credential_focus();
        cx.notify();
    }

    pub(in crate::features) fn open_security_settings_from_prompt(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.security.close_master_required_prompt();
        self.shell.set_settings_active_tab(SettingsTab::Security);
        self.open_page(NavItem::Settings, cx);
    }

    pub(in crate::features) fn lock_security_secrets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_secret_inputs(cx);
        self.security.lock_secrets();
        self.forget_text_inputs("security.editor.");
        self.connection_state.clear_editor_secrets(cx);
        self.forget_text_inputs("security.unlock.password");
        window.close_all_nya_dialogs(cx);
        cx.notify();
    }

    pub(in crate::features) fn submit_security_unlock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = window.window_handle();
        let generation = self.security.unlock_dialog_generation();
        if !self.security.unlock_dialog_matches(generation, origin) {
            return;
        }
        let Some((request_id, password)) = self.security.begin_unlock_request() else {
            return;
        };
        if let Some(input) = self.existing_text_input("security.unlock.password") {
            input.update(cx, |input, cx| input.set_disabled(true, cx));
        }
        cx.notify();
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("security-unlock", move |_| {
                let store = location.open()?;
                store
                    .verify_master_password(password.expose_secret())
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_security_unlock(request_id, generation, origin, result, window, cx);
            });
        })
        .detach();
    }

    pub(in crate::features) fn finish_security_unlock(
        &mut self,
        request_id: u64,
        generation: u64,
        origin: AnyWindowHandle,
        result: Result<bool, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if origin != window.window_handle()
            || !self.security.unlock_dialog_matches(generation, origin)
        {
            return;
        }
        let Some(dialog) = self.security.unlock_dialog() else {
            return;
        };
        if !dialog.is_active() {
            self.cancel_security_unlock_prompt(cx);
            return;
        }
        if !dialog.is_top(window, cx) {
            // A background feature may have placed another modal above this one.
            // Keep verification busy until its own dialog can safely be closed.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = this.update_in(cx, |app, window, cx| {
                    app.finish_security_unlock(request_id, generation, origin, result, window, cx);
                });
            })
            .detach();
            return;
        }
        if !self.security.finish_unlock_request(request_id) {
            return;
        }
        match result {
            Ok(true) => {
                let pending_action = self.security.complete_unlock();
                self.forget_text_inputs("security.unlock.password");
                window.close_nya_dialog(cx);
                if let Some(action) = pending_action {
                    self.execute_security_unlock_action(action, window, cx);
                }
            }
            result => {
                let error = match result {
                    Ok(_) => t!("secretUnlock.wrongPassword").to_string(),
                    Err(error) => error,
                };
                self.security.reject_unlock(error, "unlock rejected");
                if let Some(input) = self.existing_text_input("security.unlock.password") {
                    input.update(cx, |input, cx| {
                        input.set_content_silent("", cx);
                        input.set_disabled(false, cx);
                        input.hide_secret(cx);
                    });
                    window.focus(&input.read(cx).focus_handle(), cx);
                }
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn apply_security_unlock_password_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.security.apply_unlock_input(text);
        cx.notify();
    }

    fn execute_security_unlock_action(
        &mut self,
        action: SecurityUnlockAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SecurityUnlockAction::RevealConnectionPassword {
                editor_generation,
                revision,
            } => {
                self.reveal_connection_password_after_unlock((editor_generation, revision), cx);
            }
            SecurityUnlockAction::SubmitSshAuthentication(id) => {
                if self
                    .session
                    .prompt_active_credential()
                    .is_some_and(|state| state.id == id)
                {
                    self.submit_runtime_authentication(window, cx);
                }
            }
            SecurityUnlockAction::SavePasswordEditor(token) => {
                if self.security.account_editor_token() == Some(token) {
                    self.save_security_password_editor(window, cx);
                }
            }
            SecurityUnlockAction::ViewPrivateKey(id) => {
                self.view_security_private_key(id, window, cx);
            }
            SecurityUnlockAction::CopyPublicKey(id) => {
                self.copy_security_public_key(id, window, cx);
            }

            SecurityUnlockAction::RevealPassword(id) => {
                self.reveal_security_password(id, window, cx);
            }
            SecurityUnlockAction::CopyPassword(id) => {
                self.copy_security_password(id, window, cx);
            }
            SecurityUnlockAction::DeletePassword(id) => {
                self.request_delete_security_password(id, window, cx);
            }
            SecurityUnlockAction::OpenCredentialEditor(id) => {
                self.open_security_credential_editor(id, window, cx);
            }
            SecurityUnlockAction::ToggleCredentialEnabled(id) => {
                self.toggle_security_credential_list_enabled(id, window, cx);
            }
            SecurityUnlockAction::RevealCredential(id) => {
                self.reveal_security_credential_password(id, window, cx);
            }
            SecurityUnlockAction::DeleteCredential(id) => {
                self.request_delete_security_credential(id, window, cx);
            }
        }
    }

    pub(in crate::features) fn set_security_auth_tab(
        &mut self,
        tab: SecurityAuthTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_secret_inputs(cx);
        self.security.cancel_unlock_prompt();
        self.security.close_master_required_prompt();
        self.forget_text_inputs("security.unlock.password");
        self.security.set_auth_tab(tab);
        window.close_all_nya_dialogs(cx);
        if tab == SecurityAuthTab::KnownHosts {
            self.refresh_security_known_hosts(cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn refresh_security_catalog(&mut self, cx: &mut Context<Self>) {
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("security-catalog-refresh", move |_| {
                let store = location.open()?;
                load_security_catalog(&store)
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(catalog) => this.security.replace_catalog_state(catalog),
                    Err(error) => this.security.set_status(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use gpui::{
        AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Render, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px, size,
    };
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_ui::{NyaDialogWindowExt as _, NyaInputShell, NyaInputState, nya_root};
    use std::{cell::Cell, path::Path, rc::Rc};

    struct Workspace {
        focus: FocusHandle,
        clicks: Rc<Cell<usize>>,
    }
    impl Render for Workspace {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div()
                .size_full()
                .track_focus(&self.focus)
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    clicks.set(clicks.get() + 1)
                })
        }
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
        has_master: bool,
    ) -> (
        Entity<NyaTermApp>,
        &'a mut VisualTestContext,
        Rc<Cell<usize>>,
    ) {
        let app = app_with_visible_local_session(cx, root, "unlock-fixture");
        app.update(cx, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.has_master_password = has_master;
            app.settings.replace_summary(summary);
            app.security.lock_secrets();
            app.sync_component_theme(cx);
        });
        let clicks = Rc::new(Cell::new(0));
        let captured = clicks.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace {
                focus: cx.focus_handle(),
                clicks: captured,
            });
            nya_root(workspace, window, cx)
        });
        (app, cx, clicks)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    fn original_form(
        app: &Entity<NyaTermApp>,
        cx: &mut VisualTestContext,
    ) -> Entity<NyaInputState> {
        cx.update(|window, cx| {
            let field = cx.new(|cx| NyaInputState::new(cx, "unsaved draft"));
            let input = field.clone();
            app.update(cx, |app, cx| {
                app.open_form_dialog(
                    (
                        "Original editor".into(),
                        440.,
                        "Save".into(),
                        move |_, _, _| {
                            NyaInputShell::new("original-draft", &input).into_any_element()
                        },
                        |_, _, _| false,
                        |_, _| {},
                    ),
                    window,
                    cx,
                )
            });
            window.focus(&field.read(cx).focus_handle(), cx);
            field
        })
    }

    #[gpui::test]
    fn security_unlock_is_window_modal_without_a_security_panel(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-unlock-window-modal");
        let (app, cx, clicks) = hosted(cx, root.path(), true);
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx))
        });
        draw(cx);
        let content = cx
            .debug_bounds("security-unlock-dialog-content")
            .expect("window dialog");
        assert!(content.center().x > px(400.));
        cx.simulate_click(point(px(5.), px(300.)), Default::default());
        draw(cx);
        assert_eq!(clicks.get(), 0, "the modal must block the workspace");
        assert!(app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
        cx.simulate_resize(size(px(240.), px(500.)));
        draw(cx);
        let content = cx.debug_bounds("security-unlock-dialog-content").unwrap();
        assert!(content.origin.x >= px(0.));
        assert!(content.origin.x + content.size.width <= px(240.));
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
        assert!(!app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
    }

    #[gpui::test]
    fn security_unlock_cancel_preserves_the_editor_and_ignores_late_success(
        cx: &mut TestAppContext,
    ) {
        let root = TestTempDir::new("nyaterm-unlock-cancel-editor");
        let (app, cx, _) = hosted(cx, root.path(), true);
        let original = original_form(&app, cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx))
        });
        draw(cx);
        let (request, generation, origin) = cx.update(|window, cx| {
            app.update(cx, |app, _| {
                app.security.apply_unlock_input("fixture password".into());
                (
                    app.security.begin_unlock_request().unwrap().0,
                    app.security.unlock_dialog_generation(),
                    window.window_handle(),
                )
            })
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx));
            app.update(cx, |app, cx| {
                app.finish_security_unlock(request, generation, origin, Ok(true), window, cx)
            });
            assert!(window.has_active_nya_dialog(cx));
            assert_eq!(original.read(cx).value(cx), "unsaved draft");
            assert!(
                original
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            );
            assert!(!app.read(cx).security.secrets_unlocked());
        });
    }

    #[gpui::test]
    fn security_unlock_failure_hides_and_focuses_input_then_success_closes_only_once(
        cx: &mut TestAppContext,
    ) {
        let root = TestTempDir::new("nyaterm-unlock-result-editor");
        let (app, cx, _) = hosted(cx, root.path(), true);
        let original = original_form(&app, cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx))
        });
        draw(cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let field = app.existing_text_input("security.unlock.password").unwrap();
                field.update(cx, |field, cx| {
                    field.set_content_silent("wrong", cx);
                    field.set_masked(false, cx);
                });
                app.security.apply_unlock_input("wrong".into());
            })
        });
        draw(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let field = app.existing_text_input("security.unlock.password").unwrap();
                assert_eq!(field.read(cx).value(cx), "wrong");
                let request = app.security.begin_unlock_request().unwrap().0;
                assert!(app.security.begin_unlock_request().is_none());
                let generation = app.security.unlock_dialog_generation();
                app.finish_security_unlock(
                    request,
                    generation,
                    window.window_handle(),
                    Ok(false),
                    window,
                    cx,
                );
                assert!(field.read(cx).is_masked());
                assert!(!app.security.unlock_busy());
                assert!(app.security.unlock_error().is_some());
            })
        });
        draw(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let field = app.existing_text_input("security.unlock.password").unwrap();
                assert_eq!(field.read(cx).value(cx), "");
                assert!(
                    app.security.unlock_error().is_some(),
                    "clearing the field must preserve the error through redraw"
                );
                assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
                let request = app.security.begin_unlock_request().unwrap().0;
                let generation = app.security.unlock_dialog_generation();
                let origin = window.window_handle();
                app.finish_security_unlock(request, generation, origin, Ok(true), window, cx);
                assert!(app.security.secrets_unlocked());
                assert!(!app.security.unlock_prompt_open());
                assert!(window.has_active_nya_dialog(cx), "original editor remains");
                app.finish_security_unlock(request, generation, origin, Ok(true), window, cx);
                assert!(
                    window.has_active_nya_dialog(cx),
                    "duplicate completion cannot close the editor"
                );
                assert_eq!(original.read(cx).value(cx), "unsaved draft");
            })
        });
    }

    #[gpui::test]
    fn security_unlock_master_required_and_application_lock_clean_up(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-unlock-master-required");
        let (app, cx, _) = hosted(cx, root.path(), false);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx))
        });
        draw(cx);
        cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
        assert!(app.read_with(cx, |app, _| app.security.master_required_prompt_open()));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!app.read_with(cx, |app, _| app.security.master_required_prompt_open()));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut summary = app.settings.summary().clone();
                summary.has_master_password = true;
                app.settings.replace_summary(summary);
                app.open_security_unlock_prompt(window, cx);
                app.security.apply_unlock_input("pending".into());
                let request = app.security.begin_unlock_request().unwrap().0;
                app.apply_shared_screen_lock(true, window, cx);
                assert!(!app.security.finish_unlock_request(request));
                assert!(!app.security.unlock_prompt_open());
                assert!(
                    app.existing_text_input("security.unlock.password")
                        .is_none(),
                    "lock drops the password input"
                );
                assert!(!window.has_active_nya_dialog(cx));
            })
        });
    }

    #[gpui::test]
    fn security_unlock_enter_submits_the_focused_password(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-unlock-enter");
        let (app, cx, _) = hosted(cx, root.path(), true);
        // A rejected job completes on the test executor; real worker wakeups
        // are deliberately rejected by GPUI's deterministic scheduler.
        app.update(cx, |app, _| app.blocking_jobs.shutdown());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx));
        });
        draw(cx);
        let key = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: key.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent { keystroke: key });
        assert!(
            app.read_with(cx, |app, _| {
                app.security.unlock_busy()
                    || app.security.secrets_unlocked()
                    || app.security.unlock_error().is_some()
            }),
            "Enter must reach verification even when the input has focus"
        );
    }

    #[gpui::test]
    fn security_unlock_programmatic_replacement_ignores_the_old_result(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-unlock-replaced");
        let (app, cx, _) = hosted(cx, root.path(), true);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            let (request, generation) = app.update(cx, |app, _| {
                (
                    app.security.begin_unlock_request().unwrap().0,
                    app.security.unlock_dialog_generation(),
                )
            });
            window.close_nya_dialog(cx);
            window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Replacement"));
            app.update(cx, |app, cx| {
                app.finish_security_unlock(
                    request,
                    generation,
                    window.window_handle(),
                    Ok(true),
                    window,
                    cx,
                );
                assert!(!app.security.unlock_prompt_open());
                assert!(!app.security.secrets_unlocked());
            });
            assert!(window.has_active_nya_dialog(cx));
        });
        draw(cx);
        assert!(!app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
    }

    #[gpui::test]
    fn security_unlock_waits_for_the_modal_above_it_and_cleans_up_external_close(
        cx: &mut TestAppContext,
    ) {
        let root = TestTempDir::new("nyaterm-unlock-covered");
        let (app, cx, _) = hosted(cx, root.path(), true);
        let original = original_form(&app, cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_security_unlock_prompt(window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Background notification"));
            app.update(cx, |app, cx| {
                let request = app.security.begin_unlock_request().unwrap().0;
                let generation = app.security.unlock_dialog_generation();
                app.finish_security_unlock(
                    request,
                    generation,
                    window.window_handle(),
                    Ok(true),
                    window,
                    cx,
                );
                assert!(app.security.unlock_busy());
                assert!(!app.security.secrets_unlocked());
            });
            window.close_nya_dialog(cx);
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        draw(cx);
        cx.update(|window, cx| {
            assert!(app.read(cx).security.secrets_unlocked());
            assert!(window.has_active_nya_dialog(cx), "original editor remains");
            assert_eq!(original.read(cx).value(cx), "unsaved draft");
            app.update(cx, |app, cx| {
                app.security.lock_secrets();
                app.open_security_unlock_prompt(window, cx);
            });
        });
        draw(cx);
        cx.update(|window, cx| window.close_nya_dialog(cx));
        draw(cx);
        assert!(!app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
    }

    #[gpui::test]
    fn security_unlock_idle_lock_hides_drafts_and_invalidates_requests(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-unlock-idle-lock");
        let (app, cx, _) = hosted(cx, root.path(), true);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let secret = app.text_input(
                    "test.secret",
                    "unsaved",
                    crate::features::text_inputs::TextInputSetup::secret(),
                    cx,
                );
                secret.update(cx, |state, cx| state.set_masked(false, cx));
                app.open_security_unlock_prompt(window, cx);
                let request = app.security.begin_unlock_request().unwrap().0;
                app.lock_screen_for_idle(window, cx);
                assert!(secret.read(cx).is_masked());
                assert_eq!(secret.read(cx).value(cx), "unsaved");
                assert!(!app.security.finish_unlock_request(request));
                assert!(
                    app.existing_text_input("security.unlock.password")
                        .is_none()
                );
                assert!(!window.has_active_nya_dialog(cx));
            });
        });
    }
}

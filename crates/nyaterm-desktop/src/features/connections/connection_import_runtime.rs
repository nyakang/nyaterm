use rust_i18n::t;

use std::borrow::Cow;
use std::path::PathBuf;

use nyaterm_core::SecretString;
use nyaterm_core::session_import::mobaxterm::{
    MobaXtermCredentialDiagnosticKind, MobaXtermImportReport, prepare_mobaxterm_import,
};
use nyaterm_core::session_import::{
    SessionImportError, SessionImportNotice, SessionImportNoticeKind, SessionImportOptions,
    prepare_session_import_with_options, prepare_windterm_import_with_decisions,
    prepare_windterm_preview,
};
use nyaterm_core::{PreparedSessionImport, WindtermImportReport, WindtermPreview};

use gpui::{Context, IntoElement, PathPromptOptions, SharedString, Window};
use nyaterm_store::StoreDomain;
use nyaterm_ui::NyaDialogWindowExt as _;
use zeroize::Zeroize as _;

use crate::features::connections::state::import::{
    ConnectionImportReportView, MobaImportSlot, MobaXtermPasswordContext,
};
use crate::features::text_inputs::TextInputSetup;
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use crate::models::ConnectionImportSource;

#[derive(Default)]
struct ConnectionImportRequest {
    path: Option<PathBuf>,
    master_password: Option<SecretString>,
    mobaxterm_passwords: Option<PathBuf>,
    skip_mobaxterm_credentials: bool,
    auto_termius: bool,
}

enum ConnectionImportReport {
    Windterm(WindtermImportReport),
    Mobaxterm(MobaXtermImportReport),
}

enum ConnectionImportResult {
    Imported {
        count: usize,
        incomplete: usize,
        report: Option<ConnectionImportReport>,
        notices: Vec<SessionImportNotice>,
    },
    NeedsWindtermPassword(PathBuf),
    NeedsMobaxtermPassword(MobaXtermPasswordContext),
    /// WindTerm files parsed with OneKey accounts; the import waits for the user
    /// to confirm the inferred accounts before anything is written.
    NeedsWindtermAccounts {
        preview: Box<WindtermPreview>,
        path: PathBuf,
        master_password: Option<SecretString>,
    },
    Cancelled,
    Failed {
        source: ConnectionImportSource,
        auto_termius: bool,
        error: String,
    },
    Closed,
}

enum PreparedConnectionImport {
    Commit {
        prepared: PreparedSessionImport,
        report: Option<ConnectionImportReport>,
    },
    WindtermAccounts(WindtermPreview),
}

impl ConnectionImportSource {
    fn prompt_label(self) -> Cow<'static, str> {
        match self {
            Self::NyatermBackup => Cow::Borrowed("Import NyaTerm backup"),
            Self::Xshell => Cow::Borrowed("Import Xshell .xts sessions"),
            Self::MobaXterm => Cow::Borrowed("Import MobaXterm .mxtsessions or .ini"),
            Self::WindTerm => Cow::Borrowed("Import WindTerm .sessions file"),
            Self::SecureCrt => Cow::Borrowed("Import SecureCRT .xml sessions"),
            Self::FinalShell => Cow::Borrowed("Import FinalShell conn directory"),
            Self::Termius => Cow::Borrowed("Import Termius IndexedDB directory"),
            Self::SshConfig => t!("savedConnections.importSshConfigPrompt"),
            Self::Electerm => Cow::Borrowed("Import Electerm bookmarks JSON"),
            Self::NyatermJson => Cow::Borrowed("Import NyaTerm sessions JSON"),
        }
    }

    fn selecting_status(self) -> Cow<'static, str> {
        match self {
            Self::NyatermBackup => Cow::Borrowed("selecting NyaTerm backup"),
            Self::Xshell => Cow::Borrowed("selecting Xshell session import file"),
            Self::MobaXterm => Cow::Borrowed("selecting MobaXterm session import file"),
            Self::WindTerm => Cow::Borrowed("selecting WindTerm session import file"),
            Self::SecureCrt => Cow::Borrowed("selecting SecureCRT session XML"),
            Self::FinalShell => Cow::Borrowed("selecting FinalShell conn directory"),
            Self::Termius => Cow::Borrowed("selecting Termius IndexedDB directory"),
            Self::SshConfig => t!("savedConnections.importSshConfigSelecting"),
            Self::Electerm => Cow::Borrowed("selecting Electerm bookmarks JSON"),
            Self::NyatermJson => Cow::Borrowed("selecting NyaTerm session JSON"),
        }
    }

    fn uses_directory_picker(self) -> bool {
        matches!(self, Self::FinalShell | Self::Termius)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn open_connection_import_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 程序化关窗（`close_nya_dialog`）不会触发对话框的 `on_close`，取消
        // 路径可能只关掉了窗口却把导入阶段留在 `Selecting`。以「是否真有活动
        // 对话框」为准清掉这种残留，避免导入入口被永久拦住。
        if self.connection_state.import_path_prompt_active() && !window.has_active_nya_dialog(cx) {
            let _ = self.connection_state.cancel_mobaxterm_prompt();
        }
        if self.connection_state.import_path_prompt_active()
            || self.settings.config_path_prompt_active()
        {
            self.shell
                .set_status("connection import picker is already open".to_string());
            cx.notify();
            return;
        }

        self.shell
            .set_status("select a connection import source".to_string());
        self.open_content_dialog(
            t!("settings.importConfig").to_string(),
            480.,
            |app, _, cx| app.connection_import_dialog_content(cx).into_any_element(),
            |_, _| {},
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn select_connection_import_source(
        &mut self,
        source: ConnectionImportSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.import_path_prompt_active() {
            return;
        }
        window.close_nya_dialog(cx);
        if source == ConnectionImportSource::NyatermBackup {
            self.prompt_encrypted_portable_snapshot_import(window, cx);
            return;
        }
        if source == ConnectionImportSource::MobaXterm {
            self.open_mobaxterm_import_dialog(window, cx);
            return;
        }
        if source == ConnectionImportSource::Termius {
            self.start_connection_import(
                source,
                ConnectionImportRequest {
                    auto_termius: true,
                    ..ConnectionImportRequest::default()
                },
                window,
                cx,
            );
            return;
        }
        self.prompt_connection_session_import(source, window, cx);
    }

    pub(in crate::features) fn open_mobaxterm_import_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.import_path_prompt_active() {
            return;
        }
        self.connection_state
            .begin_import_path_prompt(ConnectionImportSource::MobaXterm);
        self.open_content_dialog(
            t!("savedConnections.mobaxtermImportTitle").to_string(),
            640.,
            |app, _, cx| app.mobaxterm_import_dialog_content(cx).into_any_element(),
            |app, cx| app.close_mobaxterm_import_dialog(cx),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn close_mobaxterm_import_dialog(&mut self, cx: &mut Context<Self>) {
        if self.connection_state.cancel_mobaxterm_prompt() {
            self.shell
                .set_status(t!("savedConnections.importCancelled").to_string());
        }
        cx.notify();
    }

    /// 取消按钮专用：`window.close_nya_dialog` 不会触发 `on_close`，所以先
    /// 复位阶段与槽位草稿再关窗，确保下次能重新打开且不残留旧文件。
    pub(in crate::features) fn cancel_mobaxterm_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_mobaxterm_import_dialog(cx);
        window.close_nya_dialog(cx);
    }

    pub(in crate::features) fn browse_mobaxterm_session_file(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Do not open a picker once the window is gone or the background import
        // has already started.
        if !self.connection_state.import_path_prompt_active() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("savedConnections.mobaxtermSessionFilePrompt").to_string(),
            )),
        });
        cx.spawn(async move |this, cx| {
            let picked = receiver.await;
            if let Ok(Ok(Some(paths))) = picked
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |app, cx| {
                    app.connection_state
                        .set_mobaxterm_file(MobaImportSlot::Session, path);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn browse_mobaxterm_passwords_file(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connection_state.import_path_prompt_active() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("savedConnections.mobaxtermPasswordsFilePrompt").to_string(),
            )),
        });
        cx.spawn(async move |this, cx| {
            let picked = receiver.await;
            if let Ok(Ok(Some(paths))) = picked
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |app, cx| {
                    app.connection_state
                        .set_mobaxterm_file(MobaImportSlot::Passwords, path);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn clear_mobaxterm_slot(
        &mut self,
        slot: MobaImportSlot,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.clear_mobaxterm_slot(slot) {
            cx.notify();
        }
    }

    pub(in crate::features) fn handle_mobaxterm_slot_drop(
        &mut self,
        slot: MobaImportSlot,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.apply_mobaxterm_drop(slot, &paths) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_mobaxterm_drop_hover(
        &mut self,
        slot: MobaImportSlot,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_mobaxterm_drop_hover(hovered.then_some(slot))
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn start_mobaxterm_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_path) = self
            .connection_state
            .mobaxterm_session_path()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let passwords_path = self
            .connection_state
            .mobaxterm_passwords_path()
            .map(std::path::Path::to_path_buf);
        self.start_connection_import(
            ConnectionImportSource::MobaXterm,
            ConnectionImportRequest {
                path: Some(session_path),
                mobaxterm_passwords: passwords_path,
                ..ConnectionImportRequest::default()
            },
            window,
            cx,
        );
        window.close_nya_dialog(cx);
    }

    fn prompt_connection_session_import(
        &mut self,
        source: ConnectionImportSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.import_path_prompt_active() {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: !source.uses_directory_picker(),
            directories: source.uses_directory_picker(),
            multiple: false,
            prompt: Some(SharedString::from(source.prompt_label().to_string())),
        });
        self.connection_state.begin_import_path_prompt(source);
        self.shell.set_status(source.selecting_status().to_string());
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let picked = receiver.await;
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    let result = match picked {
                        Ok(Ok(Some(paths))) => {
                            if let Some(path) = paths.into_iter().next() {
                                app.start_connection_import(
                                    source,
                                    ConnectionImportRequest {
                                        path: Some(path),
                                        ..ConnectionImportRequest::default()
                                    },
                                    window,
                                    cx,
                                );
                                return;
                            }
                            ConnectionImportResult::Cancelled
                        }
                        Ok(Ok(None)) => ConnectionImportResult::Cancelled,
                        Ok(Err(error)) => ConnectionImportResult::Failed {
                            source,
                            auto_termius: false,
                            error: error.to_string(),
                        },
                        Err(_) => ConnectionImportResult::Closed,
                    };
                    app.apply_connection_import_result(result, window, cx);
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn start_connection_import(
        &mut self,
        source: ConnectionImportSource,
        request: ConnectionImportRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ConnectionImportRequest {
            path,
            master_password,
            mobaxterm_passwords,
            skip_mobaxterm_credentials,
            auto_termius,
        } = request;
        self.connection_state.begin_session_import_job();
        self.shell
            .set_status(t!("savedConnections.importInProgress"));
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("connection-import", move |_| {
                let retained_master_password = if source == ConnectionImportSource::WindTerm {
                    master_password.clone()
                } else {
                    None
                };
                let prepared = if source == ConnectionImportSource::WindTerm {
                    prepare_windterm_source(path.as_deref(), master_password.as_deref())
                } else if source == ConnectionImportSource::MobaXterm {
                    path.as_deref()
                        .ok_or_else(|| {
                            SessionImportError::Config(
                                "connection import path was not selected".into(),
                            )
                        })
                        .and_then(|path| {
                            prepare_mobaxterm_import(
                                path,
                                SessionImportOptions {
                                    mobaxterm_passwords_path: mobaxterm_passwords.as_deref(),
                                    mobaxterm_master_password: master_password.as_deref(),
                                    mobaxterm_skip_encrypted_credentials:
                                        skip_mobaxterm_credentials,
                                    ..SessionImportOptions::default()
                                },
                            )
                        })
                        .map(|outcome| PreparedConnectionImport::Commit {
                            prepared: outcome.prepared,
                            report: Some(ConnectionImportReport::Mobaxterm(outcome.report)),
                        })
                } else {
                    prepare_connection_source(
                        source,
                        path.as_deref(),
                        master_password.as_deref(),
                        mobaxterm_passwords.as_deref(),
                    )
                    .map(|prepared| PreparedConnectionImport::Commit {
                        prepared,
                        report: None,
                    })
                };
                drop(master_password);
                match prepared {
                    Ok(PreparedConnectionImport::Commit { prepared, report }) => {
                        let incomplete = prepared.incomplete_credentials_count();
                        let notices = prepared.notices.clone();
                        match store.request_mutation_fn(StoreDomain::Connections, move |database| {
                            database.commit_session_import(prepared)
                        }) {
                            Ok(count) => ConnectionImportResult::Imported {
                                count,
                                incomplete,
                                report,
                                notices,
                            },
                            Err(error) => ConnectionImportResult::Failed {
                                source,
                                auto_termius,
                                error: error.to_string(),
                            },
                        }
                    }
                    Ok(PreparedConnectionImport::WindtermAccounts(preview)) => {
                        ConnectionImportResult::NeedsWindtermAccounts {
                            preview: Box::new(preview),
                            path: path.unwrap_or_default(),
                            master_password: retained_master_password,
                        }
                    }
                    Err(SessionImportError::WindTermMasterPasswordRequired) => match path {
                        Some(path) => ConnectionImportResult::NeedsWindtermPassword(path),
                        None => ConnectionImportResult::Failed {
                            source,
                            auto_termius,
                            error: "connection import path was not selected".into(),
                        },
                    },
                    Err(
                        error @ (SessionImportError::MobaXtermMasterPasswordRequired
                        | SessionImportError::MobaXtermCredentialDecryptionFailed),
                    ) => match path {
                        Some(path) => ConnectionImportResult::NeedsMobaxtermPassword(
                            MobaXtermPasswordContext {
                                path,
                                passwords_path: mobaxterm_passwords,
                                decryption_failed: matches!(
                                    error,
                                    SessionImportError::MobaXtermCredentialDecryptionFailed
                                ),
                            },
                        ),
                        None => ConnectionImportResult::Failed {
                            source,
                            auto_termius,
                            error: error.to_string(),
                        },
                    },
                    Err(error) => ConnectionImportResult::Failed {
                        source,
                        auto_termius,
                        error: error.to_string(),
                    },
                }
            });
            let result = match await_blocking_job(task).await {
                Ok(result) => result,
                Err(error) => ConnectionImportResult::Failed {
                    source,
                    auto_termius,
                    error,
                },
            };
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.apply_connection_import_result(result, window, cx)
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn submit_mobaxterm_password(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.resume_mobaxterm_import(false, window, cx)
    }

    pub(in crate::features) fn skip_mobaxterm_credentials(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resume_mobaxterm_import(true, window, cx);
        window.close_nya_dialog(cx);
    }

    fn resume_mobaxterm_import(
        &mut self,
        skip: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.connection_state.mobaxterm_password_context().is_none() {
            return true;
        }
        let password = if skip {
            None
        } else {
            let Some(field) = self.existing_text_input("mobaxterm-password.value") else {
                return false;
            };
            let password = SecretString::from(field.read(cx).value(cx));
            if password.is_empty() {
                self.shell
                    .set_status(t!("savedConnections.mobaxtermMasterPasswordRequired"));
                cx.notify();
                return false;
            }
            Some(password)
        };
        let Some(context) = self.connection_state.take_mobaxterm_password_context() else {
            return true;
        };
        self.clear_mobaxterm_password_input(cx);
        self.start_connection_import(
            ConnectionImportSource::MobaXterm,
            ConnectionImportRequest {
                path: Some(context.path),
                mobaxterm_passwords: context.passwords_path,
                master_password: password,
                skip_mobaxterm_credentials: skip,
                ..ConnectionImportRequest::default()
            },
            window,
            cx,
        );
        true
    }

    pub(in crate::features) fn clear_mobaxterm_password_input(&mut self, cx: &mut Context<Self>) {
        if let Some(field) = self.existing_text_input("mobaxterm-password.value") {
            field.update(cx, |field, cx| field.discard_content(cx));
        }
        self.forget_text_inputs("mobaxterm-password.");
    }

    pub(in crate::features) fn cancel_mobaxterm_password(&mut self, cx: &mut Context<Self>) {
        if self.connection_state.mobaxterm_password_context().is_none() {
            return;
        }
        self.clear_mobaxterm_password_input(cx);
        self.connection_state.finish_import_path_prompt();
        self.shell
            .set_status(t!("savedConnections.importCancelled"));
        cx.notify();
    }

    pub(in crate::features) fn submit_windterm_password(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.connection_state.awaiting_windterm_password() {
            return true;
        }
        let Some(field) = self.existing_text_input("windterm-password.value") else {
            return false;
        };
        let password = SecretString::from(field.read(cx).value(cx));
        if password.is_empty() {
            self.shell
                .set_status(t!("savedConnections.windtermMasterPasswordRequired"));
            cx.notify();
            return false;
        }
        let Some(path) = self.connection_state.take_windterm_password_path() else {
            return true;
        };
        field.update(cx, |field, cx| field.discard_content(cx));
        self.forget_text_inputs("windterm-password.");
        self.start_connection_import(
            ConnectionImportSource::WindTerm,
            ConnectionImportRequest {
                path: Some(path),
                master_password: Some(password),
                ..ConnectionImportRequest::default()
            },
            window,
            cx,
        );
        true
    }

    pub(in crate::features) fn cancel_windterm_password(&mut self, cx: &mut Context<Self>) {
        if !self.connection_state.awaiting_windterm_password() {
            return;
        }
        if let Some(field) = self.existing_text_input("windterm-password.value") {
            field.update(cx, |field, cx| field.discard_content(cx));
        }
        self.forget_text_inputs("windterm-password.");
        self.connection_state.finish_import_path_prompt();
        self.shell
            .set_status(t!("savedConnections.importCancelled"));
        cx.notify();
    }

    /// Second step of the WindTerm flow: create the accounts inferred from
    /// `onekeys.config` and commit sessions already linked to those account ids.
    pub(in crate::features) fn submit_windterm_accounts_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.connection_state.awaiting_windterm_accounts() {
            return true;
        }
        let Some(context) = self.connection_state.take_windterm_accounts_context() else {
            return true;
        };
        let decisions = context.decisions();
        let path = context.path;
        let master_password = context.master_password;
        let _ = context.preview;
        self.forget_text_inputs("windterm-account.");
        self.connection_state.begin_session_import_job();
        self.shell
            .set_status(t!("savedConnections.importInProgress"));
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("connection-import", move |_| {
                let outcome = match prepare_windterm_import_with_decisions(
                    &path,
                    master_password.as_deref(),
                    &decisions,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return ConnectionImportResult::Failed {
                            source: ConnectionImportSource::WindTerm,
                            auto_termius: false,
                            error: error.to_string(),
                        };
                    }
                };
                drop(master_password);
                let nyaterm_core::WindtermImportOutcome { prepared, report } = outcome;
                match store.request_mutation_fn(StoreDomain::Connections, move |database| {
                    database.commit_session_import(prepared)
                }) {
                    Ok(count) => ConnectionImportResult::Imported {
                        count,
                        incomplete: report.credential_warning_sessions,
                        report: Some(ConnectionImportReport::Windterm(report)),
                        notices: Vec::new(),
                    },
                    Err(error) => ConnectionImportResult::Failed {
                        source: ConnectionImportSource::WindTerm,
                        auto_termius: false,
                        error: error.to_string(),
                    },
                }
            });
            let result = match await_blocking_job(task).await {
                Ok(result) => result,
                Err(error) => ConnectionImportResult::Failed {
                    source: ConnectionImportSource::WindTerm,
                    auto_termius: false,
                    error,
                },
            };
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.apply_connection_import_result(result, window, cx)
                });
            });
        })
        .detach();
        cx.notify();
        true
    }

    pub(in crate::features) fn cancel_windterm_accounts_import(&mut self, cx: &mut Context<Self>) {
        if !self.connection_state.awaiting_windterm_accounts() {
            return;
        }
        self.forget_text_inputs("windterm-account.");
        self.connection_state.finish_import_path_prompt();
        self.shell
            .set_status(t!("savedConnections.importCancelled"));
        cx.notify();
    }

    fn prepare_windterm_account_inputs(&mut self, cx: &mut Context<Self>) {
        let drafts = self.connection_state.windterm_account_drafts().to_vec();
        for draft in drafts {
            let username_id = format!("windterm-account.{}.username", draft.one_key_uuid);
            let password_id = format!("windterm-account.{}.password", draft.one_key_uuid);
            self.text_input(username_id, &draft.username, TextInputSetup::default(), cx);
            self.text_input(
                password_id,
                draft.password.expose_secret(),
                TextInputSetup::masked(),
                cx,
            );
        }
    }

    pub(in crate::features) fn apply_windterm_account_input(
        &mut self,
        uuid: &str,
        field: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let changed = match field {
            "username" => self
                .connection_state
                .update_windterm_account_username(uuid, text),
            "password" => self
                .connection_state
                .update_windterm_account_password(uuid, text),
            _ => false,
        };
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_windterm_account_skip(
        &mut self,
        uuid: &str,
        skip: bool,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_windterm_account_skip(uuid, skip) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_windterm_account_reuse(
        &mut self,
        uuid: &str,
        account_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_windterm_account_reuse(uuid, account_id)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_connection_import_report(&mut self, cx: &mut Context<Self>) {
        self.connection_state.toggle_import_report_expanded();
        cx.notify();
    }

    pub(in crate::features) fn dismiss_connection_import_report(&mut self, cx: &mut Context<Self>) {
        self.connection_state.dismiss_import_report();
        cx.notify();
    }

    fn apply_connection_import_result(
        &mut self,
        result: ConnectionImportResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.finish_import_path_prompt();
        match result {
            ConnectionImportResult::NeedsMobaxtermPassword(context) => {
                self.connection_state.await_mobaxterm_password(
                    context.path,
                    context.passwords_path,
                    context.decryption_failed,
                );
                self.open_mobaxterm_password_dialog(window, cx);
            }
            ConnectionImportResult::NeedsWindtermPassword(path) => {
                self.connection_state.await_windterm_password(path);
                self.open_windterm_password_dialog(window, cx);
            }
            ConnectionImportResult::NeedsWindtermAccounts {
                preview,
                path,
                master_password,
            } => {
                let existing_passwords = self.security.passwords().to_vec();
                self.connection_state.await_windterm_accounts(
                    *preview,
                    path,
                    master_password,
                    &existing_passwords,
                );
                self.prepare_windterm_account_inputs(cx);
                self.open_windterm_accounts_dialog(window, cx);
            }
            ConnectionImportResult::Imported {
                count,
                incomplete,
                report,
                notices,
            } => {
                self.refresh_store_from_runtime_and_sync_theme(cx);
                self.connection_state.expand_all_catalog_groups();
                let base = if incomplete == 0 {
                    t!("savedConnections.importSuccess", count = count)
                } else {
                    t!(
                        "savedConnections.importPartialCredentials",
                        count = count,
                        incomplete = incomplete
                    )
                };
                let skipped = notices
                    .iter()
                    .filter(|notice| notice.kind.is_skipped())
                    .count();
                let mut notice_messages = notices
                    .iter()
                    .map(|notice| {
                        format!(
                            "{} — {}",
                            notice.session_name,
                            connection_import_notice_reason(notice.kind)
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(ConnectionImportReport::Mobaxterm(report)) = &report {
                    notice_messages.push(
                        t!(
                            "savedConnections.mobaxtermLinkedCredentials",
                            count = report.linked_credentials
                        )
                        .to_string(),
                    );
                    notice_messages.extend(report.diagnostics.iter().map(|diagnostic| {
                        let key = match diagnostic.kind {
                            MobaXtermCredentialDiagnosticKind::RegistryOnly => {
                                "savedConnections.mobaxtermRegistryOnly"
                            }
                            MobaXtermCredentialDiagnosticKind::SkippedEncrypted => {
                                "savedConnections.mobaxtermSkippedEncrypted"
                            }
                            MobaXtermCredentialDiagnosticKind::Undecryptable => {
                                "savedConnections.mobaxtermUndecryptable"
                            }
                            MobaXtermCredentialDiagnosticKind::MissingReference => {
                                "savedConnections.mobaxtermMissingReference"
                            }
                            MobaXtermCredentialDiagnosticKind::AmbiguousMatch => {
                                "savedConnections.mobaxtermAmbiguousMatch"
                            }
                            MobaXtermCredentialDiagnosticKind::Unmatched => {
                                "savedConnections.mobaxtermUnmatched"
                            }
                        };
                        t!(key, count = diagnostic.count).to_string()
                    }));
                }
                let report_view = match &report {
                    Some(ConnectionImportReport::Windterm(report)) => {
                        ConnectionImportReportView::from_windterm(report)
                    }
                    _ => ConnectionImportReportView::from_basic(
                        count,
                        incomplete,
                        skipped,
                        notice_messages,
                    ),
                };
                let missing = report_view.missing_one_key.len();
                let one_key_notice = report_view.one_key_state.map(|state| match state {
                    nyaterm_core::WindtermOneKeyConfigState::Absent => {
                        t!("savedConnections.windtermOneKeysConfigMissing").to_string()
                    }
                    nyaterm_core::WindtermOneKeyConfigState::Unreadable => {
                        t!("savedConnections.windtermOneKeysConfigUnreadable").to_string()
                    }
                    nyaterm_core::WindtermOneKeyConfigState::Readable => String::new(),
                });
                let message = match (missing, one_key_notice.filter(|notice| !notice.is_empty())) {
                    (0, None) => base.to_string(),
                    (0, Some(notice)) => format!("{base}；{notice}"),
                    (missing, notice) => {
                        let missing_message = t!(
                            "savedConnections.windtermMissingOneKeySummary",
                            count = missing
                        );
                        match notice {
                            Some(notice) => format!("{base}；{missing_message}；{notice}"),
                            None => format!("{base}；{missing_message}"),
                        }
                    }
                };
                self.connection_state.set_import_report(report_view);
                self.shell.set_status(message.clone());
                self.settings.update_store_status(message, true);
            }
            ConnectionImportResult::Cancelled => {
                self.shell
                    .set_status("connection import cancelled".to_string());
            }
            ConnectionImportResult::Failed {
                source,
                auto_termius,
                error,
            } => {
                if source == ConnectionImportSource::Termius
                    && auto_termius
                    && error.contains("Termius IndexedDB directory was not found")
                {
                    self.shell
                        .set_status("select Termius IndexedDB directory".to_string());
                    self.prompt_connection_session_import(
                        ConnectionImportSource::Termius,
                        window,
                        cx,
                    );
                    return;
                }
                let message = t!("savedConnections.importFailed", error = error);
                self.shell.set_status(message.clone());
                self.settings.update_store_status(message, false);
            }
            ConnectionImportResult::Closed => {
                self.shell
                    .set_status("connection import picker closed".to_string());
            }
        }
        cx.notify();
    }
}

fn connection_import_notice_reason(kind: SessionImportNoticeKind) -> String {
    match kind {
        SessionImportNoticeKind::SkippedUnsupportedType => {
            t!("savedConnections.importNoticeUnsupportedType").to_string()
        }
        SessionImportNoticeKind::SkippedMalformedEntry => {
            t!("savedConnections.importNoticeMalformedEntry").to_string()
        }
        SessionImportNoticeKind::SkippedMissingEndpoint => {
            t!("savedConnections.importNoticeMissingEndpoint").to_string()
        }
        SessionImportNoticeKind::SkippedMissingSerialPort => {
            t!("savedConnections.importNoticeMissingSerialPort").to_string()
        }
        SessionImportNoticeKind::SerialSettingsDefaulted => {
            t!("savedConnections.importNoticeSerialDefaults").to_string()
        }
        SessionImportNoticeKind::VncSettingsDefaulted => {
            t!("savedConnections.importNoticeVncDefaults").to_string()
        }
    }
}

fn prepare_windterm_source(
    path: Option<&std::path::Path>,
    master_password: Option<&str>,
) -> Result<PreparedConnectionImport, SessionImportError> {
    let path = path.ok_or_else(|| {
        SessionImportError::Config("connection import path was not selected".to_string())
    })?;
    let preview = prepare_windterm_preview(path, master_password)?;
    if preview.accounts.is_empty() {
        // No referenced OneKey account (missing/unreadable config included):
        // keep the single-step import, but carry the diagnostics with it.
        Ok(PreparedConnectionImport::Commit {
            report: Some(ConnectionImportReport::Windterm(preview.report())),
            prepared: preview.prepared,
        })
    } else {
        Ok(PreparedConnectionImport::WindtermAccounts(preview))
    }
}

fn prepare_connection_source(
    source: ConnectionImportSource,
    path: Option<&std::path::Path>,
    master_password: Option<&str>,
    mobaxterm_passwords: Option<&std::path::Path>,
) -> Result<nyaterm_core::PreparedSessionImport, SessionImportError> {
    match source {
        ConnectionImportSource::Termius => {
            let mut local_key =
                load_termius_local_key_secret().map_err(SessionImportError::Config)?;
            let result = nyaterm_core::prepare_termius_session_import(path, &local_key);
            local_key.zeroize();
            result
        }
        ConnectionImportSource::SshConfig => {
            let path = path.ok_or_else(|| {
                SessionImportError::Config(
                    t!("savedConnections.importSshConfigPathMissing").to_string(),
                )
            })?;
            nyaterm_core::prepare_ssh_config_import(path)
        }
        _ => {
            let path = path.ok_or_else(|| {
                SessionImportError::Config("connection import path was not selected".to_string())
            })?;
            prepare_session_import_with_options(
                path,
                SessionImportOptions {
                    windterm_master_password: master_password,
                    mobaxterm_passwords_path: mobaxterm_passwords,
                    ..SessionImportOptions::default()
                },
            )
        }
    }
}

fn load_termius_local_key_secret() -> Result<Vec<u8>, String> {
    let mut errors = Vec::new();

    #[cfg(target_os = "windows")]
    {
        match keyring::Entry::new_with_target("Termius/localKey", "Termius", "localKey")
            .and_then(|entry| entry.get_secret())
        {
            Ok(secret) => return Ok(secret),
            Err(error) => errors.push(format!(
                "target=Termius/localKey service=Termius user=localKey: {error}"
            )),
        }
    }

    for (service, user) in [("Termius/localKey", "localKey"), ("Termius", "localKey")] {
        match keyring::Entry::new(service, user).and_then(|entry| entry.get_secret()) {
            Ok(secret) => return Ok(secret),
            Err(error) => errors.push(format!("{service}/{user}: {error}")),
        }
    }

    Err(format!(
        "Cannot read Termius localKey from the system keychain. Tried {}",
        errors.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::ConnectionImportSource;

    #[test]
    fn connection_import_sources_choose_expected_picker_kind() {
        assert!(!ConnectionImportSource::Xshell.uses_directory_picker());
        assert!(!ConnectionImportSource::SecureCrt.uses_directory_picker());
        assert!(!ConnectionImportSource::Electerm.uses_directory_picker());
        assert!(!ConnectionImportSource::SshConfig.uses_directory_picker());
        assert!(ConnectionImportSource::FinalShell.uses_directory_picker());
        assert!(ConnectionImportSource::Termius.uses_directory_picker());
        assert_eq!(
            ConnectionImportSource::Termius.prompt_label(),
            "Import Termius IndexedDB directory"
        );
    }
}

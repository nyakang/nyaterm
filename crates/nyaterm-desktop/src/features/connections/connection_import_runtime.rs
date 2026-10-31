use rust_i18n::t;

use std::borrow::Cow;
use std::path::PathBuf;

use nyaterm_core::SecretString;
use nyaterm_core::session_import::{
    SessionImportError, SessionImportOptions, prepare_session_import_with_options,
};

use gpui::{Context, IntoElement, PathPromptOptions, SharedString, Window};
use nyaterm_store::StoreDomain;
use nyaterm_ui::NyaDialogWindowExt as _;
use zeroize::Zeroize as _;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use crate::models::ConnectionImportSource;

enum ConnectionImportResult {
    Imported {
        count: usize,
        incomplete: usize,
    },
    NeedsWindtermPassword(PathBuf),
    Cancelled,
    Failed {
        source: ConnectionImportSource,
        auto_termius: bool,
        error: String,
    },
    Closed,
}

impl ConnectionImportSource {
    fn prompt_label(self) -> Cow<'static, str> {
        match self {
            Self::NyatermBackup => Cow::Borrowed("Import NyaTerm backup"),
            Self::Xshell => Cow::Borrowed("Import Xshell .xts sessions"),
            Self::MobaXterm => Cow::Borrowed("Import MobaXterm .mxtsessions sessions"),
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
        if source == ConnectionImportSource::Termius {
            self.start_connection_import(source, None, None, true, window, cx);
            return;
        }
        self.prompt_connection_session_import(source, window, cx);
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
                                    Some(path),
                                    None,
                                    false,
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
        path: Option<PathBuf>,
        master_password: Option<SecretString>,
        auto_termius: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.begin_session_import_job();
        self.shell
            .set_status(t!("savedConnections.importInProgress"));
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("connection-import", move |_| {
                let prepared =
                    prepare_connection_source(source, path.as_deref(), master_password.as_deref());
                drop(master_password);
                match prepared {
                    Ok(prepared) => {
                        let incomplete = prepared.incomplete_credentials_count();
                        match store.request_mutation_fn(StoreDomain::Connections, move |database| {
                            database.commit_session_import(prepared)
                        }) {
                            Ok(count) => ConnectionImportResult::Imported { count, incomplete },
                            Err(error) => ConnectionImportResult::Failed {
                                source,
                                auto_termius,
                                error: error.to_string(),
                            },
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
            Some(path),
            Some(password),
            false,
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

    fn apply_connection_import_result(
        &mut self,
        result: ConnectionImportResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.finish_import_path_prompt();
        match result {
            ConnectionImportResult::NeedsWindtermPassword(path) => {
                self.connection_state.await_windterm_password(path);
                self.open_windterm_password_dialog(window, cx);
            }
            ConnectionImportResult::Imported { count, incomplete } => {
                self.refresh_store_from_runtime_and_sync_theme(cx);
                self.connection_state.expand_all_catalog_groups();
                let message = if incomplete == 0 {
                    t!("savedConnections.importSuccess", count = count)
                } else {
                    t!(
                        "savedConnections.importPartialCredentials",
                        count = count,
                        incomplete = incomplete
                    )
                };
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

fn prepare_connection_source(
    source: ConnectionImportSource,
    path: Option<&std::path::Path>,
    master_password: Option<&str>,
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

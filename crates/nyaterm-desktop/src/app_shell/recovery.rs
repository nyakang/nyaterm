//! A failed directory selection never owns a store or starts a workspace.
use gpui::{
    AnyElement, AppContext as _, Context, IntoElement, ParentElement, PathPromptOptions, Styled,
    div, prelude::FluentBuilder, px, rgb,
};
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::data_directory::{DataDirectoryRecovery, PreparedSwitch};
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use super::{AppShell, AppShellLifecycle};

pub(super) struct DirectoryRecoveryState {
    context: DataDirectoryRecovery,
    choice: Option<PreparedSwitch>,
    busy: bool,
}

impl DirectoryRecoveryState {
    pub(super) fn new(context: DataDirectoryRecovery) -> Self {
        Self {
            context,
            choice: None,
            busy: false,
        }
    }
}

impl AppShell {
    fn directory_recovery_mut(&mut self) -> Option<&mut DirectoryRecoveryState> {
        match &mut self.lifecycle {
            AppShellLifecycle::Recovery(state) => state.data_directory.as_deref_mut(),
            _ => None,
        }
    }

    pub(super) fn recovery_restart_failed(&mut self, message: String, cx: &mut Context<Self>) {
        if let AppShellLifecycle::Recovery(state) = &mut self.lifecycle {
            state.message = message;
            if let Some(directory) = &mut state.data_directory {
                directory.busy = false;
            }
            cx.notify();
        }
    }

    fn restart_directory_recovery(&mut self, use_choice: bool, cx: &mut Context<Self>) {
        let Some(state) = self.directory_recovery_mut() else {
            return;
        };
        if state.busy {
            return;
        }
        let choice = if use_choice {
            let Some(choice) = state.choice.clone() else {
                return;
            };
            Some(choice)
        } else {
            None
        };
        state.busy = true;
        let controller = self.controller.clone();
        cx.defer(move |cx| {
            controller.update(cx, |controller, cx| {
                controller.restart_from_recovery(choice, cx)
            })
        });
        cx.notify();
    }

    fn prepare_recovery_directory(&mut self, target: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(state) = self.directory_recovery_mut() else {
            return;
        };
        if state.busy || !state.context.can_select {
            return;
        }
        state.busy = true;
        state.choice = None;
        let local = self.runtime.local_dir().to_path_buf();
        let task = cx.background_spawn(async move {
            nyaterm_store::data_directory::prepare_recovery_selection(
                &local,
                &target,
                AppFlavor::current(),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |shell, cx| {
                let Some(state) = shell.directory_recovery_mut() else {
                    return;
                };
                state.busy = false;
                match result {
                    Ok(choice) => state.choice = Some(choice),
                    Err(error) => shell.recovery_restart_failed(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_recovery_directory(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("settings.dataDirectoryPickPrompt").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |shell, cx| shell.prepare_recovery_directory(path, cx));
            }
        })
        .detach();
    }

    pub(super) fn directory_recovery_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let AppShellLifecycle::Recovery(recovery) = &self.lifecycle else {
            return div().into_any_element();
        };
        let Some(state) = &recovery.data_directory else {
            return div().into_any_element();
        };
        let default_dir = state.context.default_dir.clone();
        let source = state
            .context
            .pending
            .as_ref()
            .and_then(|pending| pending.source.as_ref())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| state.context.configured.clone());
        let can_select = state.context.can_select && !state.busy;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x101214))
            .text_color(rgb(0xe7e9ea))
            .child(
                div()
                    .w(px(640.))
                    .max_w_full()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .child(t!("settings.dataDirectoryRecoveryTitle")),
                    )
                    .child(t!("settings.dataDirectoryRecoveryDesc"))
                    .child(state.context.configured.display().to_string())
                    .child(recovery.message.clone())
                    .when(state.busy, |view| view.child(t!("common.loading")))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                NyaButton::new(
                                    "directory-retry",
                                    t!("settings.dataDirectoryRecoveryRetry"),
                                )
                                .disabled(state.busy)
                                .variant(NyaButtonVariant::Primary)
                                .on_click(cx.listener(
                                    |shell, _, _, cx| shell.restart_directory_recovery(false, cx),
                                )),
                            )
                            .when(state.context.can_select, |view| {
                                view.child(
                                    NyaButton::new(
                                        "directory-default",
                                        t!("settings.dataDirectoryRecoveryDefault"),
                                    )
                                    .disabled(!can_select)
                                    .on_click(cx.listener(
                                        move |shell, _, _, cx| {
                                            shell
                                                .prepare_recovery_directory(default_dir.clone(), cx)
                                        },
                                    )),
                                )
                                .child(
                                    NyaButton::new(
                                        "directory-other",
                                        t!("settings.dataDirectoryRecoveryChoose"),
                                    )
                                    .disabled(!can_select)
                                    .on_click(cx.listener(
                                        |shell, _, _, cx| shell.choose_recovery_directory(cx),
                                    )),
                                )
                            })
                            .when(
                                state.context.pending.is_some() && state.context.can_select,
                                |view| {
                                    view.child(
                                        NyaButton::new(
                                            "directory-cancel-pending",
                                            t!("settings.dataDirectoryRecoverySource"),
                                        )
                                        .disabled(!can_select)
                                        .on_click(
                                            cx.listener(move |shell, _, _, cx| {
                                                shell.prepare_recovery_directory(source.clone(), cx)
                                            }),
                                        ),
                                    )
                                },
                            )
                            .child(
                                NyaButton::new(
                                    "directory-quit",
                                    t!("settings.dataDirectoryRecoveryQuit"),
                                )
                                .disabled(state.busy)
                                .on_click(cx.listener(
                                    |shell, _, _, cx| {
                                        let controller = shell.controller.clone();
                                        cx.defer(move |cx| {
                                            controller.update(cx, |controller, cx| {
                                                controller.request_quit(cx)
                                            })
                                        });
                                    },
                                )),
                            ),
                    )
                    .when_some(state.choice.as_ref(), |view, choice| {
                        view.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    t!(
                                        "settings.dataDirectoryRecoveryConfirm",
                                        path = choice.target.display().to_string()
                                    )
                                    .to_string(),
                                )
                                .child(
                                    NyaButton::new(
                                        "directory-confirm",
                                        t!("settings.dataDirectoryApply"),
                                    )
                                    .disabled(state.busy)
                                    .on_click(cx.listener(
                                        |shell, _, _, cx| {
                                            shell.restart_directory_recovery(true, cx)
                                        },
                                    )),
                                )
                                .child(
                                    NyaButton::new("directory-cancel", t!("common.cancel"))
                                        .disabled(state.busy)
                                        .on_click(cx.listener(|shell, _, _, cx| {
                                            if let Some(state) = shell.directory_recovery_mut() {
                                                state.choice = None;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

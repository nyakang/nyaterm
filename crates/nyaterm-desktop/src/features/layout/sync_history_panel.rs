use rust_i18n::t;

use gpui::{
    ClipboardItem, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, svg,
};
use nyaterm_core::{CloudConflictKind, truncate_preview};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    cloud_sync_status_dot_color, cloud_sync_status_text_color, configured_cloud_sync_provider,
    format_cloud_provider, format_duration_ms,
};
use crate::features::view_widgets::{
    CloudSyncHistoryRowLabels, cloud_sync_history_row, dialog_action_button,
};
use crate::widgets::small_button;
use nyaterm_ui::{NyaScrollable, NyaTooltip};

impl NyaTermApp {
    pub(in crate::features) fn sync_backup_history_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // Tauri SyncBackupHistoryPanel:
        // shared PanelHeader + status strip + optional conflict card + dense history list.
        let provider = configured_cloud_sync_provider(self.cloud_sync.settings());
        let provider_label = format_cloud_provider(&provider);
        let enabled = self.cloud_sync.settings().enabled;
        let status_message = self.cloud_sync.status().to_string();
        let last_history_status = self
            .cloud_sync
            .history()
            .first()
            .map(|entry| entry.status.as_str());
        let state = if !enabled {
            "disabled"
        } else if self.cloud_sync.conflict().is_some() {
            "conflict"
        } else if self.cloud_sync.job_running() {
            "running"
        } else {
            match last_history_status {
                Some("failed") => "failed",
                Some("success") => "success",
                _ => "idle",
            }
        };
        let state_label = match state {
            "disabled" => t!("settings.syncState.disabled"),
            "conflict" => t!("settings.syncState.conflict"),
            "failed" => t!("settings.syncState.failed"),
            "running" => t!("settings.syncState.running"),
            "success" => t!("settings.syncState.success"),
            _ => t!("settings.syncState.idle"),
        };
        let history = self.cloud_sync.history().to_vec();
        let expanded = self.cloud_sync.history_expanded().clone();
        let conflict = self.cloud_sync.conflict().cloned();
        let sync_action_enabled = enabled && !self.cloud_sync.job_running();

        let mut rows = div().flex().flex_col();
        if history.is_empty() {
            rows = rows.child(
                div()
                    .py_6()
                    .px_3()
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(t!("settings.historyNoEntries")),
            );
        } else {
            for entry in history {
                let entry_id = entry.id.clone();
                let is_open = expanded.contains(&entry_id);
                let copy_message = entry.message.clone();
                let kind_label = t!(match entry.kind.as_str() {
                    "sync" => "settings.historyKindSync",
                    "backup" => "settings.historyKindBackup",
                    _ => "settings.historyKindSync",
                });
                let status_label = t!(match entry.status.as_str() {
                    "success" => "settings.syncState.success",
                    "failed" => "settings.syncState.failed",
                    "conflict" => "settings.syncState.conflict",
                    "running" => "settings.syncState.running",
                    _ => "settings.syncState.idle",
                });
                let trigger_label =
                    t!("settings.historyTrigger", value = entry.trigger).to_string();
                let provider = entry
                    .provider
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(format_cloud_provider)
                    .unwrap_or_else(|| "-".to_string());
                let provider_label = t!("settings.historyProvider", value = provider).to_string();
                let duration =
                    format_duration_ms(entry.duration_ms).unwrap_or_else(|| "-".to_string());
                let duration_label = t!("settings.historyDuration", value = duration).to_string();
                rows = rows.child(cloud_sync_history_row(
                    palette,
                    entry,
                    CloudSyncHistoryRowLabels {
                        kind: kind_label.to_string(),
                        status: status_label.to_string(),
                        trigger: trigger_label,
                        provider: provider_label,
                        duration: duration_label,
                        revision: t!("settings.historyRevision"),
                        view_details: t!("settings.historyViewDetails"),
                        hide_details: t!("settings.historyHideDetails"),
                        copy_message: t!("settings.historyCopyMessage"),
                    },
                    is_open,
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_cloud_sync_history_details(&entry_id, cx);
                    }),
                    cx.listener(move |this, _, _, cx| {
                        if copy_message.trim().is_empty() {
                            this.shell
                                .set_status("history entry has no message".to_string());
                        } else {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_message.clone()));
                            this.shell
                                .set_status("sync history message copied".to_string());
                        }
                        cx.notify();
                    }),
                ));
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.shell_transparent_color(palette.surface))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_transparent_color(palette.surface))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .flex_none()
                                    .bg(cloud_sync_status_dot_color(palette, state)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("settings.historyCurrentState")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(cloud_sync_status_text_color(palette, state))
                                    .overflow_hidden()
                                    .child(state_label),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.border))
                                    .child("·"),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_muted))
                                    .overflow_hidden()
                                    .child(provider_label),
                            )
                            .child(sync_history_action_button(
                                palette,
                                "sync-history-push-now",
                                "icons/fe/upload.svg",
                                t!("settings.syncPushNow"),
                                sync_action_enabled,
                                cx.listener(move |this, _, window, cx| {
                                    if !this.cloud_sync.settings().enabled
                                        || this.cloud_sync.job_running()
                                    {
                                        this.shell.set_status(
                                            "cloud sync is disabled or already running".to_string(),
                                        );
                                        cx.notify();
                                        return;
                                    }
                                    this.prompt_provider_cloud_sync_push(window, cx);
                                }),
                            ))
                            .child(sync_history_action_button(
                                palette,
                                "sync-history-pull-now",
                                "icons/fe/download.svg",
                                t!("settings.syncPullNow"),
                                sync_action_enabled,
                                cx.listener(move |this, _, window, cx| {
                                    if !this.cloud_sync.settings().enabled
                                        || this.cloud_sync.job_running()
                                    {
                                        this.shell.set_status(
                                            "cloud sync is disabled or already running".to_string(),
                                        );
                                        cx.notify();
                                        return;
                                    }
                                    this.prompt_provider_cloud_sync_pull(window, cx);
                                }),
                            )),
                    )
                    .when(
                        !status_message.trim().is_empty() && conflict.is_none(),
                        |this| {
                            this.child(
                                div()
                                    .pl_4()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(truncate_preview(&status_message, 140)),
                            )
                        },
                    ),
            )
            .when_some(conflict, |this, conflict| {
                let preview = conflict.preview;
                let remote_inconsistent = preview.kind == CloudConflictKind::RemoteInconsistent;
                let recovery_revision = preview.recovery_revision.clone();
                this.child(
                    div()
                        .flex_none()
                        .m_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.warning))
                        .bg(rgb(palette.input))
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(palette.warning))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight(700.))
                                        .text_color(rgb(palette.warning))
                                        .child(if remote_inconsistent {
                                            t!("settings.syncRemoteIncompleteTitle")
                                        } else {
                                            t!("settings.syncConflictTitle")
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text))
                                .child(preview.message.clone()),
                        )
                        .child(
                            div()
                                .px_3()
                                .pb_2()
                                .grid()
                                .grid_cols(1)
                                .gap_2()
                                .child(
                                    div()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(palette.border))
                                        .bg(rgb(palette.input))
                                        .px_2()
                                        .py_1()
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(rgb(palette.text_muted))
                                                .child(t!("settings.providerLabel")),
                                        )
                                        .child(
                                            div()
                                                .mt_0()
                                                .font_family(
                                                    crate::features::shell::gpui_code_font_family(),
                                                )
                                                .text_size(px(11.))
                                                .text_color(rgb(palette.text))
                                                .child(format_cloud_provider(&preview.provider)),
                                        ),
                                )
                                .when_some(recovery_revision, |this, revision| {
                                    this.child(
                                        div()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(palette.border))
                                            .bg(rgb(palette.input))
                                            .px_2()
                                            .py_1()
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(palette.text_muted))
                                                    .child(
                                                        t!("settings.currentRemoteSnapshot"),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .font_family(
                                                        crate::features::shell::gpui_code_font_family(),
                                                    )
                                                    .text_size(px(11.))
                                                    .text_color(rgb(palette.text))
                                                    .child(truncate_preview(&revision, 16)),
                                            ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .px_3()
                                .pb_3()
                                .flex()
                                .gap_2()
                                .child(if remote_inconsistent {
                                    small_button(
                                        palette,
                                        "sync-panel-recover-current",
                                        t!("settings.useCurrentRemoteSnapshot"),
                                        cx.listener({
                                            let provider_action = conflict.provider_action;
                                            move |this, _, window, cx| {
                                                this.prompt_cloud_sync_recover_current(
                                                    provider_action,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }),
                                    )
                                    .into_any_element()
                                } else {
                                    small_button(
                                        palette,
                                        "sync-panel-force-pull",
                                        t!("settings.downloadRemoteVersion"),
                                        cx.listener({
                                            let provider_action = conflict.provider_action;
                                            move |this, _, window, cx| {
                                                this.prompt_cloud_sync_force_pull(
                                                    provider_action,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }),
                                    )
                                    .into_any_element()
                                })
                                .child(dialog_action_button(
                                    palette,
                                    "sync-panel-force-push",
                                    t!("settings.uploadLocalVersion"),
                                    false,
                                    cx.listener({
                                        let provider_action = conflict.provider_action;
                                        move |this, _, window, cx| {
                                            this.prompt_cloud_sync_force_push(
                                                provider_action,
                                                window,
                                                cx,
                                            );
                                        }
                                    }),
                                )),
                        ),
                )
            })
            .child(
                div()
                    .id(SharedString::from("sync-backup-history-list"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(rows),
            )
    }
}

fn sync_history_action_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let tooltip: SharedString = tooltip.into();
    let id: SharedString = id.into().into();
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .cursor_pointer()
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .hover(|this| {
            if enabled {
                this.bg(rgb(palette.surface_elevated))
            } else {
                this
            }
        })
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, px,
    };
    use nyaterm_core::cloud_sync::{CloudSyncSettings, SYNC_CURRENT_FILE};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use nyaterm_store::ConnectionStore;
    use nyaterm_ui::{NyaDialogWindowExt, nya_root};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::SnapshotPasswordPromptKind;
    use crate::test_support::{TestConfigDir, spawn_webdav_service_unavailable_server};

    struct SidebarHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for SidebarHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(320.))
                .h(px(720.))
                .child(self.app.update(cx, |app, cx| {
                    app.sync_backup_history_panel(cx).into_any_element()
                }))
        }
    }

    fn test_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let store = ConnectionStore::open(root.join("config")).expect("test store");
        store
            .save_master_password(Some("synthetic-password"))
            .expect("test master password");
        store
            .save_cloud_sync_settings(CloudSyncSettings {
                enabled: true,
                provider: "local_directory".to_string(),
                ..CloudSyncSettings::default()
            })
            .expect("test sync settings");
        drop(store);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        app
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    fn click_action(cx: &mut VisualTestContext, id: &'static str) {
        draw(cx);
        let bounds = cx.debug_bounds(id).expect("sync action should render");
        cx.simulate_click(bounds.center(), Modifiers::default());
        draw(cx);
    }

    #[test]
    fn sync_sidebar_actions_open_visible_password_dialogs_and_cancel_cleanly() {
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar");
        let mut cx = TestAppContext::single();
        let app = test_app(&mut cx, test_dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        for (id, kind) in [
            (
                "sync-history-push-now",
                SnapshotPasswordPromptKind::CloudProviderPush,
            ),
            (
                "sync-history-pull-now",
                SnapshotPasswordPromptKind::CloudProviderPull,
            ),
        ] {
            click_action(cx, id);
            assert!(
                cx.debug_bounds("snapshot-password-dialog-content")
                    .is_some()
            );
            assert!(cx.debug_bounds("nya-dialog-action-button").is_some());
            cx.update(|window, cx| {
                assert!(window.has_active_nya_dialog(cx));
                let state = app.read(cx);
                assert_eq!(
                    state
                        .settings
                        .snapshot_password_prompt()
                        .expect("prompt")
                        .kind,
                    kind
                );
                assert!(!state.cloud_sync.job_running());
            });
            cx.simulate_keystrokes("enter");
            draw(cx);
            cx.update(|window, cx| {
                assert!(
                    window.has_active_nya_dialog(cx),
                    "empty passwords must keep the dialog open"
                );
                assert!(app.read(cx).settings.snapshot_password_prompt_active());
                assert!(!app.read(cx).cloud_sync.job_running());
            });
            cx.simulate_keystrokes("escape");
            draw(cx);
            cx.update(|window, cx| {
                assert!(!window.has_active_nya_dialog(cx));
                assert!(!app.read(cx).settings.snapshot_password_prompt_active());
                assert!(app.read(cx).cloud_sync.history().is_empty());
            });
        }
    }

    #[test]
    fn sync_sidebar_webdav_submission_reports_http_failure() {
        let (endpoint, server) = spawn_webdav_service_unavailable_server();
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-webdav");
        let mut cx = TestAppContext::single();
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            let mut settings = app.cloud_sync.settings().clone();
            settings.provider = "webdav".to_string();
            settings.webdav.endpoint = endpoint;
            app.cloud_sync
                .replace_settings(settings, Default::default());
        });
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        click_action(cx, "sync-history-push-now");
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.reset_text_input("snapshot-password.value", "synthetic-password", cx);
                app.apply_snapshot_password_input("synthetic-password".to_string(), cx);
            })
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            draw(cx);
            let recorded = cx.update(|_, cx| {
                let state = app.read(cx);
                state
                    .cloud_sync
                    .history()
                    .iter()
                    .find(|entry| entry.trigger == "manual_provider_push")
                    .map(|entry| {
                        assert_ne!(entry.status, "success");
                        assert!(state.cloud_sync.status().contains("503"));
                        assert!(!state.cloud_sync.job_running());
                    })
                    .is_some()
            });
            if recorded {
                break;
            }
            assert!(Instant::now() < deadline, "WebDAV failure was not recorded");
            std::thread::sleep(Duration::from_millis(5));
        }
        server.join().expect("mock WebDAV server");
    }

    #[test]
    fn sync_sidebar_password_submission_runs_push_and_pull_and_records_history() {
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-roundtrip");
        let mut cx = TestAppContext::single();
        let app = test_app(&mut cx, test_dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        for (id, trigger, keyboard_submit) in [
            ("sync-history-push-now", "manual_provider_push", true),
            ("sync-history-pull-now", "manual_provider_pull", false),
        ] {
            click_action(cx, id);
            cx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.reset_text_input("snapshot-password.value", "synthetic-password", cx);
                    app.apply_snapshot_password_input("synthetic-password".to_string(), cx);
                })
            });
            draw(cx);
            if keyboard_submit {
                cx.simulate_keystrokes("enter");
            } else {
                let submit = cx
                    .debug_bounds("nya-dialog-action-button")
                    .expect("submit button");
                cx.simulate_click(submit.center(), Modifiers::default());
            }
            draw(cx);
            cx.update(|window, cx| {
                assert!(
                    !window.has_active_nya_dialog(cx),
                    "submission must close the password dialog"
                );
                assert!(!app.read(cx).settings.snapshot_password_prompt_active());
            });
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                draw(cx);
                let recorded = cx.update(|_, cx| {
                    app.read(cx)
                        .cloud_sync
                        .history()
                        .iter()
                        .find(|entry| entry.trigger == trigger)
                        .map(|entry| entry.status.clone())
                });
                if let Some(status) = recorded {
                    assert_eq!(status, "success");
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "sync submission did not record an operation"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                test_dir
                    .path()
                    .join("config/cloud-sync-local/nyaterm")
                    .join(SYNC_CURRENT_FILE)
                    .is_file(),
                "push must produce an encrypted remote snapshot"
            );
        }
    }
}

use rust_i18n::t;

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::{CloudConflictKind, truncate_preview};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    SyncHistoryTimeFormatter, cloud_sync_status_dot_color, configured_cloud_sync_provider,
    format_cloud_provider,
};
use crate::features::sync::SyncHistoryFilter;
use crate::features::view_widgets::{cloud_sync_history_row, dialog_action_button};
use crate::widgets::small_button;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaTabItem, NyaTabs, NyaTabsVariant};

impl NyaTermApp {
    pub(in crate::features) fn sync_backup_history_panel(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let provider = configured_cloud_sync_provider(self.cloud_sync.settings());
        let provider_label = format_cloud_provider(&provider);
        let status_message = self.cloud_sync.status().to_string();
        let state = self.cloud_sync.activity_status();
        let state_label = t!(match state {
            "disabled" => "settings.syncState.disabled",
            "conflict" => "settings.syncHistory.needsAttention",
            "running" => "settings.syncState.running",
            _ => "settings.syncState.idle",
        });
        let formatter = SyncHistoryTimeFormatter::new();
        let last_success = self
            .cloud_sync
            .history()
            .iter()
            .filter(|entry| {
                entry.kind == "sync"
                    && entry.status == "success"
                    && entry.provider.as_deref() == Some(provider.as_str())
            })
            .map(|entry| entry.timestamp_ms)
            .max()
            .map(|timestamp| {
                format!(
                    "{} {}",
                    formatter.day(timestamp),
                    formatter.clock(timestamp)
                )
            });
        let history = self.cloud_sync.history().to_vec();
        let expanded = self.cloud_sync.history_expanded().clone();
        let conflict = self.cloud_sync.conflict().cloned();
        let branch_view = conflict.as_ref().map(|conflict| {
            let options =
                crate::features::view_widgets::cloud_sync_conflict::branch_options(conflict);
            let select = self.select_entity(
                "cloud-conflict-branch",
                options,
                conflict.selected_revision.clone(),
                self.cloud_sync.job_running(),
                cx,
            );
            crate::features::view_widgets::cloud_sync_conflict::branch_details(
                conflict,
                div()
                    .w_full()
                    .h(px(nyaterm_ui::NYA_FORM_CONTROL_HEIGHT_PX))
                    .child(nyaterm_ui::NyaSelect::new(&select)),
            )
        });

        let filter = self.cloud_sync.history_filter();
        let limit = self.cloud_sync.history_visible_limit();
        let mut history = history;
        history.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp_ms));
        let matching_count = history.iter().filter(|entry| filter.matches(entry)).count();
        let filter_options = [
            (SyncHistoryFilter::All, t!("settings.historyAll")),
            (
                SyncHistoryFilter::Success,
                t!("settings.historyOverviewSuccess"),
            ),
            (
                SyncHistoryFilter::Failed,
                t!("settings.historyOverviewFailed"),
            ),
            (
                SyncHistoryFilter::Conflict,
                t!("settings.historyOverviewConflict"),
            ),
        ];
        let selected_index = filter_options
            .iter()
            .position(|(value, _)| *value == filter)
            .unwrap_or(0);
        let filter_focus = window
            .use_keyed_state("sync-history-filter-focus", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let filters = div()
            .id("sync-history-filter-keyboard")
            .track_focus(&filter_focus)
            .debug_selector(|| "sync-history-filter-keyboard".to_string())
            .tab_group()
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.modifiers == gpui::Modifiers::default()
                    && this
                        .cloud_sync
                        .navigate_history_filter(&event.keystroke.key)
                {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .px_2()
            .py_2()
            .flex_none()
            .child(
                NyaTabs::new("sync-history-filters")
                    .variant(NyaTabsVariant::Segmented)
                    .selected_index(selected_index)
                    .items(
                        filter_options
                            .iter()
                            .map(|(_, label)| NyaTabItem::new(label.clone())),
                    )
                    .on_select(cx.listener(move |this, index: &usize, window, cx| {
                        if let Some((value, _)) = filter_options.get(*index) {
                            window.focus(&filter_focus, cx);
                            this.cloud_sync.filter_history(*value);
                            cx.notify();
                        }
                    })),
            );
        let mut rows = div().flex().flex_col();
        if matching_count == 0 {
            rows = rows.child(
                div()
                    .py_6()
                    .px_3()
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(if history.is_empty() {
                        t!("settings.historyNoEntries")
                    } else {
                        t!("settings.historyNoResultsTitle")
                    }),
            );
        } else {
            let mut previous_day = None;
            for entry in history
                .into_iter()
                .filter(|entry| filter.matches(entry))
                .take(limit)
            {
                let day = formatter.day(entry.timestamp_ms);
                if previous_day.as_ref() != Some(&day) {
                    rows = rows.child(
                        div()
                            .px_3()
                            .pt_3()
                            .pb_1()
                            .text_size(px(10.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text_muted))
                            .child(day.clone()),
                    );
                    previous_day = Some(day);
                }
                let entry_id = entry.id.clone();
                let is_open = expanded.contains(&entry_id);
                // Copy all original fields, including full revisions and messages.
                let diagnostics = serde_json::to_string_pretty(&entry).unwrap_or_default();
                rows = rows.child(cloud_sync_history_row(
                    palette,
                    entry,
                    &formatter,
                    is_open,
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_cloud_sync_history_details(&entry_id, cx);
                    }),
                    cx.listener(move |this, _, _, cx| {
                        this.copy_text_with_feedback("copy-sync-history", diagnostics.clone(), cx);
                        cx.notify();
                    }),
                ));
            }
            if matching_count > limit {
                rows = rows.child(
                    div().p_2().child(
                        NyaButton::new(
                            "sync-history-show-more",
                            t!("settings.syncHistory.showMore"),
                        )
                        .variant(NyaButtonVariant::Ghost)
                        .small()
                        .full_width()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cloud_sync.show_more_history();
                            cx.notify();
                        })),
                    ),
                );
            }
        }

        let indicator = if state == "running" {
            nyaterm_ui::chat::running_indicator("cloud-sync-running", rgb(palette.link))
                .into_any_element()
        } else {
            div()
                .size(px(8.))
                .rounded_full()
                .flex_none()
                .bg(cloud_sync_status_dot_color(palette, state))
                .into_any_element()
        };
        let heading = div()
            .debug_selector(|| "sync-history-status-row".to_string())
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(indicator)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .truncate()
                    .child(state_label),
            )
            .child(
                div()
                    .min_w_0()
                    .max_w(gpui::relative(0.5))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(provider_label),
            );
        let last_success =
            last_success.unwrap_or_else(|| t!("settings.syncHistory.noSuccess").to_string());
        let header = div()
            .flex_none()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(rgb(palette.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(heading)
            .child(
                div()
                    .pl_4()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("settings.syncHistory.lastSuccess", value = last_success)),
            )
            .when(
                !status_message.trim().is_empty() && conflict.is_none(),
                |this| {
                    this.child(
                        div()
                            .pl_4()
                            .text_size(px(11.))
                            .line_height(px(17.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!(
                                "settings.syncHistory.operationNotice",
                                value = truncate_preview(&status_message, 140)
                            )),
                    )
                },
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.shell_transparent_color(palette.surface))
            .child(header)
            .when_some(conflict, |this, conflict| {
                let can_resolve = conflict.preview.local_content_hash.is_some();
                let can_pull = can_resolve && conflict.selected_revision.is_some();
                let preview = conflict.preview;
                let remote_inconsistent = preview.kind == CloudConflictKind::RemoteInconsistent;
                let overwrite_blocked = preview.overwrite_blocked;
                let recovery_revision = preview.recovery_revision.clone();
                let has_recovery_candidate = recovery_revision.is_some();
                this.child(
                    div()
                        .flex_none()
                        .m_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.warning))
                        .bg(rgb(palette.input))
                        .when_some(branch_view, |card, view| card.child(div().p_3().child(view)))
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
                                .child(if remote_inconsistent {
                                    t!("settings.syncHistory.incompleteHint")
                                } else {
                                    t!("settings.syncHistory.conflictExplanation")
                                }),
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
                                .child(if remote_inconsistent && has_recovery_candidate && !can_pull {
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
                                } else if remote_inconsistent && !can_pull {
                                    // Nothing on the remote can be recovered:
                                    // pulling would download the same unreadable
                                    // payload, so only the explicit overwrite is
                                    // offered.
                                    div().into_any_element()
                                } else if can_pull {
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
                                } else { div().text_xs().child(t!("settings.syncChooseBranch")).into_any_element() })
                                .when(!overwrite_blocked && can_resolve, |this| {
                                    this.child(dialog_action_button(
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
                                    ))
                                })
                                .when(overwrite_blocked, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(11.))
                                            .line_height(px(16.))
                                            .text_color(rgb(palette.warning))
                                            .child(t!("settings.overwriteBlockedNotice")),
                                    )
                                }),
                        ),
                )
            })
            .child(div().px_3().pt_3().text_size(px(12.)).font_weight(FontWeight(600.))
                .text_color(rgb(palette.text)).child(t!("settings.syncHistory.title")))
            .child(filters)
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
    use crate::test_support::{TestConfigDir, spawn_webdav_service_unavailable_server};

    struct SidebarHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for SidebarHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(320.))
                .h(px(720.))
                .child(self.app.update(cx, |app, cx| {
                    app.floating_side_panel(
                        crate::models::PanelSide::Left,
                        crate::models::NavItem::SyncBackupHistory,
                        window,
                        cx,
                    )
                    .into_any_element()
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
    fn sync_sidebar_header_actions_are_ordered_and_disabled_when_sync_is_disabled() {
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-layout");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| app.cloud_sync.toggle_enabled());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        draw(cx);
        let push = cx.debug_bounds("sync-history-push-now").unwrap();
        let pull = cx.debug_bounds("sync-history-pull-now").unwrap();
        let refresh = cx.debug_bounds("sync-history-header-refresh").unwrap();
        let status = cx.debug_bounds("sync-history-status-row").unwrap();
        assert!(push.origin.x < pull.origin.x && pull.origin.x < refresh.origin.x);
        assert_eq!(push.origin.y, refresh.origin.y);
        assert!(push.bottom() <= status.origin.y);
        for id in ["sync-history-push-now", "sync-history-pull-now"] {
            click_action(cx, id);
            cx.update(|window, cx| {
                assert!(!window.has_active_nya_dialog(cx));
                assert!(!app.read(cx).cloud_sync.job_running());
                assert!(app.read(cx).cloud_sync.history().is_empty());
            });
        }
    }

    fn wait_for_history(
        cx: &mut VisualTestContext,
        app: &Entity<NyaTermApp>,
        trigger: &str,
    ) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            draw(cx);
            let status = cx.update(|window, cx| {
                assert!(!window.has_active_nya_dialog(cx));
                assert!(!app.read(cx).settings.snapshot_password_prompt_active());
                app.read(cx)
                    .cloud_sync
                    .history()
                    .iter()
                    .find(|entry| entry.trigger == trigger)
                    .map(|entry| entry.status.clone())
            });
            if let Some(status) = status {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "sync operation did not record history"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn sync_sidebar_missing_master_password_opens_security_without_writing_a_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-missing-password");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::CloudSync, |store| {
                    store.save_master_password(None)
                })
                .unwrap();
        });
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        click_action(cx, "sync-history-push-now");
        assert_eq!(wait_for_history(cx, &app, "manual_provider_push"), "failed");
        cx.update(|_, cx| {
            let state = app.read(cx);
            assert_eq!(
                state.shell.settings_active_tab(),
                crate::models::SettingsTab::Security
            );
            assert!(state.shell.has_settings_draft());
            assert_eq!(
                state.cloud_sync.status(),
                rust_i18n::t!("settings.syncMasterPasswordRequired")
            );
            assert!(!state.cloud_sync.job_running());
        });
        assert!(
            !test_dir
                .path()
                .join("config/cloud-sync-local/nyaterm")
                .join(SYNC_CURRENT_FILE)
                .exists()
        );
    }

    #[test]
    fn local_backup_restore_requires_confirmation_before_password() {
        let test_dir = TestConfigDir::new("nyaterm-backup-restore-confirm");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.prompt_encrypted_portable_snapshot_import(window, cx)
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("nya-dialog-action-button").is_some());
        assert!(
            cx.debug_bounds("snapshot-password-dialog-content")
                .is_none()
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(!app.read(cx).settings.snapshot_password_prompt_active());
        });

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.prompt_encrypted_portable_snapshot_import(window, cx)
            });
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            cx.debug_bounds("snapshot-password-dialog-content")
                .is_some()
        );
    }

    #[test]
    fn sync_sidebar_webdav_submission_reports_http_failure() {
        let (endpoint, server) = spawn_webdav_service_unavailable_server();
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-webdav");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            let mut settings = app.cloud_sync.settings().clone();
            settings.provider = "webdav".to_string();
            settings.webdav.endpoint = endpoint;
            let persisted_settings = settings.clone();
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::CloudSync, move |store| {
                    store.save_cloud_sync_settings(persisted_settings)
                })
                .expect("persist the provider configuration used by the test");
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
        let deadline = Instant::now() + Duration::from_secs(60);
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
    fn sync_sidebar_saved_master_password_runs_push_and_pull_and_records_history() {
        let test_dir = TestConfigDir::new("nyaterm-sync-sidebar-roundtrip");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        for (id, trigger) in [
            ("sync-history-push-now", "manual_provider_push"),
            ("sync-history-pull-now", "manual_provider_pull"),
        ] {
            click_action(cx, id);
            cx.update(|window, cx| {
                assert!(
                    !window.has_active_nya_dialog(cx),
                    "cloud sync must not open a password dialog"
                );
                assert!(!app.read(cx).settings.snapshot_password_prompt_active());
            });
            let deadline = Instant::now() + Duration::from_secs(60);
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
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.prompt_cloud_sync_recover_current(true, window, cx)
            });
        });
        assert_eq!(
            wait_for_history(cx, &app, "recover_current_remote"),
            "success"
        );
    }
    #[test]
    fn cloud_sync_sidebar_branch_selection_uses_keyboard_and_restores_trigger_focus() {
        use nyaterm_core::cloud_sync::conflict::CloudConflictBranch;
        use nyaterm_core::{CloudConflictPreview, CloudSyncError};
        let test_dir = TestConfigDir::new("nyaterm-cloud-sync-branch-keyboard");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        let mut preview: CloudConflictPreview = serde_json::from_value(serde_json::json!({
            "detected_at_ms": 1, "provider": "test", "local_payload_hash": "local",
            "remote_payload_hash": "remote", "remote_revision": "r0", "remote_created_at_ms": 1,
            "remote_device_id": "device", "message": "conflict", "local_content_hash": "local-content"
        })).unwrap();
        preview.branches = (0..16)
            .map(|n| CloudConflictBranch {
                revision: format!("r{n}"),
                device_id: format!("device-{n}"),
                device_name: None,
                created_at_ms: 1,
                readable: n != 15,
                differences: Vec::new(),
            })
            .collect();
        preview.observed_revisions = preview
            .branches
            .iter()
            .map(|b| b.revision.clone())
            .collect();
        cx.update_entity(&app, |app, cx| {
            app.apply_auto_cloud_sync_result(&Err(CloudSyncError::Conflict(Box::new(preview))), cx);
        });
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let viewport = cx
            .debug_bounds("cloud-sync-branch-scroll")
            .expect("branch list is rendered");
        assert!(viewport.size.height <= px(260.));
        cx.update(|window, cx| {
            assert!(
                app.read(cx)
                    .cloud_sync
                    .conflict()
                    .unwrap()
                    .selected_revision
                    .is_none()
            );
            app.update(cx, |app, cx| {
                assert!(app.focus_select("cloud-conflict-branch", window, cx));
            });
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.simulate_keystrokes("down down down down");
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|window, cx| {
            let app = app.read(cx);
            let selected = app
                .cloud_sync
                .conflict()
                .unwrap()
                .selected_revision
                .as_deref()
                .expect("keyboard selection commits");
            assert_ne!(selected, "r0");
            assert_ne!(selected, "r15", "unreadable branches cannot be chosen");
            assert!(app.select_is_focused("cloud-conflict-branch", window, cx));
        });
    }
    #[test]
    fn sync_sidebar_running_job_disables_actions_and_rejects_duplicate_submission() {
        let dir = TestConfigDir::new("nyaterm-sync-sidebar-busy");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        let operation = crate::features::cloud_sync_operation_lock().lock().unwrap();
        click_action(cx, "sync-history-push-now");
        cx.update(|window, cx| {
            assert!(app.read(cx).cloud_sync.job_running());
            assert!(!window.has_active_nya_dialog(cx));
        });
        click_action(cx, "sync-history-push-now");
        click_action(cx, "sync-history-pull-now");
        drop(operation);
        assert_eq!(
            wait_for_history(cx, &app, "manual_provider_push"),
            "success"
        );
        cx.update(|_, cx| {
            assert_eq!(app.read(cx).cloud_sync.history().len(), 1);
        });
    }

    #[test]
    fn sync_sidebar_tabs_filter_with_pointer_and_keyboard_and_reset_paging() {
        let dir = TestConfigDir::new("nyaterm-sync-sidebar-filters");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        draw(cx);
        let bounds = cx.debug_bounds("sync-history-filter-keyboard").unwrap();
        // The four segmented tabs share the available width equally.
        cx.simulate_click(
            gpui::point(
                bounds.origin.x + bounds.size.width * 0.625,
                bounds.center().y,
            ),
            Modifiers::default(),
        );
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).cloud_sync.history_filter(),
                crate::features::sync::SyncHistoryFilter::Failed
            );
            app.update(cx, |app, _| app.cloud_sync.show_more_history());
        });
        cx.simulate_keystrokes("right");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).cloud_sync.history_filter(),
                crate::features::sync::SyncHistoryFilter::Conflict
            );
            assert_eq!(app.read(cx).cloud_sync.history_visible_limit(), 20);
        });
        cx.simulate_keystrokes("home right end left");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).cloud_sync.history_filter(),
                crate::features::sync::SyncHistoryFilter::Failed
            );
        });
    }

    #[test]
    fn sync_sidebar_remote_password_mismatch_preserves_local_state_and_remote_pointer() {
        let dir = TestConfigDir::new("nyaterm-sync-sidebar-password-mismatch");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        click_action(cx, "sync-history-push-now");
        assert_eq!(
            wait_for_history(cx, &app, "manual_provider_push"),
            "success"
        );
        let pointer_path = dir
            .path()
            .join("config/cloud-sync-local/nyaterm")
            .join(SYNC_CURRENT_FILE);
        let pointer = std::fs::read(&pointer_path).unwrap();
        let before = cx.update(|_, cx| {
            app.read(cx)
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::CloudSync, |store| {
                    store.save_master_password(Some("different-synthetic-password"))?;
                    store.load_cloud_sync_state()
                })
                .unwrap()
        });
        click_action(cx, "sync-history-pull-now");
        assert_ne!(
            wait_for_history(cx, &app, "manual_provider_pull"),
            "success"
        );
        cx.update(|_, cx| {
            let store = app.read(cx).store_blocking_client();
            let (mut after, password_matches) = store
                .request_fn(nyaterm_store::StoreDomain::CloudSync, |store| {
                    Ok((
                        store.load_cloud_sync_state()?,
                        store.verify_master_password("different-synthetic-password")?,
                    ))
                })
                .unwrap();
            // Successful upload cleanup may independently update its attempt timestamp.
            after.last_gc_attempt_at_ms = before.last_gc_attempt_at_ms;
            assert_eq!(before, after);
            assert!(password_matches);
            assert!(!app.read(cx).cloud_sync.job_running());
        });
        assert_eq!(std::fs::read(pointer_path).unwrap(), pointer);
    }
    #[test]
    fn sync_sidebar_conflict_actions_use_the_saved_master_password_and_pinned_preview() {
        use nyaterm_core::cloud_sync::{CloudLocalStore, load_sync_pointer};
        use nyaterm_core::{CloudConflictPreview, CloudSyncError, LocalCloudSyncOptions};
        let dir = TestConfigDir::new("nyaterm-sync-sidebar-resolve-password");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, dir.path());
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| SidebarHost { app: host_app });
            nya_root(host, window, cx)
        });
        click_action(cx, "sync-history-push-now");
        assert_eq!(
            wait_for_history(cx, &app, "manual_provider_push"),
            "success"
        );
        for (provider_action, use_cloud, trigger) in [
            (true, false, "manual_provider_force_push"),
            (true, true, "manual_provider_force_pull"),
            (false, false, "manual_force_push"),
            (false, true, "manual_force_pull"),
        ] {
            let store = cx.update(|_, cx| app.read(cx).store_blocking_client());
            let state = store
                .request_fn(nyaterm_store::StoreDomain::CloudSync, |store| {
                    store.load_cloud_sync_state()
                })
                .unwrap();
            let options = LocalCloudSyncOptions {
                device_name: None,
                config_dir: dir.join("config"),
                portable_key_path: None,
                remote_dir: dir.join("config/cloud-sync-local"),
                remote_root: "nyaterm".into(),
                device_id: state.device_id.clone(),
                app_version: env!("CARGO_PKG_VERSION").into(),
                master_password: "synthetic-password".into(),
                enabled: true,
            };
            let snapshot = store.build_sync_snapshot(&options).unwrap();
            let pointer = load_sync_pointer(&store, &options).unwrap().unwrap();
            // Pin a valid preview to real local content and the encrypted remote head.
            let preview: CloudConflictPreview = serde_json::from_value(serde_json::json!({
                "detected_at_ms": 1, "provider": "local_directory",
                "local_payload_hash": snapshot.meta.payload_hash,
                "local_content_hash": snapshot.sync_content_hash().unwrap(),
                "remote_payload_hash": pointer.payload_hash,
                "remote_revision": pointer.revision_id,
                "remote_created_at_ms": pointer.created_at_ms,
                "remote_device_id": pointer.device_id,
                "message": "fixture conflict",
                "observed_revisions": [pointer.revision_id],
                "branches": [{"revision": pointer.revision_id, "device_id": pointer.device_id,
                    "device_name": null, "created_at_ms": pointer.created_at_ms, "readable": true, "differences": []}]
            })).unwrap();
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.apply_auto_cloud_sync_result(
                        &Err(CloudSyncError::Conflict(Box::new(preview))),
                        cx,
                    );
                    if use_cloud {
                        app.prompt_cloud_sync_force_pull(provider_action, window, cx);
                    } else {
                        app.prompt_cloud_sync_force_push(provider_action, window, cx);
                    }
                    assert!(app.cloud_sync.job_running());
                    assert!(!app.settings.snapshot_password_prompt_active());
                });
            });
            assert_eq!(wait_for_history(cx, &app, trigger), "success", "{trigger}");
        }
    }
}

use super::cwd_paths_equal;
use crate::features::NyaTermApp;
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use crate::models::{
    NavItem, PanelOpenMode, SessionLaunchConfig, TransferCwdSyncReason, TransferJobEvent,
    TransferJobKind, TransferJobOutput, TransferJobResult,
};
use crate::test_support::{TestAppCleanup, TestConfigDir};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use nyaterm_core::TransferBrowserViewMode;
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
use nyaterm_transport::{SessionEvent, SftpCwdFollowMode, SshSessionConfig};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const SESSION: &str = "cwd-session";

mod tree_navigation;

fn with_browser(test: impl FnOnce(&Entity<NyaTermApp>, &mut VisualTestContext, &Path)) {
    let root = TestConfigDir::new("nyaterm-cwd-events");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), SESSION);
    cx.update_entity(&app, |app, cx| {
        app.session
            .metadata_mut(SESSION)
            .unwrap()
            .source_connection_id = Some("saved".into());
        let mut summary = app.settings.summary().clone();
        summary.ui_file_explorer_auto_sync_cwd_connection_ids = vec!["saved".into()];
        app.settings.replace_summary(summary);
        // Capture requests after the real navigation bookkeeping, before filesystem/network I/O.
        app.transfer.browser.directory_requests = Some(Vec::new());
        app.session.update_cwd(SESSION, path(root.path(), "shell"));
        app.open_panel(NavItem::Transfers, cx);
    });
    let vcx = session_focus_test_window(&mut cx, &app);
    let id = latest_request(&app, vcx);
    finish(&app, vcx, &id, Ok("initial"));
    test(&app, vcx, root.path());
}

fn path(root: &Path, name: &str) -> String {
    root.join(name).to_string_lossy().into_owned()
}

fn latest_request(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| {
        app.read(cx)
            .transfer
            .browser
            .directory_requests
            .as_ref()
            .unwrap()
            .last()
            .unwrap()
            .0
            .clone()
    })
}

fn request_count(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| {
        app.read(cx)
            .transfer
            .browser
            .directory_requests
            .as_ref()
            .unwrap()
            .len()
    })
}

fn finish(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    id: &str,
    result: Result<&str, &str>,
) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let target = app
                .transfer
                .browser
                .directory_requests
                .as_ref()
                .unwrap()
                .iter()
                .find(|(request_id, _)| request_id == id)
                .unwrap()
                .1
                .display_path
                .clone();
            let kind = app.transfer.transfer_job(id).unwrap().kind.clone();
            let output = result
                .map(|name| {
                    let entries = vec![SftpFileEntry {
                        name: name.into(),
                        path: format!("{target}/{name}"),
                        file_type: SftpFileType::File,
                        size: Some(1),
                        permissions: None,
                        owner: String::new(),
                        group: String::new(),
                        modified_at: None,
                        raw_path_token: None,
                        symlink_target_is_directory: false,
                    }];
                    match kind {
                        TransferJobKind::InitialDirectory => TransferJobOutput::InitialDirectory {
                            path: target,
                            entries,
                        },
                        TransferJobKind::SyncCwd { .. } => TransferJobOutput::CwdSynced {
                            remote_path: target,
                            entries,
                        },
                        TransferJobKind::ListTree { .. } => TransferJobOutput::TreeEntries(entries),
                        _ => TransferJobOutput::Entries(entries),
                    }
                })
                .map_err(str::to_string);
            if app.apply_transfer_event(
                TransferJobResult {
                    id: id.into(),
                    event: TransferJobEvent::Finished(output),
                },
                window,
                cx,
            ) {
                app.defer_transfer_panel_snapshot_flush(cx);
            }
        });
    });
    cx.run_until_parked();
}

fn report(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, cwd: String) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.apply_session_cwd(SESSION, cwd, cx);
        })
    });
}

fn navigate(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, target: String) -> String {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_transfer_browser_directory(target, window, cx);
        })
    });
    latest_request(app, cx)
}

fn assert_directory(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    target: &str,
    name: &str,
) {
    cx.update(|_, cx| {
        let app = app.read(cx);
        let browser = app.transfer.browser_view();
        assert_eq!(browser.path, target);
        assert_eq!(browser.entries[0].name, name);
        assert!(!browser.loading);
        assert!(browser.error.is_none());
    });
}

fn entry(name: &str, path: &str, file_type: SftpFileType) -> SftpFileEntry {
    SftpFileEntry {
        name: name.into(),
        path: path.into(),
        file_type,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        // A symlink keeps its own type; every other entry is already resolved.
        symlink_target_is_directory: file_type == SftpFileType::Symlink,
    }
}

fn seed_tree_listing(app: &mut NyaTermApp, path: &str, entries: Vec<SftpFileEntry>) {
    app.transfer.seed_tree_listing(
        SESSION,
        FileBrowserBackendKind::Local,
        RemoteFilePath::new(path),
        Arc::new(entries),
    );
}

/// Model the server's ancestor listings so a deep target can be revealed.
fn seed_ancestor_tree_listings(app: &mut NyaTermApp, target: &Path) {
    for child in target.ancestors() {
        let Some(parent) = child.parent() else {
            continue;
        };
        let Some(name) = child.file_name() else {
            continue;
        };
        seed_tree_listing(
            app,
            &parent.to_string_lossy(),
            vec![entry(
                &name.to_string_lossy(),
                &child.to_string_lossy(),
                SftpFileType::Directory,
            )],
        );
    }
}

fn enable_tree_mode(app: &mut NyaTermApp) {
    let mut summary = app.settings.summary().clone();
    summary.ui_file_explorer_view_mode = TransferBrowserViewMode::Tree;
    app.settings.replace_summary(summary);
}

/// The row key and expansion flag of the tree row at `target`.
fn tree_row(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, target: &str) -> (String, bool) {
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.transfer
                .tree_presentation(Some(SESSION), true)
                .rows
                .iter()
                .find(|row| row.path.display_path == target)
                .map(|row| (row.key.clone(), row.expanded))
                .unwrap_or_else(|| panic!("missing tree row for {target}"))
        })
    })
}

fn tree_focused(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.transfer
                .tree_presentation(Some(SESSION), true)
                .focused
                .clone()
        })
    })
}

fn browser_path(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| app.read(cx).transfer.browser_view().path.to_string())
}

fn select_tree_row(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, key: String) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_transfer_tree_row(key, false, false, window, cx)
        })
    });
}

fn double_click_tree_row(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.double_click_transfer_tree_row(window, cx))
    });
}

#[test]
fn manual_browsing_survives_time_and_repeated_cwd_reports_then_follows_a_real_change() {
    with_browser(|app, cx, root| {
        let target = path(root, "projects");
        let id = navigate(app, cx, target.clone());
        finish(app, cx, &id, Ok("project"));
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.transfer
                    .select_browser_entry(format!("{target}/project"));
            })
        });
        let count = request_count(app, cx);
        for _ in 0..4 {
            cx.executor().advance_clock(Duration::from_secs(3));
            cx.run_until_parked();
            report(app, cx, path(root, "shell"));
        }
        assert_eq!(request_count(app, cx), count);
        assert_directory(app, cx, &target, "project");
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .selected_remote_paths
                    .len(),
                1
            )
        });
        report(app, cx, path(root, "logs"));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("log"));
        assert_directory(app, cx, &path(root, "logs"), "log");
    });
}

#[test]
fn explicit_sync_returns_to_an_unchanged_shell_cwd_and_does_not_reload_a_loaded_directory() {
    with_browser(|app, cx, root| {
        let id = navigate(app, cx, path(root, "manual"));
        finish(app, cx, &id, Ok("manual"));
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("shell"));
        assert_directory(app, cx, &path(root, "shell"), "shell");
        let count = request_count(app, cx);
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
        assert_eq!(request_count(app, cx), count);
    });
}

#[test]
fn superseded_auto_success_and_failure_cannot_replace_manual_navigation() {
    for result in [Ok("old"), Err("old permission error")] {
        with_browser(|app, cx, root| {
            report(app, cx, path(root, "old-cwd"));
            let old = latest_request(app, cx);
            let manual = navigate(app, cx, path(root, "manual"));
            finish(app, cx, &manual, Ok("manual"));
            finish(app, cx, &old, result);
            assert_directory(app, cx, &path(root, "manual"), "manual");
        });
    }
}

#[test]
fn rapid_cwd_changes_keep_the_last_target_even_when_replies_arrive_out_of_order() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "a"));
        let a = latest_request(app, cx);
        report(app, cx, path(root, "b"));
        let b = latest_request(app, cx);
        assert_ne!(a, b);
        finish(app, cx, &b, Ok("b"));
        finish(app, cx, &a, Ok("a"));
        assert_directory(app, cx, &path(root, "b"), "b");
    });
}

#[test]
fn failed_cwd_navigation_restores_the_previous_directory_and_selection() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.transfer
                    .select_browser_entry(format!("{}/initial", path(root, "shell")));
            })
        });
        report(app, cx, path(root, "denied"));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Err("permission denied"));
        assert_directory(app, cx, &path(root, "shell"), "initial");
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .selected_remote_paths
                    .len(),
                1
            )
        });
    });
}

#[test]
fn disabling_auto_sync_retires_its_request_and_reenabling_aligns_once() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "pending"));
        let old = latest_request(app, cx);
        cx.update(|_, cx| app.update(cx, |app, cx| app.toggle_transfer_browser_auto_sync_cwd(cx)));
        assert_directory(app, cx, &path(root, "shell"), "initial");
        finish(app, cx, &old, Ok("obsolete"));
        assert_directory(app, cx, &path(root, "shell"), "initial");
        cx.update(|_, cx| app.update(cx, |app, cx| app.toggle_transfer_browser_auto_sync_cwd(cx)));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("pending"));
        let count = request_count(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                for _ in 0..3 {
                    app.flush_transfer_panel_snapshot(cx);
                    cx.notify();
                }
            })
        });
        assert_eq!(request_count(app, cx), count);
    });
}

#[test]
fn explicit_sync_is_preserved_when_auto_sync_is_disabled_including_a_promoted_request() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "target"));
        let id = latest_request(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.start_transfer_sync_cwd_job(cx);
                app.toggle_transfer_browser_auto_sync_cwd(cx);
                assert!(matches!(
                    app.transfer.transfer_job(&id).unwrap().kind,
                    TransferJobKind::SyncCwd {
                        reason: TransferCwdSyncReason::Explicit
                    }
                ));
            })
        });
        assert_eq!(latest_request(app, cx), id);
        finish(app, cx, &id, Ok("explicit"));
        assert_directory(app, cx, &path(root, "target"), "explicit");
    });
}

#[test]
fn hidden_browser_only_caches_cwd_and_reopening_aligns_once() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_panel(NavItem::Transfers, cx)));
        let count = request_count(app, cx);
        report(app, cx, path(root, "hidden"));
        assert_eq!(request_count(app, cx), count);
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_panel(NavItem::Transfers, cx)));
        assert_eq!(request_count(app, cx), count + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("visible"));
        assert_directory(app, cx, &path(root, "hidden"), "visible");
    });
}

#[test]
#[cfg(not(target_os = "macos"))]
fn a_closed_compact_drawer_only_caches_cwd_and_opening_it_aligns_once() {
    with_browser(|app, cx, root| {
        cx.simulate_resize(gpui::size(gpui::px(640.), gpui::px(480.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let was_visible = app.transfer_browser_is_visible();
                app.refresh_window_render_inputs(window, cx);
                app.transfer_browser_visibility_changed(was_visible, cx);
                assert!(!app.transfer_browser_is_visible());
            });
        });
        let count = request_count(app, cx);
        report(app, cx, path(root, "drawer"));
        assert_eq!(request_count(app, cx), count);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                match app.panel_side_for_item(NavItem::Transfers).unwrap() {
                    crate::models::PanelSide::Left => app.toggle_mobile_left_drawer(cx),
                    crate::models::PanelSide::Right => app.toggle_mobile_right_drawer(cx),
                }
                assert!(app.transfer_browser_is_visible());
            });
        });
        assert_eq!(request_count(app, cx), count + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("drawer"));
        assert_directory(app, cx, &path(root, "drawer"), "drawer");
    });
}

#[test]
fn editing_defers_the_latest_cwd_and_cancel_follows_it_without_accepting_an_old_reply() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "old"));
        let old = latest_request(app, cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.browser.home_dir = path(root, "home");
                app.begin_transfer_browser_path_edit(window, cx);
                app.apply_transfer_browser_path_input(path(root, "draft"), cx);
            })
        });
        report(app, cx, path(root, "latest"));
        finish(app, cx, &old, Ok("obsolete"));
        cx.update(|_, cx| {
            assert!(app.read(cx).transfer.browser_view().path_editing);
            assert_eq!(
                app.read(cx).transfer.browser_view().path_draft,
                &path(root, "draft")
            );
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.cancel_transfer_browser_path_edit(window, cx)
            })
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("latest"));
        assert_directory(app, cx, &path(root, "latest"), "latest");
    });
}

#[test]
fn submitting_a_manual_path_discards_an_earlier_deferred_cwd() {
    with_browser(|app, cx, root| {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.browser.home_dir = path(root, "home");
                app.begin_transfer_browser_path_edit(window, cx);
            })
        });
        report(app, cx, path(root, "deferred"));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.apply_transfer_browser_path_input(path(root, "manual"), cx);
                app.submit_transfer_browser_path_edit(window, cx);
                assert!(!app.transfer.take_pending_cwd_sync());
            })
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("manual"));
        report(app, cx, path(root, "deferred"));
        assert_directory(app, cx, &path(root, "manual"), "manual");
    });
}

#[test]
fn unknown_cwd_preserves_a_loaded_directory_until_the_first_report() {
    with_browser(|app, cx, root| {
        let count = request_count(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.clear_cwd(SESSION);
                app.start_transfer_sync_cwd_job(cx);
                assert!(!app.transfer.browser.status.is_empty());
            })
        });
        assert_eq!(request_count(app, cx), count);
        assert_directory(app, cx, &path(root, "shell"), "initial");
        report(app, cx, path(root, "reported"));
        assert_eq!(request_count(app, cx), count + 1);
    });
}

#[test]
fn rc_file_mode_uses_the_reported_session_cwd_without_an_exec_probe() {
    with_browser(|app, cx, _| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut config = SshSessionConfig::default();
                config.sftp.cwd_follow_mode = SftpCwdFollowMode::RcFile;
                let metadata = app.session.metadata_mut(SESSION).unwrap();
                metadata.ssh_config = Some(config.clone());
                metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(config));
                app.session.update_cwd(SESSION, "/srv/project".into());
                app.start_transfer_sync_cwd_job(cx);
            })
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("remote"));
        assert_directory(app, cx, "/srv/project", "remote");
    });
}

#[test]
fn cwd_path_comparison_preserves_remote_case_and_ignores_trailing_separators() {
    assert!(cwd_paths_equal(
        FileBrowserBackendKind::Remote,
        "/srv/app/",
        "/srv/app"
    ));
    assert!(!cwd_paths_equal(
        FileBrowserBackendKind::Remote,
        "/srv/App",
        "/srv/app"
    ));
    assert!(!cwd_paths_equal(FileBrowserBackendKind::Remote, "/", ""));
    #[cfg(windows)]
    {
        assert!(cwd_paths_equal(
            FileBrowserBackendKind::Local,
            "C:\\Work\\",
            "c:/work"
        ));
        assert!(cwd_paths_equal(
            FileBrowserBackendKind::Local,
            "C:\\",
            "c:/"
        ));
        assert!(!cwd_paths_equal(
            FileBrowserBackendKind::Local,
            "C:\\",
            "C:"
        ));
    }
}

#[test]
fn session_switch_aligns_to_that_sessions_latest_cwd_and_isolates_old_results() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "a-pending"));
        let old_a = latest_request(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.update_cwd("other", path(root, "b"));
                app.activate_session_id("other", cx);
            })
        });
        let b = latest_request(app, cx);
        finish(app, cx, &b, Ok("b"));
        finish(app, cx, &old_a, Ok("obsolete"));
        assert_directory(app, cx, &path(root, "b"), "b");
        let count = request_count(app, cx);
        report(app, cx, path(root, "a-background"));
        assert_eq!(request_count(app, cx), count);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.activate_session_id(SESSION, cx);
            })
        });
        let a = latest_request(app, cx);
        finish(app, cx, &a, Ok("a"));
        assert_directory(app, cx, &path(root, "a-background"), "a");
    });
}

#[test]
fn reactivation_during_a_directory_request_uses_the_stable_cache_and_retires_the_old_target() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.update_cwd(SESSION, path(root, "pending"));
                app.start_transfer_sync_cwd_job(cx);
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.update_cwd("other", path(root, "other"));
                app.activate_session_id("other", cx);
                let cache = app.transfer.browser_session_cache(SESSION).unwrap();
                assert_eq!(cache.current_path, path(root, "shell"));
                assert_eq!(cache.entries[0].name, "initial");
                // The terminal changed back while its browser was in the background.
                app.apply_session_cwd(SESSION, path(root, "shell"), cx);
            });
        });
        let count = request_count(app, cx);
        cx.update(|_, cx| app.update(cx, |app, cx| app.activate_session_id(SESSION, cx)));
        assert_eq!(request_count(app, cx), count);
        assert_directory(app, cx, &path(root, "shell"), "initial");
        let old = cx.update(|_, cx| {
            app.read(cx)
                .transfer
                .browser
                .directory_requests
                .as_ref()
                .unwrap()[1]
                .0
                .clone()
        });
        finish(app, cx, &old, Ok("obsolete"));
        assert_directory(app, cx, &path(root, "shell"), "initial");
    });
}

#[test]
fn a_slow_explicit_request_in_another_session_does_not_block_or_overwrite_the_active_browser() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.update_cwd(SESSION, path(root, "a-explicit"));
                app.start_transfer_sync_cwd_job(cx);
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.update_cwd("other", path(root, "b"));
                app.activate_session_id("other", cx);
            })
        });
        let (a, b) = cx.update(|_, cx| {
            let requests = app
                .read(cx)
                .transfer
                .browser
                .directory_requests
                .as_ref()
                .unwrap();
            (requests[1].0.clone(), requests[2].0.clone())
        });
        finish(app, cx, &b, Ok("b"));
        finish(app, cx, &a, Ok("a"));
        assert_directory(app, cx, &path(root, "b"), "b");
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer
                    .browser_session_cache(SESSION)
                    .unwrap()
                    .current_path,
                path(root, "a-explicit"),
            )
        });
    });
}

#[test]
fn initial_listing_loses_ownership_when_the_first_reliable_cwd_arrives() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.open_panel(NavItem::Transfers, cx);
                app.session.clear_cwd(SESSION);
                app.transfer.remove_browser_session_cache(SESSION);
                app.reset_transfer_browser_for_active_session();
                app.open_panel(NavItem::Transfers, cx);
            })
        });
        let initial = latest_request(app, cx);
        report(app, cx, path(root, "reported"));
        let reported = latest_request(app, cx);
        assert_ne!(initial, reported);
        finish(app, cx, &reported, Ok("reported"));
        finish(app, cx, &initial, Ok("obsolete-home"));
        assert_directory(app, cx, &path(root, "reported"), "reported");
    });
}

#[test]
fn reconnect_discards_old_cwd_and_requests_but_keeps_the_stable_browser_cache() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "old"));
        let old = latest_request(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session
                    .register_session_metadata("replacement", metadata);
                app.migrate_reconnected_session_state(SESSION, "replacement", cx);
                assert!(app.session.cwd("replacement").is_none());
                assert!(app.session.cwd(SESSION).is_none());
                assert!(app.transfer.has_browser_session_cache("replacement"));
                assert!(
                    !app.transfer
                        .browser_navigation_job_running_for_session("replacement")
                );
            })
        });
        finish(app, cx, &old, Ok("obsolete"));
        assert_directory(app, cx, &path(root, "shell"), "initial");
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.apply_session_cwd("replacement", path(root, "new"), cx);
            })
        });
        let new = latest_request(app, cx);
        finish(app, cx, &new, Ok("new"));
        assert_directory(app, cx, &path(root, "new"), "new");
    });
}

#[test]
fn disconnecting_an_inactive_session_cannot_restore_its_snapshot_over_the_active_browser() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.update_cwd(SESSION, path(root, "a-explicit"));
                app.start_transfer_sync_cwd_job(cx);
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.update_cwd("other", path(root, "b"));
                app.activate_session_id("other", cx);
            })
        });
        let b = latest_request(app, cx);
        finish(app, cx, &b, Ok("b"));
        cx.update(|_, cx| app.update(cx, |app, cx| app.mark_session_disconnected(SESSION, cx)));
        assert_directory(app, cx, &path(root, "b"), "b");
    });
}

#[test]
fn cwd_events_from_the_session_bridge_load_the_directory_without_a_poll() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.start_runtime_data_plane_drain(cx);
                app.session
                    .push_event_bridge_ui_event_for_test(SessionEvent::CwdChanged {
                        session_id: SESSION.into(),
                        cwd: path(root, "event"),
                    });
            })
        });
        cx.run_until_parked();
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("event"));
        assert_directory(app, cx, &path(root, "event"), "event");
    });
}

#[test]
fn terminal_osc_cwd_output_uses_the_same_directory_navigation_and_updates_the_panel() {
    with_browser(|app, cx, _| {
        let count = request_count(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let config = SshSessionConfig::default();
                let metadata = app.session.metadata_mut(SESSION).unwrap();
                metadata.ssh_config = Some(config.clone());
                metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(config));
                let mut screen = nyaterm_terminal::TerminalScreen::new(40, 3);
                screen.advance(b"\x1b]7;file://localhost/srv/from-osc\x07");
                app.apply_terminal_effects(SESSION, screen.take_effects(), false, cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(request_count(app, cx), count + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("osc"));
        assert_directory(app, cx, "/srv/from-osc", "osc");
    });
}

#[test]
fn closing_the_panel_retires_inflight_auto_navigation_and_discards_an_editing_intent() {
    with_browser(|app, cx, root| {
        report(app, cx, path(root, "old"));
        let old = latest_request(app, cx);
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_panel(NavItem::Transfers, cx)));
        finish(app, cx, &old, Err("obsolete failure"));
        assert_directory(app, cx, &path(root, "shell"), "initial");
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_panel(NavItem::Transfers, cx)));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("old"));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.browser.home_dir = path(root, "home");
                app.begin_transfer_browser_path_edit(window, cx);
            })
        });
        report(app, cx, path(root, "deferred"));
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.open_panel(NavItem::Transfers, cx);
                assert!(!app.transfer.take_pending_cwd_sync());
                assert!(!app.transfer.browser.path_editing);
            })
        });
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_panel(NavItem::Transfers, cx)));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("deferred"));
        assert_directory(app, cx, &path(root, "deferred"), "deferred");
    });
}

#[test]
fn disabling_auto_sync_does_not_cancel_user_directory_navigation() {
    with_browser(|app, cx, root| {
        let id = navigate(app, cx, path(root, "manual"));
        cx.update(|_, cx| app.update(cx, |app, cx| app.toggle_transfer_browser_auto_sync_cwd(cx)));
        finish(app, cx, &id, Ok("manual"));
        assert_directory(app, cx, &path(root, "manual"), "manual");
    });
}

#[test]
fn changing_only_windows_path_spelling_does_not_interrupt_manual_browsing() {
    #[cfg(windows)]
    with_browser(|app, cx, root| {
        let manual = path(root, "manual");
        let id = navigate(app, cx, manual.clone());
        finish(app, cx, &id, Ok("manual"));
        let count = request_count(app, cx);
        report(
            app,
            cx,
            format!("{}/", path(root, "shell").replace('\\', "/").to_uppercase()),
        );
        assert_eq!(request_count(app, cx), count);
        assert_directory(app, cx, &manual, "manual");
    });
}

#[test]
fn floating_and_stacked_browser_openings_align_to_cwd_once() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_panel_open_mode(PanelOpenMode::Floating, cx);
                app.session.update_cwd(SESSION, path(root, "floating"));
                app.open_panel(NavItem::Transfers, cx);
            })
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("floating"));
        assert_directory(app, cx, &path(root, "floating"), "floating");
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_panel_open_mode(PanelOpenMode::Docked, cx);
                app.toggle_panel_multi_open(cx);
                app.session.update_cwd(SESSION, path(root, "stacked"));
                app.ensure_panel_open(NavItem::Transfers, cx);
            })
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("stacked"));
        assert_directory(app, cx, &path(root, "stacked"), "stacked");
    });
}

#[test]
fn tree_mode_follows_cwd_and_seeds_the_target_directory_listing() {
    with_browser(|app, cx, root| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut summary = app.settings.summary().clone();
                summary.ui_file_explorer_view_mode = TransferBrowserViewMode::Tree;
                app.settings.replace_summary(summary);
                // Model the server's ancestor listings so the selected directory can be revealed.
                let target = root.join("tree");
                for child in target.ancestors() {
                    let Some(parent) = child.parent() else {
                        continue;
                    };
                    let Some(name) = child.file_name() else {
                        continue;
                    };
                    app.transfer.seed_tree_listing(
                        SESSION,
                        FileBrowserBackendKind::Local,
                        RemoteFilePath::new(parent.to_string_lossy().into_owned()),
                        Arc::new(vec![SftpFileEntry {
                            name: name.to_string_lossy().into_owned(),
                            path: child.to_string_lossy().into_owned(),
                            file_type: SftpFileType::Directory,
                            size: None,
                            permissions: None,
                            owner: String::new(),
                            group: String::new(),
                            modified_at: None,
                            raw_path_token: None,
                            symlink_target_is_directory: false,
                        }]),
                    );
                }
                // Following must also reveal a new target after tree initialization.
                app.reveal_transfer_tree_current_path(cx);
                assert!(app.transfer.tree_is_initialized(SESSION));
            })
        });
        report(app, cx, path(root, "tree"));
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("tree-child"));
        assert_directory(app, cx, &path(root, "tree"), "tree-child");
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                assert!(app.transfer.tree_is_initialized(SESSION));
                let presentation = app.transfer.tree_presentation(Some(SESSION), true);
                assert!(
                    presentation
                        .rows
                        .iter()
                        .any(|row| row.path.display_path == path(root, "tree")
                            && presentation.focused.as_ref() == Some(&row.key))
                );
            })
        });
    });
}

#[test]
fn double_clicking_a_cached_tree_directory_toggles_in_place_and_only_links_navigate() {
    with_browser(|app, cx, root| {
        let target = root.join("tree").to_string_lossy().into_owned();
        let child = root
            .join("tree")
            .join("child")
            .to_string_lossy()
            .into_owned();
        let link = root.join("tree-link").to_string_lossy().into_owned();
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                enable_tree_mode(app);
                seed_ancestor_tree_listings(app, Path::new(&target));
                seed_tree_listing(
                    app,
                    &root.to_string_lossy(),
                    vec![
                        entry("tree", &target, SftpFileType::Directory),
                        entry("tree-link", &link, SftpFileType::Symlink),
                    ],
                );
                // Every listing on the way to the target is already cached.
                seed_tree_listing(
                    app,
                    &target,
                    vec![entry("child", &child, SftpFileType::Directory)],
                );
                // Reveal only the parent so the cached target starts collapsed.
                app.transfer.reveal_tree_path(
                    SESSION,
                    FileBrowserBackendKind::Local,
                    RemoteFilePath::new(root.to_string_lossy().into_owned()),
                );
            })
        });
        let baseline = request_count(app, cx);
        let path_before = browser_path(app, cx);
        let (tree_key, expanded) = tree_row(app, cx, &target);
        assert!(!expanded);
        select_tree_row(app, cx, tree_key);
        double_click_tree_row(app, cx);
        assert!(tree_row(app, cx, &target).1);
        double_click_tree_row(app, cx);
        assert!(!tree_row(app, cx, &target).1);
        double_click_tree_row(app, cx);
        assert!(tree_row(app, cx, &target).1);
        // Cached expand/collapse neither navigates nor refetches.
        assert_eq!(request_count(app, cx), baseline);
        assert_eq!(browser_path(app, cx), path_before);

        // A directory symlink is not expandable, so double click still navigates.
        let (link_key, _) = tree_row(app, cx, &link);
        select_tree_row(app, cx, link_key);
        double_click_tree_row(app, cx);
        assert_eq!(request_count(app, cx), baseline + 1);
        cx.update(|_, cx| {
            let requests = app
                .read(cx)
                .transfer
                .browser
                .directory_requests
                .clone()
                .unwrap();
            let (id, requested) = requests.last().unwrap();
            assert_eq!(requested.display_path, link);
            assert!(matches!(
                app.read(cx).transfer.transfer_job(id).unwrap().kind,
                TransferJobKind::ListDir { .. }
            ));
        });
        assert_eq!(browser_path(app, cx), link);
    });
}

#[test]
fn explicit_tree_sync_reveals_a_loaded_cwd_without_refetching() {
    with_browser(|app, cx, root| {
        let target = path(root, "shell");
        let sibling = path(root, "manual");
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                enable_tree_mode(app);
                seed_ancestor_tree_listings(app, Path::new(&target));
                seed_tree_listing(
                    app,
                    &root.to_string_lossy(),
                    vec![
                        entry("shell", &target, SftpFileType::Directory),
                        entry("manual", &sibling, SftpFileType::Directory),
                    ],
                );
                app.reveal_transfer_tree_current_path(cx);
            });
        });
        let (target_key, _) = tree_row(app, cx, &target);
        let (sibling_key, _) = tree_row(app, cx, &sibling);
        select_tree_row(app, cx, sibling_key.clone());
        let count = request_count(app, cx);
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.sync_transfer_browser_to_cwd(TransferCwdSyncReason::Automatic, cx);
            });
        });
        assert_eq!(tree_focused(app, cx).as_deref(), Some(sibling_key.as_str()));
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
        assert_eq!(tree_focused(app, cx).as_deref(), Some(target_key.as_str()));
        assert_eq!(request_count(app, cx), count);
        assert_eq!(browser_path(app, cx), target);
    });
}

#[test]
fn tree_mode_reveals_the_cwd_target_before_its_listing_and_keeps_a_manual_selection() {
    with_browser(|app, cx, root| {
        let target = root.join("instant").to_string_lossy().into_owned();
        let sibling = root.join("manual").to_string_lossy().into_owned();
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                enable_tree_mode(app);
                seed_ancestor_tree_listings(app, Path::new(&target));
                seed_tree_listing(
                    app,
                    &root.to_string_lossy(),
                    vec![
                        entry("instant", &target, SftpFileType::Directory),
                        entry("manual", &sibling, SftpFileType::Directory),
                    ],
                );
                // The target itself stays uncached: the pending navigation supplies it.
                app.transfer.browser.directory_requests = Some(Vec::new());
            })
        });
        report(app, cx, target.clone());
        // A concurrent completion can revisit missing nodes before navigation finishes.
        cx.update(|_, cx| {
            app.update(cx, |app, cx| app.request_missing_expanded_tree_listings(cx));
        });
        // The navigation owns the target listing; its tree row must not be requested twice.
        let sync_id = cx.update(|_, cx| {
            let app = app.read(cx);
            let requests = app.transfer.browser.directory_requests.as_ref().unwrap();
            let mut target_requests = requests
                .iter()
                .filter(|(_, requested)| requested.display_path == target);
            let (id, _) = target_requests
                .next()
                .expect("the cwd target is requested once by navigation");
            assert!(
                target_requests.next().is_none(),
                "the cwd target must not also be listed as a tree row"
            );
            assert!(matches!(
                app.transfer.transfer_job(id).unwrap().kind,
                TransferJobKind::SyncCwd { .. }
            ));
            // Any other request may only be a still-missing ancestor listing.
            for (other_id, requested) in requests {
                if requested.display_path == target {
                    continue;
                }
                assert!(matches!(
                    app.transfer.transfer_job(other_id).unwrap().kind,
                    TransferJobKind::ListTree { .. }
                ));
            }
            id.clone()
        });
        // The cached target is already focused and expanded before its listing returns.
        let (target_key, expanded) = tree_row(app, cx, &target);
        assert!(expanded);
        assert_eq!(tree_focused(app, cx).as_deref(), Some(target_key.as_str()));
        // A manual tree selection made while the listing is pending must survive it.
        let (sibling_key, _) = tree_row(app, cx, &sibling);
        select_tree_row(app, cx, sibling_key.clone());
        let count = request_count(app, cx);
        finish(app, cx, &sync_id, Ok("child"));
        assert_directory(app, cx, &target, "child");
        assert_eq!(request_count(app, cx), count);
        assert_eq!(tree_focused(app, cx).as_deref(), Some(sibling_key.as_str()));
        assert!(tree_row(app, cx, &target).1);
    });
}

#[cfg(windows)]
#[test]
fn posix_local_shell_cwds_are_translated_for_the_windows_file_browser() {
    with_browser(|app, cx, _root| {
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let SessionLaunchConfig::Local(config) =
                    &mut app.session.metadata_mut(SESSION).unwrap().launch_config
                else {
                    panic!("local session");
                };
                config.shell_path = Some("wsl.exe".into());
            })
        });
        report(app, cx, "/mnt/d/Codes/RustProjects".into());
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).session.cwd(SESSION),
                Some("/mnt/d/Codes/RustProjects")
            );
        });
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("nyaterm"));
        assert_directory(app, cx, r"D:\Codes\RustProjects", "nyaterm");

        let before = request_count(app, cx);
        report(app, cx, "/d/Projects".into());
        assert_eq!(request_count(app, cx), before);
        cx.update(|_, cx| {
            assert_eq!(app.read(cx).session.cwd(SESSION), Some("/d/Projects"));
        });
        report(app, cx, "/home/debian/notes".into());
        assert_eq!(request_count(app, cx), before);
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx).session.cwd(SESSION),
                Some("/home/debian/notes")
            );
        });
    });
}

#[cfg(windows)]
#[test]
fn wsl_internal_cwds_navigate_the_browser_through_wsl_localhost() {
    with_browser(|app, cx, _root| {
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let SessionLaunchConfig::Local(config) =
                    &mut app.session.metadata_mut(SESSION).unwrap().launch_config
                else {
                    panic!("local session");
                };
                config.shell_path = Some("wsl.exe".into());
                config.shell_args = vec!["-d".into(), "Ubuntu".into()];
            })
        });
        report(app, cx, "/mnt/d/Projects".into());
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("nyaterm"));
        assert_directory(app, cx, r"D:\Projects", "nyaterm");

        report(app, cx, "/home/user/notes".into());
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("nyaterm"));
        assert_directory(
            app,
            cx,
            r"\\wsl.localhost\Ubuntu\home\user\notes",
            "nyaterm",
        );

        // The WSL automount root has no drive-letter form and lands on the 9P
        // namespace too.
        report(app, cx, "/mnt".into());
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("nyaterm"));
        assert_directory(app, cx, r"\\wsl.localhost\Ubuntu\mnt", "nyaterm");

        // A cwd Windows cannot represent (`:` in a component) must not send the
        // browser to a POSIX path it would resolve against the current drive.
        let before = request_count(app, cx);
        report(app, cx, "/home/user/keep:notes".into());
        assert_eq!(request_count(app, cx), before);

        // Without a distribution selection there is nothing to navigate to and
        // the browser keeps its current directory.
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let SessionLaunchConfig::Local(config) =
                    &mut app.session.metadata_mut(SESSION).unwrap().launch_config
                else {
                    panic!("local session");
                };
                config.shell_args = Vec::new();
            })
        });
        let before = request_count(app, cx);
        report(app, cx, "/home/user/notes".into());
        assert_eq!(request_count(app, cx), before);
    });
}

#[cfg(windows)]
fn with_wsl_browser(test: impl FnOnce(&Entity<NyaTermApp>, &mut VisualTestContext)) {
    with_browser(|app, cx, _| {
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let SessionLaunchConfig::Local(config) =
                    &mut app.session.metadata_mut(SESSION).unwrap().launch_config
                else {
                    panic!("local session");
                };
                config.shell_path = Some("wsl.exe".into());
                config.shell_args = vec!["-d".into(), "Ubuntu".into()];
            })
        });
        test(app, cx);
    });
}

#[cfg(windows)]
#[test]
fn wsl_case_only_cwd_changes_follow_and_explicit_sync_realigns_the_browser() {
    with_wsl_browser(|app, cx| {
        report(app, cx, "/home/user/Project".into());
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("upper"));
        let before = request_count(app, cx);

        report(app, cx, "/home/user/project".into());
        assert_eq!(request_count(app, cx), before + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("lower"));
        assert_directory(
            app,
            cx,
            r"\\wsl.localhost\Ubuntu\home\user\project",
            "lower",
        );

        // Leave the browser in the case-distinct sibling while the shell stays put.
        let id = navigate(app, cx, r"\\wsl.localhost\Ubuntu\home\user\Project".into());
        finish(app, cx, &id, Ok("manual"));
        let before = request_count(app, cx);
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
        assert_eq!(request_count(app, cx), before + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("lower"));
        assert_directory(
            app,
            cx,
            r"\\wsl.localhost\Ubuntu\home\user\project",
            "lower",
        );

        let before = request_count(app, cx);
        report(app, cx, "/home/user/project/".into());
        cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
        assert_eq!(request_count(app, cx), before);
    });
}

#[cfg(windows)]
#[test]
fn invalid_wsl_cwds_do_not_navigate_or_trim_into_another_directory_and_following_recovers() {
    with_wsl_browser(|app, cx| {
        report(app, cx, "/home/user/notes".into());
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("notes"));
        let before = request_count(app, cx);
        for cwd in [
            "/home/user/notes ",
            "/home/user/notes.",
            "/home/user/a:b",
            "/home/user/NUL",
            "/home/user/a?b",
            "/mnt/c/notes ",
            "/mnt/c/a:b",
            "/mnt/c/../notes",
        ] {
            report(app, cx, cwd.into());
            cx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_sync_cwd_job(cx)));
            assert_eq!(request_count(app, cx), before, "{cwd:?}");
            assert_directory(app, cx, r"\\wsl.localhost\Ubuntu\home\user\notes", "notes");
            cx.update(|_, cx| assert_eq!(app.read(cx).session.cwd(SESSION), Some(cwd)));
        }
        report(app, cx, "/home/user/recovered".into());
        assert_eq!(request_count(app, cx), before + 1);
        let id = latest_request(app, cx);
        finish(app, cx, &id, Ok("recovered"));
        assert_directory(
            app,
            cx,
            r"\\wsl.localhost\Ubuntu\home\user\recovered",
            "recovered",
        );
    });
}

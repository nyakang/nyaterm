use super::{
    SESSION, assert_directory, enable_tree_mode, entry, finish, latest_request, navigate, path,
    report, request_count, seed_ancestor_tree_listings, seed_tree_listing, select_tree_row,
    tree_focused, tree_row, with_browser,
};
use crate::features::NyaTermApp;
use crate::models::{
    NavItem, TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult,
    TransferJobState, TransferJobStatus,
};
use gpui::{Entity, VisualTestContext};
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileType};
use std::path::Path;

fn prepare_tree(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    root: &Path,
) -> (String, String, String) {
    let shell = path(root, "shell");
    let target = path(root, "target");
    let other = path(root, "other");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            enable_tree_mode(app);
            seed_ancestor_tree_listings(app, Path::new(&shell));
            seed_tree_listing(
                app,
                &root.to_string_lossy(),
                vec![
                    entry("shell", &shell, SftpFileType::Directory),
                    entry("target", &target, SftpFileType::Directory),
                    entry("other", &other, SftpFileType::Directory),
                ],
            );
            let entries = app.transfer.browser_view().entries.as_ref().clone();
            seed_tree_listing(app, &shell, entries);
            app.reveal_transfer_tree_current_path(cx);
        })
    });
    tree_row(app, cx, &shell);
    (shell, target, other)
}

fn start_navigation(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    target: &str,
    kind: u8,
) -> String {
    match kind {
        0 => {
            navigate(app, cx, target.into());
        }
        1 => report(app, cx, target.into()),
        2 => cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.update_cwd(SESSION, target.into());
                app.start_transfer_sync_cwd_job(cx);
            })
        }),
        _ => unreachable!(),
    }
    cx.update(|_, cx| app.read(cx).transfer.browser.navigation_jobs[SESSION].clone())
}

#[test]
fn failed_automatic_and_explicit_cwd_navigation_restore_tree_focus_and_expansion() {
    for kind in [1, 2] {
        with_browser(|app, cx, root| {
            let (shell, target, _) = prepare_tree(app, cx, root);
            let shell_key = tree_row(app, cx, &shell).0;
            let id = start_navigation(app, cx, &target, kind);
            assert_eq!(
                tree_focused(app, cx).as_deref(),
                Some(tree_row(app, cx, &target).0.as_str())
            );
            assert!(tree_row(app, cx, &target).1);
            finish(app, cx, &id, Err("permission denied"));
            assert_directory(app, cx, &shell, "initial");
            assert_eq!(tree_focused(app, cx), Some(shell_key));
            assert!(!tree_row(app, cx, &target).1);
        });
    }
}

#[test]
fn ordinary_and_cwd_navigation_success_and_failure_keep_later_manual_selection() {
    for kind in [0, 1, 2] {
        for result in [Ok("child"), Err("permission denied")] {
            with_browser(|app, cx, root| {
                let (shell, target, other) = prepare_tree(app, cx, root);
                let id = start_navigation(app, cx, &target, kind);
                let other_key = tree_row(app, cx, &other).0;
                select_tree_row(app, cx, other_key.clone());
                finish(app, cx, &id, result);
                if result.is_ok() {
                    assert_directory(app, cx, &target, "child");
                } else {
                    assert_directory(app, cx, &shell, "initial");
                }
                assert_eq!(tree_focused(app, cx), Some(other_key.clone()));
                cx.update(|_, cx| {
                    app.update(cx, |app, _| {
                        assert!(
                            app.transfer
                                .tree_presentation(Some(SESSION), true)
                                .selected
                                .contains(&other_key)
                        );
                    })
                });
            });
        }
    }
}

#[test]
fn refresh_and_home_metadata_completion_preserve_an_existing_tree_selection() {
    with_browser(|app, cx, root| {
        let (shell, _, other) = prepare_tree(app, cx, root);
        let other_key = tree_row(app, cx, &other).0;
        select_tree_row(app, cx, other_key.clone());
        let refresh = navigate(app, cx, shell.clone());
        finish(app, cx, &refresh, Ok("refreshed"));
        assert_eq!(tree_focused(app, cx), Some(other_key.clone()));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.enqueue_transfer_job(TransferJobState {
                    id: "home-metadata".into(),
                    session_id: Some(SESSION.into()),
                    kind: TransferJobKind::ResolveHome,
                    status: TransferJobStatus::Running,
                    detail: String::new(),
                    created_at_ms: 0,
                    display_name: String::new(),
                    entries: Vec::new(),
                    summary: None,
                    progress: None,
                    control: None,
                    speed: Default::default(),
                });
                app.apply_transfer_event(
                    TransferJobResult {
                        id: "home-metadata".into(),
                        event: TransferJobEvent::Finished(Ok(TransferJobOutput::HomeDir(path(
                            root, "home",
                        )))),
                    },
                    window,
                    cx,
                );
            })
        });
        assert_eq!(tree_focused(app, cx), Some(other_key));
        assert_directory(app, cx, &shell, "refreshed");
    });
}

#[test]
fn initial_directory_completion_and_failure_keep_later_manual_tree_selection() {
    for success in [true, false] {
        with_browser(|app, cx, root| {
            let (shell, _, other) = prepare_tree(app, cx, root);
            cx.update(|_, cx| {
                app.update(cx, |app, cx| app.start_transfer_initial_directory_job(cx))
            });
            let id = latest_request(app, cx);
            let other_key = tree_row(app, cx, &other).0;
            select_tree_row(app, cx, other_key.clone());
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let output = if success {
                        Ok(TransferJobOutput::InitialDirectory {
                            path: shell.clone(),
                            entries: app.transfer.browser.pending_navigations[&id]
                                .entries
                                .as_ref()
                                .clone(),
                        })
                    } else {
                        Err("permission denied".into())
                    };
                    app.apply_transfer_event(
                        TransferJobResult {
                            id: id.clone(),
                            event: TransferJobEvent::Finished(output),
                        },
                        window,
                        cx,
                    );
                })
            });
            assert_eq!(tree_focused(app, cx), Some(other_key));
            assert_directory(app, cx, &shell, "initial");
        });
    }
}

#[test]
fn panel_close_follow_disable_and_disconnect_roll_back_optimistic_tree_position() {
    for action in 0..3 {
        for manual in [false, true] {
            with_browser(|app, cx, root| {
                let (shell, target, other) = prepare_tree(app, cx, root);
                let original = tree_row(app, cx, &shell).0;
                let id = start_navigation(app, cx, &target, 1);
                let expected = if manual {
                    let other_key = tree_row(app, cx, &other).0;
                    select_tree_row(app, cx, other_key.clone());
                    other_key
                } else {
                    original
                };
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| match action {
                        0 => app.open_panel(NavItem::Transfers, cx),
                        1 => app.toggle_transfer_browser_auto_sync_cwd(cx),
                        2 => app.mark_session_disconnected(SESSION, cx),
                        _ => unreachable!(),
                    })
                });
                assert_directory(app, cx, &shell, "initial");
                assert_eq!(tree_focused(app, cx), Some(expected.clone()));
                finish(app, cx, &id, Err("obsolete error"));
                assert_eq!(tree_focused(app, cx), Some(expected));
            });
        }
    }
}

#[test]
fn latest_navigation_failure_restores_stable_tree_after_supersession() {
    with_browser(|app, cx, root| {
        let (shell, target, other) = prepare_tree(app, cx, root);
        let original = tree_row(app, cx, &shell).0;
        let old = start_navigation(app, cx, &target, 1);
        let latest = start_navigation(app, cx, &other, 1);
        finish(app, cx, &old, Err("old error"));
        assert_eq!(tree_focused(app, cx), Some(tree_row(app, cx, &other).0));
        finish(app, cx, &latest, Err("latest error"));
        assert_directory(app, cx, &shell, "initial");
        assert_eq!(tree_focused(app, cx), Some(original));
    });
}

#[test]
fn explicit_sync_reusing_ordinary_navigation_rolls_back_to_the_latest_manual_tree_state() {
    with_browser(|app, cx, root| {
        let (shell, target, other) = prepare_tree(app, cx, root);
        let navigation = start_navigation(app, cx, &target, 0);
        let other_key = tree_row(app, cx, &other).0;
        select_tree_row(app, cx, other_key.clone());
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.session.update_cwd(SESSION, target.clone());
                app.start_transfer_sync_cwd_job(cx);
                assert_eq!(app.transfer.browser.navigation_jobs[SESSION], navigation);
            })
        });
        assert_eq!(tree_focused(app, cx), Some(tree_row(app, cx, &target).0));
        finish(app, cx, &navigation, Err("permission denied"));
        assert_directory(app, cx, &shell, "initial");
        assert_eq!(tree_focused(app, cx), Some(other_key));
    });
}

#[test]
fn rapid_deep_cwd_changes_hold_one_tree_job_and_drop_obsolete_ancestor_requests() {
    with_browser(|app, cx, root| {
        prepare_tree(app, cx, root);
        let mut latest = String::new();
        let mut target = String::new();
        for index in 0..100 {
            target = path(root, &format!("branch-{index}/a/b/c"));
            latest = start_navigation(app, cx, &target, 1);
            cx.update(|_, cx| {
                let app = app.read(cx);
                let tree_requests = app
                    .transfer
                    .browser
                    .directory_requests
                    .as_ref()
                    .unwrap()
                    .iter()
                    .filter(|(id, _)| {
                        app.transfer
                            .transfer_job(id)
                            .is_some_and(|job| matches!(job.kind, TransferJobKind::ListTree { .. }))
                    })
                    .count();
                assert_eq!(tree_requests, 1);
            });
        }
        let old_tree = cx.update(|_, cx| {
            let app = app.read(cx);
            app.transfer
                .browser
                .directory_requests
                .as_ref()
                .unwrap()
                .iter()
                .find(|(id, _)| {
                    app.transfer
                        .transfer_job(id)
                        .is_some_and(|job| matches!(job.kind, TransferJobKind::ListTree { .. }))
                })
                .unwrap()
                .0
                .clone()
        });
        finish(app, cx, &latest, Ok("latest"));
        assert_directory(app, cx, &target, "latest");
        finish(app, cx, &old_tree, Ok("obsolete child"));
        cx.update(|_, cx| {
            let app = app.read(cx);
            let (id, requested) = app
                .transfer
                .browser
                .directory_requests
                .as_ref()
                .unwrap()
                .last()
                .unwrap();
            assert!(matches!(
                app.transfer.transfer_job(id).unwrap().kind,
                TransferJobKind::ListTree { .. }
            ));
            assert_eq!(requested.display_path, path(root, "branch-99"));
        });
        assert_directory(app, cx, &target, "latest");
    });
}

#[test]
fn home_metadata_success_and_failure_do_not_settle_an_inflight_directory_navigation() {
    for success in [true, false] {
        with_browser(|app, cx, root| {
            let (_, target, other) = prepare_tree(app, cx, root);
            let navigation = start_navigation(app, cx, &target, 0);
            let other_key = tree_row(app, cx, &other).0;
            select_tree_row(app, cx, other_key.clone());
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.transfer.browser.home_dir_pending = true;
                    app.transfer.enqueue_transfer_job(TransferJobState {
                        id: "home-metadata".into(),
                        session_id: Some(SESSION.into()),
                        kind: TransferJobKind::ResolveHome,
                        status: TransferJobStatus::Running,
                        detail: String::new(),
                        created_at_ms: 0,
                        display_name: String::new(),
                        entries: Vec::new(),
                        summary: None,
                        progress: None,
                        control: None,
                        speed: Default::default(),
                    });
                    let output = if success {
                        Ok(TransferJobOutput::HomeDir(path(root, "home")))
                    } else {
                        Err("home lookup failed".into())
                    };
                    app.apply_transfer_event(
                        TransferJobResult {
                            id: "home-metadata".into(),
                            event: TransferJobEvent::Finished(output),
                        },
                        window,
                        cx,
                    );
                    let browser = app.transfer.browser_view();
                    assert_eq!(browser.path, &target);
                    assert!(browser.loading);
                    assert!(browser.error.is_none());
                    assert!(!browser.home_dir_pending);
                })
            });
            assert_eq!(tree_focused(app, cx), Some(other_key.clone()));
            finish(app, cx, &navigation, Ok("child"));
            assert_directory(app, cx, &target, "child");
            assert_eq!(tree_focused(app, cx), Some(other_key));
        });
    }
}

#[test]
fn tree_worker_terminal_errors_release_the_slot_and_allow_explicit_retry() {
    for error in [
        nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED,
        "blocking job queue is full",
        "transfer worker panicked",
    ] {
        with_browser(|app, cx, root| {
            let (_, target, _) = prepare_tree(app, cx, root);
            let key = tree_row(app, cx, &target).0;
            cx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.expand_transfer_tree_row(&key, Some(true), cx)
                })
            });
            let failed = latest_request(app, cx);
            finish(app, cx, &failed, Err(error));
            cx.update(|_, cx| {
                app.update(cx, |app, _| {
                    let tree = app.transfer.tree_presentation(Some(SESSION), true);
                    let row = tree.rows.iter().find(|row| row.key == key).unwrap();
                    assert!(!row.loading);
                    assert!(row.error.is_some());
                })
            });
            cx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.expand_transfer_tree_row(&key, Some(true), cx)
                })
            });
            let retry = latest_request(app, cx);
            assert_ne!(retry, failed);
            finish(app, cx, &retry, Ok("child"));
            cx.update(|_, cx| {
                app.update(cx, |app, _| {
                    let tree = app.transfer.tree_presentation(Some(SESSION), true);
                    let row = tree.rows.iter().find(|row| row.key == key).unwrap();
                    assert!(!row.loading);
                    assert!(row.error.is_none());
                })
            });
        });
    }
}

#[test]
fn reopening_a_cached_panel_or_session_resumes_tree_demand_without_repositioning() {
    for switch_session in [false, true] {
        for manual_expansion in [false, true] {
            with_browser(|app, cx, root| {
                let (shell, target, other) = prepare_tree(app, cx, root);
                let target_key = tree_row(app, cx, &target).0;
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        if manual_expansion {
                            app.expand_transfer_tree_row(&target_key, Some(true), cx);
                        } else {
                            app.transfer.reveal_tree_path(
                                SESSION,
                                FileBrowserBackendKind::Local,
                                RemoteFilePath::new(path(root, "target/child")),
                            );
                            app.request_missing_expanded_tree_listings(cx);
                        }
                    })
                });
                let cancelled = latest_request(app, cx);
                let other_key = tree_row(app, cx, &other).0;
                select_tree_row(app, cx, other_key.clone());
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        if switch_session {
                            let metadata = app.session.metadata(SESSION).unwrap().clone();
                            app.session.register_session_metadata("other", metadata);
                            app.activate_session_id("other", cx);
                        } else {
                            app.open_panel(NavItem::Transfers, cx);
                        }
                    })
                });
                let count = request_count(app, cx);
                finish(app, cx, &cancelled, Ok("obsolete"));
                assert_eq!(request_count(app, cx), count);
                cx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        if switch_session {
                            app.activate_session_id(SESSION, cx);
                        } else {
                            app.open_panel(NavItem::Transfers, cx);
                        }
                    })
                });
                let resumed = latest_request(app, cx);
                assert_ne!(resumed, cancelled);
                cx.update(|_, cx| {
                    let app = app.read(cx);
                    let job = app.transfer.transfer_job(&resumed).unwrap();
                    assert!(matches!(&job.kind, TransferJobKind::ListTree { path, .. }
                        if path.display_path == target));
                });
                assert_directory(app, cx, &shell, "initial");
                assert_eq!(tree_focused(app, cx), Some(other_key.clone()));
                finish(app, cx, &resumed, Ok("child"));
                assert_eq!(tree_focused(app, cx), Some(other_key));
            });
        }
    }
}

#[test]
fn terminal_result_for_a_removed_tree_job_clears_pending_and_resumes_loading() {
    with_browser(|app, cx, root| {
        let (_, target, _) = prepare_tree(app, cx, root);
        let key = tree_row(app, cx, &target).0;
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.expand_transfer_tree_row(&key, Some(true), cx)
            })
        });
        let removed = latest_request(app, cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.take_transfer_job_for_event(&removed).unwrap();
                assert!(app.apply_transfer_event(
                    TransferJobResult {
                        id: removed.clone(),
                        event: TransferJobEvent::Finished(Ok(TransferJobOutput::TreeEntries(
                            Vec::new(),
                        ))),
                    },
                    window,
                    cx,
                ));
            })
        });
        let resumed = latest_request(app, cx);
        assert_ne!(resumed, removed);
        finish(app, cx, &resumed, Ok("child"));
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let tree = app.transfer.tree_presentation(Some(SESSION), true);
                let row = tree.rows.iter().find(|row| row.key == key).unwrap();
                assert!(!row.loading);
                assert!(row.error.is_none());
                assert!(tree.rows.iter().any(|row| row.label == "child"));
            })
        });
    });
}

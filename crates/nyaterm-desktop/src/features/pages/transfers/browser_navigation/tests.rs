use crate::features::NyaTermApp;
use crate::features::test_support::{app_with_visible_ssh_session, session_focus_test_window};
use crate::models::{TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult};
use crate::test_support::{TestAppCleanup, TestConfigDir};
use gpui::{
    AppContext as _, Entity, KeyDownEvent, Keystroke, Modifiers, ScrollStrategy, TestAppContext,
    VisualTestContext,
};
use nyaterm_core::TransferBrowserViewMode;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};

const SESSION: &str = "browser-navigation";

fn directory(path: &str) -> SftpFileEntry {
    SftpFileEntry {
        name: path.rsplit('/').next().unwrap().into(),
        path: path.into(),
        file_type: SftpFileType::Directory,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: RemoteFilePath::from_raw(path, path.as_bytes()).raw_path_token,
        symlink_target_is_directory: false,
    }
}

fn request(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> (String, RemoteFilePath) {
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.transfer
                .browser_directory_requests_for_test()
                .last()
                .unwrap()
                .clone()
        })
    })
}

fn finish(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    id: &str,
    result: Result<Vec<SftpFileEntry>, &str>,
) {
    cx.update(|_, cx| {
        app.read(cx)
            .transfer
            .transfer_event_sender()
            .unbounded_send(TransferJobResult {
                id: id.into(),
                event: TransferJobEvent::Finished(
                    result
                        .map(TransferJobOutput::Entries)
                        .map_err(str::to_string),
                ),
            })
            .unwrap();
    });
    cx.run_until_parked();
}

fn navigate(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, path: &str) -> String {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_transfer_browser_directory(path.into(), window, cx);
        })
    });
    request(app, cx).0
}

fn with_browser(test: impl FnOnce(&Entity<NyaTermApp>, &mut VisualTestContext)) {
    let root = TestConfigDir::new("nyaterm-browser-navigation");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_ssh_session(&mut cx, root.path(), SESSION);
    cx.update_entity(&app, |app, _| {
        app.transfer.browser_directory_requests_for_test();
        let mut summary = app.settings.summary().clone();
        summary.ui_file_explorer_view_mode = TransferBrowserViewMode::List;
        summary.ui_file_explorer_show_hidden_files = false;
        app.settings.replace_summary(summary);
    });
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_event_drain(cx)));
    vcx.run_until_parked();
    let id = navigate(&app, vcx, "/a");
    finish(&app, vcx, &id, Ok(vec![directory("/a/b")]));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_transfer_browser_directory_path(
                RemoteFilePath::from_raw("/a/b/c", b"/a/b/c"),
                cx,
            );
        })
    });
    let id = request(&app, vcx).0;
    finish(&app, vcx, &id, Ok(vec![directory("/a/b/c/inside")]));
    test(&app, vcx);
}

fn assert_revealed(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext, entry: &SftpFileEntry) {
    cx.update(|_, cx| {
        let app = app.read(cx);
        let browser = app.transfer.browser_view();
        assert!(!browser.loading);
        assert!(browser.error.is_none());
        assert_eq!(
            browser.selected_remote_path.as_deref(),
            Some(entry.identity_key().as_str())
        );
        assert_eq!(browser.selected_remote_paths.len(), 1);
        assert!(
            browser
                .selected_remote_paths
                .contains(&entry.identity_key())
        );
        assert_eq!(app.transfer.remote_path(), entry.path);
        let scroll = browser
            .list_scroll
            .0
            .borrow()
            .deferred_scroll_to_item
            .unwrap();
        assert_eq!(scroll.item_index, 2);
        assert_eq!(scroll.strategy, ScrollStrategy::Nearest);
    });
}

#[test]
fn parent_keyboard_breadcrumb_and_history_navigation_share_identity_and_reveal() {
    for route in [
        "parent",
        "backspace",
        "breadcrumb",
        "history-menu",
        "back",
        "forward",
    ] {
        with_browser(|app, cx| {
            if route == "forward" {
                let id = navigate(app, cx, "/a");
                finish(app, cx, &id, Ok(vec![directory("/a/b")]));
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        app.open_transfer_browser_history(1, window, cx)
                    })
                });
                let id = request(app, cx).0;
                finish(app, cx, &id, Ok(vec![directory("/a/b/c/inside")]));
            }
            // History stores display paths. Returning to a descendant may therefore
            // leave the source without a raw token; listing recovery must still work.
            let source_has_token = cx.update(|_, cx| {
                app.read(cx)
                    .transfer
                    .browser_remote_file_path()
                    .raw_path_token
                    .is_some()
            });
            cx.update(|window, cx| {
                app.update(cx, |app, cx| match route {
                    "parent" => app.open_transfer_parent_directory(window, cx),
                    "backspace" => app.handle_transfer_browser_key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke {
                                modifiers: Modifiers::default(),
                                key: "backspace".into(),
                                key_char: None,
                            },
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    ),
                    // Both breadcrumb clicks and the history menu call this common entry point.
                    "breadcrumb" | "history-menu" => {
                        app.open_transfer_browser_directory("/a".into(), window, cx)
                    }
                    "back" => app.open_transfer_browser_history(1, window, cx),
                    "forward" => app.open_transfer_browser_history(-1, window, cx),
                    _ => unreachable!(),
                })
            });
            let (id, target) = request(app, cx);
            let selected = directory(if matches!(route, "parent" | "backspace") {
                "/a/b/c"
            } else {
                "/a/b"
            });
            let mut expected_parent = selected.remote_path().parent().unwrap();
            let requested_identity = if source_has_token {
                selected.identity_key()
            } else {
                expected_parent.raw_path_token = None;
                selected.path.clone()
            };
            assert_eq!(target, expected_parent, "route: {route}");
            cx.update(|_, cx| {
                assert!(matches!(&app.read(cx).transfer.transfer_job(&id).unwrap().kind,
                    TransferJobKind::ListDir { select_after: Some(identity), .. } if identity == &requested_identity));
            });
            let first = directory(&format!("{}/aaa", target.display_path));
            finish(app, cx, &id, Ok(vec![selected.clone(), first]));
            assert_revealed(app, cx, &selected);
        });
    }
}

#[test]
fn refresh_descendant_and_unrelated_navigation_do_not_inherit_ancestor_selection() {
    for target in ["/a/b/c", "/a/b/c/inside", "/a/b2", "/other"] {
        with_browser(|app, cx| {
            if target == "/a/b/c" {
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| app.refresh_transfer_browser(window, cx))
                });
            } else {
                navigate(app, cx, target);
            }
            let (id, _) = request(app, cx);
            cx.update(|_, cx| {
                assert!(matches!(
                    &app.read(cx).transfer.transfer_job(&id).unwrap().kind,
                    TransferJobKind::ListDir {
                        select_after: None,
                        ..
                    }
                ));
            });
            finish(app, cx, &id, Ok(vec![directory(&format!("{target}/item"))]));
            cx.update(|_, cx| {
                let app = app.read(cx);
                let browser = app.transfer.browser_view();
                assert_eq!(browser.path, target);
                assert!(browser.selected_remote_path.is_none());
                assert!(
                    browser
                        .list_scroll
                        .0
                        .borrow()
                        .deferred_scroll_to_item
                        .is_none()
                );
            });
        });
    }
}

#[test]
fn filtered_parent_target_is_not_selected_or_revealed_later() {
    with_browser(|app, cx| {
        cx.update(|_, cx| app.update(cx, |app, _| app.transfer.set_browser_search("aaa".into())));
        let id = navigate(app, cx, "/a");
        finish(
            app,
            cx,
            &id,
            Ok(vec![directory("/a/b"), directory("/a/aaa")]),
        );
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let browser = app.transfer.browser_view();
                assert!(browser.selected_remote_path.is_none());
                assert!(browser.selected_remote_paths.is_empty());
                assert!(
                    browser
                        .list_scroll
                        .0
                        .borrow()
                        .deferred_scroll_to_item
                        .is_none()
                );
                assert_eq!(browser.search, "aaa");
                app.transfer.clear_browser_search();
                app.transfer.visible_browser_entries(true);
                assert!(app.transfer.browser_view().selected_remote_path.is_none());
            })
        });
    });
}

#[test]
fn latest_ancestor_request_keeps_selection_and_scroll_when_old_reply_arrives() {
    with_browser(|app, cx| {
        let old = navigate(app, cx, "/a/b");
        let latest = navigate(app, cx, "/a");
        let target = directory("/a/b");
        finish(
            app,
            cx,
            &latest,
            Ok(vec![target.clone(), directory("/a/aaa")]),
        );
        assert_revealed(app, cx, &target);
        finish(app, cx, &old, Ok(vec![directory("/a/b/c")]));
        assert_revealed(app, cx, &target);
        cx.update(|_, cx| assert_eq!(app.read(cx).transfer.browser_view().path, "/a"));
    });
}

#[test]
fn failed_ancestor_request_restores_path_identity_selection_and_scroll() {
    with_browser(|app, cx| {
        let selected = directory("/a/b/c/inside");
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.transfer.select_browser_entry(selected.identity_key());
                app.transfer.scroll_browser_to_item(7);
            })
        });
        let id = navigate(app, cx, "/a");
        finish(app, cx, &id, Err("permission denied"));
        cx.update(|_, cx| {
            let app = app.read(cx);
            let browser = app.transfer.browser_view();
            assert_eq!(browser.path, "/a/b/c");
            assert_eq!(
                app.transfer.browser_remote_file_path(),
                RemoteFilePath::from_raw("/a/b/c", b"/a/b/c")
            );
            assert_eq!(
                browser.selected_remote_path.as_deref(),
                Some(selected.identity_key().as_str())
            );
            assert!(
                browser
                    .selected_remote_paths
                    .contains(&selected.identity_key())
            );
            let scroll = browser
                .list_scroll
                .0
                .borrow()
                .deferred_scroll_to_item
                .unwrap();
            assert_eq!(scroll.item_index, 7);
            assert_eq!(scroll.strategy, ScrollStrategy::Top);
        });
    });
}

#[test]
fn background_ancestor_result_does_not_change_the_active_selection_or_scroll() {
    with_browser(|app, cx| {
        let old = navigate(app, cx, "/a");
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let metadata = app.session.metadata(SESSION).unwrap().clone();
                app.session.register_session_metadata("other", metadata);
                app.session.select_active_session("other");
                app.transfer
                    .begin_browser_directory_load_path(RemoteFilePath::from_raw(
                        "/other", b"/other",
                    ));
                app.transfer.select_browser_entry("other-selection".into());
                app.transfer.scroll_browser_to_item(9);
            })
        });
        finish(
            app,
            cx,
            &old,
            Ok(vec![directory("/a/b"), directory("/a/aaa")]),
        );
        cx.update(|_, cx| {
            let app = app.read(cx);
            let browser = app.transfer.browser_view();
            assert_eq!(browser.path, "/other");
            assert_eq!(
                browser.selected_remote_path.as_deref(),
                Some("other-selection")
            );
            assert_eq!(
                browser
                    .list_scroll
                    .0
                    .borrow()
                    .deferred_scroll_to_item
                    .unwrap()
                    .item_index,
                9
            );
            assert_eq!(
                app.transfer
                    .browser_session_cache(SESSION)
                    .unwrap()
                    .current_path,
                "/a"
            );
            assert_eq!(
                app.transfer
                    .browser_session_cache(SESSION)
                    .unwrap()
                    .current_raw_path_token,
                RemoteFilePath::from_raw("/a", b"/a").raw_path_token,
            );
        });
    });
}

#[test]
fn android_initial_navigation_uses_device_directory_on_first_load_and_reset() {
    for (directory, expected) in [
        ("", "/sdcard/"),
        ("/sdcard/", "/sdcard/"),
        (
            "/storage/emulated/0/Download",
            "/storage/emulated/0/Download",
        ),
    ] {
        let root = TestConfigDir::new("nyaterm-android-browser-initial");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = crate::features::test_support::app_with_visible_android_session(
            &mut cx,
            root.path(),
            SESSION,
            directory,
        );
        app.update(&mut cx, |app, cx| {
            app.transfer.browser_directory_requests_for_test();
            {
                app.reset_transfer_browser_for_active_session();
                assert_eq!(app.transfer.browser_view().path, expected);
                assert_eq!(app.transfer.browser_view().home_dir, expected);
                app.load_transfer_browser_for_active_session_if_needed(cx);
                let (_, requested) = app
                    .transfer
                    .browser_directory_requests_for_test()
                    .last()
                    .unwrap();
                assert_eq!(requested.display_path, expected);
                assert!(nyaterm_files::adb::raw_path(requested).is_ok());
            }
        });
    }
}

#[test]
fn android_cache_rejects_host_relative_and_corrupt_paths_and_retains_valid_device_history() {
    let root = TestConfigDir::new("nyaterm-android-browser-cache");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = crate::features::test_support::app_with_visible_android_session(
        &mut cx,
        root.path(),
        SESSION,
        "/sdcard/",
    );
    app.update(&mut cx, |app, cx| {
        for path in [
            RemoteFilePath::new("."),
            RemoteFilePath::new(r"C:\Users\fixture"),
            RemoteFilePath {
                display_path: "/sdcard/".into(),
                raw_path_token: Some("invalid-token".into()),
            },
        ] {
            app.reset_transfer_browser_for_active_session();
            app.transfer.begin_browser_directory_load_path(path);
            app.cache_transfer_browser_session(SESSION);
            assert!(!app.restore_transfer_browser_session_cache(SESSION, cx));
            assert!(!app.transfer.has_browser_session_cache(SESSION));
        }
        app.reset_transfer_browser_for_active_session();
        let path = RemoteFilePath::from_raw("/sdcard/Download", b"/sdcard/Download");
        app.transfer.begin_browser_directory_load_path(path.clone());
        app.transfer
            .record_browser_history("/sdcard/Download".into());
        app.cache_transfer_browser_session(SESSION);
        assert!(app.restore_transfer_browser_session_cache(SESSION, cx));
        assert_eq!(app.transfer.browser_remote_file_path(), path);
        assert!(
            app.transfer
                .browser_view()
                .history
                .iter()
                .all(|path| path.starts_with('/'))
        );
    });
}

#[test]
fn typed_android_background_failure_restores_device_context_without_rewinding_active_session() {
    let root = TestConfigDir::new("nyaterm-android-browser-event");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = crate::features::test_support::app_with_visible_android_session(
        &mut cx,
        root.path(),
        SESSION,
        "/sdcard/Download",
    );
    app.update(&mut cx, |app, cx| {
        app.transfer.browser_directory_requests_for_test();
        app.reset_transfer_browser_for_active_session();
        app.load_transfer_browser_for_active_session_if_needed(cx);
    });
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_event_drain(cx)));
    let id = request(&app, vcx).0;
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            let mut metadata = app.session.metadata(SESSION).unwrap().clone();
            metadata.launch_config = crate::models::SessionLaunchConfig::Local(Default::default());
            app.session.register_session_metadata("other", metadata);
            app.session.select_active_session("other");
            app.transfer
                .begin_browser_directory_load("local-fixture".into());
            app.transfer
                .transfer_event_sender()
                .unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::FileFinished(Err(anyhow::Error::new(
                        nyaterm_files::adb::AdbFileValidationError::InvalidPath,
                    )
                    .context(nyaterm_files::adb::AdbFileError))),
                })
                .unwrap();
        })
    });
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(app.transfer.browser_view().path, "local-fixture");
            assert!(app.transfer.browser_session_cache(SESSION).is_none());
            app.session.select_active_session(SESSION);
            app.reset_transfer_browser_for_active_session();
            assert_eq!(app.transfer.browser_view().path, "/sdcard/Download");
            app.load_transfer_browser_for_active_session_if_needed(cx);
        });
    });
    let id = request(&app, vcx).0;
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.session.select_active_session("other");
            app.transfer
                .begin_browser_directory_load("local-fixture".into());
            app.transfer
                .transfer_event_sender()
                .unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::FileFinished(Ok(TransferJobOutput::Entries(
                        Vec::new(),
                    ))),
                })
                .unwrap();
        })
    });
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let app = app.read(cx);
        assert_eq!(app.transfer.browser_view().path, "local-fixture");
        let cache = app.transfer.browser_session_cache(SESSION).unwrap();
        assert_eq!(cache.current_path, "/sdcard/Download");
        assert_eq!(cache.home_dir, "/sdcard/Download");
    });
}

#[test]
fn typed_android_directory_failure_is_localized_at_the_event_boundary() {
    let root = TestConfigDir::new("nyaterm-android-browser-failure");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = crate::features::test_support::app_with_visible_android_session(
        &mut cx,
        root.path(),
        SESSION,
        "/sdcard/",
    );
    app.update(&mut cx, |app, cx| {
        app.transfer.browser_directory_requests_for_test();
        app.reset_transfer_browser_for_active_session();
        app.load_transfer_browser_for_active_session_if_needed(cx);
    });
    let vcx = session_focus_test_window(&mut cx, &app);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.start_transfer_event_drain(cx)));
    let id = request(&app, vcx).0;
    vcx.update(|_, cx| {
        app.read(cx)
            .transfer
            .transfer_event_sender()
            .unbounded_send(TransferJobResult {
                id,
                event: TransferJobEvent::FileFinished(Err(anyhow::Error::new(
                    nyaterm_files::adb::AdbFileValidationError::InvalidPath,
                )
                .context(nyaterm_files::adb::AdbFileError))),
            })
            .unwrap();
    });
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let browser = app.read(cx).transfer.browser_view();
        assert_eq!(browser.path, "/sdcard/");
        assert_eq!(
            browser.error.as_deref(),
            Some(rust_i18n::t!("android.errors.invalidFilePath").as_ref())
        );
        assert!(!browser.loading);
    });
}

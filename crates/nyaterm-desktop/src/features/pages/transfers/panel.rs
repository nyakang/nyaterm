use super::TransferBrowserAvailability;
use crate::features::NyaTermApp;
use crate::models::{
    TransferBrowserColumnResizeState, TransferBrowserColumnWidths, TransferBrowserSortColumn,
    TransferBrowserSortDirection, TransferJobRowSnapshot, TransferRenameState,
};
use crate::theme::ThemePalette;
use gpui::{
    Context, Entity, FocusHandle, IntoElement, Render, Rgba, ScrollHandle, UniformListScrollHandle,
    WeakEntity, Window,
};
use nyaterm_core::TransferBrowserViewMode;
use nyaterm_files::sftp::SftpFileEntry;
use nyaterm_ui::NyaInputState;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// Colours the transfers panel needs that are not on the palette itself.
///
/// These depend on wallpaper and native transparency, so they cannot be derived from the palette
/// alone and have to be resolved where the shell settings live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct TransferChrome {
    pub palette: ThemePalette,
    pub transparent_surface: Rgba,
    pub transparent_section_header: Rgba,
    pub surface: Rgba,
    pub panel_width: f32,
    pub ui_font_size: f32,
}

/// The browser's render state, owned.
///
/// `TransferBrowserView` is a borrowed projection, which is exactly right for a
/// caller that already holds the state and wrong for a panel that must not touch
/// it. The costly field -- the listing -- is shared rather than copied; the rest are
/// short paths and small sets.
pub(in crate::features) struct TransferBrowserPresentation {
    pub view_mode: TransferBrowserViewMode,
    pub tree: crate::features::transfers::TransferTreePresentation,
    pub tree_focus: FocusHandle,
    pub local_backend: bool,
    pub path: String,
    pub terminal_cwd: Option<String>,
    pub home_dir: String,
    pub path_editing: bool,
    pub expanded_children_path: Option<String>,
    /// The filtered, sorted listing straight from the state's memo. A progress batch
    /// leaves the memo alone, so this is a refcount bump on the hot path.
    pub visible_entries: Arc<[SftpFileEntry]>,
    /// The unfiltered listing, shared. The footer counts against it.
    pub all_entries: Arc<Vec<SftpFileEntry>>,
    pub loading: bool,
    pub error: Option<String>,
    pub search: String,
    pub search_expanded: bool,
    pub list_scroll: UniformListScrollHandle,
    pub horizontal_scroll: ScrollHandle,
    pub visited_history: VecDeque<String>,
    pub favorites: VecDeque<String>,
    pub sort_column: TransferBrowserSortColumn,
    pub sort_direction: TransferBrowserSortDirection,
    pub column_widths: TransferBrowserColumnWidths,
    pub column_resize: Option<TransferBrowserColumnResizeState>,
    pub selected_remote_path: Option<String>,
    pub selected_remote_paths: HashSet<String>,
    pub external_drop_hover: bool,
    pub focus: FocusHandle,
    pub rename: Option<TransferRenameState>,
    pub auto_sync_cwd_enabled: bool,
    pub connection_id: Option<String>,
    /// Built where the field is revealed, never in render.
    pub search_field: Option<Entity<NyaInputState>>,
    pub path_field: Option<Entity<NyaInputState>>,
    /// Built where the rename dialog opens, so the virtualised row builder can show
    /// the field without being able to create one.
    pub rename_field: Option<Entity<NyaInputState>>,
    pub show_hidden_files: bool,
}

/// The queue's render state.
pub(in crate::features) struct TransferQueuePresentation {
    /// Already filtered to the active session and already ordered. Both used to
    /// happen in render, each with a full clone of every job.
    pub rows: Arc<[TransferJobRowSnapshot]>,
    pub has_running: bool,
    pub has_paused: bool,
    pub has_active: bool,
    pub has_completed: bool,
    pub has_stopped: bool,
    pub selected_job_id: Option<String>,
    pub download_path: String,
    pub focus: FocusHandle,
}

/// Everything the panel draws from: data, never elements.
pub(in crate::features) struct TransferSnapshot {
    pub chrome: TransferChrome,
    pub availability: TransferBrowserAvailability,
    pub panel_height: f32,
    pub height_is_resizing: bool,
    pub resize_handle_highlighted: bool,
    pub has_session: bool,
    pub browser: TransferBrowserPresentation,
    pub queue: TransferQueuePresentation,
}

pub(in crate::features) struct TransferPanel {
    /// Weak, so the panel does not keep the app alive.
    app: WeakEntity<NyaTermApp>,
    snapshot: Option<TransferSnapshot>,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    rows_built: std::cell::Cell<usize>,
}

impl TransferPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>) -> Self {
        Self {
            app,
            snapshot: None,
            #[cfg(test)]
            paint_count: 0,
            #[cfg(test)]
            rows_built: std::cell::Cell::new(0),
        }
    }

    pub(in crate::features::pages::transfers) fn snapshot(&self) -> Option<&TransferSnapshot> {
        self.snapshot.as_ref()
    }

    pub(in crate::features) fn set_snapshot(
        &mut self,
        snapshot: TransferSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// The one hop from a panel interaction back to the owner.
    ///
    /// The flush is deferred rather than run inline, and that is load-bearing: this is
    /// called from a listener (or the virtualised row builder) on this panel, so GPUI
    /// has the panel *leased* -- taken out of the entity map -- for the whole callback.
    /// A flush that reached back for `panel.read` or `panel.update` would double-lease
    /// it and abort the process. `App::defer` exists for precisely this, and runs at
    /// the end of the current effect cycle: after the lease is returned, and still
    /// before anything paints, so the panel is never drawn stale.
    pub(in crate::features::pages::transfers) fn with_app<R: Default>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>) -> R,
    ) -> R {
        let Some(app) = self.app.upgrade() else {
            return R::default();
        };
        app.update(cx, |app, cx| {
            let result = f(app, cx);
            app.defer_transfer_panel_snapshot_flush(cx);
            result
        })
    }

    /// A weak handle for a deferred callback. Render must not use it.
    pub(in crate::features::pages::transfers) fn app_handle(&self) -> WeakEntity<NyaTermApp> {
        self.app.clone()
    }

    #[cfg(test)]
    pub(in crate::features) fn paint_count(&self) -> usize {
        self.paint_count
    }

    #[cfg(test)]
    pub(in crate::features) fn queue_row_count_for_test(&self) -> usize {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.queue.rows.len())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::features::pages::transfers) fn note_rows_built(&self, count: usize) {
        self.rows_built.set(self.rows_built.get() + count);
    }

    #[cfg(test)]
    pub(in crate::features) fn rows_built(&self) -> usize {
        self.rows_built.get()
    }
}

impl Render for TransferPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.paint_count += 1;
        }
        // Zero `NyaTermApp` access, diagnostics included: GPUI records every entity
        // read during a draw, so one app read here would put this panel back on the
        // app's invalidation path and undo the isolation.
        super::transfer_panel(self, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::NavItem;
    use crate::test_support::TestConfigDir;
    use gpui::{
        AppContext as _, ClickEvent, Entity, IntoElement, Modifiers, MouseButton, MouseClickEvent,
        MouseDownEvent, MouseUpEvent, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, div, px,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
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
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    /// Mirrors what `single_side_panel` gives the real panel: a constrained,
    /// definitely-sized body and the same cached style. Measuring `.cached()` against
    /// anything looser would not measure the shipped layout.
    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let overlay = self
                .app
                .read(cx)
                .transfer
                .browser_view()
                .path_menu
                .is_some()
                .then(|| {
                    self.app.update(cx, |app, cx| {
                        crate::features::view_widgets::full_window_overlay_layer(
                            "transfer-path-menu-input-layer",
                            app.transfer_browser_path_menu_overlay(window, cx),
                        )
                        .into_any_element()
                    })
                });
            let favorites_overlay = self
                .app
                .read(cx)
                .transfer
                .browser_view()
                .favorites_menu
                .is_some()
                .then(|| {
                    self.app.update(cx, |app, cx| {
                        crate::features::view_widgets::full_window_overlay_layer(
                            "transfer-favorites-menu-input-layer",
                            app.transfer_browser_favorites_menu_overlay(cx),
                        )
                        .into_any_element()
                    })
                });
            div()
                .relative()
                .w(px(320.))
                .h(px(720.))
                .flex()
                .flex_col()
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        self.app
                            .read(cx)
                            .transfer_panel
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
                .children(overlay)
                .children(favorites_overlay)
        }
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_or_toggle_panel(NavItem::Transfers, cx);
            app.flush_transfer_panel_snapshot(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| AppHost { app: host_app });
            nyaterm_ui::nya_root(host, window, cx)
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn hosted_file_browser<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let (app, vcx) = hosted(cx, root);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.session.register_session_metadata(
                    "path-test",
                    crate::models::SessionRuntimeMetadata {
                        ssh_config: None,
                        ssh_multiplex_key: None,
                        source_connection_id: None,
                        ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                        launch_config: crate::models::SessionLaunchConfig::Local(
                            nyaterm_transport::LocalSessionConfig::default(),
                        ),
                        disconnected: false,
                    },
                );
                app.session.select_active_session("path-test");
                app.start_transfer_event_drain(cx);
                app.transfer.store_browser_session_cache(
                    "path-test".to_string(),
                    crate::models::TransferBrowserSessionCacheState {
                        entries: std::sync::Arc::new(Vec::new()),
                        current_path: "/remote".to_string(),
                        current_raw_path_token: None,
                        home_dir: root.display().to_string(),
                        history: std::collections::VecDeque::from(["/remote".to_string()]),
                        history_index: 0,
                        visited_history: std::collections::VecDeque::from(["/history".to_string()]),
                    },
                );
                app.restore_transfer_browser_session_cache("path-test", cx);
                app.flush_transfer_panel_snapshot(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        (app, vcx)
    }

    #[test]
    fn file_row_ctrl_toggle_and_plain_click_update_the_selection() {
        for remote in [false, true] {
            for name_column in [false, true] {
                let test_dir = TestConfigDir::new("nyaterm-file-selection");
                let mut cx = TestAppContext::single();
                let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
                let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
                let entries: Vec<_> = (0..4)
                    .map(super::super::tests_support::browser_entry)
                    .collect();
                vcx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        if remote {
                            let config = nyaterm_transport::SshSessionConfig::default();
                            app.session.register_session_metadata(
                                "path-test",
                                crate::models::SessionRuntimeMetadata {
                                    ssh_config: Some(config.clone()),
                                    ssh_multiplex_key: None,
                                    source_connection_id: None,
                                    ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                                    launch_config: crate::models::SessionLaunchConfig::Ssh(
                                        Box::new(config),
                                    ),
                                    disconnected: false,
                                },
                            );
                        }
                        app.transfer
                            .replace_browser_entries_for_test(entries.clone());
                        app.transfer.replace_browser_selection(
                            entries
                                .iter()
                                .take(3)
                                .map(nyaterm_files::sftp::SftpFileEntry::identity_key)
                                .collect(),
                            Some(entries[0].identity_key()),
                        );
                        app.flush_transfer_panel_snapshot(cx);
                    });
                });
                draw_path_fixture(vcx);
                let row = vcx
                    .debug_bounds("transfer-browser-entry-/remote/entry-0000")
                    .unwrap();
                let pointer = gpui::point(
                    row.left() + px(if name_column { 80. } else { 280. }),
                    row.center().y,
                );
                let ctrl = Modifiers {
                    control: true,
                    ..Modifiers::none()
                };
                vcx.simulate_mouse_move(pointer, None, ctrl);
                vcx.simulate_mouse_down(pointer, MouseButton::Left, ctrl);
                draw_path_fixture(vcx);
                // Even a small move with the button down must not reselect A.
                vcx.simulate_mouse_move(
                    pointer + gpui::point(px(1.), px(0.)),
                    MouseButton::Left,
                    ctrl,
                );
                vcx.simulate_mouse_up(pointer, MouseButton::Left, ctrl);
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    let app = app.read(cx);
                    let browser = app.transfer.browser_view();
                    assert_eq!(
                        browser.selected_remote_paths,
                        &std::collections::HashSet::from([
                            entries[1].identity_key(),
                            entries[2].identity_key()
                        ])
                    );
                    assert_eq!(
                        browser.selected_remote_path.as_deref(),
                        Some(entries[1].path.as_str())
                    );
                    assert!(!app.transfer.rename_dialog_is_open());
                });
                vcx.simulate_click(pointer, ctrl);
                draw_path_fixture(vcx);
                vcx.simulate_mouse_down(pointer, MouseButton::Left, Modifiers::none());
                vcx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx)
                            .transfer
                            .browser_view()
                            .selected_remote_paths
                            .len(),
                        3
                    )
                });
                vcx.simulate_mouse_up(pointer, MouseButton::Left, Modifiers::none());
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    let app = app.read(cx);
                    assert_eq!(
                        app.transfer.browser_view().selected_remote_paths,
                        &std::collections::HashSet::from([entries[0].identity_key()])
                    );
                    assert!(
                        !app.transfer.rename_dialog_is_open(),
                        "collapsing a multi-selection must not rename"
                    );
                });
                let last_row = vcx
                    .debug_bounds("transfer-browser-entry-/remote/entry-0002")
                    .unwrap();
                let last = gpui::point(pointer.x, last_row.center().y);
                vcx.simulate_click(
                    last,
                    Modifiers {
                        shift: true,
                        ..Modifiers::none()
                    },
                );
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx)
                            .transfer
                            .browser_view()
                            .selected_remote_paths
                            .len(),
                        3
                    )
                });
                // Ctrl+Shift extends the existing selection with the next range.
                let last_row = vcx
                    .debug_bounds("transfer-browser-entry-/remote/entry-0003")
                    .unwrap();
                vcx.simulate_click(
                    gpui::point(pointer.x, last_row.center().y),
                    Modifiers {
                        shift: true,
                        control: true,
                        ..Modifiers::none()
                    },
                );
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx)
                            .transfer
                            .browser_view()
                            .selected_remote_paths
                            .len(),
                        4
                    )
                });
                // Crossing rows activates range selection, including when the
                // pointer subsequently returns to the original pressed row.
                vcx.simulate_mouse_down(pointer, MouseButton::Left, ctrl);
                vcx.simulate_mouse_move(last, MouseButton::Left, ctrl);
                vcx.simulate_mouse_move(pointer, MouseButton::Left, ctrl);
                vcx.simulate_mouse_up(pointer, MouseButton::Left, ctrl);
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    assert_eq!(
                        app.read(cx)
                            .transfer
                            .browser_view()
                            .selected_remote_paths
                            .len(),
                        4
                    );
                });
                // Releasing outside the pressed row must abandon the deferred
                // click rather than collapse the selection on a later event.
                vcx.simulate_mouse_down(pointer, MouseButton::Left, Modifiers::none());
                vcx.simulate_mouse_up(last, MouseButton::Left, Modifiers::none());
                draw_path_fixture(vcx);
                vcx.update(|_, cx| {
                    app.update(cx, |app, _| {
                        assert_eq!(app.transfer.browser_view().selected_remote_paths.len(), 4);
                        assert!(
                            !app.transfer
                                .consume_browser_selection_click(&entries[0].identity_key())
                        );
                    });
                });
            }
        }
    }

    #[test]
    fn file_row_starts_visible_export_from_first_press_and_preserves_multiselection() {
        for remote in [false, true] {
            let test_dir = TestConfigDir::new("nyaterm-file-drag");
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
            let entries: Vec<_> = (0..3)
                .map(super::super::tests_support::browser_entry)
                .collect();
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    if remote {
                        let config = nyaterm_transport::SshSessionConfig::default();
                        app.session.register_session_metadata(
                            "path-test",
                            crate::models::SessionRuntimeMetadata {
                                ssh_config: Some(config.clone()),
                                ssh_multiplex_key: None,
                                source_connection_id: None,
                                ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                                launch_config: crate::models::SessionLaunchConfig::Ssh(Box::new(
                                    config,
                                )),
                                disconnected: false,
                            },
                        );
                    }
                    app.transfer
                        .replace_browser_entries_for_test(entries.clone());
                    assert_eq!(
                        app.session.active_file_browser_backend(),
                        Some(if remote {
                            nyaterm_files::file_browser::FileBrowserBackendKind::Remote
                        } else {
                            nyaterm_files::file_browser::FileBrowserBackendKind::Local
                        })
                    );
                    app.flush_transfer_panel_snapshot(cx);
                });
            });
            draw_path_fixture(vcx);
            let row = vcx
                .debug_bounds("transfer-browser-entry-/remote/entry-0000")
                .expect("file row");
            let start = gpui::point(row.left() + px(80.), row.top() + px(15.));
            vcx.simulate_mouse_move(start, None, Modifiers::none());
            vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
            draw_path_fixture(vcx);
            vcx.simulate_mouse_move(
                start + gpui::point(px(20.), px(5.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw_path_fixture(vcx);
            if remote && cfg!(target_os = "linux") {
                // The gesture is recognized so unsupported native export is visible.
                // No native selection or content provider is created on Linux.
                vcx.update(|_, cx| {
                    app.update(cx, |app, _| {
                        assert_eq!(
                            app.shell.status(),
                            rust_i18n::t!("fileExplorer.dragPlatformUnsupported").as_ref()
                        );
                        assert!(!app.transfer.transfer_jobs().iter().any(|job| matches!(
                            job.kind,
                            crate::models::TransferJobKind::DragExport { .. }
                        )));
                    })
                });
                vcx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
                continue;
            }
            vcx.update(|_, cx| {
                assert!(
                    cx.has_active_drag(),
                    "first press must start drag without a prior click"
                )
            });
            let preview = vcx
                .debug_bounds("transfer-file-drag-preview")
                .expect("visible file preview");
            assert!(preview.size.width > px(0.) && preview.size.height > px(0.));
            vcx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.transfer.replace_browser_selection(
                        entries
                            .iter()
                            .take(2)
                            .map(nyaterm_files::sftp::SftpFileEntry::identity_key)
                            .collect(),
                        Some(entries[0].identity_key()),
                    );
                    app.flush_transfer_panel_snapshot(cx);
                });
            });
            draw_path_fixture(vcx);
            vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
            vcx.simulate_mouse_move(
                start + gpui::point(px(20.), px(5.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw_path_fixture(vcx);
            vcx.update(|_, cx| {
                assert!(cx.has_active_drag());
                assert_eq!(
                    app.read(cx)
                        .transfer
                        .browser_view()
                        .selected_remote_paths
                        .len(),
                    2
                );
            });
            vcx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
            draw_path_fixture(vcx);
            vcx.update(|_, cx| {
                assert_eq!(
                    app.read(cx)
                        .transfer
                        .browser_view()
                        .selected_remote_paths
                        .len(),
                    2
                );
                assert!(!app.read(cx).transfer.rename_dialog_is_open());
            });
        }
    }

    #[test]
    fn tree_drag_preserves_selected_files_and_directories_from_the_first_press() {
        let test_dir = TestConfigDir::new("nyaterm-tree-file-drag");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let mut entries: Vec<_> = (0..2)
            .map(super::super::tests_support::browser_entry)
            .collect();
        entries[1].file_type = nyaterm_files::sftp::SftpFileType::Directory;
        let root = nyaterm_files::file_browser::file_browser_root(
            nyaterm_files::file_browser::FileBrowserBackendKind::Local,
            &test_dir.path().display().to_string(),
        );
        for entry in &mut entries {
            entry.path = Path::new(&root).join(&entry.name).display().to_string();
        }
        let row_key = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut settings = app.settings.summary().clone();
                settings.ui_file_explorer_view_mode = nyaterm_core::TransferBrowserViewMode::Tree;
                app.settings.replace_summary(settings);
                let path = nyaterm_files::sftp::RemoteFilePath::new(root);
                app.transfer.seed_tree_listing(
                    "path-test",
                    nyaterm_files::file_browser::FileBrowserBackendKind::Local,
                    path.clone(),
                    std::sync::Arc::new(entries.clone()),
                );
                app.transfer.reveal_tree_path(
                    "path-test",
                    nyaterm_files::file_browser::FileBrowserBackendKind::Local,
                    path,
                );
                let rows = app.transfer.tree_presentation(Some("path-test"), true).rows;
                let selected: Vec<_> = rows
                    .iter()
                    .filter(|row| row.entry.is_some())
                    .map(|row| row.key.clone())
                    .collect();
                assert_eq!(selected.len(), 2);
                for (index, key) in selected.iter().enumerate() {
                    app.transfer
                        .select_tree_row("path-test", key.clone(), index != 0, false);
                }
                let drag = crate::features::transfers::drag_export::DraggedSelection::new_tree(
                    entries[0].clone(),
                );
                app.capture_transfer_drag(&drag, cx);
                assert_eq!(drag.file_count(), 2);
                let gpui::ExternalDragPayload::Files(paths) = app
                    .resolve_transfer_drag(&drag, false, false, cx)
                    .expect("local tree snapshot")
                else {
                    panic!("real local paths required");
                };
                assert_eq!(paths.entries().len(), 2);
                assert!(paths.entries().iter().any(|(_, directory)| *directory));
                app.flush_transfer_panel_snapshot(cx);
                selected[0].clone()
            })
        });
        draw_path_fixture(vcx);
        let selector = Box::leak(format!("transfer-tree-row:{row_key}").into_boxed_str());
        let row = vcx.debug_bounds(selector).expect("tree file row");
        let start = gpui::point(row.left() + px(80.), row.top() + px(14.));
        vcx.simulate_mouse_move(start, None, Modifiers::none());
        vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(
            start + gpui::point(px(20.), px(5.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw_path_fixture(vcx);
        vcx.update(|_, cx| {
            assert!(cx.has_active_drag());
            assert_eq!(
                app.read(cx)
                    .transfer
                    .selected_tree_entries("path-test")
                    .len(),
                2
            );
        });
        assert!(vcx.debug_bounds("transfer-file-drag-preview").is_some());
    }

    #[test]
    fn tree_marks_terminal_cwd_and_keeps_scroll_position_hint_in_the_viewport() {
        use nyaterm_files::file_browser::{FileBrowserBackendKind, file_browser_root};
        use nyaterm_files::sftp::{RemoteFilePath, SftpFileType};

        let test_dir = TestConfigDir::new("nyaterm-tree-position");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let root = file_browser_root(
            FileBrowserBackendKind::Local,
            &test_dir.path().display().to_string(),
        );
        let mut entries: Vec<_> = (0..500)
            .map(super::super::tests_support::browser_entry)
            .collect();
        for entry in &mut entries {
            entry.path = Path::new(&root).join(&entry.name).display().to_string();
        }
        entries[0].file_type = SftpFileType::Directory;
        let cwd = entries[0].path.clone();
        let (scroll, current_key) = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut settings = app.settings.summary().clone();
                settings.ui_file_explorer_view_mode = nyaterm_core::TransferBrowserViewMode::Tree;
                app.settings.replace_summary(settings);
                app.session.update_cwd("path-test", cwd.clone());
                let root = RemoteFilePath::new(root);
                app.transfer.seed_tree_listing(
                    "path-test",
                    FileBrowserBackendKind::Local,
                    root.clone(),
                    std::sync::Arc::new(entries),
                );
                app.transfer
                    .reveal_tree_path("path-test", FileBrowserBackendKind::Local, root);
                let tree = app.transfer.tree_presentation(Some("path-test"), true);
                let current_key = tree
                    .rows
                    .iter()
                    .find(|row| row.path.display_path == cwd)
                    .unwrap()
                    .key
                    .clone();
                app.flush_transfer_panel_snapshot(cx);
                (tree.scroll, current_key)
            })
        });
        draw_path_fixture(vcx);
        let marker = Box::leak(format!("tree-terminal-cwd:{current_key}").into_boxed_str());
        assert!(vcx.debug_bounds(marker).is_some());
        assert!(vcx.debug_bounds("transfer-tree-position").is_none());
        let row_selector = Box::leak(format!("transfer-tree-row:{current_key}").into_boxed_str());
        let row_bounds = vcx.debug_bounds(row_selector).unwrap();
        let viewport = vcx.debug_bounds("transfer-tree-viewport").unwrap();
        assert_eq!(row_bounds.left() - viewport.left(), px(4.));
        assert_eq!(row_bounds.size.height, px(24.));
        let next_key = vcx.update(|_, cx| {
            app.read(cx)
                .transfer_panel
                .read(cx)
                .snapshot()
                .unwrap()
                .browser
                .tree
                .rows[2]
                .key
                .clone()
        });
        let next_selector = Box::leak(format!("transfer-tree-row:{next_key}").into_boxed_str());
        assert_eq!(
            vcx.debug_bounds(next_selector).unwrap().top() - row_bounds.top(),
            px(28.)
        );
        vcx.simulate_mouse_move(row_bounds.center(), None, Modifiers::none());
        draw_path_fixture(vcx);
        assert_eq!(
            vcx.debug_bounds(row_selector),
            Some(row_bounds),
            "hover must not move or resize a row"
        );
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
        draw_path_fixture(vcx);
        let details = vcx
            .debug_bounds("transfer-tree-details")
            .expect("structured file details");
        assert_eq!(details.size.width, px(208.));
        vcx.simulate_click(row_bounds.center(), Modifiers::none());
        vcx.simulate_mouse_move(
            row_bounds.center() - gpui::point(px(0.), px(56.)),
            None,
            Modifiers::none(),
        );
        vcx.simulate_mouse_move(row_bounds.center(), None, Modifiers::none());
        draw_path_fixture(vcx);
        assert_eq!(vcx.debug_bounds(row_selector), Some(row_bounds));
        assert!(vcx.debug_bounds(marker).is_some());
        assert!(vcx.debug_bounds("transfer-tree-details").is_none());
        vcx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .transfer_panel
                    .read(cx)
                    .snapshot()
                    .unwrap()
                    .browser
                    .tree
                    .selected
                    .contains(&current_key)
            );
        });
        let mut hint_top = None;
        for index in [250, 400] {
            vcx.update(|_, cx| {
                scroll.scroll_to_item(index, gpui::ScrollStrategy::Top);
                app.update(cx, |app, cx| app.flush_transfer_panel_snapshot(cx));
            });
            draw_path_fixture(vcx);
            let viewport = vcx.debug_bounds("transfer-tree-viewport").unwrap();
            let hint = vcx
                .debug_bounds("transfer-tree-position")
                .expect("scroll context hint");
            assert!(hint.top() >= viewport.top());
            assert!(hint.bottom() <= viewport.bottom());
            if let Some(previous) = hint_top {
                assert_eq!(
                    hint.top(),
                    previous,
                    "the hint must not scroll with the rows"
                );
            }
            hint_top = Some(hint.top());
        }
        let built = vcx.update(|_, cx| app.read(cx).transfer_panel.read(cx).rows_built());
        assert!(
            built < 500,
            "tree rows must remain virtualized; built {built}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn tree_marks_translated_terminal_cwd_for_windows_shells() {
        use nyaterm_files::file_browser::{FileBrowserBackendKind, file_browser_root};
        use nyaterm_files::sftp::{RemoteFilePath, SftpFileType};

        for (shell, reported, translated) in [
            ("wsl.exe", "/mnt/d/Projects", r"D:\Projects"),
            ("wsl.exe", "/home", r"\\wsl.localhost\Ubuntu\home"),
            (
                r"C:\Program Files\Git\bin\bash.exe",
                "/d/Projects",
                r"D:\Projects",
            ),
        ] {
            let test_dir = TestConfigDir::new("nyaterm-tree-translated-cwd");
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
            let current_key = vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let mut settings = app.settings.summary().clone();
                    settings.ui_file_explorer_view_mode =
                        nyaterm_core::TransferBrowserViewMode::Tree;
                    app.settings.replace_summary(settings);
                    let crate::models::SessionLaunchConfig::Local(config) =
                        &mut app.session.metadata_mut("path-test").unwrap().launch_config
                    else {
                        panic!("local session");
                    };
                    config.shell_path = Some(shell.into());
                    config.shell_args = if shell == "wsl.exe" {
                        vec!["-d".into(), "Ubuntu".into()]
                    } else {
                        Vec::new()
                    };
                    app.session.update_cwd("path-test", reported.into());
                    let root = RemoteFilePath::new(file_browser_root(
                        FileBrowserBackendKind::Local,
                        translated,
                    ));
                    let mut entry = super::super::tests_support::browser_entry(0);
                    entry.name = Path::new(translated)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    entry.path = translated.into();
                    entry.file_type = SftpFileType::Directory;
                    app.transfer.seed_tree_listing(
                        "path-test",
                        FileBrowserBackendKind::Local,
                        root.clone(),
                        std::sync::Arc::new(vec![entry]),
                    );
                    app.transfer
                        .reveal_tree_path("path-test", FileBrowserBackendKind::Local, root);
                    let tree = app.transfer.tree_presentation(Some("path-test"), true);
                    let key = tree
                        .rows
                        .iter()
                        .find(|row| row.path.display_path == translated)
                        .unwrap()
                        .key
                        .clone();
                    app.flush_transfer_panel_snapshot(cx);
                    assert_eq!(app.session.cwd("path-test"), Some(reported));
                    assert_eq!(
                        app.transfer_panel
                            .read(cx)
                            .snapshot()
                            .unwrap()
                            .browser
                            .terminal_cwd
                            .as_deref(),
                        Some(translated),
                    );
                    key
                })
            });
            draw_path_fixture(vcx);
            let marker = Box::leak(format!("tree-terminal-cwd:{current_key}").into_boxed_str());
            assert!(
                vcx.debug_bounds(marker).is_some(),
                "missing cwd marker for {shell}: {reported}"
            );
        }
    }

    #[test]
    fn path_edit_button_builds_a_focused_selected_input_and_preserves_editing_shortcuts() {
        let test_dir = TestConfigDir::new("nyaterm-path-edit");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let button = vcx
            .debug_bounds("transfer-browser-path-edit")
            .expect("edit button");
        vcx.simulate_click(button.center(), Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|window, cx| {
            let app = app.read(cx);
            let snapshot = app.transfer_panel.read(cx).snapshot().unwrap();
            let field = snapshot
                .browser
                .path_field
                .as_ref()
                .expect("snapshot must carry the path input");
            assert!(snapshot.browser.path_editing);
            assert_eq!(field.read(cx).value(cx), "/remote");
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
            let input = field.read(cx).component_state().unwrap();
            assert_eq!(input.read(cx).selected_range(), 0..7);
        });
        let bounds = vcx
            .debug_bounds("transfer-path-bar-input-shell")
            .expect("path input renders");
        assert!(bounds.size.width > px(0.));
        assert!(bounds.size.height > px(0.));
        vcx.simulate_input("/中文 folder");
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx).transfer.browser_view().path_draft,
                "/中文 folder"
            );
            assert!(app.read(cx).transfer.browser_view().search.is_empty());
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("/pasted path".to_string()));
        });
        vcx.dispatch_action(nyaterm_ui::NyaSelectAll);
        vcx.dispatch_action(nyaterm_ui::NyaPaste);
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx).transfer.browser_view().path_draft,
                "/pasted path"
            );
            assert!(app.read(cx).transfer.browser_view().search.is_empty());
        });
        vcx.simulate_keystrokes("escape");
        vcx.update(|window, cx| {
            let app = app.read(cx);
            assert!(!app.transfer.browser_view().path_editing);
            assert_eq!(app.transfer.browser_view().path, "/remote");
            assert!(app.existing_text_input("transfer.browser.path").is_none());
            assert!(app.transfer.browser_view().focus.is_focused(window));
        });
    }

    #[test]
    fn path_edit_shortcut_and_focus_restore_work_in_both_browser_views() {
        let test_dir = TestConfigDir::new("nyaterm-path-shortcut");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        for mode in [
            nyaterm_core::TransferBrowserViewMode::List,
            nyaterm_core::TransferBrowserViewMode::Tree,
        ] {
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let mut settings = app.settings.summary().clone();
                    settings.ui_file_explorer_view_mode = mode;
                    app.settings.replace_summary(settings);
                    app.flush_transfer_panel_snapshot(cx);
                    let focus = if mode == nyaterm_core::TransferBrowserViewMode::Tree {
                        app.transfer.tree_focus()
                    } else {
                        app.transfer.browser_view().focus
                    };
                    window.focus(focus, cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            vcx.simulate_keystrokes("ctrl-l");
            draw_path_fixture(vcx);
            vcx.update(|_, cx| assert!(app.read(cx).transfer.browser_view().path_editing));
            assert!(
                vcx.debug_bounds("transfer-path-bar-input-shell").is_some(),
                "path input must render in {mode:?}"
            );
            vcx.update(|window, cx| {
                let field = app
                    .read(cx)
                    .existing_text_input("transfer.browser.path")
                    .unwrap();
                assert!(
                    field.read(cx).component_focus_handle(cx).is_focused(window),
                    "path focus in {mode:?}"
                );
            });
            vcx.simulate_keystrokes("escape");
            vcx.update(|window, cx| {
                let app = app.read(cx);
                assert!(
                    !app.transfer.browser_view().path_editing,
                    "Escape must cancel in {mode:?}"
                );
                let focus = if mode == nyaterm_core::TransferBrowserViewMode::Tree {
                    app.transfer.tree_focus()
                } else {
                    app.transfer.browser_view().focus
                };
                assert!(focus.is_focused(window));
            });
        }
    }

    #[test]
    fn blank_path_stays_in_edit_mode_and_clicking_outside_cancels() {
        let test_dir = TestConfigDir::new("nyaterm-path-empty");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let display = vcx
            .debug_bounds("transfer-browser-path-display")
            .expect("path display");
        vcx.simulate_click(
            gpui::point(display.right() - px(2.), display.center().y),
            Modifiers::none(),
        );
        draw_path_fixture(vcx);
        vcx.simulate_input("   ");
        vcx.simulate_keystrokes("enter");
        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.transfer.browser_view().path_editing);
            assert_eq!(app.transfer.browser_view().path, "/remote");
        });
        vcx.simulate_click(gpui::point(px(10.), px(600.)), Modifiers::none());
        vcx.update(|_, cx| assert!(!app.read(cx).transfer.browser_view().path_editing));
    }

    #[test]
    fn path_history_click_navigates_and_enter_restores_a_directory_after_failure() {
        let test_dir = TestConfigDir::new("nyaterm-path-navigation");
        std::fs::create_dir_all(test_dir.path()).unwrap();
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let open_editor = |vcx: &mut VisualTestContext| {
            let button = vcx
                .debug_bounds("transfer-browser-path-edit")
                .expect("edit button");
            vcx.simulate_click(button.center(), Modifiers::none());
            draw_path_fixture(vcx);
        };
        open_editor(vcx);
        let history = vcx
            .debug_bounds("transfer-browser-path-history-list")
            .expect("history");
        vcx.simulate_click(history.center(), Modifiers::none());
        vcx.update(|_, cx| {
            assert!(!app.read(cx).transfer.browser_view().path_editing);
            assert!(
                app.read(cx)
                    .existing_text_input("transfer.browser.path")
                    .is_none()
            );
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .visited_history
                    .iter()
                    .any(|path| path == "/history")
            );
        });
        wait_for_browser(vcx, &app);
        open_editor(vcx);
        let target = test_dir.path().display().to_string();
        vcx.simulate_input(&target);
        vcx.simulate_keystrokes("enter");
        wait_for_browser(vcx, &app);
        vcx.update(|window, cx| {
            let app = app.read(cx);
            assert!(!app.transfer.browser_view().path_editing);
            assert_eq!(app.transfer.browser_view().path, &target);
            assert!(app.transfer.browser_view().focus.is_focused(window));
        });
        open_editor(vcx);
        vcx.simulate_input(&test_dir.path().join("does-not-exist").display().to_string());
        vcx.simulate_keystrokes("enter");
        wait_for_browser(vcx, &app);
        vcx.update(|_, cx| assert_eq!(app.read(cx).transfer.browser_view().path, &target));
    }

    fn wait_for_browser(vcx: &mut VisualTestContext, app: &Entity<NyaTermApp>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            if vcx.update(|_, cx| !app.read(cx).transfer.browser_view().loading) {
                break;
            }
            assert!(Instant::now() < deadline, "directory listing timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    fn draw_path_fixture(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    fn show_path_menu(
        vcx: &mut VisualTestContext,
        app: &Entity<NyaTermApp>,
        menu: crate::models::TransferBrowserPathMenuState,
    ) {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.transfer.open_browser_path_menu(menu);
                app.flush_transfer_panel_snapshot(cx);
                cx.notify();
            });
        });
        draw_path_fixture(vcx);
    }

    fn show_favorites_menu(vcx: &mut VisualTestContext, app: &Entity<NyaTermApp>, x: f32, y: f32) {
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_transfer_browser_favorites_menu(
                    &MouseDownEvent {
                        position: gpui::point(px(x), px(y - 18.)),
                        ..Default::default()
                    },
                    window,
                    cx,
                );
            });
        });
        draw_path_fixture(vcx);
    }

    fn activate_favorites_button(vcx: &mut VisualTestContext) {
        draw_path_fixture(vcx);
        let keystroke = gpui::Keystroke::parse("enter").unwrap();
        vcx.simulate_event(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        vcx.simulate_event(gpui::KeyUpEvent { keystroke });
        draw_path_fixture(vcx);
    }

    #[test]
    fn favorites_menu_add_remove_keyboard_and_focus_restore() {
        let test_dir = TestConfigDir::new("nyaterm-favorites-actions");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        show_favorites_menu(vcx, &app, 20., 100.);
        assert_eq!(
            vcx.debug_bounds("transfer-browser-favorites-menu")
                .unwrap()
                .size
                .width,
            px(220.)
        );
        let add = vcx
            .debug_bounds("transfer-browser-favorite-menu-add-current")
            .unwrap();
        assert_eq!(add.size.height, px(32.));
        for _ in 0..2 {
            vcx.simulate_click(add.center(), Modifiers::none());
            draw_path_fixture(vcx);
            vcx.update(|_, cx| {
                let browser = app.read(cx).transfer.browser_view();
                assert!(browser.favorites_menu.is_some());
                assert_eq!(
                    browser.favorites.iter().collect::<Vec<_>>(),
                    vec!["/remote"]
                );
            });
        }
        let remove = vcx
            .debug_bounds("transfer-browser-favorite-menu-remove-/remote")
            .unwrap();
        vcx.simulate_mouse_move(remove.center(), None, Modifiers::none());
        vcx.simulate_click(remove.center(), Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|_, cx| {
            let browser = app.read(cx).transfer.browser_view();
            assert!(browser.favorites.is_empty());
            assert!(browser.favorites_menu.is_some());
            assert_eq!(browser.path, "/remote");
        });
        // Starting from the menu container, Tab reaches add, then the path, then remove.
        vcx.simulate_keystrokes("tab");
        activate_favorites_button(vcx);
        vcx.update(|_, cx| assert_eq!(app.read(cx).transfer.browser_view().favorites.len(), 1));
        vcx.simulate_keystrokes("tab tab");
        activate_favorites_button(vcx);
        vcx.update(|_, cx| {
            assert!(app.read(cx).transfer.browser_view().favorites.is_empty());
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .favorites_menu
                    .is_some()
            );
        });
        for key in ["tab", "shift-tab", "down", "up"] {
            for _ in 0..4 {
                vcx.simulate_keystrokes(key);
                draw_path_fixture(vcx);
                vcx.update(|window, cx| {
                    assert!(
                        app.read(cx)
                            .transfer
                            .browser_view()
                            .favorites_menu
                            .as_ref()
                            .unwrap()
                            .focus
                            .contains_focused(window, cx)
                    );
                });
            }
        }
        vcx.simulate_keystrokes("escape");
        draw_path_fixture(vcx);
        vcx.update(|window, cx| {
            let browser = app.read(cx).transfer.browser_view();
            assert!(browser.favorites_menu.is_none());
            assert!(browser.focus.is_focused(window));
        });
    }

    #[test]
    fn favorites_menu_escape_restores_tree_focus() {
        let test_dir = TestConfigDir::new("nyaterm-favorites-tree-focus");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut settings = app.settings.summary().clone();
                settings.ui_file_explorer_view_mode = nyaterm_core::TransferBrowserViewMode::Tree;
                app.settings.replace_summary(settings);
                app.flush_transfer_panel_snapshot(cx);
            })
        });
        show_favorites_menu(vcx, &app, 20., 100.);
        vcx.simulate_keystrokes("escape");
        draw_path_fixture(vcx);
        vcx.update(|window, cx| {
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .favorites_menu
                    .is_none()
            );
            assert!(app.read(cx).transfer.tree_focus().is_focused(window));
        });
    }

    #[test]
    fn favorites_menu_directory_click_navigates_and_closes() {
        let test_dir = TestConfigDir::new("nyaterm-favorites-navigation");
        let target = test_dir.path().join("favorite");
        std::fs::create_dir_all(&target).unwrap();
        let target = target.display().to_string();
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.transfer.add_browser_favorite(target.clone());
            })
        });
        show_favorites_menu(vcx, &app, 20., 100.);
        let selector =
            Box::leak(format!("transfer-browser-favorite-menu-item-{target}").into_boxed_str());
        let row = vcx.debug_bounds(selector).unwrap();
        vcx.simulate_click(row.center(), Modifiers::none());
        wait_for_browser(vcx, &app);
        vcx.update(|window, cx| {
            let browser = app.read(cx).transfer.browser_view();
            assert_eq!(browser.path, &target);
            assert!(browser.favorites_menu.is_none());
            assert!(browser.focus.is_focused(window));
        });
    }

    #[test]
    fn favorites_menu_scroll_keeps_add_fixed_and_does_not_scroll_browser() {
        let test_dir = TestConfigDir::new("nyaterm-favorites-scroll");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.shell.set_viewport_size_for_test((320., 240.));
                app.transfer
                    .replace_browser_favorites((0..12).map(|i| format!("/favorite-{i}")).collect());
                app.transfer.replace_browser_entries_for_test(
                    (0..80)
                        .map(super::super::tests_support::browser_entry)
                        .collect(),
                );
                app.flush_transfer_panel_snapshot(cx);
            })
        });
        show_favorites_menu(vcx, &app, 300., 200.);
        let menu = vcx.debug_bounds("transfer-browser-favorites-menu").unwrap();
        assert!(menu.right() <= px(312.));
        assert!(menu.bottom() <= px(232.));
        let add = vcx
            .debug_bounds("transfer-browser-favorite-menu-add-current")
            .unwrap();
        let viewport = vcx
            .debug_bounds("transfer-browser-favorites-scroll")
            .unwrap();
        let list_scroll = vcx.update(|_, cx| {
            app.read(cx)
                .transfer
                .browser_view()
                .list_scroll
                .0
                .borrow()
                .base_handle
                .clone()
        });
        let before = list_scroll.offset();
        for _ in 0..2 {
            vcx.simulate_event(gpui::ScrollEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10_000.))),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            draw_path_fixture(vcx);
        }
        assert_eq!(list_scroll.offset(), before);
        assert_eq!(
            vcx.debug_bounds("transfer-browser-favorite-menu-add-current"),
            Some(add)
        );
        let last = vcx
            .debug_bounds("transfer-browser-favorite-menu-item-/favorite-11")
            .unwrap();
        assert!(last.top() >= viewport.top());
        assert!(last.bottom() <= viewport.bottom() + px(1.));
        // Keyboard focus must reveal off-screen rows without moving the pinned action.
        for _ in 0..2 {
            vcx.simulate_keystrokes("tab");
            draw_path_fixture(vcx);
        }
        let first = vcx
            .debug_bounds("transfer-browser-favorite-menu-item-/favorite-0")
            .unwrap();
        assert!(first.top() >= viewport.top());
        for _ in 0..22 {
            vcx.simulate_keystrokes("tab");
            draw_path_fixture(vcx);
        }
        let last = vcx
            .debug_bounds("transfer-browser-favorite-menu-item-/favorite-11")
            .unwrap();
        assert!(last.top() >= viewport.top());
        assert!(last.bottom() <= viewport.bottom() + px(1.));
        assert_eq!(list_scroll.offset(), before);
        assert_eq!(
            vcx.debug_bounds("transfer-browser-favorite-menu-add-current"),
            Some(add)
        );
    }

    #[test]
    fn favorites_menu_owns_pointer_gestures_and_outside_click_only_dismisses() {
        use crate::models::TransferBrowserContextTarget;
        let test_dir = TestConfigDir::new("nyaterm-favorites-pointer");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let entries = (0..8)
            .map(super::super::tests_support::browser_entry)
            .collect::<Vec<_>>();
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer
                    .replace_browser_entries_for_test(entries.clone());
                app.transfer.select_browser_entry(entries[0].identity_key());
                window.focus(app.transfer.browser_view().focus, cx);
                app.flush_transfer_panel_snapshot(cx);
            });
        });
        draw_path_fixture(vcx);
        let row = vcx
            .debug_bounds("transfer-browser-entry-/remote/entry-0001")
            .unwrap();
        let pointer = gpui::point(row.left() + px(80.), row.center().y);
        show_favorites_menu(vcx, &app, 20., f32::from(row.top()) - 40.);
        assert!(
            vcx.debug_bounds("transfer-browser-favorites-menu")
                .unwrap()
                .contains(&pointer)
        );
        for (button, click_count) in [
            (MouseButton::Left, 1),
            (MouseButton::Left, 2),
            (MouseButton::Right, 1),
            (MouseButton::Middle, 1),
        ] {
            vcx.simulate_mouse_move(pointer, None, Modifiers::none());
            vcx.simulate_event(MouseDownEvent {
                position: pointer,
                button,
                click_count,
                ..Default::default()
            });
            vcx.simulate_mouse_move(
                pointer + gpui::point(px(20.), px(5.)),
                button,
                Modifiers::none(),
            );
            vcx.simulate_event(MouseUpEvent {
                position: pointer,
                button,
                click_count,
                ..Default::default()
            });
            draw_path_fixture(vcx);
            vcx.update(|_, cx| {
                let app = app.read(cx);
                let browser = app.transfer.browser_view();
                assert!(browser.favorites_menu.is_some());
                assert_eq!(browser.path, "/remote");
                assert_eq!(
                    browser.selected_remote_path.as_deref(),
                    Some(entries[0].path.as_str())
                );
                assert_eq!(browser.selected_remote_paths.len(), 1);
                assert_eq!(
                    browser.context_target,
                    &TransferBrowserContextTarget::CurrentDirectory
                );
                assert!(browser.drag_selection.is_none());
                assert!(!app.transfer.rename_dialog_is_open());
                assert!(!cx.has_active_drag());
            });
        }
        let outside_row = vcx
            .debug_bounds("transfer-browser-entry-/remote/entry-0004")
            .unwrap();
        let outside = gpui::point(outside_row.left() + px(280.), outside_row.center().y);
        assert!(
            !vcx.debug_bounds("transfer-browser-favorites-menu")
                .unwrap()
                .contains(&outside)
        );
        vcx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::none());
        vcx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .favorites_menu
                    .is_some()
            )
        });
        vcx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.transfer.browser_view().favorites_menu.is_none());
            assert_eq!(
                app.transfer.browser_view().selected_remote_path.as_deref(),
                Some(entries[0].path.as_str())
            );
        });
        vcx.simulate_click(outside, Modifiers::none());
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .selected_remote_path
                    .as_deref(),
                Some(entries[4].path.as_str())
            );
        });
    }

    #[test]
    fn path_menu_owns_pointer_gestures_and_outside_click_only_dismisses() {
        use crate::models::{
            TransferBrowserChildrenMenuStatus, TransferBrowserContextTarget,
            TransferBrowserPathMenuKind, TransferBrowserPathMenuState,
        };
        let test_dir = TestConfigDir::new("nyaterm-path-pointer");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let entries = (0..8)
            .map(super::super::tests_support::browser_entry)
            .collect::<Vec<_>>();
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer
                    .replace_browser_entries_for_test(entries.clone());
                app.transfer.select_browser_entry(entries[0].identity_key());
                window.focus(app.transfer.browser_view().focus, cx);
                app.flush_transfer_panel_snapshot(cx);
            });
        });
        draw_path_fixture(vcx);
        let row = vcx
            .debug_bounds("transfer-browser-entry-/remote/entry-0001")
            .unwrap();
        let pointer = gpui::point(row.left() + px(80.), row.center().y);
        show_path_menu(
            vcx,
            &app,
            TransferBrowserPathMenuState {
                session_id: Some("path-test".into()),
                x: px(20.),
                y: row.top() - px(8.),
                kind: TransferBrowserPathMenuKind::Children {
                    path: "/remote".into(),
                    branch_child_path: None,
                    request_id: None,
                    status: TransferBrowserChildrenMenuStatus::Loading,
                },
            },
        );
        assert!(
            vcx.debug_bounds("transfer-browser-path-menu")
                .unwrap()
                .contains(&pointer)
        );
        for (button, click_count) in [
            (MouseButton::Left, 1),
            (MouseButton::Left, 2),
            (MouseButton::Right, 1),
            (MouseButton::Middle, 1),
        ] {
            vcx.simulate_mouse_move(pointer, None, Modifiers::none());
            vcx.simulate_event(MouseDownEvent {
                position: pointer,
                button,
                click_count,
                ..Default::default()
            });
            vcx.simulate_mouse_move(
                pointer + gpui::point(px(20.), px(5.)),
                button,
                Modifiers::none(),
            );
            vcx.simulate_event(MouseUpEvent {
                position: pointer,
                button,
                click_count,
                ..Default::default()
            });
            draw_path_fixture(vcx);
            vcx.update(|_, cx| {
                let app = app.read(cx);
                let browser = app.transfer.browser_view();
                assert!(browser.path_menu.is_some());
                assert_eq!(browser.path, "/remote");
                assert_eq!(
                    browser.selected_remote_path.as_deref(),
                    Some(entries[0].path.as_str())
                );
                assert_eq!(browser.selected_remote_paths.len(), 1);
                assert_eq!(
                    browser.context_target,
                    &TransferBrowserContextTarget::CurrentDirectory
                );
                assert!(browser.drag_selection.is_none());
                assert!(!app.transfer.rename_dialog_is_open());
                assert!(!cx.has_active_drag());
            });
        }
        let outside_row = vcx
            .debug_bounds("transfer-browser-entry-/remote/entry-0004")
            .unwrap();
        let outside = gpui::point(outside_row.left() + px(280.), outside_row.center().y);
        assert!(
            !vcx.debug_bounds("transfer-browser-path-menu")
                .unwrap()
                .contains(&outside)
        );
        vcx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::none());
        vcx.update(|_, cx| assert!(app.read(cx).transfer.browser_view().path_menu.is_some()));
        vcx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.transfer.browser_view().path_menu.is_none());
            assert_eq!(
                app.transfer.browser_view().selected_remote_path.as_deref(),
                Some(entries[0].path.as_str())
            );
        });
        vcx.simulate_click(outside, Modifiers::none());
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .selected_remote_path
                    .as_deref(),
                Some(entries[4].path.as_str())
            );
        });
    }

    #[test]
    fn path_menu_width_tracks_content_status_and_window_edges() {
        use crate::models::{
            TransferBrowserBreadcrumbSegment, TransferBrowserChildrenMenuStatus,
            TransferBrowserPathMenuKind, TransferBrowserPathMenuState,
        };
        let test_dir = TestConfigDir::new("nyaterm-path-width");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let menu = |kind| TransferBrowserPathMenuState {
            session_id: Some("path-test".into()),
            x: px(10_000.),
            y: px(100.),
            kind,
        };
        for (label, expected_width) in [("a".to_string(), 160.), ("long-name".repeat(50), 360.)] {
            show_path_menu(
                vcx,
                &app,
                menu(TransferBrowserPathMenuKind::Overflow {
                    segments: vec![TransferBrowserBreadcrumbSegment {
                        label,
                        path: "/target".into(),
                    }],
                }),
            );
            let bounds = vcx.debug_bounds("transfer-browser-path-menu").unwrap();
            assert_eq!(bounds.size.width, px(expected_width));
            let viewport = vcx.update(|window, _| window.viewport_size());
            assert!(bounds.right() <= viewport.width - px(8.));
        }
        let label = "中文目录名称".repeat(3);
        let mut entry = super::super::tests_support::browser_entry(0);
        entry.name = label.clone();
        entry.file_type = nyaterm_files::sftp::SftpFileType::Directory;
        let children = |status| TransferBrowserPathMenuKind::Children {
            path: "/remote".into(),
            branch_child_path: None,
            request_id: None,
            status,
        };
        for status in [
            TransferBrowserChildrenMenuStatus::Loading,
            TransferBrowserChildrenMenuStatus::Ready(Vec::new()),
            TransferBrowserChildrenMenuStatus::Error("failure\n".repeat(30)),
            TransferBrowserChildrenMenuStatus::Ready(vec![entry.clone()]),
        ] {
            show_path_menu(vcx, &app, menu(children(status)));
            let bounds = vcx.debug_bounds("transfer-browser-path-menu").unwrap();
            assert!((px(160.)..=px(360.)).contains(&bounds.size.width));
        }
        let measured = vcx.update(|window, cx| {
            let mut run = window.text_style().to_run(label.len());
            run.font = app.read(cx).gpui_ui_font().font();
            window
                .text_system()
                .shape_line(label.into(), window.rem_size() * 0.75, &[run], None)
                .width
        });
        let bounds = vcx.debug_bounds("transfer-browser-path-menu").unwrap();
        assert!(bounds.size.width >= (measured + px(26.)).min(px(360.)));
        let parent = test_dir.path().display().to_string();
        std::fs::create_dir_all(test_dir.path().join("directory".repeat(25))).unwrap();
        let mut loading = menu(children(TransferBrowserChildrenMenuStatus::Loading));
        if let TransferBrowserPathMenuKind::Children { path, .. } = &mut loading.kind {
            *path = parent.clone();
        }
        show_path_menu(vcx, &app, loading);
        let loading_width = vcx
            .debug_bounds("transfer-browser-path-menu")
            .unwrap()
            .size
            .width;
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.start_transfer_browser_children_job(parent, cx)
            });
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            let ready = vcx.update(|_, cx| {
                matches!(
                    app.read(cx)
                        .transfer
                        .browser_view()
                        .path_menu
                        .as_ref()
                        .map(|menu| &menu.kind),
                    Some(TransferBrowserPathMenuKind::Children {
                        status: TransferBrowserChildrenMenuStatus::Ready(_),
                        ..
                    })
                )
            });
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child directory listing timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        draw_path_fixture(vcx);
        assert!(loading_width < px(360.));
        assert_eq!(
            vcx.debug_bounds("transfer-browser-path-menu")
                .unwrap()
                .size
                .width,
            px(360.)
        );
        vcx.simulate_resize(gpui::size(px(150.), px(720.)));
        draw_path_fixture(vcx);
        let bounds = vcx.debug_bounds("transfer-browser-path-menu").unwrap();
        assert_eq!(bounds.size.width, px(134.));
        assert_eq!(bounds.left(), px(8.));
        assert!(bounds.right() <= px(142.));
    }

    #[test]
    fn path_menu_directory_click_navigates_and_escape_dismisses() {
        use crate::models::{
            TransferBrowserBreadcrumbSegment, TransferBrowserPathMenuKind,
            TransferBrowserPathMenuState,
        };
        let test_dir = TestConfigDir::new("nyaterm-path-navigation");
        let target = test_dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let target = target.display().to_string();
        let menu = TransferBrowserPathMenuState {
            session_id: Some("path-test".into()),
            x: px(20.),
            y: px(100.),
            kind: TransferBrowserPathMenuKind::Overflow {
                segments: vec![TransferBrowserBreadcrumbSegment {
                    label: "target\r\ndirectory".into(),
                    path: target.clone(),
                }],
            },
        };
        show_path_menu(vcx, &app, menu.clone());
        let row = vcx
            .debug_bounds(Box::leak(
                format!("transfer-browser-path-menu-entry-{target}").into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(row.size.height, px(28.));
        vcx.simulate_click(row.center(), Modifiers::none());
        wait_for_browser(vcx, &app);
        vcx.update(|window, cx| {
            let app = app.read(cx);
            assert!(app.transfer.browser_view().path_menu.is_none());
            assert_eq!(app.transfer.browser_view().path, &target);
            assert!(app.transfer.browser_view().focus.is_focused(window));
        });
        // A trigger press must acquire browser focus even when another surface
        // previously owned it, so Escape can dismiss the popup immediately.
        vcx.update(|window, cx| window.blur(cx));
        let trigger = vcx
            .debug_bounds(Box::leak(
                format!("transfer-browser-breadcrumb-children-{target}").into_boxed_str(),
            ))
            .unwrap();
        vcx.simulate_click(trigger.center(), Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|window, cx| {
            assert!(app.read(cx).transfer.browser_view().path_menu.is_some());
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .focus
                    .is_focused(window)
            );
        });
        vcx.simulate_keystrokes("escape");
        draw_path_fixture(vcx);
        vcx.update(|_, cx| assert!(app.read(cx).transfer.browser_view().path_menu.is_none()));
    }

    #[test]
    fn path_menu_scrolls_to_the_last_directory_and_resets_when_reopened() {
        use crate::models::{
            TransferBrowserChildrenMenuStatus, TransferBrowserPathMenuKind,
            TransferBrowserPathMenuState,
        };
        let test_dir = TestConfigDir::new("nyaterm-path-scroll");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_file_browser(&mut cx, test_dir.path());
        let entries = (0..80)
            .map(|index| {
                let mut entry = super::super::tests_support::browser_entry(index);
                entry.file_type = nyaterm_files::sftp::SftpFileType::Directory;
                entry
            })
            .collect::<Vec<_>>();
        let menu = TransferBrowserPathMenuState {
            session_id: Some("path-test".to_string()),
            x: px(20.),
            y: px(100.),
            kind: TransferBrowserPathMenuKind::Children {
                path: "/remote".to_string(),
                branch_child_path: None,
                request_id: None,
                status: TransferBrowserChildrenMenuStatus::Ready(entries),
            },
        };
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.replace_browser_entries_for_test(
                    (0..80)
                        .map(super::super::tests_support::browser_entry)
                        .collect(),
                );
                app.transfer.open_browser_path_menu(menu.clone());
                app.flush_transfer_panel_snapshot(cx);
                cx.notify();
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let viewport = vcx
            .debug_bounds("transfer-browser-path-menu-scroll")
            .expect("scroll viewport");
        let list_scroll = vcx.update(|_, cx| {
            app.read(cx)
                .transfer
                .browser_view()
                .list_scroll
                .0
                .borrow()
                .base_handle
                .clone()
        });
        let list_offset = list_scroll.offset();
        assert!(viewport.size.height <= px(324.));
        assert!(viewport.size.height > px(300.));
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .transfer_panel
                    .read(cx)
                    .snapshot()
                    .unwrap()
                    .browser
                    .expanded_children_path
                    .as_deref(),
                Some("/remote")
            )
        });
        vcx.simulate_event(gpui::ScrollEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10_000.))),
            modifiers: Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
            source: gpui::ScrollSource::Pointer,
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let last = vcx
            .debug_bounds("transfer-browser-path-menu-entry-/remote/entry-0079")
            .expect("last directory");
        assert!(last.bottom() <= viewport.bottom() + px(1.));
        assert!(last.top() >= viewport.top());
        assert_eq!(last.size.height, px(28.));
        // Scrolling beyond the popup's last row must not scroll the file list.
        vcx.simulate_event(gpui::ScrollEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10_000.))),
            ..Default::default()
        });
        draw_path_fixture(vcx);
        assert_eq!(list_scroll.offset(), list_offset);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.transfer.browser_view().path_menu_scroll.offset().y < px(0.));
                app.transfer.open_browser_path_menu(menu.clone());
                assert_eq!(
                    app.transfer.browser_view().path_menu_scroll.offset().y,
                    px(0.)
                );
                app.flush_transfer_panel_snapshot(cx);
                cx.notify();
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        // Exercise the scrollbar overlay independently of wheel scrolling.
        vcx.simulate_mouse_move(viewport.center(), None, Modifiers::none());
        draw_path_fixture(vcx);
        let thumb_start = gpui::point(viewport.right() - px(5.), viewport.top() + px(16.));
        let thumb_end = gpui::point(thumb_start.x, viewport.bottom() - px(4.));
        vcx.simulate_mouse_down(thumb_start, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(thumb_end, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_up(thumb_end, MouseButton::Left, Modifiers::none());
        draw_path_fixture(vcx);
        vcx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .transfer
                    .browser_view()
                    .path_menu_scroll
                    .offset()
                    .y
                    < px(-1_000.)
            );
        });
        assert_eq!(list_scroll.offset(), list_offset);
        vcx.simulate_click(gpui::point(px(5.), px(5.)), Modifiers::none());
        vcx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .transfer_panel
                    .read(cx)
                    .snapshot()
                    .unwrap()
                    .browser
                    .expanded_children_path
                    .is_none()
            )
        });
        for count in [2, 80] {
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let mut overflow = menu.clone();
                    overflow.kind = TransferBrowserPathMenuKind::Overflow {
                        segments: (0..count)
                            .map(|index| crate::models::TransferBrowserBreadcrumbSegment {
                                label: format!("dir-{index}"),
                                path: format!("/dir-{index}"),
                            })
                            .collect(),
                    };
                    app.transfer.open_browser_path_menu(overflow);
                    app.flush_transfer_panel_snapshot(cx);
                    cx.notify();
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let bounds = vcx.debug_bounds("transfer-browser-path-menu").unwrap();
            assert_eq!(bounds.size.height, px(if count == 2 { 72. } else { 324. }));
        }
    }

    #[test]
    fn wallpaper_load_and_clear_refresh_panel_colours_without_interaction() {
        let test_dir = TestConfigDir::new("nyaterm-wallpaper-panel-colours");
        std::fs::create_dir_all(test_dir.path()).expect("create test wallpaper directory");
        let image_path = test_dir.path().join("wallpaper.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([80, 100, 120, 255]))
            .save(&image_path)
            .expect("write test wallpaper");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.settings
                .select_background_image(image_path.display().to_string());
            app.settings.set_background_content_opacity(45);
            app.flush_connection_panel_snapshot(cx);
            app.flush_transfer_panel_snapshot(cx);
            assert_eq!(
                app.transfer_panel
                    .read(cx)
                    .snapshot()
                    .unwrap()
                    .chrome
                    .surface
                    .a,
                1.0,
            );
            app.queue_wallpaper_refresh(cx);
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.read_entity(&app, |app, _| app.wallpaper_enabled()) {
                break;
            }
            assert!(Instant::now() < deadline, "wallpaper load timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.read_entity(&app, |app, cx| {
            let connection = app
                .connection_panel
                .read(cx)
                .snapshot_key()
                .unwrap()
                .chrome();
            let transfer = app.transfer_panel.read(cx).snapshot().unwrap().chrome;
            assert_eq!(connection.transparent_surface.a, 0.0);
            assert_eq!(connection.transparent_section_header.a, 0.0);
            assert_eq!(transfer.transparent_surface.a, 0.0);
            assert_eq!(transfer.transparent_section_header.a, 0.0);
            assert_eq!(
                transfer.surface,
                app.shell_surface_color(transfer.palette.surface)
            );
        });
        cx.update_entity(&app, |app, cx| {
            app.settings.clear_background_image();
            app.queue_wallpaper_refresh(cx);
            let connection = app
                .connection_panel
                .read(cx)
                .snapshot_key()
                .unwrap()
                .chrome();
            let transfer = app.transfer_panel.read(cx).snapshot().unwrap().chrome;
            assert_eq!(connection.transparent_surface.a, 1.0);
            assert_eq!(connection.transparent_section_header.a, 1.0);
            assert_eq!(transfer.transparent_surface.a, 1.0);
            assert_eq!(transfer.transparent_section_header.a, 1.0);
            assert_eq!(transfer.surface.a, 1.0);
        });
    }

    #[test]
    fn native_material_and_content_opacity_refresh_cached_panel_colours_without_interaction() {
        use nyaterm_core::models::window_material::WindowMaterial;
        let dir = TestConfigDir::new("nyaterm-native-material-panel-colours");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, dir.path());
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            for material in [
                WindowMaterial::Blurred,
                WindowMaterial::Transparent,
                WindowMaterial::Opaque,
            ] {
                app.set_window_material(material, cx);
                app.set_background_content_opacity(45, cx);
                let native = app.shell_background().native_transparent;
                let chrome = app.transfer_panel.read(cx).snapshot().unwrap().chrome;
                assert_eq!(
                    chrome.surface,
                    app.shell_surface_color(chrome.palette.surface)
                );
                assert_eq!(chrome.transparent_surface.a, if native { 0. } else { 1. });
                if native {
                    assert!((chrome.surface.a - 0.45).abs() < 0.002);
                }
            }
            app.cancel_settings(cx);
        });
    }

    fn paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).transfer_panel.read(cx).paint_count()
    }

    #[test]
    fn ui_font_size_changes_refresh_the_transfer_snapshot_without_panel_interaction() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-font-size");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        for font_size in [12, 24, 18] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| app.set_ui_font_size_from_input(font_size, cx));
            });
            vcx.run_until_parked();
            vcx.update(|_, cx| {
                let panel = app.read(cx).transfer_panel.read(cx);
                assert_eq!(
                    panel.snapshot().unwrap().chrome.ui_font_size,
                    font_size as f32
                );
            });
        }
    }

    struct QueueHost {
        panel: Entity<super::TransferPanel>,
        width: f32,
    }

    impl Render for QueueHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let queue = self.panel.update(cx, |panel, cx| {
                super::super::queue::transfer_queue_view(panel, cx).into_any_element()
            });
            div().w(px(self.width)).h(px(240.)).child(queue)
        }
    }

    #[test]
    fn long_transfer_text_shrinks_without_displacing_status_when_panel_resizes() {
        use crate::models::{TransferJobKind, TransferJobRowSnapshot, TransferJobStatus};
        use std::path::PathBuf;
        use std::sync::Arc;

        let test_dir = TestConfigDir::new("nyaterm-transfer-queue-layout");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        let panel = cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.flush_transfer_panel_snapshot(cx);
            app.transfer_panel.clone()
        });
        let host_panel = panel.clone();
        let (host, vcx) = cx.add_window_view(move |_, _| QueueHost {
            panel: host_panel,
            width: 240.,
        });
        let vcx: &mut VisualTestContext = vcx;

        for status in [
            TransferJobStatus::Running,
            TransferJobStatus::Completed,
            TransferJobStatus::Failed,
        ] {
            vcx.update(|_, cx| {
                panel.update(cx, |panel, cx| {
                    let snapshot = panel.snapshot.as_mut().expect("flushed");
                    snapshot.has_session = true;
                    snapshot.queue.download_path = "E:/Downloads/".repeat(20);
                    snapshot.queue.rows = Arc::from([TransferJobRowSnapshot {
                        id: "long-name".to_string(),
                        kind: TransferJobKind::Download {
                            remote_path: "/remote/file.bin".to_string(),
                            raw_path_token: None,
                            local_path: PathBuf::from("file.bin"),
                        },
                        status,
                        display_name: "google-chrome-stable_current_x86_64".repeat(8),
                        detail: "long transfer error detail ".repeat(20),
                        created_at_ms: 1_785_555_123_000,
                        progress: None,
                        summary: None,
                        speed_bytes_per_sec: 1024. * 1024. * 128.,
                    }]);
                    cx.notify();
                });
            });
            let mut previous_name_width = px(0.);
            for width in [240., 320., 640.] {
                vcx.update(|window, cx| {
                    host.update(cx, |host, cx| {
                        host.width = width;
                        cx.notify();
                    });
                    _ = window.draw(cx);
                });
                vcx.run_until_parked();
                let row = vcx.debug_bounds("transfer-job-row-long-name").expect("row");
                let name = vcx
                    .debug_bounds("transfer-job-name-long-name")
                    .expect("name");
                let detail = vcx
                    .debug_bounds("transfer-job-detail-long-name")
                    .expect("detail");
                let status = vcx
                    .debug_bounds("transfer-job-status-long-name")
                    .expect("status");
                let footer = vcx
                    .debug_bounds("transfer-download-path-footer")
                    .expect("footer");

                assert!(row.right() <= px(width));
                assert!(status.right() <= row.right());
                assert!(status.size.width >= px(52.));
                assert!(name.right() < status.left());
                assert!(detail.right() < status.left());
                assert!(name.size.width > previous_name_width);
                assert!(
                    name.size.height <= px(20.),
                    "filename must remain single-line"
                );
                assert!(
                    detail.size.height <= px(16.),
                    "detail must remain single-line"
                );
                assert!(footer.right() <= px(width));
                previous_name_width = name.size.width;
            }
        }
    }

    /// 后台冲突必须自行打开窗口级对话框；激活任务在首次绘制前启动，整个过程
    /// 不发送鼠标事件，也不刷新侧栏快照，覆盖启动竞态和原先的悬浮依赖。
    #[test]
    fn duplicate_prompts_open_globally_without_panel_interaction_and_drain_in_order() {
        use nyaterm_files::sftp_transfer_types::{
            SftpDuplicateDecision, SftpDuplicateRequest, SftpTransferDirection,
        };
        use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

        let test_dir = TestConfigDir::new("nyaterm-transfer-duplicate");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.flush_transfer_panel_snapshot(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| AppHost { app: host_app });
            nya_root(host, window, cx)
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| app.start_prompt_activation_drain(window, cx));
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let broker = vcx.update(|_, cx| app.read(cx).session.prompt_duplicate_broker());
        vcx.update(|_, cx| {
            app.update(cx, |app, _| {
                // 保留旧快照作为哨兵，验证打开对话框没有借助快照刷新。
                app.transfer.set_browser_search("unflushed".to_string());
            });
        });
        let mut response_receivers = Vec::new();
        for name in ["first.txt", "second.txt"] {
            response_receivers.push(
                broker
                    .enqueue_decision_request_for_test(SftpDuplicateRequest {
                        direction: SftpTransferDirection::Upload,
                        source_path: format!("/local/{name}"),
                        target_path: format!("/remote/{name}"),
                        is_directory: false,
                    })
                    .expect("冲突请求应完成入队和唤醒"),
            );
            vcx.run_until_parked();
        }

        vcx.update(|window, cx| {
            assert!(
                window.has_active_nya_dialog(cx),
                "冲突必须主动打开全局对话框"
            );
            assert!(
                app.read(cx)
                    .transfer_panel
                    .read(cx)
                    .snapshot()
                    .unwrap()
                    .browser
                    .search
                    .is_empty(),
                "激活不能依赖侧栏快照刷新"
            );
            assert_eq!(
                app.read(cx)
                    .session
                    .prompt_active_duplicate()
                    .unwrap()
                    .request
                    .target_path,
                "/remote/first.txt"
            );
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let bounds = vcx
            .debug_bounds("duplicate-prompt-dialog")
            .expect("冲突内容应显示");
        let viewport = vcx.update(|window, _| window.viewport_size());
        assert!(bounds.size.width > px(320.), "弹框不应受侧栏宽度限制");
        assert!((bounds.center().x - viewport.width / 2.).abs() < px(1.));

        // 对话框没有默认确认项，Enter 只能保持等待，不能隐式选择 Skip。
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx));
            assert_eq!(
                app.read(cx)
                    .session
                    .prompt_active_duplicate()
                    .unwrap()
                    .request
                    .target_path,
                "/remote/first.txt"
            );
        });

        let overwrite = vcx
            .debug_bounds("duplicate-overwrite-action")
            .expect("覆盖按钮应显示");
        vcx.simulate_click(overwrite.center(), Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            response_receivers
                .remove(0)
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            SftpDuplicateDecision::Overwrite
        );
        vcx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx), "第二个冲突应自动接续");
            assert_eq!(
                app.read(cx)
                    .session
                    .prompt_active_duplicate()
                    .unwrap()
                    .request
                    .target_path,
                "/remote/second.txt"
            );
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        // 其它后台流程可能直接关闭窗口级对话框，gpui-kit 此时不会调用
        // `on_close`；生命周期兜底仍需释放第二个阻塞中的 resolver。
        vcx.update(|window, cx| window.close_nya_dialog(cx));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            response_receivers
                .remove(0)
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            SftpDuplicateDecision::Skip
        );
        vcx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.read(cx).session.prompt_active_duplicate().is_none());
        });
    }

    #[test]
    fn duplicate_batch_menu_overwrites_queued_siblings_and_leaves_other_batches_asking() {
        use crate::features::session::UploadBatchDuplicateResolver;
        use nyaterm_files::sftp_transfer_types::{
            SftpDuplicateDecision, SftpDuplicateRequest, SftpTransferDirection,
        };
        use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

        let test_dir = TestConfigDir::new("nyaterm-transfer-duplicate-batch");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| AppHost { app: host_app });
            nya_root(host, window, cx)
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| app.start_prompt_activation_drain(window, cx))
        });
        vcx.run_until_parked();
        let broker = vcx.update(|_, cx| app.read(cx).session.prompt_duplicate_broker());
        let batch = UploadBatchDuplicateResolver::new(broker.clone());
        let request = |name: &str| SftpDuplicateRequest {
            direction: SftpTransferDirection::Upload,
            source_path: format!("/local/{name}"),
            target_path: format!("/remote/{name}"),
            is_directory: true,
        };
        let first = batch.enqueue_for_test(request("first"));
        let sibling = batch.enqueue_for_test(request("second"));
        let unrelated = broker
            .enqueue_decision_request_for_test(request("unrelated"))
            .unwrap();
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let trigger = vcx
            .debug_bounds("duplicate-overwrite-menu")
            .expect("batch menu visible");
        vcx.simulate_click(trigger.center(), Modifiers::default());
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();
        assert_eq!(
            first.recv_timeout(Duration::from_secs(5)).unwrap(),
            SftpDuplicateDecision::Overwrite
        );
        assert_eq!(
            sibling.recv_timeout(Duration::from_secs(5)).unwrap(),
            SftpDuplicateDecision::Overwrite
        );
        assert_eq!(
            batch
                .enqueue_for_test(request("future"))
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            SftpDuplicateDecision::Overwrite
        );
        vcx.update(|window, cx| {
            assert!(window.has_active_nya_dialog(cx));
            assert_eq!(
                app.read(cx)
                    .session
                    .prompt_active_duplicate()
                    .unwrap()
                    .request
                    .target_path,
                "/remote/unrelated"
            );
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("duplicate-overwrite-menu").is_none(),
            "ordinary conflicts have no batch menu"
        );
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            unrelated.recv_timeout(Duration::from_secs(5)).unwrap(),
            SftpDuplicateDecision::Skip
        );
        vcx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.read(cx).session.prompt_active_duplicate().is_none());
        });
    }

    /// The point of the batch: the panel no longer rides the app's redraws.
    #[test]
    fn an_unrelated_app_notify_does_not_repaint_the_panel() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| paints(&app, cx));
        assert!(
            before > 0,
            "the panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.update(|_, cx| paints(&app, cx)),
            before,
            "five unrelated app notifies must not repaint the transfers panel"
        );
    }

    /// A flush reaches the panel inside its own transaction, not on the next paint.
    #[test]
    fn a_flush_reaches_the_snapshot_before_any_paint() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.transfer.set_browser_search("needle".to_string());
                app.flush_transfer_panel_snapshot(cx);
                let panel = app.transfer_panel.read(cx);
                assert_eq!(
                    panel.snapshot().expect("flushed").browser.search,
                    "needle",
                    "the search must be in the snapshot before anything paints"
                );
            });
        });
    }

    /// The browser list is virtualised, and the snapshot holds data rather than rows,
    /// so a large directory must still only materialise what the viewport shows.
    #[test]
    fn only_the_visible_range_is_materialised() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let entries = (0..500)
                    .map(super::super::tests_support::browser_entry)
                    .collect();
                app.transfer.replace_browser_entries_for_test(entries);
                app.flush_transfer_panel_snapshot(cx);
            });
        });
        for _ in 0..2 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }

        let built = vcx.update(|_, cx| app.read(cx).transfer_panel.read(cx).rows_built());
        assert!(
            built < 500,
            "a 500-entry listing must not materialise every row; built {built}"
        );
    }

    #[test]
    fn opening_inline_rename_builds_the_input_before_snapshotting() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());

        cx.update_entity(&app, |app, cx| {
            let entry = super::super::tests_support::browser_entry(7);
            let old_path = entry.path.clone();
            let initial_name = entry.name.clone();
            app.transfer.replace_browser_entries_for_test(vec![entry]);

            assert!(app.open_transfer_rename_for_path(old_path.clone(), cx));

            let input_id = format!("transfer.rename.{old_path}");
            let field = app
                .existing_text_input(&input_id)
                .expect("opening rename must build its input field");
            assert_eq!(field.read(cx).value(cx), initial_name);

            app.flush_transfer_panel_snapshot(cx);
            assert!(
                app.transfer_panel
                    .read(cx)
                    .snapshot()
                    .expect("flushed")
                    .browser
                    .rename_field
                    .is_some(),
                "the virtualized row must receive the rename field"
            );
        });
    }

    #[test]
    fn inline_rename_is_compact_focused_selected_and_places_the_cursor_at_the_end() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let entry = super::super::tests_support::browser_entry(7);
        let old_path = entry.path.clone();
        let initial_name = entry.name.clone();
        let input_id = "transfer.rename./remote/entry-0007";

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.replace_browser_entries_for_test(vec![entry]);
                app.open_transfer_rename_for_path_and_focus(old_path, cx);
                assert!(app.transfer.rename_dialog_is_open());
                assert!(app.transfer.rename_focus_is_pending());
                assert!(app.shell.pending_focus_clock_is_armed());
                app.flush_transfer_panel_snapshot(cx);
                app.transfer_panel.update(cx, |panel, cx| {
                    panel
                        .snapshot
                        .as_mut()
                        .expect("transfer snapshot should exist")
                        .availability = super::super::TransferBrowserAvailability::Browsable;
                    cx.notify();
                });
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        vcx.update(|window, cx| {
            assert!(!app.read(cx).transfer.rename_focus_is_pending());
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds(input_id)
            .expect("inline rename input should render");
        assert_eq!(bounds.size.height, px(24.));

        vcx.update(|window, cx| {
            let field = app
                .read(cx)
                .existing_text_input(input_id)
                .expect("rename field should still exist");
            assert!(field.read(cx).has_focus());
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
            let component = field
                .read(cx)
                .component_state()
                .expect("inline rename uses a single-line input");

            assert_eq!(component.read(cx).selected_range(), 0..initial_name.len());
            assert_eq!(component.read(cx).cursor(), initial_name.len());
        });
    }

    #[test]
    fn selected_name_renames_immediately_and_the_input_preserves_double_click() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let entry = super::super::tests_support::browser_entry(7);
        let identity = entry.identity_key();
        let click = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            },
            up: MouseUpEvent {
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            },
        });

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.transfer.replace_browser_entries_for_test(vec![entry]);
                app.transfer.select_browser_entry(identity.clone());
                assert!(app.transfer.arm_browser_rename_click(&identity, true));
                app.schedule_transfer_browser_name_rename(identity, &click, cx);
                assert!(
                    app.transfer.rename_dialog_is_open(),
                    "the click must open rename synchronously without a timer"
                );
                app.flush_transfer_panel_snapshot(cx);
                app.transfer_panel.update(cx, |panel, cx| {
                    panel
                        .snapshot
                        .as_mut()
                        .expect("transfer snapshot should exist")
                        .availability = super::super::TransferBrowserAvailability::Browsable;
                    cx.notify();
                });
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let input_bounds = vcx
            .debug_bounds("transfer.rename./remote/entry-0007")
            .expect("inline rename input should render");
        vcx.simulate_event(MouseDownEvent {
            position: input_bounds.center(),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        vcx.update(|_, cx| {
            assert!(
                !app.read(cx).transfer.rename_dialog_is_open(),
                "the second click must leave rename and preserve double-click open"
            );
        });
    }

    struct Host {
        panel: Entity<super::TransferPanel>,
    }

    impl Render for Host {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(320.)).h(px(600.)).child(self.panel.clone())
        }
    }

    /// A panel callback must not double-lease the panel.
    ///
    /// `cx.listener` leases `TransferPanel` for the whole callback, so a `with_app`
    /// that flushed inline would reach back for `panel.update` and abort the process
    /// -- not a catchable panic, a `STATUS_STACK_BUFFER_OVERRUN`. This drives
    /// `with_app` the way a listener does rather than calling `app.update` directly,
    /// which is exactly the gap that let the crash reach a build: every other test in
    /// this batch entered through the app and never held the panel lease.
    #[test]
    fn a_panel_callback_does_not_double_lease_the_panel() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_or_toggle_panel(NavItem::Transfers, cx);
            app.flush_transfer_panel_snapshot(cx);
        });
        let panel = cx.update_entity(&app, |app, _| app.transfer_panel.clone());
        let host_panel = panel.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| Host { panel: host_panel });
        let vcx: &mut gpui::VisualTestContext = vcx;
        vcx.run_until_parked();

        // Enter through the panel entity, holding its lease, the way a listener does.
        vcx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.with_app(cx, |app, cx| {
                    app.transfer.set_browser_search("leased".to_string());
                    let _ = cx;
                });
            });
        });
        vcx.run_until_parked();

        assert_eq!(
            vcx.update(|_, cx| panel
                .read(cx)
                .snapshot()
                .expect("flushed")
                .browser
                .search
                .clone()),
            "leased",
            "the deferred flush must still reach the panel"
        );
    }
}

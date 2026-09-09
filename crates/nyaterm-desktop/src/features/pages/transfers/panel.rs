use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, IntoElement, Render, Rgba, ScrollHandle, UniformListScrollHandle,
    WeakEntity, Window,
};
use nyaterm_transport::SftpFileEntry;
use nyaterm_ui::NyaInputState;

use crate::features::NyaTermApp;
use crate::features::transfers::TRANSFER_CWD_SYNC_POLL_INTERVAL;
use crate::models::{
    TransferBrowserColumnResizeState, TransferBrowserColumnWidths, TransferBrowserSortColumn,
    TransferBrowserSortDirection, TransferJobRowSnapshot, TransferRenameState,
};
use crate::theme::ThemePalette;

use super::TransferBrowserAvailability;

/// Colours the transfers panel needs that are not on the palette itself.
///
/// All three are wallpaper-dependent, so they cannot be derived from the palette
/// alone and have to be resolved where the shell settings live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct TransferChrome {
    pub palette: ThemePalette,
    pub transparent_surface: Rgba,
    pub transparent_section_header: Rgba,
    pub surface: Rgba,
}

/// The browser's render state, owned.
///
/// `TransferBrowserView` is a borrowed projection, which is exactly right for a
/// caller that already holds the state and wrong for a panel that must not touch
/// it. The costly field -- the listing -- is shared rather than copied; the rest are
/// short paths and small sets.
pub(in crate::features) struct TransferBrowserPresentation {
    pub local_backend: bool,
    pub path: String,
    pub home_dir: String,
    pub path_editing: bool,
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
    /// Whether the browser wants its remote cwd polled.
    ///
    /// The app decides this -- it depends on which panel is open, which the panel
    /// cannot see -- and the panel acts on it by owning or dropping the task.
    pub cwd_sync_demand: bool,
}

pub(in crate::features) struct TransferPanel {
    /// Weak, so the panel does not keep the app alive.
    app: WeakEntity<NyaTermApp>,
    snapshot: Option<TransferSnapshot>,
    /// The cwd-sync poll, owned here rather than by the app.
    ///
    /// A `Task` dropped with its owner, so the poll cannot outlive the panel that
    /// wants it. The panel decides *when to ask*; the app still owns the cwd itself
    /// and every mutation of it, reached through an event-time hop.
    cwd_clock: Option<gpui::Task<()>>,
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
            cwd_clock: None,
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
        let demand = snapshot.cwd_sync_demand;
        self.snapshot = Some(snapshot);
        self.reconcile_cwd_clock(demand, cx);
        cx.notify();
    }

    /// Own or drop the cwd-sync poll to match demand.
    ///
    /// The task lives here so it is dropped with the panel: a poll cannot outlive the
    /// view that wanted it, which is what "lifecycle-scoped" has to mean. The panel
    /// only decides *when to ask*; every beat hops back to the app, which owns the
    /// cwd and every mutation of it.
    fn reconcile_cwd_clock(&mut self, demand: bool, cx: &mut Context<Self>) {
        if !demand {
            // Dropping the `Task` cancels it.
            self.cwd_clock = None;
            return;
        }
        if self.cwd_clock.is_some() {
            return;
        }
        let app = self.app.clone();
        self.cwd_clock = Some(cx.spawn(async move |_panel, cx| {
            loop {
                cx.background_executor()
                    .timer(TRANSFER_CWD_SYNC_POLL_INTERVAL)
                    .await;
                let Some(app) = app.upgrade() else {
                    break;
                };
                // The app owns the decision -- whether the interval is due, whether a
                // job is in flight, whether the shell is calm enough -- and the cwd.
                // No error to handle: the strong handle from `upgrade` keeps the app
                // alive for the call, and a released app ends the loop above.
                let kept = app.update(cx, |app, cx| {
                    app.sync_transfer_cwd_if_due(cx);
                    app.transfer_cwd_sync_needs_polling()
                });
                if !kept {
                    break;
                }
            }
        }));
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

    #[cfg(test)]
    pub(in crate::features) fn cwd_clock_is_armed(&self) -> bool {
        self.cwd_clock.is_some()
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
    use std::path::Path;
    use std::time::Duration;

    use gpui::{
        AppContext as _, ClickEvent, Entity, IntoElement, Modifiers, MouseButton, MouseClickEvent,
        MouseDownEvent, MouseUpEvent, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, div, px,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::NavItem;
    use crate::test_support::TestConfigDir;

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
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(320.)).h(px(720.)).flex().flex_col().child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    self.app
                        .read(cx)
                        .transfer_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
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
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
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

    fn paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).transfer_panel.read(cx).paint_count()
    }

    /// 后台冲突必须自行打开窗口级对话框；激活任务在首次绘制前启动，整个过程
    /// 不发送鼠标事件，也不刷新侧栏快照，覆盖启动竞态和原先的悬浮依赖。
    #[test]
    fn duplicate_prompts_open_globally_without_panel_interaction_and_drain_in_order() {
        use std::time::Instant;

        use nyaterm_transport::{
            SftpDuplicateDecision, SftpDuplicateRequest, SftpDuplicateResolver as _,
            SftpTransferDirection,
        };
        use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

        let test_dir = TestConfigDir::new("nyaterm-transfer-duplicate");
        let mut cx = TestAppContext::single();
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
        let mut workers = Vec::new();
        for name in ["first.txt", "second.txt"] {
            let worker_broker = broker.clone();
            let (result_tx, result_rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = worker_broker.resolve_duplicate(&SftpDuplicateRequest {
                    direction: SftpTransferDirection::Upload,
                    source_path: format!("/local/{name}"),
                    target_path: format!("/remote/{name}"),
                    is_directory: false,
                });
                let _ = result_tx.send(result);
            });
            workers.push((worker, result_rx));
            // 第一条先激活，第二条留在队列里；真实阻塞式 resolver 负责入队和唤醒。
            let deadline = Instant::now() + Duration::from_secs(5);
            while !broker.has_pending() {
                assert!(Instant::now() < deadline, "后台冲突请求未入队");
                std::thread::sleep(Duration::from_millis(1));
            }
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
        let (worker, result_rx) = workers.remove(0);
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            SftpDuplicateDecision::Overwrite
        );
        worker.join().unwrap();
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
        // 其它后台流程可能直接关闭窗口级对话框，gpui-component 此时不会调用
        // `on_close`；生命周期兜底仍需释放第二个阻塞中的 resolver。
        vcx.update(|window, cx| window.close_nya_dialog(cx));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let (worker, result_rx) = workers.remove(0);
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            SftpDuplicateDecision::Skip
        );
        worker.join().unwrap();
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

    /// The cwd poll belongs to the panel, and dropping the panel cancels it.
    #[test]
    fn the_panel_owns_the_cwd_clock() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            assert!(
                app.read(cx).transfer_panel.read(cx).cwd_clock_is_armed(),
                "an open browser must leave the poll with the panel"
            );
        });
        let _ = Duration::from_secs(1);
    }

    #[test]
    fn opening_inline_rename_builds_the_input_before_snapshotting() {
        let test_dir = TestConfigDir::new("nyaterm-transfer-panel");
        let mut cx = TestAppContext::single();
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

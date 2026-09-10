use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{Context, Entity, FocusHandle, IntoElement, Render, Rgba, WeakEntity, Window};
use nyaterm_core::SavedConnection;
use nyaterm_ui::NyaInputState;

use crate::features::NyaTermApp;
use crate::features::connections::{
    ConnectionDragKind, ConnectionDropPosition, ConnectionDropTarget, ConnectionListRowsKey,
};
use crate::models::{ConnectionGroupEditorMode, ConnectionGroupEditorState, ConnectionSortMode};
use crate::theme::ThemePalette;

use super::list::ConnectionListRow;

/// Colours the list needs that are not on the palette itself.
///
/// Both are wallpaper-dependent, so they cannot be derived from the palette
/// alone and have to be resolved where the shell settings live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct ConnectionChrome {
    pub palette: ThemePalette,
    pub transparent_surface: Rgba,
    pub transparent_section_header: Rgba,
}

/// What has to differ before the panel is worth rebuilding.
///
/// Most of it is already maintained: `ConnectionListRowsKey` has keyed the
/// flat-row cache for a long time and is complete by construction, because the
/// catalog keeps its vectors private behind two mutators that both bump.
///
/// The rest is what that key never covered, because it keys the *row list* and
/// these change *row content*: which rows are selected, which one the arrow keys
/// are on, which folder is hovered, and where a drag is currently pointing. None
/// of them gets a counter. The two collections carry their own signal in their
/// representation (see the `PartialEq` impl below) and the rest are small enough
/// to compare as values, so no mutator anywhere has to remember to mark them.
///
/// **This is a safety net for a flush that runs, not a replacement for the
/// boundaries.** A boundary that is never reached never compares anything, so a
/// missed boundary is still a stale panel. The freshness tests drive the
/// boundaries rather than trusting this.
#[derive(Clone, Debug)]
pub(in crate::features) struct ConnectionListKey {
    rows: ConnectionListRowsKey,
    selection: Arc<HashSet<String>>,
    keyboard_active: Option<String>,
    hovered_group: Option<String>,
    drop_target: Option<ConnectionDropTarget>,
    chrome: ConnectionChrome,
}

impl ConnectionListKey {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::features) fn new(
        rows: ConnectionListRowsKey,
        selection: Arc<HashSet<String>>,
        keyboard_active: Option<String>,
        hovered_group: Option<String>,
        drop_target: Option<ConnectionDropTarget>,
        chrome: ConnectionChrome,
    ) -> Self {
        Self {
            rows,
            selection,
            keyboard_active,
            hovered_group,
            drop_target,
            chrome,
        }
    }

    pub(in crate::features) fn chrome(&self) -> ConnectionChrome {
        self.chrome
    }
}

impl PartialEq for ConnectionListKey {
    fn eq(&self, other: &Self) -> bool {
        // `selection` compares by pointer, not contents. Every write goes through
        // `Arc::make_mut` or replaces the whole set, and the live snapshot always
        // holds a clone, so a mutation cannot leave the pointer alone. That makes
        // this O(1) and, more to the point, impossible for a new mutator to get
        // wrong -- there is no counter to forget. A pointer that moved without the
        // contents changing costs one redundant rebuild, which is the safe direction
        // to be wrong in.
        Arc::ptr_eq(&self.selection, &other.selection)
            && self.rows == other.rows
            && self.keyboard_active == other.keyboard_active
            && self.hovered_group == other.hovered_group
            && self.drop_target == other.drop_target
            && self.chrome == other.chrome
    }
}

/// Everything the panel draws from: data, never elements.
///
/// The list is virtualised, so this deliberately stops at what a row is built
/// *from*. Prebuilding rows here would materialise the whole catalog in order to
/// draw the twenty rows actually on screen.
pub(in crate::features) struct ConnectionListSnapshot {
    pub(in crate::features::pages::connections) custom_icons:
        Arc<HashMap<String, Arc<gpui::RenderImage>>>,
    pub(in crate::features::pages::connections) key: ConnectionListKey,
    pub(in crate::features::pages::connections) chrome: ConnectionChrome,
    pub(in crate::features::pages::connections) rows: Arc<[ConnectionListRow]>,
    pub(in crate::features::pages::connections) widest_row: Option<usize>,
    /// Keyed, because the row builder used to reach for `connection_by_id`, which
    /// scans the catalog. Twenty visible rows meant twenty scans per frame.
    pub(in crate::features::pages::connections) connections_by_id:
        Arc<HashMap<String, SavedConnection>>,
    pub(in crate::features::pages::connections) selection: Arc<HashSet<String>>,
    pub(in crate::features::pages::connections) expanded_groups: Arc<HashSet<String>>,
    pub(in crate::features::pages::connections) keyboard_active: Option<String>,
    pub(in crate::features::pages::connections) hovered_group: Option<String>,
    pub(in crate::features::pages::connections) drop_target: Option<ConnectionDropTarget>,
    pub(in crate::features::pages::connections) group_editor: Option<ConnectionGroupEditorState>,
    pub(in crate::features::pages::connections) group_editor_field: Option<Entity<NyaInputState>>,
    pub(in crate::features::pages::connections) search_field: Entity<NyaInputState>,
    pub(in crate::features::pages::connections) search_is_empty: bool,
    pub(in crate::features::pages::connections) sort_mode: ConnectionSortMode,
    pub(in crate::features::pages::connections) store_is_empty: bool,
}

impl ConnectionListSnapshot {
    pub(in crate::features::pages::connections) fn connection(
        &self,
        connection_id: &str,
    ) -> Option<&SavedConnection> {
        self.connections_by_id.get(connection_id)
    }

    pub(in crate::features::pages::connections) fn is_selected(&self, connection_id: &str) -> bool {
        self.selection.contains(connection_id)
    }

    pub(in crate::features::pages::connections) fn is_keyboard_active(
        &self,
        connection_id: &str,
    ) -> bool {
        self.keyboard_active.as_deref() == Some(connection_id)
    }

    pub(in crate::features::pages::connections) fn group_is_expanded(
        &self,
        group_id: Option<&str>,
    ) -> bool {
        group_id.is_none_or(|id| self.expanded_groups.contains(id))
    }

    pub(in crate::features::pages::connections) fn group_is_hovered(
        &self,
        group_id: Option<&str>,
    ) -> bool {
        group_id.is_some() && self.hovered_group.as_deref() == group_id
    }

    pub(in crate::features::pages::connections) fn drop_position_for_kind_target(
        &self,
        kind: ConnectionDragKind,
        target_id: Option<&str>,
    ) -> Option<ConnectionDropPosition> {
        self.drop_target.as_ref().and_then(|target| {
            (target.kind == kind && target.id.as_deref() == target_id).then_some(target.position)
        })
    }

    pub(in crate::features::pages::connections) fn group_editor_is_renaming(
        &self,
        group_id: &str,
    ) -> bool {
        self.group_editor.as_ref().is_some_and(|draft| {
            draft.mode == ConnectionGroupEditorMode::Rename && draft.id.as_deref() == Some(group_id)
        })
    }
}

pub(in crate::features) struct ConnectionPanel {
    /// Weak, so the panel does not keep the app alive. A strong handle here plus
    /// the app's strong handle to the panel is a cycle neither side can break.
    app: WeakEntity<NyaTermApp>,
    snapshot: Option<ConnectionListSnapshot>,
    focus: FocusHandle,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    rows_built: std::cell::Cell<usize>,
}

impl ConnectionPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>, focus: FocusHandle) -> Self {
        Self {
            app,
            snapshot: None,
            focus,
            #[cfg(test)]
            paint_count: 0,
            #[cfg(test)]
            rows_built: std::cell::Cell::new(0),
        }
    }

    pub(in crate::features::pages::connections) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    #[cfg(test)]
    pub(in crate::features) fn paint_count(&self) -> usize {
        self.paint_count
    }

    /// How many rows the virtualised list has actually materialised.
    #[cfg(test)]
    pub(in crate::features::pages::connections) fn note_rows_built(&self, count: usize) {
        self.rows_built.set(self.rows_built.get() + count);
    }

    #[cfg(test)]
    fn rows_built(&self) -> usize {
        self.rows_built.get()
    }

    pub(in crate::features::pages::connections) fn snapshot(
        &self,
    ) -> Option<&ConnectionListSnapshot> {
        self.snapshot.as_ref()
    }

    pub(in crate::features) fn snapshot_key(&self) -> Option<&ConnectionListKey> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.key)
    }

    pub(in crate::features) fn icon_images_are_current(
        &self,
        images: &Arc<HashMap<String, Arc<gpui::RenderImage>>>,
    ) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| Arc::ptr_eq(&snapshot.custom_icons, images))
    }

    pub(in crate::features) fn set_snapshot(
        &mut self,
        snapshot: ConnectionListSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    /// The single entry point from a panel interaction back to authoritative app state.
    ///
    /// `cx.listener` still leases `ConnectionPanel` while the callback runs. Update
    /// `NyaTermApp` first, then defer the snapshot flush until the current GPUI effect
    /// cycle ends and the panel lease has been returned, avoiding re-entrant entity access.
    ///
    /// The return value is passed back because some event handlers decide whether to
    /// stop propagation based on the mutation result.
    pub(in crate::features::pages::connections) fn with_app<R: Default>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>) -> R,
    ) -> R {
        let Some(app) = self.app.upgrade() else {
            return R::default();
        };
        app.update(cx, |app, cx| {
            let result = f(app, cx);
            app.defer_connection_panel_snapshot_flush(cx);
            result
        })
    }

    /// A weak handle for a deferred callback -- a menu builder, a drop handler.
    ///
    /// Only for closures that run on an interaction. Render must not use it.
    pub(in crate::features::pages::connections) fn app_handle(&self) -> WeakEntity<NyaTermApp> {
        self.app.clone()
    }
}

impl Render for ConnectionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.paint_count += 1;
        }
        // Renders with **zero** `NyaTermApp` access, diagnostics included: GPUI
        // records every entity read during a draw, so one app read here would put
        // this panel back on the app's invalidation path.
        super::view::page::connections_panel(self, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;

    use gpui::{
        AppContext as _, Entity, IntoElement, Modifiers, ParentElement as _, Render, ScrollDelta,
        ScrollWheelEvent, Styled as _, TestAppContext, VisualTestContext, div, point, px,
    };
    use nyaterm_core::{AppRuntime, Group, RuntimeMode, SavedConnection};
    use nyaterm_ui::NyaInputEvent;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::connections::{
        ConnectionDragKind, ConnectionDropPosition, ConnectionDropTarget,
    };
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

    struct AppHost {
        app: Entity<NyaTermApp>,
        width: f32,
        cached: bool,
    }

    /// Mirrors what `single_side_panel` gives the real panel: a constrained,
    /// definitely-sized body, and the same cached style. Measuring `.cached()`
    /// against anything looser would not measure the shipped layout.
    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let panel = self.app.read(cx).connection_panel.clone();
            let panel = if self.cached {
                panel
                    .cached(crate::features::layout::cached_panel_style())
                    .into_any_element()
            } else {
                panel.into_any_element()
            };
            div()
                .w(px(self.width))
                .h(px(600.))
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().overflow_hidden().child(panel))
                .into_any_element()
        }
    }

    fn connection(id: &str, name: &str, group_id: Option<&str>) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            id: id.to_string(),
            name: name.to_string(),
            config: nyaterm_core::ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: group_id.map(ToOwned::to_owned),
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn group(id: &str, name: &str) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: None,
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    type Mutation = Box<dyn Fn(&mut NyaTermApp)>;

    fn select_only(app: &mut NyaTermApp, id: &str) {
        let visible = vec![id.to_string()];
        app.connection_state
            .select_list_connection(id.to_string(), &visible, false, false);
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
        connections: Vec<SavedConnection>,
        groups: Vec<Group>,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        hosted_with_layout(cx, root, connections, groups, 280., true)
    }

    fn hosted_at_width<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
        connections: Vec<SavedConnection>,
        groups: Vec<Group>,
        width: f32,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        hosted_with_layout(cx, root, connections, groups, width, false)
    }

    fn hosted_with_layout<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
        connections: Vec<SavedConnection>,
        groups: Vec<Group>,
        width: f32,
        cached: bool,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_or_toggle_panel(NavItem::Connections, cx);
            app.connection_state.replace_loaded(connections, groups);
            app.flush_connection_panel_snapshot(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost {
            app: host_app,
            width,
            cached,
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

    fn paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).connection_panel.read(cx).paint_count()
    }

    fn rows_built(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).connection_panel.read(cx).rows_built()
    }

    fn draw(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    /// The point of the whole batch: the panel no longer rides the app's redraws.
    #[test]
    fn an_unrelated_app_notify_does_not_repaint_the_panel() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "Alpha", None)],
            Vec::new(),
        );

        let before = vcx.update(|_, cx| paints(&app, cx));
        assert!(
            before > 0,
            "the panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            draw(&app, vcx);
        }
        assert_eq!(
            vcx.update(|_, cx| paints(&app, cx)),
            before,
            "five unrelated app notifies must not repaint the connections panel"
        );
    }

    /// A store reply has to reach the panel inside its own transaction, not on
    /// whatever paint happens to come next.
    #[test]
    fn a_catalog_replacement_reaches_the_snapshot_before_any_paint() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "Alpha", None)],
            Vec::new(),
        );

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.connection_state.replace_loaded(
                    vec![
                        connection("a", "Alpha", None),
                        connection("b", "Beta", None),
                    ],
                    Vec::new(),
                );
                app.flush_connection_panel_snapshot(cx);

                let panel = app.connection_panel.read(cx);
                let snapshot = panel.snapshot().expect("flushed");
                assert!(
                    snapshot.connection("b").is_some(),
                    "the new connection must be in the snapshot before anything paints"
                );
            });
        });
    }

    /// The bug the expansion guard used to have, driven through the real panel.
    #[test]
    fn a_catalog_move_under_an_unchanged_query_still_expands_the_new_match() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "prod-one", Some("g1"))],
            vec![group("g1", "One"), group("g2", "Two")],
        );

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.connection_state
                    .set_list_search_text("prod".to_string());
                app.flush_connection_panel_snapshot(cx);
                assert!(
                    app.connection_state
                        .list_expanded_groups_arc()
                        .contains("g1"),
                    "the folder holding the match must auto-expand"
                );

                // Same query, moved catalog: a second match arrives in a folder that
                // did not match before.
                app.connection_state.replace_loaded(
                    vec![
                        connection("a", "prod-one", Some("g1")),
                        connection("b", "prod-two", Some("g2")),
                    ],
                    vec![group("g1", "One"), group("g2", "Two")],
                );
                app.flush_connection_panel_snapshot(cx);
                assert!(
                    app.connection_state
                        .list_expanded_groups_arc()
                        .contains("g2"),
                    "a folder that started matching under the same query must expand"
                );
            });
        });
    }

    /// Selection, keyboard focus, hover and drop are not covered by the row-model
    /// key, so each has to move the panel key on its own.
    #[test]
    fn interaction_state_changes_reach_the_panel() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "Alpha", None)],
            Vec::new(),
        );

        for (label, mutate) in [
            (
                "selection",
                Box::new(|app: &mut NyaTermApp| select_only(app, "a")) as Mutation,
            ),
            (
                "keyboard active",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state
                        .set_list_keyboard_active_connection_id(Some("a".to_string()));
                }),
            ),
            (
                "hover",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state
                        .set_list_group_hover("g1".to_string(), true);
                }),
            ),
            (
                "drop target",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state
                        .set_list_drop_target_if_changed(ConnectionDropTarget {
                            id: Some("a".to_string()),
                            kind: ConnectionDragKind::Connection,
                            position: ConnectionDropPosition::Before,
                        });
                }),
            ),
        ] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let before = app.connection_panel.read(cx).snapshot_key().cloned();
                    mutate(app);
                    app.flush_connection_panel_snapshot(cx);
                    let after = app.connection_panel.read(cx).snapshot_key().cloned();
                    assert!(before != after, "a {label} change must move the panel key");
                });
            });
        }
    }

    /// A listener updates the panel entity while the panel is still leased.
    /// The interaction flush must wait until that lease is returned, otherwise GPUI's
    /// re-entrant entity assertion is triggered.
    #[test]
    fn panel_interaction_flushes_after_its_lease_is_released() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "Alpha", None)],
            Vec::new(),
        );
        let panel = vcx.update(|_, cx| app.read(cx).connection_panel.clone());

        vcx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.with_app(cx, |app, _| {
                    app.connection_state
                        .set_list_search_text("Alpha".to_string());
                });
                assert!(
                    panel
                        .snapshot()
                        .expect("the hosted panel should have a snapshot")
                        .search_is_empty,
                    "the snapshot must remain unchanged until the panel lease is released"
                );
            });
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let panel = app.read(cx).connection_panel.read(cx);
            assert!(
                !panel
                    .snapshot()
                    .expect("the hosted panel should have a snapshot")
                    .search_is_empty,
                "the deferred interaction flush must publish the changed search state"
            );
        });
    }

    /// A search subscription must publish the cached panel after the input event updates
    /// the authoritative connection state.
    #[test]
    fn search_input_changes_reach_the_snapshot_after_the_subscription_runs() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![connection("a", "Alpha", None)],
            Vec::new(),
        );
        let search_field = vcx.update(|_, cx| app.read(cx).connection_state.list_search_field());

        vcx.update(|_, cx| {
            search_field.update(cx, |_, cx| {
                cx.emit(NyaInputEvent::Changed("Alpha".to_string()));
            });
            assert!(
                app.read(cx)
                    .connection_panel
                    .read(cx)
                    .snapshot()
                    .expect("the hosted panel should have a snapshot")
                    .search_is_empty,
                "the snapshot must remain unchanged until the deferred flush runs"
            );
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            assert!(
                !app.read(cx)
                    .connection_panel
                    .read(cx)
                    .snapshot()
                    .expect("the hosted panel should have a snapshot")
                    .search_is_empty,
                "the search subscription must publish the changed search state"
            );
        });
    }

    /// The snapshot holds data, not rows. A big catalog must still only build the
    /// handful of rows the viewport can show.
    #[test]
    fn only_the_visible_range_is_materialised() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let connections = (0..500)
            .map(|index| connection(&format!("c{index}"), &format!("Host {index}"), None))
            .collect();
        let (app, vcx) = hosted(&mut cx, test_dir.path(), connections, Vec::new());

        let built = vcx.update(|_, cx| rows_built(&app, cx));
        assert!(
            built > 0,
            "the list must have built some rows, or this proves nothing"
        );
        assert!(
            built < 500,
            "a 500-row catalog must not materialise every row; built {built}"
        );
    }

    /// The change signal is the pointer, so this pins the representation that
    /// makes that sound: while a snapshot holds the old `Arc`, every selection
    /// mutation must allocate a new one.
    #[test]
    fn every_selection_mutation_moves_the_arc_while_a_snapshot_holds_it() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(
            &mut cx,
            test_dir.path(),
            vec![
                connection("a", "Alpha", None),
                connection("b", "Beta", None),
            ],
            Vec::new(),
        );

        let mutations: Vec<(&str, Mutation)> = vec![
            (
                "select_list_connection",
                Box::new(|app: &mut NyaTermApp| select_only(app, "a")),
            ),
            (
                "additive select",
                Box::new(|app: &mut NyaTermApp| {
                    let visible = vec!["a".to_string(), "b".to_string()];
                    app.connection_state.select_list_connection(
                        "b".to_string(),
                        &visible,
                        true,
                        false,
                    );
                }),
            ),
            (
                "clear_list_selection",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state.clear_list_selection();
                }),
            ),
            (
                "remove_list_connection_references",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state.remove_list_connection_references("a");
                }),
            ),
            (
                "clear_list_runtime_state",
                Box::new(|app: &mut NyaTermApp| {
                    app.connection_state.clear_list_runtime_state();
                }),
            ),
        ];

        for (label, mutate) in mutations {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    // The snapshot holds a clone, which is what forces `make_mut`
                    // to allocate rather than mutate in place.
                    let held = app.connection_state.list_selection();
                    mutate(app);
                    assert!(
                        !Arc::ptr_eq(&held, &app.connection_state.list_selection()),
                        "{label} must produce a new Arc while a snapshot holds the old one"
                    );
                    app.flush_connection_panel_snapshot(cx);
                });
            });
        }
    }

    /// A guard on the fixture itself: without a held clone the invariant above
    /// would be vacuous, because `make_mut` on a unique `Arc` mutates in place.
    #[test]
    fn the_selection_arc_signal_depends_on_the_snapshot_holding_a_clone() {
        let mut selection = Arc::new(HashSet::new());
        let held = selection.clone();
        Arc::make_mut(&mut selection).insert("a".to_string());
        assert!(!Arc::ptr_eq(&held, &selection));

        let mut unique = Arc::new(HashSet::new());
        let before = unique.clone();
        drop(before);
        let address = Arc::as_ptr(&unique);
        Arc::make_mut(&mut unique).insert("a".to_string());
        assert_eq!(
            address,
            Arc::as_ptr(&unique),
            "a unique Arc mutates in place, which is exactly why the snapshot must hold one"
        );
    }

    #[test]
    fn long_connection_actions_stay_at_minimum_panel_viewport_right_edge() {
        let test_dir = TestConfigDir::new("nyaterm-connection-panel");
        let mut cx = TestAppContext::single();
        let long_name = "生产环境-一段非常长且必须完整显示的-connection-name-0123456789";
        let (app, vcx) = hosted_at_width(
            &mut cx,
            test_dir.path(),
            vec![connection("long", long_name, None)],
            Vec::new(),
            160.,
        );
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                select_only(app, "long");
                app.flush_connection_panel_snapshot(cx);
            });
        });
        draw(&app, vcx);

        assert!(
            vcx.update(|_, cx| rows_built(&app, cx)) > 0,
            "the narrow viewport must still materialise its visible connection row"
        );
        let list = vcx
            .debug_bounds("connections-list-rows")
            .expect("the uniform-list viewport should be visible");
        let row = vcx
            .debug_bounds("connection-row-long")
            .expect("the long connection row should be visible");
        vcx.simulate_mouse_move(
            point(list.left() + px(12.), row.center().y),
            None,
            Modifiers::default(),
        );
        draw(&app, vcx);

        let actions_before = vcx
            .debug_bounds("connection-actions-long")
            .expect("hovering a connection should paint its actions");
        let name_before = vcx
            .debug_bounds("connection-row-name-long")
            .expect("the complete connection name should be laid out");
        assert_eq!(actions_before.right(), list.right() - px(8.));
        assert!(
            name_before.right() > list.right(),
            "the long name must overflow horizontally instead of being ellipsized"
        );

        vcx.simulate_event(ScrollWheelEvent {
            position: point(list.left() + px(12.), row.center().y),
            delta: ScrollDelta::Pixels(point(px(-180.), px(0.))),
            ..Default::default()
        });
        draw(&app, vcx);
        vcx.simulate_mouse_move(
            point(list.left() + px(12.), row.center().y),
            None,
            Modifiers::default(),
        );
        draw(&app, vcx);

        let actions_after = vcx
            .debug_bounds("connection-actions-long")
            .expect("actions should remain visible after horizontal scrolling");
        let name_after = vcx
            .debug_bounds("connection-row-name-long")
            .expect("the name should remain laid out after scrolling");
        assert_eq!(actions_after.right(), actions_before.right());
        assert!(
            name_after.left() < name_before.left(),
            "horizontal wheel input must move the long name"
        );

        // The second 24px button starts after the strip's 4px left padding and
        // the first button. Its mouse-down must not bubble to the list and clear
        // selection before its click opens the editor.
        vcx.simulate_click(
            point(actions_after.left() + px(40.), actions_after.center().y),
            Modifiers::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.connection_state.list_contains_selected_id("long"));
            assert_eq!(
                app.connection_state
                    .active_editor_draft()
                    .and_then(|draft| draft.id),
                Some("long".to_string())
            );
        });
    }
}

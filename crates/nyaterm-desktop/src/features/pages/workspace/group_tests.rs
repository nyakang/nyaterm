use gpui::{
    Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, point,
    prelude::*, px,
};
use nyaterm_core::{AiExecutionProfile, test_support::TestTempDir};
use nyaterm_transport::LocalSessionConfig;
use nyaterm_ui::NyaDropdownMenu;

use crate::features::{NyaTermApp, test_support::app_with_visible_local_session};
use crate::models::{
    SessionLaunchConfig, SessionRuntimeMetadata, SmartSplitMode, TabDockEdge, TabDockZone,
    TitleMenu,
};

struct WorkspaceFixture {
    app: Entity<NyaTermApp>,
}
struct FullWindowFixture {
    app: Entity<NyaTermApp>,
}
impl Render for FullWindowFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.app.clone())
    }
}

impl Render for WorkspaceFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu_app = self.app.clone();
        let menu = NyaDropdownMenu::new("group-stack-menu")
            .label("Tile")
            .items_dynamic(move |_, cx| {
                menu_app.update(cx, |app, cx| {
                    app.build_title_menu_items(TitleMenu::Terminal, cx)
                        .into_iter()
                        .find_map(|item| {
                            (item.test_label() == rust_i18n::t!("menu.smartSplit").as_ref())
                                .then(|| item.children().unwrap().to_vec())
                        })
                        .expect("the three smart tiling entries")
                })
            });
        let workspace = self
            .app
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(32.)).child(menu))
            .child(workspace)
    }
}

fn draw(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |_, cx| cx.notify());
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn render_groups_and_invoke_menu() {
    let root = TestTempDir::new("nyaterm-group-stack");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "s0");
    app.update(&mut cx, |app, cx| {
        app.sync_component_theme(cx);
        app.session.mark_restore_complete();
        app.shell.set_workspace_pane_layout_restored(true);
        app.shell.complete_terminal_windows_restore();
        app.reconcile_terminal_windows();
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        WorkspaceFixture { app: fixture_app }
    });
    let cx: &mut VisualTestContext = cx;
    for count in [2, 3, 4, 8] {
        eprintln!("stack regression: {count} sessions");
        app.update(cx, |app, _| {
            for index in app.session.session_order().len()..count {
                let id = format!("s{index}");
                app.session.register_session_metadata(
                    &id,
                    SessionRuntimeMetadata {
                        ssh_config: None,
                        ssh_multiplex_key: None,
                        source_connection_id: None,
                        ai_execution_profile: AiExecutionProfile::Posix,
                        launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                        disconnected: false,
                    },
                );
                app.terminal.seed_session_view(id, String::new(), "UTF-8");
            }
            app.reconcile_terminal_windows();
        });
        draw(&app, cx);
        for _ in 0..2 {
            for (index, mode) in [
                SmartSplitMode::Auto,
                SmartSplitMode::Horizontal,
                SmartSplitMode::Vertical,
            ]
            .into_iter()
            .enumerate()
            {
                // Invoke the real title-menu handlers through the dropdown's keyboard path.
                cx.simulate_click(point(px(20.), px(16.)), Default::default());
                draw(&app, cx);
                cx.simulate_keystrokes(&format!("{}enter", "down ".repeat(index + 1)));
                draw(&app, cx);
                app.read_with(cx, |app, _| {
                    let tree = app.shell.terminal_window_tree().unwrap();
                    assert_eq!(tree.collect_tab_ids().len(), count);
                    let size = app.shell.terminal_workspace_size().unwrap();
                    let expected = crate::models::TerminalWindowNode::tile_in_bounds(
                        &tree.collect_tab_ids(),
                        mode,
                        size.0,
                        size.1,
                    )
                    .unwrap();
                    assert_eq!(
                        tree.leaf_ids().len(),
                        expected.leaf_ids().len(),
                        "count={count} mode={mode:?}"
                    );
                    for group in tree.leaf_ids() {
                        let bounds = app
                            .shell
                            .terminal_split_bounds(&group)
                            .expect("rendered group");
                        assert!(
                            f32::from(bounds.size.height) > 36.,
                            "each group has room for Tabs and content"
                        );
                    }
                });
            }
        }
        app.update(cx, |app, cx| {
            app.unsplit_workspace(cx);
            let group = app.current_terminal_group().unwrap();
            app.dock_tab_on_terminal_window_leaf(
                "s1".into(),
                group,
                TabDockZone::Edge(TabDockEdge::Right),
                cx,
            );
            let group = app.current_terminal_group().unwrap();
            app.dock_tab_on_terminal_window_leaf(
                "s0".into(),
                group,
                TabDockZone::Edge(TabDockEdge::Bottom),
                cx,
            );
        });
        draw(&app, cx);
        app.update(cx, |app, cx| {
            eprintln!("stack regression: new-session menu");
            let group = app.current_terminal_group().unwrap();
            app.open_new_session_menu(
                crate::features::shell::NewSessionMenuAnchor::TerminalLeaf(group),
                cx,
            );
        });
        draw(&app, cx);
        app.update(cx, |app, cx| app.close_new_session_menu(cx));
        draw(&app, cx);
    }
    eprintln!("stack regression: full root");
    cx.update(|window, _| window.remove_window());
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, _| FullWindowFixture { app: fixture_app });
    draw(&app, cx);
}

#[test]
fn smart_tile_menu_renders_on_a_one_mib_stack_in_a_separate_process() {
    const CHILD: &str = "NYATERM_GROUP_STACK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "features::pages::workspace::group_tests::smart_tile_menu_renders_on_a_one_mib_stack_in_a_separate_process", "--nocapture"])
            .env(CHILD, "1").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().expect("start stack regression subprocess");
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > std::time::Duration::from_secs(60) {
                child.kill().unwrap();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "stack regression failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Keep the catalog cold so app construction must preload it before the
    // first translation on the one-MiB UI thread, just like isolated Windows CI.
    std::thread::Builder::new()
        .name("one-mib-workspace".into())
        .stack_size(1024 * 1024)
        .spawn(render_groups_and_invoke_menu)
        .unwrap()
        .join()
        .unwrap();
}

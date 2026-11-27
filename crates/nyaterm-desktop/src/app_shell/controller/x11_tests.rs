use crate::app_shell::controller::{DesktopController, WorkspaceWindow};
use crate::app_shell::{AppShell, AppShellStartup};
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};
use futures::executor::block_on;
use gpui::{AppContext as _, EmptyView, TestAppContext, px, size};
use nyaterm_core::x11::{X11Connection, X11Endpoint, X11Lifetime};
use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Process {
    alive: AtomicBool,
    reaped: AtomicBool,
}
impl X11Lifetime for Process {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
    }
    fn wait_stopped(&self) {
        self.reaped.store(true, Ordering::Release);
    }
}

#[test]
fn x11_is_shared_across_workspaces_and_quit_waits_for_process_cleanup() {
    let root = TestConfigDir::new("nyaterm-x11-workspace-lifetime");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
    let session_hub = cx.read(|cx| controller.read(cx).session_hub.clone());
    let ids = [WorkspaceId::new(), WorkspaceId::new()];
    let mut shells = Vec::new();
    let mut apps = Vec::new();
    let app_roots = [
        TestConfigDir::new("nyaterm-x11-window-one"),
        TestConfigDir::new("nyaterm-x11-window-two"),
    ];
    for (index, id) in ids.into_iter().enumerate() {
        let startup = cx.read(|cx| controller.read(cx).startup.for_workspace(id));
        let shell = cx.new(|cx| {
            AppShell::new(
                runtime.clone(),
                None,
                startup,
                id,
                controller.clone(),
                session_hub.clone(),
                cx,
            )
        });
        let app = app_with_visible_local_session(&mut cx, app_roots[index].path(), "x11-session");
        shell.update(&mut cx, |shell, _| shell.app = Some(app.clone()));
        let handle = cx.open_window(size(px(900.), px(700.)), |_, _| EmptyView);
        controller.update(&mut cx, |controller, _| {
            controller.windows.insert(
                id,
                WorkspaceWindow {
                    handle: handle.into(),
                    shell: shell.downgrade(),
                },
            );
        });
        shells.push(shell);
        apps.push(app);
    }
    let one = cx.read(|cx| controller.read(cx).x11_service());
    let two = cx.read(|cx| controller.read(cx).x11_service());
    assert!(Arc::ptr_eq(&one, &two));
    assert!(
        !one.manager.is_running(),
        "opening workspaces must not start X11"
    );
    let process = Arc::new(Process {
        alive: AtomicBool::new(true),
        reaped: AtomicBool::new(false),
    });
    let child = process.clone();
    let first = block_on(one.manager.acquire(move |_| {
        Ok(X11Connection {
            endpoint: X11Endpoint::Tcp {
                host: "127.0.0.1".into(),
                port: 6010,
            },
            cookie: None,
            lifetime: Some(child),
        })
    }))
    .unwrap();
    let second = block_on(
        two.manager
            .acquire(|_| panic!("a second workspace must reuse the server")),
    )
    .unwrap();
    controller.update(&mut cx, |controller, cx| {
        assert_eq!(
            controller.x11_prompt_app(ids[0], cx),
            Some(apps[0].downgrade())
        );
        controller.closing_workspaces.insert(ids[0]);
        assert_eq!(
            controller.x11_prompt_app(ids[0], cx),
            Some(apps[1].downgrade())
        );
        controller.windows.remove(&ids[0]);
    });
    drop(first);
    assert!(
        process.is_alive(),
        "closing one workspace must not stop X11"
    );
    let shutdown = controller.update(&mut cx, |controller, cx| controller.shutdown_plugins(cx));
    cx.run_until_parked();
    block_on(shutdown);
    assert!(process.reaped.load(Ordering::Acquire));
    assert!(one.manager.is_shutting_down());
    drop(second);
}

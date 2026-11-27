use super::{HttpMcpView, HttpPhase};
use crate::app_shell::{AppShellStartup, DesktopController};
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};
use gpui::{AppContext, TestAppContext};
use nyaterm_core::{
    AppRuntime, ExternalMcpSettings, RuntimeMode, WorkspaceId, ai::mcp_http::McpTransport,
};

#[test]
fn http_owner_is_explicit_and_closing_it_does_not_transfer_access() {
    let root = TestConfigDir::new("nyaterm-mcp-http-controller");
    let mut cx = TestAppContext::single();
    cx.executor().allow_parking();
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
    let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
    let app_root = TestConfigDir::new("nyaterm-mcp-http-owner");
    let app = app_with_visible_local_session(&mut cx, app_root.path(), "http-fixture");
    let first = WorkspaceId::new();
    let second = WorkspaceId::new();
    let settings = ExternalMcpSettings {
        enabled: true,
        transport: McpTransport::StreamableHttp,
        ..Default::default()
    };
    controller.update(&mut cx, |controller, cx| {
        // Locking permits ownership tests without launching a real executable.
        controller.screen_locked = true;
        controller.configure_http_mcp(first, app.downgrade(), settings.clone(), false, cx);
        assert_eq!(controller.http_mcp_view().owner, Some(first));
        assert!(matches!(controller.http_mcp.phase, HttpPhase::Locked));
        let generation = controller.http_mcp.generation;
        controller.configure_http_mcp(second, app.downgrade(), settings.clone(), false, cx);
        assert_eq!(controller.http_mcp.generation, generation);
        assert_eq!(controller.http_mcp_view().owner, Some(first));
        controller.http_mcp_owner_closed(first, cx);
        assert!(matches!(controller.http_mcp.phase, HttpPhase::NeedsOwner));
        controller.configure_http_mcp(second, app.downgrade(), settings.clone(), false, cx);
        assert_eq!(controller.http_mcp_view().owner, None);
        controller.configure_http_mcp(second, app.downgrade(), settings.clone(), true, cx);
        assert_eq!(controller.http_mcp_view().owner, Some(second));
        assert!(controller.http_mcp_config("generic").is_err());
        let mut disabled = settings;
        disabled.enabled = false;
        controller.configure_http_mcp(second, app.downgrade(), disabled, false, cx);
        let HttpMcpView { running, .. } = controller.http_mcp_view();
        assert!(!running);
        assert!(matches!(controller.http_mcp.phase, HttpPhase::Disabled));
    });
    let cleanup = app.update(&mut cx, |app, cx| app.shutdown_blocking_jobs(cx));
    cx.foreground_executor().block_test(cleanup);
}

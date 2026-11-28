//! Optional native rendering fixture; no adapter processes or credentials are used.
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::ai::agent_management::{AcpProfileProblem, AcpProfileStatus};
use crate::features::pages::settings::panel::SettingsPanel;
use crate::models::{NavItem, SettingsTab};
use crate::test_support::TestConfigDir;
use gpui::{
    App, AppContext as _, Bounds, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    WindowBounds, WindowOptions, div, px, size,
};
use nyaterm_core::{AppRuntime, RuntimeMode};
use std::time::Duration;

struct AgentSectionHost {
    panel: Entity<SettingsPanel>,
    width: f32,
}

impl Render for AgentSectionHost {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let section = self.panel.update(cx, |panel, cx| {
            panel.content_width = self.width - 40.;
            let config = panel.ai.settings_config().clone();
            let mut status = panel.ai.agent_management.clone();
            status.profiles.insert(
                "builtin-codex".into(),
                AcpProfileStatus {
                    problem: Some(AcpProfileProblem::Missing),
                    ..Default::default()
                },
            );
            panel.ai_agent_profiles_content(&config, &status, cx)
        });
        div().size_full().p_5().child(section)
    }
}

#[test]
#[ignore = "native Agent visual fixture; run alone with NYATERM_AGENT_PROBE_OUTPUT"]
fn agent_settings_native_probe() {
    crate::preload_i18n().unwrap();
    let output = std::env::var("NYATERM_AGENT_PROBE_OUTPUT").unwrap();
    let width = std::env::var("NYATERM_AGENT_PROBE_WIDTH")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(900.);
    let theme = std::env::var("NYATERM_AGENT_PROBE_THEME").unwrap_or_else(|_| "github-dark".into());
    let root = TestConfigDir::new("nyaterm-settings-agent-probe");
    let path = root.path().to_path_buf();
    gpui_platform::application()
        .with_assets(crate::test_support::ProbeAssets::new())
        .run(move |cx: &mut App| {
            let runtime = AppRuntime::from_parts_for_test(
                RuntimeMode::Portable,
                path.clone(),
                path.join("config"),
                path.join("logs"),
                path.join("cache"),
                None,
            );
            let stores = UiStoreHandles {
                startup_restore: cx.new(|_| StartupRestoreStore::default()),
                overlays: cx.new(|_| OverlayStore::default()),
            };
            let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
            app.update(cx, |app, cx| {
                app.settings.set_appearance_theme(theme);
                app.settings.set_language("zh-CN");
                app.open_page(NavItem::Settings, cx);
                app.focus_settings_tab(SettingsTab::AiAgents, cx);
                if std::env::var_os("NYATERM_AGENT_PROBE_GENERIC").is_some() {
                    for strategy in ["gemini", "opencode", "custom_acp"] {
                        app.edit_acp_profile("", "add", strategy.into(), cx);
                    }
                    let ids: Vec<_> =
                        nyaterm_core::ai::external_agents::profiles(&app.ai.pending_settings())
                            .iter()
                            .filter(|profile| profile.strategy.0 != "gemini")
                            .map(|profile| profile.id.clone())
                            .collect();
                    for id in ids {
                        if app.ai.agent_editor_view().expanded.contains(&id) {
                            app.edit_acp_profile(&id, "expanded", String::new(), cx);
                        }
                    }
                } else {
                    app.edit_acp_profile("builtin-codex", "expanded", String::new(), cx);
                }
                if std::env::var_os("NYATERM_AGENT_PROBE_GENERIC").is_none() {
                    app.edit_acp_profile("builtin-codex", "installation-help", String::new(), cx);
                }
                app.flush_settings_panel_snapshots(cx);
                app.sync_component_theme(cx);
                nyaterm_i18n::apply_locale("zh-CN");
            });
            cx.spawn(async move |cx| {
                let window = cx.update(|cx| {
                    cx.open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                                None,
                                size(px(width), px(1200.)),
                                cx,
                            ))),
                            show: true,
                            focus: false,
                            ..Default::default()
                        },
                        move |window, cx| {
                            let panel = app.read(cx).settings_panel.clone();
                            let host = cx.new(|_| AgentSectionHost { panel, width });
                            cx.new(|cx| nyaterm_ui::nya_root(host, window, cx))
                        },
                    )
                    .unwrap()
                });
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let image = cx
                    .update_window(window.into(), |_, window, _| {
                        window.render_to_image().unwrap()
                    })
                    .unwrap();
                cx.background_executor()
                    .spawn(async move {
                        image.save(output).unwrap();
                    })
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    drop(root);
}

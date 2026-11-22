//! The batch editor's own window.
//!
//! Mirrors `ConnectionEditorWindow`: same child-window contract, same header,
//! same focus policy. The form itself lives in `view.rs`.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, Subscription, Window,
    div, prelude::*, px, rgb,
};
use nyaterm_ui::{NyaWindowHandle, activate_child_window, nya_root};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
};

pub(in crate::features) struct AssetBatchEditWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    generation: u64,
    _app_subscription: Subscription,
}

impl AssetBatchEditWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let generation = app.read(cx).start_workspace.asset_batch().generation();
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            generation,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for AssetBatchEditWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self
            .app
            .read(cx)
            .start_workspace
            .asset_batch()
            .matches_generation(self.generation)
        {
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                t!("assets.batch.title").to_string(),
            )
        });
        window.set_window_title(&title);
        let content = self.app.update(cx, |app, cx| {
            app.asset_batch_edit_window_view(
                (
                    f32::from(window.viewport_size().width),
                    f32::from(window.viewport_size().height),
                ),
                cx,
            )
        });
        let close_app = self.app.clone();
        let generation = self.generation;
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| {
                    if app
                        .start_workspace
                        .asset_batch()
                        .matches_generation(generation)
                    {
                        app.dismiss_asset_batch_edit(cx);
                    }
                });
                window.remove_window();
            });
        let header_close = on_close.clone();
        focus_child_window_shell_if_idle(&self.shell_focus, window, cx);

        child_window_root(&self.shell_focus, true, on_close)
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .font(font)
            .text_size(px(font_size))
            .child(child_window_header(
                palette,
                title,
                None,
                self.chrome,
                window,
                cx,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }
}

impl NyaTermApp {
    pub(in crate::features) fn activate_asset_batch_edit_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.start_workspace.asset_batch_window_handle() else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            |app: &mut NyaTermApp| Some(app.start_workspace.asset_batch_window_slot()),
            cx,
        );
        true
    }

    pub(in crate::features) fn open_asset_batch_edit_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.activate_asset_batch_edit_window(cx) {
            return true;
        }
        if !self.start_workspace.begin_asset_batch_window_open() {
            return true;
        }
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| {
            if app
                .read(cx)
                .start_workspace
                .asset_batch_window_open_pending()
            {
                open_asset_batch_edit_window_now_from_app(app, cx);
            }
        });
        true
    }
}

fn open_asset_batch_edit_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if app
        .read(cx)
        .start_workspace
        .asset_batch_window_handle()
        .is_some()
    {
        app.update(cx, |app, cx| {
            app.start_workspace.cancel_asset_batch_window_open();
            app.activate_asset_batch_edit_window(cx);
            cx.notify();
        });
        return;
    }
    if !app.read(cx).start_workspace.asset_batch_is_open() {
        app.update(cx, |app, cx| {
            app.start_workspace.cancel_asset_batch_window_open();
            cx.notify();
        });
        return;
    }

    let title = t!("assets.batch.title").to_string();
    let spec = ChildWindowSpec::modal_editor(title, 900., 680.).min_size(420., 480.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let generation = app.read(cx).start_workspace.asset_batch().generation();
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                if app
                    .start_workspace
                    .asset_batch()
                    .matches_generation(generation)
                {
                    app.dismiss_asset_batch_edit(cx);
                }
            });
            true
        });
        let view = cx.new(|cx| AssetBatchEditWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.start_workspace.finish_asset_batch_window_open(handle);
            cx.notify();
        }
        Err(error) => {
            app.start_workspace.clear_asset_batch_window();
            app.shell.set_status(error.to_string());
            cx.notify();
        }
    });
}

#[cfg(test)]
mod native_probe {
    use super::AssetBatchEditWindow;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::assets::batch_edit::basis::compute_batch_basis;
    use crate::features::assets::batch_edit::fields::BatchFieldKey;
    use crate::features::view_widgets::ChildWindowSpec;
    use crate::test_support::TestConfigDir;
    use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::time::Duration;

    #[test]
    #[ignore = "native batch editor visual fixture; run alone with NYATERM_EDITOR_PROBE_OUTPUT"]
    fn batch_editor_native_probe() {
        crate::preload_i18n().unwrap();
        let output = std::env::var("NYATERM_EDITOR_PROBE_OUTPUT").unwrap();
        let root = TestConfigDir::new("nyaterm-batch-ui-probe");
        let path = root.path().to_path_buf();
        gpui_platform::application().run(move |cx: &mut App| {
            let runtime = AppRuntime::from_parts_for_test(RuntimeMode::Portable, path.clone(), path.join("config"), path.join("logs"), path.join("cache"), None);
            let stores = UiStoreHandles { startup_restore: cx.new(|_| StartupRestoreStore::default()), overlays: cx.new(|_| OverlayStore::default()) };
            let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
            app.update(cx, |app, cx| {
                let connections = (0..10).map(|index| serde_json::from_value(serde_json::json!({"id": format!("synthetic-{index}"), "name": format!("Synthetic {index}"), "type": if index % 2 == 0 { "ssh" } else { "ftp" }, "host": "example.invalid", "username": "fixture"})).unwrap()).collect::<Vec<nyaterm_core::SavedConnection>>();
                let ids = connections.iter().map(|connection| connection.id.clone()).collect::<Vec<_>>();
                let (basis, values, bools) = compute_batch_basis(&connections, &ids, None);
                app.start_workspace.start_asset_batch_edit(ids, basis, values, bools, Some(cx.focus_handle()));
                app.start_workspace.asset_batch_mut().initialize_scopes(&connections);
                app.connection_state.replace_loaded(connections, Vec::new());
                app.set_asset_batch_scope(None, cx);
                app.start_workspace.asset_batch_mut().set_value(BatchFieldKey::Description, "Synthetic description".into());
                if let Some(input) = app.existing_text_input(BatchFieldKey::Description.text_input_id()) {
                    input.update(cx, |input, cx| input.set_content_silent("Synthetic description", cx));
                }
                app.start_workspace.asset_batch_mut().toggle_summary();
                app.sync_component_theme(cx);
            });
            cx.spawn(async move |cx| {
                for (width, height, font_size) in [(420., 480., 12), (420., 480., 24), (900., 680., 24)] {
                    let window = cx.update(|cx| {
                        app.update(cx, |app, _| {
                            let mut summary = app.settings.summary().clone();
                            summary.ui_font_size = font_size;
                            app.settings.replace_summary(summary);
                        });
                        let app = app.clone();
                        cx.open_window(WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(height)), cx))), show: true, focus: false, ..Default::default() }, move |window, cx| {
                            let editor = cx.new(|cx| AssetBatchEditWindow::new(app, ChildWindowSpec::modal_editor("Fixture".into(), width, height).chrome(), cx));
                            cx.new(|cx| nyaterm_ui::nya_root(editor, window, cx))
                        }).unwrap()
                    });
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    let image = cx.update_window(window.into(), |_, window, _| window.render_to_image().unwrap()).unwrap();
                    let file = std::path::Path::new(&output).join(format!("batch-{width}x{height}-font{font_size}.png"));
                    cx.background_executor().spawn(async move { image.save(file).unwrap(); }).await;
                    cx.update_window(window.into(), |_, window, cx| {
                        window.dispatch_event(gpui::PlatformInput::Scroll(gpui::ScrollEvent {
                            position: gpui::point(px(width - 40.), px(240.)),
                            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-1400.))),
                            ..Default::default()
                        }), cx);
                    }).unwrap();
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    let image = cx.update_window(window.into(), |_, window, _| window.render_to_image().unwrap()).unwrap();
                    let file = std::path::Path::new(&output).join(format!("batch-{width}x{height}-font{font_size}-scrolled.png"));
                    cx.background_executor().spawn(async move { image.save(file).unwrap(); }).await;
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
        drop(root);
    }
}

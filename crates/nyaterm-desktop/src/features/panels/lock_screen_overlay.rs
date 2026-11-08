use rust_i18n::t;

use crate::window_chrome::WindowChrome;
use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, MouseButton, SharedString, Window,
    WindowControlArea, div, prelude::*, px, rgb, svg,
};
use nyaterm_ui::window_chrome::title_drag_region;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaInput};

use crate::features::view_widgets::{nyaterm_app_icon, window_control_button};
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};

const LOCK_SCREEN_TITLE_BAR_HEIGHT: f32 = 40.;
const LOCK_SCREEN_FORM_WIDTH: f32 = 320.;
const LOCK_SCREEN_CARD_PADDING: f32 = 24.;
const LOCK_SCREEN_CARD_WIDTH: f32 = LOCK_SCREEN_FORM_WIDTH + LOCK_SCREEN_CARD_PADDING * 2.;
const LOCK_SCREEN_CONTROL_HEIGHT: f32 = 44.;

impl NyaTermApp {
    pub(in crate::features) fn lock_screen_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let backdrop = palette.lock_screen_backdrop();
        let card = palette.lock_screen_card();
        let on_backdrop_text = palette.lock_screen_on_backdrop_text();
        let muted_text = palette.lock_screen_muted_text();
        let danger_text = palette.lock_screen_danger();
        let on_primary_text = palette.lock_screen_on_primary();
        let chrome_palette = palette.lock_screen_chrome_palette();
        let chrome = WindowChrome::main(window);
        let unlock_busy = self.security.screen_lock_unlock_busy();
        let password_draft = self.security.screen_lock_password_draft().to_string();
        let password_input = self.text_input(
            "lock-screen.password",
            &password_draft,
            TextInputSetup {
                placeholder: t!("settings.masterPassword").into(),
                ..TextInputSetup::masked()
            },
            cx,
        );
        password_input.update(cx, |input, cx| {
            input.set_placeholder(t!("settings.masterPassword"), cx);
            input.set_disabled(unlock_busy, cx);
        });
        let password_focus = password_input.read(cx).focus_handle();
        let password_focused = password_focus.is_focused(window)
            || password_input
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window);
        let overlay_focus = if self.settings.summary().has_master_password {
            password_focus.clone()
        } else {
            self.security.screen_lock_focus().clone()
        };
        let lock_status = if self.security.screen_lock_status().trim().is_empty()
            && !self.settings.summary().has_master_password
        {
            t!("settings.masterPasswordRequired").to_string()
        } else {
            self.security.screen_lock_status().to_string()
        };
        let status_is_error = lock_status == t!("lockScreen.wrongPassword")
            || lock_status.starts_with(t!("lockScreen.unlockFailed").as_ref());

        let password_field = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id("lock-password-input")
                    .debug_selector(|| "lock-password-input".to_string())
                    .track_focus(&password_focus)
                    .h(px(LOCK_SCREEN_CONTROL_HEIGHT))
                    .w_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if status_is_error {
                        palette.danger
                    } else if password_focused {
                        palette.focus_ring
                    } else {
                        palette.border
                    }))
                    .bg(rgb(palette.input_surface()))
                    .text_size(px(14.))
                    .text_color(rgb(palette.text))
                    .cursor_text()
                    .on_click(move |_, window, cx| {
                        if !unlock_busy {
                            window.focus(&password_focus, cx);
                        }
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(NyaInput::new(&password_input).text_size(px(14.))),
                    ),
            )
            // Reserve one line so rejection does not shift the form.
            .child(
                div()
                    .debug_selector(|| "lock-screen-status".to_string())
                    .min_h(px(18.))
                    .line_height(px(18.))
                    .text_xs()
                    .text_color(rgb(if status_is_error {
                        danger_text
                    } else {
                        muted_text
                    }))
                    .child(lock_status.clone()),
            );
        let unlock_form = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .when(self.settings.summary().has_master_password, |this| {
                this.child(password_field)
            })
            .when(!self.settings.summary().has_master_password, |this| {
                this.child(
                    div()
                        .text_center()
                        .text_xs()
                        .text_color(rgb(muted_text))
                        .child(lock_status),
                )
            })
            .child(
                div()
                    .debug_selector(|| "lock-screen-unlock".to_string())
                    .w_full()
                    .h(px(LOCK_SCREEN_CONTROL_HEIGHT))
                    .child(
                        NyaButton::new(
                            "lock-screen-unlock",
                            if unlock_busy {
                                t!("lockScreen.unlocking")
                            } else {
                                t!("lockScreen.unlock")
                            },
                        )
                        .variant(NyaButtonVariant::Primary)
                        .full_width()
                        .height(px(LOCK_SCREEN_CONTROL_HEIGHT))
                        .text_color(rgb(on_primary_text))
                        .text_size(px(14.))
                        .font_weight(FontWeight(600.))
                        .when(unlock_busy, |button| {
                            button.icon("icons/conn/spinner-arc.svg")
                        })
                        .loading(unlock_busy)
                        .disabled(unlock_busy)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_lock_unlock(cx))),
                    ),
            );

        div()
            .id(SharedString::from("lock-screen-overlay"))
            .key_context(crate::shortcuts::SCREEN_LOCKED_KEY_CONTEXT)
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .occlude()
            .flex()
            .flex_col()
            // The opaque, theme-derived backdrop must fully hide private
            // workspace content while staying below the page brightness.
            .bg(rgb(backdrop))
            .text_color(rgb(on_backdrop_text))
            .track_focus(self.security.screen_lock_focus())
            .on_any_mouse_down(|event, _, cx| {
                // Windows starts caption dragging only if the native mouse-down
                // remains unhandled. Occlusion still blocks workspace hitboxes.
                let native_title_bar = cfg!(target_os = "windows")
                    && event.button == MouseButton::Left
                    && event.position.y >= px(0.)
                    && event.position.y < px(LOCK_SCREEN_TITLE_BAR_HEIGHT);
                if !native_title_bar {
                    cx.stop_propagation();
                }
            })
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| window.focus(&overlay_focus, cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_lock_key_down(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .when(!cfg!(target_os = "macos"), |this| {
                this.child(
                    div()
                        .h(px(LOCK_SCREEN_TITLE_BAR_HEIGHT))
                        .flex_none()
                        .flex()
                        .items_center()
                        .text_color(rgb(on_backdrop_text))
                        .child(
                            div()
                                .debug_selector(|| "lock-title-drag".to_string())
                                .h_full()
                                .flex_1()
                                .map(|this| {
                                    let on_event = cx.listener(Self::handle_title_drag_event);
                                    title_drag_region(
                                        this,
                                        "lock-title-region",
                                        chrome.drag,
                                        window,
                                        cx,
                                        on_event,
                                    )
                                }),
                        )
                        .when(chrome.minimize, |this| {
                            this.child(window_control_button(
                                chrome_palette,
                                "lock-window-min",
                                "icons/window/minimize.svg",
                                WindowControlArea::Min,
                                |_, window, _| window.minimize_window(),
                            ))
                        })
                        .when(chrome.maximize, |this| {
                            this.child(window_control_button(
                                chrome_palette,
                                "lock-window-max",
                                if chrome.restore {
                                    "icons/window/restore.svg"
                                } else {
                                    "icons/window/maximize.svg"
                                },
                                WindowControlArea::Max,
                                |_, window, _| window.zoom_window(),
                            ))
                        })
                        .when(chrome.close, |this| {
                            this.child(window_control_button(
                                chrome_palette,
                                "lock-window-close",
                                "icons/window/close.svg",
                                WindowControlArea::Close,
                                cx.listener(|this, _, window, cx| {
                                    this.handle_window_close_request(window, cx);
                                }),
                            ))
                        }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .child(
                        div()
                            .id("lock-screen-card")
                            .w(px(LOCK_SCREEN_CARD_WIDTH))
                            .max_w_full()
                            .p(px(LOCK_SCREEN_CARD_PADDING))
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(card))
                            .text_color(rgb(palette.text))
                            .shadow_lg()
                            .child(
                                div()
                                    .id("lock-screen-content")
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_6()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(
                                        div()
                                            .relative()
                                            .child(
                                                div()
                                                    .size(px(72.))
                                                    .rounded_lg()
                                                    .shadow_lg()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(nyaterm_app_icon(palette, 72.)),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .right(px(-4.))
                                                    .bottom(px(-4.))
                                                    .size(px(24.))
                                                    .rounded_full()
                                                    .border_2()
                                                    .border_color(rgb(card))
                                                    .bg(rgb(palette.surface))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        svg()
                                                            .size(px(14.))
                                                            .path("icons/lock.svg")
                                                            .text_color(rgb(palette.text)),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .text_center()
                                            .child(
                                                div()
                                                    .text_size(px(20.))
                                                    .font_weight(FontWeight(600.))
                                                    .text_color(rgb(palette.text))
                                                    .child(t!("lockScreen.title")),
                                            )
                                            .when(self.security.screen_lock_is_idle(), |this| {
                                                this.child(
                                                    div()
                                                        .debug_selector(|| {
                                                            "lock-screen-idle-message".to_string()
                                                        })
                                                        .text_size(px(14.))
                                                        .line_height(px(20.))
                                                        .text_color(rgb(muted_text))
                                                        .child(t!("lockScreen.message")),
                                                )
                                            }),
                                    )
                                    .child(unlock_form),
                            ),
                    ),
            )
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Instant;

    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
        PlatformInput, Render, TestAppContext, VisualTestContext, Window, div, point, prelude::*,
        px, rgb,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode, test_support::TestTempDir};
    use nyaterm_ui::nya_root;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::text_inputs::TextInputSetup;
    use crate::features::view_widgets::modal_overlay_layer;

    struct LockScreenFixture {
        app: Entity<NyaTermApp>,
        workspace_mouse_downs: Rc<Cell<usize>>,
    }

    impl Render for LockScreenFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let overlay = self.app.update(cx, |app, cx| {
                app.lock_screen_overlay(window, cx).into_any_element()
            });
            let workspace_mouse_downs = self.workspace_mouse_downs.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .on_any_mouse_down(move |_, _, _| {
                            workspace_mouse_downs.set(workspace_mouse_downs.get() + 1);
                        }),
                )
                .child(modal_overlay_layer(overlay))
        }
    }

    fn locked_app(
        cx: &mut TestAppContext,
        root: &TestTempDir,
        has_master_password: bool,
    ) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().to_path_buf(),
            root.path().join("config"),
            root.path().join("logs"),
            root.path().join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        app.update(cx, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.has_master_password = has_master_password;
            app.settings.replace_summary(summary);
            app.sync_component_theme(cx);
            app.security.activate_screen_lock(String::new());
        });
        app
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn lock_screen_allows_native_title_drag_and_blocks_content_mouse_down(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-lock-title-drag");
        let app = locked_app(cx, &root, false);
        let workspace_mouse_downs = Rc::new(Cell::new(0));
        let fixture_mouse_downs = workspace_mouse_downs.clone();
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(|_| LockScreenFixture {
                app: fixture_app,
                workspace_mouse_downs: fixture_mouse_downs,
            });
            nya_root(fixture, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let drag_bounds = cx.debug_bounds("lock-title-drag").unwrap();
        assert_eq!(drag_bounds.size.height, px(40.));
        assert!(drag_bounds.size.width > px(100.));
        let dispatch_down = |cx: &mut VisualTestContext, position, button| {
            cx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                )
            })
        };
        assert!(dispatch_down(cx, drag_bounds.center(), MouseButton::Left).propagate);
        assert!(app.read_with(cx, |app, _| app.title_drag_active(Instant::now())));
        assert!(!dispatch_down(cx, drag_bounds.center(), MouseButton::Right).propagate);
        assert!(!dispatch_down(cx, point(px(10.), px(100.)), MouseButton::Left).propagate);
        assert_eq!(workspace_mouse_downs.get(), 0);
    }

    #[gpui::test]
    fn password_rejection_and_verification_keep_the_lock_form_stable(cx: &mut TestAppContext) {
        let root = TestTempDir::new("nyaterm-lock-form");
        let app = locked_app(cx, &root, true);
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            let fixture = cx.new(|_| LockScreenFixture {
                app: fixture_app,
                workspace_mouse_downs: Rc::new(Cell::new(0)),
            });
            nya_root(fixture, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let input_bounds = cx.debug_bounds("lock-password-input").unwrap();
        let button_bounds = cx.debug_bounds("lock-screen-unlock").unwrap();
        assert_eq!(input_bounds.size, button_bounds.size);
        assert_eq!(input_bounds.origin.x, button_bounds.origin.x);
        assert_eq!(button_bounds.origin.y - input_bounds.bottom(), px(24.));
        assert!(cx.debug_bounds("lock-screen-idle-message").is_none());

        let assert_input_border = |cx: &mut VisualTestContext, color| {
            cx.update(|window, _| {
                let quad = window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| {
                        quad.bounds == input_bounds.scale(window.scale_factor())
                            && quad.border_widths.top > px(0.).scale(window.scale_factor())
                    })
                    .expect("painted password input frame");
                assert_eq!(quad.border_color, rgb(color).into());
            });
        };
        let palette = app.read_with(cx, |app, _| app.theme_palette());
        assert_input_border(cx, palette.border);
        cx.simulate_click(input_bounds.center(), Modifiers::default());
        draw(cx);
        assert_input_border(cx, palette.focus_ring);
        cx.update(|window, cx| {
            let input = app
                .read(cx)
                .existing_text_input("lock-screen.password")
                .unwrap();
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
            window.blur(cx);
        });
        draw(cx);
        assert_input_border(cx, palette.border);

        let input = app.update(cx, |app, cx| {
            app.security
                .set_screen_lock_password_draft("draft".to_string(), String::new());
            app.text_input("lock-screen.password", "", TextInputSetup::masked(), cx)
        });
        input.update(cx, |input, cx| input.set_content_silent("draft", cx));
        draw(cx);
        app.update(cx, |app, cx| {
            app.security.clear_screen_lock_password_with_status(
                rust_i18n::t!("lockScreen.wrongPassword").to_string(),
            );
            app.clear_lock_password_input(cx);
            cx.notify();
        });
        draw(cx);
        assert!(input.read_with(cx, |input, cx| input.value(cx)).is_empty());
        assert!(app.read_with(cx, |app, _| {
            app.security.screen_lock_status() == rust_i18n::t!("lockScreen.wrongPassword")
        }));
        assert_input_border(cx, palette.danger);
        assert_eq!(
            cx.debug_bounds("lock-password-input").unwrap(),
            input_bounds
        );
        assert_eq!(
            cx.debug_bounds("lock-screen-unlock").unwrap(),
            button_bounds
        );

        let request_id = app.update(cx, |app, cx| {
            let (request_id, _) = app.security.begin_screen_lock_unlock().unwrap();
            cx.notify();
            request_id
        });
        draw(cx);
        assert_eq!(
            cx.debug_bounds("lock-password-input").unwrap(),
            input_bounds
        );
        assert_eq!(
            cx.debug_bounds("lock-screen-unlock").unwrap(),
            button_bounds
        );
        cx.update(|window, cx| window.focus(&input.read(cx).focus_handle(), cx));
        draw(cx);
        cx.simulate_keystrokes("a");
        assert!(input.read_with(cx, |input, cx| input.value(cx)).is_empty());
        cx.simulate_click(button_bounds.center(), Modifiers::default());
        assert!(app.update(cx, |app, _| {
            app.security.finish_screen_lock_unlock(request_id)
        }));

        app.update(cx, |app, cx| {
            app.security.activate_idle_screen_lock();
            cx.notify();
        });
        draw(cx);
        assert!(cx.debug_bounds("lock-screen-idle-message").is_some());
    }
}

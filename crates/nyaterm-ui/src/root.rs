//! Window-root adapter for gpui-kit.

use gpui::{
    AnyView, App, AppContext as _, Context, Font, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Window, WindowHandle, div,
};

use crate::input_focus::schedule_nya_input_blur_on_outside_pointer_down;
use crate::theme_bridge::component_typography;

/// NyaTerm's component root type.
///
/// Keep this alias inside `nyaterm-ui` so feature modules do not depend on the
/// third-party root type directly. Windows that render component-backed
/// dialogs, popovers, menus, tooltips, or inputs should use this as their first
/// view layer.
pub type NyaRoot = gpui_kit::component::Root;

pub type NyaWindowHandle = WindowHandle<NyaRoot>;

/// Install component actions and keybindings once before creating UI roots.
pub fn init_components(cx: &mut App) {
    gpui_kit::init(cx);
}

/// Override only this window's component root; restoring removes the override so
/// subsequent theme changes continue to supply the background normally.
pub fn set_nya_root_transparent(root: &mut NyaRoot, transparent: bool) {
    root.style().background = transparent.then(|| gpui::rgba(0).into());
}

struct NyaRootContent {
    view: AnyView,
}

impl NyaRootContent {
    fn new(view: impl Into<AnyView>) -> Self {
        Self { view: view.into() }
    }
}

impl Render for NyaRootContent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = component_typography(cx);
        div()
            .size_full()
            .font(typography.font)
            .text_size(typography.font_size)
            .capture_any_mouse_down(|event, window, cx| {
                schedule_nya_input_blur_on_outside_pointer_down(event.button, window, cx);
            })
            .child(self.view.clone())
    }
}

pub fn nya_root(
    view: impl Into<AnyView>,
    window: &mut Window,
    cx: &mut Context<NyaRoot>,
) -> NyaRoot {
    let content = cx.new(|_| NyaRootContent::new(view));
    // The component plugin decorates this root once with the Linux CSD frame,
    // including resize hitboxes and overlays. Child content must not add a frame.
    // Registered overlays also need the complete font stack.
    gpui_kit::component::Root::new(content, window, cx).font(component_typography(cx).font)
}

pub(crate) fn refresh_nya_root_fonts(font: Font, cx: &mut App) {
    cx.defer(move |cx| {
        for window in cx.windows() {
            if let Some(window) = window.downcast::<NyaRoot>() {
                let _ = window.update(cx, |root, _, cx| {
                    let text_style = &mut root.style().text;
                    text_style.font_family = Some(font.family.clone());
                    text_style.font_fallbacks = font.fallbacks.clone();
                    cx.notify();
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use gpui::{
        App, AppContext as _, Context, FontFallbacks, InteractiveElement as _, IntoElement,
        Modifiers, MouseButton, ParentElement as _, Render, RenderOnce,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext, Window,
        div, font, point, px,
    };

    use crate::{
        NyaDialogFooter, NyaDialogWindowExt as _, NyaInputEvent, NyaInputShell, NyaInputState,
        NyaNumberInput, NyaNumberInputOptions, NyaNumberInputState, NyaSearchInput, nya_root,
    };

    struct RootContentFixture;

    #[gpui::test]
    fn native_window_background_override_is_local_and_restores_theme_defaults(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (main, _) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| RootContentFixture);
            nya_root(content, window, cx)
        });
        let (child, _) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| RootContentFixture);
            nya_root(content, window, cx)
        });
        main.update(cx, |root, _| {
            super::set_nya_root_transparent(root, true);
            assert_eq!(root.style().background, Some(gpui::rgba(0).into()));
        });
        child.update(cx, |root, _| assert!(root.style().background.is_none()));
        main.update(cx, |root, _| {
            super::set_nya_root_transparent(root, false);
            assert!(root.style().background.is_none());
        });
    }

    #[derive(IntoElement)]
    struct TypographyCapture {
        captured: Rc<RefCell<Option<(gpui::Font, gpui::Pixels)>>>,
    }

    impl RenderOnce for TypographyCapture {
        fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
            let text_style = window.text_style();
            self.captured.replace(Some((
                text_style.font(),
                text_style.font_size.to_pixels(window.rem_size()),
            )));
            div().child("Typography capture")
        }
    }

    impl Render for RootContentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .debug_selector(|| "nya-root-content".to_string())
        }
    }

    #[gpui::test]
    fn dialog_secret_toggle_does_not_submit_and_tracks_its_stack_identity(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| RootContentFixture);
            nya_root(content, window, cx)
        });
        let submitted = Rc::new(std::cell::Cell::new(0));
        let (field, dialog) = cx.update(|window, cx| {
            let field = cx.new(|cx| {
                NyaInputState::new(cx, "fixture secret").kind(crate::NyaInputKind::Password)
            });
            let input = field.clone();
            let submitted = submitted.clone();
            let dialog = window.open_tracked_nya_dialog(cx, move |dialog, _, _| {
                let submitted = submitted.clone();
                dialog
                    .title("Password form")
                    .confirm(NyaDialogFooter::new("Cancel", "Save"))
                    .content(NyaInputShell::new("dialog-secret", &input))
                    .on_ok(move |_, _, _| {
                        submitted.set(submitted.get() + 1);
                        false
                    })
            });
            assert!(dialog.is_top(window, cx));
            (field, dialog)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&field.read(cx).component_focus_handle(cx), cx);
            window.focus_next(cx);
            _ = window.draw(cx);
            assert_ne!(
                window.focused(cx),
                Some(field.read(cx).component_focus_handle(cx))
            );
        });
        let key = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: key.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent { keystroke: key });
        cx.run_until_parked();
        assert_eq!(submitted.get(), 0);
        assert!(!field.read_with(cx, |field, _| field.is_masked()));
        cx.update(|window, cx| {
            let above = window.open_tracked_nya_dialog(cx, |dialog, _, _| dialog.title("Above"));
            assert!(dialog.is_active());
            assert!(!dialog.is_top(window, cx));
            assert!(above.is_top(window, cx));
            window.close_nya_dialog(cx);
            assert!(!above.is_active());
            assert!(dialog.is_top(window, cx));
            window.close_nya_dialog(cx);
            assert!(!dialog.is_active());
            window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Replacement"));
            assert!(!dialog.is_top(window, cx));
        });
    }

    #[gpui::test]
    fn dialog_body_preserves_focus_rings_at_both_vertical_edges(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| RootContentFixture);
            nya_root(content, window, cx)
        });

        for (kind, multi_line) in [
            (crate::NyaInputKind::Text, false),
            (crate::NyaInputKind::Password, false),
            (crate::NyaInputKind::Text, true),
        ] {
            let (first, last) = cx.update(|window, cx| {
                let first = cx.new(|cx| NyaInputState::new(cx, "First field"));
                let last = cx.new(|cx| {
                    let state = NyaInputState::new(cx, "Last field").kind(kind);
                    if multi_line {
                        state.multi_line(Some(3))
                    } else {
                        state
                    }
                });
                let first_input = first.clone();
                let last_input = last.clone();
                window.open_nya_dialog(cx, move |dialog, _, _| {
                    dialog
                        .title("Form")
                        .confirm(NyaDialogFooter::new("Cancel", "Save"))
                        .content(
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(NyaInputShell::new("first-field", &first_input))
                                .child(NyaInputShell::new("last-field", &last_input)),
                        )
                });
                (first, last)
            });
            draw(cx);
            for field in [first, last] {
                cx.update(|window, cx| {
                    window.focus(&field.read(cx).component_focus_handle(cx), cx);
                });
                draw(cx);
                cx.update(|window, cx| {
                    let color = gpui_kit::component::Theme::global(cx).ring.alpha(0.5);
                    let quads = window.painted_quads();
                    let ring: Vec<_> = quads
                        .iter()
                        .filter(|quad| quad.border_color == color)
                        .collect();
                    let bounds = ring
                        .first()
                        .expect("focused field must paint a ring")
                        .bounds;
                    // GPUI splits border-only quads into strips. Their combined
                    // paint masks must reach every edge of the outside ring.
                    let visible = ring
                        .iter()
                        .map(|quad| quad.content_mask.bounds.intersect(&quad.bounds))
                        .reduce(|left, right| left.union(&right))
                        .unwrap();
                    assert_eq!(visible, bounds, "dialog body clips the focus ring");
                });
            }
            cx.update(|window, cx| window.close_nya_dialog(cx));
        }
    }

    struct PointerContentFixture {
        down: Arc<AtomicUsize>,
        movement: Arc<AtomicUsize>,
        up: Arc<AtomicUsize>,
    }

    struct InputFocusFixture {
        first: gpui::Entity<NyaInputState>,
        second: gpui::Entity<NyaInputState>,
        ordinary: gpui::Entity<NyaInputState>,
        masked: gpui::Entity<NyaInputState>,
        number: gpui::Entity<NyaNumberInputState>,
        suffix_clicks: Arc<AtomicUsize>,
        first_blurs: usize,
        second_blurs: usize,
        _subscriptions: Vec<gpui::Subscription>,
    }

    impl InputFocusFixture {
        fn new(cx: &mut Context<Self>) -> Self {
            let first = cx.new(|cx| NyaInputState::new(cx, "").placeholder("First"));
            let second = cx.new(|cx| NyaInputState::new(cx, "").placeholder("第二个搜索框"));
            let ordinary = cx.new(|cx| NyaInputState::new(cx, "Default value"));
            let masked = cx.new(|cx| {
                NyaInputState::new(cx, "secret")
                    .placeholder("密码")
                    .masked(true)
            });
            let number =
                cx.new(|cx| NyaNumberInputState::new(cx, "1", NyaNumberInputOptions::default()));
            Self {
                first,
                second,
                ordinary,
                masked,
                number,
                suffix_clicks: Arc::new(AtomicUsize::new(0)),
                first_blurs: 0,
                second_blurs: 0,
                _subscriptions: Vec::new(),
            }
        }

        fn subscribe_to_input_events(&mut self, cx: &mut Context<Self>) {
            self._subscriptions = vec![
                cx.subscribe(&self.first, |this, _, event: &NyaInputEvent, _| {
                    if matches!(event, NyaInputEvent::Blurred(_)) {
                        this.first_blurs += 1;
                    }
                }),
                cx.subscribe(&self.second, |this, _, event: &NyaInputEvent, _| {
                    if matches!(event, NyaInputEvent::Blurred(_)) {
                        this.second_blurs += 1;
                    }
                }),
            ];
        }
    }

    impl Render for InputFocusFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let suffix_clicks = self.suffix_clicks.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div().w(px(220.)).child(
                        NyaSearchInput::new("focus-first", &self.first).trailing(
                            div()
                                .id("focus-first-suffix-action")
                                .debug_selector(|| "focus-first-suffix".to_string())
                                .size(px(18.))
                                .on_click(move |_, _, _| {
                                    suffix_clicks.fetch_add(1, Ordering::SeqCst);
                                }),
                        ),
                    ),
                )
                .child(
                    div()
                        .w(px(220.))
                        .child(NyaSearchInput::new("focus-second", &self.second)),
                )
                .child(
                    div()
                        .w(px(220.))
                        .child(NyaInputShell::new("focus-ordinary", &self.ordinary)),
                )
                .child(
                    div()
                        .w(px(220.))
                        .child(NyaInputShell::new("focus-masked", &self.masked)),
                )
                .child(
                    div()
                        .w(px(160.))
                        .h(px(32.))
                        .debug_selector(|| "focus-number".to_string())
                        .child(NyaNumberInput::new(&self.number)),
                )
                .child(
                    div()
                        .w(px(220.))
                        .h(px(80.))
                        .debug_selector(|| "focus-outside".to_string())
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation()),
                )
        }
    }

    impl Render for PointerContentFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let down = self.down.clone();
            let movement = self.movement.clone();
            let up = self.up.clone();
            div()
                .size_full()
                .on_any_mouse_down(move |_, _, _| {
                    down.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_move(move |_, _, _| {
                    movement.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_up(MouseButton::Left, move |_, _, _| {
                    up.fetch_add(1, Ordering::SeqCst);
                })
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    struct NotificationDividerFixture {
        crossing: gpui::Point<gpui::Pixels>,
        divider_downs: Rc<RefCell<usize>>,
    }

    impl Render for NotificationDividerFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let vertical_downs = self.divider_downs.clone();
            let horizontal_downs = self.divider_downs.clone();
            div()
                .size_full()
                .child(gpui::deferred(
                    div()
                        .absolute()
                        .left(self.crossing.x)
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(gpui::rgb(0xff1234))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            *vertical_downs.borrow_mut() += 1;
                        }),
                ))
                .child(gpui::deferred(
                    div()
                        .absolute()
                        .top(self.crossing.y)
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(gpui::rgb(0x12ff34))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            *horizontal_downs.borrow_mut() += 1;
                        }),
                ))
        }
    }

    #[gpui::test]
    fn notifications_paint_and_receive_pointer_input_above_deferred_dividers(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::component::WindowExt as _;
        use gpui_kit::component::notification::Notification;

        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let divider_downs = Rc::new(RefCell::new(0));
        let fixture = cx.new(|_| NotificationDividerFixture {
            crossing: point(px(0.), px(0.)),
            divider_downs: divider_downs.clone(),
        });
        let (_, cx) = cx.add_window_view(|window, cx| nya_root(fixture.clone(), window, cx));
        let toast_clicks = Rc::new(RefCell::new(0));
        cx.update(|window, cx| {
            let toast_clicks = toast_clicks.clone();
            window.push_notification(
                Notification::new()
                    .message("Copied")
                    .bg(gpui::rgb(0x1234ff))
                    .autohide(false)
                    .on_click(move |_, _, _| *toast_clicks.borrow_mut() += 1),
                cx,
            );
        });
        draw(cx);
        let toast_bounds = cx.update(|window, _| {
            let scale = window.scale_factor();
            window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.background == gpui::Background::from(gpui::rgb(0x1234ff)))
                .expect("notification background must be painted")
                .bounds
                .map(|value| px(value.0 / scale))
        });
        let crossing = toast_bounds.center();
        fixture.update(cx, |fixture, cx| {
            fixture.crossing = crossing;
            cx.notify();
        });
        draw(cx);
        cx.update(|window, _| {
            let quads = window.painted_quads();
            let order = |color| {
                quads
                    .iter()
                    .find(|quad| quad.background == gpui::Background::from(gpui::rgb(color)))
                    .expect("fixture background must be painted")
                    .order
            };
            assert!(
                order(0x1234ff) > order(0xff1234),
                "vertical divider covers toast"
            );
            assert!(
                order(0x1234ff) > order(0x12ff34),
                "horizontal divider covers toast"
            );
        });

        cx.simulate_click(crossing, Modifiers::default());
        assert_eq!(*toast_clicks.borrow(), 1);
        assert_eq!(
            *divider_downs.borrow(),
            0,
            "toast click must not start resizing"
        );
        cx.simulate_click(
            point(crossing.x, toast_bounds.bottom() + px(100.)),
            Modifiers::default(),
        );
        assert_eq!(
            *divider_downs.borrow(),
            1,
            "uncovered vertical divider must respond"
        );
        cx.simulate_click(point(px(100.), crossing.y), Modifiers::default());
        assert_eq!(
            *divider_downs.borrow(),
            2,
            "uncovered dividers must remain interactive"
        );
    }

    #[gpui::test]
    fn nya_root_renders_component_dialog_layer(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RootContentFixture);
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        draw(cx);
        assert!(cx.debug_bounds("nya-root-content").is_some());

        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| {
                dialog.content(
                    div()
                        .debug_selector(|| "nya-dialog-content".to_string())
                        .child("Dialog is visible"),
                )
            });
        });
        draw(cx);

        assert!(cx.debug_bounds("nya-dialog-content").is_some());
    }

    #[gpui::test]
    fn dialog_layer_inherits_the_application_font_fallbacks_and_size(cx: &mut TestAppContext) {
        let mut ui_font = font("Noto Sans");
        ui_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
            "Microsoft YaHei UI".to_string(),
            "Segoe UI".to_string(),
        ]));
        cx.update(|cx| {
            crate::apply_component_theme(
                crate::theme::theme_palette("github-dark"),
                ui_font.clone(),
                px(19.),
                cx,
            );
        });

        let captured = Rc::new(RefCell::new(None));
        let view = cx.new(|_| RootContentFixture);
        let (_, cx) = cx.add_window_view(move |window, cx| nya_root(view, window, cx));
        cx.update(|window, cx| {
            let captured = captured.clone();
            window.open_nya_dialog(cx, move |dialog, _, _| {
                dialog.title("Typography").content(TypographyCapture {
                    captured: captured.clone(),
                })
            });
            window.draw(cx).clear(cx);
        });

        let (rendered_font, rendered_size) = captured
            .borrow()
            .clone()
            .expect("dialog content should capture its inherited text style");
        assert_eq!(rendered_font, ui_font);
        assert_eq!(rendered_size, px(19.));
    }

    #[gpui::test]
    fn tooltip_root_tracks_ui_font_fallbacks_after_a_settings_change(cx: &mut TestAppContext) {
        let mut initial_font = font("JetBrains Mono NL");
        initial_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
            "Noto Sans SC".to_string(),
            "Microsoft YaHei UI".to_string(),
        ]));
        cx.update(|cx| {
            crate::apply_component_theme(
                crate::theme::theme_palette("github-dark"),
                initial_font.clone(),
                px(16.),
                cx,
            );
        });

        let view = cx.new(|_| RootContentFixture);
        let (root, cx) = cx.add_window_view(move |window, cx| nya_root(view, window, cx));
        root.update(cx, |root, _| {
            let style = &root.style().text;
            assert_eq!(style.font_family.as_deref(), Some("JetBrains Mono NL"));
            assert_eq!(style.font_fallbacks, initial_font.fallbacks);
        });

        let mut changed_font = font("Noto Sans SC");
        changed_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
            "Microsoft YaHei UI".to_string(),
        ]));
        cx.update(|_, cx| {
            crate::apply_component_theme(
                crate::theme::theme_palette("github-dark"),
                changed_font.clone(),
                px(18.),
                cx,
            );
        });
        cx.run_until_parked();

        root.update(cx, |root, _| {
            let style = &root.style().text;
            assert_eq!(style.font_family.as_deref(), Some("Noto Sans SC"));
            assert_eq!(style.font_fallbacks, changed_font.fallbacks);
        });
    }

    #[gpui::test]
    fn nya_dialog_blocks_lower_pointer_events_while_open_and_preserves_clicks(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let lower_down = Arc::new(AtomicUsize::new(0));
        let lower_movement = Arc::new(AtomicUsize::new(0));
        let lower_up = Arc::new(AtomicUsize::new(0));
        let dialog_clicks = Arc::new(AtomicUsize::new(0));
        let fixture_down = lower_down.clone();
        let fixture_movement = lower_movement.clone();
        let fixture_up = lower_up.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| PointerContentFixture {
                down: fixture_down,
                movement: fixture_movement,
                up: fixture_up,
            });
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        let clicks = dialog_clicks.clone();
        cx.update(|window, cx| {
            window.open_nya_dialog(cx, move |dialog, _, _| {
                let clicks = clicks.clone();
                dialog.title("Dialog").content(
                    div()
                        .id("nya-dialog-test-action")
                        .debug_selector(|| "nya-dialog-test-action".to_string())
                        .size(px(40.))
                        .on_click(move |_, _, _| {
                            clicks.fetch_add(1, Ordering::SeqCst);
                        }),
                )
            });
        });
        draw(cx);

        cx.simulate_mouse_move(point(px(12.), px(80.)), None, Modifiers::default());
        cx.simulate_mouse_up(
            point(px(12.), px(80.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(lower_movement.load(Ordering::SeqCst), 0);
        assert_eq!(lower_up.load(Ordering::SeqCst), 0);

        let action = cx
            .debug_bounds("nya-dialog-test-action")
            .expect("dialog action should be rendered");
        cx.simulate_click(action.center(), Modifiers::default());
        assert_eq!(dialog_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(lower_down.load(Ordering::SeqCst), 0);
        assert_eq!(lower_movement.load(Ordering::SeqCst), 0);
        assert_eq!(lower_up.load(Ordering::SeqCst), 0);

        cx.simulate_click(point(px(12.), px(80.)), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(lower_down.load(Ordering::SeqCst), 0);
        assert_eq!(lower_movement.load(Ordering::SeqCst), 0);
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
        });
    }

    #[gpui::test]
    fn ordinary_inputs_keep_focus_on_inside_click_and_blur_on_outside_click(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let fixture_slot = Rc::new(RefCell::new(None));
        let fixture_slot_for_window = fixture_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(InputFocusFixture::new);
            fixture.update(cx, |fixture, cx| fixture.subscribe_to_input_events(cx));
            *fixture_slot_for_window.borrow_mut() = Some(fixture.clone());
            nya_root(fixture, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let fixture = fixture_slot
            .borrow()
            .clone()
            .expect("fixture should be created");

        let first = cx.debug_bounds("focus-first").expect("first input renders");
        let second = cx
            .debug_bounds("focus-second")
            .expect("second input renders");
        let prefix = cx
            .debug_bounds("focus-first-prefix")
            .expect("search prefix renders");
        let suffix = cx
            .debug_bounds("focus-first-suffix")
            .expect("search suffix renders");
        let ordinary = cx
            .debug_bounds("focus-ordinary")
            .expect("ordinary input renders");
        let masked = cx
            .debug_bounds("focus-masked")
            .expect("masked input renders");
        let number = cx
            .debug_bounds("focus-number")
            .expect("number input renders");
        let outside = cx
            .debug_bounds("focus-outside")
            .expect("outside target renders");
        assert_eq!(first.size.height, px(32.));
        assert_eq!(second.size.height, px(32.));
        assert_eq!(ordinary.size.height, px(32.));
        assert_eq!(masked.size.height, px(32.));
        assert_eq!(number.size.height, px(32.));

        cx.simulate_click(prefix.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(fixture.read(cx).first.read(cx).has_focus());
            assert!(
                fixture
                    .read(cx)
                    .first
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            );
            assert!(window.focused(cx).is_some());
        });

        cx.simulate_click(suffix.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(fixture.read(cx).first.read(cx).has_focus());
            assert!(window.focused(cx).is_some());
            assert_eq!(fixture.read(cx).suffix_clicks.load(Ordering::SeqCst), 1);
        });

        cx.simulate_click(first.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(fixture.read(cx).first.read(cx).has_focus());
            assert!(window.focused(cx).is_some());
        });

        cx.simulate_click(second.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!fixture.read(cx).first.read(cx).has_focus());
            assert!(fixture.read(cx).second.read(cx).has_focus());
            assert!(window.focused(cx).is_some());
        });

        cx.simulate_click(ordinary.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(fixture.read(cx).ordinary.read(cx).has_focus());
            assert!(window.focused(cx).is_some());
        });

        cx.simulate_click(number.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(fixture.read(cx).number.read(cx).has_focus());
            assert!(window.focused(cx).is_some());
        });

        cx.simulate_click(outside.center(), Modifiers::default());
        draw(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!fixture.read(cx).number.read(cx).has_focus());
            assert!(window.focused(cx).is_none());
        });
    }

    #[gpui::test]
    fn input_blur_event_is_forwarded_once(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let fixture_slot = Rc::new(RefCell::new(None));
        let fixture_slot_for_window = fixture_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(InputFocusFixture::new);
            fixture.update(cx, |fixture, cx| fixture.subscribe_to_input_events(cx));
            *fixture_slot_for_window.borrow_mut() = Some(fixture.clone());
            nya_root(fixture, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let fixture = fixture_slot
            .borrow()
            .clone()
            .expect("fixture should be created");
        cx.update(|_, cx| {
            let component = fixture
                .read(cx)
                .first
                .read(cx)
                .component_state()
                .expect("first component should be initialized");
            component.update(cx, |_, cx| {
                cx.emit(gpui_kit::component::input::InputEvent::Blur);
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(fixture.read(cx).first_blurs, 1);
            assert_eq!(fixture.read(cx).second_blurs, 0);
        });
    }

    #[gpui::test]
    fn nya_confirm_dialog_renders_footer_actions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RootContentFixture);
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| {
                dialog
                    .title("Delete Item")
                    .confirm(NyaDialogFooter::new("Cancel", "Delete"))
                    .content(
                        div()
                            .debug_selector(|| "nya-confirm-dialog-content".to_string())
                            .child("Delete this item?"),
                    )
            });
        });
        draw(cx);

        assert!(cx.debug_bounds("nya-confirm-dialog-content").is_some());
        assert!(cx.debug_bounds("nya-dialog-cancel-button").is_some());
        assert!(cx.debug_bounds("nya-dialog-action-button").is_some());
    }

    #[gpui::test]
    fn nya_danger_confirm_dialog_renders_footer_actions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RootContentFixture);
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| {
                dialog
                    .title("Delete Folder")
                    .confirm(NyaDialogFooter::new("Cancel", "Delete").danger())
                    .content(
                        div()
                            .debug_selector(|| "nya-danger-dialog-content".to_string())
                            .child("Delete this folder?"),
                    )
            });
        });
        draw(cx);

        assert!(cx.debug_bounds("nya-danger-dialog-content").is_some());
        assert!(cx.debug_bounds("nya-dialog-cancel-button").is_some());
        assert!(cx.debug_bounds("nya-dialog-action-button").is_some());
    }

    #[gpui::test]
    fn nya_alert_dialog_renders_action_footer(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RootContentFixture);
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;

        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| {
                dialog.title("Notice").alert("OK").content(
                    div()
                        .debug_selector(|| "nya-alert-dialog-content".to_string())
                        .child("Something happened."),
                )
            });
        });
        draw(cx);

        assert!(cx.debug_bounds("nya-alert-dialog-content").is_some());
        assert!(cx.debug_bounds("nya-dialog-action-button").is_some());
        assert!(cx.debug_bounds("nya-dialog-cancel-button").is_none());
    }

    /// Geometry of the dialog card's own close button.
    ///
    /// `gpui-kit` paints it as an absolutely positioned overlay inset from
    /// the card's top-right corner, and gives it no debug selector, so derive it
    /// from the title and content it sits beside.
    fn dialog_close_button_center(
        cx: &mut VisualTestContext,
        width: f32,
    ) -> gpui::Point<gpui::Pixels> {
        let content = cx
            .debug_bounds("focus-dialog-content")
            .expect("dialog content should render");
        let title = cx
            .debug_bounds("focus-dialog-title")
            .expect("dialog title should render");
        // Card padding is 16px, plus the card's 1px border above the title.
        let card_left = content.origin.x - px(16.);
        let card_top = title.origin.y - px(17.);
        // The button is inset by `max(padding - 10, 8)` and is 20px across.
        point(card_left + px(width) - px(18.), card_top + px(18.))
    }

    fn open_focus_dialog(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.open_nya_dialog(cx, |dialog, _, _| {
                dialog
                    .width(400.)
                    .title(
                        div()
                            .debug_selector(|| "focus-dialog-title".to_string())
                            .child("Delete"),
                    )
                    .confirm(NyaDialogFooter::new("Cancel", "Delete").danger())
                    .content(
                        div()
                            .debug_selector(|| "focus-dialog-content".to_string())
                            .h(px(60.))
                            .child("Delete this?"),
                    )
            });
        });
        draw(cx);
        draw(cx);
    }

    /// The dialog card's close button dismisses the dialog.
    ///
    /// It does so by dispatching `Cancel`, which GPUI routes along the focused
    /// element's dispatch path, so this only holds while the dialog owns focus.
    #[gpui::test]
    fn nya_dialog_close_button_dismisses_the_dialog(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| RootContentFixture);
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        open_focus_dialog(cx);
        cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));

        let close = dialog_close_button_center(cx, 400.);
        cx.simulate_click(close, Modifiers::default());
        cx.run_until_parked();
        draw(cx);

        cx.update(|window, cx| {
            assert!(
                !window.has_active_nya_dialog(cx),
                "the close button should dismiss the dialog"
            );
        });
    }

    /// A list that owns the only context menu and aims it from its rows.
    ///
    /// This is the shape a NyaTerm list has to compose: the list carries one
    /// `NyaContextMenu`, resets the target on capture, and each row re-aims it
    /// from its own capture handler.
    struct ListContextMenuFixture {
        target: Rc<RefCell<&'static str>>,
    }

    impl Render for ListContextMenuFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let reset_target = self.target.clone();
            let row_target = self.target.clone();
            let build_target = self.target.clone();
            crate::NyaContextMenu::new_dynamic(
                div()
                    .id("list-context-menu")
                    .size_full()
                    .capture_any_mouse_down(move |event: &gpui::MouseDownEvent, _, _| {
                        if event.button == MouseButton::Right {
                            *reset_target.borrow_mut() = "list";
                        }
                    })
                    .child(
                        div()
                            .id("list-row")
                            .w(px(200.))
                            .h(px(34.))
                            .debug_selector(|| "list-row".to_string())
                            .capture_any_mouse_down(move |event: &gpui::MouseDownEvent, _, _| {
                                if event.button == MouseButton::Right {
                                    *row_target.borrow_mut() = "row";
                                }
                            })
                            // A row that swallows the bubble must still let the
                            // list's menu open.
                            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation()),
                    ),
                move |_, _| {
                    vec![crate::NyaMenuItem::action(*build_target.borrow()).on_click(
                        |_, window, cx| {
                            window.open_nya_dialog(cx, |dialog, _, _| {
                                dialog
                                    .width(400.)
                                    .title(
                                        div()
                                            .debug_selector(|| "focus-dialog-title".to_string())
                                            .child("Delete"),
                                    )
                                    .confirm(NyaDialogFooter::new("Cancel", "Delete").danger())
                                    .content(
                                        div()
                                            .debug_selector(|| "focus-dialog-content".to_string())
                                            .h(px(60.))
                                            .child("Delete this?"),
                                    )
                            });
                        },
                    )]
                },
            )
        }
    }

    fn right_click(cx: &mut VisualTestContext, position: gpui::Point<gpui::Pixels>) {
        cx.simulate_event(gpui::MouseDownEvent {
            button: MouseButton::Right,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: MouseButton::Right,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        cx.run_until_parked();
        draw(cx);
        draw(cx);
    }

    /// A dialog opened from a context-menu item stays dismissable.
    ///
    /// `ContextMenu` re-focuses its own menu on every layout pass while it is
    /// open, and a dialog is dismissed by dispatching `Cancel`/`Confirm` along the
    /// focused element's path. A menu that is still open therefore takes focus
    /// back on the frame after the dialog appears, and the close button, the
    /// cancel button and the confirm button all stop responding - permanently,
    /// since the menu goes on taking it. A list must own exactly one menu so the
    /// item click always dismisses the one that opened.
    #[gpui::test]
    fn dialog_from_a_list_context_menu_item_stays_dismissable(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let target = Rc::new(RefCell::new("none"));
        let fixture_target = target.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| ListContextMenuFixture {
                target: fixture_target,
            });
            nya_root(view, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        let row = cx.debug_bounds("list-row").expect("row should render");

        // Pressing empty space below the row keeps the list as the target.
        right_click(cx, point(row.center().x, row.bottom() + px(120.)));
        assert_eq!(*target.borrow(), "list");

        // Pressing the row re-aims the same menu at the row.
        right_click(cx, row.center());
        assert_eq!(*target.borrow(), "row");

        for round in 1..=2 {
            if round > 1 {
                right_click(cx, row.center());
            }
            cx.simulate_keystrokes("down enter");
            cx.run_until_parked();
            draw(cx);
            cx.update(|window, cx| {
                assert!(
                    window.has_active_nya_dialog(cx),
                    "round {round}: the menu item should open the dialog"
                );
            });

            // Let frames go by: a menu left open would take focus back here.
            for _ in 0..4 {
                draw(cx);
                cx.run_until_parked();
            }

            let close = dialog_close_button_center(cx, 400.);
            cx.simulate_click(close, Modifiers::default());
            cx.run_until_parked();
            draw(cx);
            cx.executor().advance_clock(Duration::from_millis(400));
            cx.run_until_parked();
            draw(cx);

            cx.update(|window, cx| {
                assert!(
                    !window.has_active_nya_dialog(cx),
                    "round {round}: the close button should dismiss the dialog"
                );
            });
        }
    }
}

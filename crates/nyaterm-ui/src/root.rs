//! Window-root adapter for gpui-kit.

use gpui::{
    AnyView, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowHandle, deferred, div,
};

use crate::input_focus::schedule_nya_input_blur_on_outside_pointer_down;
use crate::theme_bridge::component_typography;

/// The base deferred priority at the component level.
///
/// The drag divider on the main interface uses the default priority `0`,
/// while Select/Popover inside components typically use `1` or `2`.
/// The component root layer also uses `1`, so it can cover the main interface divider,
/// while internal popups are added to the deferred queue after the parent layer and
/// can still appear above the dialog card.
const COMPONENT_OVERLAY_PRIORITY: usize = 1;

/// NyaTerm's component root type.
///
/// Keep this alias inside `nyaterm-ui` so feature modules do not depend on the
/// third-party root type directly. Windows that render component-backed
/// dialogs, popovers, menus, tooltips, or inputs should use this as their first
/// view layer.
pub type NyaRoot = gpui_kit::component::Root;

pub type NyaWindowHandle = WindowHandle<NyaRoot>;

struct NyaRootContent {
    view: AnyView,
}

impl NyaRootContent {
    fn new(view: impl Into<AnyView>) -> Self {
        Self { view: view.into() }
    }
}

impl Render for NyaRootContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = component_typography(cx);
        div()
            .size_full()
            .font(typography.font)
            .text_size(typography.font_size)
            .capture_any_mouse_down(|event, window, cx| {
                schedule_nya_input_blur_on_outside_pointer_down(event.button, window, cx);
            })
            .child(self.view.clone())
            // GPUI's deferred rendering occurs after normal views. The panel boundaries
            // and drag lines in the main interface also use this mechanism,
            // so all component layers use a unified base priority to avoid hierarchies
            // being scattered across various popup types. The priority of internal popups
            // continues to take effect after the parent layer.
            .children(
                gpui_kit::component::Root::render_sheet_layer(window, cx)
                    .map(|layer| deferred(layer).with_priority(COMPONENT_OVERLAY_PRIORITY)),
            )
            .children(
                gpui_kit::component::Root::render_dialog_layer(window, cx)
                    .map(|layer| deferred(layer).with_priority(COMPONENT_OVERLAY_PRIORITY)),
            )
            .children(
                gpui_kit::component::Root::render_notification_layer(window, cx)
                    .map(|layer| deferred(layer).with_priority(COMPONENT_OVERLAY_PRIORITY)),
            )
    }
}

pub fn nya_root(
    view: impl Into<AnyView>,
    window: &mut Window,
    cx: &mut Context<NyaRoot>,
) -> NyaRoot {
    let content = cx.new(|_| NyaRootContent::new(view));
    gpui_kit::component::Root::new(content, window, cx)
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

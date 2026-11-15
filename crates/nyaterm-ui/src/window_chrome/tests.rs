use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, PointerCancelEvent, PointerCancelReason, Render,
    TestAppContext, VisualTestContext, Window, WindowControlArea, div, point, prelude::*, px,
};

use crate::theme::theme_palette;
use crate::window_chrome::{
    TitleDragBehavior, TitleDragEvent, TitleDragState, child_window_control_button,
    title_drag_region, window_control_button,
};
use crate::{NyaDropdownMenu, NyaInput, NyaInputState, NyaMenuItem};

#[test]
fn title_drag_requires_more_than_four_pixels_and_starts_once() {
    let mut state = TitleDragState::default();
    state.press(point(px(10.), px(10.)), 1);
    assert!(!state.move_pointer(point(px(14.), px(10.)), true));
    assert!(state.move_pointer(point(px(14.), px(11.)), true));
    assert!(!state.move_pointer(point(px(30.), px(30.)), true));
    assert!(!state.release(true));
    assert!(!state.move_pointer(point(px(40.), px(40.)), true));
}

#[test]
fn double_click_maximizes_only_without_drag_and_when_supported() {
    let mut state = TitleDragState::default();
    state.press(point(px(0.), px(0.)), 1);
    assert!(!state.release(true));
    state.press(point(px(0.), px(0.)), 2);
    assert!(!state.move_pointer(point(px(1.), px(1.)), true));
    assert!(state.release(true));
    assert!(!state.release(true));
    state.press(point(px(0.), px(0.)), 2);
    assert!(!state.release(false));
    state.press(point(px(0.), px(0.)), 2);
    assert!(state.move_pointer(point(px(10.), px(0.)), true));
    assert!(!state.release(true));
}

#[test]
fn cancel_and_lost_button_disarm_a_pending_drag() {
    let mut state = TitleDragState::default();
    state.press(point(px(0.), px(0.)), 2);
    state.cancel();
    assert!(!state.release(true));
    state.press(point(px(0.), px(0.)), 2);
    assert!(!state.move_pointer(point(px(20.), px(0.)), false));
    assert!(!state.move_pointer(point(px(30.), px(0.)), true));
    assert!(!state.release(true));
}

struct DragFixture {
    events: Rc<RefCell<Vec<TitleDragEvent>>>,
    clicks: Rc<RefCell<usize>>,
}

impl Render for DragFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let events = self.events.clone();
        let clicks = self.clicks.clone();
        title_drag_region(
            div().relative().w(px(300.)).h(px(40.)),
            "test-title-region",
            TitleDragBehavior::Linux { can_maximize: true },
            window,
            cx,
            move |event, _, _| events.borrow_mut().push(*event),
        )
        .child(window_control_button(
            theme_palette("github-dark"),
            "test-close",
            "icons/window/close.svg",
            WindowControlArea::Close,
            false,
            move |_, _, _| *clicks.borrow_mut() += 1,
        ))
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(100.))
                .w(px(60.))
                .h_full()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        )
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
}

fn press(cx: &mut VisualTestContext, x: f32, count: usize) {
    cx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(x), px(20.)),
                click_count: count,
                ..Default::default()
            }),
            cx,
        );
    });
}

#[gpui::test]
fn title_buttons_and_interactive_children_do_not_arm_window_drag(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let clicks = Rc::new(RefCell::new(0));
    let fixture_events = events.clone();
    let fixture_clicks = clicks.clone();
    let (_, cx) = cx.add_window_view(|_, _| DragFixture {
        events: fixture_events,
        clicks: fixture_clicks,
    });
    draw(cx);
    cx.simulate_click(point(px(23.), px(20.)), Modifiers::default());
    assert_eq!(*clicks.borrow(), 1);
    press(cx, 120., 1);
    assert!(events.borrow().is_empty());
    press(cx, 200., 1);
    assert_eq!(&*events.borrow(), &[TitleDragEvent::Pressed]);
}

#[cfg(target_os = "windows")]
struct NativeControlsFixture {
    clicks: Rc<RefCell<[usize; 3]>>,
}

#[cfg(target_os = "windows")]
impl Render for NativeControlsFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().w(px(138.)).h(px(40.)).children(
            [
                ("native-min", WindowControlArea::Min),
                ("native-max", WindowControlArea::Max),
                ("native-close", WindowControlArea::Close),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (id, area))| {
                let clicks = self.clicks.clone();
                window_control_button(
                    theme_palette("github-dark"),
                    id,
                    "icons/window/close.svg",
                    area,
                    true,
                    move |_, _, _| clicks.borrow_mut()[index] += 1,
                )
            }),
        )
    }
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn native_maximize_reaches_windows_and_application_controls_dispatch_once(cx: &mut TestAppContext) {
    let clicks = Rc::new(RefCell::new([0; 3]));
    let log = clicks.clone();
    let (_, cx) = cx.add_window_view(|_, _| NativeControlsFixture { clicks: log });
    draw(cx);
    for (index, x) in [23., 69., 115.].into_iter().enumerate() {
        press(cx, x, 1);
        let result = cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position: point(px(x), px(20.)),
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            )
        });
        assert_eq!(result.propagate, index == 1);
    }
    assert_eq!(*clicks.borrow(), [1, 0, 1]);
}

#[gpui::test]
fn drag_state_is_isolated_between_windows_and_releases_outside(cx: &mut TestAppContext) {
    let first_events = Rc::new(RefCell::new(Vec::new()));
    let second_events = Rc::new(RefCell::new(Vec::new()));
    let first_log = first_events.clone();
    let second_log = second_events.clone();
    let (_, first) = cx.add_window_view(|_, _| DragFixture {
        events: first_log,
        clicks: Rc::new(RefCell::new(0)),
    });
    let mut first = first.clone();
    let (_, second) = cx.add_window_view(|_, _| DragFixture {
        events: second_log,
        clicks: Rc::new(RefCell::new(0)),
    });
    draw(&mut first);
    draw(second);
    first.update(|window, _| window.activate_window());
    second.update(|window, _| window.activate_window());
    press(&mut first, 200., 1);
    second.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(240.), px(20.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
    });
    assert!(second_events.borrow().is_empty());
    first.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(400.), px(80.)),
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(240.), px(20.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
    });
    assert_eq!(&*first_events.borrow(), &[TitleDragEvent::Pressed]);
}

#[gpui::test]
fn title_region_emits_one_move_and_no_double_click_after_dragging(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let log = events.clone();
    let (_, cx) = cx.add_window_view(|_, _| DragFixture {
        events: log,
        clicks: Rc::new(RefCell::new(0)),
    });
    draw(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    press(cx, 200., 2);
    draw(cx);
    cx.update(|window, cx| {
        for (x, y) in [(204., 20.), (205., 70.), (230., 90.)] {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(x), px(y)),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                }),
                cx,
            );
        }
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position: point(px(230.), px(20.)),
                button: MouseButton::Left,
                click_count: 2,
                ..Default::default()
            }),
            cx,
        );
    });
    assert_eq!(
        &*events.borrow(),
        &[TitleDragEvent::Pressed, TitleDragEvent::StartMove]
    );
}

#[gpui::test]
fn pointer_cancel_disarms_drag_and_double_click_dispatches_once(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let log = events.clone();
    let (_, cx) = cx.add_window_view(|_, _| DragFixture {
        events: log,
        clicks: Rc::new(RefCell::new(0)),
    });
    draw(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    press(cx, 200., 2);
    cx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::PointerCancelled(PointerCancelEvent {
                reason: PointerCancelReason::CaptureLost,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(250.), px(20.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position: point(px(250.), px(20.)),
                button: MouseButton::Left,
                click_count: 2,
                ..Default::default()
            }),
            cx,
        );
    });
    assert_eq!(&*events.borrow(), &[TitleDragEvent::Pressed]);
    press(cx, 200., 2);
    cx.update(|window, cx| {
        for _ in 0..2 {
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position: point(px(200.), px(20.)),
                    button: MouseButton::Left,
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
        }
    });
    assert_eq!(
        &*events.borrow(),
        &[
            TitleDragEvent::Pressed,
            TitleDragEvent::Pressed,
            TitleDragEvent::DoubleClick,
        ]
    );
}

struct InteractiveFixture {
    input: Entity<NyaInputState>,
    events: Rc<RefCell<Vec<TitleDragEvent>>>,
    menu_opens: Rc<RefCell<usize>>,
}

impl Render for InteractiveFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let events = self.events.clone();
        let menu_opens = self.menu_opens.clone();
        title_drag_region(
            div().flex().w(px(360.)).h(px(40.)),
            "interactive-title-region",
            TitleDragBehavior::Linux { can_maximize: true },
            window,
            cx,
            move |event, _, _| events.borrow_mut().push(*event),
        )
        .child(
            div()
                .debug_selector(|| "title-input".into())
                .w(px(120.))
                .h(px(40.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(NyaInput::new(&self.input)),
        )
        .child(
            div()
                .debug_selector(|| "title-menu".into())
                .w(px(120.))
                .h(px(40.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    NyaDropdownMenu::new("title-test-menu")
                        .label("Menu")
                        .items_dynamic(move |_, _| {
                            *menu_opens.borrow_mut() += 1;
                            vec![NyaMenuItem::action("Example")]
                        }),
                ),
        )
    }
}

#[gpui::test]
fn title_input_keeps_focus_and_menu_opens_without_arming_drag(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let input = cx.new(|cx| NyaInputState::new(cx, ""));
    let events = Rc::new(RefCell::new(Vec::new()));
    let fixture_input = input.clone();
    let log = events.clone();
    let menu_opens = Rc::new(RefCell::new(0));
    let fixture_opens = menu_opens.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|_| InteractiveFixture {
            input: fixture_input,
            events: log,
            menu_opens: fixture_opens,
        });
        crate::nya_root(content, window, cx)
    });
    draw(cx);
    let field = cx.debug_bounds("title-input").unwrap();
    cx.simulate_click(field.center(), Modifiers::default());
    assert!(events.borrow().is_empty());
    cx.update(|window, cx| {
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
    });
    let menu = cx.debug_bounds("title-menu").unwrap();
    cx.simulate_click(menu.origin + point(px(20.), px(12.)), Modifiers::default());
    draw(cx);
    assert!(events.borrow().is_empty());
    assert_eq!(*menu_opens.borrow(), 1);
    cx.simulate_keystrokes("escape");
}

struct ChildControlsFixture {
    clicks: Arc<[AtomicUsize; 3]>,
}

impl Render for ChildControlsFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = theme_palette("github-dark");
        div().w(px(138.)).h(px(40.)).flex().children(
            [
                (
                    "test-child-min",
                    "icons/window/minimize.svg",
                    WindowControlArea::Min,
                ),
                (
                    "test-child-max",
                    "icons/window/maximize.svg",
                    WindowControlArea::Max,
                ),
                (
                    "test-child-close",
                    "icons/window/close.svg",
                    WindowControlArea::Close,
                ),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (id, icon, area))| {
                let clicks = self.clicks.clone();
                child_window_control_button(palette, id, icon, area, move |_, _, _| {
                    clicks[index].fetch_add(1, Ordering::SeqCst);
                })
            }),
        )
    }
}

#[gpui::test]
fn child_window_controls_dispatch_clicks(cx: &mut TestAppContext) {
    let clicks = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let fixture = ChildControlsFixture {
        clicks: clicks.clone(),
    };
    let (_, cx) = cx.add_window_view(|_, _| fixture);
    let cx: &mut VisualTestContext = cx;
    draw(cx);

    for (index, x) in [px(23.), px(69.), px(115.)].into_iter().enumerate() {
        cx.simulate_click(point(x, px(20.)), Modifiers::default());
        assert_eq!(clicks[index].load(Ordering::SeqCst), 1);
    }
}

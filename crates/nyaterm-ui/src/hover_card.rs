use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, PointerCancelEvent, Render,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, canvas, deferred, div, prelude::FluentBuilder as _, px,
};
use gpui_base::{POPUP_PRIORITY, Popup, Positioner};
use gpui_kit::component::ThemeStyled as _;

use crate::{NyaPopoverAlign, NyaPopoverPlacement};

/// Hover-triggered rich content that stays open while the pointer moves from
/// the trigger into the card.
///
/// This is the stable NyaTerm boundary for hover previews. It is
/// intended for concise, pointer-interactive previews; click-driven workflows
/// should continue to use [`crate::NyaPopover`].
#[derive(IntoElement)]
pub struct NyaHoverCard {
    id: SharedString,
    trigger: AnyElement,
    content: AnyElement,
    anchor: Anchor,
    placement: Option<NyaPopoverPlacement>,
    align: NyaPopoverAlign,
    offset: Pixels,
    open_delay: Duration,
    close_delay: Duration,
    appearance: bool,
    flexible_trigger: bool,
    suspended: bool,
}

impl NyaHoverCard {
    pub fn new(
        id: impl Into<SharedString>,
        trigger: impl IntoElement,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            trigger: trigger.into_any_element(),
            content: content.into_any_element(),
            anchor: Anchor::TopLeft,
            placement: None,
            align: NyaPopoverAlign::Center,
            offset: px(0.),
            open_delay: Duration::from_millis(700),
            close_delay: Duration::from_millis(250),
            appearance: true,
            flexible_trigger: false,
            suspended: false,
        }
    }

    /// Align the card to a corner of its trigger.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Place the card on a side of the measured trigger. Unlike corner
    /// anchoring, this flips at viewport edges and keeps the requested edge
    /// alignment after the flip, so a row-anchored card sits beside its row
    /// instead of covering the rows below it.
    pub fn placement(mut self, placement: NyaPopoverPlacement) -> Self {
        self.placement = Some(placement);
        self
    }

    /// Let a side-placed trigger fill and shrink within its parent's flex row.
    /// This keeps truncated text constrained by the available column width.
    pub fn flexible_trigger(mut self) -> Self {
        self.flexible_trigger = true;
        self
    }

    /// Alignment along the chosen side. Only meaningful with [`Self::placement`].
    pub fn align(mut self, align: NyaPopoverAlign) -> Self {
        self.align = align;
        self
    }

    /// Gap between the trigger and the card. Only meaningful with
    /// [`Self::placement`].
    pub fn offset(mut self, offset: Pixels) -> Self {
        self.offset = offset;
        self
    }

    pub fn open_delay(mut self, delay: Duration) -> Self {
        self.open_delay = delay;
        self
    }

    pub fn close_delay(mut self, delay: Duration) -> Self {
        self.close_delay = delay;
        self
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    /// Immediately close and cancel pending opening while a competing surface
    /// owns interaction. Resuming requires a physical pointer exit and re-entry.
    pub fn suspended(mut self, suspended: bool) -> Self {
        self.suspended = suspended;
        self
    }
}

/// Delayed open/close state shared by both positioning paths. `epoch` is what
/// retires a timer whose reason has already passed: hover events arrive faster
/// than the delays they schedule.
struct NyaHoverCardState {
    open: bool,
    open_delay: Duration,
    close_delay: Duration,
    open_task: Option<Task<()>>,
    close_task: Option<Task<()>>,
    epoch: usize,
    is_hovering_trigger: bool,
    is_hovering_content: bool,
    suspended: bool,
    requires_pointer_exit: bool,
    window_active: bool,
    trigger_bounds: Bounds<Pixels>,
    trigger_captured: bool,
    content_bounds: Option<Bounds<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl NyaHoverCardState {
    fn new(open_delay: Duration, close_delay: Duration) -> Self {
        Self {
            open: false,
            open_delay,
            close_delay,
            open_task: None,
            close_task: None,
            epoch: 0,
            is_hovering_trigger: false,
            is_hovering_content: false,
            suspended: false,
            requires_pointer_exit: false,
            window_active: true,
            trigger_bounds: Bounds::default(),
            trigger_captured: false,
            content_bounds: None,
            _subscriptions: Vec::new(),
        }
    }

    fn is_open_now(&self) -> bool {
        self.open
    }

    fn sync(&mut self, open_delay: Duration, close_delay: Duration) {
        self.open_delay = open_delay;
        self.close_delay = close_delay;
    }

    fn set_suspended(&mut self, suspended: bool, cx: &mut Context<Self>) {
        if suspended && !self.suspended {
            self.dismiss(cx);
        }
        self.suspended = suspended;
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.requires_pointer_exit |= self.is_hovering_trigger;
        self.cancel_tasks();
        self.is_hovering_content = false;
        self.content_bounds = None;
        self.set_open(false, cx);
    }

    fn can_open(&self) -> bool {
        !self.suspended && !self.requires_pointer_exit && self.window_active
    }

    fn schedule_open(&mut self, cx: &mut Context<Self>) {
        self.cancel_tasks();
        let epoch = self.next_epoch();
        let delay = self.open_delay;
        self.open_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |state, cx| {
                if state.epoch == epoch && state.can_open() && state.is_hovering_trigger {
                    state.set_open(true, cx);
                }
            });
        }));
    }

    fn schedule_close(&mut self, cx: &mut Context<Self>) {
        self.cancel_tasks();
        let epoch = self.next_epoch();
        let delay = self.close_delay;
        self.close_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |state, cx| {
                if state.epoch == epoch && !state.is_hovering_trigger && !state.is_hovering_content
                {
                    state.set_open(false, cx);
                }
            });
        }));
    }

    fn cancel_tasks(&mut self) {
        self.epoch += 1;
        self.open_task = None;
        self.close_task = None;
    }

    fn next_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    fn on_trigger_hover(&mut self, hovering: bool, cx: &mut Context<Self>) {
        self.is_hovering_trigger = hovering;
        if hovering && self.can_open() {
            self.schedule_open(cx);
        } else if !self.is_hovering_content {
            self.schedule_close(cx);
        }
    }

    fn on_content_hover(&mut self, hovering: bool, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.is_hovering_content = hovering;
        if hovering {
            self.cancel_tasks();
        } else if !self.is_hovering_trigger {
            self.schedule_close(cx);
        }
    }

    fn on_pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.dragging() || cx.has_active_drag() {
            self.dismiss(cx);
        } else if !self.suspended && !self.trigger_bounds.contains(&event.position) {
            // Occlusion can emit hover(false) while the pointer is still over
            // the trigger. Only actual pointer movement outside rearms it.
            self.requires_pointer_exit = false;
        }
    }

    fn on_pointer_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let copying = event.button == MouseButton::Left
            && self.open
            && self
                .content_bounds
                .is_some_and(|bounds| bounds.contains(&event.position));
        if !copying {
            self.dismiss(cx);
        }
    }
}

impl Render for NyaHoverCardState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl RenderOnce for NyaHoverCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id: gpui::ElementId = self.id.clone().into();
        let state = window.use_keyed_state(
            id.clone(),
            cx,
            |window, cx: &mut Context<NyaHoverCardState>| {
                let mut state = NyaHoverCardState::new(self.open_delay, self.close_delay);
                state.window_active = window.is_window_active();
                state._subscriptions.push(cx.observe_window_activation(
                    window,
                    |state, window, cx| {
                        state.window_active = window.is_window_active();
                        if !state.window_active {
                            state.dismiss(cx);
                        } else if state.is_hovering_trigger && state.can_open() {
                            state.schedule_open(cx);
                        }
                    },
                ));
                let window_handle = window.window_handle();
                let weak_state = cx.weak_entity();
                state
                    ._subscriptions
                    .push(cx.intercept_keystrokes(move |event, window, cx| {
                        if window.window_handle() == window_handle
                            && event.keystroke.key == "escape"
                        {
                            let _ = weak_state.update(cx, |state, cx| state.dismiss(cx));
                        }
                    }));
                state
            },
        );
        state.update(cx, |state, cx| {
            state.sync(self.open_delay, self.close_delay);
            let suspended = self.suspended || cx.has_active_drag();
            if state.suspended && !suspended {
                state.requires_pointer_exit =
                    state.trigger_bounds.contains(&window.mouse_position());
            }
            state.set_suspended(suspended, cx);
        });
        let open = state.read(cx).is_open_now();

        // By default the trigger host neither grows nor shrinks. Text in a flex
        // row can opt into the available width; the cross axis still stretches
        // so `h_full` resolves against the parent, not the wrapper's text height.
        let root = div()
            .id(id.clone())
            .relative()
            .when(self.placement.is_some(), |this| this.flex().flex_none())
            .when(self.flexible_trigger, |this| this.flex_1().min_w_0())
            .items_stretch()
            .child(self.trigger)
            .on_hover(window.listener_for(&state, |state, hovered, _, cx| {
                state.on_trigger_hover(*hovered, cx)
            }))
            .child(
                canvas(
                    {
                        let state = state.clone();
                        move |bounds, window, cx| {
                            let first = state.update(cx, |state, _| {
                                let first = !state.trigger_captured;
                                state.trigger_bounds = bounds;
                                state.trigger_captured = true;
                                first
                            });
                            if first {
                                window.request_animation_frame();
                            }
                        }
                    },
                    {
                        let state = state.clone();
                        move |_, _, window, _| {
                            let move_state = state.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase.capture() {
                                    move_state
                                        .update(cx, |state, cx| state.on_pointer_move(event, cx));
                                }
                            });
                            let down_state = state.clone();
                            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                                if phase.capture() {
                                    down_state
                                        .update(cx, |state, cx| state.on_pointer_down(event, cx));
                                }
                            });
                            let cancel_state = state.clone();
                            window.on_mouse_event(move |_: &PointerCancelEvent, phase, _, cx| {
                                if phase.capture() {
                                    cancel_state.update(cx, |state, cx| state.dismiss(cx));
                                }
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );

        if !open {
            return if self.placement.is_none() {
                Popup::new(id, root).anchor(self.anchor).into_any_element()
            } else {
                root.into_any_element()
            };
        }
        if self.placement.is_some() && !state.read(cx).trigger_captured {
            return root.into_any_element();
        }

        let content = div()
            .id("content")
            .occlude()
            .flex()
            .flex_col()
            .tab_group()
            .when(self.appearance, |this| {
                this.popover_style(cx)
                    .when(self.placement.is_some(), |this| {
                        this.shadow(gpui_base::Theme::global(cx).tokens.shadow.md)
                    })
                    .p_3()
            })
            .when(self.placement.is_none(), |this| {
                match self.anchor {
                    Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => {
                        this.bottom_1()
                    }
                    _ => this.top_1(),
                }
                .overflow_hidden()
            })
            .child(self.content)
            .child(
                canvas(
                    {
                        let state = state.clone();
                        move |bounds, _, cx| {
                            state.update(cx, |state, _| state.content_bounds = Some(bounds))
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_hover(window.listener_for(&state, |state, hovered, _, cx| {
                state.on_content_hover(*hovered, cx)
            }));

        let Some(placement) = self.placement else {
            return Popup::new(id, root)
                .anchor(self.anchor)
                .content(content)
                .into_any_element();
        };

        let trigger_bounds = state.read(cx).trigger_bounds;
        root.child(
            deferred(
                Positioner::side(trigger_bounds)
                    .placement(placement.into())
                    .align(self.align.into())
                    .offset(self.offset)
                    .margin(px(8.))
                    .child(content),
            )
            .with_priority(POPUP_PRIORITY),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Duration};

    use gpui::prelude::FluentBuilder as _;

    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Render, StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, point,
        px,
    };

    use super::NyaHoverCard;
    use crate::{NyaPopoverAlign, NyaPopoverPlacement};

    fn activate_and_draw(cx: &mut gpui::VisualTestContext) {
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    struct CoordinatedHarness {
        suspended: bool,
        placement: Option<NyaPopoverPlacement>,
    }

    impl Render for CoordinatedHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    NyaHoverCard::new(
                        "coordinated-card",
                        div()
                            .id("trigger")
                            .debug_selector(|| "coordinated-trigger".into())
                            .w(px(118.))
                            .h(px(36.))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, _, _, cx| {
                                    this.suspended = true;
                                    cx.notify();
                                }),
                            ),
                        div()
                            .debug_selector(|| "coordinated-content".into())
                            .size(px(80.)),
                    )
                    .suspended(self.suspended)
                    .appearance(false)
                    .open_delay(Duration::from_millis(10))
                    .close_delay(Duration::from_millis(100))
                    .when_some(self.placement, |card, placement| card.placement(placement)),
                )
                .when(self.suspended, |root| {
                    root.child(
                        // Cover the trigger just as a menu input layer does. Removing
                        // this layer synthesizes hover(true) without pointer movement.
                        div().id("menu").absolute().inset_0().occlude(),
                    )
                })
        }
    }

    #[gpui::test]
    fn right_click_cancels_pending_and_visible_cards_until_pointer_reentry(
        cx: &mut TestAppContext,
    ) {
        for placement in [None, Some(NyaPopoverPlacement::Bottom)] {
            for open_first in [false, true] {
                let (view, cx) = cx.add_window_view(|_, _| CoordinatedHarness {
                    suspended: false,
                    placement,
                });
                activate_and_draw(cx);
                let trigger = cx.debug_bounds("coordinated-trigger").unwrap();
                cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
                cx.executor()
                    .advance_clock(Duration::from_millis(if open_first { 10 } else { 5 }));
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert_eq!(cx.debug_bounds("coordinated-content").is_some(), open_first);

                cx.simulate_mouse_down(trigger.center(), MouseButton::Right, Modifiers::default());
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert!(cx.debug_bounds("coordinated-content").is_none());
                cx.simulate_mouse_up(trigger.center(), MouseButton::Right, Modifiers::default());
                cx.executor().advance_clock(Duration::from_millis(200));
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert!(cx.debug_bounds("coordinated-content").is_none());

                view.update(cx, |view, cx| {
                    view.suspended = false;
                    cx.notify();
                });
                cx.update(|window, cx| window.draw(cx).clear(cx));
                cx.executor().advance_clock(Duration::from_millis(200));
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert!(cx.debug_bounds("coordinated-content").is_none());

                cx.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::default());
                cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
                cx.executor().advance_clock(Duration::from_millis(10));
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                assert!(cx.debug_bounds("coordinated-content").is_some());
            }
        }
    }

    #[gpui::test]
    fn suspension_closes_a_card_opened_from_another_trigger(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| CoordinatedHarness {
            suspended: false,
            placement: None,
        });
        activate_and_draw(cx);
        let trigger = cx.debug_bounds("coordinated-trigger").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("coordinated-content").is_some());
        view.update(cx, |view, cx| {
            view.suspended = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("coordinated-content").is_none());
    }

    #[gpui::test]
    fn escape_closes_card_without_waiting_for_leave_delay(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Harness {
            clicked: Rc::new(Cell::new(false)),
        });
        activate_and_draw(cx);
        let trigger = cx.debug_bounds("hover-card-trigger").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_some());
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_none());
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_none());
    }

    #[gpui::test]
    fn dragging_cancels_pending_open_and_requires_fresh_hover(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Harness {
            clicked: Rc::new(Cell::new(false)),
        });
        activate_and_draw(cx);
        let trigger = cx.debug_bounds("hover-card-trigger").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.simulate_mouse_move(
            trigger.center(),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_none());
    }

    #[gpui::test]
    fn window_deactivation_closes_card_and_cancels_pending_open(cx: &mut TestAppContext) {
        for open_first in [false, true] {
            let (_, cx) = cx.add_window_view(|_, _| Harness {
                clicked: Rc::new(Cell::new(false)),
            });
            activate_and_draw(cx);
            let trigger = cx.debug_bounds("hover-card-trigger").unwrap();
            cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
            cx.executor()
                .advance_clock(Duration::from_millis(if open_first { 10 } else { 5 }));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(cx.debug_bounds("hover-card-content").is_some(), open_first);
            cx.deactivate_window();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(cx.debug_bounds("hover-card-content").is_none());
        }
    }

    struct Harness {
        clicked: Rc<Cell<bool>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            NyaHoverCard::new(
                "hover-card",
                div()
                    .debug_selector(|| "hover-card-trigger".into())
                    .w(px(118.))
                    .h(px(36.)),
                div()
                    .id("hover-card-content")
                    .debug_selector(|| "hover-card-content".into())
                    .size(px(80.))
                    .on_click(move |_, _, _| clicked.set(true)),
            )
            .appearance(false)
            .open_delay(Duration::from_millis(10))
            .close_delay(Duration::from_millis(100))
        }
    }

    /// Side-placed card with room on its preferred side.
    struct SideHarness {
        clicked: Rc<Cell<bool>>,
    }

    impl Render for SideHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_start()
                .pl_20()
                .child(
                    NyaHoverCard::new(
                        "side-hover-card",
                        div()
                            .debug_selector(|| "side-hover-card-trigger".into())
                            .w(px(118.))
                            .h(px(36.)),
                        div()
                            .id("side-hover-card-content")
                            .debug_selector(|| "side-hover-card-content".into())
                            .w(px(80.))
                            .h(px(96.))
                            .on_click(move |_, _, _| clicked.set(true)),
                    )
                    .placement(NyaPopoverPlacement::Right)
                    .align(NyaPopoverAlign::Center)
                    .offset(px(6.))
                    .appearance(false)
                    .open_delay(Duration::from_millis(10))
                    .close_delay(Duration::from_millis(100)),
                )
        }
    }

    /// Same card, but with the trigger pinned against the right edge so the
    /// preferred side cannot fit.
    struct FlippedSideHarness;

    struct FilenameHarness;

    impl Render for FilenameHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(120.)).flex().gap(px(8.)).child(div().w(px(16.)).flex_none()).child(
                NyaHoverCard::new(
                    "filename-hover-card",
                    div().debug_selector(|| "filename-trigger".into())
                        .min_w_0().flex_1().truncate()
                        .child("a-very-long-filename-that-must-fit-in-the-remaining-column-width.txt"),
                    div(),
                ).placement(NyaPopoverPlacement::Top).flexible_trigger(),
            )
        }
    }

    #[gpui::test]
    fn flexible_filename_trigger_is_constrained_to_remaining_column_width(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| FilenameHarness);
        activate_and_draw(cx);
        let trigger = cx.debug_bounds("filename-trigger").unwrap();
        assert_eq!(trigger.size.width, px(96.));
        assert_eq!(trigger.right(), px(120.));
    }

    impl Render for FlippedSideHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().items_center().justify_end().child(
                NyaHoverCard::new(
                    "flipped-hover-card",
                    div()
                        .debug_selector(|| "flipped-hover-card-trigger".into())
                        .w(px(118.))
                        .h(px(36.)),
                    div()
                        .id("flipped-hover-card-content")
                        .debug_selector(|| "flipped-hover-card-content".into())
                        .w(px(220.))
                        .h(px(96.)),
                )
                .placement(NyaPopoverPlacement::Right)
                .align(NyaPopoverAlign::Center)
                .offset(px(6.))
                .appearance(false)
                .open_delay(Duration::from_millis(10))
                .close_delay(Duration::from_millis(100)),
            )
        }
    }

    #[test]
    fn default_open_delay_avoids_triggering_on_a_brief_pass() {
        let card = NyaHoverCard::new("hover-card-delay", div(), div());
        assert_eq!(card.open_delay, Duration::from_millis(700));
    }

    #[test]
    fn side_placement_can_be_configured_independently_from_corner_anchoring() {
        let card = NyaHoverCard::new("hover-card-side", div(), div())
            .placement(NyaPopoverPlacement::Right)
            .align(NyaPopoverAlign::Center)
            .offset(px(6.));
        assert_eq!(card.placement, Some(NyaPopoverPlacement::Right));
        assert_eq!(card.align, NyaPopoverAlign::Center);
        assert_eq!(card.offset, px(6.));
    }

    #[test]
    fn placement_is_unset_by_default_so_existing_callers_stay_corner_anchored() {
        let card = NyaHoverCard::new("hover-card-default", div(), div());
        assert_eq!(card.placement, None);
    }

    #[gpui::test]
    fn content_stays_open_and_accepts_clicks_after_leaving_trigger(cx: &mut TestAppContext) {
        let clicked = Rc::new(Cell::new(false));
        let clicked_for_view = clicked.clone();
        let (_, cx) = cx.add_window_view(|_, _| Harness {
            clicked: clicked_for_view,
        });
        activate_and_draw(cx);

        let trigger = cx.debug_bounds("hover-card-trigger").unwrap();
        assert_eq!(trigger.size.width, px(118.));
        assert_eq!(trigger.size.height, px(36.));
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });

        let content = cx.debug_bounds("hover-card-content").unwrap();
        cx.simulate_mouse_move(content.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_some());

        cx.simulate_click(content.center(), Modifiers::default());
        assert!(clicked.get());

        cx.simulate_mouse_move(point(px(200.), px(200.)), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("hover-card-content").is_none());
    }

    #[gpui::test]
    fn side_placed_card_is_adjacent_to_and_centered_on_its_trigger(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let clicked = Rc::new(Cell::new(false));
        let clicked_for_view = clicked.clone();
        let (_, cx) = cx.add_window_view(|_, _| SideHarness {
            clicked: clicked_for_view,
        });
        activate_and_draw(cx);

        let trigger = cx.debug_bounds("side-hover-card-trigger").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });

        let content = cx
            .debug_bounds("side-hover-card-content")
            .expect("opened side-placed hover card");
        assert_eq!(content.left(), trigger.right() + px(6.));
        assert_eq!(content.center().y, trigger.center().y);
        // Taller than the row it points at, which is the whole reason it must
        // not sit below the row.
        assert!(content.top() < trigger.top());
        assert!(content.bottom() > trigger.bottom());

        // Crossing the gap into the card must cancel the delayed close, so the
        // copy buttons a card like this carries stay reachable.
        cx.simulate_mouse_move(content.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("side-hover-card-content").is_some());

        cx.simulate_click(content.center(), Modifiers::default());
        assert!(clicked.get());

        cx.simulate_mouse_move(point(px(4.), px(4.)), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("side-hover-card-content").is_none());
    }

    #[gpui::test]
    fn side_placed_card_flips_when_the_preferred_side_has_no_room(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| FlippedSideHarness);
        activate_and_draw(cx);

        let trigger = cx.debug_bounds("flipped-hover-card-trigger").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });

        let content = cx
            .debug_bounds("flipped-hover-card-content")
            .expect("opened side-placed hover card");
        assert_eq!(content.right() + px(6.), trigger.left());
    }
}

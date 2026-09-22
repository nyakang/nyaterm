use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    Window, canvas, deferred, div, prelude::FluentBuilder as _, px,
};
use gpui_base::{POPUP_PRIORITY, Positioner};
use gpui_kit::component::{ThemeStyled as _, hover_card::HoverCard};

use crate::{NyaPopoverAlign, NyaPopoverPlacement};

/// Hover-triggered rich content that stays open while the pointer moves from
/// the trigger into the card.
///
/// This is the stable NyaTerm boundary for `gpui-kit`'s HoverCard. It is
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
}

/// Trigger bounds for the side-placed path, captured during prepaint.
#[derive(Default)]
struct NyaHoverCardAnchorState {
    bounds: gpui::Bounds<Pixels>,
    captured: bool,
}

/// Delayed open/close state for the side-placed path.
///
/// The vendor owns an equivalent state machine, but only behind its
/// corner-anchoring host, so side placement needs its own. `epoch` is what
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
        }
    }

    fn is_open_now(&self) -> bool {
        self.open
    }

    fn sync(&mut self, open_delay: Duration, close_delay: Duration) {
        self.open_delay = open_delay;
        self.close_delay = close_delay;
    }

    fn schedule_open(&mut self, cx: &mut Context<Self>) {
        self.cancel_tasks();
        let epoch = self.next_epoch();
        let delay = self.open_delay;
        self.open_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |state, cx| {
                if state.epoch == epoch {
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
        if hovering {
            self.schedule_open(cx);
        } else if !self.is_hovering_content {
            self.schedule_close(cx);
        }
    }

    fn on_content_hover(&mut self, hovering: bool, cx: &mut Context<Self>) {
        self.is_hovering_content = hovering;
        if hovering {
            self.cancel_tasks();
        } else if !self.is_hovering_trigger {
            self.schedule_close(cx);
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
        let Some(placement) = self.placement else {
            return self.render_corner().into_any_element();
        };

        let id: gpui::ElementId = self.id.clone().into();
        let state = window.use_keyed_state(id.clone(), cx, |_, _| {
            NyaHoverCardState::new(self.open_delay, self.close_delay)
        });
        state.update(cx, |state, _| state.sync(self.open_delay, self.close_delay));
        let open = state.read(cx).is_open_now();

        let anchor_state = window.use_keyed_state((id.clone(), "side-anchor"), cx, |_, _| {
            NyaHoverCardAnchorState::default()
        });

        // The trigger host stays layout-neutral: it is a row that neither grows
        // nor shrinks and lets the cross axis stretch, so a trigger asking for
        // `h_full` still resolves against the real parent rather than against
        // this wrapper's text height.
        let root = div()
            .id(id)
            .relative()
            .flex()
            .flex_none()
            .items_stretch()
            .child(self.trigger)
            .on_hover(window.listener_for(&state, |state, hovered, _, cx| {
                state.on_trigger_hover(*hovered, cx)
            }))
            .child(
                canvas(
                    {
                        let anchor_state = anchor_state.clone();
                        move |bounds, window, cx| {
                            let first = anchor_state.update(cx, |state, _| {
                                let first = !state.captured;
                                state.bounds = bounds;
                                state.captured = true;
                                first
                            });
                            if first {
                                window.request_animation_frame();
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );

        if !open || !anchor_state.read(cx).captured {
            return root.into_any_element();
        }

        let content = div()
            .id("content")
            .occlude()
            .when(self.appearance, |this| this.popover_style(cx).p_3())
            .child(self.content)
            .on_hover(window.listener_for(&state, |state, hovered, _, cx| {
                state.on_content_hover(*hovered, cx)
            }));

        let trigger_bounds = anchor_state.read(cx).bounds;
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

impl NyaHoverCard {
    fn render_corner(self) -> impl IntoElement {
        HoverCard::new(self.id)
            .anchor(self.anchor)
            .open_delay(self.open_delay)
            .close_delay(self.close_delay)
            .appearance(self.appearance)
            .trigger(self.trigger)
            .child(self.content)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Duration};

    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, point, px,
    };

    use super::NyaHoverCard;
    use crate::{NyaPopoverAlign, NyaPopoverPlacement};

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
        cx.update(|window, cx| window.draw(cx).clear(cx));

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
        cx.update(|window, cx| window.draw(cx).clear(cx));

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
        cx.update(|window, cx| window.draw(cx).clear(cx));

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

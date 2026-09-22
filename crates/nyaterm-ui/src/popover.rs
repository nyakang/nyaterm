use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, App, Bounds, Focusable as _, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement, Pixels, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, deferred, div,
    prelude::FluentBuilder as _, px,
};
use gpui_base::{POPUP_PRIORITY, PopoverState, Positioner};
use gpui_kit::component::{Selectable, ThemeStyled as _, popover::Popover, v_flex};

type NyaPopoverOpenHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
type NyaPopoverOutsideHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaPopoverPlacement {
    Top,
    Bottom,
    Left,
    Right,
}

impl From<NyaPopoverPlacement> for gpui_base::Placement {
    fn from(value: NyaPopoverPlacement) -> Self {
        match value {
            NyaPopoverPlacement::Top => Self::Top,
            NyaPopoverPlacement::Bottom => Self::Bottom,
            NyaPopoverPlacement::Left => Self::Left,
            NyaPopoverPlacement::Right => Self::Right,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NyaPopoverAlign {
    Start,
    #[default]
    Center,
    End,
}

impl From<NyaPopoverAlign> for gpui_base::Align {
    fn from(value: NyaPopoverAlign) -> Self {
        match value {
            NyaPopoverAlign::Start => Self::Start,
            NyaPopoverAlign::Center => Self::Center,
            NyaPopoverAlign::End => Self::End,
        }
    }
}

#[derive(Default)]
struct NyaPopoverAnchorState {
    bounds: Bounds<Pixels>,
    captured: bool,
}

#[derive(IntoElement)]
struct NyaPopoverTrigger {
    element: AnyElement,
    selected: bool,
}

impl Selectable for NyaPopoverTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for NyaPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.element
    }
}

#[derive(IntoElement)]
pub struct NyaPopover {
    id: SharedString,
    trigger: AnyElement,
    content: AnyElement,
    anchor: Anchor,
    placement: Option<NyaPopoverPlacement>,
    align: NyaPopoverAlign,
    offset: Pixels,
    open: Option<bool>,
    appearance: bool,
    overlay_closable: bool,
    trigger_click: bool,
    on_open_change: Option<NyaPopoverOpenHandler>,
    on_click_outside: Option<NyaPopoverOutsideHandler>,
}

impl NyaPopover {
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
            open: None,
            appearance: true,
            overlay_closable: true,
            trigger_click: true,
            on_open_change: None,
            on_click_outside: None,
        }
    }

    /// Align the popover surface to a corner of its trigger.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Place the surface on a side of the measured trigger. Unlike corner
    /// anchoring, this flips at viewport edges and keeps the requested edge
    /// alignment after the flip.
    pub fn placement(mut self, placement: NyaPopoverPlacement) -> Self {
        self.placement = Some(placement);
        self
    }

    pub fn align(mut self, align: NyaPopoverAlign) -> Self {
        self.align = align;
        self
    }

    pub fn offset(mut self, offset: Pixels) -> Self {
        self.offset = offset;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn overlay_closable(mut self, closable: bool) -> Self {
        self.overlay_closable = closable;
        self
    }

    /// Control whether pressing the trigger toggles the popover. Disable this
    /// for hover-owned or otherwise externally controlled open state.
    pub fn trigger_click(mut self, enabled: bool) -> Self {
        self.trigger_click = enabled;
        self
    }

    pub fn on_open_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// Run after a side-positioned popover is dismissed by an outside click.
    /// Trigger toggles and Escape do not invoke this callback.
    pub fn on_click_outside(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click_outside = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NyaPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(placement) = self.placement else {
            return self.render_corner().into_any_element();
        };

        let id: gpui::ElementId = self.id.clone().into();
        let state = window.use_keyed_state(id.clone(), cx, |_, cx| PopoverState::new(false, cx));
        state.update(cx, |state, cx| {
            state.set_on_open_change(self.on_open_change.clone());
            if let Some(open) = self.open {
                state.set_open(open, cx);
            }
        });

        let anchor_state = window.use_keyed_state((id.clone(), "side-anchor"), cx, |_, _| {
            NyaPopoverAnchorState::default()
        });
        let open = state.read(cx).is_open();
        let parent_view_id = window.current_view();
        let mut root = div()
            .id(id)
            .relative()
            .child(self.trigger)
            .when(self.trigger_click, |this| {
                this.on_mouse_down(MouseButton::Left, {
                    let state = state.clone();
                    move |_, window, cx| {
                        cx.stop_propagation();
                        state.update(cx, |state, cx| {
                            state.set_open(open, cx);
                            state.toggle_open(window, cx);
                        });
                        cx.notify(parent_view_id);
                    }
                })
            })
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

        let focus_handle = state.read(cx).focus_handle(cx);
        let mut content = v_flex()
            .id("content")
            .occlude()
            .tab_group()
            .track_focus(&focus_handle)
            .key_context("Popover")
            .role(Role::Dialog)
            .on_action(window.listener_for(&state, PopoverState::on_action_cancel))
            .when(self.appearance, |this| this.popover_style(cx).p_3())
            .child(self.content);
        if self.overlay_closable {
            let on_click_outside = self.on_click_outside.clone();
            content = content.on_mouse_down_out({
                let state = state.clone();
                move |_, window, cx| {
                    state.update(cx, |state, cx| state.dismiss(window, cx));
                    if let Some(on_click_outside) = on_click_outside.as_ref() {
                        on_click_outside(window, cx);
                    }
                    cx.notify(parent_view_id);
                }
            });
        }

        let trigger_bounds = anchor_state.read(cx).bounds;
        root = root.child(
            deferred(
                Positioner::side(trigger_bounds)
                    .placement(placement.into())
                    .align(self.align.into())
                    .offset(self.offset)
                    .margin(px(8.))
                    .child(content),
            )
            .with_priority(POPUP_PRIORITY),
        );
        root.into_any_element()
    }
}

impl NyaPopover {
    fn render_corner(self) -> impl IntoElement {
        let trigger = NyaPopoverTrigger {
            element: self.trigger,
            selected: false,
        };
        let mut popover = Popover::new(self.id)
            .anchor(self.anchor)
            .trigger(trigger)
            .appearance(self.appearance)
            .overlay_closable(self.overlay_closable);
        if let Some(open) = self.open {
            popover = popover.open(open);
        }
        if let Some(on_open_change) = self.on_open_change {
            popover =
                popover.on_open_change(move |open, window, cx| on_open_change(open, window, cx));
        }
        popover.child(self.content)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        Anchor, Context, InteractiveElement as _, IntoElement as _, Modifiers, ParentElement as _,
        Render, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _,
        TestAppContext, VisualTestContext, Window, div, point, px,
    };

    use crate::NyaScrollArea;

    use super::{NyaPopover, NyaPopoverAlign, NyaPopoverPlacement};

    struct PopoverFixture {
        popup_clicks: Rc<Cell<usize>>,
        lower_clicks: Rc<Cell<usize>>,
    }

    struct SidePopoverFixture;

    struct RightSidePopoverFixture;

    impl Render for SidePopoverFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_end()
                .pr_20()
                .child(
                    NyaPopover::new(
                        "side-popover",
                        div()
                            .debug_selector(|| "side-popover-trigger".into())
                            .w(px(100.))
                            .h(px(32.)),
                        div()
                            .debug_selector(|| "side-popover-content".into())
                            .w(px(160.))
                            .overflow_hidden()
                            .child(
                                NyaScrollArea::new("side-popover-scroll")
                                    .max_h(px(96.))
                                    .child(div().h(px(48.)).flex_shrink_0())
                                    .child(div().h(px(48.)).flex_shrink_0())
                                    .child(div().h(px(48.)).flex_shrink_0())
                                    .child(
                                        div()
                                            .debug_selector(|| "side-popover-last-row".into())
                                            .h(px(48.))
                                            .flex_shrink_0(),
                                    ),
                            ),
                    )
                    .placement(NyaPopoverPlacement::Left)
                    .align(NyaPopoverAlign::Start)
                    .offset(px(4.))
                    .appearance(false)
                    .overlay_closable(false)
                    .open(true),
                )
        }
    }

    impl Render for RightSidePopoverFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_start()
                .pl_20()
                .child(
                    NyaPopover::new(
                        "right-side-popover",
                        div()
                            .debug_selector(|| "right-side-popover-trigger".into())
                            .w(px(100.))
                            .h(px(32.)),
                        div()
                            .debug_selector(|| "right-side-popover-content".into())
                            .w(px(160.))
                            .h(px(96.)),
                    )
                    .placement(NyaPopoverPlacement::Right)
                    .align(NyaPopoverAlign::Start)
                    .offset(px(4.))
                    .appearance(false)
                    .overlay_closable(false)
                    .open(true),
                )
        }
    }

    impl Render for PopoverFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let popup_clicks = self.popup_clicks.clone();
            let lower_clicks = self.lower_clicks.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .id("popover-lower-layer")
                        .absolute()
                        .inset_0()
                        .on_click(move |_, _, _| lower_clicks.set(lower_clicks.get() + 1)),
                )
                .child(
                    div().w_full().flex().justify_end().child(
                        NyaPopover::new(
                            "anchored-popover",
                            div()
                                .id("anchored-popover-trigger")
                                .debug_selector(|| "anchored-popover-trigger".into())
                                .w_8()
                                .h_8(),
                            div()
                                .id("anchored-popover-content-element")
                                .debug_selector(|| "anchored-popover-content".into())
                                .w_64()
                                .h_16()
                                .on_click(move |_, _, _| popup_clicks.set(popup_clicks.get() + 1)),
                        )
                        .anchor(Anchor::TopRight)
                        .appearance(false),
                    ),
                )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[test]
    fn anchor_defaults_to_top_left_and_can_be_overridden() {
        let default = NyaPopover::new("default", div(), div());
        assert_eq!(default.anchor, Anchor::TopLeft);

        let anchored = NyaPopover::new("anchored", div(), div()).anchor(Anchor::TopRight);
        assert_eq!(anchored.anchor, Anchor::TopRight);

        let _ = anchored.into_any_element();
    }

    #[test]
    fn side_placement_can_be_configured_independently_from_corner_anchoring() {
        let popover = NyaPopover::new("side", div(), div())
            .placement(NyaPopoverPlacement::Left)
            .align(NyaPopoverAlign::Start)
            .offset(px(4.));
        assert_eq!(popover.placement, Some(NyaPopoverPlacement::Left));
        assert_eq!(popover.align, NyaPopoverAlign::Start);
        assert_eq!(popover.offset, px(4.));
    }

    #[gpui::test]
    fn side_popover_is_adjacent_to_and_top_aligned_with_its_trigger(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| SidePopoverFixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        draw(cx);

        let trigger = cx.debug_bounds("side-popover-trigger").unwrap();
        let content = cx.debug_bounds("side-popover-content").unwrap();
        assert_eq!(content.top(), trigger.top());
        assert_eq!(content.right() + px(4.), trigger.left());
        assert!(content.bottom() > trigger.bottom());
    }

    #[gpui::test]
    fn right_side_popover_is_adjacent_to_and_top_aligned_with_its_trigger(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| RightSidePopoverFixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        draw(cx);

        let trigger = cx.debug_bounds("right-side-popover-trigger").unwrap();
        let content = cx.debug_bounds("right-side-popover-content").unwrap();
        assert_eq!(content.top(), trigger.top());
        assert_eq!(content.left(), trigger.right() + px(4.));
        assert!(content.bottom() > trigger.bottom());
    }

    #[gpui::test]
    fn side_popover_in_flow_scroll_area_handles_wheel_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| SidePopoverFixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        draw(cx);

        let content = cx.debug_bounds("side-popover-content").unwrap();
        let initial_last_y = cx.debug_bounds("side-popover-last-row").unwrap().origin.y;
        cx.simulate_event(ScrollWheelEvent {
            position: content.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-48.))),
            ..Default::default()
        });
        draw(cx);

        assert!(cx.debug_bounds("side-popover-last-row").unwrap().origin.y < initial_last_y);
    }

    struct SidePopoverDismissFixture {
        open: bool,
        outside_clicks: Rc<Cell<usize>>,
    }

    impl Render for SidePopoverDismissFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
            let outside_clicks = self.outside_clicks.clone();
            let app = cx.weak_entity();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_end()
                .pr_20()
                .child(
                    NyaPopover::new(
                        "dismiss-side-popover",
                        div().w(px(100.)).h(px(32.)),
                        div()
                            .debug_selector(|| "dismiss-side-popover-content".into())
                            .w(px(160.))
                            .h(px(96.)),
                    )
                    .placement(NyaPopoverPlacement::Left)
                    .align(NyaPopoverAlign::Start)
                    .appearance(false)
                    .open(self.open)
                    .on_open_change(cx.listener(|this, open, _, cx| {
                        this.open = *open;
                        cx.notify();
                    }))
                    .on_click_outside(move |_, cx| {
                        outside_clicks.set(outside_clicks.get() + 1);
                        _ = app.update(cx, |this, cx| {
                            this.open = false;
                            cx.notify();
                        });
                    }),
                )
        }
    }

    #[gpui::test]
    fn side_popover_outside_click_closes_and_notifies_its_owner(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let outside_clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let outside_clicks = outside_clicks.clone();
            move |_, _| SidePopoverDismissFixture {
                open: true,
                outside_clicks,
            }
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        draw(cx);
        assert!(cx.debug_bounds("dismiss-side-popover-content").is_some());

        cx.simulate_click(point(px(4.), px(4.)), Modifiers::default());
        draw(cx);
        assert_eq!(outside_clicks.get(), 1);
        assert!(cx.debug_bounds("dismiss-side-popover-content").is_none());
    }

    #[gpui::test]
    fn anchored_popover_opens_above_later_content_and_owns_pointer_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let popup_clicks = Rc::new(Cell::new(0));
        let lower_clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let popup_clicks = popup_clicks.clone();
            let lower_clicks = lower_clicks.clone();
            move |_, _| PopoverFixture {
                popup_clicks,
                lower_clicks,
            }
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        let trigger = cx.debug_bounds("anchored-popover-trigger").unwrap();
        cx.simulate_click(trigger.center(), Modifiers::default());
        draw(cx);

        let content = cx.debug_bounds("anchored-popover-content").unwrap();
        assert!(content.right() <= trigger.right());
        cx.simulate_click(content.center(), Modifiers::default());
        draw(cx);

        assert_eq!(popup_clicks.get(), 1);
        assert_eq!(lower_clicks.get(), 0);

        cx.simulate_click(point(px(4.), px(4.)), Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("anchored-popover-content").is_none());
    }
}

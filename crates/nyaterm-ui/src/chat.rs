//! Chat presentation primitives at the GPUI Kit integration boundary.

pub use gpui_kit::component::message_scroller::{
    MessageScroller as NyaMessageScroller, MessageScrollerState as NyaMessageScrollerState,
};
pub use gpui_kit::component::shimmer::ShimmerText as NyaShimmerText;

use gpui::{
    Animation, AnimationExt, App, ClickEvent, Hsla, IntoElement, RenderOnce, SharedString,
    SpringAnimation, SpringConfig, Transformation, Window, div, percentage, prelude::*, px, svg,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use std::time::Duration;

type DisclosureClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A secondary disclosure with its own typography, retaining Kit's focus and
/// keyboard activation behavior. The caller owns the expanded state.
#[derive(IntoElement)]
pub struct NyaDisclosure {
    id: SharedString,
    label: SharedString,
    open: bool,
    on_click: Option<DisclosureClickHandler>,
}

impl NyaDisclosure {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, open: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            open,
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for NyaDisclosure {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let hover_group = self.id.clone();
        let caret = svg()
            .path("icons/menu/caret.svg")
            .size(px(10.))
            .text_color(muted)
            .flex_none()
            .with_spring(
                "caret",
                SpringAnimation::new(SpringConfig::new(2500., 100., 1.))
                    .to(if self.open { 0.25_f32 } else { 0. })
                    .with_epsilon(0.01),
                |icon, angle| icon.with_transformation(Transformation::rotate(percentage(angle))),
            );
        let mut button = Button::new(self.id)
            .accessibility_label(self.label.clone())
            .toggled(self.open)
            .custom(ButtonCustomVariant::new(cx).foreground(muted))
            .h(px(20.))
            .px(px(0.))
            .py(px(0.))
            .border_0()
            .group(hover_group.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .group_hover(hover_group, move |style| style.text_color(foreground))
                    .child(caret)
                    .child(self.label),
            );
        if let Some(on_click) = self.on_click {
            button = button.on_click(on_click);
        }
        button
    }
}

/// A single subtle activity indicator; callers remove it when execution settles.
pub fn running_indicator(id: impl Into<SharedString>, color: impl Into<Hsla>) -> impl IntoElement {
    svg()
        .path("icons/conn/spinner-arc.svg")
        .size(px(12.))
        .text_color(color)
        .with_animation(
            id.into(),
            Animation::new(Duration::from_secs(2)).repeat(),
            |icon, progress| icon.with_transformation(Transformation::rotate(percentage(progress))),
        )
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{
        AppContext, AssetSource, Bounds, Context, HeadlessAppContext, IntoElement, KeyDownEvent,
        KeyUpEvent, Keystroke, ListAlignment, ListOffset, ListState, Modifiers, NoopTextSystem,
        Render, SharedString, TestAppContext, Window, div, font, list, point, prelude::*, px, rgb,
        size,
    };
    use gpui_base::ElementExt as _;

    use crate::chat::{NyaDisclosure, running_indicator};
    use crate::{apply_component_theme, theme_palette};

    #[derive(Default)]
    struct IconAssets {
        loaded: AtomicUsize,
    }

    impl AssetSource for IconAssets {
        fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
            let (bit, bytes): (usize, &'static [u8]) = match path {
                "icons/menu/caret.svg" => (
                    1,
                    include_bytes!("../../nyaterm-app/assets/icons/menu/caret.svg"),
                ),
                "icons/conn/spinner-arc.svg" => (
                    2,
                    include_bytes!("../../nyaterm-app/assets/icons/conn/spinner-arc.svg"),
                ),
                _ => return Ok(None),
            };
            self.loaded.fetch_or(bit, Ordering::Relaxed);
            Ok(Some(Cow::Borrowed(bytes)))
        }

        fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
            Ok(Vec::new())
        }
    }

    struct IconsFixture;

    impl Render for IconsFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .gap_2()
                .child(NyaDisclosure::new("disclosure", "Details", true))
                .child(running_indicator("running", rgb(0x3366ff)))
        }
    }

    struct DisclosureListFixture {
        state: ListState,
        request: Rc<Cell<bool>>,
        activations: Rc<Cell<usize>>,
    }

    impl Render for DisclosureListFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let request = self.request.clone();
            let activations = self.activations.clone();
            list(self.state.clone(), move |index, _, _| {
                let activations = activations.clone();
                let request = request.clone();
                div()
                    .id(("disclosure-row", index))
                    .relative()
                    .h(px(100.))
                    .w_full()
                    .child(
                        div()
                            .debug_selector(move || format!("disclosure-{index}"))
                            .child(
                                NyaDisclosure::new(format!("disclosure-{index}"), "Details", false)
                                    .on_click(move |_, _, _| {
                                        activations.set(activations.get() + 1);
                                    }),
                            ),
                    )
                    .on_prepaint(move |bounds, window, _| {
                        if request.get() && index == 4 {
                            window.request_autoscroll(Bounds::from_corners(
                                point(bounds.left(), bounds.top() + px(80.)),
                                point(bounds.right(), bounds.top() + px(90.)),
                            ));
                        }
                    })
                    .into_any_element()
            })
            .w(px(200.))
            .h(px(200.))
        }
    }

    #[cfg(not(target_family = "wasm"))]
    #[gpui::test]
    fn disclosures_remain_mouse_and_keyboard_operable_after_accessible_autoscroll_retry(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init_components);
        let state = ListState::new(10, ListAlignment::Top, px(0.));
        let offset = ListOffset {
            item_ix: 2,
            offset_in_item: px(50.),
        };
        state.scroll_to(offset);
        let request = Rc::new(Cell::new(false));
        let activations = Rc::new(Cell::new(0));
        let (_, vcx) = cx.add_window_view(|_, _| DisclosureListFixture {
            state: state.clone(),
            request: request.clone(),
            activations: activations.clone(),
        });
        assert!(vcx.activate_accessibility());
        request.set(true);
        state.scroll_to(offset);
        vcx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            assert!(window.is_a11y_active());
        });
        assert_eq!(state.logical_scroll_top().offset_in_item, px(90.));
        request.set(false);
        let bounds = vcx.debug_bounds("disclosure-3").unwrap();
        vcx.simulate_click(
            point(bounds.left() + px(5.), bounds.top() + px(10.)),
            Modifiers::default(),
        );
        assert_eq!(activations.get(), 1);
        vcx.update(|window, cx| {
            window.blur(cx);
            // The first row is partially clipped; Tab twice reaches the
            // visible disclosure whose mouse activation we just exercised.
            window.focus_next(cx);
            window.focus_next(cx);
            window.draw(cx).clear(cx);
            assert!(window.focused(cx).is_some());
        });
        let enter = Keystroke::parse("enter").unwrap();
        vcx.simulate_event(KeyDownEvent {
            keystroke: enter.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        vcx.simulate_event(KeyUpEvent { keystroke: enter });
        assert_eq!(activations.get(), 2);
    }

    #[test]
    fn disclosure_and_spinner_reach_svg_paint_with_and_without_reduced_motion() {
        for theme in ["github-dark", "solarized-light"] {
            for reduce_motion in [false, true] {
                let assets = Arc::new(IconAssets::default());
                let mut cx = HeadlessAppContext::with_asset_source(
                    Arc::new(NoopTextSystem::new()),
                    assets.clone(),
                );
                cx.update(|cx| {
                    gpui_kit::init(cx);
                    apply_component_theme(theme_palette(theme), font("Arial"), px(12.), cx);
                    cx.set_reduce_motion(reduce_motion);
                });
                let window = cx
                    .open_window(size(px(200.), px(40.)), |_, cx| cx.new(|_| IconsFixture))
                    .unwrap();
                cx.update_window(window.into(), |_, window, cx| {
                    _ = window.draw(cx);
                })
                .unwrap();
                // SVG layout alone does not load assets: this proves the paint
                // branch ran, which requires a color on the SVG itself.
                assert_eq!(assets.loaded.load(Ordering::Relaxed), 3);
            }
        }
    }
}

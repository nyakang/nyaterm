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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{
        AppContext, AssetSource, Context, HeadlessAppContext, IntoElement, NoopTextSystem, Render,
        SharedString, Window, div, font, prelude::*, px, rgb, size,
    };

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

use gpui::{AnyView, App, IntoElement, SharedString, Styled as _, Window, px};
use gpui_kit::component::tooltip::Tooltip;

use crate::theme_bridge::component_typography;

/// Theme-aware tooltip backed by `gpui-kit`.
pub struct NyaTooltip {
    inner: Tooltip,
}

impl NyaTooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            inner: Tooltip::new(text.into()),
        }
    }

    pub fn element<E: IntoElement>(builder: impl Fn(&mut Window, &mut App) -> E + 'static) -> Self {
        Self {
            inner: Tooltip::element(builder),
        }
    }

    /// Tightens outer margin and uses caption-sized text for dense edge bars
    /// where the default 12px margin pushes tooltips into adjacent surfaces.
    pub fn compact(mut self) -> Self {
        self.inner = self
            .inner
            .m(px(4.))
            .px(px(6.))
            .py(px(2.))
            .text_size(px(11.));
        self
    }

    pub fn build(self, window: &mut Window, cx: &mut App) -> AnyView {
        self.inner
            .font(component_typography(cx).font)
            .build(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, FontFallbacks, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, font, px,
    };
    use gpui_kit::component::tooltip::Tooltip;

    use super::NyaTooltip;

    struct EmptyView;

    impl Render for EmptyView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    #[gpui::test]
    fn tooltip_uses_the_current_ui_font_fallbacks(cx: &mut TestAppContext) {
        let mut ui_font = font("JetBrains Mono NL");
        ui_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
            "Noto Sans SC".to_string(),
            "Microsoft YaHei UI".to_string(),
        ]));
        cx.update(|cx| {
            crate::apply_component_theme(
                crate::theme::theme_palette("github-dark"),
                ui_font.clone(),
                px(16.),
                cx,
            );
        });

        let (_, cx) = cx.add_window_view(|_, _| EmptyView);
        cx.update(|window, cx| {
            let tooltip = NyaTooltip::new("安全认证")
                .build(window, cx)
                .downcast::<Tooltip>()
                .expect("NyaTooltip should build a gpui-kit tooltip");
            tooltip.update(cx, |tooltip, _| {
                let style = &tooltip.style().text;
                assert_eq!(style.font_family.as_deref(), Some("JetBrains Mono NL"));
                assert_eq!(style.font_fallbacks, ui_font.fallbacks);
            });
        });

        let changed_font = font("Noto Sans SC");
        cx.update(|_, cx| {
            crate::apply_component_theme(
                crate::theme::theme_palette("github-dark"),
                changed_font.clone(),
                px(16.),
                cx,
            );
        });
        cx.update(|window, cx| {
            for tooltip in [
                NyaTooltip::new("安全认证").build(window, cx),
                NyaTooltip::element(|_, _| gpui::div().child("安全认证")).build(window, cx),
            ] {
                let tooltip = tooltip
                    .downcast::<Tooltip>()
                    .expect("NyaTooltip should build a gpui-kit tooltip");
                tooltip.update(cx, |tooltip, _| {
                    let style = &tooltip.style().text;
                    assert_eq!(style.font_family.as_deref(), Some("Noto Sans SC"));
                    assert_eq!(style.font_fallbacks, changed_font.fallbacks);
                });
            }
        });
    }

    #[gpui::test]
    fn compact_tooltip_tightens_margin_and_text_size(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| EmptyView);
        cx.update(|window, cx| {
            let tooltip = NyaTooltip::new("关闭发送标签页")
                .compact()
                .build(window, cx)
                .downcast::<Tooltip>()
                .expect("NyaTooltip should build a gpui-kit tooltip");
            tooltip.update(cx, |tooltip, _| {
                assert_eq!(tooltip.style().margin.bottom, Some(px(4.).into()));
                assert_eq!(tooltip.style().margin.top, Some(px(4.).into()));
                assert!(tooltip.style().text.font_size.is_some());
            });
        });
    }
}

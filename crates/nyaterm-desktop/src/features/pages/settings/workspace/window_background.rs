use gpui::{
    Context, IntoElement, KeyDownEvent, MouseButton, SharedString, div, prelude::*, px, rgb,
};
use nyaterm_core::models::window_material::WindowMaterial;
use nyaterm_ui::{NyaSelectOption, NyaSlider};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::{settings_form_row, settings_form_section};
use crate::features::shell::window_material::resolve_window_material;
use crate::platform::window_material_capabilities;

#[derive(Clone, Copy)]
pub(super) enum AppearanceOpacity {
    Image,
    Content,
    Window,
}

impl SettingsPanel {
    pub(super) fn window_background_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.theme_palette();
        let requested = self.settings.summary().window_material.clone();
        let effective = resolve_window_material(&requested, window_material_capabilities());
        let native = effective != gpui::WindowBackgroundAppearance::Opaque;
        let mut options = vec![
            NyaSelectOption::new("opaque", t!("settings.windowMaterialOpaque")),
            NyaSelectOption::new("transparent", t!("settings.windowMaterialTransparent")),
            NyaSelectOption::new("blurred", t!("settings.windowMaterialBlurred")),
        ];
        if let WindowMaterial::Unknown(value) = &requested {
            options.push(NyaSelectOption::new(
                value.clone(),
                t!("settings.windowMaterialUnknown"),
            ));
        }
        let material = self.select_control(
            "appearance-window-material",
            options,
            Some(requested.as_str().into()),
            false,
            cx,
        );
        let fallback = requested != WindowMaterial::Opaque && !native;
        let content_enabled = native || self.settings.summary().background_image_path.is_some();
        settings_form_section(
            palette,
            Some(t!("settings.windowBackground")),
            None,
            div()
                .flex()
                .flex_col()
                .gap_5()
                .child(settings_form_row(
                    palette,
                    t!("settings.windowMaterial"),
                    Some(SharedString::from(t!("settings.windowMaterialDesc"))),
                    material,
                ))
                .when(fallback, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("settings.windowMaterialFallback")),
                    )
                })
                .child(self.appearance_opacity_slider(AppearanceOpacity::Window, native, cx))
                .child(self.appearance_opacity_slider(
                    AppearanceOpacity::Content,
                    content_enabled,
                    cx,
                )),
        )
    }

    pub(super) fn appearance_opacity_slider(
        &mut self,
        target: AppearanceOpacity,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (value, slider, focus, label, desc) = match target {
            AppearanceOpacity::Image => (
                self.settings.summary().background_image_opacity,
                self.image_opacity_slider.clone(),
                self.image_opacity_focus.clone(),
                t!("settings.backgroundImageOpacity"),
                None,
            ),
            AppearanceOpacity::Content => (
                self.settings.summary().background_content_opacity,
                self.content_opacity_slider.clone(),
                self.content_opacity_focus.clone(),
                t!("settings.backgroundContentOpacity"),
                Some(t!("settings.backgroundContentOpacityDesc")),
            ),
            AppearanceOpacity::Window => (
                self.settings.summary().window_background_opacity,
                self.window_opacity_slider.clone(),
                self.window_opacity_focus.clone(),
                t!("settings.windowBackgroundOpacity"),
                None,
            ),
        };
        let mouse_focus = focus.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .track_focus(&focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| window.focus(&mouse_focus, cx)),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if !enabled {
                    return;
                }
                let current = match target {
                    AppearanceOpacity::Image => this.settings.summary().background_image_opacity,
                    AppearanceOpacity::Content => {
                        this.settings.summary().background_content_opacity
                    }
                    AppearanceOpacity::Window => this.settings.summary().window_background_opacity,
                };
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => current.saturating_sub(1),
                    "right" | "up" => current.saturating_add(1).min(100),
                    "home" => 0,
                    "end" => 100,
                    _ => return,
                };
                match target {
                    AppearanceOpacity::Image => this.set_background_image_opacity(next, cx),
                    AppearanceOpacity::Content => this.set_background_content_opacity(next, cx),
                    AppearanceOpacity::Window => this.set_window_background_opacity(next, cx),
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight(500.))
                                    .text_color(rgb(palette.text))
                                    .child(label),
                            )
                            .when_some(desc, |this, desc| {
                                this.child(
                                    div()
                                        .mt_1()
                                        .text_size(px(11.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(desc),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.input))
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(format!("{value}%")),
                    ),
            )
            .child(NyaSlider::new(&slider).disabled(!enabled).w_full())
    }
}

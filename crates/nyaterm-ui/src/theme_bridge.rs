//! Bridge from NyaTerm's persisted theme palette to gpui-kit's theme.

use std::sync::Arc;

use gpui::{App, Font, Global, Hsla, Pixels, font, hsla, px, rgb, transparent_black};
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{Theme, ThemeMode, ThemeTokens};

use crate::theme::ThemePalette;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComponentTypography {
    pub(crate) font: Font,
    pub(crate) font_size: Pixels,
}

impl Default for ComponentTypography {
    fn default() -> Self {
        Self {
            font: font(".SystemUIFont"),
            font_size: px(16.),
        }
    }
}

impl Global for ComponentTypography {}

pub(crate) fn component_typography(cx: &App) -> ComponentTypography {
    cx.try_global::<ComponentTypography>()
        .cloned()
        .unwrap_or_default()
}

fn color(rgb_value: u32) -> Hsla {
    rgb(rgb_value).into()
}

fn relative_luminance(rgb_value: u32) -> f32 {
    let channel = |c: u32| -> f32 {
        let v = (c as f32) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let r = channel((rgb_value >> 16) & 0xff);
    let g = channel((rgb_value >> 8) & 0xff);
    let b = channel(rgb_value & 0xff);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn theme_mode(palette: ThemePalette) -> ThemeMode {
    if relative_luminance(palette.bg) < 0.5 {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

pub fn apply_component_theme(
    palette: ThemePalette,
    ui_font: Font,
    ui_font_size: Pixels,
    cx: &mut App,
) {
    if !cx.has_global::<Theme>() {
        gpui_kit::init(cx);
    }

    let mode = theme_mode(palette);
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }

    let ui_font_family = ui_font.family.clone();
    cx.set_global(ComponentTypography {
        font: ui_font,
        font_size: ui_font_size,
    });

    let component_theme = Theme::global_mut(cx);
    component_theme.mode = mode;
    component_theme.font_family = ui_font_family;
    component_theme.font_size = ui_font_size;
    component_theme.colors.background = color(palette.bg);
    component_theme.colors.foreground = color(palette.text);
    component_theme.colors.muted = color(palette.surface_elevated);
    component_theme.colors.muted_foreground = color(palette.text_muted);
    component_theme.colors.border = color(palette.border);
    component_theme.colors.button = color(palette.surface_elevated);
    component_theme.colors.button_active = color(palette.hover);
    component_theme.colors.button_foreground = color(palette.text);
    component_theme.colors.button_hover = color(palette.hover);
    component_theme.colors.button_danger = color(palette.danger);
    component_theme.colors.button_danger_active = color(palette.danger);
    component_theme.colors.button_danger_foreground = color(palette.on_primary);
    component_theme.colors.button_danger_hover = color(palette.danger);
    component_theme.colors.button_primary = color(palette.primary);
    component_theme.colors.button_primary_active = color(palette.primary_hover);
    component_theme.colors.button_primary_foreground = color(palette.on_primary);
    component_theme.colors.button_primary_hover = color(palette.primary_hover);
    component_theme.colors.button_secondary = color(palette.surface_elevated);
    component_theme.colors.button_secondary_active = color(palette.hover);
    component_theme.colors.button_secondary_foreground = color(palette.text);
    component_theme.colors.button_secondary_hover = color(palette.hover);
    component_theme.colors.button_success = color(palette.success);
    component_theme.colors.button_success_active = color(palette.success);
    component_theme.colors.button_success_foreground = color(palette.on_primary);
    component_theme.colors.button_success_hover = color(palette.success);
    component_theme.colors.button_warning = color(palette.warning);
    component_theme.colors.button_warning_active = color(palette.warning);
    component_theme.colors.button_warning_foreground = color(palette.on_primary);
    component_theme.colors.button_warning_hover = color(palette.warning);
    component_theme.colors.button_info = color(palette.primary);
    component_theme.colors.button_info_active = color(palette.primary_hover);
    component_theme.colors.button_info_foreground = color(palette.on_primary);
    component_theme.colors.button_info_hover = color(palette.primary_hover);
    component_theme.colors.group_box = color(palette.surface);
    component_theme.colors.group_box_foreground = color(palette.text);
    component_theme.colors.input = color(palette.border);
    component_theme.colors.caret = color(palette.focus_ring);
    component_theme.colors.primary = color(palette.primary);
    component_theme.colors.primary_hover = color(palette.primary_hover);
    component_theme.colors.primary_active = color(palette.primary_hover);
    component_theme.colors.primary_foreground = color(palette.on_primary);
    component_theme.colors.info = color(palette.primary);
    component_theme.colors.info_hover = color(palette.primary_hover);
    component_theme.colors.info_active = color(palette.primary_hover);
    component_theme.colors.info_foreground = color(palette.on_primary);
    component_theme.colors.link = color(palette.link);
    component_theme.colors.link_active = color(palette.link);
    component_theme.colors.link_hover = color(palette.link);
    component_theme.colors.list = color(palette.surface);
    component_theme.colors.list_active = color(palette.hover);
    component_theme.colors.list_active_border = color(palette.focus_ring);
    component_theme.colors.list_even = color(palette.surface);
    component_theme.colors.list_head = color(palette.surface_elevated);
    component_theme.colors.list_hover = color(palette.hover);
    component_theme.colors.secondary = color(palette.surface_elevated);
    component_theme.colors.secondary_hover = color(palette.hover);
    component_theme.colors.secondary_active = color(palette.hover);
    component_theme.colors.secondary_foreground = color(palette.text);
    component_theme.colors.danger = color(palette.danger);
    component_theme.colors.danger_hover = color(palette.danger);
    component_theme.colors.danger_active = color(palette.danger);
    component_theme.colors.danger_foreground = color(palette.on_primary);
    component_theme.colors.ring = color(palette.focus_ring);
    component_theme.colors.accent = color(palette.hover);
    component_theme.colors.accent_foreground = color(palette.text);
    component_theme.colors.accordion = color(palette.surface);
    component_theme.colors.description_list_label = color(palette.surface_elevated);
    component_theme.colors.description_list_label_foreground = color(palette.text_muted);
    component_theme.colors.drag_border = color(palette.focus_ring);
    component_theme.colors.drop_target = color(palette.hover);
    component_theme.colors.popover = color(palette.surface);
    component_theme.colors.popover_foreground = color(palette.text);
    component_theme.colors.progress_bar = color(palette.primary);
    component_theme.colors.sidebar = color(palette.surface);
    component_theme.colors.sidebar_accent = color(palette.hover);
    component_theme.colors.sidebar_accent_foreground = color(palette.text);
    component_theme.colors.sidebar_border = color(palette.border);
    component_theme.colors.sidebar_foreground = color(palette.text);
    component_theme.colors.sidebar_primary = color(palette.primary);
    component_theme.colors.sidebar_primary_foreground = color(palette.on_primary);
    component_theme.colors.skeleton = color(palette.hover);
    component_theme.colors.slider_bar = color(palette.primary);
    component_theme.colors.slider_thumb = color(palette.on_primary);
    component_theme.colors.success = color(palette.success);
    component_theme.colors.success_hover = color(palette.success);
    component_theme.colors.success_active = color(palette.success);
    component_theme.colors.success_foreground = color(palette.on_primary);
    component_theme.colors.warning = color(palette.warning);
    component_theme.colors.warning_hover = color(palette.warning);
    component_theme.colors.warning_active = color(palette.warning);
    component_theme.colors.warning_foreground = color(palette.on_primary);
    component_theme.colors.title_bar = color(palette.surface);
    component_theme.colors.title_bar_border = color(palette.border);
    component_theme.colors.status_bar = color(palette.surface);
    component_theme.colors.status_bar_border = color(palette.border);
    component_theme.colors.selection = color(palette.terminal_selection);
    component_theme.colors.scrollbar = transparent_black();
    component_theme.colors.scrollbar_thumb = color(palette.border);
    component_theme.colors.scrollbar_thumb_hover = color(palette.text_dimmed);
    // Zed-style auto-hide: fade the bar in while the pointer is over the panel
    // or that axis is scrolling, and out after idle. Set explicitly so the
    // behavior cannot change silently under a vendor bump.
    component_theme.scrollbar_mode = ScrollbarMode::Hover;
    component_theme.colors.switch = color(palette.border);
    component_theme.colors.switch_thumb = color(palette.surface);
    component_theme.colors.tab = color(palette.input);
    component_theme.colors.tab_active = color(palette.input);
    component_theme.colors.tab_active_foreground = color(palette.text);
    component_theme.colors.tab_bar = color(palette.surface_elevated);
    component_theme.colors.tab_bar_segmented = color(palette.surface_elevated);
    component_theme.colors.tab_foreground = color(palette.text_muted);
    component_theme.colors.table = color(palette.surface);
    component_theme.colors.table_active = color(palette.hover);
    component_theme.colors.table_active_border = color(palette.focus_ring);
    component_theme.colors.table_even = color(palette.surface);
    component_theme.colors.table_head = color(palette.surface_elevated);
    component_theme.colors.table_head_foreground = color(palette.text_muted);
    component_theme.colors.table_foot = color(palette.surface_elevated);
    component_theme.colors.table_foot_foreground = color(palette.text_muted);
    component_theme.colors.table_hover = color(palette.hover);
    component_theme.colors.table_row_border = color(palette.border);
    component_theme.colors.overlay = hsla(0., 0., 0., if mode.is_dark() { 0.2 } else { 0.05 });
    component_theme.colors.window_border = color(palette.border);
    component_theme.colors.red = color(palette.danger);
    component_theme.colors.red_light = color(palette.danger);
    component_theme.colors.green = color(palette.success);
    component_theme.colors.green_light = color(palette.success);
    component_theme.colors.blue = color(palette.primary);
    component_theme.colors.blue_light = color(palette.primary_hover);
    component_theme.colors.yellow = color(palette.warning);
    component_theme.colors.yellow_light = color(palette.warning);
    component_theme.colors.magenta = color(palette.accent);
    component_theme.colors.magenta_light = color(palette.accent);
    component_theme.colors.cyan = color(palette.link);
    component_theme.colors.cyan_light = color(palette.link);
    let editor_style = &mut Arc::make_mut(&mut component_theme.highlight_theme).style;
    editor_style.editor_background = Some(color(palette.input));
    editor_style.editor_active_line = None;
    editor_style.editor_gutter_background = Some(color(palette.surface_elevated));
    editor_style.editor_line_number = Some(color(palette.text_dimmed));
    editor_style.editor_active_line_number = Some(color(palette.text));
    component_theme.tokens = ThemeTokens::from(&component_theme.colors);

    // `Scrollbar` reads mode, motion, and thumb colors from the Base theme, and
    // the assignments above only touch the component theme. Without this the bar
    // keeps whatever projection the last `Theme::change` built - which happens
    // only on a light/dark flip, from the built-in palette rather than ours.
    Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use gpui::{FontFallbacks, TestAppContext, font, px};
    use gpui_kit::component::scroll::ScrollbarMode;
    use gpui_kit::component::{Theme, ThemeMode};

    use super::{
        apply_component_theme as apply_component_theme_with_typography, color, component_typography,
    };
    use crate::theme::theme_palette;

    fn apply_component_theme(palette: crate::theme::ThemePalette, cx: &mut gpui::App) {
        apply_component_theme_with_typography(palette, font("Inter"), px(16.), cx);
    }

    #[test]
    fn applying_component_theme_initializes_missing_component_theme() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            assert!(!cx.has_global::<Theme>());

            apply_component_theme(theme_palette("github-dark"), cx);

            assert!(cx.has_global::<Theme>());
        });
    }

    #[test]
    fn component_typography_follows_the_application_font_and_size() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            let mut ui_font = font("Noto Sans");
            ui_font.fallbacks = Some(FontFallbacks::from_fonts(vec![
                "Microsoft YaHei UI".to_string(),
                "Segoe UI".to_string(),
            ]));

            apply_component_theme_with_typography(
                theme_palette("github-dark"),
                ui_font.clone(),
                px(19.),
                cx,
            );

            let theme = Theme::global(cx);
            assert_eq!(theme.font_family, ui_font.family);
            assert_eq!(theme.font_size, px(19.));
            assert_eq!(
                gpui_base::Theme::global(cx).tokens.typography.sans,
                ui_font.family
            );
            assert_eq!(
                gpui_base::Theme::global(cx).tokens.typography.md.size,
                px(19.)
            );

            let typography = component_typography(cx);
            assert_eq!(typography.font, ui_font);
            assert_eq!(typography.font_size, px(19.));
        });
    }

    #[test]
    fn segmented_tab_tokens_follow_nyaterm_palette() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);
            let palette = theme_palette("github-dark");

            apply_component_theme(palette, cx);

            let tokens = &Theme::global(cx).tokens;
            assert_eq!(
                tokens.tab_bar_segmented.color,
                color(palette.surface_elevated)
            );
            assert_eq!(tokens.tab_bar.color, color(palette.surface_elevated));
            assert_eq!(tokens.tab.color, color(palette.input));
            assert_eq!(tokens.tab_active.color, color(palette.input));
            assert_eq!(tokens.tab_active_foreground.color, color(palette.text));
            assert_eq!(tokens.tab_foreground.color, color(palette.text_muted));
            assert_eq!(tokens.background.color, color(palette.bg));
            assert_eq!(tokens.secondary.color, color(palette.surface_elevated));
            assert_eq!(tokens.primary.color, color(palette.primary));
        });
    }

    #[test]
    fn editor_colors_follow_the_nyaterm_palette() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);
            let palette = theme_palette("github-dark");

            apply_component_theme(palette, cx);

            let style = &Theme::global(cx).highlight_theme.style;
            assert_eq!(style.editor_background, Some(color(palette.input)));
            assert_eq!(style.editor_active_line, None);
            assert_eq!(
                style.editor_gutter_background,
                Some(color(palette.surface_elevated))
            );
            assert_eq!(style.editor_line_number, Some(color(palette.text_dimmed)));
            assert_eq!(style.editor_active_line_number, Some(color(palette.text)));
        });
    }

    #[test]
    fn popover_tokens_follow_nyaterm_palette() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);
            let palette = theme_palette("github-dark");

            apply_component_theme(palette, cx);

            let tokens = &Theme::global(cx).tokens;
            assert_eq!(tokens.popover.color, color(palette.surface));
            assert_eq!(tokens.popover_foreground.color, color(palette.text));
            assert_eq!(tokens.button.color, color(palette.surface_elevated));
            assert_eq!(tokens.list_hover.color, color(palette.hover));
        });
    }

    #[test]
    fn component_theme_mode_follows_palette_luminance() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);

            apply_component_theme(theme_palette("github-dark"), cx);
            assert_eq!(Theme::global(cx).mode, ThemeMode::Dark);
            assert!(Theme::global(cx).is_dark());

            apply_component_theme(theme_palette("github-light"), cx);
            assert_eq!(Theme::global(cx).mode, ThemeMode::Light);
            assert!(!Theme::global(cx).is_dark());
        });
    }

    #[test]
    fn scrollbar_track_is_transparent_so_container_background_shows() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);
            let palette = theme_palette("github-dark");

            apply_component_theme(palette, cx);

            assert_eq!(
                Theme::global(cx).colors.scrollbar,
                gpui::transparent_black()
            );
        });
    }

    #[test]
    fn scrollbars_reveal_on_hover_and_fade_after_idle() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);
            let palette = theme_palette("github-dark");

            apply_component_theme(palette, cx);

            assert_eq!(
                Theme::global(cx).scrollbar_mode,
                ScrollbarMode::Hover,
                "panels fade their scrollbar in when the pointer enters or the axis \
                 scrolls, and out after idle"
            );
            assert_eq!(
                gpui_base::Theme::global(cx).scrollbar.mode(),
                ScrollbarMode::Hover,
                "the Base projection is what `Scrollbar` actually reads; assigning the \
                 component-theme field alone leaves the painted bar on the old mode"
            );
        });
    }

    #[test]
    fn applying_a_same_luminance_palette_still_reprojects_onto_base() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);

            apply_component_theme(theme_palette("github-dark"), cx);

            // Stand in for a projection left behind by an earlier theme. Only
            // `Theme::change` and the bridge's sync rebuild it, and a dark -> dark
            // apply skips the former - which is how the palette's scrollbar colors
            // used to never reach the painted bar.
            gpui_base::Theme::global_mut(cx).scrollbar =
                gpui_base::ScrollbarTheme::new().with_mode(ScrollbarMode::Always);

            apply_component_theme(theme_palette("gruvbox-dark"), cx);

            assert_eq!(
                gpui_base::Theme::global(cx).scrollbar.mode(),
                ScrollbarMode::Hover,
                "a palette change with no light/dark flip must still re-project"
            );
        });
    }

    #[test]
    fn dialog_overlay_is_translucent_so_window_context_remains_visible() {
        let cx = TestAppContext::single();

        cx.update(|cx| {
            gpui_kit::init(cx);

            apply_component_theme(theme_palette("github-dark"), cx);
            assert_eq!(Theme::global(cx).colors.overlay.a, 0.2);

            apply_component_theme(theme_palette("github-light"), cx);
            assert_eq!(Theme::global(cx).colors.overlay.a, 0.05);
        });
    }
}

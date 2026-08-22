use rust_i18n::t;

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Font, FontFallbacks, PathPromptOptions, RenderImage, SharedString,
    font, px, rgb, rgba,
};
use nyaterm_core::{
    AppSettingsSummary, ResolvedKeywordHighlightRule, merge_keyword_highlight_rules_for_paint,
};

use crate::features::NyaTermApp;
pub(in crate::features) use crate::theme::{ThemePalette, apply_component_theme, theme_palette};

const TERMINAL_FONT_SIZE_MIN: i16 = 8;
const TERMINAL_FONT_SIZE_MAX: i16 = 72;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct ResolvedAppearanceFont {
    pub family: String,
    pub fallbacks: Option<FontFallbacks>,
    /// Ordered family names used to rebuild the descriptor after runtime fallback.
    pub fallback_families: Vec<String>,
}

impl ResolvedAppearanceFont {
    pub(in crate::features) fn font(&self) -> Font {
        let mut font = font(SharedString::from(self.family.clone()));
        font.fallbacks = self.fallbacks.clone();
        font
    }

    pub(in crate::features) fn with_primary_family(&self, family: &str) -> Self {
        let fallback_families = self
            .fallback_families
            .iter()
            .filter(|candidate| !candidate.eq_ignore_ascii_case(family))
            .cloned()
            .collect::<Vec<_>>();
        let fallbacks = (!fallback_families.is_empty())
            .then(|| FontFallbacks::from_fonts(fallback_families.clone()));
        Self {
            family: family.to_string(),
            fallbacks,
            fallback_families,
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn apply_gpui_settings(
        &mut self,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        self.settings.replace_summary(settings);
        self.invalidate_paint_theme_caches();
        self.queue_wallpaper_refresh(cx);
    }

    pub(in crate::features) fn sync_component_theme(&self, cx: &mut App) {
        apply_component_theme(self.theme_palette(), cx);
    }

    pub(in crate::features) fn gpui_terminal_font(&self) -> ResolvedAppearanceFont {
        self.terminal
            .terminal_font_override()
            .cloned()
            .unwrap_or_else(|| self.gpui_configured_terminal_font())
    }

    pub(in crate::features) fn gpui_configured_terminal_font(&self) -> ResolvedAppearanceFont {
        gpui_platform_font(
            &self.settings.summary().terminal_font_family,
            gpui_terminal_font_fallback(),
            true,
        )
    }

    pub(in crate::features) fn gpui_terminal_font_for_family(
        &self,
        family: &str,
    ) -> ResolvedAppearanceFont {
        gpui_platform_font(family, gpui_terminal_font_fallback(), true)
    }

    pub(in crate::features) fn gpui_ui_font(&self) -> ResolvedAppearanceFont {
        let raw = if self.settings.summary().ui_font_family.trim().is_empty() {
            self.settings.summary().terminal_font_family.as_str()
        } else {
            self.settings.summary().ui_font_family.as_str()
        };
        gpui_platform_font(raw, gpui_ui_font_fallback(), false)
    }

    pub(in crate::features) fn theme_palette(&self) -> ThemePalette {
        theme_palette(&self.settings.summary().theme)
    }

    pub(in crate::features) fn wallpaper_enabled(&self) -> bool {
        self.shell.wallpaper_asset().is_some()
    }

    pub(in crate::features) fn queue_wallpaper_refresh(&mut self, cx: &mut Context<Self>) {
        let path = self
            .settings
            .summary()
            .background_image_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string);
        if !self.shell.request_wallpaper(path.clone()) {
            return;
        }
        let Some(path) = path else {
            cx.notify();
            return;
        };
        let task_path = path.clone();
        let task = cx.background_spawn(async move { load_wallpaper_image(&task_path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((image, width, height)) => {
                        this.shell.cache_wallpaper(path, image, width, height);
                    }
                    Err(()) if this.shell.wallpaper_is_requested(&path) => {
                        this.shell
                            .set_status("wallpaper image could not be loaded".to_string());
                    }
                    Err(()) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn ensure_appearance_font_options(&mut self, cx: &mut Context<Self>) {
        if !self.settings.begin_font_options_load() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let Ok(system_fonts) = this.update(cx, |_, cx| cx.text_system().all_font_names())
            else {
                return;
            };
            let mut ui_fonts = Vec::with_capacity(system_fonts.len());
            for family in &system_fonts {
                push_unique_font(&mut ui_fonts, family.clone());
            }

            const FONT_SCAN_BATCH_SIZE: usize = 16;
            let mut terminal_fonts = Vec::new();
            for batch in system_fonts.chunks(FONT_SCAN_BATCH_SIZE) {
                let batch = batch.to_vec();
                let Ok(monospace_fonts) =
                    this.update(cx, |_, cx| appearance_monospace_font_options(cx, &batch))
                else {
                    return;
                };
                for family in monospace_fonts {
                    push_unique_font(&mut terminal_fonts, family);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
            push_unique_font(&mut terminal_fonts, "monospace".to_string());
            let _ = this.update(cx, |this, cx| {
                this.settings
                    .finish_font_options_load(ui_fonts, terminal_fonts);
                cx.notify();
            });
        })
        .detach();
    }

    /// Wallpaper opacity applies to surface backgrounds, not their contents.
    pub(in crate::features) fn shell_surface_color(&self, color: u32) -> gpui::Rgba {
        if !self.wallpaper_enabled() {
            return rgb(color);
        }
        let alpha = ((self.settings.summary().background_content_opacity.min(100) as f32 / 100.0)
            * 255.0)
            .round() as u32;
        rgba((color << 8) | alpha.min(0xff))
    }

    /// Tauri's terminal and explicitly transparent surfaces reveal wallpaper.
    pub(in crate::features) fn shell_transparent_color(&self, color: u32) -> gpui::Rgba {
        if self.wallpaper_enabled() {
            rgba(color << 8)
        } else {
            rgb(color)
        }
    }

    pub(in crate::features) fn terminal_theme_is_dark(&self) -> bool {
        let palette = self.terminal_theme_palette();
        let r = ((palette.terminal_bg >> 16) & 0xff) as f32;
        let g = ((palette.terminal_bg >> 8) & 0xff) as f32;
        let b = (palette.terminal_bg & 0xff) as f32;
        let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        lum < 0.5
    }

    pub(in crate::features) fn resolved_keyword_highlight_rules(
        &self,
    ) -> Arc<Vec<ResolvedKeywordHighlightRule>> {
        if self.settings.summary().terminal_low_latency_mode {
            return Arc::new(Vec::new());
        }
        if let Some(cached) = self.terminal.cached_keyword_highlight_rules() {
            return cached.clone();
        }
        // Cache miss (settings path / first call without ensure): build once without storing.
        if !self.settings.keyword_config().enabled {
            return Arc::new(Vec::new());
        }
        Arc::new(merge_keyword_highlight_rules_for_paint(
            &self.settings.keyword_config().rules,
            &self.settings.keyword_config().builtin_rules,
            self.terminal_theme_is_dark(),
        ))
    }

    /// Populate paint caches used by every terminal/chrome rebuild.
    pub(in crate::features) fn ensure_paint_theme_caches(&mut self) {
        self.ensure_terminal_theme_palette_cache();
        self.ensure_keyword_highlight_rules_cache();
    }

    fn ensure_keyword_highlight_rules_cache(&mut self) {
        if self.settings.summary().terminal_low_latency_mode {
            self.terminal
                .cache_keyword_highlight_rules(Arc::new(Vec::new()));
            return;
        }
        if self.terminal.cached_keyword_highlight_rules().is_some() {
            return;
        }
        let rules = if !self.settings.keyword_config().enabled {
            Arc::new(Vec::new())
        } else {
            // terminal_theme_is_dark uses palette; ensure palette first.
            self.ensure_terminal_theme_palette_cache();
            Arc::new(merge_keyword_highlight_rules_for_paint(
                &self.settings.keyword_config().rules,
                &self.settings.keyword_config().builtin_rules,
                self.terminal_theme_is_dark(),
            ))
        };
        self.terminal.cache_keyword_highlight_rules(rules);
    }

    /// Terminal surface palette: follows optional `terminal_theme`, else UI theme.
    pub(in crate::features) fn terminal_theme_palette(&self) -> ThemePalette {
        let ui_theme = self.settings.summary().theme.as_str();
        let terminal_theme = self
            .settings
            .summary()
            .terminal_theme
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("");
        let contrast = self.settings.summary().minimum_contrast_ratio.as_str();
        if let Some((cached_ui, cached_term, cached_contrast, palette)) =
            self.terminal.cached_terminal_theme_palette()
            && cached_ui == ui_theme
            && cached_term == terminal_theme
            && cached_contrast == contrast
        {
            return palette;
        }
        Self::compute_terminal_theme_palette(
            ui_theme,
            if terminal_theme.is_empty() {
                None
            } else {
                Some(terminal_theme)
            },
            contrast,
        )
    }

    fn ensure_terminal_theme_palette_cache(&mut self) {
        let ui_theme = self.settings.summary().theme.clone();
        let terminal_theme = self
            .settings
            .summary()
            .terminal_theme
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        let contrast = self.settings.summary().minimum_contrast_ratio.clone();
        if let Some((cached_ui, cached_term, cached_contrast, _)) =
            self.terminal.cached_terminal_theme_palette()
            && cached_ui == ui_theme
            && cached_term == terminal_theme
            && cached_contrast == contrast
        {
            return;
        }
        let palette = Self::compute_terminal_theme_palette(
            &ui_theme,
            if terminal_theme.is_empty() {
                None
            } else {
                Some(terminal_theme.as_str())
            },
            &contrast,
        );
        self.terminal
            .cache_terminal_theme_palette(ui_theme, terminal_theme, contrast, palette);
    }

    fn compute_terminal_theme_palette(
        ui_theme: &str,
        terminal_theme: Option<&str>,
        minimum_contrast_ratio: &str,
    ) -> ThemePalette {
        let id = terminal_theme.unwrap_or(ui_theme);
        let id = if id == "catppuccin" {
            "catppuccin-mocha"
        } else {
            id
        };
        let mut palette = theme_palette(id);
        palette.apply_minimum_contrast_ratio(parse_minimum_contrast_ratio(minimum_contrast_ratio));
        palette
    }

    pub(in crate::features) fn invalidate_paint_theme_caches(&mut self) {
        self.terminal.invalidate_paint_caches();
    }

    pub(in crate::features) fn refresh_visible_terminal_surfaces(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        for session_id in ids {
            self.sync_terminal_surface_paint(&session_id, cx);
        }
    }

    pub(in crate::features) fn update_appearance_theme(
        &mut self,
        theme: &str,
        cx: &mut Context<Self>,
    ) {
        // Normalize legacy Settings id "catppuccin" to Tauri mocha id.
        let theme = if theme == "catppuccin" {
            "catppuccin-mocha"
        } else {
            theme
        };
        self.settings.set_appearance_theme(theme.to_string());
        self.sync_component_theme(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn update_terminal_font_family(
        &mut self,
        family: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_terminal_font_family(family.to_string()) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn invalidate_terminal_cell_metrics(&mut self, cx: &mut Context<Self>) {
        self.terminal.invalidate_cell_metrics();
        // Refresh the measured metrics before resizing the terminal. Using the
        // font-size fallback here makes the app and surface briefly disagree;
        // that is especially visible while scrolled or dragging a selection.
        self.refresh_terminal_cell_metrics(cx);
        self.sync_terminal_cell_metrics_to_screens();
        self.terminal.invalidate_all_render_caches();
        self.resize_all_known_terminal_surfaces();
        self.refresh_visible_terminal_surfaces(cx);
    }

    pub(in crate::features) fn adjust_terminal_font_size(
        &mut self,
        delta: i16,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.summary().interaction_terminal_zoom_enabled {
            self.shell
                .set_status("Terminal zoom is disabled in Settings".to_string());
            cx.notify();
            return;
        }
        let next = (self.settings.summary().terminal_font_size as i16 + delta)
            .clamp(TERMINAL_FONT_SIZE_MIN, TERMINAL_FONT_SIZE_MAX);
        if !self.settings.set_terminal_font_size(next as u16) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_terminal_font_size_from_input(
        &mut self,
        size: u16,
        cx: &mut Context<Self>,
    ) {
        let next = (size as i16).clamp(TERMINAL_FONT_SIZE_MIN, TERMINAL_FONT_SIZE_MAX) as u16;
        if !self.settings.set_terminal_font_size(next) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn reset_terminal_font_size(&mut self, cx: &mut Context<Self>) {
        if !self.settings.summary().interaction_terminal_zoom_enabled {
            self.shell
                .set_status("Terminal zoom is disabled in Settings".to_string());
            cx.notify();
            return;
        }
        let default_size = AppSettingsSummary::default().terminal_font_size;
        if !self.settings.set_terminal_font_size(default_size) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_cursor_style(&mut self, style: &str, cx: &mut Context<Self>) {
        let normalized = match style {
            "underline" | "bar" => style,
            _ => "block",
        };
        self.settings.set_cursor_style(normalized.to_string());
        self.save_appearance_settings(cx);
        self.shell
            .set_status(format!("cursor style → {normalized}"));
    }

    pub(in crate::features) fn toggle_cursor_blink(&mut self, cx: &mut Context<Self>) {
        let cursor_blink = self.settings.toggle_cursor_blink();
        self.save_appearance_settings(cx);
        // Turning it on has to start the clock; turning it off lets the clock notice
        // on its next tick and leave the caret visible.
        self.ensure_cursor_blink_clock(cx);
        self.shell.set_status(if cursor_blink {
            "cursor blink on".to_string()
        } else {
            "cursor blink off".to_string()
        });
    }

    pub(in crate::features) fn set_terminal_theme(
        &mut self,
        theme: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let theme = theme.map(str::trim).filter(|s| !s.is_empty()).map(|s| {
            if s == "catppuccin" {
                "catppuccin-mocha".to_string()
            } else {
                s.to_string()
            }
        });
        self.settings.set_terminal_theme(theme);
        self.save_appearance_settings(cx);
        self.shell
            .set_status(match self.settings.summary().terminal_theme.as_deref() {
                Some(id) => format!("terminal theme → {id}"),
                None => "terminal theme → follow UI".to_string(),
            });
    }

    pub(in crate::features) fn set_minimum_contrast_ratio(
        &mut self,
        ratio: &str,
        cx: &mut Context<Self>,
    ) {
        let ratio = match ratio {
            "3" | "4.5" | "7" | "21" => ratio,
            _ => "1",
        };
        if !self.settings.set_minimum_contrast_ratio(ratio.to_string()) {
            return;
        }
        self.save_appearance_settings(cx);
        self.shell.set_status(format!("minimum contrast → {ratio}"));
    }

    pub(in crate::features) fn update_ui_font_family(
        &mut self,
        family: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_ui_font_family(family.to_string()) {
            return;
        }
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_appearance_font_stack_entry(
        &mut self,
        terminal: bool,
        index: usize,
        family: String,
        cx: &mut Context<Self>,
    ) {
        let raw = if terminal {
            &self.settings.summary().terminal_font_family
        } else {
            &self.settings.summary().ui_font_family
        };
        let fallback = if terminal { "JetBrains Mono" } else { "Inter" };
        let mut fonts = appearance_font_stack(raw, fallback);
        let Some(font) = fonts.get_mut(index) else {
            return;
        };
        *font = family;
        self.save_appearance_font_stack(terminal, fonts, cx);
    }

    pub(in crate::features) fn add_appearance_fallback_font(
        &mut self,
        terminal: bool,
        cx: &mut Context<Self>,
    ) {
        let raw = if terminal {
            &self.settings.summary().terminal_font_family
        } else {
            &self.settings.summary().ui_font_family
        };
        let fallback = if terminal { "JetBrains Mono" } else { "Inter" };
        let mut fonts = appearance_font_stack(raw, fallback);
        let next = if terminal {
            self.settings.terminal_font_options().first()
        } else {
            self.settings.ui_font_options().first()
        }
        .cloned()
        .unwrap_or_else(|| fallback.to_string());
        fonts.push(next);
        self.save_appearance_font_stack(terminal, fonts, cx);
    }

    pub(in crate::features) fn remove_appearance_font_stack_entry(
        &mut self,
        terminal: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let raw = if terminal {
            &self.settings.summary().terminal_font_family
        } else {
            &self.settings.summary().ui_font_family
        };
        let fallback = if terminal { "JetBrains Mono" } else { "Inter" };
        let mut fonts = appearance_font_stack(raw, fallback);
        if index >= fonts.len() {
            return;
        }
        fonts.remove(index);
        if fonts.is_empty() {
            fonts.push(fallback.to_string());
        }
        self.save_appearance_font_stack(terminal, fonts, cx);
    }

    fn save_appearance_font_stack(
        &mut self,
        terminal: bool,
        fonts: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let stack = fonts.join(", ");
        if terminal {
            self.update_terminal_font_family(&stack, cx);
        } else {
            self.update_ui_font_family(&stack, cx);
        }
    }

    pub(in crate::features) fn set_ui_font_size_from_input(
        &mut self,
        size: u16,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_ui_font_size(size.clamp(12, 24)) {
            return;
        }
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_terminal_font_weight(
        &mut self,
        weight: u16,
        cx: &mut Context<Self>,
    ) {
        let weight = match weight {
            300 | 400 | 500 | 600 | 700 | 800 => weight,
            _ => 400,
        };
        if !self.settings.set_terminal_font_weight(weight) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_terminal_font_weight_bold(
        &mut self,
        weight: u16,
        cx: &mut Context<Self>,
    ) {
        let weight = match weight {
            300 | 400 | 500 | 600 | 700 | 800 => weight,
            _ => 700,
        };
        if !self.settings.set_terminal_font_weight_bold(weight) {
            return;
        }
        self.invalidate_terminal_cell_metrics(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn zoom_terminal_in(&mut self, cx: &mut Context<Self>) {
        self.adjust_terminal_font_size(1, cx);
    }

    pub(in crate::features) fn zoom_terminal_out(&mut self, cx: &mut Context<Self>) {
        self.adjust_terminal_font_size(-1, cx);
    }

    pub(in crate::features) fn prompt_background_image(&mut self, cx: &mut Context<Self>) {
        if self.settings.summary().background_image_path.is_some() {
            // allow replace
        }
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(t!("settings.selectBackgroundImage"))),
        };
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting wallpaper image".to_string());
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = path {
                    this.settings
                        .select_background_image(path.display().to_string());
                    this.queue_wallpaper_refresh(cx);
                    this.save_appearance_settings(cx);
                    this.shell
                        .set_status("wallpaper image selected".to_string());
                } else {
                    this.shell
                        .set_status("wallpaper selection cancelled".to_string());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn clear_background_image(&mut self, cx: &mut Context<Self>) {
        self.settings.clear_background_image();
        self.queue_wallpaper_refresh(cx);
        self.save_appearance_settings(cx);
        self.shell.set_status("wallpaper cleared".to_string());
    }

    pub(in crate::features) fn set_background_image_fit(
        &mut self,
        fit: &str,
        cx: &mut Context<Self>,
    ) {
        let normalized = match fit {
            "contain" => "contain",
            "stretch" | "fill" => "stretch",
            "tile" => "tile",
            _ => "cover",
        };
        self.settings
            .set_background_image_fit(normalized.to_string());
        self.save_appearance_settings(cx);
        self.shell
            .set_status(format!("wallpaper fit → {normalized}"));
    }

    pub(in crate::features) fn set_background_image_opacity(
        &mut self,
        value: u8,
        cx: &mut Context<Self>,
    ) {
        let next = value.min(100);
        if !self.settings.set_background_image_opacity(next) {
            return;
        }
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn set_background_content_opacity(
        &mut self,
        value: u8,
        cx: &mut Context<Self>,
    ) {
        let next = value.min(100);
        if !self.settings.set_background_content_opacity(next) {
            return;
        }
        self.save_appearance_settings(cx);
    }

    fn save_appearance_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_persistence(cx) {
            self.refresh_visible_terminal_surfaces(cx);
            return;
        }
        self.refresh_visible_terminal_surfaces(cx);
        self.queue_settings_save(crate::features::settings::SettingsSaveKind::Appearance, cx);
    }
}

fn load_wallpaper_image(path: &str) -> Result<(Arc<RenderImage>, u32, u32), ()> {
    let reader = image::ImageReader::open(path)
        .map_err(|_| ())?
        .with_guessed_format()
        .map_err(|_| ())?;
    let mut rgba = reader.decode().map_err(|_| ())?.into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(());
    }
    // GPUI's render atlas expects BGRA pixels.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
    Ok((image, width, height))
}

pub(in crate::features) fn appearance_font_stack(raw: &str, fallback: &str) -> Vec<String> {
    let mut fonts = Vec::new();
    for family in raw
        .split(',')
        .map(trim_gpui_font_family)
        .filter(|family| !family.is_empty())
    {
        push_unique_font(&mut fonts, family.to_string());
    }
    if fonts.is_empty() {
        fonts.push(fallback.to_string());
    }
    fonts
}

fn appearance_monospace_font_options(cx: &App, system_fonts: &[String]) -> Vec<String> {
    let text_system = cx.text_system();
    let mut terminal_fonts = Vec::new();

    for family in system_fonts {
        let font_id = text_system.resolve_font(&font(SharedString::from(family.clone())));
        let font_size = px(14.);
        let widths = ['i', 'W', '0']
            .into_iter()
            .filter_map(|ch| text_system.advance(font_id, font_size, ch).ok())
            .map(|advance| f32::from(advance.width))
            .collect::<Vec<_>>();
        let monospace = widths.len() == 3
            && widths
                .iter()
                .all(|width| (*width - widths[0]).abs() <= 0.05);
        if monospace {
            push_unique_font(&mut terminal_fonts, family.clone());
        }
    }
    terminal_fonts
}

fn push_unique_font(fonts: &mut Vec<String>, family: String) {
    if !fonts
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&family))
    {
        fonts.push(family);
    }
}

fn parse_minimum_contrast_ratio(raw: &str) -> f32 {
    match raw.trim() {
        "3" => 3.0,
        "4.5" => 4.5,
        "7" => 7.0,
        "21" => 21.0,
        _ => 1.0,
    }
}

fn gpui_platform_font(raw: &str, fallback: &str, monospace: bool) -> ResolvedAppearanceFont {
    gpui_platform_font_for_target(raw, fallback, monospace, cfg!(target_os = "windows"))
}

fn gpui_platform_font_for_target(
    raw: &str,
    fallback: &str,
    monospace: bool,
    is_windows: bool,
) -> ResolvedAppearanceFont {
    let mut families = appearance_font_stack(raw, fallback)
        .into_iter()
        .map(|family| {
            if is_windows && windows_gpui_font_should_fallback(&family, monospace) {
                fallback.to_string()
            } else {
                family
            }
        })
        .collect::<Vec<_>>();
    push_unique_font(&mut families, fallback.to_string());
    let family = families
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string());
    let fallback_families = families.into_iter().skip(1).collect::<Vec<_>>();
    ResolvedAppearanceFont {
        family,
        fallbacks: (!fallback_families.is_empty())
            .then(|| FontFallbacks::from_fonts(fallback_families.clone())),
        fallback_families,
    }
}

#[cfg(test)]
fn gpui_platform_font_family_for_target(
    raw: &str,
    fallback: &str,
    monospace: bool,
    is_windows: bool,
) -> String {
    gpui_platform_font_for_target(raw, fallback, monospace, is_windows).family
}

fn trim_gpui_font_family(value: &str) -> &str {
    value.trim().trim_matches(|ch| ch == '"' || ch == '\'')
}

fn gpui_terminal_font_fallback() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "monospace"
    }
}

pub(in crate::features) fn gpui_code_font_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "JetBrains Mono"
    }
}

fn gpui_ui_font_fallback() -> &'static str {
    if cfg!(target_os = "windows") {
        "Microsoft YaHei UI"
    } else {
        "system-ui"
    }
}

fn windows_gpui_font_should_fallback(family: &str, _monospace: bool) -> bool {
    matches!(family, "monospace" | "system-ui" | "sans-serif")
}

#[cfg(test)]
mod tests {
    use super::{
        appearance_font_stack, gpui_code_font_family, gpui_platform_font_family_for_target,
        gpui_platform_font_for_target,
    };

    #[test]
    fn windows_terminal_font_family_uses_primary_stack_entry() {
        assert_eq!(
            gpui_platform_font_family_for_target("Cascadia Mono, Consolas", "Consolas", true, true,),
            "Cascadia Mono"
        );
    }

    #[test]
    fn windows_ui_font_family_keeps_named_primary_family() {
        assert_eq!(
            gpui_platform_font_family_for_target(
                "JetBrains Mono, Noto Sans SC Variable, 微软雅黑",
                "Microsoft YaHei UI",
                false,
                true,
            ),
            "JetBrains Mono"
        );
    }

    #[test]
    fn windows_terminal_font_family_keeps_installed_named_family() {
        assert_eq!(
            gpui_platform_font_family_for_target(
                "FiraCode Nerd Font Mono, Maple Mono CN",
                "Consolas",
                true,
                true,
            ),
            "FiraCode Nerd Font Mono"
        );
    }

    #[test]
    fn windows_terminal_font_keeps_configured_fallback_stack() {
        let font = gpui_platform_font_for_target(
            "FiraCode Nerd Font Mono, Maple Mono CN",
            "Consolas",
            true,
            true,
        );

        assert_eq!(font.family, "FiraCode Nerd Font Mono");
        assert_eq!(
            font.fallbacks
                .as_ref()
                .map(|fallbacks| fallbacks.fallback_list()),
            Some(["Maple Mono CN".to_string(), "Consolas".to_string()].as_slice())
        );
    }

    #[test]
    fn promoting_configured_fallback_keeps_remaining_fallback_order() {
        let font = gpui_platform_font_for_target(
            "Missing Primary, JetBrains Mono, Maple Mono CN",
            "Consolas",
            true,
            true,
        );
        let promoted = font.with_primary_family("JetBrains Mono");

        assert_eq!(promoted.family, "JetBrains Mono");
        assert_eq!(
            promoted.fallback_families,
            ["Maple Mono CN".to_string(), "Consolas".to_string()]
        );
    }

    #[test]
    fn windows_code_font_uses_installed_platform_default() {
        if cfg!(target_os = "windows") {
            assert_eq!(gpui_code_font_family(), "Consolas");
        }
    }

    #[test]
    fn appearance_font_stack_preserves_fallback_order() {
        assert_eq!(
            appearance_font_stack("JetBrains Mono, Noto Sans SC Variable, Inter", "Inter"),
            vec!["JetBrains Mono", "Noto Sans SC Variable", "Inter"]
        );
    }

    #[test]
    fn appearance_font_stack_normalizes_quotes_whitespace_and_duplicates() {
        assert_eq!(
            appearance_font_stack(
                "  'JetBrains Mono', JetBrains Mono, \"Noto Sans SC\",  ",
                "Inter"
            ),
            vec!["JetBrains Mono", "Noto Sans SC"]
        );
    }

    #[test]
    fn appearance_font_stack_uses_fallback_for_empty_input() {
        assert_eq!(
            appearance_font_stack("  ,  ", "system-ui"),
            vec!["system-ui"]
        );
    }

    #[test]
    fn non_windows_font_family_uses_first_family() {
        assert_eq!(
            gpui_platform_font_family_for_target(
                "JetBrains Mono, monospace",
                "monospace",
                true,
                false,
            ),
            "JetBrains Mono"
        );
    }
}

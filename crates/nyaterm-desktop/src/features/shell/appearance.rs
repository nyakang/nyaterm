use rust_i18n::t;

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, FontFallbacks, FontWeight, PathPromptOptions, RenderImage,
    SharedString, font, px,
};
use nyaterm_core::{
    AppSettingsSummary, ResolvedKeywordHighlightRule, merge_keyword_highlight_rules_for_paint,
};

use crate::features::terminal::{ResolvedAppearanceFont, measure_terminal_font};
use crate::features::{
    FontAvailability, FontAvailabilityReason, FontCatalogEntry, FontCatalogLoadState,
    FontCatalogSnapshot, FontResolutionSource, FontResolutionStatus, NyaTermApp,
    font_names_fingerprint, normalize_font_family, runtime_jobs::await_blocking_job,
};
pub(in crate::features) use crate::theme::{ThemePalette, apply_component_theme};

const TERMINAL_FONT_SIZE_MIN: i16 = 8;
const TERMINAL_FONT_SIZE_MAX: i16 = 72;

impl NyaTermApp {
    pub(in crate::features) fn apply_gpui_settings(
        &mut self,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        let users_changed = self.settings.summary().ui_remote_stats_show_users
            != settings.ui_remote_stats_show_users;
        let terminal_font_changed = {
            let current = self.settings.summary();
            current.terminal_font_family != settings.terminal_font_family
                || current.terminal_font_size != settings.terminal_font_size
                || current.terminal_font_weight != settings.terminal_font_weight
                || current.terminal_font_weight_bold != settings.terminal_font_weight_bold
                || current.bold_default_foreground != settings.bold_default_foreground
        };
        crate::shortcuts::rebuild_keymap(&settings.keybindings, cx);
        self.settings.replace_summary(settings);
        if users_changed {
            self.remote_stats_users_setting_changed(cx);
        }
        crate::window_chrome::request_window_decoration_refresh(cx);
        self.sync_remote_monitoring_inputs(cx);
        self.sync_transfer_internal_editor_font_input(cx);
        self.sync_native_window_material(cx);
        if !self
            .settings
            .summary()
            .interaction_credential_autofill_enabled
        {
            self.cancel_credential_autofill(cx);
        }
        let selected = self
            .settings
            .summary()
            .ui_quick_cmd_selected_category
            .clone();
        self.commands.select_quick_category(selected);
        self.sync_quick_command_selected_category(cx);
        self.sync_component_theme(cx);
        if terminal_font_changed {
            // External loading or settings-save completion may replace font settings;
            // clear the runtime override and paint caches so the next frame cannot use
            // the old font.
            self.invalidate_terminal_cell_metrics(cx);
        }
        self.invalidate_paint_theme_caches();
        // A material or content-opacity change must refresh resolved panel
        // colours even when the wallpaper path is unchanged.
        self.queue_wallpaper_refresh(cx);
        self.refresh_background_surfaces(cx);
    }

    pub(in crate::features) fn sync_component_theme(&self, cx: &mut App) {
        apply_component_theme(
            self.theme_palette(),
            self.gpui_ui_font().font(),
            px(self.settings.summary().ui_font_size.clamp(12, 24) as f32),
            cx,
        );
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
        let fallback = gpui_ui_font_fallback();
        let effective_raw = self
            .appearance_font_resolution(false)
            .map(|status| match status.source {
                FontResolutionSource::UserFallback(index) => appearance_font_stack(raw, "Inter")
                    .get(index..)
                    .filter(|families| !families.is_empty())
                    .map(|families| families.join(", "))
                    .unwrap_or_else(|| fallback.to_string()),
                FontResolutionSource::PlatformDefault => fallback.to_string(),
                FontResolutionSource::Configured
                | FontResolutionSource::EmergencyMetricsFallback => raw.to_string(),
            })
            .unwrap_or_else(|| raw.to_string());
        gpui_platform_font(&effective_raw, fallback, false)
    }

    pub(in crate::features) fn appearance_font_resolution(
        &self,
        terminal: bool,
    ) -> Option<FontResolutionStatus> {
        if terminal && let Some(resolution) = self.terminal.terminal_font_resolution() {
            return Some(resolution.clone());
        }
        let (raw, fallback, platform_default) = if terminal {
            (
                self.settings.summary().terminal_font_family.as_str(),
                "JetBrains Mono",
                gpui_terminal_font_fallback(),
            )
        } else {
            (
                if self.settings.summary().ui_font_family.trim().is_empty() {
                    self.settings.summary().terminal_font_family.as_str()
                } else {
                    self.settings.summary().ui_font_family.as_str()
                },
                "Inter",
                gpui_ui_font_fallback(),
            )
        };
        let families = configured_appearance_font_stack(raw, fallback);
        self.settings
            .resolve_font_stack(&families, terminal, platform_default)
    }

    pub(in crate::features) fn theme_palette(&self) -> ThemePalette {
        self.settings.theme_palette()
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
        self.refresh_background_surfaces(cx);
        let Some(path) = path else {
            return;
        };
        let task_path = path.clone();
        let task = self
            .blocking_jobs
            .submit_task("wallpaper-image-load", move |_| {
                load_wallpaper_image(&task_path)
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.unwrap_or(Err(()));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((image, width, height)) => {
                        if this.shell.cache_wallpaper(path, image, width, height) {
                            this.refresh_background_surfaces(cx);
                        }
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

    pub(in crate::features) fn refresh_background_surfaces(&mut self, cx: &mut Context<Self>) {
        // Cached panel entities retain their resolved colours until their own
        // snapshots are flushed; notifying only the shell leaves them stale.
        self.flush_remote_panel_snapshots(cx);
        self.flush_connection_panel_snapshot(cx);
        self.flush_transfer_panel_snapshot(cx);
        self.flush_ai_panel_snapshot(cx);
        self.refresh_visible_terminal_surfaces(cx);
        cx.notify();
    }

    pub(crate) fn ensure_appearance_font_options(&mut self, cx: &mut Context<Self>) {
        if self.settings.font_catalog_state() == FontCatalogLoadState::Loaded {
            self.check_appearance_font_options(cx);
            return;
        }
        let Some(generation) = self.settings.begin_font_options_load() else {
            return;
        };
        self.start_appearance_font_options_scan(generation, None, cx);
    }

    fn check_appearance_font_options(&mut self, cx: &mut Context<Self>) {
        if !self.settings.begin_font_names_fingerprint_check() {
            return;
        }

        cx.spawn(async move |this, cx| {
            let Ok(text_system) = this.update(cx, |_, cx| cx.text_system().clone()) else {
                let _ = this.update(cx, |this, _| {
                    this.settings.cancel_font_names_fingerprint_check();
                });
                return;
            };
            let system_fonts = cx
                .background_spawn(async move { text_system.all_font_names() })
                .await;
            let fingerprint = font_names_fingerprint(&system_fonts);
            let _ = this.update(cx, move |this, cx| {
                if !this
                    .settings
                    .finish_font_names_fingerprint_check(fingerprint)
                {
                    return;
                }
                let Some(generation) = this.settings.refresh_font_options_load() else {
                    return;
                };
                this.start_appearance_font_options_scan(generation, Some(system_fonts), cx);
            });
        })
        .detach();
    }

    fn start_appearance_font_options_scan(
        &mut self,
        generation: u64,
        initial_system_fonts: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        self.request_settings_panel_refresh(cx);
        cx.spawn(async move |this, cx| {
            let system_fonts = if let Some(system_fonts) = initial_system_fonts {
                system_fonts
            } else {
                let Ok(text_system) = this.update(cx, |_, cx| cx.text_system().clone()) else {
                    let _ = this.update(cx, |this, cx| {
                        if this.settings.fail_font_options_load(generation) {
                            this.request_settings_panel_refresh(cx);
                            cx.notify();
                        }
                    });
                    return;
                };
                cx.background_spawn(async move { text_system.all_font_names() })
                    .await
            };

            const FONT_SCAN_BATCH_SIZE: usize = 8;
            let mut entries = Vec::with_capacity(system_fonts.len());
            let Ok(text_system) = this.update(cx, |_, cx| cx.text_system().clone()) else {
                let _ = this.update(cx, |this, cx| {
                    if this.settings.fail_font_options_load(generation) {
                        this.request_settings_panel_refresh(cx);
                        cx.notify();
                    }
                });
                return;
            };
            for batch in system_fonts.chunks(FONT_SCAN_BATCH_SIZE) {
                let batch = batch.to_vec();
                let batch_entries = {
                    let text_system = Arc::clone(&text_system);
                    cx.background_spawn(
                        async move { probe_appearance_font_batch(&text_system, &batch) },
                    )
                    .await
                };
                entries.extend(batch_entries);
                let generation_is_current = this
                    .update(cx, |this, _| {
                        this.settings.font_catalog_generation() == generation
                    })
                    .unwrap_or(false);
                if !generation_is_current {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(4))
                    .await;
            }
            let snapshot = FontCatalogSnapshot::from_entries(generation, entries);
            let _ = this.update(cx, |this, cx| {
                if !this.settings.finish_font_options_load(generation, snapshot) {
                    return;
                }
                this.sync_component_theme(cx);
                let refresh_terminal_metrics = this
                    .terminal
                    .terminal_font_metrics_need_catalog_refresh(generation);
                // Re-measure after an explicit catalog generation change, or when the active
                // font was previously unresolved or used a fallback. The initial catalog
                // commit keeps a validated metric cache untouched.
                if refresh_terminal_metrics {
                    this.invalidate_terminal_cell_metrics(cx);
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Content opacity applies to surface backgrounds, not their contents.
    pub(in crate::features) fn shell_surface_color(&self, color: u32) -> gpui::Rgba {
        self.shell_background().surface_color(color)
    }

    /// Use the same background for terminals and their lifecycle placeholders.
    /// Opaque windows retain the legacy wallpaper shading.
    pub(in crate::features) fn shell_terminal_surface_color(&self, color: u32) -> gpui::Rgba {
        self.shell_background().terminal_color(color)
    }

    /// Structural containers reveal wallpaper and the native window backdrop.
    pub(in crate::features) fn shell_transparent_color(&self, color: u32) -> gpui::Rgba {
        self.shell_background().transparent_color(color)
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
        self.ensure_keyword_highlight_rules_cache();
    }

    fn ensure_keyword_highlight_rules_cache(&mut self) {
        if self.terminal.cached_keyword_highlight_rules().is_some() {
            return;
        }
        let rules = if !self.settings.keyword_config().enabled {
            Arc::new(Vec::new())
        } else {
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
        self.settings.terminal_palette()
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
        // The side panels (connections, remote monitor, transfers, AI) render
        // from snapshots whose key embeds the panel chrome -- palette included.
        // Their flush boundaries are interactions, input subscriptions, store
        // replies, session switches, and their own clocks; a theme change is
        // not one of them, so the panels kept painting the old palette until
        // the next poll or relayout, surfacing as "the sidebars only follow
        // after Apply". Queue their flushes so the chrome follows the theme
        // immediately; each is a key comparison when nothing else moved.
        self.defer_connection_panel_snapshot_flush(cx);
        self.defer_remote_panel_snapshot_flush(cx);
        self.defer_transfer_panel_snapshot_flush(cx);
        self.defer_ai_panel_snapshot_flush(cx);
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
        // Refresh measured metrics before resizing the terminal. Using a font-size
        // fallback here would briefly desynchronize app state and the surface, especially
        // while scrolling or dragging a selection.
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
        self.sync_component_theme(cx);
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
        let options = if terminal {
            self.settings.terminal_font_options()
        } else {
            self.settings.ui_font_options()
        };
        let next = options
            .iter()
            .find(|candidate| normalize_font_family(candidate) == normalize_font_family(fallback))
            .or_else(|| options.first())
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
        self.sync_component_theme(cx);
        self.defer_transfer_panel_snapshot_flush(cx);
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

    pub(in crate::features) fn toggle_bold_default_foreground(&mut self, cx: &mut Context<Self>) {
        let enabled = self.settings.toggle_bold_default_foreground();
        self.invalidate_paint_theme_caches();
        self.notify_active_terminal_surface(cx);
        self.save_appearance_settings(cx);
        self.shell.set_status(
            if enabled {
                t!("settings.boldDefaultForegroundEnabled")
            } else {
                t!("settings.boldDefaultForegroundDisabled")
            }
            .to_string(),
        );
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
        self.refresh_background_surfaces(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn save_appearance_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(
            crate::features::settings::SettingsPersistenceDomain::Appearance,
            cx,
        ) {
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
    for pixel in rgba.as_chunks_mut::<4>().0 {
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

pub(in crate::features) fn configured_appearance_font_stack(
    raw: &str,
    fallback: &str,
) -> Vec<String> {
    if !raw
        .split(',')
        .map(trim_gpui_font_family)
        .any(|family| !family.is_empty())
    {
        Vec::new()
    } else {
        appearance_font_stack(raw, fallback)
    }
}

fn probe_appearance_font_batch(
    text_system: &gpui::TextSystem,
    system_fonts: &[String],
) -> Vec<FontCatalogEntry> {
    system_fonts
        .iter()
        .filter(|family| !normalize_font_family(family).is_empty())
        .map(|family| {
            let ui = probe_ui_font(text_system, family);
            let descriptor = ResolvedAppearanceFont {
                family: family.clone(),
                fallbacks: None,
                fallback_families: Vec::new(),
            };
            let terminal =
                match measure_terminal_font(text_system, &descriptor, px(14.), FontWeight(400.)) {
                    Ok(measurement) => FontAvailability::Available {
                        resolved_family: measurement.resolved_family.into(),
                    },
                    Err(reason) => FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::from(reason),
                    },
                };
            FontCatalogEntry::new(family.clone(), ui, terminal)
        })
        .collect()
}

fn probe_ui_font(text_system: &gpui::TextSystem, family: &str) -> FontAvailability {
    let font_id = text_system.resolve_font(&font(SharedString::from(family.to_string())));
    let Some(resolved) = text_system.get_font_for_id(font_id) else {
        return FontAvailability::Unavailable {
            reason: FontAvailabilityReason::FontNotResolved,
        };
    };
    let resolved_family = resolved.family.to_string();
    if !resolved_family.eq_ignore_ascii_case(family) {
        return FontAvailability::Unavailable {
            reason: FontAvailabilityReason::ResolvedFamilyMismatch,
        };
    }
    FontAvailability::Available {
        resolved_family: resolved_family.into(),
    }
}

fn push_unique_font(fonts: &mut Vec<String>, family: String) {
    if !fonts
        .iter()
        .any(|existing| normalize_font_family(existing) == normalize_font_family(&family))
    {
        fonts.push(family);
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

pub(in crate::features) fn gpui_terminal_font_fallback() -> &'static str {
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

pub(in crate::features) fn gpui_ui_font_fallback() -> &'static str {
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
        appearance_font_stack, configured_appearance_font_stack, gpui_code_font_family,
        gpui_platform_font_family_for_target, gpui_platform_font_for_target,
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
    fn configured_font_stack_keeps_empty_input_as_platform_default() {
        assert!(configured_appearance_font_stack("  ,  ", "Inter").is_empty());
        assert_eq!(
            configured_appearance_font_stack("JetBrains Mono, Inter", "Inter"),
            vec!["JetBrains Mono", "Inter"]
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

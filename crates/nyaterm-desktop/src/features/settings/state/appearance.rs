//! settings appearance state and transitions.

use super::SettingsFeatureState;
use crate::features::{
    FontCatalogKind, FontCatalogLoadState, FontCatalogSnapshot, FontCatalogState,
    FontResolutionStatus,
};
use nyaterm_core::AppSettingsSummary;
use std::sync::Arc;

pub(super) struct AppearanceSettingsState {
    pub(super) palette: nyaterm_ui::ThemePalette,
    pub(super) theme_options: Arc<[(String, String)]>,
    pub(super) theme_diagnostics: Arc<[String]>,
    pub(super) terminal_palette: nyaterm_ui::ThemePalette,
    pub(super) font_catalog: FontCatalogState,
    pub(super) designer: crate::features::settings::theme_designer::ThemeDesignerState,
}

impl SettingsFeatureState {
    pub(super) fn resolve_terminal_palette(
        summary: &AppSettingsSummary,
    ) -> nyaterm_ui::ThemePalette {
        let id = summary
            .terminal_theme
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.trim().is_empty());
        // A broken explicit terminal theme follows the UI, without rewriting its id.
        let id = id
            .filter(|id| {
                nyaterm_core::themes::is_builtin(id) || summary.custom_themes.resolve(id).is_some()
            })
            .unwrap_or(&summary.theme);
        let mut palette = nyaterm_ui::resolve_theme_palette(id, &summary.custom_themes);
        palette.apply_cursor_contrast_ratio(Self::terminal_contrast_ratio_for(summary));
        palette
    }
    pub(super) fn terminal_contrast_ratio_for(summary: &AppSettingsSummary) -> f32 {
        match summary.minimum_contrast_ratio.trim() {
            "3" => 3.0,
            "4.5" => 4.5,
            "7" => 7.0,
            "21" => 21.0,
            _ => 1.0,
        }
    }

    pub(in crate::features) fn terminal_minimum_contrast_ratio(&self) -> f32 {
        Self::terminal_contrast_ratio_for(&self.summary)
    }

    pub(super) fn refresh_theme_palettes(&mut self) {
        self.appearance.theme_diagnostics = self.summary.custom_themes.diagnostics().into();
        self.appearance.theme_options =
            nyaterm_ui::available_themes(&self.summary.custom_themes).into();
        self.appearance.palette =
            nyaterm_ui::resolve_theme_palette(&self.summary.theme, &self.summary.custom_themes);
        self.appearance.terminal_palette = Self::resolve_terminal_palette(&self.summary);
    }
    pub(in crate::features) fn theme_palette(&self) -> nyaterm_ui::ThemePalette {
        self.appearance.palette
    }
    pub(in crate::features) fn terminal_palette(&self) -> nyaterm_ui::ThemePalette {
        self.appearance.terminal_palette
    }
    pub(in crate::features) fn theme_options(&self) -> Arc<[(String, String)]> {
        self.appearance.theme_options.clone()
    }
    pub(in crate::features) fn theme_diagnostics(&self) -> Arc<[String]> {
        self.appearance.theme_diagnostics.clone()
    }
    pub(in crate::features) fn theme_designer(
        &self,
    ) -> &crate::features::settings::theme_designer::ThemeDesignerState {
        &self.appearance.designer
    }
    pub(in crate::features) fn theme_designer_mut(
        &mut self,
    ) -> &mut crate::features::settings::theme_designer::ThemeDesignerState {
        &mut self.appearance.designer
    }
    pub(in crate::features) fn merge_theme_commit(
        &mut self,
        commit: &nyaterm_core::themes::management::ThemeCommit,
    ) {
        commit.merge_into(&mut self.summary);
        self.refresh_theme_palettes();
    }
    pub(in crate::features) fn set_transfer_internal_editor_font_size(
        &mut self,
        size: u16,
    ) -> bool {
        let next = size.clamp(8, 72);
        let changed = next != self.summary.transfer_internal_editor_font_size;
        self.summary.transfer_internal_editor_font_size = next;
        changed
    }
    pub(in crate::features) fn adjust_transfer_internal_editor_font_size(
        &mut self,
        delta: i16,
    ) -> bool {
        let next = (i32::from(self.summary.transfer_internal_editor_font_size) + i32::from(delta))
            .clamp(8, 72) as u16;
        self.set_transfer_internal_editor_font_size(next)
    }
    pub(in crate::features) fn set_appearance_theme(&mut self, theme: String) {
        self.summary.theme = theme;
        self.refresh_theme_palettes();
    }
    pub(in crate::features) fn set_terminal_font_family(&mut self, family: String) -> bool {
        if self.summary.terminal_font_family == family {
            return false;
        }
        self.summary.terminal_font_family = family;
        true
    }
    pub(in crate::features) fn set_terminal_font_size(&mut self, size: u16) -> bool {
        if self.summary.terminal_font_size == size {
            return false;
        }
        self.summary.terminal_font_size = size;
        true
    }
    pub(in crate::features) fn set_cursor_style(&mut self, style: String) {
        self.summary.cursor_style = style;
    }
    pub(in crate::features) fn toggle_cursor_blink(&mut self) -> bool {
        self.summary.cursor_blink = !self.summary.cursor_blink;
        self.summary.cursor_blink
    }
    pub(in crate::features) fn set_terminal_theme(&mut self, theme: Option<String>) {
        self.summary.terminal_theme = theme;
        self.refresh_theme_palettes();
    }
    pub(in crate::features) fn set_minimum_contrast_ratio(&mut self, ratio: String) -> bool {
        if self.summary.minimum_contrast_ratio == ratio {
            return false;
        }
        self.summary.minimum_contrast_ratio = ratio;
        self.refresh_theme_palettes();
        true
    }
    pub(in crate::features) fn set_ui_font_family(&mut self, family: String) -> bool {
        if self.summary.ui_font_family == family {
            return false;
        }
        self.summary.ui_font_family = family;
        true
    }
    pub(in crate::features) fn set_ui_font_size(&mut self, size: u16) -> bool {
        if self.summary.ui_font_size == size {
            return false;
        }
        self.summary.ui_font_size = size;
        true
    }
    pub(in crate::features) fn set_terminal_font_weight(&mut self, weight: u16) -> bool {
        if self.summary.terminal_font_weight == weight {
            return false;
        }
        self.summary.terminal_font_weight = weight;
        true
    }
    pub(in crate::features) fn set_terminal_font_weight_bold(&mut self, weight: u16) -> bool {
        if self.summary.terminal_font_weight_bold == weight {
            return false;
        }
        self.summary.terminal_font_weight_bold = weight;
        true
    }
    pub(in crate::features) fn toggle_bold_default_foreground(&mut self) -> bool {
        self.summary.bold_default_foreground = !self.summary.bold_default_foreground;
        self.summary.bold_default_foreground
    }
    pub(in crate::features) fn select_background_image(&mut self, path: String) {
        self.summary.background_image_path = Some(path);
        if self.summary.background_image_fit.trim().is_empty() {
            self.summary.background_image_fit = "cover".to_string();
        }
    }
    pub(in crate::features) fn clear_background_image(&mut self) {
        self.summary.background_image_path = None;
    }
    pub(in crate::features) fn set_background_image_fit(&mut self, fit: String) {
        self.summary.background_image_fit = fit;
    }
    pub(in crate::features) fn set_background_image_opacity(&mut self, opacity: u8) -> bool {
        if self.summary.background_image_opacity == opacity {
            return false;
        }
        self.summary.background_image_opacity = opacity;
        true
    }
    pub(in crate::features) fn set_background_content_opacity(&mut self, opacity: u8) -> bool {
        if self.summary.background_content_opacity == opacity {
            return false;
        }
        self.summary.background_content_opacity = opacity;
        true
    }
    pub(in crate::features) fn set_window_material(
        &mut self,
        material: nyaterm_core::models::window_material::WindowMaterial,
    ) {
        self.summary.window_material = material;
    }
    pub(in crate::features) fn set_window_decorations(
        &mut self,
        decorations: nyaterm_core::models::window_decoration::WindowDecoration,
    ) -> bool {
        if self.summary.window_decorations == decorations {
            return false;
        }
        self.summary.window_decorations = decorations;
        true
    }
    pub(in crate::features) fn set_window_background_opacity(&mut self, opacity: u8) -> bool {
        if self.summary.window_background_opacity == opacity {
            return false;
        }
        self.summary.window_background_opacity = opacity;
        true
    }
    pub(in crate::features) fn ui_font_options(&self) -> &[String] {
        self.appearance.font_catalog.snapshot().ui_options()
    }
    pub(in crate::features) fn terminal_font_options(&self) -> &[String] {
        self.appearance.font_catalog.snapshot().terminal_options()
    }
    pub(in crate::features) fn font_catalog_state(&self) -> FontCatalogLoadState {
        self.appearance.font_catalog.state()
    }
    pub(in crate::features) fn font_catalog_generation(&self) -> u64 {
        self.appearance.font_catalog.generation()
    }
    pub(in crate::features) fn font_catalog_snapshot(&self) -> Arc<FontCatalogSnapshot> {
        self.appearance.font_catalog.snapshot_arc()
    }
    pub(in crate::features) fn resolve_font_stack(
        &self,
        families: &[String],
        terminal: bool,
        platform_default: &str,
    ) -> Option<FontResolutionStatus> {
        self.appearance.font_catalog.resolve_stack(
            families,
            if terminal {
                FontCatalogKind::Terminal
            } else {
                FontCatalogKind::Ui
            },
            platform_default,
        )
    }
    pub(in crate::features) fn begin_font_options_load(&mut self) -> Option<u64> {
        self.appearance.font_catalog.begin_load()
    }
    pub(in crate::features) fn refresh_font_options_load(&mut self) -> Option<u64> {
        self.appearance.font_catalog.begin_refresh()
    }
    pub(in crate::features) fn begin_font_names_fingerprint_check(&mut self) -> bool {
        self.appearance
            .font_catalog
            .begin_font_names_fingerprint_check()
    }
    pub(in crate::features) fn finish_font_names_fingerprint_check(
        &mut self,
        fingerprint: u64,
    ) -> bool {
        self.appearance
            .font_catalog
            .finish_font_names_fingerprint_check(fingerprint)
    }
    pub(in crate::features) fn cancel_font_names_fingerprint_check(&mut self) {
        self.appearance
            .font_catalog
            .cancel_font_names_fingerprint_check();
    }
    pub(in crate::features) fn finish_font_options_load(
        &mut self,
        generation: u64,
        snapshot: FontCatalogSnapshot,
    ) -> bool {
        self.appearance.font_catalog.commit(generation, snapshot)
    }
    pub(in crate::features) fn fail_font_options_load(&mut self, generation: u64) -> bool {
        self.appearance.font_catalog.fail(generation)
    }
}

//! System appearance is runtime input; only user preference changes are saved.

use gpui::{Context, Window, WindowAppearance};
use nyaterm_core::themes::appearance::{SystemAppearance, ThemeMode};

use crate::features::NyaTermApp;

pub(in crate::features) fn system_appearance(appearance: WindowAppearance) -> SystemAppearance {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
    }
}

impl NyaTermApp {
    pub(in crate::features) fn observe_system_appearance(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.system_appearance_changed(system_appearance(window.appearance()), cx);
        let subscription = cx.observe_window_appearance(window, |app, window, cx| {
            app.system_appearance_changed(system_appearance(window.appearance()), cx);
        });
        self.settings.set_appearance_subscription(subscription);
    }

    pub(in crate::features) fn system_appearance_changed(
        &mut self,
        appearance: SystemAppearance,
        cx: &mut Context<Self>,
    ) {
        let before = (
            self.settings.theme_palette(),
            self.settings.terminal_palette(),
        );
        if !self.settings.set_system_appearance(appearance) {
            return;
        }
        if before
            != (
                self.settings.theme_palette(),
                self.settings.terminal_palette(),
            )
        {
            self.refresh_applied_theme(cx);
        } else {
            self.request_settings_panel_refresh(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn update_theme_mode(
        &mut self,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_theme_mode(mode);
        self.refresh_applied_theme(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn update_system_theme(
        &mut self,
        appearance: SystemAppearance,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_system_theme(appearance, id.to_owned());
        self.refresh_applied_theme(cx);
        self.save_appearance_settings(cx);
    }

    pub(in crate::features) fn refresh_applied_theme(&mut self, cx: &mut Context<Self>) {
        self.invalidate_paint_theme_caches();
        self.sync_component_theme(cx);
        self.refresh_background_surfaces(cx);
    }
}

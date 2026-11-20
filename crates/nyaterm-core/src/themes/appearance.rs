//! Persisted theme preferences and device-local system appearance resolution.

use serde::{Deserialize, Serialize};

use crate::models::settings::AppSettingsSummary;
use crate::themes::{DEFAULT_THEME_ID, is_builtin};

pub const DEFAULT_LIGHT_THEME_ID: &str = "github-light";

/// Preserve future modes during unrelated settings edits; render them manually.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ThemeMode {
    #[default]
    Manual,
    System,
    Unknown(String),
}

impl ThemeMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Manual => "manual",
            Self::System => "system",
            Self::Unknown(value) => value,
        }
    }
}

impl From<String> for ThemeMode {
    fn from(value: String) -> Self {
        match value.as_str() {
            "manual" => Self::Manual,
            "system" => Self::System,
            _ => Self::Unknown(value),
        }
    }
}

impl From<ThemeMode> for String {
    fn from(value: ThemeMode) -> Self {
        value.as_str().to_owned()
    }
}

/// Runtime input only: never persisted, backed up, or synchronized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    #[default]
    Dark,
}

pub fn default_light_theme() -> String {
    DEFAULT_LIGHT_THEME_ID.into()
}

pub fn default_dark_theme() -> String {
    DEFAULT_THEME_ID.into()
}

/// Missing custom themes fall back to the matching built-in without rewriting
/// their saved ids, so a subsequently restored catalog can resolve them again.
pub fn effective_theme_id(settings: &AppSettingsSummary, system: SystemAppearance) -> &str {
    if settings.theme_mode != ThemeMode::System {
        return &settings.theme;
    }
    let (id, fallback) = match system {
        SystemAppearance::Light => (settings.light_theme.as_str(), DEFAULT_LIGHT_THEME_ID),
        SystemAppearance::Dark => (settings.dark_theme.as_str(), DEFAULT_THEME_ID),
    };
    if is_builtin(id) || settings.custom_themes.resolve(id).is_some() {
        id
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemAppearance, ThemeMode, effective_theme_id};
    use crate::models::settings::AppSettingsSummary;

    #[test]
    fn legacy_settings_keep_the_manual_theme() {
        let mut raw = serde_json::to_value(AppSettingsSummary::default()).unwrap();
        let object = raw.as_object_mut().unwrap();
        for key in ["theme_mode", "light_theme", "dark_theme"] {
            object.remove(key);
        }
        object.insert("theme".into(), "dracula".into());
        let settings: AppSettingsSummary = serde_json::from_value(raw).unwrap();
        assert_eq!(settings.theme_mode, ThemeMode::Manual);
        for appearance in [SystemAppearance::Light, SystemAppearance::Dark] {
            assert_eq!(effective_theme_id(&settings, appearance), "dracula");
        }
    }

    #[test]
    fn automatic_resolution_preserves_manual_and_missing_custom_theme_preferences() {
        let mut settings = AppSettingsSummary {
            theme: "dracula".into(),
            theme_mode: ThemeMode::System,
            light_theme: "missing-custom".into(),
            dark_theme: "nord".into(),
            ..Default::default()
        };
        let before = settings.clone();
        assert_eq!(
            effective_theme_id(&settings, SystemAppearance::Light),
            "github-light"
        );
        assert_eq!(
            effective_theme_id(&settings, SystemAppearance::Dark),
            "nord"
        );
        assert_eq!(settings, before);
        settings.theme_mode = ThemeMode::Manual;
        assert_eq!(
            effective_theme_id(&settings, SystemAppearance::Light),
            "dracula"
        );
    }

    #[test]
    fn future_modes_round_trip_and_use_the_saved_manual_theme() {
        let mode: ThemeMode = serde_json::from_str("\"scheduled\"").unwrap();
        assert_eq!(serde_json::to_value(&mode).unwrap(), "scheduled");
        let settings = AppSettingsSummary {
            theme_mode: mode,
            theme: "nord".into(),
            ..Default::default()
        };
        assert_eq!(
            effective_theme_id(&settings, SystemAppearance::Light),
            "nord"
        );
    }
}

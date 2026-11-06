//! Settings, security, diagnostics and update runtimes.

mod catalog;
mod config_runtime;
mod configuration_replacement;
mod lock_diagnostics_runtime;
mod security_runtime;
mod security_state;
mod settings_runtime;
mod state;
pub(in crate::features) mod theme_designer;

pub(in crate::features) use settings_runtime::SettingsSaveKind;

pub(in crate::features) use security_state::{
    SecurityCatalogState, SecurityFeatureFocus, SecurityFeatureState,
};
pub(in crate::features) use state::{
    KeybindingPresentationState, KeywordHighlightPresentationState, SearchEngineMenu,
    SearchEnginePresentationState, SettingsFeatureFocus, SettingsFeatureInit, SettingsFeatureState,
    SettingsPersistenceDomain, UiLayoutSettingsUpdate,
};

//! Theme-only edits, shared by persistence and the desktop draft adapter.
use crate::models::settings::AppSettingsSummary;
use crate::themes::appearance::{DEFAULT_LIGHT_THEME_ID, ThemeMode};
use crate::themes::{DEFAULT_THEME_ID, ThemeCatalog, ThemeConfig, is_builtin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeApplyTarget {
    Ui,
    Terminal,
    BothFollowUi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeSettings {
    pub catalog: ThemeCatalog,
    pub ui: String,
    pub mode: ThemeMode,
    pub light: String,
    pub dark: String,
    pub terminal: Option<String>,
}

impl From<&AppSettingsSummary> for ThemeSettings {
    fn from(settings: &AppSettingsSummary) -> Self {
        Self {
            catalog: settings.custom_themes.clone(),
            ui: settings.theme.clone(),
            mode: settings.theme_mode.clone(),
            light: settings.light_theme.clone(),
            dark: settings.dark_theme.clone(),
            terminal: settings.terminal_theme.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ThemeMutation {
    Save {
        theme: Box<ThemeConfig>,
        target: Option<ThemeApplyTarget>,
    },
    Apply {
        id: String,
        target: ThemeApplyTarget,
    },
    Delete {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThemeChangedFields {
    pub catalog: bool,
    pub ui: bool,
    pub mode: bool,
    pub light: bool,
    pub dark: bool,
    pub terminal: bool,
}

#[derive(Debug, Clone)]
pub struct ThemeCommit {
    pub settings: ThemeSettings,
    pub fields: ThemeChangedFields,
}

impl ThemeCommit {
    /// Merge only explicitly committed fields; unrelated settings remain a draft.
    pub fn merge_into(&self, settings: &mut AppSettingsSummary) {
        if self.fields.catalog {
            settings.custom_themes = self.settings.catalog.clone();
        }
        if self.fields.ui {
            settings.theme = self.settings.ui.clone();
        }
        if self.fields.mode {
            settings.theme_mode = self.settings.mode.clone();
        }
        if self.fields.light {
            settings.light_theme = self.settings.light.clone();
        }
        if self.fields.dark {
            settings.dark_theme = self.settings.dark.clone();
        }
        if self.fields.terminal {
            settings.terminal_theme = self.settings.terminal.clone();
        }
    }
}

impl ThemeMutation {
    pub fn prepare(&self, base: &ThemeSettings) -> Result<ThemeCommit, String> {
        let mut settings = base.clone();
        let mut fields = ThemeChangedFields::default();
        let apply = match self {
            Self::Save { theme, target } => {
                settings.catalog.upsert(theme.as_ref().clone())?;
                fields.catalog = true;
                target.map(|target| (theme.id.as_str(), target))
            }
            Self::Apply { id, target } => Some((id.as_str(), *target)),
            Self::Delete { id } => {
                if is_builtin(id) || settings.catalog.resolve(id).is_none() {
                    return Err("Theme is not an editable custom theme".into());
                }
                settings.catalog.remove(id);
                fields.catalog = true;
                if settings.ui == *id {
                    settings.ui = DEFAULT_THEME_ID.into();
                    fields.ui = true;
                }
                if settings.light == *id {
                    settings.light = DEFAULT_LIGHT_THEME_ID.into();
                    fields.light = true;
                }
                if settings.dark == *id {
                    settings.dark = DEFAULT_THEME_ID.into();
                    fields.dark = true;
                }
                if settings.terminal.as_deref() == Some(id) {
                    settings.terminal = None;
                    fields.terminal = true;
                }
                None
            }
        };
        if let Some((id, target)) = apply {
            if !is_builtin(id) && settings.catalog.resolve(id).is_none() {
                return Err("Theme is unavailable".into());
            }
            if matches!(
                target,
                ThemeApplyTarget::Ui | ThemeApplyTarget::BothFollowUi
            ) {
                settings.ui = id.into();
                fields.ui = true;
                settings.mode = ThemeMode::Manual;
                fields.mode = true;
            }
            if matches!(
                target,
                ThemeApplyTarget::Terminal | ThemeApplyTarget::BothFollowUi
            ) {
                settings.terminal = (target == ThemeApplyTarget::Terminal).then(|| id.into());
                fields.terminal = true;
            }
        }
        Ok(ThemeCommit { settings, fields })
    }
}

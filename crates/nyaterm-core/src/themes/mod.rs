//! Tauri-compatible theme documents and UI-independent color policies.
pub mod appearance;
pub mod management;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

pub const DEFAULT_THEME_ID: &str = "github-dark";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemeConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub swatch: String,
    pub colors: ThemeColors,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeColors {
    pub bg: String,
    pub bg_panel: String,
    pub bg_terminal: String,
    pub bg_hover: String,
    pub bg_input: String,
    pub bg_section_header: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub text_dimmed: String,
    pub primary: String,
    pub primary_hover: String,
    pub on_primary: String,
    pub focus_ring: String,
    pub danger: String,
    pub danger_hover: String,
    pub success: String,
    pub warning: String,
    pub link: String,
    pub shadow: String,
    pub scroll_thumb: String,
    pub accent: String,
    pub terminal: TerminalColors,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalColors {
    pub background: String,
    pub foreground: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_intense: Option<String>,
    pub cursor: String,
    pub selection_background: String,
    pub line_highlight: String,
    pub find_match_background: String,
    pub find_match_border: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Raw storage is intentional: unsupported records and unknown fields survive settings edits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ThemeCatalog(pub Value);
impl Default for ThemeCatalog {
    fn default() -> Self {
        Self(Value::Array(Vec::new()))
    }
}
impl ThemeCatalog {
    /// Diagnostics are computed alongside theme resolution, outside UI rendering.
    pub fn diagnostics(&self) -> Vec<String> {
        if !self.0.is_array() {
            return vec!["appearance.custom_themes: expected an array".into()];
        }
        let mut seen = HashSet::new();
        self.records()
            .iter()
            .enumerate()
            .filter_map(|(index, raw)| {
                let title = raw
                    .get("name")
                    .or_else(|| raw.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let error = match serde_json::from_value::<ThemeConfig>(raw.clone()) {
                    Ok(theme) => {
                        let mut errors = theme.validate();
                        if is_builtin(&theme.id) {
                            errors.push("id: reserved for a built-in theme".into());
                        } else if errors.is_empty() && !seen.insert(theme.id) {
                            errors.push("id: duplicate".into());
                        }
                        if errors.is_empty() {
                            return None;
                        }
                        errors.join(", ")
                    }
                    Err(error) => error.to_string(),
                };
                Some(format!("[{}] {title}: {error}", index + 1))
            })
            .collect()
    }
    pub fn records(&self) -> &[Value] {
        self.0.as_array().map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn resolve(&self, id: &str) -> Option<ThemeConfig> {
        if is_builtin(id) {
            return None;
        }
        self.records().iter().find_map(|raw| {
            let theme: ThemeConfig = serde_json::from_value(raw.clone()).ok()?;
            (theme.id == id && theme.validate().is_empty()).then_some(theme)
        })
    }
    pub fn available(&self) -> Vec<ThemeConfig> {
        let mut seen: HashSet<String> = builtins().iter().map(|t| t.id.clone()).collect();
        self.records()
            .iter()
            .filter_map(|raw| {
                let theme: ThemeConfig = serde_json::from_value(raw.clone()).ok()?;
                (theme.validate().is_empty() && seen.insert(theme.id.clone())).then_some(theme)
            })
            .collect()
    }
    pub fn upsert(&mut self, theme: ThemeConfig) -> Result<(), String> {
        let errors = theme.validate();
        if !errors.is_empty() {
            return Err(errors.join(", "));
        }
        if is_builtin(&theme.id) {
            return Err("Built-in themes are read-only".into());
        }
        let records = self
            .0
            .as_array_mut()
            .ok_or("Unsupported custom_themes document")?;
        let index = records.iter().position(|raw| {
            serde_json::from_value::<ThemeConfig>(raw.clone())
                .is_ok_and(|existing| existing.id == theme.id && existing.validate().is_empty())
        });
        if index.is_none()
            && records
                .iter()
                .any(|raw| raw.get("id").and_then(Value::as_str) == Some(&theme.id))
        {
            return Err("id: conflicts with an unsupported theme record".into());
        }
        let raw = serde_json::to_value(theme).map_err(|e| e.to_string())?;
        if let Some(index) = index {
            records[index] = raw;
        } else {
            records.push(raw);
        }
        Ok(())
    }
    pub fn remove(&mut self, id: &str) {
        if let Some(records) = self.0.as_array_mut() {
            records.retain(|raw| {
                !serde_json::from_value::<ThemeConfig>(raw.clone())
                    .is_ok_and(|theme| theme.id == id && theme.validate().is_empty())
            });
        }
    }
    pub fn contains_id(&self, id: &str) -> bool {
        self.records()
            .iter()
            .any(|r| r.get("id").and_then(Value::as_str) == Some(id))
    }
}
pub fn builtins() -> &'static [ThemeConfig] {
    static THEMES: OnceLock<Vec<ThemeConfig>> = OnceLock::new();
    THEMES.get_or_init(|| {
        serde_json::from_str(include_str!("builtins.json")).expect("bundled themes are valid")
    })
}
pub fn is_builtin(id: &str) -> bool {
    id == "catppuccin" || builtins().iter().any(|t| t.id == id)
}
pub fn resolve_theme(id: &str, catalog: &ThemeCatalog) -> ThemeConfig {
    let id = if id == "catppuccin" {
        "catppuccin-mocha"
    } else {
        id
    };
    catalog
        .resolve(id)
        .or_else(|| builtins().iter().find(|t| t.id == id).cloned())
        .unwrap_or_else(|| builtins()[0].clone())
}
/// RGBA bytes, kept independent of any UI framework.
pub fn parse_color(value: &str) -> Option<[u8; 4]> {
    value
        .trim()
        .parse::<csscolorparser::Color>()
        .ok()
        .map(|c| c.to_rgba8())
}
pub fn is_hex_color(value: &str) -> bool {
    let value = value.trim();
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}
pub fn contrast_ratio(left: &str, right: &str) -> Option<f32> {
    fn luminance(c: [u8; 4]) -> f32 {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
    }
    let a = luminance(parse_color(left)?);
    let b = luminance(parse_color(right)?);
    Some((a.max(b) + 0.05) / (a.min(b) + 0.05))
}
impl ThemeConfig {
    pub fn copy_as_custom(&self) -> Self {
        let mut theme = self.clone();
        theme.id = format!("custom-{}", uuid::Uuid::new_v4());
        theme.name = format!("{} Custom", self.name);
        theme.label = theme.label.chars().take(14).collect();
        theme
    }
    pub fn normalize_import(&mut self, catalog: &ThemeCatalog) {
        if self.id.trim().is_empty() || is_builtin(&self.id) || catalog.contains_id(&self.id) {
            self.id = format!("custom-{}", uuid::Uuid::new_v4());
        }
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = "Imported Theme".into();
        }
        self.label = if self.label.trim().is_empty() {
            self.name.clone()
        } else {
            self.label.trim().into()
        }
        .chars()
        .take(14)
        .collect();
    }
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for (field, value) in [
            ("id", &self.id),
            ("name", &self.name),
            ("label", &self.label),
        ] {
            if value.trim().is_empty() {
                errors.push(field.into());
            }
        }
        if self.label.chars().count() > 14 {
            errors.push("label".into());
        }
        if !is_hex_color(&self.swatch) {
            errors.push("swatch".into());
        }
        for field in COLOR_FIELDS {
            let value = self.color(*field);
            if if field.alpha() {
                parse_color(value).is_none()
            } else {
                !is_hex_color(value)
            } {
                errors.push(field.path().into());
            }
        }
        errors
    }
    pub fn color(&self, field: ThemeColorField) -> &str {
        match field {
            ThemeColorField::Bg => &self.colors.bg,
            ThemeColorField::BgPanel => &self.colors.bg_panel,
            ThemeColorField::BgTerminal => &self.colors.bg_terminal,
            ThemeColorField::BgHover => &self.colors.bg_hover,
            ThemeColorField::BgInput => &self.colors.bg_input,
            ThemeColorField::BgSectionHeader => &self.colors.bg_section_header,
            ThemeColorField::Border => &self.colors.border,
            ThemeColorField::Text => &self.colors.text,
            ThemeColorField::TextMuted => &self.colors.text_muted,
            ThemeColorField::TextDimmed => &self.colors.text_dimmed,
            ThemeColorField::Primary => &self.colors.primary,
            ThemeColorField::PrimaryHover => &self.colors.primary_hover,
            ThemeColorField::OnPrimary => &self.colors.on_primary,
            ThemeColorField::FocusRing => &self.colors.focus_ring,
            ThemeColorField::Danger => &self.colors.danger,
            ThemeColorField::DangerHover => &self.colors.danger_hover,
            ThemeColorField::Success => &self.colors.success,
            ThemeColorField::Warning => &self.colors.warning,
            ThemeColorField::Link => &self.colors.link,
            ThemeColorField::Shadow => &self.colors.shadow,
            ThemeColorField::ScrollThumb => &self.colors.scroll_thumb,
            ThemeColorField::Accent => &self.colors.accent,
            ThemeColorField::TerminalBackground => &self.colors.terminal.background,
            ThemeColorField::TerminalForeground => &self.colors.terminal.foreground,
            ThemeColorField::TerminalForegroundIntense => self
                .colors
                .terminal
                .foreground_intense
                .as_deref()
                .unwrap_or(&self.colors.terminal.foreground),
            ThemeColorField::TerminalCursor => &self.colors.terminal.cursor,
            ThemeColorField::TerminalSelectionBackground => {
                &self.colors.terminal.selection_background
            }
            ThemeColorField::TerminalLineHighlight => &self.colors.terminal.line_highlight,
            ThemeColorField::TerminalFindMatchBackground => {
                &self.colors.terminal.find_match_background
            }
            ThemeColorField::TerminalFindMatchBorder => &self.colors.terminal.find_match_border,
            ThemeColorField::TerminalBlack => &self.colors.terminal.black,
            ThemeColorField::TerminalRed => &self.colors.terminal.red,
            ThemeColorField::TerminalGreen => &self.colors.terminal.green,
            ThemeColorField::TerminalYellow => &self.colors.terminal.yellow,
            ThemeColorField::TerminalBlue => &self.colors.terminal.blue,
            ThemeColorField::TerminalMagenta => &self.colors.terminal.magenta,
            ThemeColorField::TerminalCyan => &self.colors.terminal.cyan,
            ThemeColorField::TerminalWhite => &self.colors.terminal.white,
            ThemeColorField::TerminalBrightBlack => &self.colors.terminal.bright_black,
            ThemeColorField::TerminalBrightRed => &self.colors.terminal.bright_red,
            ThemeColorField::TerminalBrightGreen => &self.colors.terminal.bright_green,
            ThemeColorField::TerminalBrightYellow => &self.colors.terminal.bright_yellow,
            ThemeColorField::TerminalBrightBlue => &self.colors.terminal.bright_blue,
            ThemeColorField::TerminalBrightMagenta => &self.colors.terminal.bright_magenta,
            ThemeColorField::TerminalBrightCyan => &self.colors.terminal.bright_cyan,
            ThemeColorField::TerminalBrightWhite => &self.colors.terminal.bright_white,
        }
    }
    pub fn set_color(&mut self, field: ThemeColorField, value: String) {
        match field {
            ThemeColorField::Bg => self.colors.bg = value,
            ThemeColorField::BgPanel => self.colors.bg_panel = value,
            ThemeColorField::BgTerminal => self.colors.bg_terminal = value,
            ThemeColorField::BgHover => self.colors.bg_hover = value,
            ThemeColorField::BgInput => self.colors.bg_input = value,
            ThemeColorField::BgSectionHeader => self.colors.bg_section_header = value,
            ThemeColorField::Border => self.colors.border = value,
            ThemeColorField::Text => self.colors.text = value,
            ThemeColorField::TextMuted => self.colors.text_muted = value,
            ThemeColorField::TextDimmed => self.colors.text_dimmed = value,
            ThemeColorField::Primary => self.colors.primary = value,
            ThemeColorField::PrimaryHover => self.colors.primary_hover = value,
            ThemeColorField::OnPrimary => self.colors.on_primary = value,
            ThemeColorField::FocusRing => self.colors.focus_ring = value,
            ThemeColorField::Danger => self.colors.danger = value,
            ThemeColorField::DangerHover => self.colors.danger_hover = value,
            ThemeColorField::Success => self.colors.success = value,
            ThemeColorField::Warning => self.colors.warning = value,
            ThemeColorField::Link => self.colors.link = value,
            ThemeColorField::Shadow => self.colors.shadow = value,
            ThemeColorField::ScrollThumb => self.colors.scroll_thumb = value,
            ThemeColorField::Accent => self.colors.accent = value,
            ThemeColorField::TerminalBackground => self.colors.terminal.background = value,
            ThemeColorField::TerminalForeground => self.colors.terminal.foreground = value,
            ThemeColorField::TerminalForegroundIntense => {
                self.colors.terminal.foreground_intense = Some(value)
            }
            ThemeColorField::TerminalCursor => self.colors.terminal.cursor = value,
            ThemeColorField::TerminalSelectionBackground => {
                self.colors.terminal.selection_background = value
            }
            ThemeColorField::TerminalLineHighlight => self.colors.terminal.line_highlight = value,
            ThemeColorField::TerminalFindMatchBackground => {
                self.colors.terminal.find_match_background = value
            }
            ThemeColorField::TerminalFindMatchBorder => {
                self.colors.terminal.find_match_border = value
            }
            ThemeColorField::TerminalBlack => self.colors.terminal.black = value,
            ThemeColorField::TerminalRed => self.colors.terminal.red = value,
            ThemeColorField::TerminalGreen => self.colors.terminal.green = value,
            ThemeColorField::TerminalYellow => self.colors.terminal.yellow = value,
            ThemeColorField::TerminalBlue => self.colors.terminal.blue = value,
            ThemeColorField::TerminalMagenta => self.colors.terminal.magenta = value,
            ThemeColorField::TerminalCyan => self.colors.terminal.cyan = value,
            ThemeColorField::TerminalWhite => self.colors.terminal.white = value,
            ThemeColorField::TerminalBrightBlack => self.colors.terminal.bright_black = value,
            ThemeColorField::TerminalBrightRed => self.colors.terminal.bright_red = value,
            ThemeColorField::TerminalBrightGreen => self.colors.terminal.bright_green = value,
            ThemeColorField::TerminalBrightYellow => self.colors.terminal.bright_yellow = value,
            ThemeColorField::TerminalBrightBlue => self.colors.terminal.bright_blue = value,
            ThemeColorField::TerminalBrightMagenta => self.colors.terminal.bright_magenta = value,
            ThemeColorField::TerminalBrightCyan => self.colors.terminal.bright_cyan = value,
            ThemeColorField::TerminalBrightWhite => self.colors.terminal.bright_white = value,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeColorField {
    Bg,
    BgPanel,
    BgTerminal,
    BgHover,
    BgInput,
    BgSectionHeader,
    Border,
    Text,
    TextMuted,
    TextDimmed,
    Primary,
    PrimaryHover,
    OnPrimary,
    FocusRing,
    Danger,
    DangerHover,
    Success,
    Warning,
    Link,
    Shadow,
    ScrollThumb,
    Accent,
    TerminalBackground,
    TerminalForeground,
    TerminalForegroundIntense,
    TerminalCursor,
    TerminalSelectionBackground,
    TerminalLineHighlight,
    TerminalFindMatchBackground,
    TerminalFindMatchBorder,
    TerminalBlack,
    TerminalRed,
    TerminalGreen,
    TerminalYellow,
    TerminalBlue,
    TerminalMagenta,
    TerminalCyan,
    TerminalWhite,
    TerminalBrightBlack,
    TerminalBrightRed,
    TerminalBrightGreen,
    TerminalBrightYellow,
    TerminalBrightBlue,
    TerminalBrightMagenta,
    TerminalBrightCyan,
    TerminalBrightWhite,
}
pub const COLOR_FIELDS: &[ThemeColorField] = &[
    ThemeColorField::Bg,
    ThemeColorField::BgPanel,
    ThemeColorField::BgTerminal,
    ThemeColorField::BgHover,
    ThemeColorField::BgInput,
    ThemeColorField::BgSectionHeader,
    ThemeColorField::Border,
    ThemeColorField::Text,
    ThemeColorField::TextMuted,
    ThemeColorField::TextDimmed,
    ThemeColorField::Primary,
    ThemeColorField::PrimaryHover,
    ThemeColorField::OnPrimary,
    ThemeColorField::FocusRing,
    ThemeColorField::Danger,
    ThemeColorField::DangerHover,
    ThemeColorField::Success,
    ThemeColorField::Warning,
    ThemeColorField::Link,
    ThemeColorField::Shadow,
    ThemeColorField::ScrollThumb,
    ThemeColorField::Accent,
    ThemeColorField::TerminalBackground,
    ThemeColorField::TerminalForeground,
    ThemeColorField::TerminalForegroundIntense,
    ThemeColorField::TerminalCursor,
    ThemeColorField::TerminalSelectionBackground,
    ThemeColorField::TerminalLineHighlight,
    ThemeColorField::TerminalFindMatchBackground,
    ThemeColorField::TerminalFindMatchBorder,
    ThemeColorField::TerminalBlack,
    ThemeColorField::TerminalRed,
    ThemeColorField::TerminalGreen,
    ThemeColorField::TerminalYellow,
    ThemeColorField::TerminalBlue,
    ThemeColorField::TerminalMagenta,
    ThemeColorField::TerminalCyan,
    ThemeColorField::TerminalWhite,
    ThemeColorField::TerminalBrightBlack,
    ThemeColorField::TerminalBrightRed,
    ThemeColorField::TerminalBrightGreen,
    ThemeColorField::TerminalBrightYellow,
    ThemeColorField::TerminalBrightBlue,
    ThemeColorField::TerminalBrightMagenta,
    ThemeColorField::TerminalBrightCyan,
    ThemeColorField::TerminalBrightWhite,
];
impl ThemeColorField {
    pub fn path(self) -> &'static str {
        match self {
            Self::Bg => "bg",
            Self::BgPanel => "bgPanel",
            Self::BgTerminal => "bgTerminal",
            Self::BgHover => "bgHover",
            Self::BgInput => "bgInput",
            Self::BgSectionHeader => "bgSectionHeader",
            Self::Border => "border",
            Self::Text => "text",
            Self::TextMuted => "textMuted",
            Self::TextDimmed => "textDimmed",
            Self::Primary => "primary",
            Self::PrimaryHover => "primaryHover",
            Self::OnPrimary => "onPrimary",
            Self::FocusRing => "focusRing",
            Self::Danger => "danger",
            Self::DangerHover => "dangerHover",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Link => "link",
            Self::Shadow => "shadow",
            Self::ScrollThumb => "scrollThumb",
            Self::Accent => "accent",
            Self::TerminalBackground => "terminal.background",
            Self::TerminalForeground => "terminal.foreground",
            Self::TerminalForegroundIntense => "terminal.foregroundIntense",
            Self::TerminalCursor => "terminal.cursor",
            Self::TerminalSelectionBackground => "terminal.selectionBackground",
            Self::TerminalLineHighlight => "terminal.lineHighlight",
            Self::TerminalFindMatchBackground => "terminal.findMatchBackground",
            Self::TerminalFindMatchBorder => "terminal.findMatchBorder",
            Self::TerminalBlack => "terminal.black",
            Self::TerminalRed => "terminal.red",
            Self::TerminalGreen => "terminal.green",
            Self::TerminalYellow => "terminal.yellow",
            Self::TerminalBlue => "terminal.blue",
            Self::TerminalMagenta => "terminal.magenta",
            Self::TerminalCyan => "terminal.cyan",
            Self::TerminalWhite => "terminal.white",
            Self::TerminalBrightBlack => "terminal.brightBlack",
            Self::TerminalBrightRed => "terminal.brightRed",
            Self::TerminalBrightGreen => "terminal.brightGreen",
            Self::TerminalBrightYellow => "terminal.brightYellow",
            Self::TerminalBrightBlue => "terminal.brightBlue",
            Self::TerminalBrightMagenta => "terminal.brightMagenta",
            Self::TerminalBrightCyan => "terminal.brightCyan",
            Self::TerminalBrightWhite => "terminal.brightWhite",
        }
    }
    pub fn terminal(self) -> bool {
        self.path().starts_with("terminal.")
    }
    pub fn alpha(self) -> bool {
        matches!(self, Self::Shadow | Self::TerminalFindMatchBackground)
    }
}

#[cfg(test)]
mod tests;

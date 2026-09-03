use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct InteractionSettings {
    pub copy_on_select: bool,
    pub allow_osc52_clipboard_write: bool,
    pub terminal_right_click_action: String,
    pub terminal_zoom_enabled: bool,
    pub command_suggestions_enabled: bool,
    pub command_suggestion_min_chars: usize,
    pub command_suggestion_max_chars: usize,
    pub duplicate_session_command_delay_ms: u64,
    pub word_separators: String,
    pub alt_as_meta: bool,
    pub ime_compatibility: bool,
    pub default_encoding: String,
    pub tab_double_click_action: String,
    pub tab_middle_click_action: String,
    pub tab_right_click_action: String,
}

#[derive(Deserialize)]
struct InteractionSettingsWire {
    copy_on_select: Option<bool>,
    allow_osc52_clipboard_write: Option<bool>,
    terminal_right_click_action: Option<String>,
    right_click_paste: Option<bool>,
    terminal_zoom_enabled: Option<bool>,
    command_suggestions_enabled: Option<bool>,
    command_suggestion_min_chars: Option<usize>,
    command_suggestion_max_chars: Option<usize>,
    duplicate_session_command_delay_ms: Option<u64>,
    word_separators: Option<String>,
    alt_as_meta: Option<bool>,
    ime_compatibility: Option<bool>,
    mac_ime_compatibility: Option<bool>,
    default_encoding: Option<String>,
    tab_double_click_action: Option<String>,
    tab_middle_click_action: Option<String>,
    tab_right_click_action: Option<String>,
}

fn default_command_suggestion_min_chars() -> usize {
    2
}

fn default_command_suggestion_max_chars() -> usize {
    64
}

fn default_duplicate_session_command_delay_ms() -> u64 {
    1000
}

fn default_word_separators() -> String {
    " ()[]{}\"':=,;|&<>".to_string()
}
fn default_encoding() -> String {
    "UTF-8".to_string()
}

fn default_terminal_right_click_action() -> String {
    "menu".to_string()
}

fn normalize_terminal_right_click_action(
    action: Option<String>,
    legacy_right_click_paste: Option<bool>,
) -> String {
    if let Some(action) = action {
        return match action.as_str() {
            "none" | "menu" | "paste" => action,
            _ => default_terminal_right_click_action(),
        };
    }

    match legacy_right_click_paste {
        Some(true) => "paste".to_string(),
        Some(false) => "menu".to_string(),
        None => "paste".to_string(),
    }
}

fn default_tab_double_click_action() -> String {
    "disconnect_session".to_string()
}

fn default_tab_middle_click_action() -> String {
    "rename_tab".to_string()
}

fn default_tab_right_click_action() -> String {
    "none".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for InteractionSettings {
    fn default() -> Self {
        Self {
            copy_on_select: false,
            allow_osc52_clipboard_write: false,
            terminal_right_click_action: default_terminal_right_click_action(),
            terminal_zoom_enabled: true,
            command_suggestions_enabled: true,
            command_suggestion_min_chars: default_command_suggestion_min_chars(),
            command_suggestion_max_chars: default_command_suggestion_max_chars(),
            duplicate_session_command_delay_ms: default_duplicate_session_command_delay_ms(),
            word_separators: default_word_separators(),
            alt_as_meta: false,
            ime_compatibility: false,
            default_encoding: default_encoding(),
            tab_double_click_action: default_tab_double_click_action(),
            tab_middle_click_action: default_tab_middle_click_action(),
            tab_right_click_action: default_tab_right_click_action(),
        }
    }
}

impl<'de> Deserialize<'de> for InteractionSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = InteractionSettingsWire::deserialize(deserializer)?;
        let defaults = Self::default();

        Ok(Self {
            copy_on_select: wire.copy_on_select.unwrap_or_else(default_true),
            allow_osc52_clipboard_write: wire
                .allow_osc52_clipboard_write
                .unwrap_or(defaults.allow_osc52_clipboard_write),
            terminal_right_click_action: normalize_terminal_right_click_action(
                wire.terminal_right_click_action,
                wire.right_click_paste,
            ),
            terminal_zoom_enabled: wire.terminal_zoom_enabled.unwrap_or_else(default_true),
            command_suggestions_enabled: wire
                .command_suggestions_enabled
                .unwrap_or_else(default_true),
            command_suggestion_min_chars: wire
                .command_suggestion_min_chars
                .unwrap_or(defaults.command_suggestion_min_chars),
            command_suggestion_max_chars: wire
                .command_suggestion_max_chars
                .unwrap_or(defaults.command_suggestion_max_chars),
            duplicate_session_command_delay_ms: wire
                .duplicate_session_command_delay_ms
                .unwrap_or(defaults.duplicate_session_command_delay_ms),
            word_separators: wire.word_separators.unwrap_or(defaults.word_separators),
            alt_as_meta: wire.alt_as_meta.unwrap_or(defaults.alt_as_meta),
            ime_compatibility: wire
                .ime_compatibility
                .or(wire.mac_ime_compatibility)
                .unwrap_or(defaults.ime_compatibility),
            default_encoding: wire.default_encoding.unwrap_or(defaults.default_encoding),
            tab_double_click_action: wire
                .tab_double_click_action
                .unwrap_or(defaults.tab_double_click_action),
            tab_middle_click_action: wire
                .tab_middle_click_action
                .unwrap_or(defaults.tab_middle_click_action),
            tab_right_click_action: wire
                .tab_right_click_action
                .unwrap_or(defaults.tab_right_click_action),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_defaults_include_tab_mouse_actions() {
        let settings = InteractionSettings::default();

        assert_eq!(settings.tab_double_click_action, "disconnect_session");
        assert_eq!(settings.tab_middle_click_action, "rename_tab");
        assert_eq!(settings.tab_right_click_action, "none");
        assert_eq!(settings.terminal_right_click_action, "menu");
        assert!(!settings.allow_osc52_clipboard_write);
        assert!(!settings.alt_as_meta);
        assert!(!settings.ime_compatibility);
        assert!(settings.terminal_zoom_enabled);
    }

    #[test]
    fn legacy_interaction_settings_deserialize_with_tab_mouse_defaults() {
        let settings: InteractionSettings = serde_json::from_value(serde_json::json!({
            "copy_on_select": false,
            "right_click_paste": false,
            "command_suggestions_enabled": true,
            "command_suggestion_min_chars": 2,
            "command_suggestion_max_chars": 64,
            "word_separators": " ()[]{}\"':=,;|&<>",
            "default_encoding": "UTF-8"
        }))
        .unwrap();

        assert_eq!(settings.tab_double_click_action, "disconnect_session");
        assert_eq!(settings.tab_middle_click_action, "rename_tab");
        assert_eq!(settings.tab_right_click_action, "none");
        assert_eq!(settings.duplicate_session_command_delay_ms, 1000);
        assert_eq!(settings.terminal_right_click_action, "menu");
        assert!(!settings.allow_osc52_clipboard_write);
        assert!(!settings.alt_as_meta);
        assert!(!settings.ime_compatibility);
        assert!(settings.terminal_zoom_enabled);
    }

    #[test]
    fn missing_interaction_fields_keep_existing_serde_defaults() {
        let settings: InteractionSettings = serde_json::from_value(serde_json::json!({})).unwrap();

        assert!(settings.copy_on_select);
        assert_eq!(settings.terminal_right_click_action, "paste");
        assert!(settings.terminal_zoom_enabled);
        assert!(settings.command_suggestions_enabled);
        assert!(!settings.ime_compatibility);
    }

    #[test]
    fn terminal_right_click_action_accepts_all_supported_values() {
        for action in ["none", "menu", "paste"] {
            let settings: InteractionSettings = serde_json::from_value(serde_json::json!({
                "terminal_right_click_action": action
            }))
            .unwrap();

            assert_eq!(settings.terminal_right_click_action, action);
        }
    }

    #[test]
    fn legacy_right_click_paste_migrates_to_terminal_right_click_action() {
        let menu: InteractionSettings = serde_json::from_value(serde_json::json!({
            "right_click_paste": false
        }))
        .unwrap();
        let paste: InteractionSettings = serde_json::from_value(serde_json::json!({
            "right_click_paste": true
        }))
        .unwrap();

        assert_eq!(menu.terminal_right_click_action, "menu");
        assert_eq!(paste.terminal_right_click_action, "paste");
    }

    #[test]
    fn terminal_right_click_action_takes_precedence_and_normalizes_invalid_values() {
        let explicit: InteractionSettings = serde_json::from_value(serde_json::json!({
            "terminal_right_click_action": "none",
            "right_click_paste": true
        }))
        .unwrap();
        let invalid: InteractionSettings = serde_json::from_value(serde_json::json!({
            "terminal_right_click_action": "invalid",
            "right_click_paste": true
        }))
        .unwrap();

        assert_eq!(explicit.terminal_right_click_action, "none");
        assert_eq!(invalid.terminal_right_click_action, "menu");
    }

    #[test]
    fn terminal_right_click_action_serialization_omits_legacy_field() {
        let settings = InteractionSettings::default();
        let value = serde_json::to_value(settings).unwrap();

        assert_eq!(value["terminal_right_click_action"], "menu");
        assert!(value.get("right_click_paste").is_none());
    }

    #[test]
    fn legacy_mac_ime_compatibility_migrates_to_ime_compatibility() {
        let settings: InteractionSettings = serde_json::from_value(serde_json::json!({
            "mac_ime_compatibility": true
        }))
        .unwrap();

        assert!(settings.ime_compatibility);
    }

    #[test]
    fn ime_compatibility_takes_precedence_over_legacy_mac_field() {
        let settings: InteractionSettings = serde_json::from_value(serde_json::json!({
            "ime_compatibility": false,
            "mac_ime_compatibility": true
        }))
        .unwrap();

        assert!(!settings.ime_compatibility);
    }
}

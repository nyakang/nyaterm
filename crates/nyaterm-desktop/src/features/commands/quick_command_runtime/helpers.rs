use std::time::{SystemTime, UNIX_EPOCH};

use nyaterm_core::{AiCommandCard, QuickCommand, QuickCommandCategory};

use crate::models::{QuickCommandSortMode, QuickCommandViewMode};

pub(super) fn validated_quick_command_text(command: &str) -> Option<String> {
    if command.trim().is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

pub(super) fn parse_segmented_send_interval(input: &str) -> Option<u64> {
    nyaterm_core::send_command::parse_interval_ms(input)
}

pub(super) fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

pub(in crate::features) fn quick_command_view_mode_from_setting(
    value: &str,
) -> QuickCommandViewMode {
    match value.trim() {
        "list" => QuickCommandViewMode::List,
        "compact" => QuickCommandViewMode::Compact,
        "commandBar" | "command_bar" => QuickCommandViewMode::CommandBar,
        "sidebar" => QuickCommandViewMode::Sidebar,
        _ => QuickCommandViewMode::Tile,
    }
}

pub(in crate::features) fn quick_command_sort_mode_from_setting(
    value: &str,
) -> QuickCommandSortMode {
    match value.trim() {
        "name" => QuickCommandSortMode::Name,
        "useCount" => QuickCommandSortMode::Usage,
        "custom" => QuickCommandSortMode::Custom,
        _ => QuickCommandSortMode::Created,
    }
}

pub(in crate::features) fn quick_command_view_mode_setting(
    mode: QuickCommandViewMode,
) -> &'static str {
    match mode {
        QuickCommandViewMode::List => "list",
        QuickCommandViewMode::Compact => "compact",
        QuickCommandViewMode::Tile => "tile",
        QuickCommandViewMode::CommandBar => "commandBar",
        QuickCommandViewMode::Sidebar => "sidebar",
    }
}

pub(super) fn quick_command_sort_mode_setting(mode: QuickCommandSortMode) -> &'static str {
    match mode {
        QuickCommandSortMode::Created => "created",
        QuickCommandSortMode::Name => "name",
        QuickCommandSortMode::Usage => "useCount",
        QuickCommandSortMode::Custom => "custom",
    }
}

pub(in crate::features) fn quick_command_category_label(
    categories: &[QuickCommandCategory],
    command: &QuickCommand,
) -> String {
    command
        .category_id
        .as_deref()
        .and_then(|id| categories.iter().find(|category| category.id == id))
        .map(|category| category.name.clone())
        .unwrap_or_default()
}

pub(super) fn ai_command_card_category_name(card: &AiCommandCard) -> String {
    card.category
        .as_deref()
        .map(str::trim)
        .filter(|category| !category.is_empty())
        .unwrap_or("AI Generated")
        .to_string()
}

pub(super) fn unique_quick_command_category_id(
    categories: &[QuickCommandCategory],
    category_name: &str,
) -> String {
    let base = format!("ai-{}", quick_command_slug(category_name));
    if !categories.iter().any(|category| category.id == base) {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !categories.iter().any(|category| category.id == candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search always returns")
}

pub(super) fn quick_command_slug(input: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if matches!(ch, '-' | '_' | ' ' | '\t' | '\n' | '\r') && !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "commands".to_string()
    } else {
        slug.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::validated_quick_command_text;

    #[test]
    fn validated_quick_command_text_preserves_surrounding_whitespace() {
        for command in [
            "echo foo ",
            "echo foo   ",
            "  echo foo ",
            "echo {{name}} ",
            "echo foo\t",
        ] {
            assert_eq!(
                validated_quick_command_text(command).as_deref(),
                Some(command)
            );
        }
    }

    #[test]
    fn segmented_interval_accepts_seconds_and_converts_to_integer_milliseconds() {
        for (input, expected) in [("0", 0), ("0.02", 20), ("1.00", 1000), ("60", 60000)] {
            assert_eq!(super::parse_segmented_send_interval(input), Some(expected));
        }
        for input in ["", "abc", "-1", "60.01", "NaN", "inf"] {
            assert_eq!(super::parse_segmented_send_interval(input), None);
        }
    }

    #[test]
    fn validated_quick_command_text_rejects_whitespace_only_input() {
        assert_eq!(validated_quick_command_text("   \t\r\n"), None);
    }
}

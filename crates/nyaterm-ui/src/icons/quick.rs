//! Quick-command icon tags.
//!
//! The old app offered exactly the 24 brand marks; the GPUI build added six
//! generic ones and has already written them into saved commands, so they stay.

use super::IconDef;
use super::connection::BRAND_ICONS;

/// Generic tags with no counterpart in the old app. Listed after the brand marks
/// so the picker reads brand-first, matching the old grid.
const GENERIC_ICONS: &[(&str, IconDef)] = &[
    (
        "terminal",
        IconDef::mono("icons/conn/terminal.svg", 0x4ade80),
    ),
    ("code", IconDef::mono("icons/file/code.svg", 0x58a6ff)),
    ("server", IconDef::mono("icons/brand/server.svg", 0x60a5fa)),
    ("folder", IconDef::mono("icons/conn/folder.svg", 0xfbbf24)),
    ("sparkles", IconDef::mono("icons/ai.svg", 0xa78bfa)),
    ("bolt", IconDef::mono("icons/commands.svg", 0xfbbf24)),
];

/// Picker contents: `None` clears the icon and falls back to the color dot.
pub fn quick_command_icon_options() -> impl Iterator<Item = Option<&'static str>> {
    std::iter::once(None).chain(
        BRAND_ICONS
            .iter()
            .chain(GENERIC_ICONS)
            .map(|(key, _)| Some(*key)),
    )
}

pub static QUICK_COMMAND_ICON_OPTIONS: std::sync::LazyLock<Vec<Option<&'static str>>> =
    std::sync::LazyLock::new(|| quick_command_icon_options().collect());

/// Resolve a stored `icon_tag`. Unlike connection icons these are always
/// monochrome — the old app rendered them as a react-icons glyph plus a CSS
/// color, which is exactly what an alpha mask reproduces.
pub fn quick_command_icon(icon_tag: &str) -> Option<IconDef> {
    let key = icon_tag.trim().to_ascii_lowercase();
    BRAND_ICONS
        .iter()
        .chain(GENERIC_ICONS)
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, def)| *def)
}

#[cfg(test)]
mod tests {
    use super::super::IconPaint;
    use super::{BRAND_ICONS, GENERIC_ICONS, QUICK_COMMAND_ICON_OPTIONS, quick_command_icon};

    /// Tags the GPUI picker has offered since the migration. Saved commands carry
    /// them, so none may stop resolving.
    const PREVIOUSLY_OFFERED_TAGS: &[&str] = &[
        "terminal", "code", "server", "folder", "sparkles", "bolt", "docker", "k8s", "linux",
        "ubuntu", "debian", "centos", "fedora", "apple", "github", "gitlab", "nginx", "redis",
        "postgres", "mysql", "mongodb", "python", "js", "ts", "rust", "go", "node", "php", "aws",
        "gcp",
    ];

    #[test]
    fn every_previously_offered_tag_still_resolves() {
        for tag in PREVIOUSLY_OFFERED_TAGS {
            assert!(
                quick_command_icon(tag).is_some(),
                "{tag} no longer resolves; saved commands would lose their icon"
            );
        }
    }

    #[test]
    fn tags_are_matched_case_insensitively_and_trimmed() {
        assert_eq!(quick_command_icon(" Docker "), quick_command_icon("docker"));
        assert!(quick_command_icon("not-a-tag").is_none());
    }

    #[test]
    fn quick_command_icons_are_all_monochrome() {
        // These render inside small list rows where the icon has to pick up the
        // row's selected/dimmed treatment, so a full-color asset would look wrong.
        for tag in PREVIOUSLY_OFFERED_TAGS {
            let def = quick_command_icon(tag).expect("tag resolves");
            assert!(
                matches!(def.paint, IconPaint::Mono(_)),
                "{tag} resolved to a full-color asset"
            );
            assert!(
                def.path.starts_with("icons/"),
                "{tag} points outside icons/"
            );
        }
    }

    #[test]
    fn picker_offers_a_clear_option_then_every_tag() {
        let options = &*QUICK_COMMAND_ICON_OPTIONS;
        assert_eq!(options.first().copied(), Some(None));
        assert_eq!(options.len(), 1 + BRAND_ICONS.len() + GENERIC_ICONS.len());
        for tag in options.iter().flatten() {
            assert!(quick_command_icon(tag).is_some());
        }
    }
}

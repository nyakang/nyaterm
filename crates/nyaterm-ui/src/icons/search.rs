//! Search-engine icons for the terminal selection search menu and its settings.

use super::IconDef;
use crate::theme::ThemePalette;

/// Engines offered by the settings picker, in display order. `default` is the
/// catch-all for a custom engine with no icon chosen.
pub const SEARCH_ENGINE_ICON_IDS: &[&str] = &[
    "google",
    "bing",
    "duckduckgo",
    "github",
    "gitlab",
    "baidu",
    "yahoo",
    "youtube",
    "bilibili",
    "zhihu",
    "openai",
    "claude",
    "gemini",
    "default",
];

const SEARCH_ICONS: &[(&str, &str, u32)] = &[
    ("google", "icons/brand/google.svg", 0x4285f4),
    // Bing has no mark of its own in the icon sets we vendor; its parent brand is
    // the closest recognizable stand-in, and it stays distinct from `default`.
    ("bing", "icons/brand/bing.svg", 0x008373),
    ("duckduckgo", "icons/brand/duckduckgo.svg", 0xde5833),
    ("github", "icons/brand/github.svg", 0x181717),
    ("gitlab", "icons/brand/gitlab.svg", 0xfc6d26),
    ("baidu", "icons/brand/baidu.svg", 0x2932e1),
    ("yahoo", "icons/brand/yahoo.svg", 0x410093),
    ("youtube", "icons/brand/youtube.svg", 0xff0000),
    ("bilibili", "icons/brand/bilibili.svg", 0x00a1d6),
    ("zhihu", "icons/brand/zhihu.svg", 0x0084ff),
    ("openai", "icons/brand/openai.svg", 0x10a37f),
    ("claude", "icons/brand/claude.svg", 0xd97757),
    ("gemini", "icons/brand/gemini.svg", 0x4285f4),
];

/// Resolve an engine's icon. Unknown and missing ids get the neutral magnifier,
/// themed rather than branded.
pub fn search_engine_icon(icon: Option<&str>, palette: ThemePalette) -> IconDef {
    known_search_engine_icon(icon)
        .map(|(path, color)| IconDef::mono(path, color.unwrap_or(palette.text_muted)))
        .unwrap_or_else(|| IconDef::mono("icons/fe/search.svg", palette.text_muted))
}

/// Resolve only configured Tauri search icons. The terminal context menu omits
/// the icon slot when the old UI would not have found a matching component.
pub fn known_search_engine_icon(icon: Option<&str>) -> Option<(&'static str, Option<u32>)> {
    let key = icon?;
    if key == "default" {
        return Some(("icons/fe/search.svg", None));
    }
    SEARCH_ICONS
        .iter()
        .find(|(candidate, _, _)| *candidate == key)
        .map(|(_, path, color)| (*path, Some(*color)))
}

#[cfg(test)]
mod tests {
    use super::{
        SEARCH_ENGINE_ICON_IDS, ThemePalette, known_search_engine_icon, search_engine_icon,
    };

    fn palette() -> ThemePalette {
        crate::theme::theme_palette("github-dark")
    }

    #[test]
    fn every_offered_id_resolves_to_its_own_asset() {
        let mut paths: Vec<_> = SEARCH_ENGINE_ICON_IDS
            .iter()
            .map(|id| search_engine_icon(Some(id), palette()).path)
            .collect();
        paths.sort_unstable();
        let total = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), total, "two engines share an icon");
    }

    #[test]
    fn unknown_and_missing_ids_get_the_neutral_magnifier() {
        let fallback = search_engine_icon(Some("default"), palette());
        assert_eq!(search_engine_icon(None, palette()), fallback);
        assert_eq!(search_engine_icon(Some("altavista"), palette()), fallback);
        assert_eq!(fallback.path, "icons/fe/search.svg");
    }

    #[test]
    fn strict_lookup_omits_unknown_and_missing_icons() {
        assert!(known_search_engine_icon(None).is_none());
        assert!(known_search_engine_icon(Some("altavista")).is_none());
        assert_eq!(
            known_search_engine_icon(Some("google")),
            Some(("icons/brand/google.svg", Some(0x4285f4)))
        );
        assert_eq!(
            known_search_engine_icon(Some("default")),
            Some(("icons/fe/search.svg", None))
        );
    }
}

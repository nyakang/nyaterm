//! One-click session highlighting: pick a swatch to color the current
//! selection without opening the full keyword-highlight editor.

use crate::features::NyaTermApp;
use gpui::Context;
use nyaterm_core::{
    KEYWORD_HIGHLIGHT_QUICK_COLORS, KeywordHighlightRule, keyword_highlight_content_key,
    keyword_highlight_rule_content_key,
};
use rust_i18n::t;

/// The last curated color is reserved for the custom-color indicator. It stays
/// out of the normal swatch row and only appears when the current text already
/// has a session rule with a non-quick color.
pub(super) const CUSTOM_HIGHLIGHT_SWATCH_INDEX: usize = KEYWORD_HIGHLIGHT_QUICK_COLORS.len() - 1;
pub(super) const VISIBLE_QUICK_COLOR_COUNT: usize = CUSTOM_HIGHLIGHT_SWATCH_INDEX;

/// Localized name used by a swatch's hover tooltip.
pub(super) fn quick_color_name_key(index: usize) -> &'static str {
    match index {
        0 => "terminalCtx.quickColorRed",
        1 => "terminalCtx.quickColorOrange",
        2 => "terminalCtx.quickColorYellow",
        3 => "terminalCtx.quickColorGreen",
        4 => "terminalCtx.quickColorTeal",
        5 => "terminalCtx.quickColorCyan",
        6 => "terminalCtx.quickColorBlue",
        7 => "terminalCtx.quickColorPurple",
        8 => "terminalCtx.quickColorMagenta",
        _ => "terminalCtx.quickColorCustom",
    }
}

/// Tooltip for a swatch. Checked dots remove, all others add, so the text stays
/// truthful when the same dot changes meaning with its checked state.
pub(super) fn quick_color_tooltip(index: usize, checked: bool) -> String {
    let color = t!(quick_color_name_key(index));
    if checked {
        t!("terminalCtx.quickHighlightRemoveColor", color = color).to_string()
    } else {
        t!("terminalCtx.quickHighlightAddColor", color = color).to_string()
    }
}

/// A swatch applies near-white text and the shared saturated dark marker
/// background to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QuickColor {
    pub(super) index: usize,
    pub(super) dark: &'static str,
    pub(super) light: &'static str,
    pub(super) background: &'static str,
}

pub(super) fn quick_color(index: usize) -> Option<QuickColor> {
    KEYWORD_HIGHLIGHT_QUICK_COLORS
        .get(index)
        .map(|(dark, light, background)| QuickColor {
            index,
            dark,
            light,
            background,
        })
}

/// Select a swatch as checked when a session rule exists whose keyword set is
/// exactly the current selection and whose colors match this quick color.
///
/// Quick swatches materialise their background into the rule, so a blank value
/// still counts (it keeps the paint-time lookup) while any other background
/// means the rule no longer looks like the swatch.
fn rule_matches_quick_color(rule: &KeywordHighlightRule, color: QuickColor) -> bool {
    if !rule.color_dark.eq_ignore_ascii_case(color.dark)
        || !rule.color_light.eq_ignore_ascii_case(color.light)
    {
        return false;
    }
    [&rule.bg_color_dark, &rule.bg_color_light]
        .iter()
        .all(|background| {
            let background = background.trim();
            background.is_empty() || background.eq_ignore_ascii_case(color.background)
        })
}

/// Whether a rule uses one of the colors a user can still pick from the menu.
/// The reserved custom-color slot is deliberately excluded.
fn rule_uses_visible_quick_color(rule: &KeywordHighlightRule) -> bool {
    (0..VISIBLE_QUICK_COLOR_COUNT)
        .any(|index| quick_color(index).is_some_and(|color| rule_matches_quick_color(rule, color)))
}

/// Whether `selected` carries the same text content as `rule`, ignoring the
/// order of its keyword lines.
fn selection_matches_rule(selected: &str, rule: &KeywordHighlightRule) -> bool {
    keyword_highlight_content_key(selected)
        .is_some_and(|key| keyword_highlight_rule_content_key(rule) == key)
}

/// The rule name a quick swatch gives to `selected`: the first non-empty line,
/// capped at 40 chars. Kept separate so the menu tooltip and the created rule
/// always agree on the name shown to the user.
pub(super) fn quick_rule_name(selected: &str) -> Option<String> {
    selected
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            if line.chars().count() > 40 {
                line.chars().take(40).collect()
            } else {
                line.to_string()
            }
        })
}

/// Build the session rule a swatch would create for `selected`. The keyword
/// set is the trimmed non-empty lines, regex-escaped, matching the editor's
/// prefill policy so one-click and the popup agree on what a selection means.
///
/// Both theme backgrounds are stored explicitly: the popup then shows the same
/// colour pair the terminal paints, instead of an empty "inherit" background
/// whose value only existed inside the renderer.
pub(super) fn rule_from_selection(
    selected: &str,
    color: QuickColor,
) -> Option<KeywordHighlightRule> {
    let lines: Vec<String> = selected
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let name = quick_rule_name(selected)?;
    Some(KeywordHighlightRule {
        id: format!("kh-quick-{}", uuid::Uuid::new_v4()),
        name,
        patterns: lines.iter().map(|line| regex::escape(line)).collect(),
        color_dark: color.dark.into(),
        color_light: color.light.into(),
        bg_color_dark: color.background.into(),
        bg_color_light: color.background.into(),
        enabled: true,
        case_sensitive: false,
        whole_word: false,
        use_regex: true,
    })
}

impl NyaTermApp {
    /// Which swatch (if any) currently colors exactly this selection, based on
    /// the session-level rules of the given terminal session.
    pub(in crate::features) fn selected_quick_highlight_color(
        &self,
        session_id: &str,
        selected: &str,
    ) -> Option<usize> {
        keyword_highlight_content_key(selected)?;
        let session = self.session_keyword_rules(session_id)?;
        (0..VISIBLE_QUICK_COLOR_COUNT).find(|&index| {
            let color = quick_color(index).expect("index from quick color range");
            session.iter().any(|rule| {
                rule.enabled
                    && rule_matches_quick_color(rule, color)
                    && selection_matches_rule(selected, rule)
            })
        })
    }

    /// The session rule coloring exactly this selection with a custom color.
    /// The context menu turns it into a checked, first-position indicator; the
    /// normal color row never reports a match for such a rule.
    pub(in crate::features) fn selected_custom_highlight_rule(
        &self,
        session_id: &str,
        selected: &str,
    ) -> Option<KeywordHighlightRule> {
        keyword_highlight_content_key(selected)?;
        let session = self.session_keyword_rules(session_id)?;
        session
            .iter()
            .find(|rule| {
                rule.enabled
                    && selection_matches_rule(selected, rule)
                    && !rule_uses_visible_quick_color(rule)
            })
            .cloned()
    }

    /// Apply a swatch to the current selection of a session. Reuses any session
    /// rule carrying the same text content (recoloring and enabling it) so
    /// repeated clicks — or a selection with the same keywords in a different
    /// order — recolor instead of stacking duplicates.
    pub(in crate::features) fn apply_quick_highlight_color(
        &mut self,
        session_id: &str,
        selected: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(color) = quick_color(index) else {
            return;
        };
        let Some(rule) = rule_from_selection(selected, color) else {
            return;
        };
        let mut rules = self
            .session_keyword_rules(session_id)
            .unwrap_or_default()
            .to_vec();
        if let Some(existing) = rules
            .iter_mut()
            .find(|existing| selection_matches_rule(selected, existing) || existing.id == rule.id)
        {
            // Keep the original rule's name/patterns/order; only recolor. A rule
            // may have been renamed or refined in the popup since it was added.
            existing.color_dark = rule.color_dark;
            existing.color_light = rule.color_light;
            existing.bg_color_dark = rule.bg_color_dark;
            existing.bg_color_light = rule.bg_color_light;
            existing.enabled = true;
        } else {
            rules.push(rule);
        }
        self.set_session_keyword_rules(session_id, rules, cx);
        self.shell
            .set_status(t!("terminalCtx.quickHighlightApplied").to_string());
        cx.notify();
    }

    /// Remove the quick-highlight rule currently applied to `selected`. This
    /// mirrors the swatch's checked state: clicking an applied color toggles it
    /// off. Only rules matching the exact keyword set are removed, so other
    /// selections keep their highlights.
    pub(in crate::features) fn remove_quick_highlight_color(
        &mut self,
        session_id: &str,
        selected: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(mut rules) = self
            .session_keyword_rules(session_id)
            .map(|rules| rules.to_vec())
        else {
            return;
        };
        let before = rules.len();
        rules.retain(|rule| !selection_matches_rule(selected, rule));
        if rules.len() == before {
            return;
        }
        self.set_session_keyword_rules(session_id, rules, cx);
        self.shell
            .set_status(t!("terminalCtx.quickHighlightRemoved").to_string());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{VISIBLE_QUICK_COLOR_COUNT, quick_color, quick_rule_name, rule_from_selection};
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::keyword_highlight_content_key as selection_content_key;

    #[test]
    fn quick_colors_are_distinct_and_have_ten_entries() {
        let colors: Vec<_> = (0..10).map(|index| quick_color(index).unwrap()).collect();
        assert_eq!(colors.len(), 10);
        assert_eq!(VISIBLE_QUICK_COLOR_COUNT, 9);
        assert!(quick_color(10).is_none());
        let mut dark = colors.iter().map(|color| color.dark).collect::<Vec<_>>();
        dark.sort_unstable();
        dark.dedup();
        assert_eq!(dark.len(), 10, "quick text colors must be distinct");
        let mut backgrounds = colors
            .iter()
            .map(|color| color.background)
            .collect::<Vec<_>>();
        backgrounds.sort_unstable();
        backgrounds.dedup();
        assert_eq!(backgrounds.len(), 10, "quick backgrounds must be distinct");
        for color in &colors {
            assert!(
                color.dark.starts_with('#')
                    && color.light.starts_with('#')
                    && color.background.starts_with('#')
            );
        }
    }

    #[test]
    fn rule_from_selection_escapes_literal_text_and_drops_blank_lines() {
        let color = quick_color(3).unwrap();
        let rule = rule_from_selection("  ERROR  \n\n[warn]\n", color).unwrap();
        assert_eq!(
            rule.patterns,
            vec![regex::escape("ERROR"), regex::escape("[warn]")]
        );
        assert_eq!(rule.color_dark, color.dark);
        assert_eq!(rule.color_light, color.light);
        assert_eq!(rule.bg_color_dark, color.background);
        assert_eq!(rule.bg_color_light, color.background);
        assert!(rule.enabled);
        assert!(rule_from_selection(" \n ", color).is_none());
    }

    #[test]
    fn quick_rule_name_uses_first_nonempty_line_and_caps_length() {
        assert_eq!(
            quick_rule_name("  \n  ERROR detail  \n[warn]").as_deref(),
            Some("ERROR detail")
        );
        let long = "x".repeat(60);
        assert_eq!(
            quick_rule_name(&long).as_deref(),
            Some("x".repeat(40).as_str())
        );
        assert_eq!(quick_rule_name(" \n\t "), None);
    }

    #[test]
    fn selection_content_key_normalizes_whitespace_and_line_order() {
        assert_eq!(
            selection_content_key("  ERROR  \n\n[warn]\n"),
            selection_content_key("[warn]\nERROR")
        );
        assert_eq!(
            selection_content_key("192.168.1.1"),
            selection_content_key(" 192.168.1.1 \n")
        );
        assert_ne!(
            selection_content_key("ERROR"),
            selection_content_key("ERROR detail")
        );
        assert_eq!(selection_content_key(" \n \t "), None);
    }

    #[test]
    fn applying_quick_color_reuses_rule_for_same_content_in_different_order() {
        let dir = TestConfigDir::new("nyaterm-quick-highlight-dedup");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        cx.update_entity(&app, |app, cx| {
            // First selection creates one rule with two keyword lines.
            app.apply_quick_highlight_color("local", "  ERROR  \n[warn]", 0, cx);
            assert_eq!(app.session_keyword_rules("local").unwrap().len(), 1);

            // Same keywords in reverse order with extra blank lines must reuse
            // the existing rule rather than adding a second one.
            let reordered = "\n[warn]  \n\nERROR\n";
            app.apply_quick_highlight_color("local", reordered, 4, cx);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(
                rules.len(),
                1,
                "duplicate rule created for identical content"
            );
            assert_eq!(
                app.selected_quick_highlight_color("local", reordered),
                Some(4)
            );
            assert_eq!(
                app.selected_quick_highlight_color("local", "ERROR\n[warn]"),
                Some(4)
            );
        });
    }

    #[test]
    fn removing_a_quick_color_deletes_only_the_matching_rule() {
        let dir = TestConfigDir::new("nyaterm-quick-highlight-remove");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        cx.update_entity(&app, |app, cx| {
            app.apply_quick_highlight_color("local", "ERROR", 0, cx);
            app.apply_quick_highlight_color("local", "other text", 2, cx);
            assert_eq!(app.session_keyword_rules("local").unwrap().len(), 2);

            // Remove is idempotent: a second call must be a no-op, which keeps
            // a double-dispatched physical click from re-adding the rule.
            app.remove_quick_highlight_color("local", "ERROR", cx);
            app.remove_quick_highlight_color("local", "ERROR", cx);

            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1, "only the matching rule should be removed");
            assert_eq!(rules[0].patterns, vec![regex::escape("other text")]);
            assert_eq!(app.selected_quick_highlight_color("local", "ERROR"), None);
            assert_eq!(
                app.selected_quick_highlight_color("local", "other text"),
                Some(2)
            );
        });
    }

    #[test]
    fn quick_color_stores_the_swatch_background_for_both_themes() {
        let dir = TestConfigDir::new("nyaterm-quick-highlight-background");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        cx.update_entity(&app, |app, cx| {
            let color = quick_color(2).unwrap();
            app.apply_quick_highlight_color("local", "ERROR", 2, cx);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].bg_color_dark, color.background);
            assert_eq!(rules[0].bg_color_light, color.background);

            // Recolouring rewrites the stored background to the new swatch, so
            // the popup keeps showing the pair the terminal actually paints.
            let next = quick_color(6).unwrap();
            app.apply_quick_highlight_color("local", "ERROR", 6, cx);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].bg_color_dark, next.background);
            assert_eq!(rules[0].bg_color_light, next.background);

            // A hand-edited background no longer reports the swatch as applied.
            let mut edited = rules[0].clone();
            edited.bg_color_dark = "#000000".into();
            app.set_session_keyword_rules("local", vec![edited], cx);
            assert_eq!(app.selected_quick_highlight_color("local", "ERROR"), None);
        });
    }

    #[test]
    fn applying_a_quick_color_creates_then_recolor_the_same_session_rule() {
        let dir = TestConfigDir::new("nyaterm-quick-highlight-apply");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        cx.update_entity(&app, |app, cx| {
            let selected = "192.168.1.1";
            assert_eq!(app.selected_quick_highlight_color("local", selected), None);

            app.apply_quick_highlight_color("local", selected, 0, cx);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].patterns, vec![regex::escape(selected)]);
            assert_eq!(
                app.selected_quick_highlight_color("local", selected),
                Some(0)
            );

            // Clicking a different swatch recolors the existing rule in place.
            app.apply_quick_highlight_color("local", selected, 5, cx);
            let rules = app.session_keyword_rules("local").unwrap();
            assert_eq!(rules.len(), 1);
            assert_eq!(
                app.selected_quick_highlight_color("local", selected),
                Some(5)
            );

            // A different selection gets its own rule.
            app.apply_quick_highlight_color("local", "other text", 2, cx);
            assert_eq!(app.session_keyword_rules("local").unwrap().len(), 2);
            assert_eq!(
                app.selected_quick_highlight_color("local", "other text"),
                Some(2)
            );
            assert_eq!(
                app.selected_quick_highlight_color("local", selected),
                Some(5)
            );
        });
    }

    #[test]
    fn custom_colored_rule_is_found_only_for_its_own_selection() {
        let dir = TestConfigDir::new("nyaterm-quick-highlight-custom");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");

        cx.update_entity(&app, |app, cx| {
            let mut custom = rule_from_selection("ERROR", quick_color(0).unwrap()).unwrap();
            custom.color_dark = "#123456".into();
            custom.color_light = "#654321".into();
            app.set_session_keyword_rules("local", vec![custom], cx);

            assert!(
                app.selected_custom_highlight_rule("local", "ERROR")
                    .is_some()
            );
            assert!(
                app.selected_custom_highlight_rule("local", "WARN")
                    .is_none()
            );
            assert_eq!(app.selected_quick_highlight_color("local", "ERROR"), None);
        });
    }
}

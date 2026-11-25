//! The matching option toggles of a keyword highlight pattern box.
//!
//! Both the session editor and the settings page show the same three
//! options next to the pattern field, so the widgets live here.

use crate::models::KeywordMatchOption;
use crate::theme::ThemePalette;
use gpui::{App, ClickEvent, IntoElement, SharedString, Window, div, prelude::*, px, rgb, rgba};
use nyaterm_ui::{IconName, NyaIcon, NyaTooltip};
use rust_i18n::t;
use std::rc::Rc;

/// The handler that receives a toggled option.
pub(in crate::features) type KeywordMatchOptionHandler =
    Rc<dyn Fn(KeywordMatchOption, &mut Window, &mut App)>;

/// Render the three matching options as a row of toggle icons.
pub(in crate::features) fn keyword_match_option_toggles(
    palette: ThemePalette,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
    on_toggle: KeywordMatchOptionHandler,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(option_button(
            palette,
            KeywordMatchOption::CaseSensitive,
            case_sensitive,
            on_toggle.clone(),
        ))
        .child(option_button(
            palette,
            KeywordMatchOption::WholeWord,
            whole_word,
            on_toggle.clone(),
        ))
        .child(option_button(
            palette,
            KeywordMatchOption::Regex,
            use_regex,
            on_toggle,
        ))
}

/// One toggle icon of the matching options.
fn option_button(
    palette: ThemePalette,
    option: KeywordMatchOption,
    enabled: bool,
    on_toggle: KeywordMatchOptionHandler,
) -> impl IntoElement {
    let (icon, label) = match option {
        KeywordMatchOption::CaseSensitive => (
            IconName::CaseSensitive,
            t!("settings.keywordHighlightMatchCaseSensitive").to_string(),
        ),
        KeywordMatchOption::WholeWord => (
            IconName::WholeWord,
            t!("settings.keywordHighlightMatchWholeWord").to_string(),
        ),
        KeywordMatchOption::Regex => (
            IconName::Regex,
            t!("settings.keywordHighlightMatchRegex").to_string(),
        ),
    };
    // An active option reads as selected, an inactive one stays quiet.
    let color = if enabled {
        palette.primary
    } else {
        palette.text_dimmed
    };
    let background = if enabled {
        rgba((palette.primary << 8) | 0x1f)
    } else {
        rgba(0)
    };
    let tooltip: SharedString = label.into();
    let selector = format!("keyword-match-option-{}", option.control_id());
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .bg(background)
        .text_color(rgb(color))
        .child(NyaIcon::new(icon).size(px(13.)).text_color(rgb(color)))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .cursor_pointer()
        .hover(move |this| this.bg(rgba((palette.hover << 8) | 0x40)))
        .on_click(move |_event: &ClickEvent, window, cx| {
            cx.stop_propagation();
            on_toggle(option, window, cx);
        })
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource as _;
    use nyaterm_ui::IconName;

    /// The toggles draw from gpui-kit's icon catalog, but the shell only
    /// resolves gpui-kit's default pack plus the icons it ships itself. An
    /// icon outside that set leaves a button without a glyph: the button still
    /// takes clicks, yet nothing is painted.
    #[test]
    fn the_toggle_icons_resolve_like_the_application_does() {
        let assets = crate::test_support::ProbeAssets::new();
        for icon in [
            IconName::CaseSensitive,
            IconName::WholeWord,
            IconName::Regex,
        ] {
            let path = icon.path();
            assert!(
                assets.load(&path).unwrap().is_some(),
                "{path} does not resolve, so its toggle would paint nothing"
            );
        }
    }
}

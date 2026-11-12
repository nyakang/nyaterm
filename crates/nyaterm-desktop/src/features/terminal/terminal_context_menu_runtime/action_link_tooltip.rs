use gpui::{Context, IntoElement, Rgba, anchored, div, point, prelude::*, px, rgb};
use rust_i18n::t;

use crate::action_links::{ActionLinkAction, ActionLinkTarget};
use crate::features::NyaTermApp;
use crate::features::view_widgets::mono_icon;
use crate::models::ActionLinkTooltipState;
use crate::theme::ThemePalette;

struct ActionLinkTooltipPresentation {
    title_key: Option<&'static str>,
    activation_key: &'static str,
    icon_path: &'static str,
}

impl ActionLinkTooltipPresentation {
    fn for_action(action: &ActionLinkAction) -> Self {
        // The execution target determines the verb; the stable action id only
        // supplies a friendlier title. Never infer behavior from command text.
        match &action.target {
            ActionLinkTarget::OpenUrl(_) => Self {
                title_key: Some("terminal.actionLinkOpenLink"),
                activation_key: "terminal.actionLinkOpen",
                icon_path: "icons/settings/link.svg",
            },
            ActionLinkTarget::CopyText(_) => Self {
                title_key: action
                    .mac_format
                    .map(|_| "terminal.actionLinkCopyMacAddress"),
                activation_key: "terminal.actionLinkCopy",
                icon_path: "icons/copy.svg",
            },
            ActionLinkTarget::PasteText(_) => Self {
                title_key: action
                    .mac_format
                    .map(|_| "terminal.actionLinkInsertMacAddress"),
                activation_key: "terminal.actionLinkInsert",
                icon_path: "icons/conn/terminal.svg",
            },
            ActionLinkTarget::ExecuteCommand(_) => {
                let (title_key, icon_path) = match action.id.as_str() {
                    "extract" => (
                        Some("terminal.actionLinkExtractArchive"),
                        "icons/file/archive.svg",
                    ),
                    "list" => (
                        Some("terminal.actionLinkListArchive"),
                        "icons/file/archive.svg",
                    ),
                    "ping" => (Some("terminal.actionLinkPingAddress"), "icons/network.svg"),
                    "curl" | "curl-http" | "curl-https" => {
                        (Some("terminal.actionLinkHttpRequest"), "icons/network.svg")
                    }
                    _ => (None, "icons/conn/terminal.svg"),
                };
                Self {
                    title_key,
                    activation_key: "terminal.actionLinkExecute",
                    icon_path,
                }
            }
        }
    }

    fn title(&self, action: &ActionLinkAction, locale: &str) -> String {
        self.title_key.map_or_else(
            || action.label.clone(),
            |key| t!(key, locale = locale).to_string(),
        )
    }

    fn hint(&self, has_more_actions: bool, is_macos: bool, locale: &str) -> String {
        let modifier = if is_macos { "⌘" } else { "Ctrl" };
        let action = t!(self.activation_key, locale = locale);
        let primary = t!(
            "terminal.actionLinkPrimaryClickHint",
            locale = locale,
            modifier = modifier,
            action = action,
        );
        if has_more_actions {
            let more = t!(
                "terminal.actionLinkMoreClickHint",
                locale = locale,
                modifier = if is_macos { "Option" } else { "Alt" },
            );
            format!("{primary} · {more}")
        } else {
            primary.to_string()
        }
    }
}

fn action_link_tooltip_card(
    tip: &ActionLinkTooltipState,
    palette: ThemePalette,
    background: Rgba,
    viewport_width: f32,
    locale: &str,
) -> impl IntoElement {
    let presentation = ActionLinkTooltipPresentation::for_action(&tip.default_action);
    // No hitbox or focus handle: even when edge placement overlaps terminal
    // text, this passive card must let selection and modifier clicks through.
    div()
        .id("action-link-tooltip-overlay")
        .debug_selector(|| "action-link-tooltip".into())
        .w(px((viewport_width - 16.).clamp(0., 354.)))
        .min_w_0()
        .flex_none()
        .whitespace_normal()
        .rounded(px(12.))
        .border_1()
        .border_color(rgb(palette.border))
        .bg(background)
        .shadow_lg()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(14.))
                .text_color(rgb(palette.text))
                .child(div().flex_none().child(mono_icon(
                    presentation.icon_path,
                    rgb(palette.text).into(),
                    14.,
                )))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(presentation.title(&tip.default_action, locale)),
                ),
        )
        .child(
            div()
                .id("action-link-tooltip-hint")
                .debug_selector(|| "action-link-tooltip-hint".into())
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(presentation.hint(tip.has_more_actions, cfg!(target_os = "macos"), locale)),
        )
}

fn action_link_tooltip_layer(
    tip: &ActionLinkTooltipState,
    palette: ThemePalette,
    background: Rgba,
    viewport_width: f32,
    locale: &str,
) -> impl IntoElement {
    // Include the edge margin in the measured bounds. Anchored's margin option
    // only applies after overflow, so a card exactly touching an edge would
    // otherwise lose its margin, especially when it fills a narrow window.
    anchored()
        .position(point(tip.x + px(8.), tip.y + px(8.)))
        .snap_to_window()
        .child(
            div()
                .p(px(8.))
                .flex()
                .flex_col()
                .flex_none()
                .child(action_link_tooltip_card(
                    tip,
                    palette,
                    background,
                    viewport_width,
                    locale,
                )),
        )
}

impl NyaTermApp {
    pub(in crate::features) fn action_link_tooltip_overlay(
        &self,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(tip) = self.terminal.menus.action_link_tooltip.as_ref() else {
            return div().into_any_element();
        };
        if !self.action_link_tooltip_has_visible_source(tip) {
            return div().into_any_element();
        }
        let palette = self.theme_palette();
        action_link_tooltip_layer(
            tip,
            palette,
            self.shell_surface_color(palette.surface_elevated),
            self.shell.viewport_size().0,
            rust_i18n::locale().as_ref(),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests;

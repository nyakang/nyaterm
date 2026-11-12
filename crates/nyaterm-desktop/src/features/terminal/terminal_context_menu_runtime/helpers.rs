use gpui::{App, ClickEvent, IntoElement, SharedString, Window, div, prelude::*, px, rgb};

use crate::features::icons::IconDef;
use crate::features::view_widgets::mono_icon;

pub(super) fn clamp_menu_position(
    x: f32,
    y: f32,
    menu_w: f32,
    menu_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    let margin = 8.0;
    let max_x = (viewport_w - menu_w - margin).max(margin);
    let max_y = (viewport_h - menu_h - margin).max(margin);
    (x.clamp(margin, max_x), y.clamp(margin, max_y))
}

pub(super) fn terminal_ctx_item_with_icon(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    icon: Option<IconDef>,
    shortcut: Option<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement + StatefulInteractiveElement {
    let label = label.into();
    let leading = div()
        .flex()
        .items_center()
        .gap_2()
        // A row with no icon still reserves the column, so labels line up.
        .child(div().size(px(14.)).flex_none().children(icon.map(|def| {
            mono_icon(
                def.path,
                rgb(def.tint(palette).unwrap_or(palette.text_muted)).into(),
                14.,
            )
        })))
        .child(label);
    let mut row = div()
        .id(SharedString::from(id.into()))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_size(px(12.))
        .text_color(rgb(palette.text))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)))
        .on_click(on_click)
        .child(leading);
    if let Some(shortcut) = shortcut {
        row = row.child(
            div()
                .text_size(px(10.))
                .text_color(rgb(palette.text_dimmed))
                .child(shortcut),
        );
    }
    row
}

pub(super) fn search_engine_url(template: &str, query: &str) -> String {
    let encoded = urlencoding_minimal(query);
    if template.is_empty() {
        format!("https://www.google.com/search?q={encoded}")
    } else {
        template.replacen("%s", &encoded, 1)
    }
}

pub(super) fn urlencoding_minimal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.as_bytes() {
        match *b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => {
                out.push(*b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub(super) fn available_translation_providers(
    settings: &nyaterm_core::TranslationSettings,
) -> Vec<(String, String)> {
    let mut providers = vec![
        ("google".to_string(), "Google".to_string()),
        ("microsoft".to_string(), "Microsoft".to_string()),
    ];
    if !settings.deepl_api_key.trim().is_empty() {
        providers.push(("deepl".to_string(), "DeepL".to_string()));
    }
    if !settings.baidu_app_id.trim().is_empty() && !settings.baidu_app_key.trim().is_empty() {
        providers.push(("baidu".to_string(), "Baidu".to_string()));
    }
    if !settings.ali_app_id.trim().is_empty() && !settings.ali_app_key.trim().is_empty() {
        providers.push(("ali".to_string(), "Aliyun".to_string()));
    }
    if !settings.youdao_app_id.trim().is_empty() && !settings.youdao_app_key.trim().is_empty() {
        providers.push(("youdao".to_string(), "Youdao".to_string()));
    }
    providers
}

#[cfg(test)]
mod tests {
    use super::{search_engine_url, urlencoding_minimal};

    #[test]
    fn online_search_url_matches_tauri_template_rules() {
        assert_eq!(
            search_engine_url("", "hello world"),
            "https://www.google.com/search?q=hello%20world"
        );
        assert_eq!(
            search_engine_url("https://search.test/?q=%s&again=%s", "Rust 中文"),
            "https://search.test/?q=Rust%20%E4%B8%AD%E6%96%87&again=%s"
        );
        assert_eq!(
            search_engine_url("https://search.test/static", "ignored"),
            "https://search.test/static"
        );
    }

    #[test]
    fn search_query_encoding_matches_javascript_encode_uri_component() {
        assert_eq!(
            urlencoding_minimal("a b/中文"),
            "a%20b%2F%E4%B8%AD%E6%96%87"
        );
        assert_eq!(urlencoding_minimal("!~*'()"), "!~*'()");
    }
}

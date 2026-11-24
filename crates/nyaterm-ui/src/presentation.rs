//! Stateless presentation helpers. Applications supply labels and actions.
use gpui::{
    AnyElement, App, ClickEvent, FontWeight, IntoElement, SharedString, TextOverflow, Window, div,
    prelude::*, px, rgb, svg,
};

use crate::theme::ThemePalette;

/// Tauri EmptyWorkspaceState row: action label (primary) + shortcut key chips.
pub fn shortcut_action(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    shortcut: impl Into<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    // Tauri EmptyWorkspaceState: primary label left, Kbd chips right with "+" separators.
    let label = label.into();
    let shortcut = shortcut.into();
    let parts: Vec<String> = shortcut
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let mut keys = div().flex().items_center().gap_1();
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            keys = keys.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child("+"),
            );
        }
        keys = keys.child(
            div()
                .h(px(24.))
                .min_w(px(28.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(part),
        );
    }

    // Tauri: grid-cols-[max-content_auto] gap-x-4 gap-y-3; label primary, kbd chips right.
    div()
        .id(SharedString::from(format!("empty-action-{label}")))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .w_full()
        .max_w(px(480.))
        .cursor_pointer()
        .child(
            div()
                .min_w(px(160.))
                .text_sm()
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.primary))
                .hover(|this| this.text_color(rgb(palette.text)))
                .child(label),
        )
        .child(keys)
        .on_click(on_click)
}

pub fn action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let detail: SharedString = detail.into();
    action_button_enabled(palette, id, label, detail, true, on_click)
}

pub fn action_button_enabled(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let detail: SharedString = detail.into();
    div()
        .id(SharedString::from(id.into()))
        .min_h(px(46.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)).border_color(rgb(0x3b82f6)))
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight(800.))
                .text_color(if enabled {
                    rgb(palette.text)
                } else {
                    rgb(palette.text_dimmed)
                })
                .child(label),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(palette.text_muted))
                .child(detail),
        )
        .when(enabled, |this| this.on_click(on_click))
}

pub fn progress_bar(palette: ThemePalette, ratio: f64) -> impl IntoElement {
    let ratio = ratio.clamp(0., 1.) as f32;
    div()
        .h(px(5.))
        .w_full()
        .overflow_hidden()
        .rounded_sm()
        .bg(rgb(palette.border))
        .child(
            div()
                .h(px(5.))
                .w(gpui::relative(ratio))
                .rounded_sm()
                .bg(if ratio >= 0.9_f32 {
                    rgb(0xfb7185)
                } else if ratio >= 0.7_f32 {
                    rgb(palette.warning)
                } else {
                    rgb(palette.primary)
                }),
        )
}

pub fn menu_item(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    icon_path: Option<&'static str>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    menu_item_with_shortcut_inner(palette, id, label, None, icon_path, enabled, on_click)
}

pub fn menu_item_with_shortcut(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    shortcut: impl Into<String>,
    icon_path: Option<&'static str>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    menu_item_with_shortcut_inner(
        palette,
        id,
        label,
        Some(shortcut.into()),
        icon_path,
        enabled,
        on_click,
    )
}

fn menu_item_with_shortcut_inner(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<String>,
    shortcut: Option<String>,
    icon_path: Option<&'static str>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id = id.into();
    let label = label.into();
    let text_color = rgb(palette.text);
    let icon_color = rgb(palette.text_muted);
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .h(px(28.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_sm()
        .text_size(px(12.))
        .text_color(text_color)
        .opacity(if enabled { 1. } else { 0.5 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click(on_click)
        })
        .when_some(icon_path, |this, icon_path| {
            this.child(
                div()
                    .w(px(24.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(svg().size(px(16.)).path(icon_path).text_color(icon_color)),
            )
        })
        .child(div().min_w_0().flex_1().child(label))
        .when_some(shortcut, |this, shortcut| {
            this.child(
                div()
                    .flex_none()
                    .pl_1()
                    .text_xs()
                    .text_color(icon_color)
                    .child(shortcut),
            )
        })
}

pub fn menu_separator(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).flex_none().my_1().bg(rgb(palette.border))
}

pub fn panel_header_with_actions(
    title: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    palette: ThemePalette,
    background: gpui::Rgba,
    actions: Option<AnyElement>,
) -> impl IntoElement {
    let title = title.into();
    let meta = meta.into();
    let show_meta = !meta.is_empty();
    let heading = div()
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .flex()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .font_weight(FontWeight(700.))
                .text_color(rgb(palette.text_muted))
                .child(title.to_uppercase()),
        )
        .when(show_meta, |this| {
            this.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .opacity(0.85)
                    .child(meta),
            )
        });
    panel_header_shell(heading.into_any_element(), palette, background, actions)
}
pub fn conversation_panel_header_with_actions(
    title: impl Into<SharedString>,
    palette: ThemePalette,
    background: gpui::Rgba,
    actions: Option<AnyElement>,
) -> impl IntoElement {
    let heading = div()
        .debug_selector(|| "ai-conversation-title".into())
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_overflow(TextOverflow::Truncate("...".into()))
        .text_size(px(11.))
        .text_color(rgb(palette.text_muted))
        .child(title.into());
    panel_header_shell(heading.into_any_element(), palette, background, actions)
}
fn panel_header_shell(
    heading: AnyElement,
    palette: ThemePalette,
    background: gpui::Rgba,
    actions: Option<AnyElement>,
) -> gpui::Div {
    // Keep controls in flow; narrow panels may need another row for actions.
    div()
        .min_h(px(36.))
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_x_3()
        .px_3()
        .py(px(3.))
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(background)
        .child(heading)
        .when_some(actions, |this, actions| {
            this.child(
                div()
                    .flex_none()
                    .max_w_full()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(actions),
            )
        })
}

pub fn number_input_box_from_state(
    id: SharedString,
    palette: crate::ThemePalette,
    field: Entity<NyaNumberInputState>,
) -> impl IntoElement {
    div()
        .id(id)
        // The form or toolbar owns the width, just as for text inputs and selects.
        .w_full()
        .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
        .min_w_0()
        .flex()
        .items_center()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaNumberInput::new(&field)),
        )
}

pub fn format_file_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / 1024. / 1024.)
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.)
    } else {
        format!("{size} B")
    }
}

pub fn gpui_code_font_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "JetBrains Mono"
    }
}

pub fn compact_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= 12 {
        trimmed.to_string()
    } else {
        let prefix: String = trimmed.chars().take(8).collect();
        let suffix: String = trimmed
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{prefix}..{suffix}")
    }
}

use crate::{NYA_FORM_CONTROL_HEIGHT_PX, NyaNumberInput, NyaNumberInputState};
use gpui::Entity;

#[cfg(test)]
mod tests {
    use super::{menu_item, menu_item_with_shortcut, menu_separator};
    use crate::theme::theme_palette;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px};
    use std::{cell::Cell, rc::Rc};

    struct MenuFixture(Rc<Cell<usize>>);
    impl Render for MenuFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = theme_palette("github-dark");
            let mut menu = div().w(px(200.)).flex().flex_col();
            for (id, enabled, icon) in [
                ("enabled", true, Some("icons/copy.svg")),
                ("disabled", false, None),
            ] {
                let clicks = self.0.clone();
                menu = menu.child(menu_item(palette, id, id, icon, enabled, move |_, _, _| {
                    clicks.set(clicks.get() + 1)
                }));
            }
            let shortcut_clicks = self.0.clone();
            menu.child(menu_item_with_shortcut(
                palette,
                "shortcut",
                "Shortcut",
                "Ctrl+Shift+Alt+F11",
                None,
                true,
                move |_, _, _| shortcut_clicks.set(shortcut_clicks.get() + 1),
            ))
            .child(menu_separator(palette))
        }
    }

    #[gpui::test]
    fn menu_ids_height_and_disabled_clicks_remain_stable(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let (_, vcx) = cx.add_window_view(|_, _| MenuFixture(clicks.clone()));
        vcx.run_until_parked();
        for id in ["enabled", "disabled"] {
            let bounds = vcx.debug_bounds(id).unwrap();
            assert_eq!(bounds.size.height, px(28.));
            vcx.simulate_click(bounds.center(), Default::default());
        }
        assert_eq!(clicks.get(), 1);
    }
}

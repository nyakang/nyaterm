//! Stateless presentation helpers. Applications supply labels and actions.
use gpui::{
    App, ClickEvent, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px, rgb, svg,
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
}

pub fn menu_separator(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).flex_none().my_1().bg(rgb(palette.border))
}

#[cfg(test)]
mod tests {
    use super::{menu_item, menu_separator};
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
            menu.child(menu_separator(palette))
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

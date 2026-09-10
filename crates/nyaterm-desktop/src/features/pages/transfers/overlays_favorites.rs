use rust_i18n::t;

use gpui::{
    App, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::NyaScrollable;

use crate::features::{NyaTermApp, shell::gpui_code_font_family};
use crate::models::TransferBrowserFavoritesMenuState;
use crate::theme::ThemePalette;

use super::{normalized_transfer_browser_path, transfer_menu_position};

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn open_transfer_browser_favorites_menu(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.transfer.open_browser_favorites_menu(
            TransferBrowserFavoritesMenuState {
                x: event.position.x,
                y: event.position.y + px(18.),
            },
            t!("fileExplorer.favorites").to_string(),
        );
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_browser_favorites_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.transfer.close_browser_favorites_menu();
        cx.notify();
    }

    pub(in crate::features) fn transfer_browser_favorites_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let state = self.transfer.browser_view().favorites_menu.unwrap_or(
            TransferBrowserFavoritesMenuState {
                x: px(24.),
                y: px(24.),
            },
        );
        let current_path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        let favorite_paths = self
            .transfer
            .browser_view()
            .favorites
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (menu_x, menu_y, menu_max_height) = transfer_menu_position(
            f32::from(state.x),
            f32::from(state.y),
            300.,
            96. + favorite_paths.len() as f32 * 38.,
            viewport_w,
            viewport_h,
        );

        let mut list = div().flex().flex_col().gap_1();
        for path in favorite_paths {
            let is_current = path == current_path;
            let open_path = path.clone();
            let remove_path = path.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "transfer-browser-favorite-menu-item-{path}"
                    )))
                    .min_h(px(30.))
                    .rounded_sm()
                    .border_1()
                    .border_color(if is_current {
                        rgb(0x256d3f)
                    } else {
                        rgb(palette.border)
                    })
                    .bg(if is_current {
                        rgb(palette.hover)
                    } else {
                        rgb(palette.surface)
                    })
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_transfer_browser_favorites_menu(cx);
                        this.open_transfer_browser_directory(open_path.clone(), window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .font_family(gpui_code_font_family())
                            .text_size(px(10.))
                            .text_color(if is_current {
                                rgb(0x93c5fd)
                            } else {
                                rgb(palette.text)
                            })
                            .child(truncate_preview(&path, 46)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "transfer-browser-favorite-menu-remove-{remove_path}"
                            )))
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .text_color(rgb(0x86efac))
                            .hover(|this| this.bg(rgb(palette.border)).text_color(rgb(0xfca5a5)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                cx.stop_propagation();
                                this.remove_transfer_browser_favorite_path(remove_path.clone(), cx);
                                this.defer_transfer_panel_snapshot_flush(cx);
                            }))
                            .child(
                                svg()
                                    .size(px(14.))
                                    .path("icons/fe/bookmark-remove.svg")
                                    .text_color(rgb(0x86efac)),
                            ),
                    ),
            );
        }

        div()
            .id(SharedString::from(
                "transfer-browser-favorites-menu-overlay",
            ))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_transfer_browser_favorites_menu(cx);
            }))
            .child(
                div()
                    .id(SharedString::from("transfer-browser-favorites-menu"))
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(300.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .max_h(px(menu_max_height))
                            .overflow_y_scrollbar()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div().flex().items_center().justify_between().gap_2().child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight(800.))
                                        .text_color(rgb(palette.text))
                                        .child(t!("fileExplorer.favorites")),
                                ),
                            )
                            .child(favorite_menu_button(
                                palette,
                                "transfer-browser-favorite-menu-add-current",
                                t!("fileExplorer.addCurrentDirToFavorites"),
                                cx.listener(|this, _, _, cx| {
                                    this.add_current_transfer_browser_favorite(cx);
                                }),
                            ))
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(rgb(palette.border))
                                    .pt_2()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .when(
                                        self.transfer.browser_view().favorites.is_empty(),
                                        |this| {
                                            this.child(
                                                div()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(palette.border))
                                                    .bg(rgb(palette.input))
                                                    .px_2()
                                                    .py_2()
                                                    .text_xs()
                                                    .text_color(rgb(palette.text_muted))
                                                    .child(t!("fileExplorer.noFavorites")),
                                            )
                                        },
                                    )
                                    .when(
                                        !self.transfer.browser_view().favorites.is_empty(),
                                        |this| this.child(list),
                                    ),
                            ),
                    ),
            )
    }
}

fn favorite_menu_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.into()))
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .text_color(rgb(palette.text))
        .text_xs()
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)))
        .child(label.into())
        .on_click(on_click)
}

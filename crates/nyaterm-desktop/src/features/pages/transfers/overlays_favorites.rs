use rust_i18n::t;

use gpui::{
    Context, InteractiveElement as _, IntoElement, MouseDownEvent, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rgb,
};
use nyaterm_ui::{NyaCompactMenuRow, NyaScrollable};

use crate::features::NyaTermApp;
use crate::models::TransferBrowserFavoritesMenuState;

use super::{normalized_transfer_browser_path, transfer_menu_position};

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn open_transfer_browser_favorites_menu(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.transfer.open_browser_favorites_menu(
            TransferBrowserFavoritesMenuState {
                x: event.position.x,
                y: event.position.y + px(18.),
                focus,
                scroll: gpui::ScrollHandle::new(),
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
        let Some(state) = self.transfer.browser_view().favorites_menu.clone() else {
            return div().into_any_element();
        };
        let current_path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        let favorite_paths = self.transfer.browser_view().favorites.clone();
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let menu_width = 220_f32.min((viewport_w - 16.).max(1.));
        let (menu_x, menu_y, menu_max_height) = transfer_menu_position(
            f32::from(state.x),
            f32::from(state.y),
            menu_width,
            42. + favorite_paths.len().max(1) as f32 * 32.,
            viewport_w,
            viewport_h,
        );
        let mut list = div().flex_none().flex().flex_col();
        for (index, path) in favorite_paths.into_iter().enumerate() {
            let is_current = path == current_path;
            let open_path = path.clone();
            let remove_path = path.clone();
            let menu_focus = state.focus.clone();
            list = list.child(
                NyaCompactMenuRow::new(
                    format!("transfer-browser-favorite-menu-item-{path}"),
                    path,
                    "icons/session/folder-open.svg",
                    0xf5c542,
                    palette,
                    cx.listener(move |this, _, window, cx| {
                        this.close_transfer_browser_favorites_menu(cx);
                        this.restore_transfer_browser_focus(window, cx);
                        this.open_transfer_browser_directory(open_path.clone(), window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    }),
                )
                .current(is_current)
                .scroll_on_focus(state.scroll.clone(), index)
                .trailing_action(
                    format!("transfer-browser-favorite-menu-remove-{remove_path}"),
                    "icons/fe/bookmark-remove.svg",
                    t!("fileExplorer.removeFavorite"),
                    cx.listener(move |this, _, window, cx| {
                        // The focused action is about to disappear; keep keys in this menu.
                        window.focus(&menu_focus, cx);
                        this.remove_transfer_browser_favorite_path(remove_path.clone(), cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    }),
                ),
            );
        }

        div()
            .id("transfer-browser-favorites-menu-overlay")
            .absolute()
            .inset_0()
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_transfer_browser_favorites_menu(cx);
                this.restore_transfer_browser_focus(window, cx);
            }))
            .on_aux_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_transfer_browser_favorites_menu(cx);
                this.restore_transfer_browser_focus(window, cx);
            }))
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("transfer-browser-favorites-menu")
                    .debug_selector(|| "transfer-browser-favorites-menu".to_string())
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(menu_width))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .tab_group()
                    .track_focus(&state.focus)
                    .tab_index(0)
                    .tab_stop(false)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                this.close_transfer_browser_favorites_menu(cx);
                                this.restore_transfer_browser_focus(window, cx);
                            }
                            "tab" | "down" | "up" => {
                                if let Some(menu) =
                                    this.transfer.browser_view().favorites_menu.as_ref()
                                {
                                    move_menu_focus(
                                        &menu.focus,
                                        event.keystroke.key == "up"
                                            || (event.keystroke.key == "tab"
                                                && event.keystroke.modifiers.shift),
                                        window,
                                        cx,
                                    );
                                }
                            }
                            _ => {}
                        }
                        cx.stop_propagation();
                    }))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_aux_click(|_, _, cx| cx.stop_propagation())
                    .child(NyaCompactMenuRow::new(
                        "transfer-browser-favorite-menu-add-current",
                        t!("fileExplorer.addCurrentDirToFavorites"),
                        "icons/session/folder-open.svg",
                        palette.link,
                        palette,
                        cx.listener(|this, _, _, cx| {
                            this.add_current_transfer_browser_favorite(cx);
                        }),
                    ))
                    .child(
                        div()
                            .relative()
                            .min_h_0()
                            .child(
                                list.id("transfer-browser-favorites-scroll")
                                    .debug_selector(|| {
                                        "transfer-browser-favorites-scroll".to_string()
                                    })
                                    .max_h(px((menu_max_height - 42.).max(0.)))
                                    .overflow_y_scroll()
                                    .track_scroll(&state.scroll)
                                    .when(
                                        self.transfer.browser_view().favorites.is_empty(),
                                        |list| {
                                            list.child(
                                                div()
                                                    .h(px(32.))
                                                    .px(px(8.))
                                                    .flex()
                                                    .items_center()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(palette.text_muted))
                                                    .child(t!("fileExplorer.noFavorites")),
                                            )
                                        },
                                    ),
                            )
                            .vertical_scrollbar(&state.scroll),
                    ),
            )
            .into_any_element()
    }
}

// Tab groups order controls but do not trap focus. Cycle past other groups so
// neither Tab nor arrow navigation can hand an open menu's keys to the terminal.
fn move_menu_focus(
    focus: &gpui::FocusHandle,
    reverse: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let mut first = None;
    loop {
        if reverse {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        if focus.contains_focused(window, cx) {
            break;
        }
        let candidate = window.focused(cx);
        if candidate.is_none() || first.as_ref() == Some(&candidate) {
            window.focus(focus, cx);
            break;
        }
        first.get_or_insert(candidate);
    }
}

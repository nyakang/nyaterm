use super::{icon_button, small_button};
use crate::features::file_manager::pane::{PaneEndpoint, PaneId};
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::theme::ThemePalette;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem, NyaSearchInput};
use rust_i18n::t;

impl FileManagerWorkspace {
    pub(super) fn navigation_view(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.pane(pane);
        let endpoint = state.endpoint.clone();
        let source = self.read_app(cx, |app| app.endpoint_label(&endpoint));
        let mut choices = self.read_app(cx, |app| app.file_manager_source_choices());
        if !choices.iter().any(|(choice, _)| choice == &endpoint) {
            choices.push((endpoint.clone(), source.clone()));
        }
        let items = choices
            .into_iter()
            .map(|(choice, label)| {
                NyaMenuItem::action(label)
                    .checked(choice == endpoint)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let id = this.id.clone();
                        let endpoint = choice.clone();
                        this.focus_pane(pane, window, cx);
                        this.dispatch_app(window, cx, move |app, _, cx| {
                            app.switch_file_manager_source(id, pane, endpoint, cx)
                        });
                    }))
            })
            .collect::<Vec<_>>();
        let source_menu = NyaDropdownMenu::new(format!("fm-source-{}-{}", self.id, pane.as_str()))
            .select_trigger()
            .tooltip(source.clone())
            .label(source)
            .items(items);
        let compact_navigation = self.pane_width(pane) < 400.;
        let mut actions = div()
            .debug_selector(move || format!("file-manager-actions-{}", pane.as_str()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.));
        for (delta, icon, tooltip) in [
            (-1, "icons/fe/arrow-left.svg", t!("fileManager.goBack")),
            (1, "icons/fe/arrow-right.svg", t!("fileManager.goForward")),
        ] {
            actions = actions.child(
                icon_button(
                    palette,
                    format!("fm-history-{}-{delta}", pane.as_str()),
                    icon,
                    tooltip,
                    cx.listener(move |this, _, window, cx| {
                        this.focus_pane(pane, window, cx);
                        if let Some(path) = this.pane_mut(pane).history_path(delta) {
                            this.navigate_to(pane, path, window, cx);
                        }
                    }),
                )
                .disabled(if delta < 0 {
                    state.history_index == 0
                } else {
                    state.history_index + 1 >= state.history.len()
                }),
            );
        }
        actions = actions
            .child(icon_button(
                palette,
                format!("fm-up-{}", pane.as_str()),
                "icons/fe/up.svg",
                t!("fileExplorer.goUp"),
                cx.listener(move |this, _, window, cx| this.navigate_parent(pane, window, cx)),
            ))
            .child(icon_button(
                palette,
                format!("fm-home-{}", pane.as_str()),
                "icons/android/home.svg",
                t!("fileManager.goHome"),
                cx.listener(move |this, _, window, cx| {
                    let id = this.id.clone();
                    this.focus_pane(pane, window, cx);
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        app.navigate_file_manager_home(id, pane, cx)
                    });
                }),
            ))
            .child(icon_button(
                palette,
                format!("fm-refresh-{}", pane.as_str()),
                "icons/fe/refresh.svg",
                t!("fileExplorer.refresh"),
                cx.listener(move |this, _, window, cx| {
                    let id = this.id.clone();
                    this.focus_pane(pane, window, cx);
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        app.refresh_file_manager_pane(id, pane, cx)
                    });
                }),
            ))
            .child(icon_button(
                palette,
                format!("fm-copy-path-{}", pane.as_str()),
                "icons/copy.svg",
                t!("fileExplorer.copyDirPath"),
                cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        this.pane(pane).path.display_path.clone(),
                    ));
                }),
            ))
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("file-manager-swap-{}", pane.as_str()))
                    .child(icon_button(
                        palette,
                        format!("fm-swap-{}", pane.as_str()),
                        "icons/swap-horiz.svg",
                        t!("fileManager.swapPanes"),
                        cx.listener(move |this, _, window, cx| {
                            this.focus_pane(pane, window, cx);
                            this.swap_panes();
                            for pane in [PaneId::Left, PaneId::Right] {
                                let search = this.pane(pane).search.clone();
                                this.inputs(pane)
                                    .search
                                    .update(cx, |input, cx| input.set_content_silent(&search, cx));
                            }
                            let id = this.id.clone();
                            this.dispatch_app(window, cx, move |app, _, cx| {
                                app.refresh_file_manager_pane(id.clone(), PaneId::Left, cx);
                                app.refresh_file_manager_pane(id, PaneId::Right, cx);
                                app.persist_file_manager_settings(cx);
                            });
                            this.focus_pane(this.active, window, cx);
                        }),
                    )),
            );
        actions = actions.child(icon_button(
            palette,
            format!("fm-search-{}", pane.as_str()),
            "icons/fe/search.svg",
            t!("fileExplorer.search"),
            cx.listener(move |this, _, window, cx| this.focus_search(pane, None, window, cx)),
        ));
        if let Some(session) = endpoint.session_id().map(str::to_owned) {
            let (available, status) = self.read_app(cx, |app| {
                (
                    app.file_sudo_available(&session),
                    app.file_sudo_status(&session),
                )
            });
            if available || status.is_sudo() {
                actions = actions.child(
                    icon_button(
                        palette,
                        format!("fm-sudo-{}", pane.as_str()),
                        "icons/shield.svg",
                        crate::features::transfers::privilege_runtime::sudo_label(status),
                        cx.listener(move |this, _, window, cx| {
                            let session = session.clone();
                            this.dispatch_app(window, cx, move |app, window, cx| {
                                app.toggle_file_sudo(session, window, cx)
                            });
                        }),
                    )
                    .text_color(rgb(if status.is_sudo() {
                        palette.warning
                    } else {
                        palette.text_muted
                    }))
                    .disabled(status.is_busy()),
                );
            }
        }
        actions = actions.when(compact_navigation, |actions| {
            actions.w_full().min_w_0().flex_wrap().justify_end()
        });
        let toolbar = div()
            .relative()
            .min_w_0()
            .min_h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(4.))
            .when(compact_navigation, |row| row.flex_wrap())
            .child(
                div()
                    .debug_selector(move || format!("file-manager-source-{}", pane.as_str()))
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .child(source_menu),
            )
            .child(actions)
            .when(
                state.search_expanded || !state.search.is_empty(),
                |toolbar| {
                    let close = icon_button(
                        palette,
                        format!("fm-search-close-{}", pane.as_str()),
                        "icons/window/close.svg",
                        t!("common.close"),
                        cx.listener(move |this, _, window, cx| {
                            this.clear_or_close_search(pane, window, cx)
                        }),
                    );
                    toolbar.child(nyaterm_ui::file_navigation::search_overlay(
                        palette,
                        NyaSearchInput::new(
                            format!("fm-search-input-{}", pane.as_str()),
                            &self.inputs(pane).search,
                        )
                        .compact()
                        .bare()
                        .trailing(close)
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if event.keystroke.key == "escape" {
                                    cx.stop_propagation();
                                    this.clear_or_close_search(pane, window, cx);
                                }
                            },
                        )),
                    ))
                },
            );
        let mut chrome = div()
            .flex_none()
            .min_w_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(self.path_navigation_view(palette, pane, cx));
        if state.disconnected || matches!(endpoint, PaneEndpoint::OfflineConnection(_)) {
            chrome = chrome.child(
                div()
                    .h(px(28.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(palette.warning))
                    .child(t!("fileManager.disconnected").to_string())
                    .child(small_button(
                        format!("fm-reconnect-{}", pane.as_str()),
                        t!("fileManager.reconnect").to_string(),
                        cx.listener(move |this, _, window, cx| {
                            let endpoint = this.pane(pane).endpoint.clone();
                            this.dispatch_app(window, cx, move |app, window, cx| {
                                app.reconnect_file_manager_source(endpoint, window, cx)
                            });
                        }),
                    )),
            );
        }
        if let Some(error) = &state.error {
            chrome = chrome.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(palette.warning))
                    .truncate()
                    .child(error.clone()),
            );
        }
        chrome.into_any_element()
    }
}

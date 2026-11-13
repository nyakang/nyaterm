use super::small_button;
use crate::features::file_manager::pane::{PaneEndpoint, PaneId};
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::theme::ThemePalette;
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_files::file_browser::{file_browser_name, file_browser_parent};
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_ui::{NyaDropdownMenu, NyaInput, NyaMenuItem, NyaTooltip};
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
            .label(source)
            .items(items);
        let mut chrome = div().flex_none().min_w_0().flex().flex_col().child(
            div()
                .h(px(30.))
                .min_w_0()
                .flex()
                .items_center()
                .px_1()
                .child(source_menu),
        );
        let mut navigation = div()
            .h(px(28.))
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1();
        for (delta, label) in [(-1, "←"), (1, "→")] {
            navigation = navigation.child(
                small_button(
                    format!("fm-history-{}-{delta}", pane.as_str()),
                    label,
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
        if let Some(session) = endpoint.session_id().map(str::to_owned) {
            let (available, status) = self.read_app(cx, |app| {
                (
                    app.file_sudo_available(&session),
                    app.file_sudo_status(&session),
                )
            });
            if available || status.is_sudo() {
                navigation = navigation.child(
                    small_button(
                        format!("fm-sudo-{}", pane.as_str()),
                        crate::features::transfers::privilege_runtime::sudo_label(status),
                        cx.listener(move |this, _, window, cx| {
                            let session = session.clone();
                            this.dispatch_app(window, cx, move |app, window, cx| {
                                app.toggle_file_sudo(session, window, cx)
                            });
                        }),
                    )
                    .disabled(status.is_busy()),
                );
            }
        }
        let path_view = if state.path_editing {
            div()
                .h(px(24.))
                .min_w_0()
                .flex_1()
                .child(NyaInput::new(&self.inputs(pane).path))
                .into_any_element()
        } else {
            let mut crumbs = Vec::new();
            let mut current = state.path.clone();
            for _ in 0..128 {
                if current.display_path.is_empty() {
                    break;
                }
                let parent = file_browser_parent(endpoint.kind(), &current.display_path);
                let label = if parent == current.display_path {
                    current.display_path.clone()
                } else {
                    file_browser_name(endpoint.kind(), &current.display_path)
                };
                crumbs.push((label, current.clone()));
                if parent == current.display_path {
                    break;
                }
                current = if endpoint.kind()
                    == nyaterm_files::file_browser::FileBrowserBackendKind::Remote
                {
                    match current.parent() {
                        Ok(parent) => parent,
                        Err(_) => break,
                    }
                } else {
                    RemoteFilePath::new(parent)
                };
            }
            crumbs.reverse();
            let mut bar = div()
                .id(SharedString::from(format!("fm-path-{}", pane.as_str())))
                .h(px(24.))
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .on_click(
                    cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.begin_path_edit(pane, window, cx);
                        }
                    }),
                );
            for (index, (label, path)) in crumbs.into_iter().enumerate() {
                if index > 0 {
                    bar = bar.child(div().text_color(rgb(palette.text_muted)).child(" › "));
                }
                let tooltip = path.display_path.clone();
                bar = bar.child(
                    div()
                        .id(SharedString::from(format!(
                            "fm-crumb-{}-{index}",
                            pane.as_str()
                        )))
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_xs()
                        .cursor_pointer()
                        .child(label)
                        .tooltip(move |window, cx| {
                            NyaTooltip::new(tooltip.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if event.click_count() >= 2 {
                                    this.begin_path_edit(pane, window, cx);
                                } else {
                                    this.navigate_to(pane, path.clone(), window, cx);
                                }
                            },
                        )),
                );
            }
            bar.into_any_element()
        };
        navigation = navigation
            .child(path_view)
            .child(small_button(
                format!("fm-up-{}", pane.as_str()),
                "↑",
                cx.listener(move |this, _, window, cx| this.navigate_parent(pane, window, cx)),
            ))
            .child(small_button(
                format!("fm-home-{}", pane.as_str()),
                "⌂",
                cx.listener(move |this, _, window, cx| {
                    let id = this.id.clone();
                    this.focus_pane(pane, window, cx);
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        app.navigate_file_manager_home(id, pane, cx)
                    });
                }),
            ))
            .child(small_button(
                format!("fm-refresh-{}", pane.as_str()),
                "↻",
                cx.listener(move |this, _, window, cx| {
                    let id = this.id.clone();
                    this.focus_pane(pane, window, cx);
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        app.refresh_file_manager_pane(id, pane, cx)
                    });
                }),
            ))
            .child(small_button(
                format!("fm-copy-path-{}", pane.as_str()),
                "⧉",
                cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        this.pane(pane).path.display_path.clone(),
                    ));
                }),
            ));
        chrome = chrome.child(navigation).child(
            div()
                .h(px(28.))
                .min_w_0()
                .px_1()
                .child(NyaInput::new(&self.inputs(pane).search)),
        );
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

use super::icon_button;
use crate::features::file_manager::pane::{PaneId, PanePathMenu};
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::theme::ThemePalette;
use gpui::{AnyElement, Context, IntoElement, SharedString, deferred, div, prelude::*, px, rgb};
use nyaterm_files::file_browser::file_browser_name;
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_ui::file_navigation::{BreadcrumbMenu, BreadcrumbRow, PathMetrics, breadcrumb_row};
use nyaterm_ui::{NyaInputShell, NyaScrollable};
use rust_i18n::t;

use nyaterm_file_browser_gpui::path_navigation::{breadcrumbs, child_paths};

impl FileManagerWorkspace {
    pub(super) fn handle_path_menu_key(
        &mut self,
        key: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pane = self.active;
        let state = self.pane(pane);
        let Some(menu) = &state.path_menu else {
            return false;
        };
        if key == "escape" {
            self.pane_mut(pane).path_menu = None;
            self.focus_pane(pane, window, cx);
            return true;
        }
        let hidden = self.read_app(cx, |app| {
            app.settings.summary().ui_file_explorer_show_hidden_files
        });
        let favorites = matches!(menu, PanePathMenu::Favorites);
        let paths = match menu {
            PanePathMenu::Paths(paths) => paths
                .iter()
                .map(|(_, path)| path.clone())
                .collect::<Vec<_>>(),
            PanePathMenu::Children {
                result: Some(Ok(entries)),
                access_generation,
                ..
            } if *access_generation
                == state
                    .endpoint
                    .session_id()
                    .and_then(|id| self.read_app(cx, |app| app.file_access_generation(id))) =>
            {
                child_paths(entries, hidden)
                    .into_iter()
                    .map(|(_, path)| path)
                    .collect()
            }
            PanePathMenu::Favorites => self
                .read_app(cx, |app| app.file_manager_favorites(&state.endpoint))
                .into_iter()
                .map(|path| {
                    if path == state.path.display_path {
                        state.path.clone()
                    } else {
                        RemoteFilePath::new(path)
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        let count = paths.len() + usize::from(favorites);
        let index = state.menu_selection.min(count.saturating_sub(1));
        let next = match key {
            "up" => index.saturating_sub(1),
            "down" => (index + 1).min(count.saturating_sub(1)),
            "home" => 0,
            "end" => count.saturating_sub(1),
            "enter" => {
                if favorites && index == 0 {
                    let endpoint = state.endpoint.clone();
                    let path = state.path.display_path.clone();
                    let favorite = !paths.iter().any(|p| p.display_path == path);
                    self.pane_mut(pane).path_menu = None;
                    self.dispatch_app(window, cx, move |app, _, cx| {
                        app.set_file_manager_favorite(endpoint, path, favorite, cx)
                    });
                    cx.notify();
                } else if let Some(path) = paths.get(index.saturating_sub(usize::from(favorites))) {
                    self.navigate_to(pane, path.clone(), window, cx);
                }
                return true;
            }
            _ => return false,
        };
        let state = self.pane_mut(pane);
        state.menu_selection = next;
        state.menu_scroll.set_offset(gpui::point(
            px(0.),
            px(-(next.saturating_sub(4) as f32) * 28.),
        ));
        cx.notify();
        true
    }

    fn request_path_children(
        &mut self,
        pane: PaneId,
        path: RemoteFilePath,
        branch: Option<RemoteFilePath>,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.id.clone();
        let state = self.pane(pane);
        let request = crate::features::file_manager::navigation_runtime::ChildrenRequest {
            endpoint: state.endpoint.clone(),
            generation: state.generation,
            path,
            branch,
        };
        self.dispatch_app(window, cx, move |app, _, cx| {
            app.file_manager_children(id, pane, request, cx)
        });
    }

    pub(super) fn path_navigation_view(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.pane(pane);
        let metrics = PathMetrics::new(
            self.read_app(cx, |app| f32::from(app.settings.summary().ui_font_size)),
        );
        let favorites = self.read_app(cx, |app| app.file_manager_favorites(&state.endpoint));
        let favorite = favorites.contains(&state.path.display_path);
        let expanded = match &state.path_menu {
            Some(PanePathMenu::Children { path, .. }) => Some(path.clone()),
            _ => None,
        };
        let path_view = if state.path_editing {
            let field = self.inputs(pane).path.clone();
            let focus = field.read(cx).component_focus_handle(cx);
            div()
                .h(px(metrics.control_height))
                .min_w_0()
                .flex_1()
                .px_1()
                .rounded_sm()
                .bg(rgb(palette.input_surface()))
                .font_family(crate::features::shell::gpui_code_font_family())
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    window.focus(&focus, cx)
                })
                .child({
                    let enter = cx.entity();
                    let tab = cx.entity();
                    let escape = cx.entity();
                    NyaInputShell::new(format!("fm-path-input-{}", pane.as_str()), &field)
                        .bare()
                        .height(px(metrics.control_height))
                        .on_plain_enter(move |_, cx| {
                            enter.update(cx, |this, cx| this.submit_path_edit(pane, cx))
                        })
                        .on_vertical_navigation(cx.listener(move |this, down, _, cx| {
                            this.pane_mut(pane).completion.move_selection(*down);
                            cx.notify();
                        }))
                        .on_completion_tab(move |window, cx| {
                            tab.update(cx, |this, cx| {
                                let index = this
                                    .pane(pane)
                                    .completion
                                    .presentation
                                    .selected
                                    .unwrap_or(0);
                                this.accept_path_candidate(pane, index, window, cx)
                            })
                        })
                        .on_escape(move |window, cx| {
                            escape.update(cx, |this, cx| {
                                this.escape_path_completion(pane, window, cx)
                            })
                        })
                })
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .font_family(crate::features::shell::gpui_code_font_family())
                .child(breadcrumb_row(
                    BreadcrumbRow {
                        prefix: format!("fm-{}", pane.as_str()),
                        palette,
                        display_path: state.path.display_path.clone(),
                        segments: breadcrumbs(state.endpoint.kind(), &state.path),
                        expanded,
                        overflow_label: t!("fileExplorer.breadcrumbOverflow").to_string(),
                        width: (self.pane_width(pane) - 32. - metrics.control_height * 2.).max(0.),
                        metrics,
                    },
                    move |this: &mut Self, _, window, cx| this.begin_path_edit(pane, window, cx),
                    move |this, path, window, cx| this.navigate_to(pane, path, window, cx),
                    move |this, action, event, window, cx| {
                        this.focus_pane(pane, window, cx);
                        this.pane_mut(pane).menu_anchor_x = Some(event.position.x);
                        match action {
                            BreadcrumbMenu::Overflow(paths) => {
                                let kind = this.pane(pane).endpoint.kind();
                                this.pane_mut(pane).open_path_menu(PanePathMenu::Paths(
                                    paths
                                        .into_iter()
                                        .map(|path| {
                                            (file_browser_name(kind, &path.display_path), path)
                                        })
                                        .collect(),
                                ));
                                cx.notify();
                            }
                            BreadcrumbMenu::Children(path, branch) => {
                                this.request_path_children(pane, path, branch, window, cx);
                            }
                        }
                    },
                    cx,
                ))
                .into_any_element()
        };
        let row = div()
            .relative()
            .min_w_0()
            .flex_none()
            .min_h(px(metrics.row_height()))
            .px_2()
            .py(px(2.))
            .border_b_1()
            .border_color(rgb(palette.border))
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(path_view)
                    .when(!state.path_editing, |row| {
                        row.child(
                            nyaterm_ui::file_navigation::path_icon(
                                palette,
                                format!("fm-path-edit-{}", pane.as_str()),
                                "icons/edit.svg",
                                t!("fileExplorer.editPath"),
                                metrics.control_height,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.begin_path_edit(pane, window, cx);
                                },
                            )),
                        )
                    })
                    .child(
                        nyaterm_ui::file_navigation::path_icon(
                            palette,
                            format!("fm-favorites-{}", pane.as_str()),
                            if favorite {
                                "icons/fe/star.svg"
                            } else {
                                "icons/fe/star-outline.svg"
                            },
                            t!("fileExplorer.favorites"),
                            metrics.control_height,
                            favorite,
                        )
                        .ml_1()
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.focus_pane(pane, window, cx);
                                this.pane_mut(pane).menu_anchor_x = Some(event.position().x);
                                this.pane_mut(pane).open_path_menu(PanePathMenu::Favorites);
                                cx.notify();
                            },
                        )),
                    ),
            );
        let recent = (state.path_editing && !state.completion.edited).then(|| {
            state
                .recent_paths
                .iter()
                .take(5)
                .map(|path| (path.display_path.clone(), path.clone()))
                .collect::<Vec<_>>()
        });
        let favorite_width = favorites
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(20);
        let label_chars = match &state.path_menu {
            Some(PanePathMenu::Paths(paths)) => paths
                .iter()
                .map(|(label, _)| label.chars().count())
                .max()
                .unwrap_or(20),
            Some(PanePathMenu::Children {
                result: Some(Ok(entries)),
                ..
            }) => entries
                .iter()
                .filter(|entry| entry.is_directory())
                .map(|entry| entry.name.chars().count())
                .max()
                .unwrap_or(20),
            Some(PanePathMenu::Favorites) => favorite_width,
            _ => 24,
        };
        let pane_width = (self.pane_width(pane) - 2.).max(0.);
        let popup_width = if state.path_editing {
            pane_width
        } else {
            (label_chars as f32 * metrics.font_size * 0.6 + 48.)
                .clamp(160., 360.)
                .min(pane_width)
        };
        let pane_origin = self.pane_bounds[if pane == PaneId::Left { 0 } else { 1 }]
            .map(|bounds| f32::from(bounds.origin.x))
            .unwrap_or(0.);
        let popup_left = if state.path_editing {
            0.
        } else {
            state
                .menu_anchor_x
                .map(|x| f32::from(x) - pane_origin)
                .unwrap_or(0.)
                .clamp(0., (pane_width - popup_width).max(0.))
        };
        let completing = state.path_editing && state.completion.presentation.visible();
        let popup = if completing {
            let entity = cx.entity();
            Some(
                nyaterm_file_browser_gpui::completion::path_candidates(
                    format!("fm-path-completion-{}", pane.as_str()),
                    &state.completion.presentation,
                    palette,
                    &state.completion.scroll,
                    self.pane_bounds[if pane == PaneId::Left { 0 } else { 1 }]
                        .map(|b| (f32::from(b.size.height) - 130.).clamp(28., 224.))
                        .unwrap_or(224.),
                    move |index, expected_path, window, cx| {
                        entity.update(cx, |this, cx| {
                            if this
                                .pane(pane)
                                .completion
                                .presentation
                                .candidates
                                .get(index)
                                .is_some_and(|c| &c.path == expected_path)
                            {
                                this.accept_path_candidate(pane, index, window, cx);
                            }
                        });
                    },
                )
                .into_any_element(),
            )
        } else {
            match &state.path_menu {
                Some(PanePathMenu::Paths(paths)) => {
                    Some(self.path_entries(palette, pane, paths.clone(), None, cx))
                }
                Some(PanePathMenu::Children {
                    path,
                    branch,
                    result,
                    access_generation,
                    ..
                }) => {
                    let current_access = state
                        .endpoint
                        .session_id()
                        .and_then(|id| self.read_app(cx, |app| app.file_access_generation(id)));
                    if current_access != *access_generation {
                        None
                    } else {
                        Some(match result {
                            None => div()
                                .px_2()
                                .py_1()
                                .child(t!("fileExplorer.loadingChildDirectories").to_string())
                                .into_any_element(),
                            Some(Ok(entries)) => {
                                let hidden = self.read_app(cx, |app| {
                                    app.settings.summary().ui_file_explorer_show_hidden_files
                                });
                                let entries = child_paths(entries, hidden);
                                if entries.is_empty() {
                                    div()
                                        .px_2()
                                        .py_1()
                                        .child(t!("fileExplorer.noChildDirectories").to_string())
                                        .into_any_element()
                                } else {
                                    self.path_entries(palette, pane, entries, branch.as_ref(), cx)
                                }
                            }
                            Some(Err(_)) => {
                                let path = path.clone();
                                let branch = branch.clone();
                                div()
                                    .px_2()
                                    .py_1()
                                    .flex()
                                    .flex_col()
                                    .child(t!("fileExplorer.childDirectoriesFailed").to_string())
                                    .child(super::small_button(
                                        format!("fm-children-retry-{}", pane.as_str()),
                                        t!("common.retry").to_string(),
                                        cx.listener(move |this, _, window, cx| {
                                            this.request_path_children(
                                                pane,
                                                path.clone(),
                                                branch.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                    ))
                                    .into_any_element()
                            }
                        })
                    }
                }
                Some(PanePathMenu::Favorites) => {
                    Some(self.favorites_menu(palette, pane, favorites, cx))
                }
                None => recent
                    .filter(|paths| !paths.is_empty())
                    .map(|paths| self.path_entries(palette, pane, paths, None, cx)),
            }
        };
        let max_popup_height = self.pane_bounds[if pane == PaneId::Left { 0 } else { 1 }]
            .map(|bounds| (f32::from(bounds.size.height) - 130.).clamp(28., 324.))
            .unwrap_or(324.);
        row.when(state.path_editing || state.path_menu.is_some(), |row| {
            row.child(
                deferred(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(popup_left))
                        .w(px(popup_width))
                        .pt(px(metrics.row_height()))
                        .flex()
                        .flex_col()
                        .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                            this.pane_mut(pane).completion.reset();
                            this.pane_mut(pane).path_editing = false;
                            this.pane_mut(pane).path_menu = None;
                            cx.notify();
                        }))
                        .when_some(popup, |shell, popup| {
                            if completing {
                                return shell.child(popup);
                            }
                            shell.child(
                                div()
                                    .min_w_0()
                                    .w_full()
                                    .occlude()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .rounded_sm()
                                    .bg(rgb(palette.surface))
                                    .shadow_md()
                                    .vertical_scrollbar(&state.menu_scroll)
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "fm-path-popup-{}",
                                                pane.as_str()
                                            )))
                                            .max_h(px(max_popup_height))
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .track_scroll(&state.menu_scroll)
                                            .child(popup),
                                    ),
                            )
                        }),
                )
                .with_priority(2),
            )
        })
        .into_any_element()
    }

    fn path_entries(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        paths: Vec<(String, RemoteFilePath)>,
        branch: Option<&RemoteFilePath>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_height = if self.pane(pane).path_editing {
            PathMetrics::new(
                self.read_app(cx, |app| f32::from(app.settings.summary().ui_font_size)),
            )
            .control_height
        } else {
            28.
        };
        let mut list = div().flex().flex_col();
        for (i, (label, path)) in paths.into_iter().enumerate() {
            let selected = branch == Some(&path);
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "fm-path-item-{}-{i}",
                        pane.as_str()
                    )))
                    .debug_selector(move || format!("fm-path-item-{}-{i}", pane.as_str()))
                    .h(px(row_height))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(
                        self.pane(pane).path_menu.is_some() && self.pane(pane).menu_selection == i,
                        |row| row.bg(rgb(palette.hover)),
                    )
                    .hover(move |row| row.bg(rgb(palette.hover)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(label.replace(['\r', '\n'], " ")),
                    )
                    .when(selected, |row| {
                        row.child(
                            gpui::svg()
                                .size(px(13.))
                                .path("icons/check.svg")
                                .text_color(rgb(palette.link)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.navigate_to(pane, path.clone(), window, cx);
                    })),
            );
        }
        list.into_any_element()
    }

    fn favorites_menu(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        favorites: Vec<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.pane(pane);
        let endpoint = state.endpoint.clone();
        let path = state.path.display_path.clone();
        let favorite = favorites.contains(&path);
        let mut list = div().flex().flex_col().child(super::small_button(
            format!("fm-favorite-current-{}", pane.as_str()),
            t!(if favorite {
                "fileExplorer.removeFavorite"
            } else {
                "fileExplorer.addCurrentDirToFavorites"
            })
            .to_string(),
            cx.listener(move |this, _, window, cx| {
                let endpoint = endpoint.clone();
                let path = path.clone();
                this.pane_mut(pane).path_menu = None;
                this.dispatch_app(window, cx, move |app, _, cx| {
                    app.set_file_manager_favorite(endpoint, path, !favorite, cx)
                });
            }),
        ));
        for (i, path) in favorites.into_iter().enumerate() {
            let endpoint = state.endpoint.clone();
            let remove_path = path.clone();
            let target = if path == state.path.display_path {
                state.path.clone()
            } else {
                RemoteFilePath::new(path.clone())
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "fm-favorite-path-{}-{i}",
                                pane.as_str()
                            )))
                            .min_w_0()
                            .flex_1()
                            .h(px(28.))
                            .px_2()
                            .truncate()
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(palette.hover)))
                            .child(path.replace(['\r', '\n'], " "))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate_to(pane, target.clone(), window, cx)
                            })),
                    )
                    .child(icon_button(
                        palette,
                        format!("fm-favorite-remove-{}-{i}", pane.as_str()),
                        "icons/window/close.svg",
                        t!("fileExplorer.removeFavorite"),
                        cx.listener(move |this, _, window, cx| {
                            let endpoint = endpoint.clone();
                            let path = remove_path.clone();
                            this.dispatch_app(window, cx, move |app, _, cx| {
                                app.set_file_manager_favorite(endpoint, path, false, cx)
                            });
                        }),
                    )),
            );
        }
        list.into_any_element()
    }
}

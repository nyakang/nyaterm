use rust_i18n::t;

use gpui::{
    ClipboardItem, Context, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, SharedString,
    StatefulInteractiveElement as _, Window, div, prelude::*, px, rgb, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_transport::SftpFileEntry;
use nyaterm_ui::{NyaInput, NyaScrollable};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{
    TransferBrowserBreadcrumbSegment, TransferBrowserChildrenMenuStatus,
    TransferBrowserPathMenuKind, TransferBrowserPathMenuState,
};

use super::helpers::{normalized_transfer_browser_path, remote_child_path};
use super::transfer_menu_position;
use crate::features::transfers::natural_compare_ascii;

use super::panel::TransferPanel;

/// The browser path bar.
///
/// A free function over the snapshot: it is part of the panel body, so it must not
/// reach the app during a draw. Its handlers still do, at event time.
pub(in crate::features::pages::transfers) fn transfer_browser_path_row(
    panel: &TransferPanel,
    current_browser_path: String,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let snapshot = panel
        .snapshot()
        .expect("the caller returns early without a snapshot");
    let chrome = snapshot.chrome;
    let browser = &snapshot.browser;
    let display_browser_path =
        display_transfer_browser_home_path(&current_browser_path, &browser.home_dir);
    let is_current_favorite = browser
        .favorites
        .iter()
        .any(|path| path == &current_browser_path);
    let history_paths = browser
        .visited_history
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>();
    let palette = chrome.palette;
    // Built by `begin_transfer_browser_path_edit`, which is the only thing that
    // can set `path_editing`. Render only reads it.
    let path_input = browser
        .path_editing
        .then(|| browser.path_field.clone())
        .flatten()
        .map(|field| {
            let focus = field.read(cx).focus_handle();
            div()
                .id("transfer-path-bar-input-shell")
                .h(px(20.))
                .min_w_0()
                .flex_1()
                .px_1()
                .flex()
                .items_center()
                .rounded_sm()
                .bg(rgb(palette.input))
                .cursor_text()
                .on_click(move |_, window, cx| {
                    window.focus(&focus, cx);
                })
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text))
                        .child(NyaInput::new(&field)),
                )
                .into_any_element()
        });
    let breadcrumbs = build_transfer_browser_breadcrumbs(&current_browser_path, &browser.home_dir);
    let (visible_breadcrumbs, overflow_breadcrumbs) =
        collapse_transfer_browser_breadcrumbs(&breadcrumbs);

    // Tauri FileExplorerPathBar: minHeight ~26px, mono path, favorites on the right.
    div()
        .flex()
        .flex_col()
        .gap_0()
        .min_h(px(26.))
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(chrome.transparent_surface)
        .px_2()
        .py(px(2.))
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .when(browser.path_editing, |this| {
                    this.child(
                        div()
                            .id(SharedString::from("transfer-browser-path-input"))
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .font_family(crate::features::shell::gpui_code_font_family())
                            .text_size(px(10.))
                            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.mark_user_activity();
                                    match event.keystroke.key.as_str() {
                                        "enter" => {
                                            cx.stop_propagation();
                                            this.submit_transfer_browser_path_edit(window, cx);
                                        }
                                        "escape" => {
                                            cx.stop_propagation();
                                            this.cancel_transfer_browser_path_edit(cx);
                                        }
                                        _ => {}
                                    }
                                })
                            }))
                            .children(path_input),
                    )
                })
                .when(!browser.path_editing, |this| {
                    this.child(transfer_browser_breadcrumb_row(
                        TransferBrowserBreadcrumbRowPresentation {
                            palette,
                            display_path: display_browser_path.clone(),
                            current_path: current_browser_path.clone(),
                            all_segments: breadcrumbs.clone(),
                            visible_segments: visible_breadcrumbs.clone(),
                            overflow_segments: overflow_breadcrumbs.clone(),
                            overflow_label: t!("fileExplorer.breadcrumbOverflow").to_string(),
                        },
                        cx,
                    ))
                })
                .child(
                    div()
                        .id(SharedString::from("transfer-browser-path-favorite"))
                        .ml_1()
                        .size(px(22.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_sm()
                        .text_color(if is_current_favorite {
                            rgb(palette.link)
                        } else {
                            rgb(palette.text_muted)
                        })
                        .cursor_pointer()
                        .hover(|this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .tooltip({
                            let label = t!("fileExplorer.favorites").to_string();
                            move |window, cx| {
                                nyaterm_ui::NyaTooltip::new(label.clone()).build(window, cx)
                            }
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|panel, event: &MouseDownEvent, _window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    cx.stop_propagation();
                                    this.open_transfer_browser_favorites_menu(event, cx);
                                })
                            }),
                        )
                        .child(
                            svg()
                                .size(px(14.))
                                .flex_none()
                                .path(if is_current_favorite {
                                    "icons/fe/star.svg"
                                } else {
                                    "icons/fe/star-outline.svg"
                                })
                                .text_color(if is_current_favorite {
                                    rgb(palette.link)
                                } else {
                                    rgb(palette.text_muted)
                                }),
                        ),
                ),
        )
        .when(browser.path_editing && !history_paths.is_empty(), |this| {
            this.child(transfer_browser_path_history_list(
                palette,
                chrome.surface,
                current_browser_path,
                browser.home_dir.clone(),
                history_paths,
                cx,
            ))
        })
}

impl NyaTermApp {
    pub(super) fn copy_current_transfer_browser_path(&mut self, cx: &mut Context<Self>) {
        let path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
        self.shell
            .set_status("copied current remote directory".to_string());
        self.transfer
            .set_browser_status(truncate_preview(&path, 92));
        cx.notify();
    }

    pub(in crate::features) fn close_transfer_browser_path_menu(&mut self, cx: &mut Context<Self>) {
        self.transfer.close_browser_path_menu();
        cx.notify();
    }

    fn open_transfer_browser_breadcrumb_overflow(
        &mut self,
        segments: Vec<TransferBrowserBreadcrumbSegment>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .open_browser_path_menu(TransferBrowserPathMenuState {
                session_id: self.session.active_id_owned(),
                x: event.position.x,
                y: event.position.y,
                kind: TransferBrowserPathMenuKind::Overflow { segments },
            });
        cx.notify();
    }

    fn open_transfer_browser_children_menu(
        &mut self,
        path: String,
        branch_child_path: Option<String>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(&path);
        let current_path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        let status = if path == current_path {
            TransferBrowserChildrenMenuStatus::Ready(transfer_browser_child_directories(
                self.transfer.browser_view().entries,
                self.settings.summary().ui_file_explorer_show_hidden_files,
            ))
        } else {
            TransferBrowserChildrenMenuStatus::Loading
        };
        self.transfer
            .open_browser_path_menu(TransferBrowserPathMenuState {
                session_id: self.session.active_id_owned(),
                x: event.position.x,
                y: event.position.y,
                kind: TransferBrowserPathMenuKind::Children {
                    path: path.clone(),
                    branch_child_path,
                    request_id: None,
                    status,
                },
            });
        if path != current_path {
            self.start_transfer_browser_children_job(path, cx);
        } else {
            cx.notify();
        }
    }

    fn retry_transfer_browser_children_menu(&mut self, cx: &mut Context<Self>) {
        let path = self
            .transfer
            .browser_view()
            .path_menu
            .as_ref()
            .and_then(|menu| match &menu.kind {
                TransferBrowserPathMenuKind::Children { path, .. } => Some(path.clone()),
                TransferBrowserPathMenuKind::Overflow { .. } => None,
            });
        if let Some(path) = path {
            self.start_transfer_browser_children_job(path, cx);
        }
    }

    pub(in crate::features) fn transfer_browser_path_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let menu = self.transfer.browser_view().path_menu.clone().unwrap_or(
            TransferBrowserPathMenuState {
                session_id: None,
                x: px(8.),
                y: px(8.),
                kind: TransferBrowserPathMenuKind::Overflow {
                    segments: Vec::new(),
                },
            },
        );
        let preferred_height = match &menu.kind {
            TransferBrowserPathMenuKind::Overflow { segments } => 16. + segments.len() as f32 * 28.,
            TransferBrowserPathMenuKind::Children { status, .. } => match status {
                TransferBrowserChildrenMenuStatus::Ready(entries) => {
                    16. + entries.len().min(11) as f32 * 28.
                }
                TransferBrowserChildrenMenuStatus::Loading => 56.,
                TransferBrowserChildrenMenuStatus::Error(_) => 112.,
            },
        };
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (menu_x, menu_y, menu_max_height) = transfer_menu_position(
            f32::from(menu.x),
            f32::from(menu.y),
            280.,
            preferred_height,
            viewport_w,
            viewport_h,
        );

        let content = match menu.kind {
            TransferBrowserPathMenuKind::Overflow { segments } => {
                transfer_browser_path_menu_entries(palette, segments, None, cx).into_any_element()
            }
            TransferBrowserPathMenuKind::Children {
                branch_child_path,
                status,
                ..
            } => match status {
                TransferBrowserChildrenMenuStatus::Loading => div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("fileExplorer.loadingChildDirectories"))
                    .into_any_element(),
                TransferBrowserChildrenMenuStatus::Error(error) => div()
                    .px_2()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.danger))
                            .child(t!("fileExplorer.childDirectoriesFailed")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(truncate_preview(&error, 120)),
                    )
                    .child(
                        div()
                            .id(SharedString::from("transfer-browser-path-menu-retry"))
                            .h(px(26.))
                            .px_2()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(rgb(palette.link))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(palette.hover)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.retry_transfer_browser_children_menu(cx);
                                this.defer_transfer_panel_snapshot_flush(cx);
                            }))
                            .child(t!("common.retry")),
                    )
                    .into_any_element(),
                TransferBrowserChildrenMenuStatus::Ready(entries) if entries.is_empty() => div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("fileExplorer.noChildDirectories"))
                    .into_any_element(),
                TransferBrowserChildrenMenuStatus::Ready(entries) => {
                    let segments = entries
                        .into_iter()
                        .map(|entry| TransferBrowserBreadcrumbSegment {
                            label: entry.name,
                            path: entry.path,
                        })
                        .collect();
                    transfer_browser_path_menu_entries(
                        palette,
                        segments,
                        branch_child_path.as_deref(),
                        cx,
                    )
                    .into_any_element()
                }
            },
        };

        div()
            .id(SharedString::from("transfer-browser-path-menu-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_transfer_browser_path_menu(cx);
            }))
            .child(
                div()
                    .id(SharedString::from("transfer-browser-path-menu"))
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(280.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .max_h(px(menu_max_height.min(324.)))
                            .overflow_y_scrollbar()
                            .p_1()
                            .child(content),
                    ),
            )
    }

    pub(super) fn send_current_transfer_browser_path_to_terminal(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id().is_none() {
            self.shell
                .set_status("start a session before sending remote path".to_string());
            cx.notify();
            return;
        }
        let path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        if self.send_terminal_input(path.clone().into_bytes(), cx) {
            self.shell
                .set_status("sent current remote directory to terminal".to_string());
            self.transfer
                .set_browser_status(truncate_preview(&path, 92));
            cx.notify();
        }
    }

    pub(super) fn begin_transfer_browser_path_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        self.transfer.begin_browser_path_edit(path);
        self.forget_text_inputs("transfer.self.transfer.browser_view().path");
        self.start_transfer_browser_home_dir_job(cx);
        let field = self.text_input(
            "transfer.self.transfer.browser_view().path",
            &self.transfer.browser_view().path_draft.clone(),
            TextInputSetup::placeholder(t!("fileExplorer.editPath")),
            cx,
        );
        window.focus(&field.read(cx).focus_handle(), cx);
        field.update(cx, |field, cx| field.select_all(window, cx));
        cx.notify();
    }

    pub(super) fn cancel_transfer_browser_path_edit(&mut self, cx: &mut Context<Self>) {
        self.transfer.cancel_browser_path_edit();
        self.forget_text_inputs("transfer.self.transfer.browser_view().path");
        cx.notify();
    }

    pub(in crate::features) fn apply_transfer_browser_path_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.transfer.browser_view().path_editing {
            return;
        }
        self.mark_user_activity();
        self.transfer.update_browser_path_draft(text);
        cx.notify();
    }

    pub(super) fn submit_transfer_browser_path_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = expand_transfer_browser_home_path(
            self.transfer.browser_view().path_draft,
            self.transfer.browser_view().home_dir,
        );
        if path.is_empty() {
            self.transfer
                .set_browser_status("enter a remote directory path");
            cx.notify();
            return;
        }
        if path == "~" || path.starts_with("~/") {
            let status = if self.transfer.browser_view().home_dir_pending {
                "remote home is still resolving".to_string()
            } else {
                "remote home is unavailable for this session".to_string()
            };
            self.transfer.set_browser_status(status);
            cx.notify();
            return;
        }
        self.transfer.finish_browser_path_edit();
        self.forget_text_inputs("transfer.self.transfer.browser_view().path");
        self.open_transfer_browser_directory(path, window, cx);
    }
}

fn transfer_browser_breadcrumb_row(
    presentation: TransferBrowserBreadcrumbRowPresentation,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let TransferBrowserBreadcrumbRowPresentation {
        palette,
        display_path,
        current_path,
        all_segments,
        visible_segments,
        overflow_segments,
        overflow_label,
    } = presentation;
    let mut row = div()
        .id(SharedString::from("transfer-browser-path-display"))
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .overflow_hidden()
        .font_family(crate::features::shell::gpui_code_font_family())
        .text_size(px(10.))
        .tooltip(move |window, cx| {
            nyaterm_ui::NyaTooltip::new(display_path.clone()).build(window, cx)
        });

    if !overflow_segments.is_empty() {
        let open_segments = overflow_segments.clone();
        row = row.child(
            div()
                .id(SharedString::from("transfer-browser-breadcrumb-overflow"))
                .size(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .text_color(rgb(palette.text_muted))
                .cursor_pointer()
                .hover(|this| {
                    this.bg(rgb(palette.surface_elevated))
                        .text_color(rgb(palette.text))
                })
                .tooltip(move |window, cx| {
                    nyaterm_ui::NyaTooltip::new(overflow_label.clone()).build(window, cx)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                        panel.with_app(cx, |this, cx| {
                            cx.stop_propagation();
                            this.open_transfer_browser_breadcrumb_overflow(
                                open_segments.clone(),
                                event,
                                cx,
                            );
                        })
                    }),
                )
                .child(
                    svg()
                        .size(px(14.))
                        .path("icons/session/more.svg")
                        .text_color(rgb(palette.text_muted)),
                ),
        );
    }

    for segment in visible_segments {
        let is_current = segment.path == current_path;
        let branch_child_path = all_segments
            .iter()
            .position(|candidate| candidate.path == segment.path)
            .and_then(|index| all_segments.get(index + 1))
            .map(|candidate| candidate.path.clone());
        let label_path = segment.path.clone();
        let children_path = segment.path.clone();
        let children_branch = branch_child_path.clone();
        let children_tooltip =
            t!("fileExplorer.showChildDirectories", path = segment.path).to_string();
        row = row.child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "transfer-browser-breadcrumb-label-{}",
                            segment.path
                        )))
                        .h(px(20.))
                        .max_w(px(128.))
                        .px_1()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .rounded_l_sm()
                        .text_color(if is_current {
                            rgb(palette.text)
                        } else {
                            rgb(palette.text_muted)
                        })
                        .cursor_pointer()
                        .hover(|this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .on_click(cx.listener(move |panel, _, window, cx| {
                            panel.with_app(cx, |this, cx| {
                                if is_current {
                                    this.begin_transfer_browser_path_edit(window, cx);
                                } else {
                                    this.open_transfer_browser_directory(
                                        label_path.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            })
                        }))
                        .child(truncate_preview(&segment.label, 18)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "transfer-browser-breadcrumb-children-{}",
                            segment.path
                        )))
                        .h(px(20.))
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_r_sm()
                        .text_color(rgb(palette.text_muted))
                        .cursor_pointer()
                        .hover(|this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .tooltip(move |window, cx| {
                            nyaterm_ui::NyaTooltip::new(children_tooltip.clone()).build(window, cx)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                                panel.with_app(cx, |this, cx| {
                                    cx.stop_propagation();
                                    this.open_transfer_browser_children_menu(
                                        children_path.clone(),
                                        children_branch.clone(),
                                        event,
                                        cx,
                                    );
                                })
                            }),
                        )
                        .child(
                            svg()
                                .size(px(11.))
                                .path("icons/chevron-down.svg")
                                .text_color(rgb(palette.text_muted)),
                        ),
                ),
        );
    }
    row
}

struct TransferBrowserBreadcrumbRowPresentation {
    palette: crate::theme::ThemePalette,
    display_path: String,
    current_path: String,
    all_segments: Vec<TransferBrowserBreadcrumbSegment>,
    visible_segments: Vec<TransferBrowserBreadcrumbSegment>,
    overflow_segments: Vec<TransferBrowserBreadcrumbSegment>,
    overflow_label: String,
}

fn transfer_browser_path_menu_entries(
    palette: crate::theme::ThemePalette,
    segments: Vec<TransferBrowserBreadcrumbSegment>,
    branch_child_path: Option<&str>,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let mut list = div().flex().flex_col();
    for segment in segments {
        let is_branch = branch_child_path == Some(segment.path.as_str());
        let open_path = segment.path.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!(
                    "transfer-browser-path-menu-entry-{}",
                    segment.path
                )))
                .h(px(28.))
                .w_full()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(palette.text))
                .cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_transfer_browser_path_menu(cx);
                    this.open_transfer_browser_directory(open_path.clone(), window, cx);
                    this.defer_transfer_panel_snapshot_flush(cx);
                }))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .child(truncate_preview(&segment.label, 44)),
                )
                .when(is_branch, |this| {
                    this.child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/check.svg")
                            .text_color(rgb(palette.link)),
                    )
                }),
        );
    }
    list
}

fn transfer_browser_child_directories(
    entries: &[SftpFileEntry],
    show_hidden_files: bool,
) -> Vec<SftpFileEntry> {
    let mut directories = entries
        .iter()
        .filter(|entry| {
            entry.is_directory()
                && entry.name != "."
                && entry.name != ".."
                && (show_hidden_files || !entry.name.starts_with('.'))
        })
        .cloned()
        .collect::<Vec<_>>();
    directories.sort_by(|left, right| {
        natural_compare_ascii(&left.name, &right.name).then_with(|| left.name.cmp(&right.name))
    });
    directories
}

fn build_transfer_browser_breadcrumbs(
    current_path: &str,
    home_dir: &str,
) -> Vec<TransferBrowserBreadcrumbSegment> {
    let current_path = normalized_transfer_browser_path(current_path);
    if current_path.contains('\\') || current_path.as_bytes().get(1) == Some(&b':') {
        let mut path = std::path::PathBuf::new();
        let mut segments = Vec::new();
        for component in std::path::Path::new(&current_path).components() {
            path.push(component.as_os_str());
            match component {
                std::path::Component::RootDir => {
                    segments.push(TransferBrowserBreadcrumbSegment {
                        label: path.to_string_lossy().into_owned(),
                        path: path.to_string_lossy().into_owned(),
                    });
                }
                std::path::Component::Normal(name) => {
                    segments.push(TransferBrowserBreadcrumbSegment {
                        label: name.to_string_lossy().into_owned(),
                        path: path.to_string_lossy().into_owned(),
                    });
                }
                std::path::Component::Prefix(_) => {}
                std::path::Component::CurDir | std::path::Component::ParentDir => {}
            }
        }
        if segments.is_empty() {
            segments.push(TransferBrowserBreadcrumbSegment {
                label: current_path.clone(),
                path: current_path,
            });
        }
        return segments;
    }
    if current_path == "." || !current_path.starts_with('/') {
        let mut path = String::new();
        return current_path
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| {
                path = remote_child_path(&path, part);
                TransferBrowserBreadcrumbSegment {
                    label: part.to_string(),
                    path: path.clone(),
                }
            })
            .collect();
    }

    let home_dir = normalized_transfer_browser_path(home_dir);
    let use_home = home_dir.starts_with('/')
        && (current_path == home_dir
            || current_path
                .strip_prefix(&home_dir)
                .is_some_and(|suffix| suffix.starts_with('/')));
    let root_path = if use_home { home_dir.as_str() } else { "/" };
    let mut segments = vec![TransferBrowserBreadcrumbSegment {
        label: if use_home { "~" } else { "/" }.to_string(),
        path: root_path.to_string(),
    }];
    let suffix = current_path
        .strip_prefix(root_path)
        .unwrap_or(current_path.as_str());
    let mut path = root_path.to_string();
    for part in suffix.split('/').filter(|part| !part.is_empty()) {
        path = remote_child_path(&path, part);
        segments.push(TransferBrowserBreadcrumbSegment {
            label: part.to_string(),
            path: path.clone(),
        });
    }
    segments
}

fn collapse_transfer_browser_breadcrumbs(
    segments: &[TransferBrowserBreadcrumbSegment],
) -> (
    Vec<TransferBrowserBreadcrumbSegment>,
    Vec<TransferBrowserBreadcrumbSegment>,
) {
    if segments.len() <= 4 {
        return (segments.to_vec(), Vec::new());
    }
    let split = segments.len() - 2;
    let mut visible = Vec::with_capacity(3);
    visible.push(segments[0].clone());
    visible.extend_from_slice(&segments[split..]);
    (visible, segments[1..split].to_vec())
}

fn transfer_browser_path_history_list(
    palette: crate::theme::ThemePalette,
    popup_bg: gpui::Rgba,
    current_browser_path: String,
    home_dir: String,
    paths: Vec<String>,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let mut list = div()
        .id(SharedString::from("transfer-browser-path-history-list"))
        .mt(px(1.))
        .max_h(px(120.))
        .overflow_scrollbar()
        .rounded_b_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(popup_bg)
        .shadow_lg()
        .flex()
        .flex_col();

    for path in paths {
        let is_current = path == current_browser_path;
        let display_path = display_transfer_browser_home_path(&path, &home_dir);
        let open_path = path.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!(
                    "transfer-browser-path-history-{path}"
                )))
                .h(px(24.))
                .w_full()
                .px_2()
                .flex()
                .items_center()
                .font_family(crate::features::shell::gpui_code_font_family())
                .text_size(px(10.))
                .text_color(if is_current {
                    rgb(palette.link)
                } else {
                    rgb(palette.text)
                })
                .cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click(cx.listener(move |panel, _, window, cx| {
                    panel.with_app(cx, |this, cx| {
                        this.transfer.dismiss_browser_path_edit();
                        this.open_transfer_browser_directory(open_path.clone(), window, cx);
                    })
                }))
                .child(truncate_preview(&display_path, 72)),
        );
    }

    list
}

fn display_transfer_browser_home_path(path: &str, home_dir: &str) -> String {
    let path = normalized_transfer_browser_path(path);
    let home_dir = normalized_transfer_browser_path(home_dir);
    if home_dir.is_empty() || home_dir == "." {
        return path;
    }
    if path == home_dir {
        return "~".to_string();
    }
    for separator in ['/', '\\'] {
        let home_prefix = format!("{home_dir}{separator}");
        if let Some(suffix) = path.strip_prefix(&home_prefix) {
            return format!("~{separator}{suffix}");
        }
    }
    path
}

fn expand_transfer_browser_home_path(path: &str, home_dir: &str) -> String {
    let trimmed = path.trim();
    let home_dir = normalized_transfer_browser_path(home_dir);
    if home_dir.is_empty() || home_dir == "." {
        return normalized_transfer_browser_path(trimmed);
    }
    if trimmed == "~" {
        return home_dir;
    }
    if let Some(suffix) = trimmed.strip_prefix("~/") {
        return normalized_transfer_browser_path(&remote_child_path(&home_dir, suffix));
    }
    if let Some(suffix) = trimmed.strip_prefix("~\\") {
        return normalized_transfer_browser_path(&remote_child_path(&home_dir, suffix));
    }
    normalized_transfer_browser_path(trimmed)
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::{SftpFileEntry, SftpFileType};

    use super::{
        build_transfer_browser_breadcrumbs, collapse_transfer_browser_breadcrumbs,
        transfer_browser_child_directories,
    };

    fn entry(name: &str, path: &str, file_type: SftpFileType) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: path.to_string(),
            file_type,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    #[test]
    fn breadcrumbs_use_home_as_root_for_descendants() {
        let segments = build_transfer_browser_breadcrumbs("/home/nya/work/src", "/home/nya");
        let labels = segments
            .iter()
            .map(|segment| segment.label.as_str())
            .collect::<Vec<_>>();
        let paths = segments
            .iter()
            .map(|segment| segment.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["~", "work", "src"]);
        assert_eq!(
            paths,
            vec!["/home/nya", "/home/nya/work", "/home/nya/work/src"]
        );
    }

    #[test]
    fn breadcrumbs_use_filesystem_root_outside_home() {
        let segments = build_transfer_browser_breadcrumbs("/var/log", "/home/nya");
        let values = segments
            .into_iter()
            .map(|segment| (segment.label, segment.path))
            .collect::<Vec<_>>();

        assert_eq!(
            values,
            vec![
                ("/".to_string(), "/".to_string()),
                ("var".to_string(), "/var".to_string()),
                ("log".to_string(), "/var/log".to_string()),
            ]
        );
    }

    #[test]
    fn long_breadcrumbs_keep_root_and_last_two_segments_visible() {
        let segments = build_transfer_browser_breadcrumbs("/a/b/c/d/e", "");
        let (visible, overflow) = collapse_transfer_browser_breadcrumbs(&segments);

        assert_eq!(
            visible
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["/", "d", "e"]
        );
        assert_eq!(
            overflow
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn child_directory_menu_filters_files_and_hidden_entries() {
        let entries = vec![
            entry("dir10", "/dir10", SftpFileType::Directory),
            entry("file", "/file", SftpFileType::File),
            entry(".hidden", "/.hidden", SftpFileType::Directory),
            entry("dir2", "/dir2", SftpFileType::Directory),
            entry("..", "/..", SftpFileType::Directory),
        ];

        let visible = transfer_browser_child_directories(&entries, false);
        assert_eq!(
            visible
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["dir2", "dir10"]
        );

        let with_hidden = transfer_browser_child_directories(&entries, true);
        assert_eq!(
            with_hidden
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden", "dir2", "dir10"]
        );
    }
}

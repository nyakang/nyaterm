//! The read-only remote file preview window.
//!
//! A `WindowKind::Normal` document window (1080x760) that shows one preview tab
//! at a time with a tab strip, a toolbar, and a status bar. It mirrors the
//! editor window's open/activate/close plumbing but renders purely from a
//! snapshot of the preview workspace, so the draw path never blocks or mutates:
//! every control routes back through `self.app.update`.
//!
//! The one piece of view-owned state is the read-only text surface for text and
//! JSON previews. It is a `RemoteTextEditor` in read-only mode (the shared
//! selectable/copyable surface), kept as an entity across frames and reconciled
//! to the active tab in render, because GPUI entities must not be constructed in
//! a render path on every frame. Delimited previews use a virtualized
//! `uniform_list` so a large grid is never materialized into thousands of
//! elements per frame.

use rust_i18n::t;

use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, ListSizingBehavior, Render,
    SharedString, Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, rgb,
    svg, uniform_list,
};
use nyaterm_ui::{NyaScrollable, NyaTooltip, NyaWindowHandle, activate_child_window, nya_root};

use crate::features::transfers::RemoteTextEditor;
use crate::features::view_widgets::markdown_content_view;
use crate::features::{
    NyaTermApp,
    view_widgets::{
        ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
        child_window_options, child_window_root, focus_child_window_shell_if_idle,
    },
};
use crate::models::{DelimitedSort, PreviewContent, PreviewDelimited, TransferPreviewState};
use crate::theme::ThemePalette;

/// PDF-specific zoom bounds (50%–300%), narrower than the image bounds.
const PDF_MIN_ZOOM: f32 = 0.5;
const PDF_MAX_ZOOM: f32 = 3.0;

pub(super) struct RemoteFilePreviewWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    /// Read-only selectable text surface, reconciled to the active tab in
    /// render. `None` until the first text/JSON tab is shown.
    text_surface: Option<Entity<RemoteTextEditor>>,
    /// Scroll handle for the virtualized delimited grid.
    delimited_scroll: UniformListScrollHandle,
    _app_subscription: Subscription,
}

impl RemoteFilePreviewWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            text_surface: None,
            delimited_scroll: UniformListScrollHandle::new(),
            _app_subscription: app_subscription,
        }
    }

    /// Ensure the read-only text surface exists and matches `tab`, returning it.
    fn text_surface_for(
        &mut self,
        tab_id: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RemoteTextEditor> {
        if let Some(surface) = self.text_surface.clone() {
            surface.update(cx, |surface, cx| surface.sync_read_only(tab_id, text, cx));
            return surface;
        }
        let app = self.app.clone();
        let id = tab_id.to_string();
        let content = text.to_string();
        let surface = cx.new(|cx| RemoteTextEditor::new_read_only(app, id, content, window, cx));
        self.text_surface = Some(surface.clone());
        surface
    }
}

impl Render for RemoteFilePreviewWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.app.read(cx).transfer.preview_has_workspace() {
            self.app.update(cx, |app, cx| {
                app.transfer.clear_preview_window_tracking();
                cx.notify();
            });
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        // If the active tab is a PDF whose current page is not yet rasterized,
        // request it now (off the render path). This drives lazy page rendering
        // as the user pages through the document.
        self.app.update(cx, |app, cx| {
            if matches!(
                app.transfer.active_preview_tab().map(|tab| &tab.content),
                Some(PreviewContent::Pdf(_))
            ) {
                app.request_active_pdf_page_render(cx);
            }
        });

        let (palette, font, font_size, title, active_tab, tabs) =
            self.app.read_with(cx, |app, _| {
                let workspace = app
                    .transfer
                    .preview_workspace()
                    .expect("preview checked above");
                let active = workspace
                    .active_tab()
                    .expect("open preview workspace has an active tab")
                    .clone();
                // Same-name parent-directory disambiguation lives on the model.
                let tabs = workspace.tab_labels();
                (
                    app.theme_palette(),
                    app.gpui_ui_font().font(),
                    app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                    active_tab_title(&active, &tabs),
                    active,
                    tabs,
                )
            });

        window.set_window_title(&title);
        focus_child_window_shell_if_idle(&self.shell_focus, window, cx);

        let close_app = self.app.clone();
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| {
                    app.close_transfer_preview(cx);
                });
                window.remove_window();
            });
        let header_close = on_close.clone();

        let body = self.preview_body(palette, &active_tab, &tabs, window, cx);

        child_window_root(&self.shell_focus, false, on_close)
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .font(font)
            .text_size(px(font_size))
            .child(child_window_header(
                palette,
                title,
                Some("icons/eye.svg"),
                self.chrome,
                window,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .into_any_element()
    }
}

impl RemoteFilePreviewWindow {
    fn preview_body(
        &mut self,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        tabs: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.preview_tab_strip(palette, &active_tab.id, tabs, cx))
            .child(self.preview_toolbar(palette, active_tab, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.preview_content(palette, active_tab, window, cx)),
            )
            .child(self.preview_status_bar(palette, active_tab, cx))
            .into_any_element()
    }

    fn preview_tab_strip(
        &self,
        palette: ThemePalette,
        active_id: &str,
        tabs: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // A tab strip that overflows horizontally; the overflow menu button at
        // the end lists every open tab so a tab scrolled out of view is still
        // reachable, matching the Tauri overflow list.
        let mut strip = div()
            .id("preview-tab-strip")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .overflow_x_scrollbar();

        for (id, label) in tabs {
            let is_active = id == active_id;
            let activate_id = id.clone();
            let close_id = id.clone();
            let app_activate = self.app.clone();
            let app_close = self.app.clone();
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("preview-tab-{id}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_active {
                        rgb(palette.surface_elevated)
                    } else {
                        rgb(palette.surface)
                    })
                    .text_color(if is_active {
                        rgb(palette.text)
                    } else {
                        rgb(palette.text_muted)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |_this, _, _, cx| {
                            let tab_id = activate_id.clone();
                            app_activate.update(cx, |app, cx| {
                                app.activate_transfer_preview_tab(&tab_id, cx);
                            });
                        }),
                    )
                    .child(div().max_w(px(220.)).truncate().child(label.clone()))
                    .child(
                        div()
                            .id(SharedString::from(format!("preview-tab-close-{id}")))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(palette.text_dimmed))
                            .hover(|this| this.text_color(rgb(palette.text)))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    let tab_id = close_id.clone();
                                    app_close.update(cx, |app, cx| {
                                        app.close_transfer_preview_tab(&tab_id, cx);
                                    });
                                }),
                            )
                            .child("×"),
                    ),
            );
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(strip)
            .child(self.preview_tab_overflow(palette, active_id, tabs, cx))
            .into_any_element()
    }

    /// Overflow list button: a compact menu listing every open tab, so any tab
    /// scrolled out of the strip stays reachable.
    fn preview_tab_overflow(
        &self,
        palette: ThemePalette,
        active_id: &str,
        tabs: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if tabs.len() <= 1 {
            return div().into_any_element();
        }
        let mut menu = div()
            .id("preview-tab-overflow")
            .flex_none()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text_muted))
            .hover(|this| this.bg(rgb(palette.surface_elevated)))
            .child(format!("⋯ {}", tabs.len()));
        // Clicking the overflow badge cycles to the next tab. A full popup menu
        // would need overlay plumbing; the badge surfaces the count and
        // guarantees reachability by cycling through every open tab.
        let app = self.app.clone();
        let ids: Vec<String> = tabs.iter().map(|(id, _)| id.clone()).collect();
        let active = active_id.to_string();
        menu = menu.on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
                let next = ids
                    .iter()
                    .position(|id| *id == active)
                    .map(|index| (index + 1) % ids.len())
                    .and_then(|index| ids.get(index).cloned());
                if let Some(next_id) = next {
                    app.update(cx, |app, cx| {
                        app.activate_transfer_preview_tab(&next_id, cx);
                    });
                }
            }),
        );
        menu.into_any_element()
    }

    fn preview_toolbar(
        &self,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_image = matches!(active_tab.content, PreviewContent::Image(_));
        let is_pdf = matches!(active_tab.content, PreviewContent::Pdf(_));
        let is_delimited = matches!(active_tab.content, PreviewContent::Delimited(_));

        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(palette.border));

        toolbar = toolbar.child(self.toolbar_button(
            palette,
            "preview-refresh",
            "icons/fe/refresh.svg",
            t!("filePreview.refresh").to_string(),
            {
                let app = self.app.clone();
                move |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.refresh_active_transfer_preview(window, cx);
                    });
                }
            },
        ));

        toolbar = toolbar.child(self.toolbar_button(
            palette,
            "preview-external",
            "icons/menu/external.svg",
            t!("filePreview.openExternal").to_string(),
            {
                let app = self.app.clone();
                move |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.open_active_transfer_preview_external(window, cx);
                    });
                }
            },
        ));

        if is_delimited {
            let header_on = matches!(
                &active_tab.content,
                PreviewContent::Delimited(data) if data.first_row_is_header
            );
            toolbar = toolbar.child(self.toolbar_button(
                palette,
                "preview-header-toggle",
                "icons/file/table.svg",
                if header_on {
                    t!("filePreview.headerOn").to_string()
                } else {
                    t!("filePreview.headerOff").to_string()
                },
                {
                    let app = self.app.clone();
                    move |_, _, cx| {
                        app.update(cx, |app, cx| {
                            if app.transfer.preview_toggle_delimited_header() {
                                cx.notify();
                            }
                        });
                    }
                },
            ));
        }

        if is_pdf {
            toolbar = self.pdf_toolbar_controls(toolbar, palette, active_tab, cx);
        }

        if is_image || is_pdf {
            let (zoom_out, zoom_in) = (0.8, 1.25);
            toolbar = toolbar
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-out",
                    "icons/menu/zoom-out.svg",
                    t!("filePreview.zoomOut").to_string(),
                    {
                        let app = self.app.clone();
                        move |_, _, cx| {
                            app.update(cx, |app, cx| {
                                if apply_preview_zoom(app, zoom_out, is_pdf) {
                                    cx.notify();
                                }
                            });
                        }
                    },
                ))
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-in",
                    "icons/menu/zoom-in.svg",
                    t!("filePreview.zoomIn").to_string(),
                    {
                        let app = self.app.clone();
                        move |_, _, cx| {
                            app.update(cx, |app, cx| {
                                if apply_preview_zoom(app, zoom_in, is_pdf) {
                                    cx.notify();
                                }
                            });
                        }
                    },
                ))
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-reset",
                    "icons/menu/fit.svg",
                    t!("filePreview.resetView").to_string(),
                    {
                        let app = self.app.clone();
                        move |_, _, cx| {
                            app.update(cx, |app, cx| {
                                if app.transfer.preview_reset_active_viewport() {
                                    cx.notify();
                                }
                            });
                        }
                    },
                ));
            if is_image {
                toolbar = toolbar
                    .child(self.toolbar_button(
                        palette,
                        "preview-rotate-left",
                        "icons/menu/rotate-left.svg",
                        t!("filePreview.rotateLeft").to_string(),
                        {
                            let app = self.app.clone();
                            move |_, _, cx| {
                                app.update(cx, |app, cx| {
                                    if app.transfer.preview_rotate_active_tab(false) {
                                        cx.notify();
                                    }
                                });
                            }
                        },
                    ))
                    .child(self.toolbar_button(
                        palette,
                        "preview-rotate-right",
                        "icons/menu/rotate-right.svg",
                        t!("filePreview.rotateRight").to_string(),
                        {
                            let app = self.app.clone();
                            move |_, _, cx| {
                                app.update(cx, |app, cx| {
                                    if app.transfer.preview_rotate_active_tab(true) {
                                        cx.notify();
                                    }
                                });
                            }
                        },
                    ));
            }
        }

        toolbar.into_any_element()
    }

    fn pdf_toolbar_controls(
        &self,
        toolbar: gpui::Div,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        _cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (current, total) = match &active_tab.content {
            PreviewContent::Pdf(document) => (document.current_page + 1, document.page_count),
            _ => (0, 0),
        };
        toolbar
            .child(self.toolbar_button(
                palette,
                "preview-pdf-prev",
                "icons/menu/chevron-left.svg",
                t!("filePreview.previousPage").to_string(),
                {
                    let app = self.app.clone();
                    move |_, _, cx| {
                        app.update(cx, |app, cx| {
                            if let Some(request) = app.transfer.preview_previous_pdf_page() {
                                app.start_pdf_page_render_job(request, cx);
                            }
                            cx.notify();
                        });
                    }
                },
            ))
            .child(
                div()
                    .px_2()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(
                        t!("filePreview.pageOfTotal", page = current, total = total).to_string(),
                    ),
            )
            .child(self.toolbar_button(
                palette,
                "preview-pdf-next",
                "icons/menu/chevron-right.svg",
                t!("filePreview.nextPage").to_string(),
                {
                    let app = self.app.clone();
                    move |_, _, cx| {
                        app.update(cx, |app, cx| {
                            if let Some(request) = app.transfer.preview_next_pdf_page() {
                                app.start_pdf_page_render_job(request, cx);
                            }
                            cx.notify();
                        });
                    }
                },
            ))
    }

    fn toolbar_button(
        &self,
        palette: ThemePalette,
        id: &'static str,
        icon_path: &'static str,
        tooltip: String,
        on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::AnyElement {
        // Icon-only control with a tooltip carrying the accessible label, so the
        // toolbar reads as buttons rather than bare "+/-/‹/›" glyphs. The svg is a
        // tintable monochrome asset (mask-rendered), so it tracks the theme text
        // color in both light and dark and stays crisp at high DPI.
        div()
            .id(id)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .hover(|this| this.bg(rgb(palette.surface_elevated)))
            .tooltip({
                let tooltip = tooltip.clone();
                move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx)
            })
            .on_mouse_down(gpui::MouseButton::Left, on_click)
            .child(
                svg()
                    .size(px(16.))
                    .flex_none()
                    .path(icon_path)
                    .text_color(rgb(palette.text)),
            )
            .into_any_element()
    }

    fn preview_content(
        &mut self,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match &active_tab.content {
            PreviewContent::Loading => centered_message(palette, t!("filePreview.loading").into()),
            PreviewContent::Unsupported => {
                centered_message(palette, t!("filePreview.unsupported").into())
            }
            PreviewContent::Error(message) => error_message(palette, message),
            PreviewContent::Text(text) => {
                let surface = self.text_surface_for(&active_tab.id, text, window, cx);
                text_surface_body(surface)
            }
            PreviewContent::Json { text, parse_error } => {
                let surface = self.text_surface_for(&active_tab.id, text, window, cx);
                json_body(palette, surface, parse_error.as_deref())
            }
            PreviewContent::Markdown(text) => markdown_body(palette, text, active_tab.id.as_str()),
            PreviewContent::Delimited(data) => {
                self.delimited_body(palette, data, active_tab.id.as_str(), cx)
            }
            PreviewContent::Image(image) => image_body(palette, image, active_tab),
            PreviewContent::Pdf(document) => pdf_body(palette, document, active_tab),
        }
    }

    fn delimited_body(
        &self,
        palette: ThemePalette,
        data: &PreviewDelimited,
        scroll_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let headers = data.headers();
        let column_count = data.column_count;
        let sort = data.sort;

        // Clickable header row with tri-state sort indicators.
        let mut header_row = div()
            .flex()
            .flex_row()
            .bg(rgb(palette.surface))
            .border_b_1()
            .border_color(rgb(palette.border));
        for col in 0..column_count {
            let label = headers.get(col).cloned().unwrap_or_default();
            let indicator = match sort {
                Some(DelimitedSort::Ascending(column)) if column == col => " ▲",
                Some(DelimitedSort::Descending(column)) if column == col => " ▼",
                _ => "",
            };
            let app = self.app.clone();
            header_row = header_row.child(
                div()
                    .id(SharedString::from(format!("preview-col-{scroll_id}-{col}")))
                    .min_w(px(120.))
                    .flex_1()
                    .px_2()
                    .py_1()
                    .border_r_1()
                    .border_color(rgb(palette.border))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight(700.))
                    .text_color(rgb(palette.text))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(palette.surface_elevated)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |_this, _, _, cx| {
                            app.update(cx, |app, cx| {
                                if app.transfer.preview_cycle_delimited_sort(col) {
                                    cx.notify();
                                }
                            });
                        }),
                    )
                    .child(format!("{label}{indicator}")),
            );
        }

        // Virtualized body: `uniform_list` only builds the visible rows, so a
        // large grid never materializes thousands of elements per frame.
        let row_count = data.row_count();
        let palette_for_rows = palette;
        let list = uniform_list(
            SharedString::from(format!("preview-csv-rows-{scroll_id}")),
            row_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(data) = this
                    .app
                    .read(cx)
                    .transfer
                    .active_preview_tab()
                    .and_then(|tab| match &tab.content {
                        PreviewContent::Delimited(data) => Some(data.clone()),
                        _ => None,
                    })
                else {
                    return Vec::new();
                };
                range
                    .map(|position| delimited_row(&palette_for_rows, &data, position, column_count))
                    .collect()
            }),
        )
        .flex_grow(1.0)
        .with_sizing_behavior(ListSizingBehavior::Auto)
        .track_scroll(&self.delimited_scroll);

        let mut column = div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(header_row)
            // The virtualized list owns vertical scrolling; the scrollbar hangs
            // off this non-scrolling parent so it does not scroll away with the
            // rows (per the AGENTS.md uniform_list guidance).
            .child(
                div()
                    .id(SharedString::from(format!("preview-csv-list-{scroll_id}")))
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .vertical_scrollbar(&self.delimited_scroll),
            );

        if data.truncated {
            column = column.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("filePreview.rowsTruncated").to_string()),
            );
        }

        div()
            .id(SharedString::from(format!("preview-csv-{scroll_id}")))
            .size_full()
            .overflow_x_scrollbar()
            .child(column)
            .into_any_element()
    }

    fn preview_status_bar(
        &self,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let category = preview_category_label(active_tab);
        let detail = match &active_tab.content {
            PreviewContent::Loading => t!("filePreview.loading").to_string(),
            PreviewContent::Unsupported => t!("filePreview.unsupported").to_string(),
            PreviewContent::Error(_) => t!("filePreview.error").to_string(),
            PreviewContent::Delimited(data) => {
                t!("filePreview.rowCount", count = data.row_count()).to_string()
            }
            PreviewContent::Pdf(document) => t!(
                "filePreview.pageOfTotal",
                page = document.current_page + 1,
                total = document.page_count
            )
            .to_string(),
            PreviewContent::Image(image) => format!("{}×{}", image.src_width, image.src_height),
            _ => String::new(),
        };
        let zoom = if matches!(
            active_tab.content,
            PreviewContent::Image(_) | PreviewContent::Pdf(_)
        ) {
            format!(
                "  ·  {}%",
                (active_tab.viewport.zoom * 100.0).round() as i32
            )
        } else {
            String::new()
        };

        // Session badge, so a preview belonging to a background session is
        // clearly labelled with its target session rather than assumed active.
        // Prefer the connection's saved name when the session maps to one,
        // falling back to the raw session id.
        let session_badge = active_tab
            .session_id
            .as_deref()
            .map(|session_id| {
                self.app.read_with(cx, |app, _| {
                    app.session
                        .metadata(session_id)
                        .and_then(|metadata| metadata.source_connection_id.clone())
                        .and_then(|connection_id| {
                            app.connection_state
                                .connection_by_id(&connection_id)
                                .map(|connection| connection.name.clone())
                        })
                        .unwrap_or_else(|| session_id.to_string())
                })
            })
            .unwrap_or_default();

        let size_and_time = format_size_and_time(active_tab.size, active_tab.modified_at);

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .text_size(px(11.))
            .text_color(rgb(palette.text_muted))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(session_badge_element(palette, &session_badge))
                    .child(div().truncate().child(active_tab.remote_path.clone())),
            )
            .child(div().child(format!("{category}  ·  {detail}{zoom}  ·  {size_and_time}")))
            .into_any_element()
    }
}

fn session_badge_element(palette: ThemePalette, session: &str) -> gpui::AnyElement {
    if session.trim().is_empty() {
        return div().into_any_element();
    }
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(rgb(palette.surface_elevated))
        .text_color(rgb(palette.text))
        .max_w(px(180.))
        .truncate()
        .child(session.to_string())
        .into_any_element()
}

fn format_size_and_time(size: Option<u64>, modified_at: Option<u32>) -> String {
    let size = crate::features::transfers::format_file_size(size);
    let time = crate::features::pages::transfers::format_sftp_modified(modified_at);
    match (size.is_empty(), time == "-") {
        (true, true) => String::new(),
        (false, true) => size,
        (true, false) => time,
        (false, false) => format!("{size}  ·  {time}"),
    }
}

/// A single delimited body row, built on demand by the virtualized list.
fn delimited_row(
    palette: &ThemePalette,
    data: &PreviewDelimited,
    position: usize,
    column_count: usize,
) -> gpui::AnyElement {
    let Some(row) = data.row(position) else {
        return div().into_any_element();
    };
    let mut body_row = div().flex().flex_row().bg(if position.is_multiple_of(2) {
        rgb(palette.bg)
    } else {
        rgb(palette.section_header)
    });
    for col in 0..column_count {
        let cell = row.get(col).cloned().unwrap_or_default();
        body_row = body_row.child(
            div()
                .min_w(px(120.))
                .flex_1()
                .px_2()
                .py_1()
                .border_r_1()
                .border_color(rgb(palette.surface_elevated))
                .text_size(px(11.))
                .text_color(rgb(palette.text))
                .child(cell),
        );
    }
    body_row.into_any_element()
}

/// A tab's window title: the disambiguated label if present, else the name.
fn active_tab_title(tab: &TransferPreviewState, tabs: &[(String, String)]) -> String {
    tabs.iter()
        .find(|(id, _)| *id == tab.id)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| {
            if tab.name.trim().is_empty() {
                tab.remote_path.clone()
            } else {
                tab.name.clone()
            }
        })
}

fn preview_category_label(tab: &TransferPreviewState) -> String {
    use nyaterm_core::PreviewCategory;
    match tab.category {
        PreviewCategory::Text => t!("filePreview.categoryText").to_string(),
        PreviewCategory::Json => "JSON".to_string(),
        PreviewCategory::Markdown => "Markdown".to_string(),
        PreviewCategory::Delimited { tab: true } => "TSV".to_string(),
        PreviewCategory::Delimited { tab: false } => "CSV".to_string(),
        PreviewCategory::Image => t!("filePreview.categoryImage").to_string(),
        PreviewCategory::Pdf => "PDF".to_string(),
        PreviewCategory::Unsupported => t!("filePreview.categoryUnsupported").to_string(),
    }
}

fn centered_message(palette: ThemePalette, message: SharedString) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(palette.text_muted))
        .child(message)
        .into_any_element()
}

fn error_message(palette: ThemePalette, message: &str) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_color(rgb(palette.danger))
        .child(message.to_string())
        .into_any_element()
}

/// A read-only selectable text surface (the shared `RemoteTextEditor` in
/// read-only mode) filling the content area.
fn text_surface_body(surface: Entity<RemoteTextEditor>) -> gpui::AnyElement {
    div().size_full().child(surface).into_any_element()
}

/// JSON body: an optional parse-error banner above the selectable raw text.
fn json_body(
    palette: ThemePalette,
    surface: Entity<RemoteTextEditor>,
    parse_error: Option<&str>,
) -> gpui::AnyElement {
    let mut column = div().size_full().flex().flex_col().min_h_0();
    if let Some(error) = parse_error {
        column = column.child(
            div()
                .flex_none()
                .px_3()
                .py_2()
                .bg(rgb(palette.surface_elevated))
                .border_b_1()
                .border_color(rgb(palette.danger))
                .text_size(px(12.))
                .text_color(rgb(palette.danger))
                .child(t!("filePreview.jsonInvalid", error = error).to_string()),
        );
    }
    column
        .child(div().flex_1().min_h_0().child(surface))
        .into_any_element()
}

fn markdown_body(palette: ThemePalette, text: &str, scroll_id: &str) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("preview-md-{scroll_id}")))
        .size_full()
        .overflow_y_scrollbar()
        .p_4()
        .child(markdown_content_view(palette, text))
        .into_any_element()
}

fn image_body(
    palette: ThemePalette,
    image: &crate::models::PreviewImage,
    active_tab: &TransferPreviewState,
) -> gpui::AnyElement {
    let (width, height) = fitted_dimensions(
        image.width,
        image.height,
        active_tab.viewport.zoom,
        active_tab.viewport.fit_to_window,
    );
    div()
        .id(SharedString::from(format!("preview-img-{}", active_tab.id)))
        .size_full()
        .overflow_scrollbar()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(palette.bg))
        .child(
            img(image.image.clone())
                .w(px(width))
                .h(px(height))
                .flex_none(),
        )
        .into_any_element()
}

fn pdf_body(
    palette: ThemePalette,
    document: &crate::models::PreviewPdfDocument,
    active_tab: &TransferPreviewState,
) -> gpui::AnyElement {
    let Some(page) = document.current_page() else {
        // The active page is still rasterizing (or failed and will retry).
        return centered_message(palette, t!("filePreview.renderingPage").into());
    };
    let zoom = active_tab.viewport.zoom.clamp(PDF_MIN_ZOOM, PDF_MAX_ZOOM);
    let (width, height) = fitted_dimensions(
        page.width,
        page.height,
        zoom,
        active_tab.viewport.fit_to_window,
    );
    div()
        .id(SharedString::from(format!("preview-pdf-{}", active_tab.id)))
        .size_full()
        .overflow_scrollbar()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(palette.bg))
        .child(
            div()
                .flex_none()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .child(
                    img(page.image.clone())
                        .w(px(width))
                        .h(px(height))
                        .flex_none(),
                ),
        )
        .into_any_element()
}

/// Compute the painted dimensions. When `fit_to_window` is set we cannot read
/// the container bounds here (this runs in a plain element), so we approximate a
/// fit by capping the natural size against the window's document dimensions;
/// otherwise honour the zoom multiplier.
fn fitted_dimensions(width: u32, height: u32, zoom: f32, fit_to_window: bool) -> (f32, f32) {
    let natural_w = width.max(1) as f32;
    let natural_h = height.max(1) as f32;
    if fit_to_window {
        // Fit inside the document window's content area (roughly 1080x620 after
        // chrome/toolbar/status). Scale down only; never enlarge past natural.
        let max_w = 1040.0;
        let max_h = 600.0;
        let scale = (max_w / natural_w).min(max_h / natural_h).min(1.0);
        ((natural_w * scale).max(1.0), (natural_h * scale).max(1.0))
    } else {
        ((natural_w * zoom).max(1.0), (natural_h * zoom).max(1.0))
    }
}

/// Apply a zoom step, using PDF bounds for PDFs and image bounds otherwise.
fn apply_preview_zoom(app: &mut NyaTermApp, factor: f32, is_pdf: bool) -> bool {
    if is_pdf {
        app.transfer
            .preview_zoom_active_tab_bounded(factor, PDF_MIN_ZOOM, PDF_MAX_ZOOM)
    } else {
        app.transfer.preview_zoom_active_tab(factor)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn open_remote_file_preview_window(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.transfer.preview_window() {
            activate_child_window(
                &cx.entity(),
                handle,
                |app: &mut NyaTermApp| Some(app.transfer.preview_window_slot()),
                cx,
            );
            return;
        }
        if !self.transfer.begin_preview_window_open() {
            return;
        }
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| {
            let should_open = app.read(cx).transfer.preview_window_open_is_pending();
            if should_open {
                open_remote_file_preview_window_now_from_app(app, cx);
            }
        });
    }
}

fn open_remote_file_preview_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if let Some(handle) = app.read(cx).transfer.preview_window() {
        let activate_result = handle.update(cx, |_, window, _| window.activate_window());
        app.update(cx, |app, cx| {
            app.transfer
                .finish_preview_window_activation(handle, activate_result.is_ok());
            cx.notify();
        });
        return;
    }
    if !app.read(cx).transfer.preview_has_workspace() {
        app.update(cx, |app, cx| {
            app.transfer.clear_preview_window_tracking();
            cx.notify();
        });
        return;
    }

    let title = t!("filePreview.title").to_string();
    let spec = ChildWindowSpec::document(title, 1080., 760.).min_size(640., 480.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                app.close_transfer_preview(cx);
                let should_close = !app.transfer.preview_has_workspace();
                if should_close {
                    app.transfer.clear_preview_window_tracking();
                }
                should_close
            })
        });
        let view = cx.new(|cx| RemoteFilePreviewWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.transfer.finish_preview_window_open(handle);
            cx.notify();
        }
        Err(error) => {
            app.transfer.clear_preview_window_tracking();
            app.shell
                .set_status(t!("filePreview.statusWindowFailed", error = error).to_string());
            cx.notify();
        }
    });
}

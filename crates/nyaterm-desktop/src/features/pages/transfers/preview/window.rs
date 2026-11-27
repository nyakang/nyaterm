//! The read-only remote file preview window.
//!
//! A `WindowKind::Normal` document window (1080x760) that shows one preview tab
//! at a time with a tab strip, a toolbar, and a status bar. It mirrors the
//! editor window's open/activate/close plumbing but renders purely from a
//! snapshot of the preview workspace, so the draw path never blocks or mutates:
//! every control routes back through `self.app.update`.
//!
//! Text and JSON previews retain a `RemoteTextEditor` in read-only mode (the shared
//! selectable/copyable surface), kept as an entity across frames and reconciled
//! to the active tab in render, because GPUI entities must not be constructed in
//! a render path on every frame. Markdown keeps a separate `NyaTextState`, updated
//! by app notifications so parsing never starts from render. Delimited previews use a virtualized
//! `uniform_list` so a large grid is never materialized into thousands of
//! elements per frame.

use nyaterm_file_browser_gpui::preview::{
    PDF_MAX_ZOOM, PDF_MIN_ZOOM, centered_message, delimited_headers, delimited_row, error_message,
    format_size_and_time, json_body, markdown_body, session_badge_element, text_surface_body,
};
use rust_i18n::t;

use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, ListSizingBehavior, Render,
    SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*, px, rgb,
    uniform_list,
};
use nyaterm_ui::text::{NyaTextFormat, NyaTextState};
use nyaterm_ui::{NyaScrollable, NyaWindowHandle, activate_child_window, nya_root};

use crate::features::transfers::RemoteTextEditor;
use crate::features::{
    NyaTermApp,
    view_widgets::{
        ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
        child_window_options, child_window_root, focus_child_window_shell_if_idle,
    },
};
use crate::models::{PreviewContent, TransferPreviewState};
use crate::theme::ThemePalette;
use nyaterm_preview::delimited::{DelimitedSort, PreviewDelimited};

#[cfg(test)]
mod tests;

pub(super) struct RemoteFilePreviewWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    /// Read-only selectable text surface, reconciled to the active tab in
    /// render. `None` until the first text/JSON tab is shown.
    text_surface: Option<Entity<RemoteTextEditor>>,
    markdown_preview: Option<MarkdownPreview>,
    /// Scroll handle for the virtualized delimited grid.
    delimited_scroll: UniformListScrollHandle,
    _app_subscription: Subscription,
}

struct MarkdownPreview {
    tab_id: String,
    text: NyaTextState,
    _layout_subscription: Subscription,
}

impl RemoteFilePreviewWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |this, _, cx| {
            this.sync_markdown_preview(cx);
            cx.notify();
        });
        let mut view = Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            text_surface: None,
            markdown_preview: None,
            delimited_scroll: UniformListScrollHandle::new(),
            _app_subscription: app_subscription,
        };
        view.sync_markdown_preview(cx);
        view
    }

    fn sync_markdown_preview(&mut self, cx: &mut Context<Self>) {
        // Clone a source only when it changed. App notifications include unrelated
        // transfer progress; they must not enqueue another Markdown parse.
        let update = self
            .app
            .read_with(cx, |app, _| match app.transfer.active_preview_tab() {
                Some(tab) => match &tab.content {
                    PreviewContent::Markdown(source) => {
                        if self.markdown_preview.as_ref().is_some_and(|preview| {
                            preview.tab_id == tab.id && preview.text.matches_source(source)
                        }) {
                            None
                        } else {
                            Some(Some((tab.id.clone(), source.clone())))
                        }
                    }
                    _ => Some(None),
                },
                None => Some(None),
            });
        let Some(update) = update else { return };
        let Some((tab_id, source)) = update else {
            if let Some(previous) = self.markdown_preview.take() {
                previous.text.invalidate_selection(cx);
            }
            return;
        };
        if let Some(preview) = &self.markdown_preview
            && preview.tab_id == tab_id
        {
            preview.text.set_text(&source, cx);
            return;
        }
        if let Some(previous) = self.markdown_preview.take() {
            previous.text.invalidate_selection(cx);
        }
        let text = NyaTextState::new(NyaTextFormat::Markdown, &source, cx);
        let subscription = text.observe_layout(cx, |_, cx| cx.notify());
        self.markdown_preview = Some(MarkdownPreview {
            tab_id,
            text,
            _layout_subscription: subscription,
        });
    }

    /// Ensure the read-only text surface exists and matches `tab`, returning it.
    fn text_surface_for(
        &mut self,
        tab_id: &str,
        path: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RemoteTextEditor> {
        if let Some(surface) = self.text_surface.clone() {
            surface.update(cx, |surface, cx| {
                surface.sync_read_only(tab_id, path, text, cx)
            });
            return surface;
        }
        let app = self.app.clone();
        let id = tab_id.to_string();
        let content = text.to_string();
        // The tab ID can contain an opaque raw-path token. Only the display path
        // carries the filename needed for syntax recognition.
        let surface =
            cx.new(|cx| RemoteTextEditor::new_read_only(app, id, path, content, window, cx));
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
                cx,
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

    fn preview_controls(&self) -> nyaterm_file_browser_gpui::preview_controls::PreviewControls {
        use nyaterm_file_browser_gpui::preview_controls::{PreviewAction, PreviewControls};
        let app = self.app.downgrade();
        PreviewControls::new(Rc::new(move |action, window, cx| {
            let Some(app) = app.upgrade() else {
                return;
            };
            app.update(cx, |app, cx| {
                let changed = match action {
                    PreviewAction::Activate(id) => {
                        app.activate_transfer_preview_tab(&id, cx);
                        false
                    }
                    PreviewAction::Close(id) => {
                        app.close_transfer_preview_tab(&id, cx);
                        false
                    }
                    PreviewAction::Encoding(selection) => {
                        app.transfer.set_preview_read_encoding(selection);
                        app.refresh_active_transfer_preview(window, cx);
                        false
                    }
                    PreviewAction::Refresh => {
                        app.refresh_active_transfer_preview(window, cx);
                        false
                    }
                    PreviewAction::External => {
                        app.open_active_transfer_preview_external(window, cx);
                        false
                    }
                    PreviewAction::ToggleHeader => app.transfer.preview_toggle_delimited_header(),
                    PreviewAction::Zoom { factor, pdf } => apply_preview_zoom(app, factor, pdf),
                    PreviewAction::Reset => app.transfer.preview_reset_active_viewport(),
                    PreviewAction::Rotate(right) => app.transfer.preview_rotate_active_tab(right),
                    PreviewAction::PreviousPage | PreviewAction::NextPage => {
                        let request = if matches!(action, PreviewAction::PreviousPage) {
                            app.transfer.preview_previous_pdf_page()
                        } else {
                            app.transfer.preview_next_pdf_page()
                        };
                        if let Some(request) = request {
                            app.start_pdf_page_render_job(request, cx);
                        }
                        true
                    }
                };
                if changed {
                    cx.notify();
                }
            });
        }))
    }

    fn preview_tab_strip(
        &self,
        palette: ThemePalette,
        active_id: &str,
        tabs: &[(String, String)],
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.preview_controls().tab_strip(palette, active_id, tabs)
    }

    fn preview_toolbar(
        &self,
        palette: ThemePalette,
        active_tab: &TransferPreviewState,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use nyaterm_file_browser_gpui::preview_controls::PreviewToolbar;
        let view = match &active_tab.content {
            PreviewContent::Image(_) => PreviewToolbar::Image,
            PreviewContent::Pdf(document) => PreviewToolbar::Pdf {
                current: document.current_page + 1,
                total: document.page_count,
            },
            PreviewContent::Delimited(data) => PreviewToolbar::Delimited {
                header: data.first_row_is_header(),
            },
            _ => PreviewToolbar::Other,
        };
        let encoding = (!active_tab.category.is_binary() && active_tab.category.is_fetchable())
            .then_some(
                nyaterm_file_browser_gpui::preview_controls::PreviewTextEncoding {
                    selection: active_tab.text_encoding.read,
                    format: active_tab.text_encoding.format,
                    loading: matches!(active_tab.content, PreviewContent::Loading),
                },
            );
        self.preview_controls().toolbar(palette, view, encoding)
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
                let surface = self.text_surface_for(
                    &active_tab.id,
                    &active_tab.remote_path,
                    text,
                    window,
                    cx,
                );
                text_surface_body(surface)
            }
            PreviewContent::Json { text, parse_error } => {
                let surface = self.text_surface_for(
                    &active_tab.id,
                    &active_tab.remote_path,
                    text,
                    window,
                    cx,
                );
                json_body(palette, surface, parse_error.as_deref())
            }
            PreviewContent::Markdown(_) => {
                match self
                    .markdown_preview
                    .as_ref()
                    .filter(|preview| preview.tab_id == active_tab.id)
                {
                    Some(preview) => markdown_body(palette, &preview.text, &active_tab.id),
                    None => centered_message(palette, t!("filePreview.loading").into()),
                }
            }
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
        let headers = delimited_headers(data);
        let column_count = data.column_count();
        let sort = data.sort();

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

        if data.truncated() {
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

fn image_body(
    palette: ThemePalette,
    image: &crate::models::PreviewImage,
    tab: &TransferPreviewState,
) -> gpui::AnyElement {
    nyaterm_file_browser_gpui::preview::image_body(
        palette,
        nyaterm_file_browser_gpui::preview::ImagePresentation {
            id: &tab.id,
            image: image.image.clone(),
            width: image.width,
            height: image.height,
            zoom: tab.viewport.zoom,
            fit_to_window: tab.viewport.fit_to_window,
        },
    )
}
fn pdf_body(
    palette: ThemePalette,
    document: &crate::models::PreviewPdfDocument,
    tab: &TransferPreviewState,
) -> gpui::AnyElement {
    nyaterm_file_browser_gpui::preview::pdf_body(
        palette,
        document.current_page().map(
            |page| nyaterm_file_browser_gpui::preview::ImagePresentation {
                id: &tab.id,
                image: page.image.clone(),
                width: page.width,
                height: page.height,
                zoom: tab.viewport.zoom,
                fit_to_window: tab.viewport.fit_to_window,
            },
        ),
    )
}

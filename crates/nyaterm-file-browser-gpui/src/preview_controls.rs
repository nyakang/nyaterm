//! Preview chrome consumes display metadata and sends actions to the workspace owner.
use gpui::{App, IntoElement, SharedString, Window, div, prelude::*, px, rgb, svg};
use nyaterm_ui::{NyaScrollable as _, NyaTooltip, ThemePalette};
use rust_i18n::t;
use std::rc::Rc;

#[derive(Clone)]
pub enum PreviewAction {
    Activate(String),
    Close(String),
    Refresh,
    External,
    ToggleHeader,
    Zoom { factor: f32, pdf: bool },
    Reset,
    Rotate(bool),
    PreviousPage,
    NextPage,
}
pub type PreviewActionHandler = Rc<dyn Fn(PreviewAction, &mut Window, &mut App)>;

#[derive(Default)]
pub enum PreviewToolbar {
    #[default]
    Other,
    Delimited {
        header: bool,
    },
    Image,
    Pdf {
        current: usize,
        total: usize,
    },
}
pub struct PreviewControls {
    handler: PreviewActionHandler,
}
impl PreviewControls {
    pub fn new(handler: PreviewActionHandler) -> Self {
        Self { handler }
    }
    pub fn tab_strip(
        &self,
        palette: ThemePalette,
        active_id: &str,
        tabs: &[(String, String)],
    ) -> gpui::AnyElement {
        // A tab strip that overflows horizontally; the overflow menu button at
        // the end cycles through open tabs so a tab scrolled out of view is still
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
            let app_activate = self.handler.clone();
            let app_close = self.handler.clone();
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
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                        let tab_id = activate_id.clone();
                        app_activate(PreviewAction::Activate(tab_id), window, cx);
                    })
                    .child(div().max_w(px(220.)).truncate().child(label.clone()))
                    .child(
                        div()
                            .id(SharedString::from(format!("preview-tab-close-{id}")))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(palette.text_dimmed))
                            .hover(|this| this.text_color(rgb(palette.text)))
                            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                let tab_id = close_id.clone();
                                app_close(PreviewAction::Close(tab_id), window, cx);
                            })
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
            .child(self.tab_overflow(palette, active_id, tabs))
            .into_any_element()
    }

    /// Overflow badge: cycle through open tabs, so any tab
    /// scrolled out of the strip stays reachable.
    fn tab_overflow(
        &self,
        palette: ThemePalette,
        active_id: &str,
        tabs: &[(String, String)],
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
        let app = self.handler.clone();
        let ids: Vec<String> = tabs.iter().map(|(id, _)| id.clone()).collect();
        let active = active_id.to_string();
        menu = menu.on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            let next = ids
                .iter()
                .position(|id| *id == active)
                .map(|index| (index + 1) % ids.len())
                .and_then(|index| ids.get(index).cloned());
            if let Some(next_id) = next {
                app(PreviewAction::Activate(next_id), window, cx);
            }
        });
        menu.into_any_element()
    }

    pub fn toolbar(&self, palette: ThemePalette, view: PreviewToolbar) -> gpui::AnyElement {
        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(self.toolbar_button(
                palette,
                "preview-refresh",
                "icons/fe/refresh.svg",
                t!("filePreview.refresh").to_string(),
                PreviewAction::Refresh,
            ))
            .child(self.toolbar_button(
                palette,
                "preview-external",
                "icons/menu/external.svg",
                t!("filePreview.openExternal").to_string(),
                PreviewAction::External,
            ));
        if let PreviewToolbar::Delimited { header } = view {
            toolbar = toolbar.child(self.toolbar_button(
                palette,
                "preview-header-toggle",
                "icons/file/table.svg",
                if header {
                    t!("filePreview.headerOn").to_string()
                } else {
                    t!("filePreview.headerOff").to_string()
                },
                PreviewAction::ToggleHeader,
            ));
        }
        if let PreviewToolbar::Pdf { current, total } = view {
            toolbar = toolbar
                .child(self.toolbar_button(
                    palette,
                    "preview-pdf-prev",
                    "icons/menu/chevron-left.svg",
                    t!("filePreview.previousPage").to_string(),
                    PreviewAction::PreviousPage,
                ))
                .child(
                    div()
                        .px_2()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(
                            t!("filePreview.pageOfTotal", page = current, total = total)
                                .to_string(),
                        ),
                )
                .child(self.toolbar_button(
                    palette,
                    "preview-pdf-next",
                    "icons/menu/chevron-right.svg",
                    t!("filePreview.nextPage").to_string(),
                    PreviewAction::NextPage,
                ));
        }
        let pdf = matches!(view, PreviewToolbar::Pdf { .. });
        if pdf || matches!(view, PreviewToolbar::Image) {
            toolbar = toolbar
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-out",
                    "icons/menu/zoom-out.svg",
                    t!("filePreview.zoomOut").to_string(),
                    PreviewAction::Zoom { factor: 0.8, pdf },
                ))
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-in",
                    "icons/menu/zoom-in.svg",
                    t!("filePreview.zoomIn").to_string(),
                    PreviewAction::Zoom { factor: 1.25, pdf },
                ))
                .child(self.toolbar_button(
                    palette,
                    "preview-zoom-reset",
                    "icons/menu/fit.svg",
                    t!("filePreview.resetView").to_string(),
                    PreviewAction::Reset,
                ));
            if !pdf {
                toolbar = toolbar
                    .child(self.toolbar_button(
                        palette,
                        "preview-rotate-left",
                        "icons/menu/rotate-left.svg",
                        t!("filePreview.rotateLeft").to_string(),
                        PreviewAction::Rotate(false),
                    ))
                    .child(self.toolbar_button(
                        palette,
                        "preview-rotate-right",
                        "icons/menu/rotate-right.svg",
                        t!("filePreview.rotateRight").to_string(),
                        PreviewAction::Rotate(true),
                    ));
            }
        }
        toolbar.into_any_element()
    }
    fn toolbar_button(
        &self,
        palette: ThemePalette,
        id: &'static str,
        icon_path: &'static str,
        tooltip: String,
        action: PreviewAction,
    ) -> gpui::AnyElement {
        let handler = self.handler.clone();
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
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                handler(action.clone(), window, cx)
            })
            .child(
                svg()
                    .size(px(16.))
                    .flex_none()
                    .path(icon_path)
                    .text_color(rgb(palette.text)),
            )
            .into_any_element()
    }
}

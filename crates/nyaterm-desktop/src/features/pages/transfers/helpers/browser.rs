use crate::models::transfer_ui::browser_sort_column_label;
use gpui::{
    ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px, rgb, rgba,
};

use crate::features::pages::transfers::panel::TransferPanel;
use crate::theme::ThemePalette;
use nyaterm_files::file_browser::listing::{FileBrowserSortColumn, FileBrowserSortDirection};

/// Height of the sort-header row.
///
/// The browser's vertical scrollbar overlay starts below the header, so the two
/// must agree or the bar's track is offset from the rows it scrolls.
pub(in crate::features::pages::transfers) const FILE_BROWSER_HEADER_HEIGHT_PX: f32 = 28.;

pub(in crate::features::pages::transfers) fn transfer_browser_search_status(
    query: &str,
    visible: usize,
    total: usize,
) -> String {
    if query.trim().is_empty() {
        format!("{total} item(s)")
    } else {
        format!("{visible} of {total} item(s) match search")
    }
}

pub(in crate::features::pages::transfers) fn sort_header_cell(
    palette: ThemePalette,
    column: FileBrowserSortColumn,
    localized_label: impl Into<SharedString>,
    width: Pixels,
    state: TransferBrowserSortHeaderState,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let localized_label: SharedString = localized_label.into();
    let is_active = column == state.active_column;
    let is_resizing = state.resizing_column == Some(column);
    let direction_icon = match state.direction {
        FileBrowserSortDirection::Ascending => "icons/fe/sort-ascending.svg",
        FileBrowserSortDirection::Descending => "icons/fe/sort-descending.svg",
    };

    div()
        .id(SharedString::from(format!(
            "transfer-browser-sort-{}",
            browser_sort_column_label(column).to_lowercase()
        )))
        .h(px(FILE_BROWSER_HEADER_HEIGHT_PX))
        .w(width)
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .border_r_1()
        .border_color(rgb(palette.surface_elevated))
        .cursor_pointer()
        .bg(if is_active {
            rgba((palette.primary << 8) | 0x14)
        } else {
            state.header_bg
        })
        .text_size(px(10.))
        .font_weight(FontWeight(800.))
        .text_color(if is_active {
            rgb(palette.link)
        } else {
            rgb(palette.text_muted)
        })
        .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
        .on_click(cx.listener(move |panel, _, _, cx| {
            panel.with_app(cx, |this, cx| {
                this.toggle_transfer_browser_sort(column, cx);
            })
        }))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(localized_label.to_uppercase()),
        )
        .when(is_active, |this| {
            this.child(
                gpui::svg()
                    .size(px(14.))
                    .flex_none()
                    .path(direction_icon)
                    .text_color(rgb(palette.link)),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!(
                    "transfer-browser-resize-{}",
                    browser_sort_column_label(column).to_lowercase()
                )))
                .absolute()
                .right(px(-3.))
                .top(px(4.))
                .bottom(px(4.))
                .w(px(7.))
                .rounded_sm()
                .cursor_col_resize()
                .bg(if is_resizing {
                    rgb(palette.success)
                } else {
                    rgb(0x1b2433)
                })
                .hover(|this| this.bg(rgb(palette.success)))
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |panel, event: &MouseDownEvent, _, cx| {
                        panel.with_app(cx, |this, cx| {
                            cx.stop_propagation();
                            this.start_transfer_browser_column_resize(column, event, cx);
                        })
                    }),
                ),
        )
}

#[derive(Clone, Copy)]
pub(in crate::features::pages::transfers) struct TransferBrowserSortHeaderState {
    pub header_bg: Rgba,
    pub active_column: FileBrowserSortColumn,
    pub direction: FileBrowserSortDirection,
    pub resizing_column: Option<FileBrowserSortColumn>,
}

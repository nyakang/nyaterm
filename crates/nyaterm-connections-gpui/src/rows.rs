//! Saved-connection row presentation. Selection, activation and drag routing are injected by the caller.
use crate::{
    details::ConnectionDetailRow, forms::connection_tree_indent_px,
    hover_card::ConnectionDetailsHoverCard,
};
use gpui::{
    AnyElement, Div, FontWeight, SharedString, Stateful, div, prelude::*, px, relative, rgb, rgba,
};
use nyaterm_ui::{NyaHoverCard, NyaPopoverAlign, NyaPopoverPlacement, ThemePalette};
use std::{sync::Arc, time::Duration};

pub const CONNECTION_LIST_ROW_HEIGHT_PX: f32 = 34.;
pub const CONNECTION_ACTION_CLEARANCE_PX: f32 = 56.;
pub const CONNECTION_LIST_PADDING_PX: f32 = 6.;

pub struct ConnectionRow {
    pub id: String,
    pub name: String,
    pub depth: usize,
    pub palette: ThemePalette,
    pub selected: bool,
    pub keyboard_active: bool,
    pub drop_before: bool,
    pub drop_after: bool,
    pub drop_inside: bool,
    pub icon: AnyElement,
    pub details: Arc<[ConnectionDetailRow]>,
    pub share_text: String,
}

pub fn saved_connection_row(
    view: ConnectionRow,
    hover_viewport: &gpui::ScrollHandle,
) -> Stateful<Div> {
    let selected = view.selected;
    let keyboard_active = view.keyboard_active;
    let show_before = view.drop_before;
    let show_after = view.drop_after;
    let show_inside = view.drop_inside;
    let palette = view.palette;
    let row_selector = format!("connection-row-{}", view.id);
    let name_selector = format!("connection-row-name-{}", view.id);
    div()
        .id(SharedString::from(row_selector.clone()))
        .debug_selector(move || row_selector.clone())
        .relative()
        .h(px(CONNECTION_LIST_ROW_HEIGHT_PX))
        // The list scrolls sideways, so a row is at least the panel width and
        // grows past it when the name is long.
        .min_w(relative(1.))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .pl(px(connection_tree_indent_px(view.depth)))
        .bg(if selected {
            rgba((palette.primary << 8) | 0x1a)
        } else if keyboard_active {
            rgba((palette.primary << 8) | 0x12)
        } else if show_inside {
            rgb(palette.hover)
        } else {
            rgba(0x00000000)
        })
        .when(keyboard_active && !selected, |this| {
            this.border_1().border_color(rgb(palette.primary))
        })
        .hover(move |this| this.bg(rgb(palette.hover)))
        .when(show_inside, |this| {
            this.border_1().border_color(rgb(palette.link))
        })
        .cursor_default()
        // Match Tauri's hoverable TooltipContent with a real HoverCard: the
        // pointer can cross into the card and click host/port/user copy buttons.
        .child(
            NyaHoverCard::new(
                format!("connection-details-hover-card-{}", view.id),
                div()
                    .id(SharedString::from(name_selector.clone()))
                    .debug_selector(move || name_selector.clone())
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    // Match Tauri's `pr-14`: at maximum horizontal scroll the
                    // final glyph can move clear of the viewport-fixed actions.
                    .pr(px(CONNECTION_ACTION_CLEARANCE_PX))
                    .child(view.icon)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(500.))
                            .text_color(if selected {
                                rgb(palette.link)
                            } else {
                                rgb(palette.text)
                            })
                            // Full name, never clipped — the list scrolls to reach it.
                            .whitespace_nowrap()
                            .child(view.name),
                    ),
                ConnectionDetailsHoverCard::new(
                    view.id.clone(),
                    view.details,
                    view.share_text,
                    palette,
                ),
            )
            // Use the visible list edges, independent of name width, nesting
            // and horizontal scrolling. Side placement flips at window edges.
            .horizontal_viewport(hover_viewport)
            .placement(NyaPopoverPlacement::Right)
            .align(NyaPopoverAlign::Center)
            // The scroll viewport excludes the list shell padding.
            .offset(px(CONNECTION_LIST_PADDING_PX + 6.))
            .open_delay(Duration::from_millis(350))
            .close_delay(Duration::from_millis(120))
            .appearance(false),
        )
        .child(div().flex_1().min_w_0())
        .when(show_before, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .top_0()
                    .h(px(2.))
                    .rounded_full()
                    .bg(rgb(palette.link)),
            )
        })
        .when(show_after, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .bottom_0()
                    .h(px(2.))
                    .rounded_full()
                    .bg(rgb(palette.link)),
            )
        })
}

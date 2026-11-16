use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, Render, Window, div, px, rgb, rgba};
use nyaterm_core::truncate_preview;

#[derive(Clone, Debug)]
pub(in crate::features) struct ConnectionDragPayload {
    pub kind: ConnectionDragKind,
    pub id: String,
    pub label: String,
    pub panel: gpui::WeakEntity<crate::features::pages::connections::panel::ConnectionPanel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum ConnectionDragKind {
    Connection,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum ConnectionDropPosition {
    Before,
    After,
    Inside,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct ConnectionDropTarget {
    pub id: Option<String>,
    pub kind: ConnectionDragKind,
    pub position: ConnectionDropPosition,
}

pub(in crate::features) struct ConnectionDragPreview {
    payload: ConnectionDragPayload,
    position: gpui::Point<gpui::Pixels>,
}

impl ConnectionDragPreview {
    pub(in crate::features) fn new(
        payload: ConnectionDragPayload,
        position: gpui::Point<gpui::Pixels>,
    ) -> Self {
        Self { payload, position }
    }
}

impl Render for ConnectionDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let destination = self
            .payload
            .panel
            .upgrade()
            .and_then(|panel| panel.read(cx).drag_preview_label());
        let (icon, accent) = match self.payload.kind {
            ConnectionDragKind::Connection => ("icons/brand/server.svg", rgb(0x3fb950)),
            ConnectionDragKind::Group => ("icons/conn/folder.svg", rgb(0x58a6ff)),
        };
        div()
            .pl(self.position.x - px(90.))
            .pt(self.position.y - px(16.))
            .child(
                div()
                    .w(px(200.))
                    .min_h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x388bfd))
                    .bg(rgba(0x0d1117ee))
                    .shadow_lg()
                    .child(crate::features::view_widgets::mono_icon(
                        icon,
                        accent.into(),
                        13.,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(0xe5edf7))
                            .flex()
                            .flex_col()
                            .child(truncate_preview(&self.payload.label, 24))
                            .when_some(destination, |row, label| {
                                row.child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight(400.))
                                        .child(truncate_preview(&label, 32)),
                                )
                            }),
                    ),
            )
    }
}

mod dnd;
mod menus;
mod selection;

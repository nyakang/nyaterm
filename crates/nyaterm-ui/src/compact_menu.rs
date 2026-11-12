use gpui::{
    App, ClickEvent, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px, rgb, svg,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};

use crate::theme::ThemePalette;

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

struct TrailingAction {
    id: SharedString,
    icon: SharedString,
    tooltip: SharedString,
    handler: Handler,
}

/// A compact menu row with an independently focusable, hover-revealed action.
#[derive(IntoElement)]
pub struct NyaCompactMenuRow {
    id: SharedString,
    label: SharedString,
    icon: SharedString,
    icon_color: u32,
    palette: ThemePalette,
    current: bool,
    handler: Handler,
    trailing: Option<TrailingAction>,
    scroll_on_focus: Option<(gpui::ScrollHandle, usize)>,
}

impl NyaCompactMenuRow {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: impl Into<SharedString>,
        icon_color: u32,
        palette: ThemePalette,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: icon.into(),
            icon_color,
            palette,
            current: false,
            handler: Box::new(handler),
            trailing: None,
            scroll_on_focus: None,
        }
    }

    /// Keep keyboard navigation visible in a caller-owned scrollable list.
    pub fn scroll_on_focus(mut self, scroll: gpui::ScrollHandle, index: usize) -> Self {
        self.scroll_on_focus = Some((scroll, index));
        self
    }

    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    pub fn trailing_action(
        mut self,
        id: impl Into<SharedString>,
        icon: impl Into<SharedString>,
        tooltip: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.trailing = Some(TrailingAction {
            id: id.into(),
            icon: icon.into(),
            tooltip: tooltip.into(),
            handler: Box::new(handler),
        });
        self
    }
}

impl RenderOnce for NyaCompactMenuRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Render kit buttons in this component's scope so these keyed handles
        // are the actual button handles, not copies in a different component scope.
        let focused = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .is_focused(window);
        let action_focused = self.trailing.as_ref().is_some_and(|action| {
            window
                .use_keyed_state(action.id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .is_focused(window)
        });
        let previous_focus = window.use_keyed_state(
            (gpui::ElementId::from(self.id.clone()), "row-focused"),
            cx,
            |_, _| false,
        );
        let has_focus = focused || action_focused;
        let newly_focused = has_focus && !*previous_focus.read(cx);
        previous_focus.update(cx, |previous, _| *previous = has_focus);
        if newly_focused && let Some((scroll, index)) = &self.scroll_on_focus {
            scroll.scroll_to_item(*index);
        }
        let group = SharedString::from(format!("{}-row", self.id));
        let debug_id = self.id.clone();
        let palette = self.palette;
        div()
            .group(group.clone())
            .h(px(32.))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .rounded_sm()
            .hover(|style| style.bg(rgb(palette.hover)))
            .when(self.current, |row| row.bg(rgb(palette.surface_elevated)))
            .child(
                Button::new(self.id)
                    .debug_selector(move || debug_id.to_string())
                    .ghost()
                    .h_full()
                    .min_w_0()
                    .flex_1()
                    .px(px(8.))
                    .border_0()
                    .tooltip(self.label.clone())
                    .accessibility_label(self.label.clone())
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                svg()
                                    .path(self.icon)
                                    .size(px(18.))
                                    .flex_none()
                                    .text_color(rgb(self.icon_color)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(if self.current {
                                        palette.link
                                    } else {
                                        palette.text
                                    }))
                                    .child(self.label),
                            ),
                    )
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        (self.handler)(event, window, cx);
                    })
                    .render(window, cx),
            )
            .when_some(self.trailing, |row, action| {
                let debug_id = action.id.clone();
                row.child(
                    Button::new(action.id)
                        .debug_selector(move || debug_id.to_string())
                        .ghost()
                        .size(px(24.))
                        .mr(px(4.))
                        .p_0()
                        .border_0()
                        .flex_none()
                        .tooltip(action.tooltip.clone())
                        .accessibility_label(action.tooltip)
                        .when(!focused && !action_focused, |button| {
                            button
                                .opacity(0.)
                                .group_hover(group, |style| style.opacity(1.))
                        })
                        .child(
                            svg()
                                .path(action.icon)
                                .size(px(14.))
                                .text_color(rgb(palette.text_muted)),
                        )
                        .on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            (action.handler)(event, window, cx);
                        })
                        .render(window, cx),
                )
            })
    }
}

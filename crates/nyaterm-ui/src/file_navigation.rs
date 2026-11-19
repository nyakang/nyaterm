//! Stateless file-navigation presentation. Hosts retain path identities and I/O.

use crate::{NyaTooltip, ThemePalette};
use gpui::{
    ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, SharedString, div, prelude::*,
    px, rgb, svg,
};
use nyaterm_core::truncate_preview;
use std::rc::Rc;

#[derive(Clone, Copy)]
pub struct PathMetrics {
    pub font_size: f32,
    pub line_height: f32,
    pub control_height: f32,
    pub children_width: f32,
    pub max_label_width: f32,
}

impl PathMetrics {
    pub fn new(ui_font_size: f32) -> Self {
        let font_size = (ui_font_size * 0.75).max(10.);
        let line_height = font_size * 1.5;
        Self {
            font_size,
            line_height,
            control_height: (line_height + 4.).max(22.),
            children_width: (font_size * 0.8).max(16.),
            max_label_width: 128. * font_size / 10.,
        }
    }
    pub fn row_height(self) -> f32 {
        self.control_height + 5.
    }
    pub fn segment_width(self, label: &str) -> f32 {
        (truncate_preview(label, 18).chars().count() as f32 * self.font_size * 0.6 + 8.)
            .min(self.max_label_width)
            + self.children_width
    }
}

/// Original indices survive label truncation and overflow folding.
pub fn collapsed_indices(
    labels: &[&str],
    width: f32,
    metrics: PathMetrics,
) -> (Vec<usize>, Vec<usize>) {
    let mut shown = vec![true; labels.len()];
    loop {
        let used = labels
            .iter()
            .zip(&shown)
            .filter(|(_, shown)| **shown)
            .map(|(label, _)| metrics.segment_width(label))
            .sum::<f32>()
            + if shown.iter().any(|shown| !shown) {
                metrics.control_height
            } else {
                0.
            };
        if used <= width.max(0.) {
            break;
        }
        let last = labels.len().saturating_sub(1);
        let hide = (1..last.saturating_sub(1))
            .find(|&i| shown[i])
            .or_else(|| (last > 1 && shown[0]).then_some(0))
            .or_else(|| (last > 0 && shown[last - 1]).then_some(last - 1));
        let Some(i) = hide else { break };
        shown[i] = false;
    }
    (0..labels.len()).partition(|&i| shown[i])
}

pub struct Breadcrumb<K> {
    pub key: K,
    pub id: String,
    pub label: String,
    pub children_tooltip: String,
}

pub struct BreadcrumbRow<K> {
    pub prefix: String,
    pub palette: ThemePalette,
    pub display_path: String,
    pub segments: Vec<Breadcrumb<K>>,
    pub expanded: Option<K>,
    pub overflow_label: String,
    pub width: f32,
    pub metrics: PathMetrics,
}

pub enum BreadcrumbMenu<K> {
    Children(K, Option<K>),
    Overflow(Vec<K>),
}

pub fn breadcrumb_row<V: 'static, K: Clone + PartialEq + 'static>(
    model: BreadcrumbRow<K>,
    edit: impl Fn(&mut V, &ClickEvent, &mut gpui::Window, &mut Context<V>) + 'static,
    navigate: impl Fn(&mut V, K, &mut gpui::Window, &mut Context<V>) + 'static,
    menu: impl Fn(&mut V, BreadcrumbMenu<K>, &MouseDownEvent, &mut gpui::Window, &mut Context<V>)
    + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let BreadcrumbRow {
        prefix,
        palette,
        display_path,
        segments,
        expanded,
        overflow_label,
        width,
        metrics,
    } = model;
    let edit = Rc::new(edit);
    let navigate = Rc::new(navigate);
    let menu = Rc::new(menu);
    let selector = format!("{prefix}-path-display");
    let mut row = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .overflow_hidden()
        .text_size(px(metrics.font_size))
        .line_height(px(metrics.line_height))
        .cursor_text()
        .on_click(cx.listener({
            let edit = edit.clone();
            move |v, e, w, cx| edit(v, e, w, cx)
        }))
        .tooltip(move |w, cx| NyaTooltip::new(display_path.clone()).build(w, cx));
    let labels = segments
        .iter()
        .map(|s| s.label.as_str())
        .collect::<Vec<_>>();
    let (visible, hidden) = collapsed_indices(&labels, width, metrics);
    if !hidden.is_empty() {
        let selector = format!("{prefix}-breadcrumb-overflow");
        let keys = hidden
            .iter()
            .map(|&i| segments[i].key.clone())
            .collect::<Vec<_>>();
        let menu = menu.clone();
        row = row.child(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .size(px(metrics.control_height))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(palette.surface_elevated)))
                .tooltip(move |w, cx| NyaTooltip::new(overflow_label.clone()).build(w, cx))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, e, w, cx| {
                        cx.stop_propagation();
                        menu(v, BreadcrumbMenu::Overflow(keys.clone()), e, w, cx);
                    }),
                )
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    svg()
                        .size(px(14.))
                        .path("icons/session/more.svg")
                        .text_color(rgb(palette.text_muted)),
                ),
        );
    }
    let current_width = (width
        - if hidden.is_empty() {
            0.
        } else {
            metrics.control_height
        }
        - visible
            .iter()
            .filter(|&&i| i + 1 != segments.len())
            .map(|&i| metrics.segment_width(&segments[i].label))
            .sum::<f32>()
        - metrics.children_width)
        .clamp(16., metrics.max_label_width);
    for i in visible {
        let segment = &segments[i];
        let current = i + 1 == segments.len();
        let is_expanded = expanded.as_ref() == Some(&segment.key);
        let key = segment.key.clone();
        let child_key = key.clone();
        let branch = segments.get(i + 1).map(|s| s.key.clone());
        let label_selector = format!("{prefix}-breadcrumb-label-{}", segment.id);
        let children_selector = format!("{prefix}-breadcrumb-children-{}", segment.id);
        let tooltip = segment.children_tooltip.clone();
        let edit = edit.clone();
        let navigate = navigate.clone();
        let menu = menu.clone();
        row = row.child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .child(
                    div()
                        .id(SharedString::from(label_selector.clone()))
                        .debug_selector(move || label_selector.clone())
                        .h(px(metrics.control_height))
                        .max_w(px(if current {
                            current_width
                        } else {
                            metrics.max_label_width
                        }))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .rounded_l_sm()
                        .text_color(rgb(if current {
                            palette.text
                        } else {
                            palette.text_muted
                        }))
                        .cursor_pointer()
                        .hover(move |s| {
                            s.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .on_click(cx.listener(move |v, e, w, cx| {
                            cx.stop_propagation();
                            if current {
                                edit(v, e, w, cx);
                            } else {
                                navigate(v, key.clone(), w, cx);
                            }
                        }))
                        .child(truncate_preview(&segment.label, 18)),
                )
                .child(
                    div()
                        .id(SharedString::from(children_selector.clone()))
                        .debug_selector(move || children_selector.clone())
                        .h(px(metrics.control_height))
                        .w(px(metrics.children_width))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_r_sm()
                        .when(is_expanded, |s| s.bg(rgb(palette.surface_elevated)))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(palette.surface_elevated)))
                        .tooltip(move |w, cx| NyaTooltip::new(tooltip.clone()).build(w, cx))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, e, w, cx| {
                                cx.stop_propagation();
                                menu(
                                    v,
                                    BreadcrumbMenu::Children(child_key.clone(), branch.clone()),
                                    e,
                                    w,
                                    cx,
                                );
                            }),
                        )
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            svg()
                                .size(px(11.))
                                .path(if is_expanded {
                                    "icons/chevron-down.svg"
                                } else {
                                    "icons/menu/chevron-right.svg"
                                })
                                .text_color(rgb(palette.text_muted)),
                        ),
                ),
        );
    }
    row
}

pub fn search_overlay(palette: ThemePalette, input: impl IntoElement) -> gpui::Div {
    div()
        .absolute()
        .top(px(4.))
        .bottom(px(4.))
        .left(px(6.))
        .right(px(6.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.primary))
        .bg(rgb(palette.surface))
        .shadow_lg()
        .occlude()
        .flex()
        .items_center()
        .child(input)
}

pub fn path_icon(
    palette: ThemePalette,
    id: impl Into<SharedString>,
    icon: &'static str,
    label: impl Into<SharedString>,
    size: f32,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let id = id.into();
    let selector = id.to_string();
    let label = label.into();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |row| row.bg(rgb(palette.surface_elevated)))
        .tooltip(move |window, cx| NyaTooltip::new(label.clone()).build(window, cx))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon)
                .text_color(rgb(if active {
                    palette.link
                } else {
                    palette.text_muted
                })),
        )
}

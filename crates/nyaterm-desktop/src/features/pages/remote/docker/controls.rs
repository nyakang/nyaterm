use std::collections::HashSet;

use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, MouseButton, Window, div, prelude::*, px,
    rgb,
};
use nyaterm_transport::RemoteDockerOverview;
use nyaterm_ui::{NyaTabItem, NyaTabs};

use super::super::panels::RemoteMonitorPanel;
use crate::features::shell::gpui_code_font_family;
use crate::models::DockerTab;
use crate::theme::ThemePalette;

use super::{DockerRenderContext, docker_tab_menu_layer};

fn tab_label(label: String, count: usize, loaded: bool) -> String {
    if loaded {
        format!("{label} {count}")
    } else {
        label
    }
}

pub(in crate::features::pages::remote) struct DockerTabBarLabels {
    pub tabs: [String; 5],
    pub more: String,
}

pub(in crate::features::pages::remote) struct DockerTabBarState<'a> {
    pub active_tab: DockerTab,
    pub overview: &'a RemoteDockerOverview,
    pub loaded_resources: &'a HashSet<DockerTab>,
}

pub(in crate::features::pages::remote) fn docker_overview_strip(
    palette: ThemePalette,
    overview: &RemoteDockerOverview,
    images_loaded: bool,
    labels: [String; 3],
) -> impl IntoElement {
    let [running_label, stopped_label, images_label] = labels;
    let running = overview
        .containers
        .iter()
        .filter(|container| container.state.eq_ignore_ascii_case("running"))
        .count();
    let stopped = overview.containers.len().saturating_sub(running);

    div()
        .h(px(30.))
        .flex_none()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.section_header))
        .px_1()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(docker_overview_stat(
            palette,
            running_label,
            Some(running),
            Some(0x86efac),
        ))
        .child(docker_overview_stat(
            palette,
            stopped_label,
            Some(stopped),
            Some(0xcbd5e1),
        ))
        .child(docker_overview_stat(
            palette,
            images_label,
            images_loaded.then_some(overview.images.len()),
            None,
        ))
}

fn docker_overview_stat(
    palette: ThemePalette,
    label: String,
    value: Option<usize>,
    accent: Option<u32>,
) -> impl IntoElement {
    div()
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .gap_1()
        .text_size(px(10.))
        .text_color(accent.map(rgb).unwrap_or_else(|| rgb(palette.text_muted)))
        .child(
            div()
                .flex_none()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .font_family(gpui_code_font_family())
                .text_size(px(11.))
                .font_weight(FontWeight(600.))
                .child(value.map_or_else(|| "-".to_string(), |value| value.to_string())),
        )
}

pub(in crate::features::pages::remote) fn docker_tab_bar(
    context: DockerRenderContext,
    state: DockerTabBarState<'_>,
    labels: DockerTabBarLabels,
    panel_width: f32,
    menu_open: bool,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerTabBarState {
        active_tab,
        overview,
        loaded_resources,
    } = state;
    let DockerRenderContext {
        palette, menu_bg, ..
    } = context;
    let DockerTabBarLabels {
        tabs: labels,
        more: more_label,
    } = labels;
    let [
        containers_label,
        images_label,
        volumes_label,
        networks_label,
        compose_label,
    ] = labels;
    let mut tabs = vec![
        (
            DockerTab::Containers,
            tab_label(containers_label, overview.containers.len(), true),
        ),
        (
            DockerTab::Images,
            tab_label(
                images_label,
                overview.images.len(),
                loaded_resources.contains(&DockerTab::Images),
            ),
        ),
        (
            DockerTab::Volumes,
            tab_label(
                volumes_label,
                overview.volumes.len(),
                loaded_resources.contains(&DockerTab::Volumes),
            ),
        ),
        (
            DockerTab::Networks,
            tab_label(
                networks_label,
                overview.networks.len(),
                loaded_resources.contains(&DockerTab::Networks),
            ),
        ),
    ];
    if overview.compose_available {
        tabs.push((
            DockerTab::Compose,
            tab_label(
                compose_label,
                overview.compose_projects.len(),
                loaded_resources.contains(&DockerTab::Compose),
            ),
        ));
    }
    // Tauri switches overflowed tabs into a More menu. These thresholds keep
    // the GPUI tab strip stable while preserving access to every tab.
    let visible_count = if panel_width > 0. && panel_width < 300. {
        1
    } else if panel_width > 0. && panel_width < 390. {
        2
    } else if panel_width > 0. && panel_width < 500. {
        3
    } else if panel_width > 0. && panel_width < 620. {
        4
    } else {
        tabs.len()
    };
    let visible_count = visible_count.min(tabs.len());
    let visible_tabs = &tabs[..visible_count];
    let hidden_tabs = &tabs[visible_count..];
    let more_active = hidden_tabs.iter().any(|(tab, _)| *tab == active_tab);
    let visible_tab_values = visible_tabs.iter().map(|(tab, _)| *tab).collect::<Vec<_>>();
    let mut bar = div()
        .id("docker-tab-bar")
        .relative()
        .h(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .gap_1();
    bar = bar.child(
        div().min_w_0().flex_1().child(
            NyaTabs::new("docker-tabs")
                .items(
                    visible_tabs
                        .iter()
                        .map(|(_, label)| NyaTabItem::new((*label).clone())),
                )
                .selected_index_if_visible(
                    visible_tabs.iter().position(|(tab, _)| *tab == active_tab),
                )
                .on_select(cx.listener(move |panel, index: &usize, _, cx| {
                    panel.with_app(cx, |this, cx| {
                        let Some(tab) = visible_tab_values.get(*index).copied() else {
                            return;
                        };
                        this.set_docker_tab(tab, cx);
                    });
                })),
        ),
    );
    if !hidden_tabs.is_empty() {
        let mut more = div()
            .relative()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(docker_tab_button(
                palette,
                "docker-tab-more",
                more_label,
                menu_open || more_active,
                cx.listener(|panel, _, _, cx| {
                    panel.with_app(cx, |this, cx| {
                        this.toggle_docker_tab_menu(cx);
                    });
                }),
            ));
        if menu_open {
            let mut menu = div()
                .id("docker-tab-more-menu")
                .w(px(160.))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(menu_bg)
                .shadow_lg()
                .py_1()
                .flex()
                .flex_col()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (index, (tab, label)) in hidden_tabs.iter().enumerate() {
                let tab = *tab;
                let compose_disabled = tab == DockerTab::Compose && !overview.compose_available;
                menu = menu.child(docker_tab_menu_item(
                    palette,
                    format!("docker-tab-more-{index}"),
                    label.clone(),
                    active_tab == tab,
                    compose_disabled,
                    cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |this, cx| {
                            this.set_docker_tab(tab, cx);
                        });
                    }),
                ));
            }
            more = more.child(docker_tab_menu_layer(menu));
        }
        bar = bar.child(more);
    }
    bar
}

fn docker_tab_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: String,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(gpui::SharedString::from(id.into()))
        .h(px(24.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .bg(if active {
            rgb(palette.surface_elevated)
        } else {
            rgb(palette.bg)
        })
        .text_color(if active {
            rgb(palette.text)
        } else {
            rgb(palette.text_muted)
        })
        .text_size(px(11.))
        .font_weight(if active {
            FontWeight(600.)
        } else {
            FontWeight(500.)
        })
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
        .child(label)
        .on_click(on_click)
}

fn docker_tab_menu_item(
    palette: ThemePalette,
    id: String,
    label: String,
    active: bool,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(gpui::SharedString::from(id))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .text_size(px(11.))
        .text_color(if disabled {
            rgb(palette.text_dimmed)
        } else {
            rgb(palette.text)
        })
        .bg(if active {
            rgb(palette.hover)
        } else {
            rgb(palette.surface_elevated)
        })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
        })
        .child(label)
        .when(!disabled, |this| this.on_click(on_click))
}

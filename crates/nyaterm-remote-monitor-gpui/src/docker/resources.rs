use super::super::panels::RemoteMonitorPanel;
use super::DockerLabels;
use crate::actions::RemoteMonitorAction;
use crate::models::{DockerConfirmAction, DockerConfirmState};
use gpui::{
    Context, FontWeight, IntoElement, SharedString, UniformListScrollHandle, div, prelude::*, px,
    rgb, uniform_list,
};
use nyaterm_remote_ops::docker::{DockerImage, DockerNetwork, DockerVolume};
use nyaterm_ui::ThemePalette;
use nyaterm_ui::presentation::{compact_id, gpui_code_font_family};
use nyaterm_ui::{NyaScrollable, NyaTooltip};
use nyaterm_ui::{empty_panel, svg_icon_button};
use std::sync::Arc;

const DOCKER_RESOURCE_ROW_PX: f32 = 64.;

pub(crate) fn docker_images_panel(
    palette: ThemePalette,
    images: Arc<[DockerImage]>,
    labels: DockerLabels,
    scroll: UniformListScrollHandle,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    if images.is_empty() {
        return docker_resource_empty(palette, "Images", labels.no_matches.clone());
    }

    let rows = uniform_list(
        "docker-image-rows",
        images.len(),
        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .filter_map(|index| images.get(index).cloned().map(|image| (index, image)))
                .map(|(row_index, image)| {
                    let image_id = image.id.clone();
                    let label = docker_image_label(&image);
                    let row_labels = labels.clone();
                    div()
                        .h(px(DOCKER_RESOURCE_ROW_PX))
                        .pb(px(6.))
                        .flex_none()
                        .child(
                            docker_resource_row(
                                palette,
                                label.clone(),
                                format!(
                                    "{} · {} · {}",
                                    compact_id(&image.id),
                                    image.created_since,
                                    image.size
                                ),
                            )
                            .child(svg_icon_button(
                                format!(
                                    "docker-image-remove-{row_index}-{}",
                                    compact_id(&image_id)
                                ),
                                "icons/fe/delete.svg",
                                14.,
                                SharedString::from(row_labels.remove_image.to_string()),
                                palette,
                                cx.listener(move |panel, _, window, cx| {
                                    {
                                        panel.dispatch_in(
                                            RemoteMonitorAction::ConfirmDocker(
                                                DockerConfirmState {
                                                    title: row_labels
                                                        .confirm_action_title
                                                        .to_string(),
                                                    detail: row_labels.confirm_description(
                                                        &row_labels.remove_image,
                                                        &label,
                                                    ),
                                                    action: DockerConfirmAction::ImageRemove {
                                                        image_id: image_id.clone(),
                                                        force: false,
                                                    },
                                                },
                                            ),
                                            window,
                                            cx,
                                        );
                                    };
                                }),
                            )),
                        )
                        .into_any_element()
                })
                .collect()
        }),
    )
    .size_full()
    .track_scroll(&scroll);
    docker_resource_panel("Images", rows, &scroll)
}

pub(crate) fn docker_volumes_panel(
    palette: ThemePalette,
    volumes: Arc<[DockerVolume]>,
    labels: DockerLabels,
    scroll: UniformListScrollHandle,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    if volumes.is_empty() {
        return docker_resource_empty(palette, "Volumes", labels.no_matches.clone());
    }

    let rows = uniform_list(
        "docker-volume-rows",
        volumes.len(),
        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .filter_map(|index| volumes.get(index).cloned())
                .map(|volume| {
                    let volume_name = volume.name.clone();
                    let row_labels = labels.clone();
                    div()
                        .h(px(DOCKER_RESOURCE_ROW_PX))
                        .pb(px(6.))
                        .flex_none()
                        .child(
                            docker_resource_row(
                                palette,
                                volume.name,
                                row_labels.volume_driver_label(&volume.driver),
                            )
                            .child(svg_icon_button(
                                format!("docker-volume-remove-{volume_name}"),
                                "icons/fe/delete.svg",
                                14.,
                                SharedString::from(row_labels.remove_volume.to_string()),
                                palette,
                                cx.listener(move |panel, _, window, cx| {
                                    {
                                        panel.dispatch_in(
                                            RemoteMonitorAction::ConfirmDocker(
                                                DockerConfirmState {
                                                    title: row_labels
                                                        .confirm_action_title
                                                        .to_string(),
                                                    detail: row_labels.confirm_description(
                                                        &row_labels.remove_volume,
                                                        &volume_name,
                                                    ),
                                                    action: DockerConfirmAction::VolumeRemove {
                                                        volume_name: volume_name.clone(),
                                                        force: false,
                                                    },
                                                },
                                            ),
                                            window,
                                            cx,
                                        );
                                    };
                                }),
                            )),
                        )
                        .into_any_element()
                })
                .collect()
        }),
    )
    .size_full()
    .track_scroll(&scroll);
    docker_resource_panel("Volumes", rows, &scroll)
}

pub(crate) fn docker_networks_panel(
    palette: ThemePalette,
    networks: Arc<[DockerNetwork]>,
    labels: DockerLabels,
    scroll: UniformListScrollHandle,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    if networks.is_empty() {
        return docker_resource_empty(palette, "Networks", labels.no_matches.clone());
    }

    let rows = uniform_list(
        "docker-network-rows",
        networks.len(),
        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .filter_map(|index| networks.get(index).cloned())
                .map(|network| {
                    let network_id = network.id.clone();
                    let name = network.name.clone();
                    let row_labels = labels.clone();
                    div()
                        .h(px(DOCKER_RESOURCE_ROW_PX))
                        .pb(px(6.))
                        .flex_none()
                        .child(
                            docker_resource_row(
                                palette,
                                network.name,
                                format!(
                                    "{} · {} · {}",
                                    compact_id(&network.id),
                                    network.driver,
                                    network.scope
                                ),
                            )
                            .child(svg_icon_button(
                                format!("docker-network-remove-{}", compact_id(&network_id)),
                                "icons/fe/delete.svg",
                                14.,
                                SharedString::from(row_labels.remove_network.to_string()),
                                palette,
                                cx.listener(move |panel, _, window, cx| {
                                    {
                                        panel.dispatch_in(
                                            RemoteMonitorAction::ConfirmDocker(
                                                DockerConfirmState {
                                                    title: row_labels
                                                        .confirm_action_title
                                                        .to_string(),
                                                    detail: row_labels.confirm_description(
                                                        &row_labels.remove_network,
                                                        &name,
                                                    ),
                                                    action: DockerConfirmAction::NetworkRemove {
                                                        network_id: network_id.clone(),
                                                    },
                                                },
                                            ),
                                            window,
                                            cx,
                                        );
                                    };
                                }),
                            )),
                        )
                        .into_any_element()
                })
                .collect()
        }),
    )
    .size_full()
    .track_scroll(&scroll);
    docker_resource_panel("Networks", rows, &scroll)
}

fn docker_resource_empty(
    palette: ThemePalette,
    title: &'static str,
    message: impl Into<SharedString>,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!(
            "docker-resource-{}",
            title.to_ascii_lowercase()
        )))
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(empty_panel(message, palette))
        .into_any_element()
}

pub(crate) fn docker_resource_panel(
    title: &'static str,
    rows: impl IntoElement,
    scroll: &UniformListScrollHandle,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!(
            "docker-resource-{}",
            title.to_ascii_lowercase()
        )))
        .relative()
        .size_full()
        .overflow_hidden()
        .px_2()
        .pb_2()
        .child(rows)
        .vertical_scrollbar(scroll)
        .into_any_element()
}

pub(crate) fn docker_resource_static_panel(
    _palette: ThemePalette,
    title: &'static str,
    _count: usize,
    rows: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "docker-resource-{}",
            title.to_ascii_lowercase()
        )))
        .size_full()
        .overflow_scrollbar()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(rows)
}

pub(crate) fn docker_resource_row(
    palette: ThemePalette,
    title: String,
    detail: String,
) -> gpui::Div {
    let title_tooltip = title.clone();
    let detail_tooltip = detail.clone();
    // ~64px Tauri SIMPLE_ROW_HEIGHT-ish dense resource row (slightly tighter chrome).
    div()
        .h(px(58.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .px_3()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .hover(|this| this.bg(rgb(palette.hover)))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "resource-title-{title}-{detail}"
                        )))
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .truncate()
                        .tooltip(move |window, cx| {
                            NyaTooltip::new(title_tooltip.clone()).build(window, cx)
                        })
                        .child(title.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "resource-detail-{title}-{detail}"
                        )))
                        .font_family(gpui_code_font_family())
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_dimmed))
                        .truncate()
                        .tooltip(move |window, cx| {
                            NyaTooltip::new(detail_tooltip.clone()).build(window, cx)
                        })
                        .child(detail),
                ),
        )
}

pub(crate) fn docker_image_label(image: &DockerImage) -> String {
    match (
        image.repository.trim().is_empty(),
        image.tag.trim().is_empty(),
    ) {
        (true, true) => compact_id(&image.id),
        (false, true) => image.repository.clone(),
        (true, false) => image.tag.clone(),
        (false, false) => format!("{}:{}", image.repository, image.tag),
    }
}

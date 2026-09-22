use rust_i18n::t;

use gpui::{Context, IntoElement, UniformListScrollHandle, div, prelude::*, px};

use gpui::Entity;
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaInputState, NyaSearchInput};

use super::panels::{PanelChrome, RemoteMonitorPanel};
use crate::features::remote::{DockerDerivedItems, DockerPresentationState};
use crate::models::DockerTab;
use crate::widgets::empty_panel_with_icon;

use super::docker::{
    DockerComposePanelState, DockerContainersPanelState, DockerRenderContext, DockerTabBarLabels,
    DockerTabBarState, docker_compose_panel, docker_containers_panel, docker_images_panel,
    docker_labels, docker_networks_panel, docker_overview_strip, docker_tab_bar,
    docker_volumes_panel,
};

/// The Docker panel, rendered from a snapshot.
///
/// Takes no `NyaTermApp`. GPUI records every entity read during a view's render as a
/// dependency of that view, so a single app read here would re-dirty this panel on every
/// unrelated `app.notify()`.
#[allow(clippy::too_many_arguments)]
pub(in crate::features::pages::remote) fn docker_panel(
    chrome: PanelChrome,
    has_session: bool,
    mut docker: DockerPresentationState,
    filtered: DockerDerivedItems,
    active_tab: DockerTab,
    panel_width: f32,
    search: Entity<NyaInputState>,
    container_scroll: UniformListScrollHandle,
    resource_scroll: UniformListScrollHandle,
    cx: &mut Context<RemoteMonitorPanel>,
) -> gpui::AnyElement {
    let palette = chrome.palette;
    let labels = docker_labels();
    // Built before the view, which reads `self` throughout: creating the
    // box needs it mutably.
    // Built from the handle the snapshot carries. Reading that entity here is wanted:
    // typing notifies it, which invalidates this panel and nothing else.
    let docker_search_input =
        NyaSearchInput::new("remote.docker.filter", &search).into_any_element();
    if !has_session {
        return div()
            .size_full()
            .bg(chrome.transparent_surface)
            .child(empty_panel_with_icon(
                labels.no_session.clone(),
                palette,
                "icons/docker.svg",
            ))
            .into_any_element();
    }
    let Some(overview) = docker.overview.take() else {
        let message = if docker.pending || !docker.status.contains("failed") {
            labels.loading.clone()
        } else {
            let detail = docker
                .status
                .strip_prefix("Docker operation failed:")
                .unwrap_or(&docker.status)
                .trim();
            format!("{}\n{}", labels.error, truncate_preview(detail, 240)).into()
        };
        return div()
            .size_full()
            .bg(chrome.transparent_surface)
            .child(empty_panel_with_icon(message, palette, "icons/docker.svg"))
            .into_any_element();
    };
    if !overview.available {
        return div()
            .size_full()
            .bg(chrome.transparent_surface)
            .child(empty_panel_with_icon(
                labels.unavailable.clone(),
                palette,
                "icons/docker.svg",
            ))
            .into_any_element();
    }
    // Both the effective tab and the filtered list are resolved by
    // `RemoteOpsFeatureState`, which recomputes them when the overview, the query
    // or the tab changes. This pass only reads them.
    let query_empty = docker.search_draft.trim().is_empty();
    let menu_bg = chrome.surface;
    let render_context = DockerRenderContext {
        palette,
        menu_bg,
        labels: labels.clone(),
    };
    let docker_content = match filtered {
        DockerDerivedItems::Containers(filtered) => docker_containers_panel(
            render_context.clone(),
            DockerContainersPanelState {
                has_snapshot: true,
                has_session,
                docker_available: overview.available,
                filtered_containers: filtered,
                query_empty,
                open_menu_id: docker.container_menu_id.clone(),
            },
            container_scroll,
            cx,
        )
        .into_any_element(),
        DockerDerivedItems::Images(filtered) => docker_images_panel(
            palette,
            filtered,
            labels.clone(),
            resource_scroll.clone(),
            cx,
        )
        .into_any_element(),
        DockerDerivedItems::Volumes(filtered) => docker_volumes_panel(
            palette,
            filtered,
            labels.clone(),
            resource_scroll.clone(),
            cx,
        )
        .into_any_element(),
        DockerDerivedItems::Networks(filtered) => {
            docker_networks_panel(palette, filtered, labels.clone(), resource_scroll, cx)
                .into_any_element()
        }
        DockerDerivedItems::Compose(filtered) => docker_compose_panel(
            render_context.clone(),
            DockerComposePanelState {
                projects: filtered.as_ref(),
                expanded_projects: &docker.compose_expanded,
                services_by_project: &docker.compose_services,
                service_errors: &docker.compose_service_errors,
                open_menu_id: docker.compose_menu_id.as_deref(),
            },
            cx,
        )
        .into_any_element(),
    };
    let docker_content =
        if active_tab != DockerTab::Containers && !docker.loaded_resources.contains(&active_tab) {
            let message = if docker.status.contains("failed") {
                format!(
                    "{}\n{}",
                    labels.error,
                    truncate_preview(&docker.status, 240)
                )
                .into()
            } else {
                labels.loading.clone()
            };
            empty_panel_with_icon(message, palette, "icons/docker.svg").into_any_element()
        } else {
            docker_content
        };

    // Tauri DockerManager shell: header actions + dense search + tabs + flex list body.
    // Shared PanelHeader already shows title/meta; avoid page-like section headers.
    div()
        .flex()
        .flex_col()
        .size_full()
        .relative()
        .overflow_hidden()
        .p(px(8.))
        .gap(px(8.))
        .bg(chrome.transparent_surface)
        .when(overview.available, |this| {
            this.child(docker_overview_strip(
                palette,
                &overview,
                docker.loaded_resources.contains(&DockerTab::Images),
                [
                    t!("dockerManager.running").to_string(),
                    t!("dockerManager.stopped").to_string(),
                    t!("dockerManager.images").to_string(),
                ],
            ))
        })
        .child(
            div()
                .h(px(32.))
                .flex_none()
                .flex()
                .items_center()
                .child(div().flex_1().min_w_0().child(docker_search_input)),
        )
        .child(docker_tab_bar(
            render_context,
            DockerTabBarState {
                active_tab,
                overview: &overview,
                loaded_resources: &docker.loaded_resources,
            },
            DockerTabBarLabels {
                tabs: [
                    t!("dockerManager.containers").to_string(),
                    t!("dockerManager.images").to_string(),
                    t!("dockerManager.volumes").to_string(),
                    t!("dockerManager.networks").to_string(),
                    t!("dockerManager.compose").to_string(),
                ],
                more: t!("common.more").to_string(),
            },
            panel_width,
            docker.tab_menu_open,
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(docker_content),
        )
        .into_any_element()
}

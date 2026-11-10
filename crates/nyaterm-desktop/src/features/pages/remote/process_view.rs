use super::panels::{PanelChrome, RemoteMonitorPanel};
use super::process::{
    ProcessDetailLabels, ProcessDisplayMode, ProcessTableLabels, ProcessTableRowActions,
    ProcessTableRowPresentation, process_column, process_details, process_display_mode,
    process_sort_button, process_table_row,
};
use crate::features::remote::ProcessPresentationState;
use crate::features::text_inputs::number_input_box_from_state;
use crate::models::RemoteProcessSortKey;
use crate::widgets::empty_panel_with_icon;
use gpui::{Context, Entity, IntoElement, ListState, SharedString, div, list, prelude::*, px, rgb};
use nyaterm_remote_ops::process_control::ProcessControlCapabilities;
use nyaterm_remote_ops::remote_process::{PROCESS_LIST_UNSUPPORTED_ERROR, RemoteProcess};
use nyaterm_ui::{NyaInputState, NyaNumberInputState, NyaScrollable, NyaSearchInput};
use rust_i18n::t;
use std::borrow::Cow;
use std::sync::Arc;

/// The Processes panel, rendered from a snapshot.
///
/// Takes no `NyaTermApp`. GPUI records every entity read during a view's render as a
/// dependency of that view, so a single app read here would re-dirty this panel on every
/// unrelated `app.notify()`.
#[allow(clippy::too_many_arguments)]
pub(in crate::features::pages::remote) fn processes_panel(
    chrome: PanelChrome,
    capabilities: ProcessControlCapabilities,
    host_identity: crate::features::remote::remote_runtime::host_tools::HostInteractionIdentity,
    has_session: bool,
    process_state: ProcessPresentationState,
    filtered_processes: Arc<[RemoteProcess]>,
    panel_width: f32,
    search: Entity<NyaInputState>,
    nice: Option<Entity<NyaNumberInputState>>,
    process_list: ListState,
    cx: &mut Context<RemoteMonitorPanel>,
) -> gpui::AnyElement {
    let palette = chrome.palette;
    // Built from the handle the snapshot carries. Reading that entity here is wanted:
    // typing notifies it, which invalidates this panel and nothing else.
    let process_search_input =
        NyaSearchInput::new("remote.process.filter", &search).into_any_element();
    if !has_session {
        return div()
            .size_full()
            .bg(chrome.transparent_surface)
            .child(empty_panel_with_icon(
                t!("processManager.noSession"),
                palette,
                "icons/processes.svg",
            ))
            .into_any_element();
    }
    if !process_state.snapshot_loaded {
        let message = if process_state.pending || !process_state.status.contains("failed") {
            t!("common.loading")
        } else if process_state
            .status
            .contains(PROCESS_LIST_UNSUPPORTED_ERROR)
        {
            t!("processManager.unsupported")
        } else {
            t!("processManager.error")
        };
        return div()
            .size_full()
            .bg(chrome.transparent_surface)
            .child(empty_panel_with_icon(
                message,
                palette,
                "icons/processes.svg",
            ))
            .into_any_element();
    }
    let menu_bg = chrome.surface;
    let table_labels = ProcessTableLabels {
        more: t!("common.more"),
        copy_pid: t!("processManager.copyPid"),
        copy_command: t!("processManager.copyCommand"),
        signal_term: t!("processManager.signalTerm"),
        signal_hup: t!("processManager.signalHup"),
        signal_stop: t!("processManager.signalStop"),
        signal_cont: t!("processManager.signalCont"),
        signal_kill: t!("processManager.signalKill"),
    };
    let detail_labels = ProcessDetailLabels {
        cpu: t!("processManager.sortCpu"),
        memory: t!("resourceMonitor.memory"),
        rss: Cow::Borrowed("RSS"),
        elapsed: t!("processManager.elapsed"),
        copy_command: t!("processManager.copyCommand"),
        apply_nice: t!("processManager.applyNice"),
    };
    let mode = process_display_mode(panel_width);
    // Filtering and sorting are owned by the feature state, independently of width.

    let total_filtered = filtered_processes.len();
    let rows = if filtered_processes.is_empty() {
        empty_panel_with_icon(
            t!("processManager.noMatches"),
            palette,
            "icons/processes.svg",
        )
        .into_any_element()
    } else {
        let rows_processes = filtered_processes.clone();
        let rows_state = process_state.clone();
        let rows_table_labels = table_labels.clone();
        let rows_detail_labels = detail_labels.clone();
        let rows_nice = nice.clone();
        list(
            process_list.clone(),
            cx.processor(move |_panel, index: usize, _, cx| {
                let Some(process) = rows_processes.get(index).cloned() else {
                    return div().into_any_element();
                };
                let pid = process.pid;
                let selected = rows_state.selected_pid == Some(pid);
                let nice_input = selected.then(|| rows_nice.clone()).flatten().map(|field| {
                    number_input_box_from_state(
                        SharedString::from(format!("remote.process.{pid}.nice")),
                        palette,
                        field,
                    )
                    .into_any_element()
                });
                let details = if selected {
                    let value = if process.command_line.trim().is_empty() {
                        process.command.clone()
                    } else {
                        process.command_line.clone()
                    };
                    process_details(
                        palette,
                        host_identity.clone(),
                        &process,
                        mode,
                        rows_detail_labels.clone(),
                        nice_input,
                        cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |this, cx| {
                                this.copy_process_text(value.clone(), "command", cx);
                            });
                        }),
                        cx,
                    )
                } else {
                    div().into_any_element()
                };

                process_table_row(
                    ProcessTableRowPresentation {
                        palette,
                        capabilities,
                        menu_bg,
                        mode,
                        labels: rows_table_labels.clone(),
                        selected,
                        menu_open: rows_state.menu_pid == Some(pid),
                    },
                    &process,
                    ProcessTableRowActions {
                        on_select: cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |this, cx| {
                                this.remote_ops.close_process_menu();
                                this.toggle_process_selection(pid, cx);
                            });
                        }),
                        on_menu: cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |this, cx| {
                                cx.stop_propagation();
                                this.remote_ops.toggle_process_menu(pid);
                                cx.notify();
                            });
                        }),
                        on_copy_pid: cx.listener({
                            let value = pid.to_string();
                            move |panel, _, _, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.remote_ops.close_process_menu();
                                    this.copy_process_text(value.clone(), "pid", cx);
                                });
                            }
                        }),
                        on_copy_command: cx.listener({
                            let value = if process.command_line.trim().is_empty() {
                                process.command.clone()
                            } else {
                                process.command_line.clone()
                            };
                            move |panel, _, _, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.remote_ops.close_process_menu();
                                    this.copy_process_text(value.clone(), "command", cx);
                                });
                            }
                        }),
                        on_term: cx.listener({
                            let identity = host_identity.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if this.host_interaction_identity(cx) == identity {
                                        this.remote_ops.close_process_menu();
                                        this.request_process_signal(pid, "TERM", window, cx);
                                    }
                                });
                            }
                        }),
                        on_hup: cx.listener({
                            let identity = host_identity.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if this.host_interaction_identity(cx) == identity {
                                        this.remote_ops.close_process_menu();
                                        this.request_process_signal(pid, "HUP", window, cx);
                                    }
                                });
                            }
                        }),
                        on_stop: cx.listener({
                            let identity = host_identity.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if this.host_interaction_identity(cx) == identity {
                                        this.remote_ops.close_process_menu();
                                        this.request_process_signal(pid, "STOP", window, cx);
                                    }
                                });
                            }
                        }),
                        on_cont: cx.listener({
                            let identity = host_identity.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if this.host_interaction_identity(cx) == identity {
                                        this.remote_ops.close_process_menu();
                                        this.request_process_signal(pid, "CONT", window, cx);
                                    }
                                });
                            }
                        }),
                        on_kill: cx.listener({
                            let identity = host_identity.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if this.host_interaction_identity(cx) == identity {
                                        this.remote_ops.close_process_menu();
                                        this.request_process_signal(pid, "KILL", window, cx);
                                    }
                                });
                            }
                        }),
                    },
                )
                .child(details)
                .into_any_element()
            }),
        )
        .size_full()
        .into_any_element()
    };

    let sort_options = [
        (RemoteProcessSortKey::Command, t!("processManager.process")),
        (RemoteProcessSortKey::Pid, t!("processManager.sortPid")),
        (RemoteProcessSortKey::Cpu, t!("processManager.sortCpu")),
        (
            RemoteProcessSortKey::Memory,
            t!("processManager.sortMemory"),
        ),
        (RemoteProcessSortKey::User, t!("processManager.user")),
    ];
    let sort_label = sort_options
        .iter()
        .find(|(key, _)| *key == process_state.sort_key)
        .map(|(_, label)| format!("{} {}", label, process_state.sort_direction.marker()))
        .unwrap_or_default();
    let sort_menu = nyaterm_ui::NyaDropdownMenu::new("process-sort")
        .label(sort_label)
        .items(sort_options.iter().map(|(key, label)| {
            let key = *key;
            nyaterm_ui::NyaMenuItem::action(label.to_string())
                .checked(key == process_state.sort_key)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| app.toggle_process_sort(key, cx));
                }))
        }));
    let mut header = div()
        .h(px(32.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .border_b_1()
        .border_color(rgb(palette.border));
    for (key, label) in sort_options {
        if key == RemoteProcessSortKey::User && mode != ProcessDisplayMode::Wide {
            continue;
        }
        let width = match key {
            RemoteProcessSortKey::Command => None,
            RemoteProcessSortKey::Pid => Some(72.),
            RemoteProcessSortKey::Cpu | RemoteProcessSortKey::Memory => Some(64.),
            RemoteProcessSortKey::User => Some(72.),
        };
        header = header.child(process_column(
            width,
            process_sort_button(
                palette,
                format!("process-sort-{}", key.label()),
                label,
                key == process_state.sort_key,
                process_state.sort_direction,
                matches!(
                    key,
                    RemoteProcessSortKey::Pid
                        | RemoteProcessSortKey::Cpu
                        | RemoteProcessSortKey::Memory
                ),
                cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| app.toggle_process_sort(key, cx));
                }),
            ),
        ));
    }
    header = header.child(process_column(Some(28.), div()));
    let count_label = if process_state.search_draft.trim().is_empty() {
        process_state.items.len().to_string()
    } else {
        format!("{} / {}", total_filtered, process_state.items.len())
    };
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .p(px(10.))
        .gap_2()
        .bg(chrome.transparent_surface)
        .child(
            div()
                .h(px(32.))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(process_search_input),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(count_label),
                ),
        )
        .child(div().h(px(28.)).flex_none().min_w_0().child(sort_menu))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .when(
                    matches!(mode, ProcessDisplayMode::Medium | ProcessDisplayMode::Wide),
                    |this| this.child(header),
                )
                .child(
                    div()
                        .id("process-list-scroll")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .child(rows)
                        .when(total_filtered > 0, |this| {
                            this.vertical_scrollbar(&process_list)
                        }),
                ),
        )
        .into_any_element()
}

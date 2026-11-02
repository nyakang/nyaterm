use std::sync::Arc;

use nyaterm_transport::{
    DockerContainer, DockerContainerDetails, DockerImage, NetworkInfo, NetworkSummaryInfo,
    RemoteDockerOverview, RemoteGpu, RemoteGpuOverview, RemoteGpuProcess, RemoteNpu,
    RemoteNpuOverview, RemoteNpuProcess, RemoteProcess, RemoteStats,
};

use super::{
    AcceleratorProcessList, DockerDerivedItems, ProcessApplyOutcome, RemoteOpsFeatureFocus,
    RemoteOpsFeatureState, StatsApplyOutcome,
};
use crate::features::remote::job_state::RemoteJobState;
use crate::features::runtime_jobs::{
    DockerResource, ProcessJobOutput, ProcessJobResult, StatsJobResult,
};
use crate::models::{DockerTab, RemoteProcessSortKey};

fn process(pid: u32) -> RemoteProcess {
    RemoteProcess {
        pid,
        ppid: 1,
        user: "user".to_string(),
        state: "S".to_string(),
        cpu_percent: 1.0,
        memory_percent: 2.0,
        rss_kb: 3,
        vsz_kb: 4,
        elapsed: "00:01".to_string(),
        command: "sleep".to_string(),
        command_line: "sleep 10".to_string(),
    }
}

fn gpu_process(pid: u32) -> RemoteGpuProcess {
    RemoteGpuProcess {
        gpu_uuid: "gpu".to_string(),
        gpu_index: Some(0),
        pid,
        process_name: "proc".to_string(),
        used_memory_mb: u64::from(pid),
    }
}

fn docker_container(id: &str, name: &str) -> DockerContainer {
    DockerContainer {
        id: id.to_string(),
        name: name.to_string(),
        image: "image".to_string(),
        status: "Up".to_string(),
        state: "running".to_string(),
        ports: String::new(),
        created_at: String::new(),
        size: String::new(),
        stats: None,
    }
}

fn docker_image(id: &str, repository: &str) -> DockerImage {
    DockerImage {
        id: id.to_string(),
        repository: repository.to_string(),
        tag: "latest".to_string(),
        size: String::new(),
        created_since: String::new(),
    }
}

fn derived_containers(items: DockerDerivedItems) -> Arc<[DockerContainer]> {
    match items {
        DockerDerivedItems::Containers(items) => items,
        _ => panic!("expected derived Docker containers"),
    }
}

fn derived_images(items: DockerDerivedItems) -> Arc<[DockerImage]> {
    match items {
        DockerDerivedItems::Images(items) => items,
        _ => panic!("expected derived Docker images"),
    }
}

fn gpu(index: u32, uuid: &str) -> RemoteGpu {
    RemoteGpu {
        index,
        uuid: uuid.to_string(),
        name: format!("GPU {index}"),
        temperature_c: None,
        utilization_gpu_percent: None,
        utilization_memory_percent: None,
        memory_total_mb: 0,
        memory_used_mb: 0,
        memory_free_mb: 0,
        power_draw_w: None,
        power_limit_w: None,
        fan_speed_percent: None,
        pstate: String::new(),
    }
}

fn npu(index: u32, chip_id: u32, device_key: &str) -> RemoteNpu {
    RemoteNpu {
        index,
        chip_id,
        physical_id: None,
        device_key: device_key.to_string(),
        name: format!("NPU {index}:{chip_id}"),
        health: String::new(),
        bus_id: String::new(),
        temperature_c: None,
        utilization_aicore_percent: None,
        utilization_memory_percent: None,
        memory_total_mb: 0,
        memory_used_mb: 0,
        memory_free_mb: 0,
        memory_kind: String::new(),
        hbm_total_mb: None,
        hbm_used_mb: None,
        power_draw_w: None,
    }
}

#[test]
fn remote_job_state_matches_job_and_session_before_completion() {
    let mut state = RemoteJobState::<u8>::new();
    let mut rx = state
        .take_event_receiver()
        .expect("the state holds its receiver until the drain starts");
    let first = state.begin("session-a".to_string());
    first
        .tx
        .unbounded_send(7)
        .expect("receiver should stay owned");

    let second = state.begin("session-b".to_string());

    assert_eq!(rx.try_recv().ok(), Some(7));
    assert!(!state.complete_if_matches(first.job_id, "session-a"));
    assert!(state.is_pending_for("session-b"));
    assert!(!state.complete_if_matches(second.job_id, "session-a"));
    assert!(state.complete_if_matches(second.job_id, "session-b"));
    assert!(!state.is_pending());
}

#[test]
fn remote_job_state_tracks_refresh_failures_and_resets_session_runtime() {
    let mut state = RemoteJobState::<u8>::new();
    state.begin("session-a".to_string());
    state.mark_refresh_started();
    assert_eq!(state.record_refresh_failure(false), 1);
    assert_eq!(state.record_refresh_failure(true), 3);

    state.reset_for_session_switch();

    assert!(!state.is_pending());
    assert!(state.last_refresh_at().is_none());
    assert_eq!(state.consecutive_refresh_failures(), 0);
}

#[test]
fn docker_owner_excludes_menus_and_cleans_removed_details_and_compose_data() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});

    state.toggle_docker_tab_menu();
    state.toggle_docker_header_menu();
    let presentation = state.docker_presentation();
    assert!(!presentation.tab_menu_open);
    assert!(state.docker_header_menu_open());

    state.toggle_docker_container_menu("container".to_string());
    state.toggle_docker_compose_menu("compose".to_string());
    let presentation = state.docker_presentation();
    assert!(!state.docker_header_menu_open());
    assert!(state.docker_menus_open());
    assert!(presentation.container_menu_id.is_none());
    assert_eq!(presentation.compose_menu_id.as_deref(), Some("compose"));

    state.start_docker_details("gone".to_string(), "loading".to_string());
    state.apply_docker_details("gone".to_string(), DockerContainerDetails::default());
    state.toggle_compose_project("old".to_string(), "old");
    state.set_compose_services("old".to_string(), Vec::new());
    state.toggle_compose_project("failed".to_string(), "failed");
    state.set_compose_service_error("failed".to_string(), "error".to_string());
    state.apply_docker_overview(RemoteDockerOverview::default());

    let presentation = state.docker_presentation();
    let second_presentation = state.docker_presentation();
    assert!(Arc::ptr_eq(
        presentation.overview.as_ref().expect("overview"),
        second_presentation.overview.as_ref().expect("overview"),
    ));
    assert!(Arc::ptr_eq(
        &presentation.compose_expanded,
        &second_presentation.compose_expanded,
    ));
    assert!(Arc::ptr_eq(
        &presentation.compose_services,
        &second_presentation.compose_services,
    ));
    assert!(Arc::ptr_eq(
        &presentation.compose_service_errors,
        &second_presentation.compose_service_errors,
    ));
    assert!(presentation.details.is_none());
    assert!(presentation.details_container_id.is_none());
    assert!(state.docker_details_refresh().is_none());
    assert!(presentation.compose_expanded.is_empty());
    assert!(presentation.compose_services.is_empty());
    assert!(presentation.compose_service_errors.is_empty());
}

#[test]
fn docker_owner_caches_active_tab_derivations_and_invalidates_changed_inputs() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_docker_overview(RemoteDockerOverview {
        available: true,
        containers: vec![
            docker_container("one", "alpha"),
            docker_container("two", "beta"),
        ],
        images: vec![docker_image("image-one", "alpha-image")],
        ..Default::default()
    });

    let initial = derived_containers(state.derived_docker_items());
    let reused = derived_containers(state.derived_docker_items());
    assert!(Arc::ptr_eq(&initial, &reused));
    assert_eq!(initial.len(), 2);

    state.apply_docker_search("alpha".to_string());
    let searched = derived_containers(state.derived_docker_items());
    assert!(!Arc::ptr_eq(&initial, &searched));
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].id, "one");

    state.apply_docker_search("  ALPHA  ".to_string());
    let normalized = derived_containers(state.derived_docker_items());
    assert!(Arc::ptr_eq(&searched, &normalized));

    state.set_docker_tab(DockerTab::Images);
    let images = derived_images(state.derived_docker_items());
    assert_eq!(images.len(), 1);
    assert!(Arc::ptr_eq(
        &images,
        &derived_images(state.derived_docker_items()),
    ));

    state.set_docker_tab(DockerTab::Containers);
    let before_refresh = derived_containers(state.derived_docker_items());
    state.apply_docker_overview(RemoteDockerOverview {
        available: true,
        containers: vec![docker_container("three", "alpha-new")],
        ..Default::default()
    });
    let refreshed = derived_containers(state.derived_docker_items());
    assert!(!Arc::ptr_eq(&before_refresh, &refreshed));
    assert_eq!(refreshed[0].id, "three");

    state.clear_docker_overview();
    let cleared = derived_containers(state.derived_docker_items());
    assert!(!Arc::ptr_eq(&refreshed, &cleared));
    assert!(cleared.is_empty());
}

#[test]
fn docker_summary_preserves_loaded_resources_and_empty_lists_are_loaded() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_docker_summary(RemoteDockerOverview {
        available: true,
        containers: vec![docker_container("one", "first")],
        ..Default::default()
    });
    state.set_docker_tab(DockerTab::Images);
    assert!(state.docker_resource_load_due(10));
    state.mark_docker_resource_started(DockerTab::Images);
    assert!(!state.docker_resource_load_due(10));

    state.apply_docker_resource(DockerResource::Images(Vec::new()));
    assert!(!state.docker_resource_load_due(10));
    assert!(
        state
            .docker_presentation()
            .loaded_resources
            .contains(&DockerTab::Images)
    );
    state.apply_docker_resource(DockerResource::Images(vec![docker_image("img", "repo")]));
    state.apply_docker_summary(RemoteDockerOverview {
        available: true,
        containers: vec![docker_container("two", "second")],
        ..Default::default()
    });
    let overview = state
        .docker_presentation()
        .overview
        .expect("Docker overview");
    assert_eq!(overview.containers[0].id, "two");
    assert_eq!(overview.images[0].id, "img");

    state.set_docker_tab(DockerTab::Volumes);
    assert!(state.docker_resource_load_due(10));
    state.reset_for_session_switch();
    assert!(state.docker_presentation().loaded_resources.is_empty());
    state.apply_docker_summary(RemoteDockerOverview {
        available: true,
        ..Default::default()
    });
    assert!(state.docker_resource_load_due(10));
    assert!(
        state
            .docker_presentation()
            .overview
            .expect("new host")
            .images
            .is_empty()
    );
}

#[test]
fn docker_summary_disables_compose_without_dropping_other_resource_caches() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_docker_summary(RemoteDockerOverview {
        available: true,
        compose_available: true,
        ..Default::default()
    });
    state.apply_docker_resource(DockerResource::Compose(vec![
        nyaterm_transport::DockerComposeProject {
            name: "project".to_string(),
            status: "running".to_string(),
            config_files: "/compose.yml".to_string(),
        },
    ]));
    state.apply_docker_resource(DockerResource::Images(vec![docker_image("img", "repo")]));
    state.apply_docker_summary(RemoteDockerOverview {
        available: true,
        compose_available: false,
        ..Default::default()
    });
    let presentation = state.docker_presentation();
    let overview = presentation.overview.expect("Docker overview");
    assert!(overview.compose_projects.is_empty());
    assert!(!presentation.loaded_resources.contains(&DockerTab::Compose));
    assert_eq!(overview.images[0].id, "img");
}

#[test]
fn process_owner_cleans_pid_scoped_interaction_when_results_change() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_processes(vec![process(42)]);
    state.toggle_process_selection(42);
    state.toggle_process_menu(42);
    state.apply_process_nice_input("-1234x".to_string());

    let presentation = state.process_presentation();
    assert_eq!(presentation.nice_draft, "-123");
    assert_eq!(presentation.selected_pid, Some(42));
    assert_eq!(presentation.menu_pid, Some(42));

    state.apply_processes(Vec::new());

    let presentation = state.process_presentation();
    assert!(presentation.selected_pid.is_none());
    assert!(presentation.menu_pid.is_none());
    assert_eq!(presentation.nice_draft, "0");
}

#[test]
fn process_owner_caches_derived_items_by_data_search_and_sort() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    let mut first_process = process(1);
    first_process.command = "alpha".to_string();
    let mut second_process = process(2);
    second_process.command = "beta".to_string();
    state.apply_processes(vec![first_process, second_process]);

    let initial = state.derived_processes();
    assert!(Arc::ptr_eq(&initial, &state.derived_processes()));

    state.apply_process_search("alpha".to_string());
    let searched = state.derived_processes();
    assert!(!Arc::ptr_eq(&initial, &searched));
    assert_eq!(searched.len(), 1);

    state.toggle_process_sort(RemoteProcessSortKey::Pid);
    let sorted = state.derived_processes();
    assert!(!Arc::ptr_eq(&searched, &sorted));

    state.apply_processes(vec![process(3)]);
    let refreshed = state.derived_processes();
    assert!(!Arc::ptr_eq(&sorted, &refreshed));
}

#[test]
fn process_owner_reduces_matching_and_stale_job_events() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    let ticket = state.begin_process_job("session-a".to_string());
    let outcome = state.apply_process_event(
        ProcessJobResult {
            job_id: ticket.job_id,
            session_id: "session-a".to_string(),
            result: Ok(ProcessJobOutput::Listed(vec![process(7)])),
        },
        Some("session-a"),
    );
    assert!(matches!(outcome, ProcessApplyOutcome::Applied { .. }));
    assert_eq!(state.process_presentation().items[0].pid, 7);

    assert!(matches!(
        state.apply_process_event(
            ProcessJobResult {
                job_id: ticket.job_id,
                session_id: "session-a".to_string(),
                result: Err("stale".to_string()),
            },
            Some("session-a"),
        ),
        ProcessApplyOutcome::Ignored
    ));
}

#[test]
fn stats_owner_resets_session_runtime_without_losing_expansion_preference() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.toggle_stats_cpu_expanded();
    state.begin_stats_job("session-a".to_string(), false);

    state.reset_for_session_switch();

    let presentation = state.stats_presentation();
    assert!(!presentation.pending);
    assert!(presentation.data.is_none());
    assert!(presentation.cpu_expanded);
    assert_eq!(
        state.stats_status(),
        "start an SSH session to inspect remote stats"
    );
}

#[test]
fn stats_owner_reduces_matching_success_failure_and_stale_events() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    let ticket = state.begin_stats_job("session-a".to_string(), false);
    let mut stats = RemoteStats::default();
    stats.system.hostname = "host-a".to_string();
    let outcome = state.apply_stats_event(
        StatsJobResult {
            users_revision: 0,
            job_id: ticket.job_id,
            session_id: "session-a".to_string(),
            result: Ok(stats),
        },
        Some("session-a"),
    );
    assert!(matches!(outcome, StatsApplyOutcome::Applied { .. }));
    assert_eq!(
        state.stats_presentation().data.unwrap().system.hostname,
        "host-a"
    );

    assert!(matches!(
        state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: ticket.job_id,
                session_id: "session-a".to_string(),
                result: Err("stale".to_string()),
            },
            Some("session-a"),
        ),
        StatsApplyOutcome::Ignored
    ));

    for failure in 1..=3 {
        let ticket = state.begin_stats_job("session-a".to_string(), false);
        let outcome = state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: ticket.job_id,
                session_id: "session-a".to_string(),
                result: Err(format!("failure-{failure}")),
            },
            Some("session-a"),
        );
        assert!(matches!(outcome, StatsApplyOutcome::Failed { .. }));
    }
    let presentation = state.stats_presentation();
    assert!(presentation.data.is_none());
    assert_eq!(presentation.consecutive_refresh_failures, 3);
}

#[test]
fn stats_network_history_is_bounded_isolated_and_cleaned_up() {
    fn stats(rate: f64) -> RemoteStats {
        RemoteStats {
            networks: vec![NetworkInfo {
                nic: "eth0".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: rate,
                tx_bytes_per_sec: rate * 2.0,
            }],
            network_summary: NetworkSummaryInfo {
                rx_bytes_per_sec: rate,
                tx_bytes_per_sec: rate * 2.0,
            },
            ..Default::default()
        }
    }

    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    for rate in 0..61 {
        let ticket = state.begin_stats_job("session-a".to_string(), false);
        assert!(matches!(
            state.apply_stats_event(
                StatsJobResult {
                    users_revision: 0,
                    job_id: ticket.job_id,
                    session_id: "session-a".to_string(),
                    result: Ok(stats(rate as f64)),
                },
                Some("session-a"),
            ),
            StatsApplyOutcome::Applied { .. }
        ));
    }
    let history = state.stats_presentation().network_history;
    assert_eq!(history.len(), 60);
    assert_eq!(history[0].rx_bytes_per_sec, 1.0);
    assert_eq!(history[59].interfaces["eth0"], (60.0, 120.0));

    let ticket = state.begin_stats_job("session-b".to_string(), false);
    assert!(matches!(
        state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: ticket.job_id,
                session_id: "session-b".to_string(),
                result: Ok(stats(100.0)),
            },
            Some("session-a"),
        ),
        StatsApplyOutcome::CompletedInactive
    ));
    state.reset_for_session_switch();
    state.activate_stats_session("session-b");
    assert_eq!(
        state
            .stats_presentation()
            .data
            .unwrap()
            .network_summary
            .rx_bytes_per_sec,
        100.0
    );
    assert_eq!(
        state.stats_presentation().network_history[0].rx_bytes_per_sec,
        100.0
    );
    state.reset_for_session_switch();
    state.activate_stats_session("session-a");
    assert_eq!(
        state
            .stats_presentation()
            .data
            .unwrap()
            .network_summary
            .rx_bytes_per_sec,
        60.0
    );
    assert_eq!(state.stats_presentation().network_history.len(), 60);

    let stale = state.begin_stats_job("session-a".to_string(), false);
    state.clear_stats_sample("session-a");
    assert!(state.stats_presentation().data.is_none());
    assert!(matches!(
        state.apply_stats_event(
            StatsJobResult {
                users_revision: 0,
                job_id: stale.job_id,
                session_id: "session-a".into(),
                result: Ok(stats(999.0))
            },
            Some("session-a")
        ),
        StatsApplyOutcome::Ignored
    ));
    state.activate_stats_session("session-a");
    assert!(state.stats_presentation().data.is_none());
    assert!(state.stats_presentation().network_history.is_empty());
}

#[test]
fn accelerator_owner_tracks_search_offsets_and_prunes_missing_gpu_devices() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_gpu(
        "session-a",
        RemoteGpuOverview {
            available: true,
            gpus: vec![gpu(0, "gpu-a"), gpu(1, "gpu-b")],
            ..Default::default()
        },
    );

    state.toggle_gpu_device_expanded("gpu-a".to_string());
    assert!(state.set_gpu_process_offset(12));
    assert_eq!(state.gpu_presentation().process_list_offset, 12);

    state.apply_gpu_search("python".to_string());
    let presentation = state.gpu_presentation();
    assert_eq!(presentation.search_draft, "python");
    assert_eq!(presentation.process_list_offset, 0);
    assert!(presentation.expanded_devices.contains("gpu-a"));

    state.toggle_gpu_device_expanded("gpu-b".to_string());
    state.apply_gpu(
        "session-a",
        RemoteGpuOverview {
            available: true,
            gpus: vec![gpu(1, "gpu-b")],
            ..Default::default()
        },
    );

    let presentation = state.gpu_presentation();
    assert!(!presentation.expanded_devices.contains("gpu-a"));
    assert!(presentation.expanded_devices.contains("gpu-b"));
}

#[test]
fn accelerator_owner_tracks_search_offsets_and_prunes_missing_npu_devices() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_npu(
        "session-a",
        RemoteNpuOverview {
            available: true,
            npus: vec![npu(0, 0, "npu-a"), npu(1, 0, "npu-b")],
            ..Default::default()
        },
    );

    state.toggle_npu_device_expanded("npu-a".to_string());
    assert!(state.set_npu_process_offset(9));
    assert_eq!(state.npu_presentation().process_list_offset, 9);

    state.apply_npu_search("train".to_string());
    let presentation = state.npu_presentation();
    assert_eq!(presentation.search_draft, "train");
    assert_eq!(presentation.process_list_offset, 0);
    assert!(presentation.expanded_devices.contains("npu-a"));

    state.toggle_npu_device_expanded("npu-b".to_string());
    state.apply_npu(
        "session-a",
        RemoteNpuOverview {
            available: true,
            npus: vec![npu(1, 0, "npu-b")],
            ..Default::default()
        },
    );

    let presentation = state.npu_presentation();
    assert!(!presentation.expanded_devices.contains("npu-a"));
    assert!(presentation.expanded_devices.contains("npu-b"));
}

#[test]
fn accelerator_owner_caches_unavailable_sessions_until_success() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    let session_id = "session-a";

    state.apply_gpu(
        session_id,
        nyaterm_transport::RemoteGpuOverview {
            available: false,
            ..Default::default()
        },
    );

    assert!(state.gpu_unavailable_for(session_id));

    state.reset_for_session_switch();
    assert!(state.gpu_unavailable_for(session_id));

    state.apply_gpu(
        session_id,
        nyaterm_transport::RemoteGpuOverview {
            available: true,
            ..Default::default()
        },
    );

    assert!(!state.gpu_unavailable_for(session_id));
}

#[test]
/// Moved here with the four functions it exercises, which used to run inside
/// `stats_view`'s render pass.
fn gpu_process_filter_and_sort_follow_tauri_rules() {
    let mut processes = vec![
        RemoteGpuProcess {
            gpu_uuid: "gpu-b".to_string(),
            gpu_index: Some(1),
            pid: 20,
            process_name: "python".to_string(),
            used_memory_mb: 2048,
        },
        RemoteGpuProcess {
            gpu_uuid: "gpu-a".to_string(),
            gpu_index: Some(0),
            pid: 10,
            process_name: "worker".to_string(),
            used_memory_mb: 4096,
        },
        RemoteGpuProcess {
            gpu_uuid: "gpu-c".to_string(),
            gpu_index: None,
            pid: 5,
            process_name: "python".to_string(),
            used_memory_mb: 2048,
        },
    ];

    assert!(RemoteGpuOverview::process_matches(&processes[0], "python"));
    assert!(RemoteGpuOverview::process_matches(&processes[1], "10"));
    assert!(RemoteGpuOverview::process_matches(&processes[2], "gpu-c"));

    RemoteGpuOverview::sort_processes(&mut processes);
    assert_eq!(
        processes
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        [10, 20, 5]
    );
}

#[test]
fn npu_process_filter_and_sort_follow_tauri_rules() {
    let mut processes = vec![
        RemoteNpuProcess {
            npu_index: 1,
            chip_id: 0,
            device_key: "npu-b".to_string(),
            pid: 30,
            process_name: "train".to_string(),
            used_memory_mb: 1024,
        },
        RemoteNpuProcess {
            npu_index: 0,
            chip_id: 1,
            device_key: "npu-a".to_string(),
            pid: 20,
            process_name: "infer".to_string(),
            used_memory_mb: 2048,
        },
        RemoteNpuProcess {
            npu_index: 0,
            chip_id: 0,
            device_key: "npu-c".to_string(),
            pid: 10,
            process_name: "train".to_string(),
            used_memory_mb: 2048,
        },
    ];

    assert!(RemoteNpuOverview::process_matches(&processes[0], "train"));
    assert!(RemoteNpuOverview::process_matches(&processes[1], "0 1"));
    assert!(RemoteNpuOverview::process_matches(&processes[2], "npu-c"));

    RemoteNpuOverview::sort_processes(&mut processes);
    assert_eq!(
        processes
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        [10, 20, 30]
    );
}

/// And for a GPU card process list, which had no derived cache at all before: its
/// filtering ran inside `stats_view`, so only the view knew the row count.
#[test]
fn shorter_gpu_results_clamp_the_stored_offset_with_no_render() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_gpu(
        "session",
        RemoteGpuOverview {
            available: true,
            processes: (1..=40).map(gpu_process).collect(),
            ..Default::default()
        },
    );
    assert!(state.set_gpu_process_offset(30));

    state.apply_gpu(
        "session",
        RemoteGpuOverview {
            available: true,
            processes: (1..=2).map(gpu_process).collect(),
            ..Default::default()
        },
    );

    assert_eq!(state.gpu_presentation().process_list_offset, 0);
    assert_eq!(state.derived_gpu_processes().len(), 2);
}

/// A query or sort change must leave the derived list correct for a reader holding
/// only `&self`.
///
/// The accessor taking `&self` is the point: the old one took `&mut self` and
/// computed on demand, so the only reader that could see a fresh list was one able
/// to mutate -- which is why the render pass had to. The values are asserted too, so
/// a future change that makes the recompute lazy again fails here rather than merely
/// failing to compile.
#[test]
fn a_query_or_sort_change_leaves_the_derived_list_correct_for_a_reader() {
    fn read(state: &RemoteOpsFeatureState) -> Vec<u32> {
        state
            .derived_processes()
            .iter()
            .map(|process| process.pid)
            .collect()
    }

    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    let mut alpha = process(1);
    alpha.command = "alpha".to_string();
    let mut beta = process(2);
    beta.command = "beta".to_string();
    state.apply_processes(vec![alpha, beta]);
    assert_eq!(read(&state).len(), 2);

    state.apply_process_search("beta".to_string());
    assert_eq!(read(&state), vec![2], "the query narrowed the list");

    state.apply_process_search(String::new());
    state.toggle_process_sort(RemoteProcessSortKey::Pid);
    assert_eq!(read(&state), vec![1, 2], "ascending by pid");
    state.toggle_process_sort(RemoteProcessSortKey::Pid);
    assert_eq!(read(&state), vec![2, 1], "and reversed");
}

/// Every mutation that changes a pane presentation must bump that pane revision,
/// and must not bump another pane.
///
/// The flush compares this counter to decide whether to build and push a snapshot,
/// so a mutator that changes the presentation without bumping it would leave the
/// panel showing stale data with nothing to detect it. Each pane is asserted
/// individually rather than in a loop, because the point is coverage of the
/// mutators, and a loop would hide which one stopped bumping.
#[test]
fn presentation_mutations_bump_only_their_own_pane_revision() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});

    let expect_bump = |state: &mut RemoteOpsFeatureState,
                       what: &str,
                       mutate: &dyn Fn(&mut RemoteOpsFeatureState)| {
        let before = (
            state.stats_revision(),
            state.gpu_revision(),
            state.npu_revision(),
        );
        mutate(state);
        let after = (
            state.stats_revision(),
            state.gpu_revision(),
            state.npu_revision(),
        );
        assert_ne!(before, after, "{what} must bump a revision");
        after
    };

    // Stats.
    let mut previous = (
        state.stats_revision(),
        state.gpu_revision(),
        state.npu_revision(),
    );
    for (what, mutate) in [
        (
            "apply_stats",
            &(|state: &mut RemoteOpsFeatureState| state.apply_stats(RemoteStats::default()))
                as &dyn Fn(&mut RemoteOpsFeatureState),
        ),
        ("set_stats_status", &|state: &mut RemoteOpsFeatureState| {
            state.set_stats_status("loading")
        }),
        (
            "toggle_stats_cpu_expanded",
            &|state: &mut RemoteOpsFeatureState| state.toggle_stats_cpu_expanded(),
        ),
        (
            "record_stats_refresh_failure",
            &|state: &mut RemoteOpsFeatureState| {
                state.record_stats_refresh_failure();
            },
        ),
        (
            "reset_stats_refresh_failures",
            &|state: &mut RemoteOpsFeatureState| state.reset_stats_refresh_failures(),
        ),
    ] {
        let after = expect_bump(&mut state, what, mutate);
        assert_eq!(
            (after.1, after.2),
            (previous.1, previous.2),
            "{what} must not disturb the GPU or NPU panes"
        );
        previous = after;
    }

    // GPU, and the NPU pane must not move with it.
    let after = expect_bump(
        &mut state,
        "apply_gpu",
        &|state: &mut RemoteOpsFeatureState| {
            state.apply_gpu("session", RemoteGpuOverview::default())
        },
    );
    assert_eq!(after.2, previous.2, "apply_gpu must not touch the NPU pane");
    previous = after;

    for (what, mutate) in [
        (
            "apply_gpu_search",
            &(|state: &mut RemoteOpsFeatureState| state.apply_gpu_search("q".to_string()))
                as &dyn Fn(&mut RemoteOpsFeatureState),
        ),
        (
            "toggle_gpu_device_expanded",
            &|state: &mut RemoteOpsFeatureState| state.toggle_gpu_device_expanded("0".to_string()),
        ),
        ("set_gpu_status", &|state: &mut RemoteOpsFeatureState| {
            state.set_gpu_status("loading")
        }),
    ] {
        let after = expect_bump(&mut state, what, mutate);
        assert_eq!(after.2, previous.2, "{what} must not touch the NPU pane");
        previous = after;
    }

    // NPU.
    let after = expect_bump(
        &mut state,
        "apply_npu",
        &|state: &mut RemoteOpsFeatureState| {
            state.apply_npu("session", RemoteNpuOverview::default())
        },
    );
    assert_eq!(
        after.0, previous.0,
        "apply_npu must not touch the stats pane"
    );
    previous = after;

    // A session switch clears every pane, so it must move every revision. This
    // arm was missing when the counter first landed, and the stats pane silently
    // did not bump: `reset_for_session_switch` wrote `data` and `status` directly
    // rather than going through the methods that touch. A workspace-restore test
    // caught it, one layer further out than it should have needed.
    let after = expect_bump(
        &mut state,
        "reset_for_session_switch",
        &|state: &mut RemoteOpsFeatureState| state.reset_for_session_switch(),
    );
    assert_ne!(after.0, previous.0, "the stats revision must advance");
    assert_ne!(after.1, previous.1, "the GPU revision must advance");
    assert_ne!(after.2, previous.2, "the NPU revision must advance");
}

/// Every Docker mutation must bump the Docker pane revision, and no other pane's.
///
/// Seventeen mutators on `RemoteOpsFeatureState` write Docker fields directly -- menus,
/// offsets, details, compose state -- so nothing structural stops one from being added
/// without a touch. This drives every one of them; it is what makes the counter
/// trustworthy rather than the field access pattern.
#[test]
fn docker_presentation_mutations_bump_the_revision() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_docker_overview(RemoteDockerOverview {
        available: true,
        containers: vec![docker_container("c0", "name")],
        images: (0..40)
            .map(|index| docker_image(&format!("i{index}"), "repo"))
            .collect(),
        ..Default::default()
    });

    type Mutation = Box<dyn Fn(&mut RemoteOpsFeatureState)>;
    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "set_docker_status",
            Box::new(|s: &mut RemoteOpsFeatureState| s.set_docker_status("x")),
        ),
        (
            "apply_docker_search",
            Box::new(|s: &mut RemoteOpsFeatureState| s.apply_docker_search("q".to_string())),
        ),
        (
            "set_docker_tab",
            Box::new(|s: &mut RemoteOpsFeatureState| s.set_docker_tab(DockerTab::Images)),
        ),
        (
            "toggle_docker_tab_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| s.toggle_docker_tab_menu()),
        ),
        (
            "toggle_docker_header_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| s.toggle_docker_header_menu()),
        ),
        (
            "close_docker_menus",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.close_docker_menus();
            }),
        ),
        (
            "toggle_docker_container_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.toggle_docker_container_menu("c0".to_string())
            }),
        ),
        (
            "close_docker_container_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_container_menu()),
        ),
        (
            "toggle_docker_compose_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| s.toggle_docker_compose_menu("k".to_string())),
        ),
        (
            "close_docker_compose_menu",
            Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_compose_menu()),
        ),
        (
            "toggle_compose_project",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.toggle_compose_project("k".to_string(), "proj");
            }),
        ),
        (
            "start_docker_container_action",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.start_docker_container_action("acting".to_string())
            }),
        ),
        (
            "start_docker_details",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.start_docker_details("c0".to_string(), "loading".to_string())
            }),
        ),
        (
            "apply_docker_details",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.apply_docker_details("c0".to_string(), DockerContainerDetails::default())
            }),
        ),
        (
            "close_docker_details",
            Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_details()),
        ),
        (
            "set_compose_services",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.set_compose_services("k".to_string(), Vec::new())
            }),
        ),
        (
            "set_compose_service_error",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.set_compose_service_error("k".to_string(), "boom".to_string())
            }),
        ),
        (
            "clear_compose_service_error",
            Box::new(|s: &mut RemoteOpsFeatureState| s.clear_compose_service_error("k")),
        ),
        (
            "begin_docker_job",
            Box::new(|s: &mut RemoteOpsFeatureState| {
                s.begin_docker_job("session".to_string());
            }),
        ),
        (
            "clear_docker_overview",
            Box::new(|s: &mut RemoteOpsFeatureState| s.clear_docker_overview()),
        ),
        (
            "reset_for_session_switch",
            Box::new(|s: &mut RemoteOpsFeatureState| s.reset_for_session_switch()),
        ),
    ];

    for (label, mutate) in mutations {
        let before = (
            state.docker_revision(),
            state.stats_revision(),
            state.process_revision(),
        );
        mutate(&mut state);
        let after = (
            state.docker_revision(),
            state.stats_revision(),
            state.process_revision(),
        );
        assert_ne!(
            before.0, after.0,
            "{label} changes the Docker presentation and must bump its revision"
        );
        if label != "reset_for_session_switch" {
            assert_eq!(
                (after.1, after.2),
                (before.1, before.2),
                "{label} must not disturb the stats or process panes"
            );
        }
    }
}

/// The failure ladder clears the overview at three, and that clear has to bump.
///
/// `record_docker_refresh_failure` returning >= 3 is what makes the caller call
/// `clear_docker_overview`, so the two are only correct together: a bump on the record
/// but not the clear would leave a panel showing containers the pane no longer has.
#[test]
fn the_docker_failure_ladder_clears_the_overview_and_bumps() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.apply_docker_overview(RemoteDockerOverview {
        available: true,
        containers: vec![docker_container("c0", "name")],
        ..Default::default()
    });
    assert!(state.docker_presentation().overview.is_some());

    // The failure count is deliberately *not* part of the Docker presentation --
    // unlike the accelerator panes, whose title-bar readout carries it -- so recording
    // a failure changes nothing rendered and must not bump. What is rendered is the
    // overview, which the third failure drops.
    let revision = state.docker_revision();
    let mut failures = 0;
    for _ in 0..3 {
        failures = state.record_docker_refresh_failure();
    }
    assert_eq!(failures, 3, "three failures is the documented threshold");
    assert_eq!(
        state.docker_revision(),
        revision,
        "a failure streak alone changes nothing on screen"
    );

    state.clear_docker_overview();
    assert!(
        state.docker_presentation().overview.is_none(),
        "the overview is dropped once the streak reaches three"
    );
    assert_ne!(
        state.docker_revision(),
        revision,
        "and the clear must bump, or the panel keeps rendering stale containers"
    );
}

#[test]
fn docker_details_ignore_results_after_close_switch_or_removal() {
    for reason in ["close", "session", "removed", "different-container"] {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.start_docker_details("old".into(), "loading".into());
        match reason {
            "close" => state.close_docker_details(),
            "session" => state.reset_for_session_switch(),
            "removed" => state.apply_docker_overview(RemoteDockerOverview::default()),
            _ => state.start_docker_details("new".into(), "loading".into()),
        }
        let before = state.docker_revision();
        state.apply_docker_details("old".into(), DockerContainerDetails::default());
        assert!(state.docker_presentation().details.is_none(), "{reason}");
        assert_eq!(
            state.docker_revision(),
            before,
            "ignored results must not invalidate views"
        );
        assert_ne!(
            state.docker_presentation().details_container_id.as_deref(),
            Some("old")
        );
    }
}

#[test]
fn docker_details_refresh_preserves_content_but_a_new_target_clears_it() {
    let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
    state.start_docker_details("a".into(), "loading".into());
    state.apply_docker_details("a".into(), DockerContainerDetails::default());
    state.start_docker_details("a".into(), "refreshing".into());
    assert!(state.docker_presentation().details.is_some());
    state.start_docker_details("b".into(), "loading".into());
    assert!(state.docker_presentation().details.is_none());
}

use crate::{
    AiExecutionProfile, AssetAccelerator, AssetAcceleratorSnapshot, AssetAcceleratorType,
    AssetDisk, AssetDiskSnapshot, AssetDisplayLabels, AssetFilterKey, AssetMetadata,
    AssetMonitoringCache, AssetSortDirection, AssetSortKey, AssetSortState, AssetStatsSnapshot,
    ConnectionType, Group, RecordAssetMonitoringPatch, SavedConnection,
    build_asset_patch_from_accelerator_snapshot, build_asset_patch_from_stats_snapshot,
    build_asset_records, build_group_path, collect_descendant_group_ids,
    connection_matches_filters, connections_for_asset_group, sort_asset_records,
};

fn group(id: &str, name: &str, parent: Option<&str>, sort_order: i32) -> Group {
    Group {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: parent.map(ToOwned::to_owned),
        sort_order,
        created_at_ms: None,
        updated_at_ms: None,
    }
}

fn connection(id: &str, name: &str, host: &str, group_id: Option<&str>) -> SavedConnection {
    SavedConnection {
        extensions: Default::default(),
        id: id.to_string(),
        name: name.to_string(),
        config: ConnectionType::Ssh {
            host: host.to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            ai_execution_profile: AiExecutionProfile::Auto,
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: group_id.map(ToOwned::to_owned),
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: None,
        network: None,
        post_login: None,
        recording: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}

#[test]
fn group_paths_and_descendants_tolerate_cycles_and_missing_parents() {
    let groups = vec![
        group("root", "Region", None, 0),
        group("lab", "AI Lab", Some("root"), 0),
        group("a", "A", Some("b"), 1),
        group("b", "B", Some("a"), 2),
        group("orphan", "Orphan", Some("missing"), 3),
    ];
    assert_eq!(
        build_group_path(&groups, Some("lab"))
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>(),
        ["assets.root", "Region", "AI Lab"]
    );
    assert_eq!(
        build_group_path(&groups, Some("orphan"))
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>(),
        ["assets.root", "Orphan"]
    );
    assert_eq!(build_group_path(&groups, Some("a")).len(), 3);
    let descendants = collect_descendant_group_ids(&groups, Some("root"));
    assert_eq!(descendants, ["root".to_string(), "lab".to_string()].into());
}

#[test]
fn selected_group_includes_descendants_and_root_keeps_ungrouped() {
    let groups = vec![
        group("root", "Root", None, 0),
        group("child", "Child", Some("root"), 0),
    ];
    let connections = vec![
        connection("root-host", "Root", "10.0.0.1", Some("root")),
        connection("child-host", "Child", "10.0.0.2", Some("child")),
        connection("loose", "Loose", "10.0.0.3", None),
    ];
    assert_eq!(
        connections_for_asset_group(&connections, &groups, None).len(),
        3
    );
    assert_eq!(
        connections_for_asset_group(&connections, &groups, Some("root"))
            .into_iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["root-host", "child-host"]
    );
}

#[test]
fn filters_combine_with_and_semantics_and_sort_matches_tauri() {
    let groups = vec![];
    let mut gpu = connection("gpu", "node-10", "10.0.0.10", None);
    gpu.asset = Some(AssetMetadata {
        os_name: Some("Ubuntu Linux".to_string()),
        cpu_cores: Some(32),
        memory_bytes: Some(64 * 1024_u64.pow(3)),
        accelerators: Some(vec![AssetAccelerator {
            r#type: AssetAcceleratorType::Gpu,
            vendor: Some("NVIDIA".to_string()),
            model: Some("H100".to_string()),
            count: Some(2),
            memory_bytes: None,
        }]),
        ..AssetMetadata::default()
    });
    gpu.last_used_at_ms = Some(20);
    let mut win = connection("win", "node-2", "10.0.0.2", None);
    win.asset = Some(AssetMetadata {
        os_name: Some("Windows Server".to_string()),
        ..AssetMetadata::default()
    });
    win.last_used_at_ms = Some(10);
    let missing = connection("missing", "node-1", "host", None);

    assert!(connection_matches_filters(
        &gpu,
        &[AssetFilterKey::Linux, AssetFilterKey::Gpu]
    ));
    assert!(!connection_matches_filters(
        &gpu,
        &[AssetFilterKey::Linux, AssetFilterKey::Npu]
    ));

    let labels = AssetDisplayLabels::default();
    let mut records = build_asset_records(&[gpu, win, missing], &groups, "Assets");
    sort_asset_records(
        &mut records,
        Some(AssetSortState {
            key: AssetSortKey::Name,
            direction: AssetSortDirection::Asc,
        }),
        &labels,
    );
    assert_eq!(
        records
            .iter()
            .map(|item| item.connection.name.as_str())
            .collect::<Vec<_>>(),
        ["node-1", "node-2", "node-10"]
    );
    sort_asset_records(
        &mut records,
        Some(AssetSortState {
            key: AssetSortKey::Address,
            direction: AssetSortDirection::Asc,
        }),
        &labels,
    );
    assert_eq!(records[0].connection.id, "win");
    sort_asset_records(
        &mut records,
        Some(AssetSortState {
            key: AssetSortKey::ConnectionTime,
            direction: AssetSortDirection::Desc,
        }),
        &labels,
    );
    assert_eq!(
        records
            .iter()
            .map(|item| item.connection.id.as_str())
            .collect::<Vec<_>>(),
        ["gpu", "win", "missing"]
    );
}

#[test]
fn monitoring_snapshots_merge_per_session_and_accelerator_type() {
    let stats = build_asset_patch_from_stats_snapshot(&AssetStatsSnapshot {
        hostname: "node-01".to_string(),
        os: "Ubuntu Linux".to_string(),
        arch: "x86_64".to_string(),
        cpu_model: "EPYC".to_string(),
        cpu_cores: 32,
        memory_total_bytes: 64 * 1024_u64.pow(3),
        disks: vec![AssetDiskSnapshot {
            device: "/dev/nvme0n1".to_string(),
            mount: "/".to_string(),
            total_bytes: 2 * 1024_u64.pow(4),
        }],
    })
    .expect("stats patch");
    assert_eq!(stats.hostname.as_deref(), Some("node-01"));
    assert_eq!(
        stats
            .disks
            .as_ref()
            .and_then(|items| items.first())
            .and_then(|disk: &AssetDisk| disk.count),
        Some(1)
    );

    let gpu = build_asset_patch_from_accelerator_snapshot(
        AssetAcceleratorType::Gpu,
        "NVIDIA",
        true,
        &[
            AssetAcceleratorSnapshot {
                vendor: String::new(),
                model: "H100".to_string(),
                memory_total_mb: 80 * 1024,
            },
            AssetAcceleratorSnapshot {
                vendor: String::new(),
                model: "H100".to_string(),
                memory_total_mb: 80 * 1024,
            },
        ],
    )
    .expect("gpu patch");
    assert_eq!(
        gpu.accelerators
            .as_ref()
            .and_then(|items| items.first())
            .and_then(|item| item.count),
        Some(2)
    );

    let mut cache = AssetMonitoringCache::new();
    assert!(cache.record(RecordAssetMonitoringPatch {
        source_session_id: "s1",
        target_session_id: "s1",
        connection_id: "c1",
        patch: Some(stats)
    }));
    assert!(cache.record(RecordAssetMonitoringPatch {
        source_session_id: "s1",
        target_session_id: "s1",
        connection_id: "c1",
        patch: Some(gpu)
    }));
    let entry = cache.take("s1").expect("cached patch");
    assert_eq!(entry.connection_id, "c1");
    assert_eq!(entry.last_asset_patch.hostname.as_deref(), Some("node-01"));
    assert_eq!(
        entry.last_asset_patch.accelerators.as_ref().map(Vec::len),
        Some(1)
    );

    assert!(!cache.record(RecordAssetMonitoringPatch {
        source_session_id: "old",
        target_session_id: "new",
        connection_id: "c2",
        patch: Some(AssetMetadata::default())
    }));
}

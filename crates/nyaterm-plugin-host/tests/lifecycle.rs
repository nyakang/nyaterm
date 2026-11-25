use futures::executor::block_on;
use nyaterm_core::plugins::invocation::{ActionInput, ActionResult, ParameterValue};
use nyaterm_core::plugins::{ErrorCode, MAX_TEXT_BYTES, PluginStatus};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_plugin_host::manager::PluginManager;
use nyaterm_plugin_host::runtime::{RuntimeLimits, validate_api_marker};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const HOST: &str = "2.0.0-preview.4";
const FIXTURE: &[u8] = include_bytes!("fixtures/lifecycle.wasm");

fn root() -> TestTempDir {
    let root = TestTempDir::new("nyaterm-plugin-lifecycle");
    fs::create_dir_all(root.path()).unwrap();
    root
}
fn manager(root: &Path) -> PluginManager {
    PluginManager::open(root.join("host"), HOST, RuntimeLimits::default()).unwrap()
}
fn fixture(root: &Path, id: &str) -> std::path::PathBuf {
    let source = root.join(id);
    fs::create_dir_all(&source).unwrap();
    let mut manifest = format!(
        "schema_version = 2\nid = \"{id}\"\nname = \"Lifecycle fixture\"\nversion = \"1.0.0\"\nauthors = []\ndescription = \"SDK-built fixture\"\nhost_version = \">=2.0.0-preview.4\"\napi_version = \"2.0.0\"\ncomponent = \"plugin.wasm\"\n"
    );
    for action in ["count", "loop", "memory", "trap", "oversize", "controls"] {
        manifest.push_str(&format!("\n[[actions]]\nid = \"{action}\"\nname = \"{action}\"\ndescription = \"fixture\"\nresult = \"text\"\n"));
    }
    fs::write(source.join("plugin.toml"), manifest).unwrap();
    fs::write(source.join("plugin.wasm"), FIXTURE).unwrap();
    source
}
fn call(manager: &PluginManager, id: &str, action: &str) -> ActionResult {
    block_on(
        manager
            .invoke(&format!("{id}:{action}"), ActionInput::default())
            .unwrap()
            .result(),
    )
    .unwrap()
}
fn text(value: ActionResult) -> String {
    match value {
        ActionResult::Text(text) => text,
        _ => panic!("expected text"),
    }
}

#[test]
fn sdk_components_keep_state_isolate_instances_and_restore_enabled_preferences() {
    let root = root();
    let mut host = manager(&root);
    host.install(&fixture(&root, "first"), false).unwrap();
    host.install(&fixture(&root, "second"), false).unwrap();
    assert_eq!(text(call(&host, "first", "count")), "1");
    assert_eq!(text(call(&host, "first", "count")), "2");
    assert_eq!(text(call(&host, "second", "count")), "1");
    assert_eq!(host.snapshot().plugins[0].status, PluginStatus::Running);
    host.set_enabled("first", false).unwrap();
    assert!(
        !host
            .snapshot()
            .contributions
            .iter()
            .any(|c| c.plugin_id == "first")
    );
    assert!(host.invoke("first:count", ActionInput::default()).is_err());
    drop(host);
    let mut host = manager(&root);
    assert!(
        !host
            .snapshot()
            .plugins
            .iter()
            .find(|p| p.id == "first")
            .unwrap()
            .enabled
    );
    assert!(
        host.snapshot()
            .plugins
            .iter()
            .find(|p| p.id == "second")
            .unwrap()
            .enabled
    );
    host.set_enabled("first", true).unwrap();
    assert_eq!(text(call(&host, "first", "count")), "1");
    host.uninstall("first").unwrap();
    assert!(
        host.snapshot()
            .contributions
            .iter()
            .all(|c| c.plugin_id != "first")
    );
    assert!(!root.join("host/installed/first").exists());
}

#[test]
fn sdk_guest_submits_host_io_without_blocking_its_invocation() {
    let root = root();
    let source = fixture(&root, "host-io");
    let manifest = source.join("plugin.toml");
    let mut raw = fs::read_to_string(&manifest).unwrap().replace(
        "schema_version = 2",
        "schema_version = 2\ncapabilities = [\"storage\"]",
    );
    for id in ["host-submit", "host-poll"] {
        raw.push_str(&format!("\n[[actions]]\nid = \"{id}\"\nname = \"{id}\"\ndescription = \"Host task\"\nresult = \"text\"\n"));
    }
    fs::write(manifest, raw).unwrap();
    let mut host = manager(&root);
    host.install(&source, false).unwrap();
    assert_eq!(text(call(&host, "host-io", "host-submit")), "submitted");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = text(call(&host, "host-io", "host-poll"));
        if result == "complete" {
            break;
        }
        assert_eq!(result, "pending");
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read(root.join("host/host/data/host-io/probe")).unwrap(),
        b"sdk-host-round-trip"
    );
}

#[test]
fn sdk_command_and_text_examples_execute_actual_guest_exports() {
    let root = root();
    let mut host = manager(&root);
    for (id, component) in [
        (
            "diagnostic-command",
            include_bytes!("fixtures/diagnostic-command.wasm").as_slice(),
        ),
        (
            "text-tools",
            include_bytes!("fixtures/text-tools.wasm").as_slice(),
        ),
    ] {
        let source = root.join(id);
        fs::create_dir(&source).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/plugins")
                .join(id)
                .join("plugin.toml"),
            source.join("plugin.toml"),
        )
        .unwrap();
        fs::write(source.join("plugin.wasm"), component).unwrap();
        host.install(&source, false).unwrap();
    }
    let mut input = ActionInput::default();
    input
        .parameters
        .insert("host".into(), ParameterValue::String("example.org".into()));
    let result = block_on(
        host.invoke("diagnostic-command:diagnose", input)
            .unwrap()
            .result(),
    )
    .unwrap();
    assert!(
        matches!(result, ActionResult::Command { command, .. } if command == "nslookup example.org\nping example.org")
    );
    let input = ActionInput {
        text: Some("c\na\nb".into()),
        ..ActionInput::default()
    };
    assert_eq!(
        text(block_on(host.invoke("text-tools:sort", input).unwrap().result()).unwrap()),
        "a\nb\nc"
    );
}

#[test]
fn reload_switches_atomically_and_failed_development_reload_preserves_old_instance() {
    let root = root();
    let mut host = manager(&root);
    let source = fixture(&root, "development");
    host.install(&source, true).unwrap();
    assert_eq!(text(call(&host, "development", "count")), "1");
    let old_revision = host.snapshot().plugins[0].revision;
    fs::write(source.join("plugin.wasm"), b"broken component").unwrap();
    assert!(host.reload("development").is_err());
    assert_eq!(host.snapshot().plugins[0].revision, old_revision);
    assert_eq!(text(call(&host, "development", "count")), "2");
    fs::write(source.join("plugin.wasm"), FIXTURE).unwrap();
    let stale = host
        .invoke("development:count", ActionInput::default())
        .unwrap();
    host.reload("development").unwrap();
    assert_eq!(
        block_on(stale.result()).err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert_eq!(host.snapshot().contributions.len(), 6);
    assert!(host.snapshot().plugins[0].revision > old_revision);
    assert_eq!(text(call(&host, "development", "count")), "1");
}

#[test]
fn snapshot_prevents_source_mutation_and_update_failure_preserves_disk_and_runtime() {
    let root = root();
    let mut host = manager(&root);
    let source = fixture(&root, "atomic");
    host.install(&source, false).unwrap();
    fs::write(source.join("plugin.wasm"), b"broken").unwrap();
    assert_eq!(text(call(&host, "atomic", "count")), "1");
    assert!(host.update("atomic", &source).is_err());
    assert_eq!(
        fs::read(root.join("host/installed/atomic/plugin.wasm")).unwrap(),
        FIXTURE
    );
    assert_eq!(text(call(&host, "atomic", "count")), "2");
    fs::write(source.join("plugin.wasm"), FIXTURE).unwrap();
    host.update("atomic", &source).unwrap();
    host.reload("atomic").unwrap();
    assert_eq!(host.snapshot().contributions.len(), 6);
    fs::create_dir(root.join("host/work/data/atomic")).unwrap();
    fs::write(root.join("host/work/data/atomic/retained"), b"data").unwrap();
    host.uninstall("atomic").unwrap();
    assert!(root.join("host/work/data/atomic/retained").exists());
    assert_eq!(fs::read_dir(root.join("host/staging")).unwrap().count(), 0);
}

#[test]
fn archive_install_rejects_traversal_links_duplicate_names_and_limits() {
    let root = root();
    let mut host = manager(&root);
    let source = fixture(&root, "archive");
    let archive = root.join("valid.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for name in ["plugin.toml", "plugin.wasm"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&fs::read(source.join(name)).unwrap())
            .unwrap();
    }
    writer.finish().unwrap();
    host.install(&archive, false).unwrap();
    assert_eq!(text(call(&host, "archive", "count")), "1");
    for name in ["../outside", "C:/evil", "CON.txt", "a:stream", "a\\b"] {
        let archive = root.join("bad.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"bad").unwrap();
        writer.finish().unwrap();
        assert_eq!(
            host.install(&archive, false).err().unwrap().code,
            ErrorCode::UnsafePath
        );
    }
    let archive = root.join("duplicate.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for name in ["entry", "ENTRY"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"data").unwrap();
    }
    writer.finish().unwrap();
    assert_eq!(
        host.install(&archive, false).err().unwrap().code,
        ErrorCode::UnsafePath
    );
    let archive = root.join("link.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .add_symlink(
            "escape",
            "../outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert_eq!(
        host.install(&archive, false).err().unwrap().code,
        ErrorCode::UnsafePath
    );
    let crowded = root.join("crowded");
    fs::create_dir(&crowded).unwrap();
    for i in 0..513 {
        fs::write(crowded.join(format!("file{i}")), b"").unwrap();
    }
    assert_eq!(
        host.install(&crowded, false).err().unwrap().code,
        ErrorCode::PackageLimit
    );
    assert!(!root.join("outside").exists());
}

#[cfg(unix)]
#[test]
fn package_below_symlinked_ancestor_installs_when_package_tree_has_no_links() {
    let root = root();
    let physical_parent = root.join("physical-parent");
    fs::create_dir(&physical_parent).unwrap();
    let source = fixture(&physical_parent, "linkedparent");
    let aliased_parent = root.join("aliased-parent");
    std::os::unix::fs::symlink(&physical_parent, &aliased_parent).unwrap();

    let mut host = manager(&root);
    host.install(&aliased_parent.join(source.file_name().unwrap()), false)
        .unwrap();

    assert_eq!(text(call(&host, "linkedparent", "count")), "1");
}

#[test]
fn infinite_loop_deadline_actually_stops_worker_and_other_plugins_still_run() {
    let root = root();
    let limits = RuntimeLimits {
        fuel: u64::MAX,
        timeout: Duration::from_millis(120),
        ..RuntimeLimits::default()
    };
    let mut host = PluginManager::open(root.join("host"), HOST, limits).unwrap();
    host.install(&fixture(&root, "broken"), false).unwrap();
    host.install(&fixture(&root, "healthy"), false).unwrap();
    let started = Instant::now();
    let error = block_on(
        host.invoke("broken:loop", ActionInput::default())
            .unwrap()
            .result(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        host.snapshot()
            .plugins
            .iter()
            .find(|p| p.id == "broken")
            .unwrap()
            .status,
        PluginStatus::Faulted
    );
    assert!(
        host.snapshot()
            .contributions
            .iter()
            .all(|c| c.plugin_id != "broken")
    );
    assert_eq!(text(call(&host, "healthy", "count")), "1");
    host.shutdown();
    assert!(host.snapshot().stopped);
    assert!(host.snapshot().contributions.is_empty());
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn fuel_memory_trap_and_result_failures_retire_only_their_instance() {
    let root = root();
    let mut host = PluginManager::open(
        root.join("host"),
        HOST,
        RuntimeLimits {
            memory_bytes: 8 * 1024 * 1024,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    host.install(&fixture(&root, "healthy"), false).unwrap();
    for action in ["loop", "memory", "trap", "oversize", "controls"] {
        let id = format!("fault-{action}");
        host.install(&fixture(&root, &id), false).unwrap();
        let error = block_on(
            host.invoke(&format!("{id}:{action}"), ActionInput::default())
                .unwrap()
                .result(),
        )
        .err()
        .unwrap();
        if action == "loop" {
            assert_eq!(error.code, ErrorCode::Budget);
        }
        if action == "memory" {
            assert_eq!(error.code, ErrorCode::MemoryLimit);
        }
        if matches!(action, "oversize" | "controls") {
            assert_eq!(error.code, ErrorCode::InvalidResult);
        }
        assert_eq!(
            host.snapshot()
                .plugins
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .status,
            PluginStatus::Faulted
        );
        host.uninstall(&id).unwrap();
        assert!(!text(call(&host, "healthy", "count")).is_empty());
    }
}

#[test]
fn bounded_queue_cancellation_reload_and_shutdown_reap_real_guest_execution() {
    let root = root();
    let limits = RuntimeLimits {
        fuel: u64::MAX,
        timeout: Duration::from_secs(2),
        queue_capacity: 1,
        ..RuntimeLimits::default()
    };
    let mut host = PluginManager::open(root.join("host"), HOST, limits).unwrap();
    host.install(&fixture(&root, "queued"), false).unwrap();
    let looping = host.invoke("queued:loop", ActionInput::default()).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let pending = host.invoke("queued:count", ActionInput::default()).unwrap();
    assert_eq!(
        host.invoke("queued:count", ActionInput::default())
            .err()
            .unwrap()
            .code,
        ErrorCode::QueueFull
    );
    looping.cancel();
    assert_eq!(
        block_on(looping.result()).err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert!(block_on(pending.result()).is_err());
    host.reload("queued").unwrap();
    let looping = host.invoke("queued:loop", ActionInput::default()).unwrap();
    let started = Instant::now();
    host.set_enabled("queued", false).unwrap();
    assert_eq!(
        block_on(looping.result()).err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    host.set_enabled("queued", true).unwrap();
    let looping = host.invoke("queued:loop", ActionInput::default()).unwrap();
    host.reload("queued").unwrap();
    assert_eq!(
        block_on(looping.result()).err().unwrap().code,
        ErrorCode::Cancelled
    );
    let looping = host.invoke("queued:loop", ActionInput::default()).unwrap();
    let started = Instant::now();
    host.shutdown();
    assert!(block_on(looping.result()).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(host);
    fs::remove_dir_all(root.join("host")).unwrap(); // Windows handles have actually been released.
}

#[test]
fn initialization_failures_abi_forgery_and_input_limits_are_rejected() {
    let root = root();
    let mut host = PluginManager::open(
        root.join("host"),
        HOST,
        RuntimeLimits {
            timeout: Duration::from_millis(100),
            fuel: u64::MAX,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    for id in ["init-fail", "init-loop"] {
        host.install(&fixture(&root, id), false).unwrap();
        assert!(
            host.snapshot()
                .plugins
                .iter()
                .find(|plugin| plugin.id == id)
                .unwrap()
                .error
                .is_none()
        );
        let invocation = host
            .invoke(&format!("{id}:count"), ActionInput::default())
            .unwrap();
        assert!(block_on(invocation.result()).is_err());
    }
    assert!(validate_api_marker(b"not-wasm").is_err());
    let source = fixture(&root, "forged");
    let mut bytes = FIXTURE.to_vec();
    let start = bytes
        .windows(b"nyaterm:plugin-api".len())
        .position(|w| w == b"nyaterm:plugin-api")
        .unwrap();
    bytes[start] = b'x';
    fs::write(source.join("plugin.wasm"), &bytes).unwrap();
    assert_eq!(
        host.install(&source, false).err().unwrap().code,
        ErrorCode::Incompatible
    );
    // A valid version marker cannot compensate for a different actual interface.
    let mut incompatible = FIXTURE.to_vec();
    let positions: Vec<_> = incompatible
        .windows(6)
        .enumerate()
        .filter_map(|(i, bytes)| (bytes == b"invoke").then_some(i))
        .collect();
    assert!(!positions.is_empty());
    for i in positions {
        incompatible[i + 5] = b'x';
    }
    validate_api_marker(&incompatible).unwrap();
    fs::write(source.join("plugin.wasm"), incompatible).unwrap();
    assert!(host.install(&source, false).is_err());
    let source = fixture(&root, "valid");
    host.install(&source, false).unwrap();
    assert!(
        host.invoke("valid:undeclared", ActionInput::default())
            .is_err()
    );
    let input = ActionInput {
        text: Some("x".repeat(MAX_TEXT_BYTES + 1)),
        ..ActionInput::default()
    };
    assert!(host.invoke("valid:count", input).is_err());
    assert_eq!(
        host.install(&source, false).err().unwrap().code,
        ErrorCode::Conflict
    );
}

#[test]
fn declarative_contributions_do_not_write_any_user_command_catalog() {
    let root = root();
    let mut host = manager(&root);
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/templates");
    let original = b"{\"commands\":[{\"id\":\"user-command\",\"command\":\"echo keep\"}]}";
    fs::write(root.join("quick_commands.json"), original).unwrap();
    host.install(&source, false).unwrap();
    assert!(
        matches!(block_on(host.invoke("shell-templates:inspect", ActionInput::default()).unwrap().result()).unwrap(), ActionResult::Command { command, .. } if command.trim() == "ls -lah -- '.'")
    );
    host.set_enabled("shell-templates", false).unwrap();
    assert!(host.snapshot().contributions.is_empty());
    host.set_enabled("shell-templates", true).unwrap();
    host.reload("shell-templates").unwrap();
    assert_eq!(host.snapshot().contributions.len(), 1);
    host.uninstall("shell-templates").unwrap();
    assert!(host.snapshot().contributions.is_empty());
    assert_eq!(
        fs::read(root.join("quick_commands.json")).unwrap(),
        original
    );
}

#[test]
fn failed_preference_commit_rolls_back_actual_promoted_directories() {
    use nyaterm_core::plugins::preferences::PluginPreferences;
    use nyaterm_plugin_host::package::{StagingDirectory, promote};
    use nyaterm_store::plugin_preferences::PluginPreferenceStore;
    let root = root();
    let staging = root.join("staging");
    fs::create_dir(&staging).unwrap();
    let target = root.join("installed");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("version"), b"old").unwrap();
    let mut candidate = StagingDirectory::new(&staging).unwrap();
    fs::write(candidate.path.join("version"), b"new").unwrap();
    let store = PluginPreferenceStore::open(&root.join("preferences.redb")).unwrap();
    let invalid = PluginPreferences {
        schema_version: 999,
        ..PluginPreferences::default()
    };
    assert!(promote(&mut candidate, &target, &staging, || store.save(&invalid)).is_err());
    assert_eq!(fs::read(target.join("version")).unwrap(), b"old");
    assert_eq!(fs::read(candidate.path.join("version")).unwrap(), b"new");
    assert_eq!(store.load().unwrap(), PluginPreferences::default());
}

#[cfg(windows)]
#[test]
fn windows_in_use_file_does_not_destroy_old_installation() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = root();
    let mut host = manager(&root);
    let source = fixture(&root, "locked");
    host.install(&source, false).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(root.join("host/installed/locked/plugin.wasm"))
        .unwrap();
    assert!(host.update("locked", &source).is_err());
    assert_eq!(text(call(&host, "locked", "count")), "1");
    assert_eq!(
        fs::read(root.join("host/installed/locked/plugin.wasm")).unwrap(),
        FIXTURE
    );
    drop(lock);
    host.update("locked", &source).unwrap();
    host.uninstall("locked").unwrap();
}

#[cfg(windows)]
#[test]
fn windows_junction_directory_cannot_escape_snapshot_validation() {
    let root = root();
    let mut host = manager(&root);
    let source = fixture(&root, "linked");
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(source.join("linked-directory"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(result.status.success(), "junction creation failed");
    assert_eq!(
        host.install(&source, false).err().unwrap().code,
        ErrorCode::UnsafePath
    );
    // Remove only the junction, never recurse into its target.
    fs::remove_dir(source.join("linked-directory")).unwrap();
}

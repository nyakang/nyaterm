//! Opt-in acceptance against published packages or two locally signed test versions.
use std::path::PathBuf;
use std::sync::Arc;

use futures::executor::block_on;
use nyaterm_core::plugins::native::NativeComponentId;
use nyaterm_core::runtime::{AppRuntime, RuntimeMode};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_plugin_host::native::{NativeAction, NativeService};

fn component() -> NativeComponentId {
    let name = std::env::var("NYATERM_TEST_NATIVE_COMPONENT").unwrap_or_else(|_| "vnc".into());
    let id = NativeComponentId::ALL
        .into_iter()
        .find(|id| id.slug() == name)
        .expect("unknown acceptance component");
    assert!(
        std::env::var_os(id.override_variable()).is_none(),
        "unset helper override for acceptance"
    );
    id
}

fn service(root: &TestTempDir) -> Arc<NativeService> {
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    NativeService::start(&runtime, Arc::new(|| {})).unwrap()
}

fn run(service: &NativeService, id: NativeComponentId, action: NativeAction) {
    block_on(service.submit(id, action).unwrap())
        .unwrap()
        .unwrap();
}

#[test]
#[ignore = "downloads an official native release; requires the public signed catalog"]
fn public_release_installs_and_reopens_in_isolated_registry() {
    let root = TestTempDir::new("nyaterm-native-public-acceptance");
    let id = component();
    let installer = service(&root);
    run(&installer, id, NativeAction::Probe);
    run(&installer, id, NativeAction::Install);
    let launch = block_on(installer.acquire(id).unwrap()).unwrap().unwrap();
    assert!(launch.executable.is_file());
    assert!(launch.root.starts_with(root.path()));
    let expected = launch.executable.clone();
    drop(launch);
    installer.shutdown();
    drop(installer);
    let reopened = service(&root);
    let launch = block_on(reopened.acquire(id).unwrap()).unwrap().unwrap();
    assert_eq!(launch.executable, expected);
    drop(launch);
    reopened.shutdown();
}

#[test]
#[ignore = "requires two different compatible signed ZIPs in NYATERM_NATIVE_TEST_OLD/NEW"]
fn signed_import_update_rollback_and_failed_install_preserve_selection() {
    let root = TestTempDir::new("nyaterm-native-update-acceptance");
    let id = component();
    let installer = service(&root);
    let old = PathBuf::from(std::env::var_os("NYATERM_NATIVE_TEST_OLD").expect("old signed ZIP"));
    let new = PathBuf::from(std::env::var_os("NYATERM_NATIVE_TEST_NEW").expect("new signed ZIP"));
    run(&installer, id, NativeAction::Import(old));
    let old_launch = block_on(installer.acquire(id).unwrap()).unwrap().unwrap();
    run(&installer, id, NativeAction::Import(new));
    let new_launch = block_on(installer.acquire(id).unwrap()).unwrap().unwrap();
    assert_ne!(old_launch.root, new_launch.root);
    assert!(old_launch.executable.is_file());
    run(&installer, id, NativeAction::Rollback);
    let reverted = block_on(installer.acquire(id).unwrap()).unwrap().unwrap();
    assert_eq!(reverted.root, old_launch.root);
    assert!(new_launch.executable.is_file());
    let corrupt = root.join("corrupt.zip");
    std::fs::write(&corrupt, b"not a signed package").unwrap();
    assert!(
        block_on(installer.submit(id, NativeAction::Import(corrupt)).unwrap())
            .unwrap()
            .is_err()
    );
    let after_failure = block_on(installer.acquire(id).unwrap()).unwrap().unwrap();
    assert_eq!(after_failure.root, old_launch.root);
    drop((old_launch, new_launch, reverted, after_failure));
    installer.shutdown();
}

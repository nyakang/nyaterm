use crate::runtime::RuntimeLimits;
use crate::service::{DispatchHook, OperationReply, PluginOperation, PluginService, TestHooks};
use futures::{StreamExt, executor::block_on};
use nyaterm_core::plugins::invocation::{ActionInput, ActionResult};
use nyaterm_core::plugins::{ErrorCode, PluginStatus};
use nyaterm_core::runtime::{AppRuntime, RuntimeMode};
use nyaterm_core::test_support::TestTempDir;
use std::fs;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::thread;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

fn runtime(root: &TestTempDir) -> AppRuntime {
    AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    )
}

fn fixture(root: &TestTempDir, id: &str) -> std::path::PathBuf {
    let source = root.join(id);
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("plugin.toml"),
        format!(
            "schema_version = 2\nid = \"{id}\"\nname = \"Service fixture\"\nversion = \"1.0.0\"\nauthors = []\ndescription = \"SDK guest\"\nhost_version = \">=2.0.0-preview.4\"\napi_version = \"2.0.0\"\ncomponent = \"plugin.wasm\"\n\n[[actions]]\nid = \"count\"\nname = \"Count\"\ndescription = \"Count calls\"\nresult = \"text\"\n\n[[actions]]\nid = \"loop\"\nname = \"Loop\"\ndescription = \"Infinite loop\"\nresult = \"text\"\n"
        ),
    )
    .unwrap();
    fs::write(
        source.join("plugin.wasm"),
        include_bytes!("../../tests/fixtures/lifecycle.wasm"),
    )
    .unwrap();
    source
}

fn install(service: &Arc<PluginService>, root: &TestTempDir, id: &str) {
    block_on(
        service
            .submit(PluginOperation::Install {
                source: fixture(root, id),
                development: false,
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
}

fn invocation(service: &Arc<PluginService>, id: &str, action: &str) -> PluginOperation {
    PluginOperation::Invoke {
        contribution_id: format!("{id}:{action}"),
        expected_revision: service
            .snapshot()
            .contributions
            .iter()
            .find(|c| c.plugin_id == id)
            .unwrap()
            .revision,
        input: ActionInput::default(),
    }
}

fn count(service: &Arc<PluginService>, id: &str) -> String {
    let reply = block_on(service.submit(invocation(service, id, "count")).unwrap())
        .unwrap()
        .unwrap();
    let OperationReply::Invocation(call) = reply else {
        panic!("missing invocation");
    };
    let ActionResult::Text(text) = block_on(call.result()).unwrap() else {
        panic!("missing count");
    };
    text
}

#[test]
fn explicit_grant_preserves_guest_state_and_stale_revision_cannot_grant() {
    let root = TestTempDir::new("nyaterm-plugin-live-approval");
    let source = fixture(&root, "approval");
    let path = source.join("plugin.toml");
    let manifest = fs::read_to_string(&path).unwrap().replace(
        "schema_version = 2",
        "schema_version = 2\ncapabilities = [\"sessions_read\"]",
    );
    fs::write(path, manifest).unwrap();
    let (service, _events) = PluginService::start(runtime(&root)).unwrap();
    block_on(
        service
            .submit(PluginOperation::Install {
                source,
                development: false,
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(count(&service, "approval"), "1");
    let revision = service.snapshot().plugins[0].revision;
    assert!(!service.snapshot().plugins[0].session_events_authorized);
    let authorize = |expected_revision| PluginOperation::Authorize {
        id: "approval".into(),
        expected_revision,
        capability: "sessions_read".into(),
        scope: String::new(),
    };
    assert!(
        block_on(service.submit(authorize(revision + 1)).unwrap())
            .unwrap()
            .is_err()
    );
    block_on(service.submit(authorize(revision)).unwrap())
        .unwrap()
        .unwrap();
    assert!(service.snapshot().plugins[0].session_events_authorized);
    assert_eq!(service.snapshot().plugins[0].revision, revision);
    assert_eq!(count(&service, "approval"), "2");
    block_on(
        service
            .submit(PluginOperation::RevokeAuthorizations {
                id: "approval".into(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(!service.snapshot().plugins[0].session_events_authorized);
    assert!(!service.snapshot().plugins[0].enabled);
    service.shutdown();
}

// A one-shot barrier pauses the actual actor at a known dispatch boundary.
fn gate(predicate: fn(&PluginOperation) -> bool) -> (DispatchHook, Receiver<()>, SyncSender<()>) {
    let (entered, arrival) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let resume = Mutex::new(resume);
    let armed = AtomicBool::new(true);
    let hook = Arc::new(move |operation: &PluginOperation| {
        if predicate(operation) && armed.swap(false, Ordering::AcqRel) {
            entered.send(()).unwrap();
            resume.lock().unwrap().recv_timeout(WAIT).unwrap();
        }
    });
    (hook, arrival, release)
}

#[test]
fn abandoned_queued_call_never_executes_and_keeps_guest_healthy() {
    let root = TestTempDir::new("nyaterm-plugin-service-cancel-queue");
    let (hook, arrived, release) = gate(|op| matches!(op, PluginOperation::Invoke { .. }));
    let (service, _) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            before_dispatch: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    install(&service, &root, "healthy");
    let queued = service
        .submit(invocation(&service, "healthy", "count"))
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    drop(queued);
    release.send(()).unwrap();
    assert_eq!(count(&service, "healthy"), "1");
    assert_eq!(service.snapshot().plugins[0].status, PluginStatus::Running);
    service.shutdown();
}

#[test]
fn reload_rejects_old_queued_revision_before_guest_state_changes() {
    let root = TestTempDir::new("nyaterm-plugin-service-revision");
    let (hook, arrived, release) = gate(|op| matches!(op, PluginOperation::Reload { .. }));
    let (service, _) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            before_dispatch: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    install(&service, &root, "healthy");
    let old = invocation(&service, "healthy", "count");
    let reload = service
        .submit(PluginOperation::Reload {
            id: "healthy".into(),
        })
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    let stale = service.submit(old).unwrap();
    release.send(()).unwrap();
    block_on(reload).unwrap().unwrap();
    assert_eq!(
        block_on(stale).unwrap().err().unwrap().code,
        ErrorCode::Cancelled
    );
    assert_eq!(count(&service, "healthy"), "1");
    service.shutdown();
}

#[test]
fn dropping_running_call_interrupts_real_guest_and_other_instance_still_works() {
    let root = TestTempDir::new("nyaterm-plugin-service-running-cancel");
    let (entered, arrived) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let resume = Mutex::new(resume);
    let (service, _) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits {
            fuel: u64::MAX,
            on_call_epoch: Some(Arc::new(move |action| {
                if action != "loop" {
                    return;
                }
                entered.send(()).unwrap();
                resume.lock().unwrap().recv_timeout(WAIT).unwrap();
            })),
            ..RuntimeLimits::default()
        },
        TestHooks::default(),
    )
    .unwrap();
    install(&service, &root, "broken");
    install(&service, &root, "healthy");
    let reply = block_on(
        service
            .submit(invocation(&service, "broken", "loop"))
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    let OperationReply::Invocation(call) = reply else {
        panic!("missing invocation")
    };
    let lease = call.lease();
    arrived.recv_timeout(WAIT).unwrap(); // A real guest reached its epoch check.
    drop(call);
    release.send(()).unwrap();
    // Reload joins the cancelled worker and starts a new generation.
    block_on(
        service
            .submit(PluginOperation::Reload {
                id: "broken".into(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(!lease.is_current());
    assert_eq!(count(&service, "healthy"), "1");
    assert_eq!(count(&service, "broken"), "1");
    service.shutdown();
    fs::remove_dir_all(root.join("plugins")).unwrap();
}

#[test]
fn accepted_management_survives_receiver_close_and_publishes_to_other_observers() {
    let root = TestTempDir::new("nyaterm-plugin-service-management-close");
    let (hook, arrived, release) = gate(|op| matches!(op, PluginOperation::Install { .. }));
    let (service, mut events) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            after_start: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    let task = service
        .submit(PluginOperation::Install {
            source: fixture(&root, "healthy"),
            development: false,
        })
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    drop(task);
    release.send(()).unwrap();
    // A second window's ordered request observes the completed transaction even
    // though the first window stopped waiting for its reply.
    block_on(
        service
            .submit(PluginOperation::Reload {
                id: "healthy".into(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(block_on(events.next()).is_some());
    assert!(!service.snapshot().contributions.is_empty());
    assert_eq!(count(&service, "healthy"), "1");
    service.shutdown();
    drop(service);
    let (service, _) = PluginService::start(runtime(&root)).unwrap();
    // Ordered reload waits for startup and proves enabled preferences persisted.
    block_on(
        service
            .submit(PluginOperation::Reload {
                id: "healthy".into(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(count(&service, "healthy"), "1");
    service.shutdown();
}

#[test]
fn shutdown_finishes_started_transaction_rejects_queue_and_joins_concurrent_callers() {
    let root = TestTempDir::new("nyaterm-plugin-service-shutdown");
    let (hook, arrived, release) = gate(|op| matches!(op, PluginOperation::Install { .. }));
    let (service, events) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            after_start: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    let current = service
        .submit(PluginOperation::Install {
            source: fixture(&root, "healthy"),
            development: false,
        })
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    let mut queued = Vec::new();
    for _ in 0..16 {
        queued.push(
            service
                .submit(PluginOperation::Reload {
                    id: "healthy".into(),
                })
                .unwrap(),
        );
    }
    assert_eq!(
        service
            .submit(PluginOperation::Reload {
                id: "healthy".into()
            })
            .err()
            .unwrap()
            .code,
        ErrorCode::QueueFull
    );
    service.begin_shutdown();
    assert_eq!(
        service
            .submit(PluginOperation::Uninstall {
                id: "healthy".into()
            })
            .err()
            .unwrap()
            .code,
        ErrorCode::Shutdown
    );
    thread::scope(|scope| {
        let first = scope.spawn(|| service.shutdown());
        let second = scope.spawn(|| service.shutdown());
        release.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
    });
    assert!(matches!(
        block_on(current).unwrap().unwrap(),
        OperationReply::Changed
    ));
    for task in queued {
        assert_eq!(
            block_on(task).unwrap().err().unwrap().code,
            ErrorCode::Shutdown
        );
    }
    assert!(service.snapshot().stopped);
    assert!(service.snapshot().contributions.is_empty());
    assert!(!block_on(events.collect::<Vec<_>>()).is_empty());
    service.shutdown();
    assert!(
        root.join("plugins/v2/installed/healthy/plugin.toml")
            .exists()
    );
    // Database, component and epoch resources are reclaimed before return.
    fs::remove_dir_all(root.join("plugins")).unwrap();
}

#[test]
fn shutdown_before_dispatch_never_starts_queued_management() {
    let root = TestTempDir::new("nyaterm-plugin-service-queued-shutdown");
    let (hook, arrived, release) = gate(|op| matches!(op, PluginOperation::Install { .. }));
    let (service, events) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            before_dispatch: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    let queued = service
        .submit(PluginOperation::Install {
            source: fixture(&root, "healthy"),
            development: false,
        })
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    service.begin_shutdown();
    release.send(()).unwrap();
    service.shutdown();
    assert_eq!(
        block_on(queued).unwrap().err().unwrap().code,
        ErrorCode::Shutdown
    );
    assert!(!root.join("plugins/v2/installed/healthy").exists());
    assert!(service.snapshot().stopped);
    block_on(events.collect::<Vec<_>>());
    fs::remove_dir_all(root.join("plugins")).unwrap();
}

#[test]
fn failed_startup_closes_event_stream_and_preserves_error_and_preferences() {
    let root = TestTempDir::new("nyaterm-plugin-service-startup-failure");
    let path = root.join("plugins/v2/dev/preferences.redb");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"corrupt preference fixture").unwrap();
    let before = fs::read(&path).unwrap();
    let (service, events) = PluginService::start(runtime(&root)).unwrap();
    let error = block_on(
        service
            .submit(PluginOperation::Reload {
                id: "healthy".into(),
            })
            .unwrap(),
    )
    .unwrap()
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::Storage);
    service.shutdown();
    service.shutdown();
    assert!(service.snapshot().stopped);
    assert_eq!(
        service.snapshot().startup_error.unwrap().code,
        ErrorCode::Storage
    );
    assert!(!block_on(events.collect::<Vec<_>>()).is_empty());
    assert_eq!(fs::read(path).unwrap(), before);
    fs::remove_dir_all(root.join("plugins")).unwrap();
}

#[test]
fn different_plugins_prepare_concurrently_and_invocations_bypass_preparation() {
    let root = TestTempDir::new("nyaterm-plugin-service-prepare-concurrency");
    let (hook, arrived, release) =
        gate(|op| matches!(op, PluginOperation::Reload { id } if id == "slow"));
    let (service, _) = PluginService::start_inner(
        runtime(&root),
        RuntimeLimits::default(),
        TestHooks {
            before_prepare: Some(hook),
            ..TestHooks::default()
        },
    )
    .unwrap();
    install(&service, &root, "slow");
    install(&service, &root, "other");
    let slow = service
        .submit(PluginOperation::Reload { id: "slow".into() })
        .unwrap();
    arrived.recv_timeout(WAIT).unwrap();
    // Both dispatch and execution complete while the other package worker is
    // deliberately paused. A serial control plane cannot pass this barrier.
    let (done, completed) = mpsc::sync_channel(1);
    let observer = service.clone();
    let probe = thread::spawn(move || {
        let first = count(&observer, "other");
        block_on(
            observer
                .submit(PluginOperation::Reload { id: "other".into() })
                .unwrap(),
        )
        .unwrap()
        .unwrap();
        done.send((first, count(&observer, "other"))).unwrap();
    });
    let outcome = completed.recv_timeout(WAIT);
    release.send(()).unwrap();
    probe.join().unwrap();
    assert_eq!(outcome.unwrap(), ("1".into(), "1".into()));
    block_on(slow).unwrap().unwrap();
    service.shutdown();
}

#[test]
fn real_guest_fault_updates_catalog_without_another_request_or_polling() {
    let root = TestTempDir::new("nyaterm-plugin-service-fault-event");
    let (service, mut events) = PluginService::start(runtime(&root)).unwrap();
    install(&service, &root, "broken");
    assert_eq!(service.snapshot().plugins[0].status, PluginStatus::Running);
    let OperationReply::Invocation(call) = block_on(
        service
            .submit(invocation(&service, "broken", "loop"))
            .unwrap(),
    )
    .unwrap()
    .unwrap() else {
        panic!("missing invocation");
    };
    assert!(block_on(call.result()).is_err());
    let (done, completed) = mpsc::sync_channel(1);
    let observer = service.clone();
    let listener = thread::spawn(move || {
        let faulted = block_on(async {
            // Native startup and installation share this stream. Keep their
            // queued events and inspect the snapshot only when an event arrives.
            while events.next().await.is_some() {
                let snapshot = observer.snapshot();
                if snapshot.stopped {
                    return false;
                }
                if snapshot.plugins[0].status == PluginStatus::Faulted {
                    return true;
                }
            }
            false
        });
        done.send(faulted).unwrap();
    });
    let outcome = completed.recv_timeout(WAIT);
    service.shutdown();
    listener.join().unwrap();
    assert!(outcome.unwrap());
    assert!(service.snapshot().stopped);
}

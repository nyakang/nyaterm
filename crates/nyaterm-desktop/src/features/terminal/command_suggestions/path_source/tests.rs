use super::{CacheKey, Claim, PathCompletionRuntime, PathSource, Running, State};
use crate::blocking_jobs::BlockingJobScheduler;
use futures::{FutureExt, channel::oneshot};
use nyaterm_core::completion::path::path_query_for_input;
use nyaterm_core::completion::types::{CompletionScope, ShellDialect};
use nyaterm_files::{file_browser::FileBrowserService, sftp::SftpTransferControl};
use std::sync::{Arc, Mutex};

fn source(dir: &std::path::Path) -> PathSource {
    let epoch = Arc::new(());
    let mut state = State::default();
    state.epochs.insert("session".into(), epoch.clone());
    PathSource {
        epoch,
        service: FileBrowserService::local(),
        query: path_query_for_input("cd pr", ShellDialect::Bash, CompletionScope::legacy())
            .unwrap(),
        key: CacheKey {
            session: "session".into(),
            context: "identity".into(),
            directory: dir.to_string_lossy().into_owned(),
        },
        state: Arc::new(Mutex::new(state)),
        jobs: BlockingJobScheduler::with_limits(1, 8).unwrap(),
    }
}

#[test]
fn cancelled_waiters_cannot_restart_io_for_an_obsolete_context() {
    let dir = tempfile::tempdir().unwrap();
    let source = source(dir.path());
    let runtime = PathCompletionRuntime {
        state: source.state.clone(),
        jobs: None,
    };
    let (release, wait) = std::sync::mpsc::channel();
    let blocker = source
        .jobs
        .submit_task("blocked", move |_| wait.recv().unwrap())
        .unwrap();
    assert!(source.listing().now_or_never().is_none());
    let mut next = source.clone();
    next.key.directory.push_str("/other");
    let mut waiting = Box::pin(next.listing());
    assert!(waiting.as_mut().now_or_never().is_none());
    runtime.cancel_session("session");
    release.send(()).unwrap();
    futures::executor::block_on(blocker).unwrap();
    assert!(futures::executor::block_on(waiting).is_none());
    assert_eq!(source.jobs.metrics().submitted, 2);
    assert!(source.state.lock().unwrap().listings.is_empty());
}
#[test]
fn a_directory_listing_is_shared_across_keystrokes_and_cached() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("projects")).unwrap();
    let source = source(dir.path());
    let (release, wait) = std::sync::mpsc::channel();
    let blocked = source
        .jobs
        .submit_task("test-held-worker", move |_| wait.recv().unwrap())
        .unwrap();
    assert!(source.candidates("cd pr").now_or_never().is_none());
    for _ in 0..20 {
        assert!(source.candidates("cd pr").now_or_never().is_none());
    }
    assert_eq!(
        source.jobs.metrics().submitted,
        2,
        "one blocker and one listing"
    );
    release.send(()).unwrap();
    futures::executor::block_on(blocked).unwrap();
    let candidates = futures::executor::block_on(source.candidates("cd pr"));
    assert_eq!(candidates[0].target_line("cd pr").unwrap(), "cd projects/");
    std::fs::remove_dir(dir.path().join("projects")).unwrap();
    assert_eq!(
        futures::executor::block_on(source.candidates("cd pr")).len(),
        1
    );
    assert_eq!(source.jobs.metrics().submitted, 2);
}
#[test]
fn an_old_claim_cannot_remove_or_cache_over_a_new_owner() {
    let state = Arc::new(Mutex::new(State::default()));
    let key = CacheKey {
        session: "s".into(),
        context: "old".into(),
        directory: "/".into(),
    };
    let (sender, receiver) = oneshot::channel();
    let answer = receiver.map(|v| v.unwrap_or(None)).boxed().shared();
    let mut old = Claim {
        state: state.clone(),
        key: key.clone(),
        id: 1,
        response: Some(sender),
        control: SftpTransferControl::default(),
    };
    state.lock().unwrap().running.insert(
        "s".into(),
        Running {
            id: 2,
            key,
            answer,
            control: SftpTransferControl::default(),
        },
    );
    old.finish(Some(Arc::new(Vec::new())));
    let state = state.lock().unwrap();
    assert_eq!(state.running["s"].id, 2);
    assert!(state.listings.is_empty());
}
#[test]
fn an_unexecuted_claim_releases_its_waiters_on_drop() {
    let state = Arc::new(Mutex::new(State::default()));
    let key = CacheKey {
        session: "s".into(),
        context: "c".into(),
        directory: "/".into(),
    };
    let (sender, receiver) = oneshot::channel();
    let answer = receiver.map(|v| v.unwrap_or(None)).boxed().shared();
    state.lock().unwrap().running.insert(
        "s".into(),
        Running {
            id: 1,
            key: key.clone(),
            answer: answer.clone(),
            control: SftpTransferControl::default(),
        },
    );
    drop(Claim {
        state: state.clone(),
        key,
        id: 1,
        response: Some(sender),
        control: SftpTransferControl::default(),
    });
    assert!(futures::executor::block_on(answer).is_none());
    assert!(state.lock().unwrap().running.is_empty());
}

#[cfg(unix)]
#[test]
fn directory_links_are_shared_cached_and_can_continue_into_escaped_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("target")).unwrap();
    std::fs::create_dir(dir.path().join("target/child")).unwrap();
    std::fs::write(dir.path().join("file"), []).unwrap();
    for (name, target) in [
        ("project $link", "target"),
        ("project-file", "file"),
        ("project-broken", "missing"),
        ("project-loop", "project-loop"),
    ] {
        std::os::unix::fs::symlink(target, dir.path().join(name)).unwrap();
    }
    let source = source(dir.path());
    let candidates = futures::executor::block_on(source.candidates("cd pr"));
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].target_line("cd pr").unwrap(),
        r"cd project\ \$link/"
    );
    assert_eq!(source.jobs.metrics().submitted, 2);
    let (release, wait) = std::sync::mpsc::channel();
    let blocked = source
        .jobs
        .submit_task("test-held-link-worker", move |_| wait.recv().unwrap())
        .unwrap();
    // A cached probe succeeds even while the I/O worker cannot run.
    assert_eq!(
        futures::executor::block_on(source.candidates("cd pr")).len(),
        1
    );
    assert_eq!(source.jobs.metrics().submitted, 3);
    release.send(()).unwrap();
    futures::executor::block_on(blocked).unwrap();
    let mut next = source.clone();
    next.query = path_query_for_input(
        r"cd project\ \$link/ch",
        ShellDialect::Bash,
        CompletionScope::legacy(),
    )
    .unwrap();
    next.key.directory = format!("{}/project $link/", dir.path().display());
    assert_eq!(
        futures::executor::block_on(next.candidates(r"cd project\ \$link/ch"))[0]
            .target_line(r"cd project\ \$link/ch")
            .unwrap(),
        r"cd project\ \$link/child/"
    );
}

#[test]
fn link_probe_waiters_coalesce_and_cancellation_cannot_repopulate_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = source(dir.path());
    let runtime = PathCompletionRuntime {
        state: source.state.clone(),
        jobs: None,
    };
    let (release, wait) = std::sync::mpsc::channel();
    let blocked = source
        .jobs
        .submit_task("test-held-probe", move |_| wait.recv().unwrap())
        .unwrap();
    let names = vec!["project-link".into()];
    for _ in 0..20 {
        assert!(source.probe_links(names.clone()).now_or_never().is_none());
    }
    assert_eq!(source.jobs.metrics().submitted, 2);
    runtime.cancel_session("session");
    release.send(()).unwrap();
    futures::executor::block_on(blocked).unwrap();
    assert!(futures::executor::block_on(source.probe_links(names)).is_none());
    assert!(source.state.lock().unwrap().links.is_empty());
    assert_eq!(source.jobs.metrics().submitted, 2);
}

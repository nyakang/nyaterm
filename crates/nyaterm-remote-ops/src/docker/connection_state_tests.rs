use crate::docker::{DockerConnectionState, DockerElevation};
use std::sync::Arc;

#[test]
fn jobs_sharing_a_connection_observe_one_elevation_state() {
    let state = DockerConnectionState::default();
    let first_job = state.clone();
    let next_job = state.clone();
    std::thread::spawn(move || {
        *first_job.inner.lock().unwrap() = Some(DockerElevation::Sudo);
    })
    .join()
    .unwrap();
    assert!(matches!(
        *next_job.inner.lock().unwrap(),
        Some(DockerElevation::Sudo)
    ));
    assert!(Arc::ptr_eq(&state.inner, &next_job.inner));
}

#[test]
fn replacement_connections_start_without_the_previous_elevation_or_password() {
    let previous = DockerConnectionState::default();
    *previous.inner.lock().unwrap() = Some(DockerElevation::Password(
        "test-only-elevation-secret".into(),
    ));
    let replacement = DockerConnectionState::default();
    assert!(replacement.inner.lock().unwrap().is_none());
    assert!(!Arc::ptr_eq(&previous.inner, &replacement.inner));
    assert!(!format!("{previous:?}").contains("test-only-elevation-secret"));
}

#[test]
fn the_final_connection_or_job_owner_releases_elevation_state() {
    let state = DockerConnectionState::default();
    let pending_job = state.clone();
    let weak = Arc::downgrade(&state.inner);
    drop(state);
    assert!(weak.upgrade().is_some());
    drop(pending_job);
    assert!(weak.upgrade().is_none());
}

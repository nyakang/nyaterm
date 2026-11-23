use super::run_resolved;
use crate::acp::{AgentError, AgentEvent, AgentOperation};
use nyaterm_core::ai::external_agents::{
    AgentEnvironmentVariable, ExternalAgentProfile, ExternalSessionBinding,
};
use nyaterm_core::test_support::TestTempDir;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn fixture(scenario: &str) -> (TestTempDir, ExternalAgentProfile) {
    let directory = TestTempDir::new("nyaterm-acp-runtime");
    std::fs::create_dir_all(directory.path()).unwrap();
    let script = directory.join("fixture.ps1");
    std::fs::write(&script, include_str!("fixture.ps1")).unwrap();
    let wrapper = directory.join("fixture.cmd");
    std::fs::write(&wrapper, format!("@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"\r\n",script.display())).unwrap();
    let mut profile = ExternalAgentProfile::new("fixture".into(), "claude");
    profile.executable = wrapper.to_string_lossy().into_owned();
    profile.working_directory = Some(directory.path().to_string_lossy().into_owned());
    profile.environment.push(AgentEnvironmentVariable {
        id: "scenario".into(),
        name: "TEST_SCENARIO".into(),
        value: scenario.into(),
    });
    (directory, profile)
}
fn prompt(binding: Option<ExternalSessionBinding>) -> AgentOperation {
    AgentOperation::Prompt {
        text: "fixture".into(),
        model: Some("fixture".into()),
        binding: binding.map(Box::new),
        legacy_session_id: None,
    }
}
fn execute(
    profile: &ExternalAgentProfile,
    operation: AgentOperation,
    timeout: Duration,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(AgentEvent) -> Result<(), String>,
    revoke: &mut dyn FnMut(),
) -> Result<super::AgentRunResult, AgentError> {
    run_resolved(
        profile,
        std::path::Path::new(&profile.executable),
        None,
        operation,
        Some(std::path::Path::new(&profile.executable)),
        HashMap::from([("REQUEST_TOKEN".into(), "fixture-only".into())]),
        cancel,
        timeout,
        sink,
        revoke,
    )
}
#[test]
fn session_resume_discards_replay_and_foreign_notifications() {
    let (_directory, profile) = fixture("success");
    let cancel = AtomicBool::new(false);
    let first = execute(
        &profile,
        prompt(None),
        Duration::from_secs(10),
        &cancel,
        &mut |_| Ok(()),
        &mut || {},
    )
    .unwrap();
    assert_eq!(first.text, "OK");
    let second = execute(
        &profile,
        prompt(first.binding),
        Duration::from_secs(10),
        &cancel,
        &mut |_| Ok(()),
        &mut || {},
    )
    .unwrap();
    assert_eq!(second.text, "OK");
}
#[test]
fn foreign_sessions_and_native_tools_cannot_be_authorized_by_their_title() {
    for scenario in ["permission", "foreign-permission"] {
        let (_directory, profile) = fixture(scenario);
        let result = execute(
            &profile,
            prompt(None),
            Duration::from_secs(10),
            &AtomicBool::new(false),
            &mut |_| Ok(()),
            &mut || {},
        )
        .unwrap();
        assert_eq!(result.text, "DENIED");
    }
}
#[test]
fn eof_oversized_frames_refusals_and_closed_consumer_revoke_before_cleanup() {
    for scenario in ["eof", "oversized", "refusal", "success"] {
        let (_directory, profile) = fixture(scenario);
        let mut revoked = false;
        let result = execute(
            &profile,
            prompt(None),
            Duration::from_secs(10),
            &AtomicBool::new(false),
            &mut |_| {
                if scenario == "success" {
                    Err("closed".into())
                } else {
                    Ok(())
                }
            },
            &mut || revoked = true,
        );
        assert!(result.is_err(), "{scenario}");
        assert!(revoked, "{scenario}");
    }
}
#[test]
fn cancellation_notifies_agent_and_setup_timeout_reaps_owned_process() {
    let (_directory, profile) = fixture("hang");
    let cancel = AtomicBool::new(false);
    let mut revoked = false;
    let started = Instant::now();
    let result = execute(
        &profile,
        prompt(None),
        Duration::from_secs(10),
        &cancel,
        &mut |event| {
            if matches!(event, AgentEvent::Session(_)) {
                cancel.store(true, Ordering::Release);
            }
            Ok(())
        },
        &mut || revoked = true,
    );
    assert!(matches!(result, Err(AgentError::Cancelled)));
    assert!(revoked);
    assert!(started.elapsed() < Duration::from_secs(8));
    let (_directory, profile) = fixture("hang-init");
    let result = execute(
        &profile,
        prompt(None),
        Duration::from_millis(100),
        &AtomicBool::new(false),
        &mut |_| Ok(()),
        &mut || {},
    );
    assert!(matches!(result, Err(AgentError::Timeout)));
}
#[test]
fn failed_restore_never_creates_a_replacement_session() {
    let (_directory, mut profile) = fixture("success");
    let first = execute(
        &profile,
        prompt(None),
        Duration::from_secs(10),
        &AtomicBool::new(false),
        &mut |_| Ok(()),
        &mut || {},
    )
    .unwrap();
    profile.environment[0].value = "restore-error".into();
    let result = execute(
        &profile,
        prompt(first.binding),
        Duration::from_secs(10),
        &AtomicBool::new(false),
        &mut |event| {
            assert!(!matches!(event, AgentEvent::Session(_)));
            Ok(())
        },
        &mut || {},
    );
    assert!(matches!(result, Err(AgentError::Resume)));
}

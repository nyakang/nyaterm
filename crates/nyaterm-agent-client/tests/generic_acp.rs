#![cfg(feature = "test-support")]

use nyaterm_agent_client::acp::{AgentError, AgentEvent, AgentOperation, AgentRunResult, run};
use nyaterm_core::ai::external_agents::{ExternalAgentProfile, ExternalSessionBinding};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn profile(scenario: &str, directory: &Path) -> ExternalAgentProfile {
    let mut profile = ExternalAgentProfile::new("fixture".into(), "custom_acp");
    profile.enabled = true;
    profile.executable = env!("CARGO_BIN_EXE_nyaterm-acp-test-agent").into();
    profile.args = vec!["fixture argument with spaces".into(), scenario.into()];
    profile.working_directory = Some(directory.to_string_lossy().into_owned());
    profile.default_model = Some("fixture".into());
    profile
}
fn prompt(binding: Option<ExternalSessionBinding>) -> AgentOperation {
    AgentOperation::Prompt {
        text: "fixture".into(),
        model: None,
        binding: binding.map(Box::new),
        legacy_session_id: None,
    }
}
fn execute(
    profile: &ExternalAgentProfile,
    operation: AgentOperation,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(AgentEvent) -> Result<(), String>,
) -> Result<AgentRunResult, AgentError> {
    run(
        profile,
        operation,
        Some(Path::new(&profile.executable)),
        HashMap::from([("REQUEST_TOKEN".into(), "fresh".into())]),
        cancel,
        Duration::from_secs(5),
        sink,
        &mut || {},
    )
}

#[test]
fn handshake_optional_auth_models_and_fresh_process_resume() {
    let directory = tempfile::tempdir().unwrap();
    for scenario in ["modern-model", "legacy-model"] {
        let profile = profile(scenario, directory.path());
        let cancel = AtomicBool::new(false);
        let mut capabilities = false;
        execute(&profile, AgentOperation::Inspect, &cancel, &mut |event| {
            if let AgentEvent::Capabilities {
                auth_methods,
                logout,
            } = event
            {
                assert_eq!(auth_methods.len(), 1);
                assert!(!logout);
                capabilities = true;
            }
            Ok(())
        })
        .unwrap();
        assert!(capabilities);
        execute(
            &profile,
            AgentOperation::Authenticate {
                method: "login".into(),
            },
            &cancel,
            &mut |_| Ok(()),
        )
        .unwrap();
        let first = execute(&profile, prompt(None), &cancel, &mut |_| Ok(())).unwrap();
        assert_eq!(first.text, "fresh");
        let second = run(
            &profile,
            prompt(first.binding),
            Some(Path::new(&profile.executable)),
            HashMap::from([("REQUEST_TOKEN".into(), "second-request".into())]),
            &cancel,
            Duration::from_secs(5),
            &mut |_| Ok(()),
            &mut || {},
        )
        .unwrap();
        assert_eq!(second.text, "second-request");
    }
}

#[test]
fn permissions_do_not_block_other_requests_or_streaming() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile("permissions", directory.path());
    let mut pending = Vec::new();
    let mut streaming = false;
    let result = execute(
        &profile,
        prompt(None),
        &AtomicBool::new(false),
        &mut |event| {
            match event {
                AgentEvent::Permission(request) => pending.push(request),
                AgentEvent::Delta { text, .. } if text == "waiting;" => streaming = true,
                _ => {}
            }
            if pending.len() == 2 && streaming {
                for request in &pending {
                    if request.is_pending() {
                        assert!(request.respond(true));
                        assert!(!request.respond(true));
                    }
                }
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.text, "waiting;done");
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|request| !request.is_pending()));
}

#[test]
fn denial_foreign_sessions_and_allow_always_do_not_authorize_tools() {
    let directory = tempfile::tempdir().unwrap();
    for scenario in ["deny", "foreign", "no-once"] {
        let profile = profile(scenario, directory.path());
        let mut count = 0;
        let result = execute(
            &profile,
            prompt(None),
            &AtomicBool::new(false),
            &mut |event| {
                if let AgentEvent::Permission(request) = event {
                    count += 1;
                    assert_ne!(scenario, "foreign");
                    if scenario == "no-once" {
                        assert!(!request.view().allow_once);
                    }
                    request.respond(scenario == "no-once");
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(count, if scenario == "foreign" { 0 } else { 1 });
        assert!(result.text.ends_with("done"));
    }
}

#[test]
fn cancel_revokes_pending_permissions_and_terminates_the_peer() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile("cancel", directory.path());
    let cancel = AtomicBool::new(false);
    let mut pending = None;
    let result = execute(&profile, prompt(None), &cancel, &mut |event| {
        if let AgentEvent::Permission(request) = event {
            pending = Some(request);
            cancel.store(true, Ordering::Release);
        }
        Ok(())
    });
    assert!(matches!(result, Err(AgentError::Cancelled)));
    let pending = pending.unwrap();
    assert!(!pending.is_pending());
    assert!(!pending.respond(true));
}

#[test]
fn protocol_crash_timeout_and_unsupported_resume_fail_without_new_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    assert!(matches!(
        execute(
            &profile("bad-version", directory.path()),
            AgentOperation::Inspect,
            &cancel,
            &mut |_| Ok(())
        ),
        Err(AgentError::Protocol(_))
    ));
    assert!(
        execute(
            &profile("crash", directory.path()),
            prompt(None),
            &cancel,
            &mut |_| Ok(())
        )
        .is_err()
    );
    let profile = profile("no-resume", directory.path());
    let first = execute(&profile, prompt(None), &cancel, &mut |_| Ok(())).unwrap();
    assert!(matches!(
        execute(&profile, prompt(first.binding), &cancel, &mut |_| Ok(())),
        Err(AgentError::Resume)
    ));
    let mut timeout = profile;
    timeout.args[1] = "timeout".into();
    assert!(matches!(
        run(
            &timeout,
            AgentOperation::Inspect,
            None,
            HashMap::new(),
            &cancel,
            Duration::from_millis(30),
            &mut |_| Ok(()),
            &mut || {}
        ),
        Err(AgentError::Timeout)
    ));
}

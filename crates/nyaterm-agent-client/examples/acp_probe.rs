//! Read-only adapter handshake. Prints observations, never credentials or protocol payloads.
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use nyaterm_agent_client::acp::{AgentEvent, AgentOperation, run};
use nyaterm_core::ai::external_agents::ExternalAgentProfile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let strategy = args
        .get(1)
        .ok_or("usage: acp_probe <codex|claude> <adapter>")?;
    let mut profile = ExternalAgentProfile::new("probe".into(), strategy);
    profile.executable = args.get(2).ok_or("adapter path is required")?.clone();
    let operation = if let Some(config) = args.get(3) {
        // A fixture config is explicit. Never dump its contents or peer messages.
        profile = serde_json::from_str(&std::fs::read_to_string(config)?)?;
        AgentOperation::Prompt {
            text: "Call the acceptance_ping MCP tool once, then reply OK.".into(),
            model: None,
            binding: None,
            legacy_session_id: None,
        }
    } else {
        AgentOperation::Inspect
    };
    let helper = args.get(4).map(std::path::Path::new);
    let prompt = matches!(operation, AgentOperation::Prompt { .. });
    let first = run(
        &profile,
        operation,
        helper,
        std::collections::HashMap::from([("ACCEPTANCE_REQUEST_ID".into(), "first".into())]),
        &AtomicBool::new(false),
        Duration::from_secs(50),
        &mut |event| {
            match event {
                AgentEvent::Capabilities {
                    auth_methods,
                    logout,
                } => println!(
                    "handshake=true auth_methods={} logout={logout}",
                    auth_methods.len()
                ),
                AgentEvent::Authentication { authenticated } => {
                    println!("authenticated={authenticated:?}")
                }
                AgentEvent::Session(_) => println!("session=true"),
                AgentEvent::Models(models) => println!("models={}", models.len()),
                AgentEvent::Tool { status, .. } => println!("tool_status={status}"),
                _ => {}
            }
            Ok(())
        },
        &mut || {},
    )?;
    if prompt {
        let second = run(
            &profile,
            AgentOperation::Prompt {
                text: "NYATERM_ACCEPTANCE_SECOND: Call acceptance_ping again, then reply OK."
                    .into(),
                model: None,
                binding: first.binding.map(Box::new),
                legacy_session_id: None,
            },
            helper,
            std::collections::HashMap::from([("ACCEPTANCE_REQUEST_ID".into(), "second".into())]),
            &AtomicBool::new(false),
            Duration::from_secs(30),
            &mut |_| Ok(()),
            &mut || {},
        )?;
        println!("resumed={}", second.binding.is_some());
    }
    Ok(())
}

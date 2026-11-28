//! Generic ACP agents own their tools. This is launch hygiene, not a sandbox.
use std::process::Command;

use nyaterm_core::ai::external_agents::ExternalAgentProfile;

use crate::acp::AgentError;

fn reserved(name: &str) -> bool {
    ["NYATERM_", "MCP_", "NODE_", "BUN_", "LD_", "DYLD_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || matches!(name, "BASH_ENV" | "ENV")
}

pub(super) fn configure(
    command: &mut Command,
    profile: &ExternalAgentProfile,
) -> Result<(), AgentError> {
    if profile.config_directory.is_some() {
        return Err(AgentError::Configuration(
            "Use this Agent's arguments or environment to select its configuration directory",
        ));
    }
    for (name, _) in std::env::vars_os() {
        if reserved(&name.to_string_lossy().to_ascii_uppercase()) {
            command.env_remove(name);
        }
    }
    for variable in &profile.environment {
        let name = variable.name.to_ascii_uppercase();
        if reserved(&name) || matches!(name.as_str(), "PATH" | "PATHEXT" | "COMSPEC") {
            return Err(AgentError::Configuration(
                "Reserved Agent environment variable",
            ));
        }
        command.env(&variable.name, variable.value.expose_secret());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::acp::policy;
    use nyaterm_core::ai::external_agents::{AgentEnvironmentVariable, ExternalAgentProfile};
    use std::process::Command;

    #[test]
    fn generic_configuration_preserves_agent_options_but_rejects_credential_and_loader_overrides() {
        let mut profile = ExternalAgentProfile::new("test".into(), "gemini");
        profile.environment.push(AgentEnvironmentVariable {
            id: "env".into(),
            name: "GEMINI_API_KEY".into(),
            value: "fixture-only".into(),
        });
        assert!(policy::configure(&mut Command::new("fixture"), &profile).is_ok());
        for name in [
            "NYATERM_MCP_TOKEN",
            "MCP_TOKEN",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "PATH",
            "COMSPEC",
        ] {
            profile.environment[0].name = name.into();
            assert!(policy::configure(&mut Command::new("fixture"), &profile).is_err());
        }
        profile.environment.clear();
        profile.strategy.0 = "codex".into();
        assert!(policy::configure(&mut Command::new("fixture"), &profile).is_err());
        profile.strategy.0 = "unknown".into();
        assert!(policy::admit_execution(&profile).is_err());
    }
}

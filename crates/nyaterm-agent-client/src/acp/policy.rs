//! Versioned restrictions belong to the adapter boundary, never to prompt text.
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use nyaterm_core::ai::external_agents::ExternalAgentProfile;
use serde_json::{Value, json};

use crate::acp::AgentError;

pub const CODEX_ADAPTER_VERSION: &str = "2.10.0";
pub const CODEX_ENGINE_VERSION: &str = "0.171.0";
pub const CLAUDE_ADAPTER_VERSION: &str = "0.110.0";

pub fn expected_version(profile: &ExternalAgentProfile) -> Result<&'static str, AgentError> {
    match profile.strategy.0.as_str() {
        "codex" => Ok(CODEX_ADAPTER_VERSION),
        "claude" => Ok(CLAUDE_ADAPTER_VERSION),
        _ => Err(AgentError::UnsupportedStrategy),
    }
}

/// Admission is separate from discovery: an installed adapter is not a sandbox.
pub fn admit_execution(profile: &ExternalAgentProfile) -> Result<(), AgentError> {
    if !platform_supported(profile) {
        return Err(AgentError::UnsupportedPlatform);
    }
    match profile.strategy.0.as_str() {
        "claude" | "codex" => Ok(()),
        _ => Err(AgentError::UnsupportedStrategy),
    }
}

pub fn platform_supported(profile: &ExternalAgentProfile) -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && profile.strategy.0 == "codex")
}

pub fn matches_version(actual: &str, expected: &str) -> bool {
    actual
        .split_whitespace()
        .any(|word| word.trim_start_matches('v') == expected)
}

pub(crate) fn clear_unsafe_environment(command: &mut Command) {
    // A profile cannot inherit an alternative executable, Node preload, hooks, or tool policy.
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if ["CODEX_", "CLAUDE_", "NODE_", "BUN_", "LD_", "DYLD_", "MCP_"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            || matches!(
                upper.as_str(),
                "BASH_ENV"
                    | "ENV"
                    | "INITIAL_AGENT_MODE"
                    | "APP_SERVER_LOGS"
                    | "DEFAULT_AUTH_REQUEST"
                    | "MODEL_PROVIDER"
            )
        {
            command.env_remove(name);
        }
    }
}

pub fn configure(command: &mut Command, profile: &ExternalAgentProfile) -> Result<(), AgentError> {
    profile.validate().map_err(AgentError::Configuration)?;
    expected_version(profile)?;
    for directory in [&profile.working_directory, &profile.config_directory]
        .into_iter()
        .flatten()
    {
        if directory.contains('\0') || !std::path::Path::new(directory).is_absolute() {
            return Err(AgentError::Configuration(
                "Agent directories must be absolute paths",
            ));
        }
    }

    if !profile.args.is_empty() {
        return Err(AgentError::Configuration(
            "Extra adapter arguments are not admitted by the strict policy",
        ));
    }
    clear_unsafe_environment(command);
    for variable in &profile.environment {
        let name = variable.name.to_ascii_uppercase();
        // Policy, executable injection, and inherited MCP credentials are host-owned.
        if name.starts_with("NYATERM_")
            || name.starts_with("CODEX_")
            || name.starts_with("CLAUDE_")
            || name.starts_with("NODE_")
            || name.starts_with("BUN_")
            || name.starts_with("MCP_")
            || name.starts_with("LD_")
            || name.starts_with("DYLD_")
            || matches!(
                name.as_str(),
                "PATH"
                    | "PATHEXT"
                    | "COMSPEC"
                    | "BASH_ENV"
                    | "ENV"
                    | "INITIAL_AGENT_MODE"
                    | "APP_SERVER_LOGS"
                    | "DEFAULT_AUTH_REQUEST"
                    | "MODEL_PROVIDER"
            )
        {
            return Err(AgentError::Configuration(
                "Reserved Agent environment variable",
            ));
        }
        command.env(&variable.name, variable.value.expose_secret());
    }
    for name in [
        "CODEX_CONFIG",
        "CODEX_PATH",
        "INITIAL_AGENT_MODE",
        "APP_SERVER_LOGS",
        "CLAUDE_CODE_EXECUTABLE",
        "CLAUDE_AGENT_ACP_PROTOCOL",
        "NODE_OPTIONS",
        "NODE_PATH",
    ] {
        command.env_remove(name);
    }
    match profile.strategy.0.as_str() {
        "codex" => {
            command.env("INITIAL_AGENT_MODE", "read-only");
            command.env("CODEX_CONFIG", json!({
                "sandbox_mode": "read-only", "approval_policy": "on-request",
                "approvals_reviewer": "user", "web_search": "disabled",
                "tools": {"experimental_request_user_input": {"enabled": false}, "update_plan": {"enabled": false}},
                "features": {
                    "shell_tool": false, "unified_exec": false, "apply_patch_freeform": false,
                    "js_repl": false, "code_mode": false, "code_mode_host": false,
                    "multi_agent": false, "multi_agent_v2": false, "collab": false,
                    "apps": false, "connectors": false, "plugins": false,
                    "hooks": false, "codex_hooks": false, "plugin_hooks": false,
                    "browser_use": false, "computer_use": false, "image_generation": false,
                    "view_image": false, "memory_tool": false, "memories": false,
                    "shell_snapshot": false, "shell_snapshot_v2": false,
                    "goals": false, "default_mode_request_user_input": false, "collaboration_modes": false,
                    "request_permissions": false, "request_permissions_tool": false
                }
            }).to_string());
            if let Some(directory) = &profile.config_directory {
                command.env("CODEX_HOME", directory);
            }
        }
        "claude" => {
            command.env("CLAUDE_AGENT_ACP_PROTOCOL", "v1");
            command.env("ENABLE_TOOL_SEARCH", "false");
            if let Some(directory) = &profile.config_directory {
                command.env("CLAUDE_CONFIG_DIR", directory);
            }
        }
        _ => return Err(AgentError::UnsupportedStrategy),
    }
    Ok(())
}

pub fn session_metadata(profile: &ExternalAgentProfile) -> Value {
    match profile.strategy.0.as_str() {
        "claude" => json!({"claudeCode": {"options": {
            "tools": [], "strictMcpConfig": true, "settingSources": [],
            "allowDangerouslySkipPermissions": false, "permissionMode": "default",
            "mcpServersDelivery": "initialize", "settingsDelivery": "initialize",
            "persistSession": profile.persistent,
            "settings": {"disableAllHooks": true, "permissions": {"disableBypassPermissionsMode": "disable"}}
        }}}),
        _ => json!({}),
    }
}

pub fn mcp_server(helper: &Path, environment: &HashMap<String, String>) -> Value {
    json!({"name":"nyaterm", "command":helper, "args":[],
        "env":environment.iter().map(|(name,value)| json!({"name":name,"value":value})).collect::<Vec<_>>()})
}

/// Never authorize by title, kind, or free-form descriptions.
pub fn is_nyaterm_permission(tool_call: &Value) -> bool {
    tool_call
        .pointer("/_meta/claudeCode/toolName")
        .or_else(|| tool_call.pointer("/_meta/toolName"))
        .and_then(Value::as_str)
        .is_some_and(|name| {
            name.starts_with("mcp__nyaterm__") && name.len() > "mcp__nyaterm__".len()
        })
}

/// Codex 2.10.0 permissions are partial tool updates. Bind their IDs to a prior
/// structured MCP notification, never to the human-readable title or `kind`.
#[derive(Default)]
pub(super) struct CodexPermissions {
    pending: std::collections::HashSet<String>,
}

impl CodexPermissions {
    pub(super) fn observe(&mut self, update: &Value) {
        let Some(id) = update["toolCallId"].as_str() else {
            return;
        };
        if update["sessionUpdate"] == "tool_call" {
            self.pending.remove(id);
        }
        if matches!(update["status"].as_str(), Some("completed" | "failed")) {
            self.pending.remove(id);
        } else if update["sessionUpdate"] == "tool_call"
            && update["_meta"]["is_mcp_tool_call"] == true
            && update["rawInput"]["server"] == super::codex_policy::SERVER_NAME
            && update["rawInput"]["tool"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
            && self.pending.len() < 256
        {
            self.pending.insert(id.into());
        }
    }

    pub(super) fn authorize(&mut self, request: &Value) -> bool {
        request["_meta"]["is_mcp_tool_approval"] == true
            && request["toolCall"]["toolCallId"]
                .as_str()
                .is_some_and(|id| self.pending.remove(id))
    }
}

#[cfg(test)]
mod tests {
    use crate::acp::policy::{is_nyaterm_permission, matches_version, session_metadata};
    use nyaterm_core::ai::external_agents::ExternalAgentProfile;
    use serde_json::json;

    #[test]
    fn a_tool_title_cannot_authorize_native_execution() {
        assert!(!is_nyaterm_permission(
            &json!({"title":"mcp__nyaterm__execute", "kind":"execute"})
        ));
        assert!(!is_nyaterm_permission(
            &json!({"_meta":{"toolName":"mcp__other__execute"}})
        ));
        assert!(is_nyaterm_permission(
            &json!({"_meta":{"toolName":"mcp__nyaterm__execute"}})
        ));
        assert!(!matches_version("12.10.0", "2.10.0"));
    }

    #[test]
    fn codex_permissions_require_a_live_structured_mcp_call_and_are_single_use() {
        use super::CodexPermissions;
        let mut permissions = CodexPermissions::default();
        let request =
            json!({"_meta":{"is_mcp_tool_approval":true},"toolCall":{"toolCallId":"call"}});
        assert!(!permissions.authorize(&request));
        let mut notification = json!({"sessionUpdate":"tool_call","toolCallId":"call","status":"pending",
            "_meta":{"is_mcp_tool_call":true},"rawInput":{"server":"foreign","tool":"terminal_execute"}});
        permissions.observe(&notification);
        assert!(!permissions.authorize(&request));
        notification["rawInput"]["server"] = json!(super::super::codex_policy::SERVER_NAME);
        permissions.observe(&notification);
        assert!(
            !permissions
                .authorize(&json!({"toolCall":{"toolCallId":"call","title":"NyaTerm MCP"}}))
        );
        assert!(permissions.authorize(&request));
        assert!(!permissions.authorize(&request));
        permissions.observe(&notification);
        permissions.observe(&json!({"toolCallId":"call","status":"completed"}));
        assert!(!permissions.authorize(&request));
    }

    #[test]
    fn claude_uses_stdin_and_excludes_native_tools_and_inherited_settings() {
        let value = session_metadata(&ExternalAgentProfile::new("a".into(), "claude"));
        let options = &value["claudeCode"]["options"];
        assert_eq!(options["tools"], json!([]));
        assert_eq!(options["settingSources"], json!([]));
        assert_eq!(options["mcpServersDelivery"], "initialize");
        assert_eq!(options["allowDangerouslySkipPermissions"], false);
    }
}

//! Persisted external-agent identities and policy-neutral ACP contracts.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SecretString;
use crate::ai::{AiAgentKind, AiSettings, CodexThreadMode};

pub const CODEX_PROFILE_ID: &str = "builtin-codex";
pub const CLAUDE_PROFILE_ID: &str = "builtin-claude";

/// Keep unfamiliar strategy identifiers intact; only the client can admit a strategy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct AgentStrategy(pub String);

impl AgentStrategy {
    pub fn supported(&self) -> bool {
        matches!(self.0.as_str(), "codex" | "claude")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEnvironmentVariable {
    pub id: String,
    pub name: String,
    pub value: SecretString,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalAgentProfile {
    pub id: String,
    pub name: String,
    pub strategy: AgentStrategy,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub enabled: bool,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: Vec<AgentEnvironmentVariable>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub config_directory: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default = "persistent_default")]
    pub persistent: bool,
    /// A legacy engine path is not an ACP executable and must never be launched as one.
    #[serde(default)]
    pub legacy_engine_path: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl std::fmt::Debug for ExternalAgentProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalAgentProfile")
            .field("id", &self.id)
            .field("strategy", &self.strategy)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

fn persistent_default() -> bool {
    true
}

impl ExternalAgentProfile {
    pub fn new(id: String, strategy: &str) -> Self {
        let (name, executable) = match strategy {
            "codex" => ("Codex", "codex-acp"),
            "claude" => ("Claude Code", "claude-agent-acp"),
            _ => ("ACP Agent", ""),
        };
        Self {
            id,
            name: name.into(),
            strategy: AgentStrategy(strategy.into()),
            revision: 0,
            enabled: false,
            executable: executable.into(),
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: None,
            config_directory: None,
            default_model: None,
            persistent: true,
            legacy_engine_path: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn agent_kind(&self) -> AiAgentKind {
        AiAgentKind::Acp(self.id.clone())
    }

    /// Resolve UI masks only within the same profile and environment-row identity.
    pub fn merge_masked_environment(&mut self, previous: Option<&Self>) {
        let previous = previous.filter(|profile| profile.id == self.id);
        for variable in &mut self.environment {
            if variable.value == crate::ai::MASKED_SECRET_VALUE {
                variable.value = previous
                    .and_then(|profile| {
                        profile
                            .environment
                            .iter()
                            .find(|old| old.id == variable.id && old.name == variable.name)
                    })
                    .map(|old| old.value.clone())
                    .unwrap_or_default();
            }
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.name.trim().is_empty()
        {
            return Err("Agent identity and name must not be empty");
        }
        if self.executable.contains(['\0', '\n', '\r'])
            || self.args.iter().any(|arg| arg.contains('\0'))
        {
            return Err("Invalid Agent executable or arguments");
        }
        let mut names = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for variable in &self.environment {
            if variable.id.is_empty() || !ids.insert(&variable.id) {
                return Err("Agent environment row identities must be unique");
            }
            let name = variable.name.to_ascii_uppercase();
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !names.insert(name)
                || variable.value.contains('\0')
            {
                return Err("Invalid or duplicate Agent environment variable");
            }
        }
        Ok(())
    }
}

/// None means a legacy document; Some(empty) means the user removed all profiles.
pub fn migrate_profiles(settings: &mut AiSettings) {
    if settings.external_agents.is_some() {
        return;
    }
    let mut codex = ExternalAgentProfile::new(CODEX_PROFILE_ID.into(), "codex");
    codex.enabled = settings.codex.enabled;
    codex.default_model = settings.codex.default_model.clone();
    codex.config_directory = settings.codex.config_directory.clone();
    codex.legacy_engine_path = settings.codex.executable_path.clone();
    codex.persistent = settings.codex.thread_mode == CodexThreadMode::Persistent;
    let mut claude = ExternalAgentProfile::new(CLAUDE_PROFILE_ID.into(), "claude");
    claude.enabled = settings.claude_code.enabled;
    claude.default_model = settings.claude_code.default_model.clone();
    claude.config_directory = settings.claude_code.config_directory.clone();
    claude.legacy_engine_path = settings.claude_code.executable_path.clone();
    settings.external_agents = Some(vec![codex, claude]);
}

pub fn profile_id(kind: &AiAgentKind) -> Option<&str> {
    match kind {
        AiAgentKind::Nyaterm => None,
        AiAgentKind::Codex => Some(CODEX_PROFILE_ID),
        AiAgentKind::ClaudeCode => Some(CLAUDE_PROFILE_ID),
        AiAgentKind::Acp(id) => Some(id),
    }
}

pub fn same_agent(left: &AiAgentKind, right: &AiAgentKind) -> bool {
    profile_id(left) == profile_id(right)
}

pub fn profiles(settings: &AiSettings) -> &[ExternalAgentProfile] {
    settings.external_agents.as_deref().unwrap_or_default()
}

pub fn resolve_profile<'a>(
    settings: &'a AiSettings,
    kind: &AiAgentKind,
) -> Option<&'a ExternalAgentProfile> {
    let id = profile_id(kind)?;
    profiles(settings).iter().find(|profile| profile.id == id)
}

pub fn display_name(settings: &AiSettings, kind: &AiAgentKind) -> String {
    resolve_profile(settings, kind)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| match kind {
            AiAgentKind::Nyaterm => "NyaTerm".into(),
            AiAgentKind::Codex => "Codex".into(),
            AiAgentKind::ClaudeCode => "Claude Code".into(),
            AiAgentKind::Acp(_) => "Unavailable ACP Agent".into(),
        })
}

pub fn model_label(settings: &AiSettings, kind: &AiAgentKind) -> String {
    resolve_profile(settings, kind)
        .and_then(|p| p.default_model.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| display_name(settings, kind))
}

pub fn is_enabled(settings: &AiSettings, kind: &AiAgentKind) -> bool {
    if *kind == AiAgentKind::Nyaterm {
        return true;
    }
    if settings.external_agents.is_none() {
        return match kind {
            AiAgentKind::Codex => settings.codex.enabled,
            AiAgentKind::ClaudeCode => settings.claude_code.enabled,
            _ => false,
        };
    }
    resolve_profile(settings, kind)
        .is_some_and(|profile| profile.enabled && profile.strategy.supported())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSessionBinding {
    pub profile_id: String,
    /// Strings preserve unfamiliar future protocols without reinterpreting their session IDs.
    pub protocol: String,
    #[serde(default)]
    pub revision: u64,
    pub external_id: String,
    pub working_directory: String,
    pub config_directory: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ExternalSessionBinding {
    pub fn can_resume(&self, profile: &ExternalAgentProfile, cwd: &str) -> bool {
        self.protocol == "acp_v1"
            && self.profile_id == profile.id
            && self.revision == profile.revision
            && self.working_directory == cwd
            && self.config_directory == profile.config_directory
            && !self.external_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use crate::ai::external_agents::{ExternalAgentProfile, migrate_profiles, same_agent};
    use crate::ai::{AiAgentKind, AiSettings};

    #[test]
    fn legacy_migration_retains_engine_path_without_launching_it_as_acp() {
        let mut settings: AiSettings = serde_json::from_value(serde_json::json!({
            "schema_version": 7,
            "codex": {"enabled": true, "executable_path": "old-codex.exe"}
        }))
        .unwrap();
        migrate_profiles(&mut settings);
        let profile = &settings.external_agents.as_ref().unwrap()[0];
        assert!(profile.enabled);
        assert_eq!(profile.executable, "codex-acp");
        assert_eq!(profile.legacy_engine_path.as_deref(), Some("old-codex.exe"));
        settings.external_agents = Some(Vec::new());
        migrate_profiles(&mut settings);
        assert!(settings.external_agents.unwrap().is_empty());
    }

    #[test]
    fn unknown_agent_strategy_and_fields_survive_round_trip() {
        let mut value =
            serde_json::to_value(ExternalAgentProfile::new("a".into(), "future-agent")).unwrap();
        value["futureOptions"] = serde_json::json!({"version": 42});
        let profile: ExternalAgentProfile = serde_json::from_value(value.clone()).unwrap();
        assert!(!profile.strategy.supported());
        assert_eq!(serde_json::to_value(profile).unwrap(), value);
    }

    #[test]
    fn legacy_aliases_match_only_their_original_instance() {
        assert!(same_agent(
            &AiAgentKind::Codex,
            &AiAgentKind::Acp("builtin-codex".into())
        ));
        assert!(!same_agent(
            &AiAgentKind::Codex,
            &AiAgentKind::Acp("other-codex".into())
        ));
    }
    #[test]
    fn profile_revisions_and_directories_prevent_cross_configuration_resume() {
        let mut profile = ExternalAgentProfile::new("a".into(), "claude");
        let binding = super::ExternalSessionBinding {
            profile_id: "a".into(),
            protocol: "acp_v1".into(),
            revision: 0,
            external_id: "external".into(),
            working_directory: "cwd".into(),
            config_directory: None,
            source_session_id: None,
            extra: Default::default(),
        };
        assert!(binding.can_resume(&profile, "cwd"));
        assert!(!binding.can_resume(&profile, "other"));
        profile.revision += 1;
        assert!(!binding.can_resume(&profile, "cwd"));
    }
    #[test]
    fn environment_masked_values_cannot_cross_profile_or_row_identity() {
        let mut profile = ExternalAgentProfile::new("a".into(), "claude");
        profile.environment.push(super::AgentEnvironmentVariable {
            id: "row".into(),
            name: "TOKEN".into(),
            value: "fixture-secret".into(),
        });
        let settings = AiSettings {
            external_agents: Some(vec![profile]),
            ..Default::default()
        };
        let masked = crate::ai::mask_ai_settings(settings.clone());
        assert!(
            !serde_json::to_string(&masked)
                .unwrap()
                .contains("fixture-secret")
        );
        assert_eq!(
            crate::ai::merge_masked_ai_settings(&settings, masked.clone()).external_agents,
            settings.external_agents
        );
        let mut copied = masked;
        copied.external_agents.as_mut().unwrap()[0].id = "copy".into();
        assert!(
            crate::ai::merge_masked_ai_settings(&settings, copied)
                .external_agents
                .unwrap()[0]
                .environment[0]
                .value
                .is_empty()
        );
    }
    #[test]
    fn profile_ids_cannot_escape_the_owned_workspace() {
        for id in ["../escape", "a/b", "a\\b", ".", "", "a.b"] {
            assert!(
                ExternalAgentProfile::new(id.into(), "claude")
                    .validate()
                    .is_err()
            );
        }
    }
}

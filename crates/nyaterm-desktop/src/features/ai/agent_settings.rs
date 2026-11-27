//! Agent drafts are separate from the profiles used by running sessions.
use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::AiSettings;
use nyaterm_core::ai::external_agents::{AgentEnvironmentVariable, ExternalAgentProfile, profiles};

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct AgentEditorView {
    pub advanced: BTreeSet<String>,
    pub delete_pending: Option<String>,
    /// Translation keys only; validation must never echo an environment value.
    pub errors: BTreeMap<String, &'static str>,
    pub dirty: BTreeSet<String>,
}

#[derive(Default)]
pub(in crate::features) struct AgentSettingsEditor {
    profiles: Option<Vec<ExternalAgentProfile>>,
    invalid_args: BTreeSet<String>,
    deleted: BTreeSet<String>,
    view: AgentEditorView,
}

impl AgentSettingsEditor {
    pub fn view(&self) -> &AgentEditorView {
        &self.view
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn present(&mut self, id: &str, action: &str) {
        match action {
            "advanced" => {
                if !self.view.advanced.remove(id) {
                    self.view.advanced.insert(id.into());
                }
            }
            "request-delete" => self.view.delete_pending = Some(id.into()),
            "cancel-delete" => self.view.delete_pending = None,
            _ => {}
        }
    }

    pub fn project(&self, settings: &mut AiSettings) {
        if let Some(profiles) = &self.profiles {
            settings.external_agents = Some(profiles.clone());
            if let Some(id) =
                nyaterm_core::ai::external_agents::profile_id(&settings.default_agent_kind)
                && self.deleted.contains(id)
            {
                settings.default_agent_kind = nyaterm_core::AiAgentKind::Nyaterm;
            }
        }
    }

    pub fn edit(
        &mut self,
        settings: &AiSettings,
        id: &str,
        action: &str,
        value: String,
    ) -> Result<bool, &'static str> {
        let normalized = settings.external_agents.is_none().then(|| {
            let mut normalized = settings.clone();
            nyaterm_core::ai::external_agents::migrate_profiles(&mut normalized);
            normalized
        });
        let original_profiles = profiles(normalized.as_ref().unwrap_or(settings));
        let profiles = self
            .profiles
            .get_or_insert_with(|| original_profiles.to_vec());
        if action == "add" {
            profiles.push(ExternalAgentProfile::new(nyaterm_core::uuid(), &value));
        } else {
            let Some(index) = profiles.iter().position(|profile| profile.id == id) else {
                return Ok(false);
            };
            match action {
                "delete" => {
                    self.deleted.insert(id.into());
                    profiles.remove(index);
                    self.invalid_args.remove(id);
                    self.view.advanced.remove(id);
                    self.view.delete_pending = None;
                }
                "copy" => {
                    let mut profile = profiles[index].clone();
                    profile.id = nyaterm_core::uuid();
                    profile.name = format!("{} ({})", profile.name, rust_i18n::t!("ai.copy"));
                    profile.enabled = false;
                    for variable in &mut profile.environment {
                        variable.value = Default::default();
                    }
                    profiles.push(profile);
                }
                _ => {
                    let mut profile = profiles[index].clone();
                    let optional = || (!value.trim().is_empty()).then(|| value.clone());
                    match action {
                        "name" => profile.name = value,
                        "executable" => profile.executable = value,
                        "strategy" => {
                            profile.strategy.0 = value;
                            profile.enabled = false;
                        }
                        "args" => match serde_json::from_str::<Vec<String>>(&value) {
                            Ok(args) => {
                                profile.args = args;
                                self.invalid_args.remove(id);
                            }
                            Err(_) => {
                                self.invalid_args.insert(id.into());
                            }
                        },
                        "cwd" => profile.working_directory = optional(),
                        "config" => profile.config_directory = optional(),
                        "model" => profile.default_model = optional(),
                        "enabled" => {
                            profile.enabled = !profile.enabled && profile.strategy.supported()
                        }
                        "persistent" => profile.persistent = !profile.persistent,
                        "add-env" => {
                            let name = (1..)
                                .map(|i| format!("VARIABLE_{i}"))
                                .find(|name| {
                                    !profile
                                        .environment
                                        .iter()
                                        .any(|env| env.name.eq_ignore_ascii_case(name))
                                })
                                .unwrap();
                            profile.environment.push(AgentEnvironmentVariable {
                                id: nyaterm_core::uuid(),
                                name,
                                value: Default::default(),
                            });
                            self.view.advanced.insert(id.into());
                        }
                        field => {
                            let Some((row, field)) = field
                                .strip_prefix("env.")
                                .and_then(|rest| rest.rsplit_once('.'))
                            else {
                                return Ok(false);
                            };
                            if field == "delete" {
                                profile.environment.retain(|env| env.id != row);
                            } else if let Some(env) =
                                profile.environment.iter_mut().find(|env| env.id == row)
                            {
                                match field {
                                    "name" => env.name = value,
                                    "value" => env.value = value.into(),
                                    _ => return Ok(false),
                                }
                            }
                        }
                    }
                    if let Some(original) = original_profiles.iter().find(|old| old.id == id) {
                        // A draft receives one revision, regardless of how many keystrokes it took.
                        let mut comparable = profile.clone();
                        comparable.name.clone_from(&original.name);
                        comparable.enabled = original.enabled;
                        comparable.default_model.clone_from(&original.default_model);
                        comparable.revision = original.revision;
                        profile.revision = if comparable == *original {
                            original.revision
                        } else {
                            original
                                .revision
                                .checked_add(1)
                                .ok_or("Agent revision exhausted")?
                        };
                    }
                    profiles[index] = profile;
                }
            }
        }
        self.view.errors.clear();
        self.view.dirty.clear();
        for profile in profiles.iter() {
            if let Some(key) =
                profile_validation_error(profile, self.invalid_args.contains(&profile.id))
            {
                self.view.errors.insert(profile.id.clone(), key);
            }
            if self.invalid_args.contains(&profile.id) || !original_profiles.contains(profile) {
                self.view.dirty.insert(profile.id.clone());
            }
        }
        Ok(true)
    }
}

fn profile_validation_error(
    profile: &ExternalAgentProfile,
    invalid_args: bool,
) -> Option<&'static str> {
    if profile.name.trim().is_empty() {
        return Some("ai.acp.nameRequired");
    }
    if invalid_args {
        return Some("ai.acp.argsInvalid");
    }
    if profile.validate().is_err() {
        return Some("ai.acp.invalidProfile");
    }
    for directory in [&profile.working_directory, &profile.config_directory]
        .into_iter()
        .flatten()
    {
        if directory.contains('\0') || !std::path::Path::new(directory).is_absolute() {
            return Some("ai.acp.absoluteDirectory");
        }
    }
    None
}

pub(in crate::features) fn execution_unavailable_key(
    profile: &ExternalAgentProfile,
) -> Option<&'static str> {
    use nyaterm_agent_client::acp::{AgentError, policy};
    match policy::admit_execution(profile) {
        Err(AgentError::UnsupportedPlatform) => Some("ai.acp.platformUnavailable"),
        Err(AgentError::UnsupportedStrategy) => Some("ai.acp.strategyUnavailable"),
        Err(_) => Some("ai.acp.detectionOnly"),
        Ok(()) if !profile.args.is_empty() => Some("ai.acp.argsUnsupported"),
        Ok(()) => None,
    }
}

pub(in crate::features) fn default_selectable(profile: &ExternalAgentProfile) -> bool {
    profile.enabled
        && execution_unavailable_key(profile).is_none()
        && !profile.executable.trim().is_empty()
        && profile_validation_error(profile, false).is_none()
}

#[cfg(test)]
mod tests {
    use crate::features::ai::agent_settings::{AgentSettingsEditor, default_selectable};
    use nyaterm_core::AiSettings;
    use nyaterm_core::ai::external_agents::{AgentEnvironmentVariable, ExternalAgentProfile};

    fn settings() -> AiSettings {
        let mut profile = ExternalAgentProfile::new("agent".into(), "claude");
        profile
            .extra
            .insert("futureField".into(), serde_json::json!({"version": 2}));
        profile.environment.push(AgentEnvironmentVariable {
            id: "row".into(),
            name: "API_KEY".into(),
            value: "test-secret".into(),
        });
        AiSettings {
            external_agents: Some(vec![profile]),
            ..Default::default()
        }
    }

    #[test]
    fn typing_and_cancel_leave_applied_profiles_and_future_fields_intact() {
        let original = settings();
        let mut editor = AgentSettingsEditor::default();
        editor
            .edit(&original, "agent", "executable", "edited".into())
            .unwrap();
        editor
            .edit(&original, "agent", "executable", "edited-adapter".into())
            .unwrap();
        let mut pending = original.clone();
        editor.project(&mut pending);
        let profile = &pending.external_agents.as_ref().unwrap()[0];
        assert_eq!(profile.revision, 1);
        assert_eq!(profile.executable, "edited-adapter");
        assert_eq!(
            profile.extra,
            original.external_agents.as_ref().unwrap()[0].extra
        );
        assert_eq!(
            original.external_agents.as_ref().unwrap()[0].executable,
            "claude-agent-acp"
        );
        let serialized = serde_json::to_value(&pending).unwrap();
        let restored: AiSettings = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.external_agents, pending.external_agents);
        editor.clear();
        let mut cancelled = original.clone();
        editor.project(&mut cancelled);
        assert_eq!(cancelled, original);
    }

    #[test]
    fn incomplete_json_and_empty_names_block_apply_until_repaired() {
        let original = settings();
        let mut editor = AgentSettingsEditor::default();
        editor.edit(&original, "agent", "args", "[".into()).unwrap();
        assert_eq!(editor.view().errors["agent"], "ai.acp.argsInvalid");
        assert!(editor.view().dirty.contains("agent"));
        editor
            .edit(&original, "agent", "args", "[]".into())
            .unwrap();
        assert!(editor.view().errors.is_empty());
        assert!(editor.view().dirty.is_empty());
        editor
            .edit(&original, "agent", "name", String::new())
            .unwrap();
        assert_eq!(editor.view().errors["agent"], "ai.acp.nameRequired");
        editor
            .edit(&original, "agent", "name", "Renamed".into())
            .unwrap();
        assert!(editor.view().errors.is_empty());
    }

    #[test]
    fn duplicate_clears_secrets_and_delete_can_be_cancelled_with_the_settings_draft() {
        let mut original = settings();
        original.default_agent_kind = nyaterm_core::AiAgentKind::Acp("agent".into());
        let mut editor = AgentSettingsEditor::default();
        editor
            .edit(&original, "agent", "copy", String::new())
            .unwrap();
        editor
            .edit(&original, "agent", "delete", String::new())
            .unwrap();
        let mut pending = original.clone();
        editor.project(&mut pending);
        assert_eq!(
            pending.default_agent_kind,
            nyaterm_core::AiAgentKind::Nyaterm
        );
        let copy = &pending.external_agents.as_ref().unwrap()[0];
        assert_ne!(copy.id, "agent");
        assert!(!copy.enabled);
        assert!(copy.environment[0].value.is_empty());
        assert_eq!(
            copy.extra,
            original.external_agents.as_ref().unwrap()[0].extra
        );
        editor.clear();
        let mut cancelled = original.clone();
        editor.project(&mut cancelled);
        assert_eq!(cancelled, original);
    }

    #[test]
    fn disabled_unsupported_and_argument_bearing_agents_cannot_be_defaults() {
        let mut profile = ExternalAgentProfile::new("agent".into(), "claude");
        assert!(!default_selectable(&profile));
        profile.enabled = true;
        assert_eq!(default_selectable(&profile), cfg!(windows));
        profile.args.push("--extra".into());
        assert!(!default_selectable(&profile));
        profile.args.clear();
        profile.strategy.0 = "codex".into();
        assert_eq!(
            default_selectable(&profile),
            cfg!(any(windows, target_os = "linux"))
        );
        profile.strategy.0 = "future".into();
        assert!(!default_selectable(&profile));
    }
}

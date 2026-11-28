//! One presentation policy for the profile summary and default-agent selector.
use crate::features::ai::agent_management::{AcpProfileProblem, AgentManagementView};
use crate::features::ai::agent_settings::{AgentEditorView, execution_unavailable_key};
use nyaterm_core::ai::external_agents::ExternalAgentProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum ProfileAction {
    Check,
    Recheck,
    Install,
    Login,
    Unavailable,
}

pub(in crate::features) struct ProfilePresentation {
    pub message_key: &'static str,
    pub warning: bool,
    pub busy: bool,
    pub action: ProfileAction,
}

pub(in crate::features) fn project(
    profile: &ExternalAgentProfile,
    editor: &AgentEditorView,
    management: &AgentManagementView,
) -> ProfilePresentation {
    let busy = management.pending && management.active_profile.as_deref() == Some(&profile.id);
    let unavailable = editor
        .errors
        .get(&profile.id)
        .copied()
        .or_else(|| execution_unavailable_key(profile));
    let (message_key, warning, action) = if let Some(key) = unavailable {
        (key, true, ProfileAction::Unavailable)
    } else if busy {
        ("ai.acp.checking", false, ProfileAction::Check)
    } else if let Some(status) = management.profiles.get(&profile.id) {
        if let Some(problem) = status.problem {
            let action = match problem {
                AcpProfileProblem::Missing | AcpProfileProblem::Version => ProfileAction::Install,
                AcpProfileProblem::Platform | AcpProfileProblem::Strategy => {
                    ProfileAction::Unavailable
                }
                _ => ProfileAction::Check,
            };
            (
                problem.message_key(),
                problem != AcpProfileProblem::Cancelled,
                action,
            )
        } else {
            match status.authenticated {
                Some(true) => ("ai.acp.connected", false, ProfileAction::Recheck),
                Some(false) => (
                    "ai.acp.loginRequired",
                    true,
                    if status.auth_methods.is_empty() {
                        ProfileAction::Recheck
                    } else {
                        ProfileAction::Login
                    },
                ),
                None => ("ai.acp.authUnknown", false, ProfileAction::Recheck),
            }
        }
    } else {
        ("ai.acp.notChecked", false, ProfileAction::Check)
    };
    ProfilePresentation {
        message_key,
        warning,
        busy,
        action,
    }
}

#[cfg(test)]
mod tests {
    use crate::features::ai::agent_management::{
        AcpProfileProblem, AcpProfileStatus, AgentManagementView,
    };
    use crate::features::ai::agent_settings::AgentEditorView;
    use crate::features::ai::agent_settings::status::{ProfileAction, project};
    use nyaterm_agent_client::acp::AgentError;
    use nyaterm_core::ai::external_agents::ExternalAgentProfile;

    #[test]
    fn errors_are_classified_without_exposing_provider_details() {
        for (error, expected) in [
            (AgentError::Missing, AcpProfileProblem::Missing),
            (AgentError::UnsupportedVersion, AcpProfileProblem::Version),
            (AgentError::UnsupportedPlatform, AcpProfileProblem::Platform),
            (
                AgentError::Configuration("private detail"),
                AcpProfileProblem::Configuration,
            ),
            (
                AgentError::Protocol("private detail"),
                AcpProfileProblem::Protocol,
            ),
            (AgentError::Transport, AcpProfileProblem::Startup),
            (AgentError::Timeout, AcpProfileProblem::Timeout),
            (AgentError::Cancelled, AcpProfileProblem::Cancelled),
        ] {
            assert_eq!(AcpProfileProblem::from(error), expected);
            assert!(expected.message_key().starts_with("ai.acp."));
        }
    }

    #[test]
    fn unsupported_platforms_and_invalid_drafts_take_precedence_over_old_success() {
        let profile = ExternalAgentProfile::new("test".into(), "unknown");
        let mut management = AgentManagementView::default();
        management.profiles.insert(
            profile.id.clone(),
            AcpProfileStatus {
                authenticated: Some(true),
                ..Default::default()
            },
        );
        let view = project(&profile, &AgentEditorView::default(), &management);
        assert_eq!(view.action, ProfileAction::Unavailable);
        assert!(view.warning);
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let codex = ExternalAgentProfile::new("test".into(), "codex");
            assert_eq!(
                project(&codex, &AgentEditorView::default(), &management).message_key,
                "ai.acp.platformUnavailable"
            );
        }
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn inspection_results_choose_recovery_actions_without_confusing_enabled_with_ready() {
        let mut profile = ExternalAgentProfile::new("test".into(), "codex");
        profile.enabled = true;
        let mut management = AgentManagementView::default();
        let mut editor = AgentEditorView::default();
        assert_eq!(
            project(&profile, &editor, &management).action,
            ProfileAction::Check
        );
        for (problem, action) in [
            (AcpProfileProblem::Missing, ProfileAction::Install),
            (AcpProfileProblem::Version, ProfileAction::Install),
            (AcpProfileProblem::Timeout, ProfileAction::Check),
            (AcpProfileProblem::Startup, ProfileAction::Check),
            (AcpProfileProblem::Cancelled, ProfileAction::Check),
        ] {
            management.profiles.insert(
                profile.id.clone(),
                AcpProfileStatus {
                    problem: Some(problem),
                    ..Default::default()
                },
            );
            let view = project(&profile, &editor, &management);
            assert_eq!(view.action, action);
            assert_eq!(view.warning, problem != AcpProfileProblem::Cancelled);
        }
        for (authenticated, key, action) in [
            (Some(true), "ai.acp.connected", ProfileAction::Recheck),
            (Some(false), "ai.acp.loginRequired", ProfileAction::Login),
            (None, "ai.acp.authUnknown", ProfileAction::Recheck),
        ] {
            management.profiles.insert(
                profile.id.clone(),
                AcpProfileStatus {
                    authenticated,
                    auth_methods: vec![("login".into(), "Sign in".into())],
                    ..Default::default()
                },
            );
            let view = project(&profile, &editor, &management);
            assert_eq!(view.message_key, key);
            assert_eq!(view.action, action);
        }
        management.pending = true;
        management.active_profile = Some(profile.id.clone());
        assert!(project(&profile, &editor, &management).busy);
        assert_eq!(
            project(&profile, &editor, &management).message_key,
            "ai.acp.checking"
        );
        editor
            .errors
            .insert(profile.id.clone(), "ai.acp.nameRequired");
        assert_eq!(
            project(&profile, &editor, &management).action,
            ProfileAction::Unavailable
        );
    }
}

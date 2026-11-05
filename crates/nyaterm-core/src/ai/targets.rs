//! Session scope, terminal target and attachment policy.

use std::collections::HashSet;

use thiserror::Error;

use super::{
    AiChatRequest, AiCommandCard, AiSessionScope, AiSessionScopeType, AiSettings, AiTerminalTarget,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AiRequestContractError {
    #[error("terminal target id is empty")]
    EmptyTargetId,
    #[error("terminal target '{0}' is duplicated")]
    DuplicateTarget(String),
    #[error("default terminal target '{0}' is not available")]
    UnknownDefaultTarget(String),
    #[error("terminal context target '{0}' is not available")]
    UnknownContextTarget(String),
    #[error("{scope:?} session scope requires a target id")]
    MissingScopeTarget { scope: AiSessionScopeType },
    #[error("session scope connection id is empty")]
    EmptyScopeConnection,
    #[error("session scope connection '{0}' is duplicated")]
    DuplicateScopeConnection(String),
    #[error("attachment id is empty")]
    EmptyAttachmentId,
    #[error("attachment '{0}' is duplicated")]
    DuplicateAttachment(String),
    #[error("attachment '{name}' is {size} bytes, exceeding the {limit} byte limit")]
    AttachmentTooLarge { name: String, size: u64, limit: u64 },
    #[error("Agent mode requires a terminal target")]
    MissingAgentTarget,
    #[error("Agent command is missing targetTerminalSessionId")]
    MissingAgentTargetSelection,
    #[error("Agent command target '{0}' is not an available terminal")]
    UnknownAgentTarget(String),
}

pub fn validate_ai_request_contract(
    request: &AiChatRequest,
    settings: &AiSettings,
) -> Result<(), AiRequestContractError> {
    validate_scope(&request.owner_scope)?;

    let mut target_ids = HashSet::new();
    for target in &request.targets {
        let id = target.terminal_session_id.trim();
        if id.is_empty() {
            return Err(AiRequestContractError::EmptyTargetId);
        }
        if !target_ids.insert(id) {
            return Err(AiRequestContractError::DuplicateTarget(id.to_string()));
        }
    }
    if let Some(default_id) = request
        .default_target_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        && !target_ids.contains(default_id)
    {
        return Err(AiRequestContractError::UnknownDefaultTarget(
            default_id.to_string(),
        ));
    }
    for context in &request.target_contexts {
        if let Some(target) = context.target.as_ref()
            && !target_ids.contains(target.terminal_session_id.trim())
        {
            return Err(AiRequestContractError::UnknownContextTarget(
                target.terminal_session_id.clone(),
            ));
        }
    }

    let mut attachment_ids = HashSet::new();
    for attachment in &request.attachments {
        let id = attachment.id.trim();
        if id.is_empty() {
            return Err(AiRequestContractError::EmptyAttachmentId);
        }
        if !attachment_ids.insert(id) {
            return Err(AiRequestContractError::DuplicateAttachment(id.to_string()));
        }
        if let Some(size) = attachment.size_bytes
            && size > settings.max_ai_file_size_bytes
        {
            return Err(AiRequestContractError::AttachmentTooLarge {
                name: attachment.name.clone(),
                size,
                limit: settings.max_ai_file_size_bytes,
            });
        }
    }
    Ok(())
}

pub fn resolve_ai_terminal_target(
    request: &AiChatRequest,
    target_terminal_session_id: Option<&str>,
) -> Result<AiTerminalTarget, AiRequestContractError> {
    match request.targets.as_slice() {
        [] => request
            .terminal_session_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .map(|terminal_session_id| AiTerminalTarget {
                terminal_session_id: terminal_session_id.clone(),
                connection_id: request.connection_id.clone(),
                label: terminal_session_id.clone(),
                host: request.context.host.clone(),
                username: request.context.username.clone(),
                session_type: "unknown".to_string(),
            })
            .ok_or(AiRequestContractError::MissingAgentTarget),
        [target] => Ok(target.clone()),
        targets => {
            let selected = target_terminal_session_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or(AiRequestContractError::MissingAgentTargetSelection)?;
            targets
                .iter()
                .find(|target| target.terminal_session_id == selected)
                .cloned()
                .ok_or_else(|| AiRequestContractError::UnknownAgentTarget(selected.to_string()))
        }
    }
}

pub fn bind_command_card_targets(cards: &mut [AiCommandCard], request: &AiChatRequest) {
    match request.targets.as_slice() {
        [] => {}
        [target] => {
            for card in cards {
                card.target = Some(target.clone());
                card.target_terminal_session_id = Some(target.terminal_session_id.clone());
            }
        }
        targets => {
            for card in cards {
                card.target = resolve_card_target(card, targets);
                card.target_terminal_session_id = card
                    .target
                    .as_ref()
                    .map(|target| target.terminal_session_id.clone());
            }
        }
    }
}

pub(super) fn user_input_with_target_contexts(request: &AiChatRequest) -> String {
    let mut result = format!(
        "<nyaterm-user-task untrusted=\"true\" bytes=\"{}\">\n{}\n</nyaterm-user-task>",
        request.user_input.len(),
        request.user_input
    );

    if !request.targets.is_empty() {
        result.push_str("\n\n<nyaterm-terminal-targets untrusted=\"true\">\n");
        for target in &request.targets {
            result.push_str(&format!(
                "- id={} label={:?} type={:?} host={:?} user={:?}\n",
                target.terminal_session_id,
                target.label,
                target.session_type,
                target.host.as_deref().unwrap_or("-"),
                target.username.as_deref().unwrap_or("-")
            ));
        }
        if request.targets.len() > 1 {
            result.push_str(&super::prompts::catalog(&request.options.language).targets);
        }
        result.push_str("</nyaterm-terminal-targets>\n");
    }

    if !request.target_contexts.is_empty() {
        result.push_str("\n<nyaterm-target-contexts untrusted=\"true\">\n");
        for item in &request.target_contexts {
            let target = item
                .target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str())
                .unwrap_or("unknown");
            let context = &item.context;
            result.push_str(&format!(
                "[target={target}]\ncwd={:?}\ninput={:?}\nselected_text:\n{}\nrecent_output:\n{}\n",
                context.cwd.as_deref().unwrap_or("-"),
                context.input_buffer,
                context.selected_text,
                context.recent_output
            ));
        }
        result.push_str("</nyaterm-target-contexts>\n");
    }
    if !request.attachments.is_empty() {
        result.push_str("\n<nyaterm-attachments untrusted=\"true\">\n");
        for attachment in &request.attachments {
            result.push_str(&format!(
                "- id={:?} name={:?} mime={:?} size={:?}\n",
                attachment.id, attachment.name, attachment.mime_type, attachment.size_bytes
            ));
        }
        result.push_str("</nyaterm-attachments>\n");
    }
    result
}

fn validate_scope(scope: &AiSessionScope) -> Result<(), AiRequestContractError> {
    if matches!(
        scope.r#type,
        AiSessionScopeType::Terminal | AiSessionScopeType::Workspace
    ) && scope
        .target_id
        .as_deref()
        .is_none_or(|target| target.trim().is_empty())
    {
        return Err(AiRequestContractError::MissingScopeTarget {
            scope: scope.r#type.clone(),
        });
    }
    let mut ids = HashSet::new();
    for connection_id in &scope.connection_ids {
        let id = connection_id.trim();
        if id.is_empty() {
            return Err(AiRequestContractError::EmptyScopeConnection);
        }
        if !ids.insert(id) {
            return Err(AiRequestContractError::DuplicateScopeConnection(
                id.to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_card_target(
    card: &AiCommandCard,
    targets: &[AiTerminalTarget],
) -> Option<AiTerminalTarget> {
    let selected = card
        .target_terminal_session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            card.target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str())
        })?;
    targets
        .iter()
        .find(|target| target.terminal_session_id == selected)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::{
        AiChatRequest, AiCommandCard, AiRequestContractError, AiSessionScope, AiSessionScopeType,
        AiSettings, AiTerminalTarget, bind_command_card_targets, resolve_ai_terminal_target,
        user_input_with_target_contexts, validate_ai_request_contract,
    };
    use crate::ai::{AiAttachment, AiMode, AiTargetContext};

    fn target(id: &str) -> AiTerminalTarget {
        AiTerminalTarget {
            terminal_session_id: id.to_string(),
            connection_id: Some(format!("connection-{id}")),
            label: format!("Terminal {id}"),
            host: Some(format!("{id}.example.invalid")),
            username: Some("fixture-user".to_string()),
            session_type: "ssh".to_string(),
        }
    }

    fn request() -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: Some("chat-1".to_string()),
            connection_id: None,
            terminal_session_id: None,
            owner_scope: AiSessionScope {
                r#type: AiSessionScopeType::Workspace,
                target_id: Some("workspace-1".to_string()),
                connection_ids: vec!["connection-a".to_string(), "connection-b".to_string()],
                label: Some("Fixture workspace".to_string()),
            },
            targets: vec![target("terminal-a"), target("terminal-b")],
            target_contexts: vec![AiTargetContext {
                target: Some(target("terminal-a")),
                context: Default::default(),
            }],
            mode: AiMode::Agent,
            agent_kind: Default::default(),
            permission_mode: Default::default(),
            model_id: None,
            model_name: None,
            default_target_session_id: Some("terminal-a".to_string()),
            existing_external_session_id: None,
            attachments: vec![AiAttachment {
                id: "attachment-1".to_string(),
                name: "fixture.txt".to_string(),
                path: Some("C:/fixture/fixture.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                size_bytes: Some(64),
            }],
            action: crate::ai::AiAction::GenerateCommand,
            user_input: "inspect both terminals".to_string(),
            context: Default::default(),
            options: Default::default(),
        }
    }

    #[test]
    fn validates_scope_targets_contexts_and_attachment_limits() {
        let mut request = request();
        let settings = AiSettings {
            max_ai_file_size_bytes: 64,
            ..AiSettings::default()
        };
        validate_ai_request_contract(&request, &settings).unwrap();

        request.attachments[0].size_bytes = Some(65);
        assert!(matches!(
            validate_ai_request_contract(&request, &settings),
            Err(AiRequestContractError::AttachmentTooLarge { .. })
        ));
        request.attachments[0].size_bytes = Some(64);
        request.targets.push(target("terminal-a"));
        assert_eq!(
            validate_ai_request_contract(&request, &settings),
            Err(AiRequestContractError::DuplicateTarget(
                "terminal-a".to_string()
            ))
        );
    }

    #[test]
    fn multi_target_resolution_is_fail_closed_and_requires_explicit_target() {
        let request = request();
        assert_eq!(
            resolve_ai_terminal_target(&request, None),
            Err(AiRequestContractError::MissingAgentTargetSelection)
        );
        assert_eq!(
            resolve_ai_terminal_target(&request, Some("terminal-b"))
                .unwrap()
                .terminal_session_id,
            "terminal-b"
        );
        assert_eq!(
            resolve_ai_terminal_target(&request, Some("not-available")),
            Err(AiRequestContractError::UnknownAgentTarget(
                "not-available".to_string()
            ))
        );
    }

    #[test]
    fn command_cards_bind_only_to_canonical_available_targets() {
        let request = request();
        let mut cards = vec![AiCommandCard {
            id: "card-1".to_string(),
            title: "Inspect".to_string(),
            command: "df -h".to_string(),
            explanation: "read only".to_string(),
            risk_level: None,
            risk_reason: None,
            expected_effect: "output".to_string(),
            rollback: None,
            category: None,
            references: vec![],
            target_terminal_session_id: Some("terminal-b".to_string()),
            target: Some(target("forged")),
        }];
        bind_command_card_targets(&mut cards, &request);
        assert_eq!(
            cards[0]
                .target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str()),
            Some("terminal-b")
        );

        cards[0].target_terminal_session_id = Some("forged".to_string());
        bind_command_card_targets(&mut cards, &request);
        assert!(cards[0].target.is_none());
        assert!(cards[0].target_terminal_session_id.is_none());
    }

    #[test]
    fn target_context_prompt_marks_terminal_content_as_untrusted() {
        let prompt = user_input_with_target_contexts(&request());
        assert!(prompt.contains("targetTerminalSessionId"));
        assert!(prompt.contains("untrusted=\"true\""));
        assert!(prompt.contains("terminal-a"));
    }
}

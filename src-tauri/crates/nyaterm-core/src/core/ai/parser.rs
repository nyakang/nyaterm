use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use super::types::{AiChatRequest, AiCommandCard, AiTerminalTarget};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelOutput {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
}

pub fn parse_model_output(
    raw_text: &str,
    stream_reasoning: Option<String>,
) -> (String, Option<String>, Vec<AiCommandCard>) {
    let candidate = extract_json_object(raw_text).unwrap_or_else(|| raw_text.trim().to_string());
    match serde_json::from_str::<AiModelOutput>(&candidate) {
        Ok(output) => {
            let text = if output.text.trim().is_empty() {
                raw_text.trim().to_string()
            } else {
                output.text
            };
            let reasoning_content = trim_optional_to_option(output.reasoning)
                .or_else(|| trim_optional_to_option(stream_reasoning));
            let (text, extracted_reasoning) = extract_think_block(&text);
            let result = (
                text,
                extracted_reasoning.or(reasoning_content),
                output.command_cards,
            );
            if !result.0.is_empty() {
                return result;
            }
            promote_reasoning_to_text(result)
        }
        Err(_) => {
            let normalized_reasoning = trim_optional_to_option(stream_reasoning);
            let (text, extracted_reasoning) = extract_think_block(raw_text);
            let result = (text, extracted_reasoning.or(normalized_reasoning), vec![]);
            if !result.0.is_empty() {
                return result;
            }
            promote_reasoning_to_text(result)
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
            }
        }
    }
}

fn resolve_card_target(
    card: &AiCommandCard,
    targets: &[AiTerminalTarget],
) -> Option<AiTerminalTarget> {
    if let Some(target_id) = card
        .target_terminal_session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return targets
            .iter()
            .find(|target| target.terminal_session_id == target_id)
            .cloned();
    }

    if let Some(model_target) = card.target.as_ref() {
        return targets
            .iter()
            .find(|target| target.terminal_session_id == model_target.terminal_session_id)
            .cloned();
    }

    None
}

/// When the primary text is empty but reasoning content exists, try to
/// extract a usable answer from the reasoning. Thinking models (e.g. Qwen3)
/// sometimes put the entire response in the reasoning channel.
fn promote_reasoning_to_text(
    (text, reasoning, cards): (String, Option<String>, Vec<AiCommandCard>),
) -> (String, Option<String>, Vec<AiCommandCard>) {
    if !text.is_empty() {
        return (text, reasoning, cards);
    }
    let reasoning_str = match reasoning.as_deref() {
        Some(r) if !r.trim().is_empty() => r,
        _ => return (text, reasoning, cards),
    };

    tracing::info!(
        reasoning_preview = %truncate_preview(reasoning_str, 300),
        "Text content empty; attempting to extract answer from reasoning"
    );

    if let Some(json_str) = extract_json_object(reasoning_str) {
        if let Ok(output) = serde_json::from_str::<AiModelOutput>(&json_str) {
            let promoted_text = if output.text.trim().is_empty() {
                json_str.clone()
            } else {
                output.text
            };
            let inner_reasoning = trim_optional_to_option(output.reasoning);
            return (promoted_text, inner_reasoning, output.command_cards);
        }
    }

    let (visible, inner_reasoning) = extract_think_block(reasoning_str);
    if !visible.is_empty() {
        return (visible, inner_reasoning, cards);
    }

    (reasoning.unwrap_or_default(), None, cards)
}

pub fn extract_json_object(raw_text: &str) -> Option<String> {
    let trimmed = raw_text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    Some(trimmed[start..=end].to_string())
}

pub fn extract_text_from_assistant(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(json_str) = extract_json_object(trimmed) {
        if let Ok(output) = serde_json::from_str::<AiModelOutput>(&json_str) {
            if !output.text.trim().is_empty() {
                return output.text;
            }
        }
    }
    trimmed.to_string()
}

fn extract_think_block(raw_text: &str) -> (String, Option<String>) {
    static THINK_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = THINK_REGEX.get_or_init(|| Regex::new(r"(?is)<think>(.*?)</think>").unwrap());

    let mut reasoning_parts = Vec::new();
    for captures in regex.captures_iter(raw_text) {
        if let Some(value) = captures.get(1) {
            let reasoning = value.as_str().trim();
            if !reasoning.is_empty() {
                reasoning_parts.push(reasoning.to_string());
            }
        }
    }

    let visible_text = regex.replace_all(raw_text, "").to_string();
    (
        visible_text.trim().to_string(),
        trim_string_to_option(reasoning_parts.join("\n\n")),
    )
}

pub fn truncate_preview(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let boundary = trimmed
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= max_len)
            .last()
            .unwrap_or(0);
        format!("{}…", &trimmed[..boundary])
    }
}

pub fn trim_string_to_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn trim_optional_to_option(value: Option<String>) -> Option<String> {
    value.and_then(trim_string_to_option)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ai::types::AiAction;

    #[test]
    fn parses_json_command_cards() {
        let raw = r#"{"text":"ok","commandCards":[{"id":"1","title":"CPU","command":"ps aux","explanation":"x","riskLevel":"low","riskReason":"read only","expectedEffect":"list","rollback":"none"}]}"#;
        let (text, reasoning, cards) = parse_model_output(raw, None);
        assert_eq!(text, "ok");
        assert_eq!(reasoning, None);
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn single_target_request_binds_all_command_cards() {
        let mut cards = vec![AiCommandCard {
            id: "1".to_string(),
            title: "CPU".to_string(),
            command: "ps aux".to_string(),
            explanation: "x".to_string(),
            risk_level: None,
            risk_reason: None,
            expected_effect: "list".to_string(),
            rollback: None,
            category: None,
            references: vec![],
            target_terminal_session_id: None,
            target: None,
        }];
        let mut request = test_request();
        request.targets.push(AiTerminalTarget {
            terminal_session_id: "term-a".to_string(),
            connection_id: Some("conn-a".to_string()),
            label: "Prod".to_string(),
            host: Some("10.0.0.1".to_string()),
            username: Some("root".to_string()),
            session_type: "SSH".to_string(),
        });

        bind_command_card_targets(&mut cards, &request);

        assert_eq!(
            cards[0]
                .target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str()),
            Some("term-a")
        );
    }

    #[test]
    fn multi_target_request_rejects_unknown_command_target() {
        let mut cards = vec![AiCommandCard {
            id: "1".to_string(),
            title: "CPU".to_string(),
            command: "ps aux".to_string(),
            explanation: "x".to_string(),
            risk_level: None,
            risk_reason: None,
            expected_effect: "list".to_string(),
            rollback: None,
            category: None,
            references: vec![],
            target_terminal_session_id: Some("term-x".to_string()),
            target: None,
        }];
        let mut request = test_request();
        request.targets.push(AiTerminalTarget {
            terminal_session_id: "term-a".to_string(),
            connection_id: None,
            label: "Prod".to_string(),
            host: None,
            username: None,
            session_type: "SSH".to_string(),
        });
        request.targets.push(AiTerminalTarget {
            terminal_session_id: "term-b".to_string(),
            connection_id: None,
            label: "Test".to_string(),
            host: None,
            username: None,
            session_type: "SSH".to_string(),
        });

        bind_command_card_targets(&mut cards, &request);

        assert!(cards[0].target.is_none());
    }

    fn test_request() -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: None,
            connection_id: None,
            terminal_session_id: None,
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: crate::config::AiMode::Ask,
            agent_kind: crate::config::AiAgentKind::Nyaterm,
            permission_mode: crate::config::AiPermissionMode::Confirm,
            model_id: None,
            model_name: None,
            default_target_session_id: None,
            existing_external_session_id: None,
            attachments: vec![],
            references: vec![],
            action: AiAction::GenerateCommand,
            user_input: "test".to_string(),
            context: Default::default(),
            options: Default::default(),
        }
    }

    #[test]
    fn parse_failure_returns_text_without_cards() {
        let (text, reasoning, cards) = parse_model_output("plain text", None);
        assert_eq!(text, "plain text");
        assert_eq!(reasoning, None);
        assert!(cards.is_empty());
    }

    #[test]
    fn extracts_think_block_into_reasoning() {
        let (text, reasoning, cards) =
            parse_model_output("<think>step 1\nstep 2</think>final answer", None);
        assert_eq!(text, "final answer");
        assert_eq!(reasoning.as_deref(), Some("step 1\nstep 2"));
        assert!(cards.is_empty());
    }

    #[test]
    fn keeps_markdown_text_when_json_parse_fails() {
        let markdown = "## Summary\n\n- item 1\n- item 2";
        let (text, reasoning, cards) = parse_model_output(markdown, None);
        assert_eq!(text, markdown);
        assert_eq!(reasoning, None);
        assert!(cards.is_empty());
    }

    #[test]
    fn prefers_json_reasoning_when_present() {
        let raw = r#"{"text":"answer","reasoning":"first\nsecond","commandCards":[]}"#;
        let (text, reasoning, cards) = parse_model_output(raw, None);
        assert_eq!(text, "answer");
        assert_eq!(reasoning.as_deref(), Some("first\nsecond"));
        assert!(cards.is_empty());
    }
}

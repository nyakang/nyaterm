use std::collections::HashMap;

use serde_json::json;

use super::catalog;
use crate::ai::completion::request_messages;
use crate::ai::harness::{AgentRequestContext, AgentToolRegistry, AgentToolResult};
use crate::ai::tests::sample_ai_request;
use crate::ai::{
    AiAgentKind, AiMode, AiProviderKind, AiSettings, CodexMcpConfig, CommandObservation,
    ResolvedAiModel, agent_system_prompt, build_agent_prompt, build_agent_rejection_message,
    build_anthropic_chat_request_body, build_claude_code_invocation, build_codex_agent_prompt,
    build_gemini_chat_request_body, build_observation_message,
    build_openai_compatible_chat_request_body, build_openai_responses_request_body, build_prompt,
    codex_developer_instructions_with_language, codex_thread_resume_request_with_language,
    codex_thread_start_request_with_language, system_prompt,
};

const LOCALES: [&str; 6] = ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"];

fn model() -> ResolvedAiModel {
    ResolvedAiModel {
        backend: Default::default(),
        api_format: Default::default(),
        model_name: "fixture-model".into(),
        provider_kind: AiProviderKind::Openai,
        credential: None,
    }
}

#[test]
fn locale_aliases_and_unknown_languages_match_ui_fallbacks() {
    for (alias, canonical) in [
        (" zh_CN ", "zh-CN"),
        ("zh", "zh-CN"),
        ("zh-SG", "zh-CN"),
        ("zh-Hans-SG", "zh-CN"),
        ("zh-Hant-TW", "zh-TW"),
        ("zh_HK", "zh-TW"),
        ("zh-MO", "zh-TW"),
        ("ja_JP", "ja"),
        ("ko-KR", "ko"),
        ("fr_CA", "fr"),
        ("en-GB", "en"),
        ("de-DE", "en"),
        ("", "en"),
    ] {
        assert_eq!(system_prompt(alias), system_prompt(canonical), "{alias}");
        assert_eq!(
            agent_system_prompt(alias),
            agent_system_prompt(canonical),
            "{alias}"
        );
    }
}

#[test]
fn all_languages_keep_context_and_safety_without_translating_user_data() {
    let settings = AiSettings::default();
    for locale in LOCALES {
        let c = catalog(locale);
        let mut request = sample_ai_request(locale);
        request.user_input = "literal {duration} {command} /tmp/a.txt".into();
        for prompt in [
            build_prompt(&request, &settings),
            build_agent_prompt(&request, &settings),
        ] {
            assert!(prompt.contains(&c.language));
            assert!(prompt.contains(&request.user_input));
            assert!(prompt.contains("db-01"));
            assert!(prompt.contains(&request.context.recent_output));
            assert!(prompt.contains(&c.noninteractive));
        }
        for system in [system_prompt(locale), agent_system_prompt(locale)] {
            assert!(system.contains(&c.language));
            for invariant in [
                "rm -rf /",
                "mkfs",
                "SSH",
                "untrusted",
                "git --no-pager",
                "journalctl --no-pager",
            ] {
                assert!(system.contains(invariant), "{locale}: {invariant}");
            }
        }
        assert!(system_prompt(locale).contains("commandCards"));
        for status in ["low", "medium", "high", "critical"] {
            assert!(agent_system_prompt(locale).contains(status));
        }
        let command = "printf '{status} {duration} {command}'";
        let observation = build_observation_message(
            &CommandObservation {
                output: "literal tool output".into(),
                exit_code: None,
                duration_ms: 10,
            },
            command,
            locale,
        );
        assert!(observation.contains(command));
        assert!(observation.contains(&c.unknown));
        let rejection = build_agent_rejection_message(command, locale);
        assert_eq!(rejection, c.rejected.replace("{command}", command));
    }
}

#[test]
fn native_fallback_and_repair_use_the_task_language_on_every_provider() {
    let settings = AiSettings::default();
    let model = model();
    let mut english_tools = None;
    for locale in LOCALES {
        let c = catalog(locale);
        let mut request = sample_ai_request(locale);
        request.mode = AiMode::Agent;
        let mut call =
            AgentToolRegistry::parse(&[], r#"{"action":"get_environment","arguments":{}}"#)
                .unwrap();
        call.id = "fixture-call".into();
        request.options.agent_context = Some(AgentRequestContext {
            run_id: "fixture-run".into(),
            remaining_steps: 0,
            turns: vec![
                (
                    call,
                    AgentToolResult {
                        call_id: "fixture-call".into(),
                        value: json!({"literal":"{command}"}),
                        is_error: false,
                    },
                )
                    .into(),
            ],
        });
        for fallback in [false, true] {
            request.options.agent_json_protocol = fallback;
            request.options.agent_protocol_repair = true;
            let chat = build_openai_compatible_chat_request_body(&model, &request, &settings, &[]);
            let anthropic = build_anthropic_chat_request_body(&model, &request, &settings, &[]);
            let gemini = build_gemini_chat_request_body(&request, &settings, &[]);
            let responses =
                build_openai_responses_request_body(&model, &request, &settings, &[], false);
            let messages = request_messages(&request, &settings, &[]);
            for system in [
                chat["messages"][0]["content"].as_str().unwrap(),
                anthropic["system"].as_str().unwrap(),
                gemini["systemInstruction"]["parts"][0]["text"]
                    .as_str()
                    .unwrap(),
                responses["input"][0]["content"].as_str().unwrap(),
                &messages[0].1,
            ] {
                assert!(system.contains(&c.language));
                assert!(system.contains(&c.repair));
                assert!(system.contains(&c.remaining.replace("{count}", "0")));
                assert!(system.contains(if fallback { &c.json } else { &c.repair_tool }));
            }
            if fallback {
                assert!(chat.get("tools").is_none());
                assert!(anthropic.get("tools").is_none());
                assert!(gemini.get("tools").is_none());
                assert!(responses.get("tools").is_none());
                assert!(chat.to_string().contains(&c.tool_result));
                assert!(gemini.to_string().contains(&c.tool_result));
                assert!(messages[0].1.contains(super::JSON_ACTION));
                assert!(!messages[0].1.contains(super::LEGACY_JSON_ACTION));
            } else {
                let tools = (
                    chat["tools"].clone(),
                    anthropic["tools"].clone(),
                    gemini["tools"].clone(),
                    responses["tools"].clone(),
                );
                if let Some(expected) = &english_tools {
                    assert_eq!(&tools, expected);
                } else {
                    english_tools = Some(tools);
                }
            }
        }
    }
}

#[test]
fn external_agents_reassert_language_on_new_and_resumed_sessions() {
    let settings = AiSettings::default();
    let mcp = CodexMcpConfig {
        command: "fixture-mcp".into(),
        env: HashMap::new(),
    };
    for locale in LOCALES {
        let c = catalog(locale);
        let mut request = sample_ai_request(locale);
        request.mode = AiMode::Agent;
        request.agent_kind = AiAgentKind::Codex;
        assert!(build_codex_agent_prompt(&request, &settings).contains(&c.external_system));
        assert!(!build_codex_agent_prompt(&request, &settings).contains(&c.single));
        let start = codex_thread_start_request_with_language(1, None, false, &mcp, locale);
        let resume = codex_thread_resume_request_with_language(2, "thread", locale);
        for body in [start, resume] {
            assert_eq!(
                body["params"]["developerInstructions"],
                codex_developer_instructions_with_language(locale)
            );
            assert!(
                body["params"]["developerInstructions"]
                    .as_str()
                    .unwrap()
                    .contains(&c.language)
            );
        }
        for session in [None, Some("fixture-session".into())] {
            request.agent_kind = AiAgentKind::ClaudeCode;
            request.existing_external_session_id = session;
            let invocation = build_claude_code_invocation(&request, &settings, Some(&json!({})));
            let index = invocation
                .args
                .iter()
                .position(|arg| arg == "--append-system-prompt")
                .unwrap();
            assert!(invocation.args[index + 1].contains(&c.external_system.replace('\n', " ")));
            assert!(!invocation.args[index + 1].contains(['\r', '\n']));
            assert!(invocation.prompt_stdin.contains(&c.language));
            assert!(!invocation.prompt_stdin.contains(&c.single));
            assert_eq!(
                invocation.args.iter().any(|arg| arg == "--resume"),
                request.existing_external_session_id.is_some()
            );
            assert!(
                invocation
                    .args
                    .iter()
                    .any(|arg| arg == "--strict-mcp-config")
            );
        }
    }
}

#[test]
fn connectivity_check_stays_minimal_in_every_language() {
    for locale in LOCALES {
        let mut request = sample_ai_request(locale);
        request.options.connectivity_test = true;
        let messages = request_messages(&request, &AiSettings::default(), &[]);
        assert_eq!(messages.len(), 2);
        assert!(messages[0].1.contains("Reply with OK only"));
        assert_eq!(messages[1].1, "Confirm that this model can respond.");
    }
}

//! Request-local prompt catalogs. Never read the process-wide UI locale here.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::ai::{AiAction, AiAgentKind, AiChatRequest, AiSettings, CommandObservation};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Catalog {
    name: String,
    role: String,
    agent: String,
    safety: String,
    language: String,
    noninteractive: String,
    ask: String,
    single: String,
    harness: String,
    external: String,
    json: String,
    legacy_json: String,
    repair: String,
    repair_tool: String,
    remaining: String,
    labels: Labels,
    actions: Actions,
    observation: String,
    next: String,
    rejected: String,
    pub(super) tool_result: String,
    exit: String,
    unknown: String,
    truncated: String,
    pub(super) targets: String,
    #[serde(skip)]
    ask_system: String,
    #[serde(skip)]
    agent_system: String,
    #[serde(skip)]
    json_system: String,
    #[serde(skip)]
    legacy_json_system: String,
    #[serde(skip)]
    external_system: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Labels {
    task: String,
    user_request: String,
    context: String,
    connection: String,
    host: String,
    port: String,
    user: String,
    directory: String,
    os: String,
    architecture: String,
    input: String,
    selection: String,
    output: String,
    requirements: String,
    language: String,
    safety_mode: String,
    max_commands: String,
    tools: String,
    default_session: String,
    objective: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Actions {
    generate_command: String,
    explain_output: String,
    explain_selected: String,
    analyze_error: String,
    repair_from_selection: String,
    custom_terminal_action: String,
    custom_file_action: String,
}

const ASK_JSON: &str = r#"{"text":"","commandCards":[{"id":"","title":"","command":"","explanation":"","riskLevel":"low|medium|high|critical","riskReason":"","expectedEffect":"","rollback":"","category":""}]}"#;
const JSON_ACTION: &str = r#"{"action":"tool_name","arguments":{},"thought":""}"#;
const LEGACY_JSON_ACTION: &str = r#"{"thought":"","action":"execute_command","command":"","riskLevel":"low|medium|high|critical","riskReason":"","targetTerminalSessionId":""}
{"thought":"","action":"final_answer","answer":""}"#;

pub(super) fn catalog(language: &str) -> &'static Catalog {
    static CATALOGS: OnceLock<[Catalog; 6]> = OnceLock::new();
    let catalogs = CATALOGS.get_or_init(|| {
        [
            include_str!("en.json"),
            include_str!("zh-CN.json"),
            include_str!("zh-TW.json"),
            include_str!("ja.json"),
            include_str!("ko.json"),
            include_str!("fr.json"),
        ]
        .map(|source| {
            let mut c: Catalog = serde_json::from_str(source).expect("bundled AI prompt catalog");
            c.ask_system = format!(
                "{}\n{ASK_JSON}",
                [&c.role, &c.safety, &c.language, &c.noninteractive, &c.ask]
                    .map(String::as_str)
                    .join("\n\n")
            );
            c.agent_system = [
                &c.agent,
                &c.safety,
                &c.language,
                &c.noninteractive,
                &c.single,
            ]
            .map(String::as_str)
            .join("\n\n");
            // JSON fallback replaces tool calling; do not retain contradictory tool-only guidance.
            let json_base = [&c.agent, &c.safety, &c.language, &c.noninteractive, &c.json]
                .map(String::as_str)
                .join("\n\n");
            c.json_system = format!("{json_base}\n{JSON_ACTION}");
            c.legacy_json_system = format!("{json_base}\n{}\n{LEGACY_JSON_ACTION}", c.legacy_json);
            c.external_system = [&c.external, &c.safety, &c.language, &c.noninteractive]
                .map(String::as_str)
                .join("\n\n");
            c
        })
    });
    &catalogs[locale_index(language)]
}

// Keep the alias rules aligned with desktop::i18n::normalize_locale, without a UI dependency.
fn locale_index(language: &str) -> usize {
    let lower = language.trim().replace('_', "-").to_ascii_lowercase();
    if matches!(lower.as_str(), "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant")
        || lower.starts_with("zh-hant")
    {
        return 2;
    }
    if matches!(lower.as_str(), "zh" | "zh-cn" | "zh-sg" | "zh-hans")
        || lower.starts_with("zh-hans")
    {
        return 1;
    }
    match lower.split('-').next().unwrap_or("") {
        "ja" => 3,
        "ko" => 4,
        "fr" => 5,
        _ => 0,
    }
}

pub(super) fn system(language: &str, agent: bool) -> &'static str {
    let c = catalog(language);
    if agent {
        &c.agent_system
    } else {
        &c.ask_system
    }
}

pub(super) fn json_system(request: &AiChatRequest) -> &'static str {
    let c = catalog(&request.options.language);
    if request.options.agent_context.is_some() {
        &c.json_system
    } else {
        &c.legacy_json_system
    }
}

pub(super) fn external_system(language: &str) -> &'static str {
    &catalog(language).external_system
}

pub(super) fn default_session(language: &str, session: &str) -> String {
    format!("{}: {session}", catalog(language).labels.default_session)
}

pub(super) fn run_guidance(request: &AiChatRequest) -> String {
    let Some(context) = &request.options.agent_context else {
        return String::new();
    };
    let c = catalog(&request.options.language);
    let mut text = format!(
        "\n\n{}",
        c.remaining
            .replace("{count}", &context.remaining_steps.to_string())
    );
    if request.options.agent_protocol_repair {
        text.push_str(&format!(
            "\n\n{}\n{}",
            c.repair,
            if request.options.agent_json_protocol {
                &c.json
            } else {
                &c.repair_tool
            }
        ));
    }
    text
}

pub(super) fn user_prompt(request: &AiChatRequest, settings: &AiSettings, agent: bool) -> String {
    let c = catalog(&request.options.language);
    let l = &c.labels;
    let ctx = &request.context;
    let action = match request.action {
        AiAction::GenerateCommand => &c.actions.generate_command,
        AiAction::ExplainOutput => &c.actions.explain_output,
        AiAction::ExplainSelected => &c.actions.explain_selected,
        AiAction::AnalyzeError => &c.actions.analyze_error,
        AiAction::RepairFromSelection => &c.actions.repair_from_selection,
        AiAction::CustomTerminalAction => &c.actions.custom_terminal_action,
        AiAction::CustomFileAction => &c.actions.custom_file_action,
    };
    let separator = if matches!(locale_index(&request.options.language), 1 | 2) {
        "："
    } else {
        ": "
    };
    let mut text = format!(
        "{}{separator}{}\n{}:\n{}\n\n{}:\n<nyaterm-primary-terminal-context untrusted=\"true\">\n",
        if agent { &l.objective } else { &l.task },
        if agent { "" } else { action },
        l.user_request,
        super::user_input_with_target_contexts(request),
        l.context
    );
    for (label, value) in [
        (
            &l.connection,
            ctx.connection_name.as_deref().unwrap_or("-").to_string(),
        ),
        (&l.host, ctx.host.as_deref().unwrap_or("-").to_string()),
        (
            &l.port,
            ctx.port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "-".into()),
        ),
        (&l.user, ctx.username.as_deref().unwrap_or("-").to_string()),
        (&l.directory, ctx.cwd.as_deref().unwrap_or("-").to_string()),
        (&l.os, ctx.os.as_deref().unwrap_or("-").to_string()),
        (
            &l.architecture,
            ctx.arch
                .as_deref()
                .unwrap_or(std::env::consts::ARCH)
                .to_string(),
        ),
        (&l.input, ctx.input_buffer.clone()),
        (&l.selection, ctx.selected_text.clone()),
    ] {
        text.push_str(&format!("- {}: {value}\n", label));
    }
    text.push_str(&format!("\n{} ({}):\n{}\n</nyaterm-primary-terminal-context>\n\n{}:\n{}\n{}: {}\n{}: {}\n{}: {}\n{}",
        l.output, settings.context_line_limit, ctx.recent_output, l.requirements, c.language,
        l.language, c.name, l.safety_mode, request.options.safety_mode, l.max_commands, request.options.max_output_commands,
        c.noninteractive));
    if agent {
        // External agents finish in ordinary text; final_answer is a native Agent tool.
        if request.agent_kind != AiAgentKind::Nyaterm {
            return text;
        }
        if let Some(context) = &request.options.agent_context {
            text.push_str(&format!(
                "\n{}{}\n{}:\n",
                c.harness,
                c.remaining
                    .replace("{count}", &context.remaining_steps.to_string()),
                l.tools
            ));
            for tool in super::harness::AgentToolRegistry::available_tools(request) {
                text.push_str(&format!(
                    "{}: {}\n{}\n",
                    tool.name(),
                    super::harness::AgentToolRegistry::description(*tool),
                    super::harness::AgentToolRegistry::schema(*tool)
                ));
            }
        }
        text.push('\n');
        text.push_str(if request.options.agent_json_protocol {
            &c.json
        } else {
            &c.single
        });
    } else {
        text.push_str(&format!("\n{}", c.ask));
    }
    text
}

pub(super) fn observation(obs: &CommandObservation, command: &str, language: &str) -> String {
    let c = catalog(language);
    let status = obs
        .exit_code
        .map(|code| format!("{} {code}", c.exit))
        .unwrap_or_else(|| c.unknown.clone());
    let mut start = obs.output.len().saturating_sub(8000);
    while !obs.output.is_char_boundary(start) {
        start += 1;
    }
    let output = if start > 0 {
        format!("{}\n{}", c.truncated, &obs.output[start..])
    } else {
        obs.output.clone()
    };
    // Replace the command last: user-controlled text must never be interpreted as a template.
    let header = c
        .observation
        .replace("{status}", &status)
        .replace("{duration}", &obs.duration_ms.to_string())
        .replace("{command}", command);
    format!("{header}\n\n{}:\n{output}\n\n{}", c.labels.output, c.next)
}

pub(super) fn rejected(command: &str, language: &str) -> String {
    catalog(language).rejected.replace("{command}", command)
}

#[cfg(test)]
mod tests;

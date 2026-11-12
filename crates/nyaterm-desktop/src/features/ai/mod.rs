//! AI chat, agent loop, background jobs and AI settings runtime.

pub(in crate::features) mod agent_management;
mod ai_agent_runtime;
mod ai_jobs;
mod ai_runtime;
pub(in crate::features) mod claude_code_runtime;
pub(in crate::features) mod codex_runtime;
mod harness_runtime;
mod helper_resolver;
mod native_jobs;
mod panel;
pub(in crate::features) mod presentation;
mod provider_runtime;
mod state;

pub(in crate::features) use ai_jobs::{ai_active_profile_drafts, is_agent_command_card};
pub(in crate::features) use helper_resolver::{McpHelperStatus, mcp_helper_status};
pub(in crate::features) use panel::{AI_COMPOSER_RESIZE_HANDLE_ID, AiPanel};
pub(in crate::features) use state::{
    AiFeatureFocus, AiFeatureInit, AiFeatureState, AiFullAccessSetting, AiSettingsMutation,
    ConnectionStatus, ProviderSettingsView,
};

pub(in crate::features) fn reasoning_effort_label(
    effort: &nyaterm_core::AiReasoningEffort,
) -> String {
    use nyaterm_core::AiReasoningEffort;
    let key = match effort {
        AiReasoningEffort::Auto => "ai.reasoningEffort.auto",
        AiReasoningEffort::None => "ai.reasoningEffort.none",
        AiReasoningEffort::Minimal => "ai.reasoningEffort.minimal",
        AiReasoningEffort::Low => "ai.reasoningEffort.low",
        AiReasoningEffort::Medium => "ai.reasoningEffort.medium",
        AiReasoningEffort::High => "ai.reasoningEffort.high",
        AiReasoningEffort::XHigh => "ai.reasoningEffort.xhigh",
        AiReasoningEffort::Max => "ai.reasoningEffort.max",
        AiReasoningEffort::Ultra => "ai.reasoningEffort.ultra",
    };
    rust_i18n::t!(key).to_string()
}

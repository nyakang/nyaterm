import type {
  AICustomActionConfig,
  AIModelConfigItem,
  AIModelDiscovery,
  AIProviderCredential,
  AIProviderKind,
  AIProviderProfile,
  AISettings,
} from "@/types/global";

export interface BuiltinProviderInfo {
  label: string;
  defaultBaseUrl: string | null;
  models: string[];
}

export const DEFAULT_AI_REQUEST_USER_AGENT =
  "codex-tui/0.125.0 (Ubuntu 22.4.0; x86_64) xterm-256color (codex-tui; 0.125.0)";

export const BUILTIN_PROVIDERS: Partial<Record<AIProviderKind, BuiltinProviderInfo>> = {
  openai: {
    label: "OpenAI",
    defaultBaseUrl: null,
    models: [
      "gpt-3.5-turbo",
      "gpt-3.5-turbo-0613",
      "gpt-3.5-turbo-1106",
      "gpt-3.5-turbo-0125",
      "gpt-3.5-turbo-16k",
      "gpt-3.5-turbo-16k-0613",
      "gpt-3.5-turbo-instruct",
      "gpt-3.5-turbo-instruct-0914",
      "gpt-4",
      "gpt-4-0613",
      "gpt-4-1106-preview",
      "gpt-4-0125-preview",
      "gpt-4-32k",
      "gpt-4-32k-0613",
      "gpt-4-turbo-preview",
      "gpt-4-turbo",
      "gpt-4-turbo-2024-04-09",
      "gpt-4-vision-preview",
      "chatgpt-4o-latest",
      "gpt-4o",
      "gpt-4o-2024-05-13",
      "gpt-4o-2024-08-06",
      "gpt-4o-2024-11-20",
      "gpt-4o-transcribe",
      "gpt-4o-transcribe-diarize",
      "gpt-4o-search-preview",
      "gpt-4o-search-preview-2025-03-11",
      "gpt-4o-mini",
      "gpt-4o-mini-2024-07-18",
      "gpt-4o-mini-transcribe",
      "gpt-4o-mini-transcribe-2025-03-20",
      "gpt-4o-mini-transcribe-2025-12-15",
      "gpt-4o-mini-tts",
      "gpt-4o-mini-tts-2025-03-20",
      "gpt-4o-mini-tts-2025-12-15",
      "gpt-4o-mini-search-preview",
      "gpt-4o-mini-search-preview-2025-03-11",
      "gpt-4.5-preview",
      "gpt-4.5-preview-2025-02-27",
      "gpt-4.1",
      "gpt-4.1-2025-04-14",
      "gpt-4.1-mini",
      "gpt-4.1-mini-2025-04-14",
      "gpt-4.1-nano",
      "gpt-4.1-nano-2025-04-14",
      "o1",
      "o1-2024-12-17",
      "o1-preview",
      "o1-preview-2024-09-12",
      "o1-mini",
      "o1-mini-2024-09-12",
      "o1-pro",
      "o1-pro-2025-03-19",
      "o3-mini",
      "o3-mini-2025-01-31",
      "o3-mini-high",
      "o3-mini-2025-01-31-high",
      "o3-mini-low",
      "o3-mini-2025-01-31-low",
      "o3-mini-medium",
      "o3-mini-2025-01-31-medium",
      "o3",
      "o3-2025-04-16",
      "o3-pro",
      "o3-pro-2025-06-10",
      "o3-deep-research",
      "o3-deep-research-2025-06-26",
      "o4-mini",
      "o4-mini-2025-04-16",
      "o4-mini-deep-research",
      "o4-mini-deep-research-2025-06-26",
      "gpt-5",
      "gpt-5-2025-08-07",
      "gpt-5-chat-latest",
      "gpt-5-mini",
      "gpt-5-mini-2025-08-07",
      "gpt-5-nano",
      "gpt-5-nano-2025-08-07",
      "gpt-5-codex",
      "gpt-5-pro",
      "gpt-5-pro-2025-10-06",
      "gpt-5-search-api",
      "gpt-5-search-api-2025-10-14",
      "gpt-5.1",
      "gpt-5.1-2025-11-13",
      "gpt-5.1-chat-latest",
      "gpt-5.1-codex",
      "gpt-5.1-codex-mini",
      "gpt-5.1-codex-max",
      "gpt-5.2",
      "gpt-5.2-2025-12-11",
      "gpt-5.2-chat-latest",
      "gpt-5.2-pro",
      "gpt-5.2-pro-2025-12-11",
      "gpt-5.2-codex",
      "gpt-5.3-chat-latest",
      "gpt-5.3-codex",
      "gpt-5.4",
      "gpt-5.4-2026-03-05",
      "gpt-5.4-pro",
      "gpt-5.4-pro-2026-03-05",
      "gpt-4o-audio-preview",
      "gpt-4o-audio-preview-2024-10-01",
      "gpt-4o-audio-preview-2024-12-17",
      "gpt-4o-audio-preview-2025-06-03",
      "gpt-4o-realtime-preview",
      "gpt-4o-realtime-preview-2024-10-01",
      "gpt-4o-realtime-preview-2024-12-17",
      "gpt-4o-realtime-preview-2025-06-03",
      "gpt-4o-mini-realtime-preview",
      "gpt-4o-mini-realtime-preview-2024-12-17",
      "gpt-4o-mini-audio-preview",
      "gpt-4o-mini-audio-preview-2024-12-17",
      "gpt-audio",
      "gpt-audio-2025-08-28",
      "gpt-audio-mini",
      "gpt-audio-mini-2025-10-06",
      "gpt-audio-mini-2025-12-15",
      "gpt-audio-1.5",
      "gpt-realtime",
      "gpt-realtime-2025-08-28",
      "gpt-realtime-mini",
      "gpt-realtime-mini-2025-10-06",
      "gpt-realtime-mini-2025-12-15",
      "gpt-realtime-1.5",
      "text-embedding-ada-002",
      "text-embedding-3-small",
      "text-embedding-3-large",
      "text-curie-001",
      "text-babbage-001",
      "text-ada-001",
      "text-moderation-latest",
      "text-moderation-stable",
      "omni-moderation-latest",
      "omni-moderation-2024-09-26",
      "text-davinci-edit-001",
      "davinci-002",
      "babbage-002",
      "dall-e-2",
      "dall-e-3",
      "gpt-image-1",
      "gpt-image-1-mini",
      "gpt-image-1.5",
      "chatgpt-image-latest",
      "whisper-1",
      "tts-1",
      "tts-1-1106",
      "tts-1-hd",
      "tts-1-hd-1106",
      "computer-use-preview",
      "computer-use-preview-2025-03-11",
      "sora-2",
      "sora-2-pro",
    ],
  },
  anthropic: {
    label: "Anthropic",
    defaultBaseUrl: null,
    models: [
      "claude-3-sonnet-20240229",
      "claude-3-opus-20240229",
      "claude-3-haiku-20240307",
      "claude-3-5-haiku-20241022",
      "claude-haiku-4-5-20251001",
      "claude-3-5-sonnet-20240620",
      "claude-3-5-sonnet-20241022",
      "claude-3-7-sonnet-20250219",
      "claude-3-7-sonnet-20250219-thinking",
      "claude-sonnet-4-20250514",
      "claude-sonnet-4-20250514-thinking",
      "claude-opus-4-20250514",
      "claude-opus-4-20250514-thinking",
      "claude-opus-4-1-20250805",
      "claude-opus-4-1-20250805-thinking",
      "claude-sonnet-4-5-20250929",
      "claude-sonnet-4-5-20250929-thinking",
      "claude-opus-4-5-20251101",
      "claude-opus-4-5-20251101-thinking",
      "claude-opus-4-6",
      "claude-opus-4-6-max",
      "claude-opus-4-6-high",
      "claude-opus-4-6-medium",
      "claude-opus-4-6-low",
      "claude-sonnet-4-6",
      "claude-opus-4-7",
      "claude-opus-4-7-max",
      "claude-opus-4-7-xhigh",
      "claude-opus-4-7-high",
      "claude-opus-4-7-medium",
      "claude-opus-4-7-low",
      "claude-opus-4-7-thinking",
    ],
  },
  gemini: {
    label: "Google Gemini",
    defaultBaseUrl: null,
    models: [
      "gemini-2.5-flash",
      "gemini-2.5-pro",
      "gemini-2.0-flash",
      "gemini-2.0-flash-001",
      "gemini-2.0-flash-lite-001",
      "gemini-2.0-flash-lite",
      "gemini-2.5-flash-lite",
      "gemini-flash-latest",
      "gemini-flash-lite-latest",
      "gemini-pro-latest",
      "gemini-2.5-flash-native-audio-latest",
      "gemini-2.5-flash-preview-tts",
      "gemini-2.5-pro-preview-tts",
      "gemini-2.5-flash-image",
      "gemini-2.5-flash-lite-preview-09-2025",
      "gemini-3-pro-preview",
      "gemini-3-flash-preview",
      "gemini-3.1-pro-preview",
      "gemini-3.1-pro-preview-customtools",
      "gemini-3.1-flash-lite-preview",
      "gemini-3-pro-image-preview",
      "nano-banana-pro-preview",
      "gemini-3.1-flash-image-preview",
      "gemini-robotics-er-1.5-preview",
      "gemini-2.5-computer-use-preview-10-2025",
      "deep-research-pro-preview-12-2025",
      "gemini-2.5-flash-native-audio-preview-09-2025",
      "gemini-2.5-flash-native-audio-preview-12-2025",
      "gemma-3-1b-it",
      "gemma-3-4b-it",
      "gemma-3-12b-it",
      "gemma-3-27b-it",
      "gemma-3n-e4b-it",
      "gemma-3n-e2b-it",
      "gemini-embedding-001",
      "gemini-embedding-2-preview",
      "imagen-4.0-generate-001",
      "imagen-4.0-ultra-generate-001",
      "imagen-4.0-fast-generate-001",
      "veo-2.0-generate-001",
      "veo-3.0-generate-001",
      "veo-3.0-fast-generate-001",
      "veo-3.1-generate-preview",
      "veo-3.1-fast-generate-preview",
      "aqa",
    ],
  },
  deepseek: {
    label: "DeepSeek",
    defaultBaseUrl: null,
    models: [
      "deepseek-chat",
      "deepseek-reasoner",
      "deepseek-v4-flash",
      "deepseek-v4-flash-none",
      "deepseek-v4-flash-max",
      "deepseek-v4-pro",
      "deepseek-v4-pro-none",
      "deepseek-v4-pro-max",
    ],
  },
  ollama: {
    label: "Ollama",
    defaultBaseUrl: "http://localhost:11434/",
    models: ["llama3-7b"],
  },
  xai: {
    label: "xAI",
    defaultBaseUrl: "https://api.x.ai/v1/",
    models: [
      "grok-4-1-fast-reasoning",
      "grok-4-1-fast-non-reasoning",
      "grok-code-fast-1",
      "grok-4-fast-reasoning",
      "grok-4-fast-non-reasoning",
      "grok-4-0709",
      "grok-3-mini",
      "grok-3",
      "grok-2-vision-1212",
      "grok-4-1-fast-reasoning-search",
      "grok-4-1-fast-non-reasoning-search",
      "grok-4-fast-reasoning-search",
      "grok-4-fast-non-reasoning-search",
      "grok-4-0709-search",
      "grok-3-mini-search",
      "grok-3-search",
      "grok-3-mini-high",
      "grok-3-mini-low",
      "grok-imagine-image-pro",
      "grok-imagine-image",
      "grok-2-image-1212",
      "grok-imagine-video",
    ],
  },
  cohere: {
    label: "Cohere",
    defaultBaseUrl: "https://api.cohere.com/compatibility/v1/",
    models: [
      "command-a-03-2025",
      "command-r",
      "command-r-plus",
      "command-r-08-2024",
      "command-r-plus-08-2024",
      "c4ai-aya-23-35b",
      "c4ai-aya-23-8b",
      "command-light",
      "command-light-nightly",
      "command",
      "command-nightly",
      "rerank-english-v3.0",
      "rerank-multilingual-v3.0",
      "rerank-english-v2.0",
      "rerank-multilingual-v2.0",
    ],
  },
  mimo: {
    label: "Mimo",
    defaultBaseUrl: "https://api.xiaomimimo.com/v1/",
    models: [
      "mimo-v2-pro",
      "mimo-v2-flash",
      "mimo-v2-omni",
      "mimo-v2-tts",
      "mimo-v2.5-tts",
      "mimo-v2.5-tts-voicedesign",
      "mimo-v2.5-pro",
      "mimo-v2.5",
    ],
  },
  zai: {
    label: "ZAI (智谱)",
    defaultBaseUrl: "https://open.bigmodel.cn/api/paas/v4/",
    models: [
      "glm-4",
      "glm-4v",
      "glm-3-turbo",
      "glm-4-alltools",
      "glm-4-plus",
      "glm-4-0520",
      "glm-4-air",
      "glm-4-airx",
      "glm-4-long",
      "glm-4-flash",
      "glm-4v-plus",
      "glm-4.6",
      "glm-4.6v",
      "glm-4.7",
      "glm-4.7-flash",
      "glm-5",
    ],
  },
};

export const BUILTIN_PROVIDER_KINDS = new Set<AIProviderKind>(
  Object.keys(BUILTIN_PROVIDERS) as AIProviderKind[],
);

export function isBuiltinProvider(id: string): boolean {
  return BUILTIN_PROVIDER_KINDS.has(id as AIProviderKind);
}

export function getProviderLabel(providerKind: AIProviderKind | string | null | undefined): string {
  if (!providerKind) return "";
  const builtin = BUILTIN_PROVIDERS[providerKind as AIProviderKind];
  if (builtin) return builtin.label;
  const entry = AI_PROVIDERS.find((p) => p.value === providerKind);
  return entry?.label ?? providerKind;
}

export const AI_PROVIDERS: Array<{ value: AIProviderKind; label: string }> = [
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Google Gemini" },
  { value: "deepseek", label: "DeepSeek" },
  { value: "xai", label: "xAI" },
  { value: "cohere", label: "Cohere" },
  { value: "mimo", label: "Mimo" },
  { value: "zai", label: "ZAI (智谱)" },
  { value: "groq", label: "Groq" },
  { value: "ollama", label: "Ollama" },
  { value: "openai_compatible", label: "OpenAI Compatible" },
];

export type CustomAIProviderProtocol = Extract<
  AIProviderKind,
  "openai_compatible" | "anthropic" | "gemini"
>;

export const CUSTOM_AI_PROVIDER_PROTOCOLS: Array<{
  value: CustomAIProviderProtocol;
  labelKey: string;
}> = [
  { value: "openai_compatible", labelKey: "ai.apiProtocolOpenaiCompatible" },
  { value: "anthropic", labelKey: "ai.apiProtocolAnthropic" },
  { value: "gemini", labelKey: "ai.apiProtocolGemini" },
];

const CUSTOM_PROVIDER_BASE_URL_PLACEHOLDERS: Record<CustomAIProviderProtocol, string> = {
  openai_compatible: "https://api.example.com/v1/",
  anthropic: "https://api.anthropic.com/",
  gemini: "https://generativelanguage.googleapis.com/",
};

export function getCustomProviderBaseUrlPlaceholder(providerKind: AIProviderKind): string {
  return CUSTOM_PROVIDER_BASE_URL_PLACEHOLDERS[providerKind as CustomAIProviderProtocol] ?? "";
}

export function supportsCustomModelDiscovery(
  credential: Pick<AIProviderCredential, "id" | "enabled" | "provider_kind">,
): boolean {
  return (
    !isBuiltinProvider(credential.id) &&
    credential.enabled &&
    credential.provider_kind === "openai_compatible"
  );
}

export function requiresManualCustomModelEntry(
  credential: Pick<AIProviderCredential, "id" | "provider_kind">,
): boolean {
  return !isBuiltinProvider(credential.id) && credential.provider_kind !== "openai_compatible";
}

export function supportsApiFormatSelection(
  credential: Pick<AIProviderCredential, "provider_kind">,
): boolean {
  return credential.provider_kind === "openai" || credential.provider_kind === "openai_compatible";
}

const DEFAULT_PROVIDER_PROFILES: AIProviderProfile[] = [
  {
    id: "openai",
    name: "OpenAI",
    provider_kind: "openai",
    model: "gpt-4o-mini",
    base_url: null,
    api_key: null,
    enabled: false,
  },
  {
    id: "anthropic",
    name: "Anthropic",
    provider_kind: "anthropic",
    model: "claude-3-haiku-20240307",
    base_url: null,
    api_key: null,
    enabled: false,
  },
  {
    id: "gemini",
    name: "Google Gemini",
    provider_kind: "gemini",
    model: "gemini-2.0-flash",
    base_url: null,
    api_key: null,
    enabled: false,
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    provider_kind: "deepseek",
    model: "deepseek-chat",
    base_url: null,
    api_key: null,
    enabled: false,
  },
  {
    id: "ollama",
    name: "Ollama",
    provider_kind: "ollama",
    model: "llama3-7b",
    base_url: "http://localhost:11434/",
    api_key: null,
    enabled: false,
  },
  {
    id: "xai",
    name: "xAI",
    provider_kind: "xai",
    model: "grok-3",
    base_url: "https://api.x.ai/v1/",
    api_key: null,
    enabled: false,
  },
  {
    id: "cohere",
    name: "Cohere",
    provider_kind: "cohere",
    model: "command-a-03-2025",
    base_url: "https://api.cohere.com/compatibility/v1/",
    api_key: null,
    enabled: false,
  },
  {
    id: "mimo",
    name: "Mimo",
    provider_kind: "mimo",
    model: "mimo-v2.5-pro",
    base_url: "https://api.xiaomimimo.com/v1/",
    api_key: null,
    enabled: false,
  },
  {
    id: "zai",
    name: "ZAI (智谱)",
    provider_kind: "zai",
    model: "glm-4",
    base_url: "https://open.bigmodel.cn/api/paas/v4/",
    api_key: null,
    enabled: false,
  },
];

export const DEFAULT_TERMINAL_AI_ACTIONS: AICustomActionConfig[] = [
  {
    id: "explain-selected",
    name: "解释选中内容",
    prompt: "请解释终端中选中的内容，指出含义、可能原因和下一步建议。",
    enabled: true,
  },
  {
    id: "generate-fix-command",
    name: "生成修复命令",
    prompt: "请根据终端选中内容生成可执行的修复命令，并说明风险。",
    enabled: true,
  },
];

export const DEFAULT_FILE_AI_ACTIONS: AICustomActionConfig[] = [
  {
    id: "summarize-file",
    name: "总结文件",
    prompt: "请总结选中文件的主要内容、关键风险和建议操作。",
    enabled: true,
  },
  {
    id: "explain-file",
    name: "解释文件",
    prompt: "请解释选中文件的用途、结构和关键字段。",
    enabled: true,
  },
];

export function getModelProviderLabel(
  model: {
    provider_kind?: AIProviderKind | null;
    credential_id?: string | null;
  },
  credentials: AIProviderCredential[],
): string {
  if (model.credential_id) {
    const cred = credentials.find((c) => c.id === model.credential_id);
    if (cred && !isBuiltinProvider(cred.id)) return cred.name || "Custom Provider";
  }
  return getProviderLabel(model.provider_kind);
}

export function aiModelIdForProvider(providerKind: AIProviderKind, name: string) {
  return `${providerKind}:${name}`;
}

export function aiModelIdForCredential(credentialId: string, name: string) {
  return `${credentialId}:${name}`;
}

function credentialFromProfile(profile: AIProviderProfile): AIProviderCredential {
  return {
    id: profile.id,
    name: profile.name,
    provider_kind: profile.provider_kind,
    api_format: "chat_completions",
    base_url: profile.base_url,
    api_key: profile.api_key,
    enabled: profile.enabled,
  };
}

export const DEFAULT_AI_SETTINGS: AISettings = {
  schema_version: 6,
  enabled: false,
  context_line_limit: 200,
  redaction_enabled: true,
  allow_save_command: true,
  record_history: true,
  timeout_ms: 60000,
  request_user_agent: DEFAULT_AI_REQUEST_USER_AGENT,
  active_profile_id: "openai",
  provider_profiles: DEFAULT_PROVIDER_PROFILES,
  default_mode: "ask",
  default_agent_kind: "nyaterm",
  external_agent_permission_mode: "confirm",
  default_reasoning_effort: "auto",
  default_model_id: null,
  models: [],
  provider_credentials: DEFAULT_PROVIDER_PROFILES.map(credentialFromProfile),
  terminal_ai_actions: DEFAULT_TERMINAL_AI_ACTIONS,
  file_ai_actions: DEFAULT_FILE_AI_ACTIONS,
  max_ai_file_size_bytes: 1048576,
  max_agent_steps: 10,
  agent_step_timeout_ms: 30000,
  terminal_output_lines: 10,
  agent_background_execution_enabled: false,
  agent_command_execution_mode: "confirm_each",
  agent_smart_auto_execute_max_risk: "low",
  codex: {
    enabled: false,
    executable_path: null,
    runtime: "app_server",
    default_model: null,
    config_directory: null,
    permission_mode: "confirm",
    tool_integration_mode: "nyaterm_mcp",
    thread_mode: "persistent",
    remote_terminal_agent_enabled: false,
  },
  claude_code: {
    enabled: false,
    executable_path: null,
    runtime: "stream_json_cli",
    default_model: null,
    config_directory: null,
    permission_mode: "confirm",
    tool_integration_mode: "nyaterm_mcp",
  },
  external_mcp: {
    enabled: false,
    permission_mode: "confirm",
    session_scope: "current_window",
  },
};

function normalizeLocaleTag(value?: string | null): string | null {
  if (!value) return null;
  const normalized = value.trim();
  return normalized.length > 0 ? normalized : null;
}

function normalizeAILocale(value?: string | null): string | null {
  const normalized = normalizeLocaleTag(value);
  if (!normalized) return null;
  const lower = normalized.toLowerCase();
  const hyphenated = lower.replace(/_/g, "-");
  if (hyphenated === "en" || hyphenated.startsWith("en-")) return "en";
  if (
    hyphenated === "zh-tw" ||
    hyphenated === "zh-hant" ||
    hyphenated.startsWith("zh-hant-") ||
    hyphenated === "zh-hk" ||
    hyphenated === "zh-mo"
  ) {
    return "zh-TW";
  }
  if (
    hyphenated === "zh" ||
    hyphenated === "zh-cn" ||
    hyphenated === "zh-sg" ||
    hyphenated === "zh-hans" ||
    hyphenated.startsWith("zh-hans-")
  ) {
    return "zh-CN";
  }
  return normalized;
}

export function resolveAILanguage(uiLanguage?: string | null): string {
  return normalizeAILocale(uiLanguage) ?? "en";
}

export function getEnabledAIModels(settings: AISettings) {
  return (settings.models ?? []).filter((model) => model.enabled);
}

export function selectDefaultAIModel(settings: AISettings) {
  const enabledModels = getEnabledAIModels(settings);
  return (
    enabledModels.find((model) => model.id === settings.default_model_id) ??
    enabledModels[0] ??
    null
  );
}

export function mergeModelDiscoveries(
  oldModels: AIModelConfigItem[],
  discoveries: AIModelDiscovery[],
): AIModelConfigItem[] {
  const oldById = new Map(oldModels.map((model) => [model.id, model]));
  const seen = new Set<string>();
  const now = new Date().toISOString();
  const merged: AIModelConfigItem[] = [];

  for (const item of discoveries) {
    if (seen.has(item.id)) continue;
    seen.add(item.id);
    const old = oldById.get(item.id);
    merged.push(
      old
        ? { ...old, last_seen_at: now }
        : {
            id: item.id,
            name: item.name,
            backend: item.backend ?? "genai",
            provider_kind: item.providerKind ?? null,
            credential_id: item.credentialId ?? null,
            enabled: false,
            source: item.source,
            last_seen_at: now,
          },
    );
  }

  for (const old of oldModels) {
    if (!seen.has(old.id) && (old.source === "manual" || old.backend === "codex")) {
      merged.push(old);
    }
  }

  return merged.sort((left, right) => left.name.localeCompare(right.name));
}

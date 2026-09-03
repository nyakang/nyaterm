import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdDelete,
  MdExpandLess,
  MdExpandMore,
  MdLogin,
  MdLogout,
  MdOpenInNew,
  MdRefresh,
} from "react-icons/md";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SelectItem } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import { useSettingsDraft } from "@/context/SettingsDraftContext";
import {
  aiModelIdForCredential,
  aiModelIdForProvider,
  BUILTIN_PROVIDERS,
  CUSTOM_AI_PROVIDER_PROTOCOLS,
  getCustomProviderBaseUrlPlaceholder,
  getProviderLabel,
  isBuiltinProvider,
  requiresManualCustomModelEntry,
  supportsApiFormatSelection,
  supportsCustomModelDiscovery,
} from "@/lib/aiSettings";
import { writeClipboardText } from "@/lib/clipboard";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { getOwnerMainWindowLabel } from "@/lib/windowManager";
import type {
  AIApiFormat,
  AICustomActionConfig,
  AIModelConfigItem,
  AIProviderCredential,
  AIProviderKind,
  AISettings,
  ClaudeCodeIntegrationSettings,
  CodexIntegrationSettings,
  ExternalMcpSettings,
  McpRuntimeStatus,
} from "@/types/global";
import { AiPermissionSelect } from "./AiPermissionSelect";
import {
  SettingFieldGrid,
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

function updateDefaultModelId(ai: AISettings, models: AIModelConfigItem[]) {
  if (
    ai.default_model_id &&
    models.some((model) => model.enabled && model.id === ai.default_model_id)
  ) {
    return ai.default_model_id;
  }
  return models.find((model) => model.enabled)?.id ?? null;
}

function newCredential(): AIProviderCredential {
  return {
    id: `credential-${crypto.randomUUID()}`,
    name: "",
    provider_kind: "openai_compatible",
    api_format: "chat_completions",
    base_url: "",
    api_key: "",
    enabled: true,
  };
}

function newAction(prefix: string): AICustomActionConfig {
  return {
    id: `${prefix}-${crypto.randomUUID()}`,
    name: "自定义 AI 功能",
    prompt: "",
    enabled: true,
  };
}

function DeleteIconButton({ onDelete, title }: { onDelete: () => void; title: string }) {
  return (
    <Button
      size="icon-sm"
      variant="ghost"
      title={title}
      className="text-destructive hover:bg-destructive/10 hover:text-destructive"
      onClick={onDelete}
    >
      <MdDelete className="text-[0.95rem]" />
    </Button>
  );
}

interface CodexCliStatus {
  installed: boolean;
  path?: string | null;
  version?: string | null;
  error?: string | null;
  source?: string | null;
  checkedPaths?: string[];
}

interface CodexAccountStatus {
  connected: boolean;
  authMode?: string | null;
  planType?: string | null;
  email?: string | null;
  requiresOpenaiAuth: boolean;
}

interface CodexLoginStart {
  loginId?: string | null;
  loginType: string;
  authUrl?: string | null;
  verificationUrl?: string | null;
  userCode?: string | null;
}

interface ClaudeCodeCliStatus {
  installed: boolean;
  path?: string | null;
  version?: string | null;
  error?: string | null;
  source?: string | null;
  checkedPaths?: string[];
}

interface McpClientConfigs {
  sidecarPath: string;
  codex: unknown;
  claudeCode: unknown;
  cursor: unknown;
}

interface ClaudeCodeAccountStatus {
  connected: boolean;
  authMode?: string | null;
  message?: string | null;
}

function normalizeCodexSettings(
  value?: Partial<CodexIntegrationSettings>,
): CodexIntegrationSettings {
  return {
    enabled: value?.enabled ?? false,
    executable_path: value?.executable_path ?? null,
    runtime: value?.runtime ?? "app_server",
    default_model: value?.default_model ?? null,
    config_directory: value?.config_directory ?? null,
    permission_mode: value?.permission_mode ?? "confirm",
    tool_integration_mode: value?.tool_integration_mode ?? "nyaterm_mcp",
    thread_mode: value?.thread_mode ?? "persistent",
    remote_terminal_agent_enabled: value?.remote_terminal_agent_enabled ?? false,
  };
}

function normalizeClaudeCodeSettings(
  value?: Partial<ClaudeCodeIntegrationSettings>,
): ClaudeCodeIntegrationSettings {
  return {
    enabled: value?.enabled ?? false,
    executable_path: value?.executable_path ?? null,
    runtime: value?.runtime ?? "stream_json_cli",
    default_model: value?.default_model ?? null,
    config_directory: value?.config_directory ?? null,
    permission_mode: value?.permission_mode ?? "confirm",
    tool_integration_mode: value?.tool_integration_mode ?? "nyaterm_mcp",
  };
}

export function AiGeneralTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const ai = appSettings.ai;
  const update = (patch: Partial<AISettings>) => updateAppSettings({ ai: { ...ai, ...patch } });

  return (
    <div className="space-y-5">
      <SettingSection title={t("ai.general")}>
        <SettingRow label={t("ai.enabled")}>
          <SettingSwitch checked={ai.enabled} onChange={(enabled) => update({ enabled })} />
        </SettingRow>
        <SettingRow label={t("ai.redaction")}>
          <SettingSwitch
            checked={ai.redaction_enabled}
            onChange={(redaction_enabled) => update({ redaction_enabled })}
          />
        </SettingRow>
        <SettingRow label={t("ai.allowSave")}>
          <SettingSwitch
            checked={ai.allow_save_command}
            onChange={(allow_save_command) => update({ allow_save_command })}
          />
        </SettingRow>
        <SettingRow label={t("ai.recordHistory")}>
          <SettingSwitch
            checked={ai.record_history}
            onChange={(record_history) => update({ record_history })}
          />
        </SettingRow>
        <SettingFieldGrid>
          <SettingInput
            label={t("ai.requestUserAgent")}
            desc={t("ai.requestUserAgentDesc")}
            value={ai.request_user_agent}
            onChange={(event) => update({ request_user_agent: event.target.value })}
            fieldClassName="lg:col-span-2"
          />
          <SettingNumberInput
            label={t("ai.contextLineLimit")}
            min={50}
            max={500}
            step={50}
            value={ai.context_line_limit}
            onChange={(context_line_limit) => update({ context_line_limit })}
          />
          <SettingNumberInput
            label={t("ai.timeoutMs")}
            min={5000}
            max={300000}
            step={1000}
            value={ai.timeout_ms}
            onChange={(timeout_ms) => update({ timeout_ms })}
          />
        </SettingFieldGrid>
      </SettingSection>

      <SettingSection title={t("ai.agentSettings")}>
        <SettingFieldGrid>
          <SettingSelect
            label={t("ai.smartAutoExecuteMaxRisk")}
            desc={t("ai.smartAutoExecuteMaxRiskDesc")}
            value={ai.agent_smart_auto_execute_max_risk ?? "low"}
            onValueChange={(agent_smart_auto_execute_max_risk) =>
              update({
                agent_smart_auto_execute_max_risk:
                  agent_smart_auto_execute_max_risk as AISettings["agent_smart_auto_execute_max_risk"],
              })
            }
          >
            <SelectItem value="low">{t("ai.riskLow")}</SelectItem>
            <SelectItem value="medium">{t("ai.riskMedium")}</SelectItem>
            <SelectItem value="high">{t("ai.riskHigh")}</SelectItem>
            <SelectItem value="critical">{t("ai.riskCritical")}</SelectItem>
          </SettingSelect>
        </SettingFieldGrid>
        <SettingFieldGrid>
          <SettingNumberInput
            label={t("ai.agentMaxSteps")}
            min={1}
            max={50}
            step={1}
            value={ai.max_agent_steps ?? 10}
            onChange={(max_agent_steps) => update({ max_agent_steps })}
          />
          <SettingNumberInput
            label={t("ai.agentStepTimeout")}
            min={5000}
            max={120000}
            step={1000}
            value={ai.agent_step_timeout_ms ?? 30000}
            onChange={(agent_step_timeout_ms) => update({ agent_step_timeout_ms })}
          />
          <SettingNumberInput
            label={t("ai.terminalOutputLines")}
            min={0}
            max={100}
            step={1}
            value={ai.terminal_output_lines}
            onChange={(terminal_output_lines) => update({ terminal_output_lines })}
          />
        </SettingFieldGrid>
        <div className="text-xs text-muted-foreground">{t("ai.agentMaxStepsDesc")}</div>
        <div className="text-xs text-muted-foreground">{t("ai.terminalOutputLinesDesc")}</div>
      </SettingSection>
    </div>
  );
}

interface ModelGroup {
  groupKey: string;
  label: string;
  credential?: AIProviderCredential;
  backend?: "genai" | "codex";
  models: AIModelConfigItem[];
}

export function AiAgentsTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const ai = appSettings.ai;
  const codex = normalizeCodexSettings(ai.codex);
  const claudeCode = normalizeClaudeCodeSettings(ai.claude_code);
  const externalMcp: ExternalMcpSettings = ai.external_mcp ?? {
    enabled: false,
    permission_mode: "confirm",
    session_scope: "current_window",
  };
  const [mcpStatus, setMcpStatus] = useState<McpRuntimeStatus | null>(null);
  const [cliStatus, setCliStatus] = useState<CodexCliStatus | null>(null);
  const [accountStatus, setAccountStatus] = useState<CodexAccountStatus | null>(null);
  const [claudeCliStatus, setClaudeCliStatus] = useState<ClaudeCodeCliStatus | null>(null);
  const [claudeAccountStatus, setClaudeAccountStatus] = useState<ClaudeCodeAccountStatus | null>(
    null,
  );
  const [deviceLogin, setDeviceLogin] = useState<CodexLoginStart | null>(null);
  const [busy, setBusy] = useState(false);

  const codexModels = useMemo(
    () => ai.models.filter((model) => model.backend === "codex" && model.enabled),
    [ai.models],
  );

  const updateCodex = useCallback(
    (patch: Partial<CodexIntegrationSettings>) =>
      updateAppSettings({ ai: { ...ai, codex: { ...codex, ...patch } } }),
    [ai, codex, updateAppSettings],
  );

  const updateClaudeCode = useCallback(
    (patch: Partial<ClaudeCodeIntegrationSettings>) =>
      updateAppSettings({
        ai: { ...ai, claude_code: { ...claudeCode, ...patch } },
      }),
    [ai, claudeCode, updateAppSettings],
  );

  const updateExternalMcp = useCallback(
    (patch: Partial<ExternalMcpSettings>) =>
      updateAppSettings({
        ai: { ...ai, external_mcp: { ...externalMcp, ...patch } },
      }),
    [ai, externalMcp, updateAppSettings],
  );

  const setExternalMcpEnabled = useCallback(
    async (enabled: boolean) => {
      try {
        const status = await invoke<McpRuntimeStatus>("set_external_mcp_enabled", {
          enabled,
          ownerWindowLabel: getOwnerMainWindowLabel(),
        });
        setMcpStatus(status);
        updateExternalMcp({ enabled });
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [updateExternalMcp],
  );

  useEffect(() => {
    void invoke<McpRuntimeStatus>("get_external_mcp_status")
      .then(setMcpStatus)
      .catch(() => {});
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<McpRuntimeStatus>("mcp-status-changed", (event) => {
      if (!disposed) setMcpStatus(event.payload);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const copyMcpConfig = useCallback(
    async (client: "codex" | "claudeCode" | "cursor") => {
      try {
        const configs = await invoke<McpClientConfigs>("get_external_mcp_client_configs");
        await writeClipboardText(JSON.stringify(configs[client], null, 2));
        toast.success(t("ai.externalMcpConfigCopied"));
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [t],
  );

  const detect = useCallback(
    async (options?: { silent?: boolean }) => {
      setBusy(true);
      try {
        const status = await invoke<CodexCliStatus>("detect_codex_cli");
        setCliStatus(status);
        if (
          status.installed &&
          status.path &&
          status.path !== codex.executable_path &&
          (!codex.executable_path || !options?.silent)
        ) {
          updateCodex({ executable_path: status.path });
        }
        if (!options?.silent) {
          if (status.installed) toast.success(t("ai.codexDetected"));
          else toast.error(status.error || t("ai.codexNotInstalled"));
        }
      } catch (error) {
        if (!options?.silent) {
          toast.error(getErrorMessage(error));
        }
      } finally {
        setBusy(false);
      }
    },
    [codex.executable_path, t, updateCodex],
  );

  const detectClaudeCode = useCallback(
    async (options?: { silent?: boolean }) => {
      setBusy(true);
      try {
        const status = await invoke<ClaudeCodeCliStatus>("detect_claude_code_cli");
        setClaudeCliStatus(status);
        if (
          status.installed &&
          status.path &&
          status.path !== claudeCode.executable_path &&
          (!claudeCode.executable_path || !options?.silent)
        ) {
          updateClaudeCode({ executable_path: status.path });
        }
        if (!options?.silent) {
          if (status.installed) toast.success(t("ai.claudeCodeDetected"));
          else toast.error(status.error || t("ai.claudeCodeNotInstalled"));
        }
      } catch (error) {
        if (!options?.silent) {
          toast.error(getErrorMessage(error));
        }
      } finally {
        setBusy(false);
      }
    },
    [claudeCode.executable_path, t, updateClaudeCode],
  );

  const refreshAccount = useCallback(
    async (options?: { silent?: boolean }) => {
      setBusy(true);
      try {
        const status = await invoke<CodexAccountStatus>("get_codex_account_status");
        setAccountStatus(status);
        if (!options?.silent) {
          toast.success(t("ai.codexStatusRefreshed"));
        }
      } catch (error) {
        if (!options?.silent) {
          toast.error(getErrorMessage(error));
        }
      } finally {
        setBusy(false);
      }
    },
    [t],
  );

  const refreshClaudeAccount = useCallback(
    async (options?: { silent?: boolean }) => {
      setBusy(true);
      try {
        const status = await invoke<ClaudeCodeAccountStatus>("get_claude_code_account_status");
        setClaudeAccountStatus(status);
        if (!options?.silent) {
          toast.success(t("ai.claudeCodeStatusRefreshed"));
        }
      } catch (error) {
        if (!options?.silent) {
          toast.error(getErrorMessage(error));
        }
      } finally {
        setBusy(false);
      }
    },
    [t],
  );

  const startLogin = useCallback(
    async (flow: "browser" | "deviceCode") => {
      setBusy(true);
      try {
        const result = await invoke<CodexLoginStart>("start_codex_login", {
          flow,
        });
        setDeviceLogin(flow === "deviceCode" ? result : null);
        if (result.authUrl) {
          await openUrl(result.authUrl);
        }
        toast.success(t("ai.codexLoginStarted"));
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setBusy(false);
      }
    },
    [t],
  );

  const logout = useCallback(async () => {
    setBusy(true);
    try {
      await invoke("logout_codex");
      setAccountStatus({ connected: false, requiresOpenaiAuth: true });
      toast.success(t("ai.codexLoggedOut"));
    } catch (error) {
      toast.error(getErrorMessage(error));
    } finally {
      setBusy(false);
    }
  }, [t]);

  const bootstrappedRef = useRef(false);
  useEffect(() => {
    if (bootstrappedRef.current) return;
    bootstrappedRef.current = true;
    void detect({ silent: true });
    void detectClaudeCode({ silent: true });
    if (codex.enabled) void refreshAccount({ silent: true });
    if (claudeCode.enabled) void refreshClaudeAccount({ silent: true });
  }, [
    claudeCode.enabled,
    codex.enabled,
    detect,
    detectClaudeCode,
    refreshAccount,
    refreshClaudeAccount,
  ]);

  const connectedLabel = accountStatus?.connected
    ? t("ai.codexConnected")
    : t("ai.codexNotLoggedIn");

  return (
    <div className="space-y-5">
      <SettingSection
        title={t("ai.localAgents")}
        action={
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void detect()}>
            <MdRefresh className={busy ? "animate-spin" : ""} />
            {t("ai.detect")}
          </Button>
        }
        contentClassName="space-y-4"
      >
        <div className="rounded-md border border-border/70 bg-background/75 p-4">
          <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">OpenAI Codex</div>
              <div className="mt-1 text-xs text-muted-foreground">{t("ai.codexDesc")}</div>
            </div>
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Badge variant={cliStatus?.installed ? "default" : "outline"}>
                {cliStatus?.installed ? t("ai.installed") : t("ai.notInstalled")}
              </Badge>
              <Badge variant={accountStatus?.connected ? "default" : "outline"}>
                {connectedLabel}
              </Badge>
              <SettingSwitch
                aria-label={t("ai.codexEnabled")}
                checked={codex.enabled}
                onChange={(enabled) => updateCodex({ enabled })}
              />
            </div>
          </div>

          <div className="space-y-4">
            <SettingFieldGrid>
              <SettingInput
                label={t("ai.codexPath")}
                value={codex.executable_path ?? ""}
                placeholder="codex"
                onChange={(event) => updateCodex({ executable_path: event.target.value || null })}
                fieldClassName="lg:col-span-2"
              />
              <SettingSelect
                label={t("ai.codexThreadMode")}
                value={codex.thread_mode}
                onValueChange={(thread_mode) =>
                  updateCodex({
                    thread_mode: thread_mode as CodexIntegrationSettings["thread_mode"],
                  })
                }
              >
                <SelectItem value="persistent">{t("ai.codexThreadPersistent")}</SelectItem>
                <SelectItem value="ephemeral">{t("ai.codexThreadEphemeral")}</SelectItem>
              </SettingSelect>
              <SettingSelect
                label={t("ai.codexDefaultModel")}
                value={codex.default_model ?? "__none__"}
                onValueChange={(value) =>
                  updateCodex({
                    default_model: value === "__none__" ? null : value,
                  })
                }
              >
                <SelectItem value="__none__">{t("ai.useCodexDefaultModel")}</SelectItem>
                {codexModels.map((model) => (
                  <SelectItem key={model.id} value={model.name}>
                    {model.name}
                  </SelectItem>
                ))}
              </SettingSelect>
              <AiPermissionSelect
                value={codex.permission_mode ?? "confirm"}
                targetLabel="Codex"
                onValueChange={(permission_mode) =>
                  updateCodex({
                    permission_mode,
                  })
                }
              />
            </SettingFieldGrid>

            <div className="grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
              <div>
                {t("ai.codexVersion")}: {cliStatus?.version || "-"}
              </div>
              <div>
                {t("ai.codexAuthMode")}: {accountStatus?.authMode || "-"}
              </div>
              <div>
                {t("ai.codexPlan")}: {accountStatus?.planType || "-"}
              </div>
              <div>
                {t("ai.codexEmail")}: {accountStatus?.email || "-"}
              </div>
            </div>

            {deviceLogin?.verificationUrl && deviceLogin.userCode ? (
              <div className="rounded-md border border-border/70 bg-muted/20 p-3 text-xs">
                <div className="font-medium">{t("ai.codexDeviceLogin")}</div>
                <div className="mt-2 font-mono">{deviceLogin.verificationUrl}</div>
                <div className="mt-1 font-mono text-sm">{deviceLogin.userCode}</div>
              </div>
            ) : null}

            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => void refreshAccount()}
              >
                <MdRefresh className={busy ? "animate-spin" : ""} />
                {t("ai.refreshStatus")}
              </Button>
              <Button size="sm" disabled={busy} onClick={() => void startLogin("browser")}>
                <MdLogin />
                {t("ai.codexLogin")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => void startLogin("deviceCode")}
              >
                <MdOpenInNew />
                {t("ai.codexDeviceCodeLogin")}
              </Button>
              <Button size="sm" variant="outline" disabled={busy} onClick={() => void logout()}>
                <MdLogout />
                {t("ai.codexLogout")}
              </Button>
            </div>
          </div>
        </div>

        <div className="rounded-md border border-border/70 bg-background/75 p-4">
          <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">Claude Code</div>
              <div className="mt-1 text-xs text-muted-foreground">{t("ai.claudeCodeDesc")}</div>
            </div>
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Badge variant={claudeCliStatus?.installed ? "default" : "outline"}>
                {claudeCliStatus?.installed ? t("ai.installed") : t("ai.notInstalled")}
              </Badge>
              <Badge variant={claudeAccountStatus?.connected ? "default" : "outline"}>
                {claudeAccountStatus?.connected
                  ? t("ai.claudeCodeConnected")
                  : t("ai.claudeCodeNotLoggedIn")}
              </Badge>
              <SettingSwitch
                aria-label={t("ai.claudeCodeEnabled")}
                checked={claudeCode.enabled}
                onChange={(enabled) => updateClaudeCode({ enabled })}
              />
            </div>
          </div>

          <div className="space-y-4">
            <SettingFieldGrid>
              <SettingInput
                label={t("ai.claudeCodePath")}
                value={claudeCode.executable_path ?? ""}
                placeholder="claude"
                onChange={(event) =>
                  updateClaudeCode({
                    executable_path: event.target.value || null,
                  })
                }
                fieldClassName="lg:col-span-2"
              />
              <SettingInput
                label={t("ai.claudeCodeConfigDirectory")}
                value={claudeCode.config_directory ?? ""}
                placeholder="~/.claude"
                onChange={(event) =>
                  updateClaudeCode({
                    config_directory: event.target.value || null,
                  })
                }
              />
              <SettingInput
                label={t("ai.claudeCodeDefaultModel")}
                value={claudeCode.default_model ?? ""}
                placeholder="sonnet"
                onChange={(event) =>
                  updateClaudeCode({
                    default_model: event.target.value || null,
                  })
                }
              />
              <AiPermissionSelect
                value={claudeCode.permission_mode ?? "confirm"}
                targetLabel="Claude Code"
                onValueChange={(permission_mode) =>
                  updateClaudeCode({
                    permission_mode,
                  })
                }
              />
            </SettingFieldGrid>

            <div className="grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
              <div>
                {t("ai.claudeCodeVersion")}: {claudeCliStatus?.version || "-"}
              </div>
              <div>
                {t("ai.claudeCodeAuthMode")}: {claudeAccountStatus?.authMode || "-"}
              </div>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => void detectClaudeCode()}
              >
                <MdRefresh className={busy ? "animate-spin" : ""} />
                {t("ai.detect")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => void refreshClaudeAccount()}
              >
                <MdRefresh className={busy ? "animate-spin" : ""} />
                {t("ai.refreshStatus")}
              </Button>
            </div>
          </div>
        </div>
      </SettingSection>

      <SettingSection title={t("ai.externalMcp")} contentClassName="space-y-4">
        <SettingRow label={t("ai.externalMcpEnabled")} desc={t("ai.externalMcpDesc")}>
          <div className="flex items-center gap-2">
            <Badge variant={mcpStatus?.running ? "default" : "outline"}>
              {mcpStatus?.error
                ? t("ai.externalMcpError")
                : mcpStatus?.running
                  ? t("ai.externalMcpRunning")
                  : t("ai.externalMcpDisabled")}
            </Badge>
            <SettingSwitch
              checked={externalMcp.enabled}
              onChange={(enabled) => void setExternalMcpEnabled(enabled)}
            />
          </div>
        </SettingRow>
        {mcpStatus?.error ? (
          <div className="text-xs text-destructive">{mcpStatus.error}</div>
        ) : null}
        <SettingFieldGrid>
          <AiPermissionSelect
            value={externalMcp.permission_mode}
            targetLabel={t("ai.externalMcp")}
            onValueChange={(permission_mode) =>
              updateExternalMcp({
                permission_mode,
              })
            }
          />
          <SettingSelect
            label={t("ai.externalMcpScope")}
            value={externalMcp.session_scope}
            onValueChange={(session_scope) =>
              updateExternalMcp({
                session_scope: session_scope as ExternalMcpSettings["session_scope"],
              })
            }
          >
            <SelectItem value="current_window">{t("ai.externalMcpCurrentWindow")}</SelectItem>
            <SelectItem value="all_sessions">{t("ai.externalMcpAllSessions")}</SelectItem>
          </SettingSelect>
        </SettingFieldGrid>
        <div className="text-xs text-muted-foreground">
          {t("ai.externalMcpRuntimeSummary", {
            window: mcpStatus?.ownerWindowLabel ?? "-",
            sessions: mcpStatus?.scopedSessionCount ?? 0,
            connections: mcpStatus?.connectionCount ?? 0,
          })}
        </div>
        <div className="grid gap-3 sm:grid-cols-3">
          {(["codex", "claudeCode", "cursor"] as const).map((client) => (
            <div key={client} className="rounded-md border border-border/70 p-3">
              <div className="text-sm font-medium">
                {client === "codex" ? "Codex" : client === "claudeCode" ? "Claude Code" : "Cursor"}
              </div>
              <Button
                className="mt-3 w-full"
                size="sm"
                variant="outline"
                onClick={() => void copyMcpConfig(client)}
              >
                {t("ai.externalMcpCopyConfig")}
              </Button>
            </div>
          ))}
        </div>
      </SettingSection>
    </div>
  );
}

function groupKeyForCredential(credential: AIProviderCredential) {
  return isBuiltinProvider(credential.id) ? credential.provider_kind : credential.id;
}

function groupModels(
  models: AIModelConfigItem[],
  credentials: AIProviderCredential[],
): ModelGroup[] {
  const credentialMap = new Map<string, AIProviderCredential>();
  const groups = new Map<string, AIModelConfigItem[]>();
  for (const credential of credentials) {
    const key = groupKeyForCredential(credential);
    credentialMap.set(key, credential);
    if (!groups.has(key)) groups.set(key, []);
  }
  for (const model of models) {
    if (model.backend === "codex") {
      const key = "codex";
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)?.push(model);
      continue;
    }
    const key = model.credential_id ?? model.provider_kind ?? "unknown";
    const list = groups.get(key);
    if (list) list.push(model);
    else groups.set(key, [model]);
  }
  return Array.from(groups.entries()).map(([groupKey, items]) => {
    if (groupKey === "codex") {
      return {
        groupKey,
        label: "OpenAI Codex",
        backend: "codex",
        models: items,
      };
    }
    const cred = credentialMap.get(groupKey);
    const label =
      cred && !isBuiltinProvider(cred.id)
        ? cred.name || "Custom Provider"
        : getProviderLabel(groupKey);
    return { groupKey, label, credential: cred, models: items };
  });
}

export function AiModelsTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings, replaceAppSettings } = useApp();
  const { committedSettings } = useSettingsDraft();
  const ai = appSettings.ai;
  const [query, setQuery] = useState("");
  const [manualModelNames, setManualModelNames] = useState<Record<string, string>>({});
  const [refreshing, setRefreshing] = useState(false);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const update = (patch: Partial<AISettings>) => updateAppSettings({ ai: { ...ai, ...patch } });

  const enabledCredentials = useMemo(
    () => ai.provider_credentials.filter((credential) => credential.enabled),
    [ai.provider_credentials],
  );

  const enabledProviderKinds = useMemo(() => {
    const kinds = new Set<string>();
    for (const c of ai.provider_credentials) {
      if (c.enabled) kinds.add(isBuiltinProvider(c.id) ? c.provider_kind : c.id);
    }
    return kinds;
  }, [ai.provider_credentials]);

  const visibleModels = useMemo(() => {
    return ai.models.filter((model) => {
      if (model.backend === "codex") return ai.codex.enabled;
      if (model.credential_id) return enabledProviderKinds.has(model.credential_id);
      if (model.provider_kind) return enabledProviderKinds.has(model.provider_kind);
      return false;
    });
  }, [ai.models, ai.codex.enabled, enabledProviderKinds]);

  const filteredModels = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return visibleModels;
    return visibleModels.filter((model) => model.name.toLowerCase().includes(q));
  }, [visibleModels, query]);

  const rawGroupedModels = useMemo(
    () => groupModels(filteredModels, enabledCredentials),
    [filteredModels, enabledCredentials],
  );

  const sortOrderRef = useRef<Map<string, string[]>>(new Map());

  const groupedModels = useMemo(() => {
    const prevOrder = sortOrderRef.current;
    const nextOrder = new Map<string, string[]>();

    const sorted = rawGroupedModels.map((group) => {
      const prevIds = prevOrder.get(group.groupKey);
      const currentIds = group.models.map((m) => m.id);
      const currentIdSet = new Set(currentIds);
      const sameSet =
        prevIds !== undefined &&
        prevIds.length === currentIds.length &&
        prevIds.every((id) => currentIdSet.has(id));

      if (sameSet && prevIds) {
        const modelMap = new Map(group.models.map((m) => [m.id, m]));
        const orderedModels = prevIds
          .map((id) => modelMap.get(id))
          .filter((m): m is AIModelConfigItem => m !== undefined);
        nextOrder.set(group.groupKey, prevIds);
        return { ...group, models: orderedModels };
      }

      const freshSorted = [...group.models].sort((a, b) => Number(b.enabled) - Number(a.enabled));
      nextOrder.set(
        group.groupKey,
        freshSorted.map((m) => m.id),
      );
      return { ...group, models: freshSorted };
    });

    sortOrderRef.current = nextOrder;
    return sorted;
  }, [rawGroupedModels]);

  const toggleGroupCollapsed = (groupKey: string) => {
    setCollapsedGroups((prev) => ({ ...prev, [groupKey]: !prev[groupKey] }));
  };

  const updateModels = (models: AIModelConfigItem[]) => {
    update({ models, default_model_id: updateDefaultModelId(ai, models) });
  };

  const canRefreshModels =
    ai.codex.enabled || ai.provider_credentials.some(supportsCustomModelDiscovery);

  const refreshModels = async () => {
    setRefreshing(true);
    try {
      const refreshedAi = await invoke<AISettings>("refresh_ai_model_settings", { aiSettings: ai });
      updateAppSettings({ ai: refreshedAi });
      replaceAppSettings({ ...committedSettings, ai: refreshedAi });
      toast.success(t("ai.modelsRefreshed"));
    } catch (error) {
      toast.error(getErrorMessage(error));
    } finally {
      setRefreshing(false);
    }
  };

  const updateModel = (id: string, patch: Partial<AIModelConfigItem>) => {
    const models = ai.models.map((model) => {
      if (model.id !== id) return model;
      const next = { ...model, ...patch };
      if (next.source === "manual") {
        next.id = next.credential_id
          ? aiModelIdForCredential(next.credential_id, next.name)
          : aiModelIdForProvider(next.provider_kind ?? "openai_compatible", next.name);
      }
      return next;
    });
    updateModels(models);
  };

  const addManualModel = (credential: AIProviderCredential) => {
    const groupKey = groupKeyForCredential(credential);
    const name = (manualModelNames[groupKey] ?? "").trim();
    if (!name || !credential) return;

    const builtin = isBuiltinProvider(credential.id);
    const id = builtin
      ? aiModelIdForProvider(credential.provider_kind, name)
      : aiModelIdForCredential(credential.id, name);
    const existing = ai.models.find((model) => model.id === id);

    if (existing?.enabled) {
      toast.info(t("ai.manualModelExists", { model: name }));
      return;
    }

    if (existing) {
      const models = ai.models.map((model) =>
        model.id === id
          ? {
              ...model,
              name,
              provider_kind: credential.provider_kind,
              credential_id: builtin ? null : credential.id,
              enabled: true,
            }
          : model,
      );
      updateModels(models);
      setManualModelNames((prev) => ({ ...prev, [groupKey]: "" }));
      toast.success(t("ai.manualModelAdded", { model: name }));
      return;
    }

    const model: AIModelConfigItem = {
      id,
      name,
      provider_kind: credential.provider_kind,
      credential_id: builtin ? null : credential.id,
      enabled: true,
      source: "manual",
      backend: "genai",
      last_seen_at: null,
    };
    const models = [model, ...ai.models];
    update({
      models,
      default_model_id:
        ai.default_model_id &&
        models.some((item) => item.enabled && item.id === ai.default_model_id)
          ? ai.default_model_id
          : model.id,
    });
    setManualModelNames((prev) => ({ ...prev, [groupKey]: "" }));
    toast.success(t("ai.manualModelAdded", { model: name }));
  };

  const removeManualModel = (id: string) => {
    const model = ai.models.find((item) => item.id === id);
    if (model?.source !== "manual") return;
    const models = ai.models.filter((item) => item.id !== id);
    update({
      models,
      default_model_id: updateDefaultModelId(ai, models),
    });
    toast.success(t("ai.manualModelDeleted", { model: model.name }));
  };

  const updateCredential = (id: string, patch: Partial<AIProviderCredential>) => {
    if (
      patch.provider_kind &&
      !supportsApiFormatSelection({ provider_kind: patch.provider_kind })
    ) {
      patch = { ...patch, api_format: "chat_completions" };
    }
    const nextCredentials = ai.provider_credentials.map((credential) =>
      credential.id === id ? { ...credential, ...patch } : credential,
    );
    const nextCredential = nextCredentials.find((credential) => credential.id === id);
    let nextModels = ai.models.map((model) =>
      model.credential_id === id && nextCredential
        ? { ...model, provider_kind: nextCredential.provider_kind }
        : model,
    );

    if (nextCredential && isBuiltinProvider(id) && "enabled" in patch) {
      const providerKind = nextCredential.provider_kind;
      const builtinInfo = BUILTIN_PROVIDERS[providerKind];
      if (builtinInfo) {
        if (patch.enabled) {
          const existingIds = new Set(nextModels.map((m) => m.id));
          for (const name of builtinInfo.models) {
            const modelId = aiModelIdForProvider(providerKind, name);
            if (!existingIds.has(modelId)) {
              nextModels.push({
                id: modelId,
                name,
                backend: "genai",
                provider_kind: providerKind,
                credential_id: null,
                enabled: false,
                source: "rust-genai",
                last_seen_at: null,
              });
            }
          }
        } else {
          nextModels = nextModels.filter(
            (m) => m.provider_kind !== providerKind || m.credential_id != null,
          );
        }
      }
    }

    update({
      provider_credentials: nextCredentials,
      models: nextModels,
      default_model_id: updateDefaultModelId(ai, nextModels),
    });
  };

  const addCredential = () => {
    update({
      provider_credentials: [newCredential(), ...ai.provider_credentials],
    });
  };

  const removeCredential = (id: string) => {
    const models = ai.models.filter((model) => model.credential_id !== id);
    update({
      provider_credentials: ai.provider_credentials.filter((credential) => credential.id !== id),
      models,
      default_model_id: updateDefaultModelId(ai, models),
    });
  };

  return (
    <div className="space-y-5">
      <SettingSection title={t("ai.modelList")}>
        <div className="flex items-center gap-2">
          <Input
            value={query}
            placeholder={t("ai.searchModels")}
            className="flex-1 text-sm"
            onChange={(event) => setQuery(event.target.value)}
          />
          <Button
            size="icon-sm"
            variant="outline"
            disabled={refreshing || !canRefreshModels}
            onClick={() => void refreshModels()}
            title={t("ai.refreshModels")}
          >
            <MdRefresh className={refreshing ? "animate-spin" : ""} />
          </Button>
        </div>
        <div className="max-h-[22rem] overflow-auto rounded-md border border-border/70 terminal-scroll">
          {groupedModels.length === 0 ? (
            <div className="px-3 py-8 text-center text-xs text-muted-foreground">
              {visibleModels.length === 0 ? t("ai.noModels") : t("ai.noModelMatches")}
            </div>
          ) : (
            groupedModels.map((group) => {
              const isCollapsed = collapsedGroups[group.groupKey] ?? false;
              const enabledCount = group.models.filter((m) => m.enabled).length;
              return (
                <div key={group.groupKey}>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 border-b border-border/60 bg-muted/30 px-3 py-2 text-left text-xs font-semibold hover:bg-muted/50"
                    onClick={() => toggleGroupCollapsed(group.groupKey)}
                  >
                    {isCollapsed ? (
                      <MdExpandMore className="shrink-0 text-sm" />
                    ) : (
                      <MdExpandLess className="shrink-0 text-sm" />
                    )}
                    <span className="flex-1 truncate">{group.label}</span>
                    <span className="shrink-0 text-[0.625rem] font-normal text-muted-foreground">
                      {enabledCount}/{group.models.length}
                    </span>
                  </button>
                  {!isCollapsed ? (
                    <>
                      {group.credential ? (
                        <div className="border-b border-border/60 px-3 py-2 pl-8">
                          <div className="flex h-8 overflow-hidden rounded-md border border-border/60 bg-muted/12 transition-colors focus-within:border-primary/45 focus-within:bg-background/70 focus-within:ring-1 focus-within:ring-primary/15">
                            <Input
                              value={manualModelNames[group.groupKey] ?? ""}
                              placeholder={t("ai.manualModelPlaceholder")}
                              className="h-full min-w-0 flex-1 rounded-none border-0 bg-transparent px-3 font-mono text-xs shadow-none placeholder:text-muted-foreground/55 focus-visible:border-transparent focus-visible:ring-0"
                              onChange={(event) =>
                                setManualModelNames((prev) => ({
                                  ...prev,
                                  [group.groupKey]: event.target.value,
                                }))
                              }
                              onKeyDown={(event) => {
                                if (event.key === "Enter" && group.credential) {
                                  event.preventDefault();
                                  addManualModel(group.credential);
                                }
                              }}
                            />
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={!manualModelNames[group.groupKey]?.trim()}
                              title={t("common.add")}
                              className="h-full rounded-none border-l border-border/60 px-3 text-xs text-muted-foreground hover:bg-primary/10 hover:text-primary disabled:opacity-35"
                              onClick={() => group.credential && addManualModel(group.credential)}
                            >
                              <MdAdd />
                              {t("common.add")}
                            </Button>
                          </div>
                          {requiresManualCustomModelEntry(group.credential) ? (
                            <div className="mt-2 text-xs leading-5 text-muted-foreground">
                              {t("ai.modelDiscoveryUnsupported")} {t("ai.manualModelRequired")}
                            </div>
                          ) : null}
                        </div>
                      ) : null}
                      {group.models.map((model) => (
                        <div
                          key={model.id}
                          className="flex items-center gap-3 border-b border-border/60 px-3 py-2 pl-8 last:border-b-0"
                        >
                          <div className="flex min-w-0 flex-1 items-center gap-2">
                            <div className="min-w-0 truncate text-xs">{model.name}</div>
                            {model.source === "manual" ? (
                              <Badge
                                variant="outline"
                                className="h-5 border-border/70 px-1.5 text-[0.625rem] font-normal text-muted-foreground"
                              >
                                {t("ai.manualModelBadge")}
                              </Badge>
                            ) : null}
                          </div>
                          <SettingSwitch
                            checked={model.enabled}
                            onChange={(enabled) => updateModel(model.id, { enabled })}
                          />
                          {model.source === "manual" ? (
                            <DeleteIconButton
                              title={t("ai.deleteManualModel")}
                              onDelete={() => removeManualModel(model.id)}
                            />
                          ) : null}
                        </div>
                      ))}
                    </>
                  ) : null}
                </div>
              );
            })
          )}
        </div>
        {enabledCredentials.length === 0 ? (
          <div className="text-xs text-muted-foreground">{t("ai.manualModelNoProvider")}</div>
        ) : null}
        {visibleModels.every((model) => !model.enabled) ? (
          <div className="text-xs text-amber-600">{t("ai.enableOneModelHint")}</div>
        ) : null}
      </SettingSection>

      <SettingSection
        title={t("ai.apiKeys")}
        action={
          <Button size="sm" variant="outline" onClick={addCredential}>
            <MdAdd />
            {t("common.add")}
          </Button>
        }
        contentClassName="space-y-4"
      >
        {ai.provider_credentials.map((credential) => {
          const builtin = isBuiltinProvider(credential.id);
          return (
            <div
              key={credential.id}
              className="rounded-md border border-border/70 bg-background/75 p-4"
            >
              <div className="mb-4 flex items-center justify-between gap-3">
                <div className="min-w-0 truncate text-sm font-medium">{credential.name}</div>
                <div className="flex items-center gap-2">
                  <SettingSwitch
                    checked={credential.enabled}
                    onChange={(enabled) => updateCredential(credential.id, { enabled })}
                  />
                  {!builtin ? (
                    <DeleteIconButton
                      title={t("common.delete")}
                      onDelete={() => removeCredential(credential.id)}
                    />
                  ) : null}
                </div>
              </div>
              {builtin ? (
                <SettingFieldGrid>
                  <SettingInput
                    label={t("settings.apiKey")}
                    type="password"
                    placeholder={credential.api_key === "__SET__" ? "__SET__" : "sk-..."}
                    value={credential.api_key ?? ""}
                    onChange={(event) =>
                      updateCredential(credential.id, {
                        api_key: event.target.value,
                      })
                    }
                  />
                  {supportsApiFormatSelection(credential) ? (
                    <SettingSelect
                      label={t("ai.apiFormat")}
                      value={credential.api_format ?? "chat_completions"}
                      onValueChange={(api_format) =>
                        updateCredential(credential.id, {
                          api_format: api_format as AIApiFormat,
                        })
                      }
                    >
                      <SelectItem value="chat_completions">
                        {t("ai.apiFormatChatCompletions")}
                      </SelectItem>
                      <SelectItem value="responses">{t("ai.apiFormatResponses")}</SelectItem>
                    </SettingSelect>
                  ) : null}
                </SettingFieldGrid>
              ) : (
                <SettingFieldGrid>
                  <SettingInput
                    label={t("ai.profileName")}
                    value={credential.name}
                    onChange={(event) =>
                      updateCredential(credential.id, {
                        name: event.target.value,
                      })
                    }
                  />
                  <SettingSelect
                    label={t("ai.apiProtocol")}
                    value={credential.provider_kind}
                    onValueChange={(provider_kind) =>
                      updateCredential(credential.id, {
                        provider_kind: provider_kind as AIProviderKind,
                      })
                    }
                    triggerClassName="min-w-0 [&>span]:truncate"
                  >
                    {CUSTOM_AI_PROVIDER_PROTOCOLS.map((protocol) => (
                      <SelectItem key={protocol.value} value={protocol.value}>
                        {t(protocol.labelKey)}
                      </SelectItem>
                    ))}
                  </SettingSelect>
                  <SettingInput
                    label={t("ai.baseUrl")}
                    desc={t("ai.baseUrlRootHint")}
                    placeholder={getCustomProviderBaseUrlPlaceholder(credential.provider_kind)}
                    value={credential.base_url ?? ""}
                    onChange={(event) =>
                      updateCredential(credential.id, {
                        base_url: event.target.value,
                      })
                    }
                  />
                  {supportsApiFormatSelection(credential) ? (
                    <SettingSelect
                      label={t("ai.apiFormat")}
                      value={credential.api_format ?? "chat_completions"}
                      onValueChange={(api_format) =>
                        updateCredential(credential.id, {
                          api_format: api_format as AIApiFormat,
                        })
                      }
                    >
                      <SelectItem value="chat_completions">
                        {t("ai.apiFormatChatCompletions")}
                      </SelectItem>
                      <SelectItem value="responses">{t("ai.apiFormatResponses")}</SelectItem>
                    </SettingSelect>
                  ) : null}
                  <SettingInput
                    label={t("settings.apiKey")}
                    type="password"
                    placeholder={credential.api_key === "__SET__" ? "__SET__" : "sk-..."}
                    value={credential.api_key ?? ""}
                    onChange={(event) =>
                      updateCredential(credential.id, {
                        api_key: event.target.value,
                      })
                    }
                  />
                </SettingFieldGrid>
              )}
            </div>
          );
        })}
      </SettingSection>
    </div>
  );
}

function ActionListEditor({
  title,
  actions,
  onChange,
}: {
  title: string;
  actions: AICustomActionConfig[];
  onChange: (actions: AICustomActionConfig[]) => void;
}) {
  const { t } = useTranslation();

  const updateAction = (id: string, patch: Partial<AICustomActionConfig>) => {
    onChange(actions.map((action) => (action.id === id ? { ...action, ...patch } : action)));
  };

  return (
    <SettingSection
      title={title}
      action={
        <Button
          size="sm"
          variant="outline"
          onClick={() => onChange([...actions, newAction("ai-action")])}
        >
          <MdAdd />
          {t("common.add")}
        </Button>
      }
      contentClassName="space-y-4"
    >
      {actions.map((action) => (
        <div key={action.id} className="rounded-md border border-border/70 bg-background/75 p-4">
          <div className="mb-3 flex items-center justify-between gap-3">
            <div className="min-w-0 truncate text-sm font-medium">{action.name}</div>
            <div className="flex items-center gap-2">
              <SettingSwitch
                checked={action.enabled}
                onChange={(enabled) => updateAction(action.id, { enabled })}
              />
              <DeleteIconButton
                title={t("common.delete")}
                onDelete={() => onChange(actions.filter((item) => item.id !== action.id))}
              />
            </div>
          </div>
          <div className="space-y-3">
            <Input
              value={action.name}
              className="text-sm"
              placeholder={t("ai.actionName")}
              onChange={(event) => updateAction(action.id, { name: event.target.value })}
            />
            <Textarea
              value={action.prompt}
              className="min-h-20 resize-y text-sm"
              placeholder={t("ai.actionPrompt")}
              onChange={(event) => updateAction(action.id, { prompt: event.target.value })}
            />
          </div>
        </div>
      ))}
    </SettingSection>
  );
}

export function AiRulesTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const ai = appSettings.ai;
  const update = (patch: Partial<AISettings>) => updateAppSettings({ ai: { ...ai, ...patch } });
  const MB = 1024 * 1024;

  return (
    <div className="space-y-5">
      <SettingSection title={t("ai.rules")}>
        <SettingNumberInput
          label={`${t("ai.maxAiFileSize")} (MB)`}
          desc={t("ai.maxAiFileSizeDesc")}
          min={1}
          max={256}
          step={1}
          value={Math.max(1, Math.round(ai.max_ai_file_size_bytes / MB))}
          onChange={(value) => update({ max_ai_file_size_bytes: value * MB })}
        />
      </SettingSection>
      <ActionListEditor
        title={t("ai.terminalActions")}
        actions={ai.terminal_ai_actions}
        onChange={(terminal_ai_actions) => update({ terminal_ai_actions })}
      />
      <ActionListEditor
        title={t("ai.fileActions")}
        actions={ai.file_ai_actions}
        onChange={(file_ai_actions) => update({ file_ai_actions })}
      />
    </div>
  );
}

export function AiTab() {
  return <AiGeneralTab />;
}

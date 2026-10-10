import { randomUUID } from "@/lib/uuid";
import { runtime } from "@/lib/backend/runtime";
import { pickBrowserImage } from "@/lib/backend/browserArtifacts";
import { listen } from "@/lib/backend/api";
import { open as openDialog } from "@/lib/backend/platform/dialog";
import { openUrl } from "@/lib/backend/platform/opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdDelete,
  MdEdit,
  MdLogin,
  MdLogout,
  MdOpenInNew,
  MdRefresh,
  MdVisibility,
  MdVisibilityOff,
} from "react-icons/md";
import { TbPlugConnected } from "react-icons/tb";
import { toast } from "sonner";
import { ProviderBadge } from "@/components/ai/ProviderBadge";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useApp } from "@/context/AppContext";
import {
  AI_PROVIDERS,
  aiModelIdForCredential,
  aiModelIdForProvider,
  BUILTIN_PROVIDERS,
  DEFAULT_AI_SETTINGS,
  DEFAULT_MODEL_REASONING_EFFORTS,
  getCustomProviderBaseUrlPlaceholder,
  getProviderLabel,
  isBuiltinProvider,
  MODEL_REASONING_EFFORTS,
  supportsApiFormatSelection,
} from "@/lib/aiSettings";
import { writeClipboardText } from "@/lib/clipboard";
import { isCloudSecretMasked, secretInputValue, secretPlaceholder } from "@/lib/cloudSync";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { getOwnerMainWindowLabel } from "@/lib/windowManager";
import type {
  AICustomActionConfig,
  AIModelConfigItem,
  AIModelReasoningEffort,
  AIProviderApiProtocol,
  AIProviderCredential,
  AIProviderKind,
  AIProxySettings,
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
    id: `credential-${randomUUID()}`,
    name: "",
    provider_kind: "openai_compatible",
    api_protocol: null,
    api_format: "chat_completions",
    base_url: "",
    api_key: "",
    enabled: true,
  };
}

function providerNameTaken(
  name: string,
  credentials: AIProviderCredential[],
  excludeId?: string,
): boolean {
  const normalized = name.trim().toLocaleLowerCase();
  return (
    normalized.length > 0 &&
    credentials.some(
      (credential) =>
        credential.enabled &&
        credential.id !== excludeId &&
        credential.name.trim().toLocaleLowerCase() === normalized,
    )
  );
}

function availableProviderName(
  baseName: string,
  credentials: AIProviderCredential[],
  excludeId?: string,
): string {
  if (!providerNameTaken(baseName, credentials, excludeId)) return baseName;
  let suffix = 2;
  while (providerNameTaken(`${baseName}-${suffix}`, credentials, excludeId)) suffix += 1;
  return `${baseName}-${suffix}`;
}

function defaultProviderName(
  credential: AIProviderCredential,
  credentials: AIProviderCredential[],
): string {
  const baseName =
    credential.provider_kind === "openai_compatible"
      ? "my-provider"
      : getProviderLabel(credential.provider_kind);
  return availableProviderName(baseName, credentials, credential.id);
}

function providerApiProtocol(credential: AIProviderCredential): AIProviderApiProtocol {
  if (credential.api_protocol) return credential.api_protocol;
  switch (credential.provider_kind) {
    case "anthropic":
      return "anthropic";
    case "gemini":
      return "gemini";
    case "ollama":
      return "ollama";
    default:
      return "openai_compatible";
  }
}

function providerProtocolSelectValue(credential: AIProviderCredential): string {
  const protocol = providerApiProtocol(credential);
  if (protocol !== "openai_compatible") return protocol;
  return credential.api_format === "responses" ? "responses" : "chat_completions";
}

function newAction(prefix: string): AICustomActionConfig {
  return {
    id: `${prefix}-${randomUUID()}`,
    name: "自定义 AI 功能",
    prompt: "",
    enabled: true,
  };
}

function DeleteIconButton({
  onDelete,
  title,
  disabled,
}: {
  onDelete: () => void;
  title: string;
  disabled?: boolean;
}) {
  return (
    <Button
      type="button"
      size="icon-sm"
      variant="ghost"
      title={title}
      aria-label={title}
      disabled={disabled}
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
  const proxy = { ...DEFAULT_AI_SETTINGS.proxy, ...ai.proxy };
  const updateProxy = (patch: Partial<AIProxySettings>) =>
    update({ proxy: { ...proxy, ...patch } });

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

      <SettingSection title={t("ai.proxyTitle")} desc={t("ai.proxyDescription")}>
        <SettingSelect
          label={t("ai.proxyMode")}
          value={proxy.mode}
          onValueChange={(mode) => updateProxy({ mode: mode as AIProxySettings["mode"] })}
        >
          <SelectItem value="system">{t("ai.proxySystem")}</SelectItem>
          <SelectItem value="direct">{t("ai.proxyDirect")}</SelectItem>
          <SelectItem value="custom">{t("ai.proxyCustom")}</SelectItem>
        </SettingSelect>
        {proxy.mode === "custom" && (
          <>
            <SettingSelect
              label={t("settings.proxyProtocol")}
              value={proxy.protocol}
              onValueChange={(protocol) =>
                updateProxy({
                  protocol: protocol as AIProxySettings["protocol"],
                })
              }
            >
              <SelectItem value="http">HTTP</SelectItem>
              <SelectItem value="socks5">SOCKS5</SelectItem>
            </SettingSelect>
            <SettingFieldGrid>
              <SettingInput
                label={t("settings.proxyHost")}
                aria-label={t("settings.proxyHost")}
                placeholder="127.0.0.1"
                value={proxy.host}
                onChange={(event) => updateProxy({ host: event.target.value })}
              />
              <SettingNumberInput
                label={t("settings.proxyPort")}
                min={1}
                max={65535}
                value={proxy.port}
                onChange={(port) => updateProxy({ port })}
              />
              <SettingInput
                label={t("ai.proxyUsername")}
                aria-label={t("ai.proxyUsername")}
                autoComplete="off"
                value={proxy.username ?? ""}
                onChange={(event) => updateProxy({ username: event.target.value })}
              />
            </SettingFieldGrid>
            <SettingRow label={t("ai.proxyPassword")} desc={t("ai.proxyPasswordDescription")}>
              <Input
                type="password"
                aria-label={t("ai.proxyPassword")}
                autoComplete="new-password"
                value={secretInputValue(proxy.password)}
                placeholder={secretPlaceholder(proxy.password, t("ai.proxyPassword"))}
                onChange={(event) => updateProxy({ password: event.target.value })}
              />
              <Button
                type="button"
                variant="outline"
                disabled={!proxy.password}
                onClick={() => updateProxy({ password: "" })}
              >
                {t("ai.proxyClearPassword")}
              </Button>
            </SettingRow>
            <SettingInput
              label={t("ai.proxyBypass")}
              aria-label={t("ai.proxyBypass")}
              desc={t("ai.proxyBypassDescription")}
              value={proxy.no_proxy}
              onChange={(event) => updateProxy({ no_proxy: event.target.value })}
            />
          </>
        )}
        <p className="text-xs text-muted-foreground">{t("ai.proxyTestHint")}</p>
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
    terminal_display_mode: "inline",
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
          <SettingSelect
            label={t("ai.externalMcpTerminalDisplay")}
            value={externalMcp.terminal_display_mode ?? "inline"}
            onValueChange={(terminal_display_mode) =>
              updateExternalMcp({
                terminal_display_mode:
                  terminal_display_mode as ExternalMcpSettings["terminal_display_mode"],
              })
            }
          >
            <SelectItem value="inline">
              {t("ai.externalMcpTerminalInline")}
            </SelectItem>
            <SelectItem value="silent">
              {t("ai.externalMcpTerminalSilent")}
            </SelectItem>
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

function reconcileProviderModels(
  models: AIModelConfigItem[],
  credential: AIProviderCredential,
  names: string[],
): AIModelConfigItem[] {
  const builtin = isBuiltinProvider(credential.id);
  const groupKey = groupKeyForCredential(credential);
  const returnedNames = new Map<string, string>();
  for (const name of names) {
    const trimmed = name.trim();
    if (!trimmed) continue;
    const id = builtin
      ? aiModelIdForProvider(credential.provider_kind, trimmed)
      : aiModelIdForCredential(credential.id, trimmed);
    returnedNames.set(id, trimmed);
  }
  const now = new Date().toISOString();
  const nextModels = models.map((model) => {
    if (model.backend !== "genai" || (model.credential_id ?? model.provider_kind) !== groupKey) {
      return model;
    }
    return {
      ...model,
      last_seen_at: returnedNames.has(model.id) ? now : null,
    };
  });
  const existingIds = new Set(nextModels.map((model) => model.id));
  for (const [id, name] of returnedNames) {
    if (existingIds.has(id)) continue;
    nextModels.push({
      id,
      name,
      backend: "genai",
      provider_kind: credential.provider_kind,
      credential_id: builtin ? null : credential.id,
      enabled: false,
      source: "rust-genai",
      last_seen_at: now,
    });
  }
  return nextModels;
}

export function AiModelsTab() {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const ai = appSettings.ai;
  const [selectedProviderCredentialId, setSelectedProviderCredentialId] = useState<string | null>(
    null,
  );
  const [showProviderChoices, setShowProviderChoices] = useState(false);
  const [providerDraft, setProviderDraft] = useState<{
    credential: AIProviderCredential;
    isAutomatic?: boolean;
  } | null>(() =>
    ai.provider_credentials.some((credential) => credential.enabled)
      ? null
      : { credential: newCredential(), isAutomatic: true },
  );
  const [showProviderApiKey, setShowProviderApiKey] = useState(false);
  const [revealedProviderApiKey, setRevealedProviderApiKey] = useState<{
    credentialId: string;
    value: string;
  } | null>(null);
  const [revealingProviderApiKey, setRevealingProviderApiKey] = useState(false);
  const providerApiKeyRevealGeneration = useRef(0);
  const [providerNameInput, setProviderNameInput] = useState<{
    credentialId: string;
    value: string;
  } | null>(null);
  const [showProviderValidation, setShowProviderValidation] = useState(false);
  const [testingProviderId, setTestingProviderId] = useState<string | null>(null);
  const [providerModelCount, setProviderModelCount] = useState<number | null>(null);
  const [testedDraftModels, setTestedDraftModels] = useState<{
    credentialId: string;
    names: string[];
  } | null>(null);
  const [providerConnectionStatus, setProviderConnectionStatus] = useState<
    "idle" | "testing" | "success" | "error"
  >("idle");
  const [providerStatuses, setProviderStatuses] = useState<
    Record<string, "idle" | "testing" | "success" | "error">
  >({});
  const providerConnectionTestGeneration = useRef(0);
  const providerListRefreshGeneration = useRef(0);
  const [manualModelNames, setManualModelNames] = useState<Record<string, string>>({});
  const [editingModelId, setEditingModelId] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshingCurrentModels, setRefreshingCurrentModels] = useState(false);
  const [testingModelId, setTestingModelId] = useState<string | null>(null);
  const [modelTestResults, setModelTestResults] = useState<Record<string, "success" | "error">>({});
  const modelTestGeneration = useRef(0);
  const proxySignature = JSON.stringify(ai.proxy);
  const cancelProviderConnectionTest = () => {
    providerConnectionTestGeneration.current += 1;
    setTestingProviderId(null);
    setRefreshingCurrentModels(false);
    if (testingProviderId) {
      setProviderStatuses((current) =>
        current[testingProviderId] === "testing"
          ? { ...current, [testingProviderId]: "idle" }
          : current,
      );
    }
  };
  const invalidateProviderListRefresh = () => {
    providerListRefreshGeneration.current += 1;
    setRefreshing(false);
    setProviderStatuses((current) => {
      const next = { ...current };
      for (const id of Object.keys(next)) {
        if (next[id] === "testing") next[id] = "idle";
      }
      return next;
    });
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: Configuration changes invalidate pending model tests and their results.
  useEffect(() => {
    modelTestGeneration.current += 1;
    setTestingModelId(null);
    setModelTestResults({});
  }, [
    ai.models,
    ai.provider_credentials,
    ai.default_reasoning_effort,
    ai.request_user_agent,
    ai.timeout_ms,
    proxySignature,
  ]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Proxy changes invalidate all pending provider requests and results.
  useEffect(() => {
    providerConnectionTestGeneration.current += 1;
    providerListRefreshGeneration.current += 1;
    setTestingProviderId(null);
    setRefreshingCurrentModels(false);
    setRefreshing(false);
    setProviderStatuses({});
    setProviderConnectionStatus("idle");
    setProviderModelCount(null);
    setTestedDraftModels(null);
  }, [proxySignature]);
  const update = (patch: Partial<AISettings>) => updateAppSettings({ ai: { ...ai, ...patch } });

  const enabledCredentials = useMemo(
    () => ai.provider_credentials.filter((credential) => credential.enabled),
    [ai.provider_credentials],
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: Adding or removing an account closes the preset picker.
  useEffect(() => {
    setShowProviderChoices(false);
  }, [enabledCredentials.length]);
  useEffect(() => {
    if (enabledCredentials.length === 0 && !providerDraft) {
      setProviderDraft({ credential: newCredential(), isAutomatic: true });
    } else if (enabledCredentials.length > 0 && providerDraft?.isAutomatic) {
      setProviderDraft(null);
    }
  }, [enabledCredentials.length, providerDraft]);
  const selectedProviderCredential =
    providerDraft?.credential ??
    enabledCredentials.find((credential) => credential.id === selectedProviderCredentialId) ??
    enabledCredentials[0];
  const selectedProviderConnectionStatus =
    selectedProviderCredential && !providerDraft
      ? (providerStatuses[selectedProviderCredential.id] ?? "idle")
      : providerConnectionStatus;
  const selectedProviderName =
    providerNameInput &&
    selectedProviderCredential &&
    providerNameInput.credentialId === selectedProviderCredential.id
      ? providerNameInput.value
      : (selectedProviderCredential?.name ?? "");
  const selectedProviderDefaultName = selectedProviderCredential
    ? defaultProviderName(selectedProviderCredential, enabledCredentials)
    : "my-provider";
  const selectedProviderTitle = selectedProviderName.trim() || selectedProviderDefaultName;
  const selectedProviderNameTaken = providerNameTaken(
    selectedProviderName,
    enabledCredentials,
    selectedProviderCredential?.id,
  );
  const selectedProviderEndpointMissing = !selectedProviderCredential?.base_url?.trim();
  const selectedProviderApiKeyRequired =
    !!selectedProviderCredential &&
    selectedProviderCredential.provider_kind !== "ollama" &&
    selectedProviderCredential.provider_kind !== "openai_compatible";
  const selectedProviderApiKeyMissing =
    selectedProviderApiKeyRequired && !selectedProviderCredential?.api_key?.trim();
  // biome-ignore lint/correctness/useExhaustiveDependencies: These transient fields belong to the selected account.
  useEffect(() => {
    setProviderNameInput(null);
    setShowProviderValidation(false);
    setEditingModelId(null);
  }, [selectedProviderCredential?.id]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Switching accounts must hide and discard the previously revealed secret.
  useEffect(() => {
    providerApiKeyRevealGeneration.current += 1;
    setShowProviderApiKey(false);
    setRevealedProviderApiKey(null);
    setRevealingProviderApiKey(false);
  }, [selectedProviderCredential?.id]);

  const hideProviderApiKey = () => {
    providerApiKeyRevealGeneration.current += 1;
    setShowProviderApiKey(false);
    setRevealedProviderApiKey(null);
    setRevealingProviderApiKey(false);
  };

  const toggleProviderApiKeyVisibility = async () => {
    if (!selectedProviderCredential) return;
    if (showProviderApiKey) {
      hideProviderApiKey();
      return;
    }
    if (!isCloudSecretMasked(selectedProviderCredential.api_key)) {
      setShowProviderApiKey(true);
      return;
    }

    const credentialId = selectedProviderCredential.id;
    const generation = ++providerApiKeyRevealGeneration.current;
    setRevealingProviderApiKey(true);
    try {
      const value = await invoke<string>("reveal_ai_provider_api_key", {
        credentialId,
      });
      if (providerApiKeyRevealGeneration.current !== generation) return;
      setRevealedProviderApiKey({ credentialId, value });
      setShowProviderApiKey(true);
    } catch (error) {
      if (providerApiKeyRevealGeneration.current === generation) {
        toast.error(getErrorMessage(error));
      }
    } finally {
      if (providerApiKeyRevealGeneration.current === generation) {
        setRevealingProviderApiKey(false);
      }
    }
  };

  const visibleModels = useMemo(() => {
    if (!selectedProviderCredential) return [];
    if (providerDraft) {
      return testedDraftModels?.credentialId === selectedProviderCredential.id
        ? reconcileProviderModels([], selectedProviderCredential, testedDraftModels.names)
        : [];
    }
    const groupKey = groupKeyForCredential(selectedProviderCredential);
    return ai.models.filter(
      (model) =>
        model.backend !== "codex" && (model.credential_id ?? model.provider_kind) === groupKey,
    );
  }, [ai.models, selectedProviderCredential, providerDraft, testedDraftModels]);

  const hasEnabledModel = ai.models.some(
    (model) =>
      model.enabled &&
      (model.backend === "codex"
        ? ai.codex.enabled
        : enabledCredentials.some(
            (credential) =>
              groupKeyForCredential(credential) === (model.credential_id ?? model.provider_kind),
          )),
  );

  const rawGroupedModels = useMemo(
    () =>
      groupModels(visibleModels, selectedProviderCredential ? [selectedProviderCredential] : []),
    [visibleModels, selectedProviderCredential],
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

  const updateModels = (models: AIModelConfigItem[]) => {
    update({ models, default_model_id: updateDefaultModelId(ai, models) });
  };

  const refreshProviders = async () => {
    if (refreshing || enabledCredentials.length === 0) return;
    const generation = ++providerListRefreshGeneration.current;
    setRefreshing(true);
    setProviderModelCount(null);
    setProviderStatuses((current) => {
      const next = { ...current };
      for (const credential of enabledCredentials) next[credential.id] = "testing";
      return next;
    });
    try {
      const results = await Promise.all(
        enabledCredentials.map(async (credential) => {
          try {
            const names = await invoke<string[]>("test_ai_provider_connection", {
              aiSettings: ai,
              credentialId: credential.id,
            });
            if (providerListRefreshGeneration.current === generation) {
              setProviderStatuses((current) => ({
                ...current,
                [credential.id]: "success",
              }));
            }
            return { credential, names };
          } catch {
            if (providerListRefreshGeneration.current === generation) {
              setProviderStatuses((current) => ({
                ...current,
                [credential.id]: "error",
              }));
            }
            return null;
          }
        }),
      );
      if (providerListRefreshGeneration.current !== generation) return;
      const successes = results.filter(
        (result): result is NonNullable<typeof result> => result !== null,
      );
      if (successes.length > 0) {
        updateAppSettings((current) => {
          let models = current.ai.models;
          for (const { credential, names } of successes) {
            if (
              !current.ai.provider_credentials.some(
                (item) => item.id === credential.id && item.enabled,
              )
            ) {
              continue;
            }
            models = reconcileProviderModels(models, credential, names);
          }
          return {
            ai: {
              ...current.ai,
              models,
              default_model_id: updateDefaultModelId(current.ai, models),
            },
          };
        });
      }
    } finally {
      if (providerListRefreshGeneration.current === generation) setRefreshing(false);
    }
  };

  const testModel = async (model: AIModelConfigItem) => {
    if (testingModelId || model.backend !== "genai") return;
    const generation = ++modelTestGeneration.current;
    setTestingModelId(model.id);
    setModelTestResults((previous) => {
      const next = { ...previous };
      delete next[model.id];
      return next;
    });
    try {
      await invoke("test_ai_model_connection", {
        aiSettings: ai,
        modelId: model.id,
      });
      if (modelTestGeneration.current !== generation) return;
      setModelTestResults((previous) => ({
        ...previous,
        [model.id]: "success",
      }));
      toast.success(t("ai.modelTestSucceeded", { model: model.name }));
    } catch (error) {
      if (modelTestGeneration.current !== generation) return;
      setModelTestResults((previous) => ({ ...previous, [model.id]: "error" }));
      toast.error(t("ai.modelTestFailed", { model: model.name }), {
        description: getErrorMessage(error),
      });
    } finally {
      if (modelTestGeneration.current === generation) setTestingModelId(null);
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

  const toggleModelReasoningEffort = (model: AIModelConfigItem, effort: AIModelReasoningEffort) => {
    const selected = new Set(model.supported_reasoning_efforts ?? DEFAULT_MODEL_REASONING_EFFORTS);
    if (selected.has(effort)) selected.delete(effort);
    else selected.add(effort);
    updateModel(model.id, {
      supported_reasoning_efforts: MODEL_REASONING_EFFORTS.filter((value) => selected.has(value)),
    });
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

  const removeModel = (id: string) => {
    const model = ai.models.find((item) => item.id === id);
    if (!model) return;
    setEditingModelId((current) => (current === id ? null : current));
    const models = ai.models.filter((item) => item.id !== id);
    update({
      models,
      default_model_id: updateDefaultModelId(ai, models),
    });
    toast.success(t("ai.manualModelDeleted", { model: model.name }));
  };

  const updateCredential = (id: string, patch: Partial<AIProviderCredential>) => {
    if (typeof patch.name === "string" && providerNameTaken(patch.name, enabledCredentials, id)) {
      return;
    }
    invalidateProviderListRefresh();
    setProviderStatuses((current) => ({ ...current, [id]: "idle" }));
    cancelProviderConnectionTest();
    setProviderConnectionStatus("idle");
    setProviderModelCount(null);
    if (patch.provider_kind) {
      patch = { ...patch, api_protocol: null };
    }
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

  const updateSelectedProviderCredential = (id: string, patch: Partial<AIProviderCredential>) => {
    if (providerDraft?.credential.id !== id) {
      updateCredential(id, patch);
      return;
    }

    cancelProviderConnectionTest();
    setProviderConnectionStatus("idle");
    setProviderModelCount(null);
    if (patch.provider_kind) {
      patch = { ...patch, api_protocol: null };
    }
    if (
      patch.provider_kind &&
      !supportsApiFormatSelection({ provider_kind: patch.provider_kind })
    ) {
      patch = { ...patch, api_format: "chat_completions" };
    }
    if (
      "base_url" in patch ||
      "api_key" in patch ||
      "api_protocol" in patch ||
      "api_format" in patch
    ) {
      setTestedDraftModels(null);
    }
    setProviderDraft((current) =>
      current?.credential.id === id
        ? {
            ...current,
            isAutomatic: false,
            credential: { ...current.credential, ...patch },
          }
        : current,
    );
  };

  const changeCustomProviderIcon = async () => {
    const credential = selectedProviderCredential;
    if (!credential || credential.provider_kind !== "openai_compatible") return;

    try {
      if (runtime === "web") {
        const image = await pickBrowserImage();
        if (image)
          updateSelectedProviderCredential(credential.id, {
            icon_data_url: image.dataUrl,
          });
        return;
      }
      const selectedPath = await openDialog({
        directory: false,
        multiple: false,
        filters: [
          {
            name: t("dialog.connectionIconFiles"),
            extensions: ["png", "jpg", "jpeg", "webp", "bmp", "gif"],
          },
        ],
        title: t("ai.changeProviderIcon"),
      });
      if (typeof selectedPath !== "string" || !selectedPath) return;

      const iconDataUrl = await invoke<string>("import_ai_provider_icon", {
        path: selectedPath,
      });
      updateSelectedProviderCredential(credential.id, {
        icon_data_url: iconDataUrl,
      });
    } catch (error) {
      toast.error(getErrorMessage(error));
    }
  };

  const changeSelectedProviderName = (value: string) => {
    if (!selectedProviderCredential) return;
    if (providerDraft) {
      updateSelectedProviderCredential(selectedProviderCredential.id, {
        name: value,
      });
      return;
    }
    setProviderNameInput({
      credentialId: selectedProviderCredential.id,
      value,
    });
    if (
      value.trim() &&
      !providerNameTaken(value, enabledCredentials, selectedProviderCredential.id)
    ) {
      updateSelectedProviderCredential(selectedProviderCredential.id, {
        name: value,
      });
    }
  };

  const finishSelectedProviderName = () => {
    if (!selectedProviderCredential || providerDraft) return;
    const value =
      providerNameInput?.credentialId === selectedProviderCredential.id
        ? providerNameInput.value
        : selectedProviderCredential.name;
    if (providerNameTaken(value, enabledCredentials, selectedProviderCredential.id)) {
      toast.error(t("ai.providerNameDuplicate"));
    } else if (!value.trim()) {
      updateSelectedProviderCredential(selectedProviderCredential.id, {
        name: selectedProviderDefaultName,
      });
    }
    setProviderNameInput(null);
  };

  const updateSelectedProviderProtocol = (credential: AIProviderCredential, value: string) => {
    const previousProtocol = providerApiProtocol(credential);
    const api_protocol: AIProviderApiProtocol =
      value === "chat_completions" || value === "responses"
        ? "openai_compatible"
        : (value as AIProviderApiProtocol);
    const patch: Partial<AIProviderCredential> = {
      api_protocol,
      api_format: value === "responses" ? "responses" : "chat_completions",
    };
    if (!credential.base_url?.trim()) {
      patch.base_url =
        BUILTIN_PROVIDERS[credential.provider_kind]?.defaultBaseUrl ?? credential.base_url;
    }
    if (credential.provider_kind === "ollama") {
      const baseUrl = credential.base_url?.trim();
      if (previousProtocol === "ollama" && api_protocol === "openai_compatible") {
        if (baseUrl === "http://localhost:11434/") {
          patch.base_url = "http://localhost:11434/v1/";
        }
      } else if (previousProtocol === "openai_compatible" && api_protocol === "ollama") {
        if (baseUrl === "http://localhost:11434/v1/") {
          patch.base_url = "http://localhost:11434/";
        }
      }
    }
    updateSelectedProviderCredential(credential.id, patch);
  };

  const selectProviderPreset = (providerKind: AIProviderKind) => {
    cancelProviderConnectionTest();
    setProviderModelCount(null);
    setProviderConnectionStatus("idle");
    setTestedDraftModels(null);
    hideProviderApiKey();
    const providerInfo = BUILTIN_PROVIDERS[providerKind];
    const providerLabel = getProviderLabel(providerKind);
    const existingBuiltin = providerInfo
      ? ai.provider_credentials.find((credential) => credential.id === providerKind)
      : undefined;
    let credential: AIProviderCredential;
    if (providerInfo && existingBuiltin && !existingBuiltin.enabled) {
      credential = {
        ...existingBuiltin,
        name: availableProviderName(
          existingBuiltin.name.trim() || providerLabel,
          enabledCredentials,
        ),
        api_protocol: null,
        api_format: "chat_completions",
        base_url: providerInfo.defaultBaseUrl,
        enabled: true,
      };
    } else {
      credential = {
        ...newCredential(),
        id: providerInfo && !existingBuiltin ? providerKind : `credential-${randomUUID()}`,
        name: availableProviderName(providerLabel, enabledCredentials),
        provider_kind: providerKind,
        base_url: providerInfo ? providerInfo.defaultBaseUrl : "",
      };
    }
    addProviderCredential(credential);
  };

  const stageCustomProvider = () => {
    cancelProviderConnectionTest();
    setProviderModelCount(null);
    setProviderConnectionStatus("idle");
    setTestedDraftModels(null);
    hideProviderApiKey();
    setProviderDraft({ credential: newCredential() });
  };

  const addProviderCredential = (draftCredential: AIProviderCredential) => {
    const credentialId = draftCredential.id;
    const name = availableProviderName(
      draftCredential.name.trim() || defaultProviderName(draftCredential, enabledCredentials),
      enabledCredentials,
      credentialId,
    );
    invalidateProviderListRefresh();
    const credential = {
      ...draftCredential,
      id: credentialId,
      name,
      base_url: draftCredential.base_url?.trim() ?? "",
      api_key: draftCredential.api_key?.trim() ?? "",
      enabled: true,
    };
    const existingIndex = ai.provider_credentials.findIndex((item) => item.id === credentialId);
    const providerCredentials = [...ai.provider_credentials];
    if (existingIndex >= 0) providerCredentials[existingIndex] = credential;
    else providerCredentials.unshift(credential);

    let models = [...ai.models];
    const providerInfo = BUILTIN_PROVIDERS[credential.provider_kind];
    if (providerInfo) {
      const existingModelIds = new Set(models.map((model) => model.id));
      for (const name of providerInfo.models) {
        const id =
          credentialId === credential.provider_kind
            ? aiModelIdForProvider(credential.provider_kind, name)
            : aiModelIdForCredential(credentialId, name);
        if (existingModelIds.has(id)) continue;
        models.push({
          id,
          name,
          backend: "genai",
          provider_kind: credential.provider_kind,
          credential_id: credentialId === credential.provider_kind ? null : credentialId,
          enabled: false,
          source: "rust-genai",
          last_seen_at: null,
        });
      }
    }
    if (testedDraftModels?.credentialId === draftCredential.id) {
      models = reconcileProviderModels(models, credential, testedDraftModels.names);
    }

    update({
      provider_credentials: providerCredentials,
      models,
      default_model_id: updateDefaultModelId(ai, models),
    });
    setProviderDraft(null);
    setProviderStatuses((current) => ({
      ...current,
      [credentialId]:
        providerDraft?.credential.id === credentialId && providerConnectionStatus === "success"
          ? "success"
          : "idle",
    }));
    setTestedDraftModels(null);
    setSelectedProviderCredentialId(credentialId);
    setProviderNameInput(null);
    setShowProviderValidation(false);
    setShowProviderChoices(false);
    setProviderConnectionStatus("idle");
    setProviderModelCount(null);
    cancelProviderConnectionTest();
  };

  const addCustomProvider = () => {
    const credential =
      providerDraft?.credential.provider_kind === "openai_compatible"
        ? providerDraft.credential
        : newCredential();
    addProviderCredential(credential);
  };

  const removeCredential = (id: string) => {
    invalidateProviderListRefresh();
    setProviderStatuses((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
    const models = ai.models.filter((model) => model.credential_id !== id);
    update({
      provider_credentials: ai.provider_credentials.filter((credential) => credential.id !== id),
      models,
      default_model_id: updateDefaultModelId(ai, models),
    });
  };

  const deleteSelectedProvider = () => {
    if (!selectedProviderCredential) return;
    if (providerDraft) {
      setProviderDraft(null);
      setTestedDraftModels(null);
    } else {
      const id = selectedProviderCredential.id;
      const nextCredential = enabledCredentials.find((credential) => credential.id !== id);
      if (isBuiltinProvider(id)) {
        updateCredential(id, { enabled: false });
      } else {
        removeCredential(id);
      }
      setSelectedProviderCredentialId(nextCredential?.id ?? null);
    }
    cancelProviderConnectionTest();
    setProviderModelCount(null);
    setProviderConnectionStatus("idle");
    hideProviderApiKey();
  };

  const testProviderConnection = async (fromModelList = false) => {
    if (!selectedProviderCredential) return;
    const credentialId = selectedProviderCredential.id;
    const requestGeneration = ++providerConnectionTestGeneration.current;
    setTestingProviderId(credentialId);
    if (fromModelList) setRefreshingCurrentModels(true);
    setProviderModelCount(null);
    setProviderConnectionStatus("testing");
    if (!providerDraft) {
      setProviderStatuses((current) => ({
        ...current,
        [credentialId]: "testing",
      }));
    }
    try {
      const aiSettings = providerDraft
        ? {
            ...ai,
            provider_credentials: [...ai.provider_credentials, selectedProviderCredential],
          }
        : ai;
      const modelNames = await invoke<string[]>("test_ai_provider_connection", {
        aiSettings,
        credentialId,
      });
      if (providerConnectionTestGeneration.current === requestGeneration) {
        setProviderModelCount(modelNames.length);
        setProviderConnectionStatus("success");
        if (!providerDraft) {
          setProviderStatuses((current) => ({
            ...current,
            [credentialId]: "success",
          }));
        }
        if (providerDraft) {
          setTestedDraftModels({ credentialId, names: modelNames });
        } else {
          updateAppSettings((current) => {
            const models = reconcileProviderModels(
              current.ai.models,
              selectedProviderCredential,
              modelNames,
            );
            return {
              ai: {
                ...current.ai,
                models,
                default_model_id: updateDefaultModelId(current.ai, models),
              },
            };
          });
        }
        toast.success(t(fromModelList ? "ai.modelsRefreshed" : "ai.connectionTestSucceeded"));
      }
    } catch (error) {
      if (providerConnectionTestGeneration.current === requestGeneration) {
        setProviderConnectionStatus("error");
        if (!providerDraft) {
          setProviderStatuses((current) => ({
            ...current,
            [credentialId]: "error",
          }));
        }
        toast.error(getErrorMessage(error));
      }
    } finally {
      if (providerConnectionTestGeneration.current === requestGeneration) {
        if (fromModelList) setRefreshingCurrentModels(false);
        setTestingProviderId(null);
      }
    }
  };

  const protocolOptions = (credential: AIProviderCredential) => (
    <>
      <SelectItem value="chat_completions">{t("ai.providerProtocolChatCompletions")}</SelectItem>
      <SelectItem value="responses">{t("ai.providerProtocolResponses")}</SelectItem>
      <SelectItem value="anthropic">{t("ai.providerProtocolAnthropic")}</SelectItem>
      {(credential.provider_kind === "gemini" ||
        credential.provider_kind === "openai_compatible") && (
        <SelectItem value="gemini">{t("ai.providerProtocolGemini")}</SelectItem>
      )}
      {credential.provider_kind === "ollama" && (
        <SelectItem value="ollama">{t("ai.providerProtocolOllama")}</SelectItem>
      )}
    </>
  );

  const renderModelListCard = () => (
    <SettingSection
      title={t("ai.modelList")}
      headerRowClassName="flex-row items-center justify-between sm:items-center"
      action={
        <Button
          type="button"
          size="icon-sm"
          variant="outline"
          disabled={
            refreshing ||
            refreshingCurrentModels ||
            testingProviderId !== null ||
            !selectedProviderCredential ||
            selectedProviderEndpointMissing ||
            selectedProviderApiKeyMissing
          }
          onClick={() => void testProviderConnection(true)}
          title={t("ai.refreshModels")}
          aria-label={t("ai.refreshModels")}
        >
          <MdRefresh className={refreshingCurrentModels ? "animate-spin" : ""} />
        </Button>
      }
    >
      <div>
        {groupedModels.length === 0 ? (
          <div className="px-3 py-8 text-center text-xs text-muted-foreground">
            {t("ai.noModels")}
          </div>
        ) : (
          groupedModels.map((group) => (
            <div key={group.groupKey}>
              {group.credential && !providerDraft ? (
                <div className="border-b border-border/60 px-3 py-2">
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
                </div>
              ) : null}
              {group.models.map((model) => (
                <div key={model.id} className="border-b border-border/60 last:border-b-0">
                  <div className="flex items-center gap-3 px-3 py-2">
                    <div className="flex min-w-0 flex-1 items-center gap-2">
                      <div className="min-w-0 truncate text-xs">{model.name}</div>
                      {model.backend === "genai" && model.last_seen_at ? (
                        <Badge
                          variant="outline"
                          className="h-5 shrink-0 border-primary/40 bg-primary/10 px-1.5 text-[0.625rem] font-normal text-primary"
                        >
                          {t("ai.providerReturnedBadge")}
                        </Badge>
                      ) : null}
                      {model.source === "manual" ? (
                        <Badge
                          variant="outline"
                          className="h-5 border-border/70 px-1.5 text-[0.625rem] font-normal text-muted-foreground"
                        >
                          {t("ai.manualModelBadge")}
                        </Badge>
                      ) : null}
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      <Button
                        type="button"
                        size="icon-sm"
                        variant="ghost"
                        title={t(
                          model.backend === "codex"
                            ? "ai.modelTestUnsupported"
                            : modelTestResults[model.id] === "success"
                              ? "ai.modelTestSucceededShort"
                              : modelTestResults[model.id] === "error"
                                ? "ai.modelTestFailedShort"
                                : "ai.testModel",
                        )}
                        aria-label={t("ai.testModel")}
                        className={
                          modelTestResults[model.id] === "success"
                            ? "text-emerald-500"
                            : modelTestResults[model.id] === "error"
                              ? "text-destructive"
                              : undefined
                        }
                        disabled={
                          !!providerDraft || model.backend !== "genai" || testingModelId !== null
                        }
                        onClick={() => void testModel(model)}
                      >
                        <TbPlugConnected
                          className={`text-[0.95rem] ${
                            testingModelId === model.id ? "animate-pulse" : ""
                          }`}
                        />
                      </Button>
                      <Button
                        type="button"
                        size="icon-sm"
                        variant="ghost"
                        title={t("ai.editModelConfig")}
                        aria-label={t("ai.editModelConfig")}
                        aria-expanded={editingModelId === model.id}
                        aria-controls={`model-config-${encodeURIComponent(model.id)}`}
                        disabled={!!providerDraft}
                        onClick={() =>
                          setEditingModelId((current) => (current === model.id ? null : model.id))
                        }
                      >
                        <MdEdit className="text-[0.95rem]" />
                      </Button>
                      <DeleteIconButton
                        title={t("ai.deleteModel")}
                        onDelete={() => removeModel(model.id)}
                        disabled={!!providerDraft}
                      />
                    </div>
                    <SettingSwitch
                      checked={model.enabled}
                      disabled={!!providerDraft}
                      onChange={(enabled) => updateModel(model.id, { enabled })}
                    />
                  </div>
                  {editingModelId === model.id ? (
                    <div
                      id={`model-config-${encodeURIComponent(model.id)}`}
                      className="border-t border-border/60 bg-muted/10 px-3 py-3"
                    >
                      <div className="text-xs font-medium">{t("ai.supportedReasoningEfforts")}</div>
                      <fieldset
                        aria-label={t("ai.supportedReasoningEfforts")}
                        className="mt-2 flex flex-wrap gap-2"
                      >
                        {MODEL_REASONING_EFFORTS.map((effort) => {
                          const selected = (
                            model.supported_reasoning_efforts ?? DEFAULT_MODEL_REASONING_EFFORTS
                          ).includes(effort);
                          return (
                            <Button
                              key={effort}
                              type="button"
                              size="sm"
                              variant="outline"
                              aria-pressed={!!selected}
                              className={`h-7 px-2.5 text-xs font-normal ${
                                selected ? "border-primary bg-primary/10 text-primary" : ""
                              }`}
                              onClick={() => toggleModelReasoningEffort(model, effort)}
                            >
                              {effort}
                            </Button>
                          );
                        })}
                      </fieldset>
                    </div>
                  ) : null}
                </div>
              ))}
            </div>
          ))
        )}
      </div>
      {enabledCredentials.length === 0 ? (
        <div className="text-xs text-muted-foreground">{t("ai.manualModelNoProvider")}</div>
      ) : null}
      {!hasEnabledModel ? (
        <div className="text-xs text-amber-600">{t("ai.enableOneModelHint")}</div>
      ) : null}
    </SettingSection>
  );

  return (
    <div className="space-y-5">
      <SettingSection
        title={t("ai.providerList")}
        headerRowClassName="flex-row flex-wrap items-center justify-between sm:items-center"
        contentClassName={
          enabledCredentials.length === 0 && !showProviderChoices ? "hidden" : undefined
        }
        action={
          <div className="flex items-center gap-2">
            <Button
              type="button"
              size="icon-sm"
              variant="outline"
              className="rounded-lg border-border/70"
              disabled={refreshing || testingProviderId !== null || enabledCredentials.length === 0}
              onClick={() => void refreshProviders()}
              title={t("ai.refreshProviders")}
              aria-label={t("ai.refreshProviders")}
            >
              <MdRefresh className={refreshing ? "animate-spin" : ""} />
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="rounded-lg border-border/70"
              aria-expanded={showProviderChoices}
              onClick={() => {
                setShowProviderChoices(true);
                stageCustomProvider();
              }}
            >
              <MdAdd />
              {t("ai.addProvider")}
            </Button>
          </div>
        }
      >
        <div className="space-y-3">
          <div className="flex min-w-0 flex-wrap items-center gap-2 px-0.5 py-1">
            {enabledCredentials.map((credential) => (
              <Button
                key={credential.id}
                type="button"
                size="default"
                variant="outline"
                className={`h-10 min-w-0 max-w-full rounded-lg px-3 text-sm font-normal ${
                  selectedProviderCredential?.id === credential.id
                    ? "border-primary bg-primary/5 ring-1 ring-primary/20"
                    : "border-border/70"
                }`}
                aria-pressed={selectedProviderCredential?.id === credential.id}
                title={credential.name.trim() || getProviderLabel(credential.provider_kind)}
                onClick={() => {
                  cancelProviderConnectionTest();
                  setProviderDraft(null);
                  setSelectedProviderCredentialId(credential.id);
                  setShowProviderChoices(false);
                  hideProviderApiKey();
                  setProviderModelCount(null);
                  setProviderConnectionStatus("idle");
                }}
              >
                <ProviderBadge
                  kind={credential.provider_kind}
                  iconDataUrl={credential.icon_data_url}
                />
                <span className="min-w-0 max-w-40 truncate">
                  {credential.name.trim() || getProviderLabel(credential.provider_kind)}
                </span>
                <span
                  role="img"
                  className={`size-2.5 shrink-0 rounded-full ${
                    providerStatuses[credential.id] === "testing"
                      ? "animate-pulse bg-primary"
                      : providerStatuses[credential.id] === "success"
                        ? "bg-emerald-500"
                        : providerStatuses[credential.id] === "error"
                          ? "bg-destructive"
                          : "bg-muted-foreground/30"
                  }`}
                  title={t(`ai.connectionStatus.${providerStatuses[credential.id] ?? "idle"}`)}
                  aria-label={t(`ai.connectionStatus.${providerStatuses[credential.id] ?? "idle"}`)}
                />
              </Button>
            ))}
          </div>
          {showProviderChoices ? (
            <div className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-3">
              {AI_PROVIDERS.filter((provider) => BUILTIN_PROVIDERS[provider.value]).map(
                (provider) => (
                  <Button
                    key={provider.value}
                    type="button"
                    size="default"
                    variant="outline"
                    className={`h-10 rounded-lg border-border/70 px-3 text-sm font-normal ${
                      providerDraft?.credential.provider_kind === provider.value
                        ? "border-primary bg-primary/5 ring-1 ring-primary/20"
                        : ""
                    }`}
                    aria-pressed={providerDraft?.credential.provider_kind === provider.value}
                    onClick={() => selectProviderPreset(provider.value)}
                  >
                    <ProviderBadge kind={provider.value} />
                    {provider.label}
                  </Button>
                ),
              )}
              <Button
                type="button"
                size="default"
                variant="outline"
                className={`h-10 rounded-lg border-border/70 px-3 text-sm font-normal ${
                  providerDraft?.credential.provider_kind === "openai_compatible"
                    ? "border-primary bg-primary/5 ring-1 ring-primary/20"
                    : ""
                }`}
                aria-pressed={providerDraft?.credential.provider_kind === "openai_compatible"}
                onClick={addCustomProvider}
              >
                <MdAdd />
                {t("ai.customProviderOption")}
              </Button>
            </div>
          ) : null}
        </div>
      </SettingSection>

      {enabledCredentials.length > 0 ? (
        <SettingSection
          title={
            selectedProviderCredential ? (
              <span className="flex min-w-0 items-center gap-2">
                {selectedProviderCredential.provider_kind === "openai_compatible" ? (
                  <button
                    type="button"
                    className="group relative size-6 shrink-0 cursor-pointer rounded-full focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    onClick={() => void changeCustomProviderIcon()}
                    title={t("ai.changeProviderIcon")}
                    aria-label={t("ai.changeProviderIcon")}
                  >
                    <ProviderBadge
                      kind={selectedProviderCredential.provider_kind}
                      iconDataUrl={selectedProviderCredential.icon_data_url}
                    />
                    <span className="absolute inset-0 grid place-items-center rounded-full bg-background/80 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100">
                      <MdEdit className="size-3.5" />
                    </span>
                  </button>
                ) : (
                  <ProviderBadge
                    kind={selectedProviderCredential.provider_kind}
                    iconDataUrl={selectedProviderCredential.icon_data_url}
                  />
                )}
                <span className="truncate">{selectedProviderTitle}</span>
              </span>
            ) : undefined
          }
          className="min-h-24"
          headerRowClassName="sm:items-center"
          contentClassName="space-y-0 px-4 py-2 sm:px-5"
          action={
            selectedProviderCredential ? (
              <div className="flex items-center gap-2">
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <Button
                      type="button"
                      size="icon-sm"
                      variant="outline"
                      title={t("ai.deleteProvider")}
                      aria-label={t("ai.deleteProvider")}
                      className="text-destructive hover:text-destructive"
                    >
                      <MdDelete />
                    </Button>
                  </AlertDialogTrigger>
                  <AlertDialogContent size="sm">
                    <AlertDialogHeader>
                      <AlertDialogTitle>{t("ai.deleteProvider")}</AlertDialogTitle>
                      <AlertDialogDescription>
                        {t(
                          providerDraft ? "ai.discardProviderConfirm" : "ai.deleteProviderConfirm",
                          { name: selectedProviderTitle },
                        )}
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
                      <AlertDialogAction variant="destructive" onClick={deleteSelectedProvider}>
                        {t("common.delete")}
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              </div>
            ) : undefined
          }
        >
          {selectedProviderCredential ? (
            <div>
              <div className="grid gap-2 border-b border-border/60 py-2.5 first:pt-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,2.2fr)] sm:items-center">
                <Label className="text-xs font-normal">{t("ai.profileName")}</Label>
                <div className="space-y-1">
                  <Input
                    value={selectedProviderName}
                    placeholder={selectedProviderDefaultName}
                    className="h-9 rounded-lg text-sm"
                    aria-invalid={selectedProviderNameTaken}
                    onChange={(event) => changeSelectedProviderName(event.target.value)}
                    onBlur={finishSelectedProviderName}
                  />
                  {selectedProviderNameTaken ? (
                    <p className="text-xs text-destructive">{t("ai.providerNameDuplicate")}</p>
                  ) : null}
                </div>
              </div>
              <div className="grid gap-2 border-b border-border/60 py-2.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,2.2fr)] sm:items-center">
                <Label className="text-xs font-normal">
                  {t("ai.providerEndpoint")} <span aria-hidden="true">*</span>
                </Label>
                <div className="space-y-1">
                  <Input
                    value={selectedProviderCredential.base_url ?? ""}
                    placeholder={getCustomProviderBaseUrlPlaceholder(
                      selectedProviderCredential.provider_kind,
                    )}
                    className="h-9 rounded-lg text-sm"
                    required
                    aria-invalid={showProviderValidation && selectedProviderEndpointMissing}
                    onBlur={() => setShowProviderValidation(true)}
                    onChange={(event) =>
                      updateSelectedProviderCredential(selectedProviderCredential.id, {
                        base_url: event.target.value,
                      })
                    }
                  />
                  {showProviderValidation && selectedProviderEndpointMissing ? (
                    <p className="text-xs text-destructive">{t("ai.providerEndpointRequired")}</p>
                  ) : null}
                </div>
              </div>
              <div className="grid gap-2 border-b border-border/60 py-2.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,2.2fr)] sm:items-center">
                <Label className="text-xs font-normal">{t("ai.providerProtocol")}</Label>
                <Select
                  value={providerProtocolSelectValue(selectedProviderCredential)}
                  onValueChange={(value) =>
                    updateSelectedProviderProtocol(selectedProviderCredential, value)
                  }
                >
                  <SelectTrigger className="h-9 w-full rounded-lg text-left text-sm">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>{protocolOptions(selectedProviderCredential)}</SelectContent>
                </Select>
              </div>
              <div className="grid gap-2 border-b border-border/60 py-2.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,2.2fr)] sm:items-center">
                <Label className="text-xs font-normal">
                  {t("settings.apiKey")}
                  {selectedProviderApiKeyRequired ? <span aria-hidden="true"> *</span> : null}
                </Label>
                <div className="space-y-1">
                  <div className="relative">
                    <Input
                      data-custom-password-reveal
                      type={showProviderApiKey ? "text" : "password"}
                      value={
                        showProviderApiKey &&
                        revealedProviderApiKey?.credentialId === selectedProviderCredential.id &&
                        isCloudSecretMasked(selectedProviderCredential.api_key)
                          ? revealedProviderApiKey.value
                          : secretInputValue(selectedProviderCredential.api_key)
                      }
                      placeholder={secretPlaceholder(
                        selectedProviderCredential.api_key,
                        t("settings.apiKey"),
                      )}
                      className="h-9 rounded-lg pr-10 text-sm"
                      required={selectedProviderApiKeyRequired}
                      aria-invalid={showProviderValidation && selectedProviderApiKeyMissing}
                      onBlur={() => setShowProviderValidation(true)}
                      onChange={(event) => {
                        setRevealedProviderApiKey(null);
                        updateSelectedProviderCredential(selectedProviderCredential.id, {
                          api_key: event.target.value,
                        });
                      }}
                    />
                    <Button
                      type="button"
                      size="icon-sm"
                      variant="ghost"
                      className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground"
                      title={t(showProviderApiKey ? "ai.hideApiKey" : "ai.showApiKey")}
                      aria-label={t(showProviderApiKey ? "ai.hideApiKey" : "ai.showApiKey")}
                      disabled={revealingProviderApiKey}
                      onClick={() => void toggleProviderApiKeyVisibility()}
                    >
                      {showProviderApiKey ? (
                        <MdVisibilityOff className="size-3.5" />
                      ) : (
                        <MdVisibility className="size-3.5" />
                      )}
                    </Button>
                  </div>
                  {showProviderValidation && selectedProviderApiKeyMissing ? (
                    <p className="text-xs text-destructive">{t("ai.providerApiKeyRequired")}</p>
                  ) : null}
                </div>
              </div>
              <div className="grid gap-2 py-2.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,2.2fr)] sm:items-center">
                <Label className="text-xs font-normal">{t("ai.connectionTest")}</Label>
                <div className="flex items-center justify-end gap-2">
                  {providerModelCount !== null ? (
                    <span className="text-xs text-muted-foreground">
                      {t("ai.connectionModelCount", {
                        count: providerModelCount,
                      })}
                    </span>
                  ) : null}
                  <span
                    role="img"
                    className={`size-2.5 rounded-full ${
                      selectedProviderConnectionStatus === "testing"
                        ? "animate-pulse bg-primary"
                        : selectedProviderConnectionStatus === "success"
                          ? "bg-emerald-500"
                          : selectedProviderConnectionStatus === "error"
                            ? "bg-destructive"
                            : "bg-muted-foreground/30"
                    }`}
                    title={t(`ai.connectionStatus.${selectedProviderConnectionStatus}`)}
                    aria-label={t(`ai.connectionStatus.${selectedProviderConnectionStatus}`)}
                  />
                  <Button
                    type="button"
                    className="h-9 rounded-lg px-3 text-sm"
                    disabled={
                      selectedProviderEndpointMissing ||
                      selectedProviderApiKeyMissing ||
                      refreshing ||
                      testingProviderId === selectedProviderCredential.id
                    }
                    onClick={() => void testProviderConnection()}
                  >
                    {t("ai.connectionTest")}
                  </Button>
                </div>
              </div>
            </div>
          ) : null}
        </SettingSection>
      ) : null}

      {enabledCredentials.length > 0 ? renderModelListCard() : null}
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

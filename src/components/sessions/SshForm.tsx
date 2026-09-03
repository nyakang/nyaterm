import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronsUpDownIcon, Eye, EyeOff } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdCheck,
  MdChevronRight,
  MdClose,
  MdDeleteOutline,
  MdExpandMore,
  MdKeyboardArrowDown,
  MdKeyboardArrowUp,
  MdRefresh,
  MdSettings,
} from "react-icons/md";
import type { ConnectionOption } from "@/components/network/shared";
import { KeyManagementTab } from "@/components/panel/security-auth/KeyManagementTab";
import { PasswordManagementTab } from "@/components/panel/security-auth/PasswordManagementTab";
import { ConnectionRecordingSettings } from "@/components/sessions/ConnectionRecordingSettings";
import { SessionNetworkSection } from "@/components/sessions/SessionNetworkSection";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NumberInput } from "@/components/ui/number-input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { isLinux, isMacOS, isWindows } from "@/lib/platform";
import {
  MAX_SSH_AGENT_FORWARDING_ENDPOINTS,
  MAX_SSH_AGENT_FORWARDING_IDENTITIES,
} from "@/lib/sshAgent";
import { cn } from "@/lib/utils";
import type {
  AlgorithmOption,
  OtpEntry,
  ProxyConfig,
  RecordingMode,
  SavedPassword,
  SftpSettings,
  SshAgentEndpoint,
  SshAgentForwardingConfig,
  SshAgentForwardingEndpointError,
  SshAgentForwardingIdentity,
  SshAgentForwardingIdentityResponse,
  SshAlgorithmDefaults,
  SshAlgorithmPreferences,
  SshKey,
  SshProfile,
  SshTerminalType,
  SupportedSshAlgorithms,
} from "@/types/global";

const MASKED_PASSWORD_PLACEHOLDER = "••••••••";
const DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS = 3000;
const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS = 100;
const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS = 60_000;
export type SshAuthMode = "none" | "password" | "key" | "agent";
type PasswordSource = "ask" | "direct" | "saved";
type SshTerminalTypeSelection = SshTerminalType | "default";

function isSupportedSshAgentEndpoint(type: SshAgentEndpoint["type"]): boolean {
  if (type === "auto") return true;
  if (isWindows) return type === "pageant" || type === "windows_open_ssh";
  return (isMacOS || isLinux) && (type === "environment" || type === "unix_socket");
}

function defaultForwardingEndpoint(): SshAgentEndpoint {
  if (isWindows) return { type: "windows_open_ssh" };
  return { type: "environment", variable: "SSH_AUTH_SOCK" };
}

interface SshFormProps {
  host: string;
  setHost: (v: string) => void;
  port: number;
  setPort: (v: number) => void;
  username: string;
  setUsername: (v: string) => void;
  authType: SshAuthMode;
  setAuthType: (v: SshAuthMode) => void;
  passwordId: string;
  setPasswordId: (v: string) => void;
  password: string;
  setPassword: (v: string) => void;
  hasPassword: boolean;
  setHasPassword: (v: boolean) => void;
  keyId: string;
  setKeyId: (v: string) => void;
  proxyId: string;
  setProxyId: (v: string) => void;
  proxies: ProxyConfig[];
  jumpHostId: string;
  setJumpHostId: (v: string) => void;
  jumpHostOptions: ConnectionOption[];
  otpId: string;
  setOtpId: (v: string) => void;
  autoFillOtp: boolean;
  setAutoFillOtp: (v: boolean) => void;
  otpEntries: OtpEntry[];
  postLoginEnabled: boolean;
  setPostLoginEnabled: (v: boolean) => void;
  postLoginCommand: string;
  setPostLoginCommand: (v: string) => void;
  postLoginDelayMs: number;
  setPostLoginDelayMs: (v: number) => void;
  minPostLoginDelayMs: number;
  maxPostLoginDelayMs: number;
  backspaceMode: string;
  setBackspaceMode: (v: string) => void;
  x11Forwarding: boolean;
  setX11Forwarding: (v: boolean) => void;
  authAgentEndpoint: SshAgentEndpoint;
  setAuthAgentEndpoint: (v: SshAgentEndpoint) => void;
  authAgentEndpointError?: string;
  agentForwardingConfig: SshAgentForwardingConfig;
  setAgentForwardingConfig: (v: SshAgentForwardingConfig) => void;
  agentForwardingEndpointError?: string;
  sshAlgorithms: SshAlgorithmPreferences;
  setSshAlgorithms: (v: SshAlgorithmPreferences) => void;
  sshProfile: SshProfile;
  setSshProfile: (v: SshProfile) => void;
  sshTerminalType: SshTerminalTypeSelection;
  setSshTerminalType: (v: SshTerminalTypeSelection) => void;
  sftpSettings: SftpSettings;
  setSftpSettings: (v: SftpSettings) => void;
  remoteDynamicTabTitle: boolean;
  setRemoteDynamicTabTitle: (v: boolean) => void;
  recordingUseGlobal: boolean;
  setRecordingUseGlobal: (v: boolean) => void;
  recordingAutoStart: boolean;
  setRecordingAutoStart: (v: boolean) => void;
  recordingMode: RecordingMode;
  setRecordingMode: (v: RecordingMode) => void;
  connectionId?: string;
  encoding: string;
  setEncoding: (v: string) => void;
  passwordSecretsUnlocked?: boolean;
  onUnlockPasswordSecrets?: () => void;
  onLockPasswordSecrets?: () => void;
}

function RequiredMark() {
  return <span className="ml-0.5 text-destructive">*</span>;
}

interface AdvancedComboboxOption {
  id: string;
  label: string;
  searchText: string;
  subtitle: string;
}

function AdvancedCombobox({
  value,
  options,
  placeholder,
  searchPlaceholder,
  emptyText,
  missingSelectionLabel,
  clearLabel,
  onChange,
}: {
  value: string;
  options: AdvancedComboboxOption[];
  placeholder: string;
  searchPlaceholder: string;
  emptyText: string;
  missingSelectionLabel: string;
  clearLabel: string;
  onChange: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = options.find((option) => option.id === value);
  const displayLabel = selected ? selected.label : value ? missingSelectionLabel : placeholder;
  const displaySubtitle = selected?.subtitle ?? "";

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="h-auto min-h-10 w-full justify-between px-3 py-2 font-normal"
        >
          <div className="min-w-0 text-left">
            <div
              className={cn(
                "truncate text-sm",
                !selected && !value ? "text-muted-foreground" : "text-foreground",
              )}
            >
              {displayLabel}
            </div>
            {(selected || value) && (
              <div className="truncate text-xs text-muted-foreground">
                {displaySubtitle || missingSelectionLabel}
              </div>
            )}
          </div>
          <ChevronsUpDownIcon className="ml-3 shrink-0 text-sm text-muted-foreground opacity-70" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="bottom"
        collisionPadding={16}
        className="w-[min(32rem,calc(100vw-2rem))] p-0"
      >
        <Command>
          <CommandInput placeholder={searchPlaceholder} />
          <CommandList className="max-h-72">
            <CommandEmpty>{emptyText}</CommandEmpty>
            <CommandGroup className="p-0">
              <CommandItem
                value={clearLabel}
                className="items-start gap-3 px-3 py-2"
                onSelect={() => {
                  onChange("");
                  setOpen(false);
                }}
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm">{clearLabel}</div>
                </div>
                {!value ? <MdCheck className="mt-0.5 text-sm text-primary" /> : null}
              </CommandItem>
            </CommandGroup>
            <CommandGroup className="p-0">
              {options.map((option) => (
                <CommandItem
                  key={option.id}
                  value={`${option.label} ${option.searchText}`}
                  className="items-start gap-3 px-3 py-2"
                  onSelect={() => {
                    onChange(option.id);
                    setOpen(false);
                  }}
                >
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">{option.label}</div>
                    <div className="truncate text-xs text-muted-foreground">{option.subtitle}</div>
                  </div>
                  {option.id === value ? <MdCheck className="mt-0.5 text-sm text-primary" /> : null}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function formatOtpLabel(entry: OtpEntry) {
  return entry.issuer && entry.username
    ? `${entry.issuer} (${entry.username})`
    : entry.issuer || entry.username || entry.id;
}

function formatOtpSubtitle(entry: OtpEntry) {
  return [entry.otp_type.toUpperCase(), entry.algorithm, `${entry.digits}`]
    .filter(Boolean)
    .join(" · ");
}

function emptyAlgorithmPreferences(mode: SshAlgorithmPreferences["mode"]): SshAlgorithmPreferences {
  return {
    mode,
    kex: [],
    ciphers: [],
    macs: [],
    host_keys: [],
  };
}

function withDefaultAlgorithms(
  value: SshAlgorithmPreferences,
  defaults: SshAlgorithmDefaults,
): SshAlgorithmPreferences {
  return {
    mode: value.mode,
    kex: value.kex.length > 0 ? value.kex : defaults.kex,
    ciphers: value.ciphers.length > 0 ? value.ciphers : defaults.ciphers,
    macs: value.macs.length > 0 ? value.macs : defaults.macs,
    host_keys: value.host_keys.length > 0 ? value.host_keys : defaults.host_keys,
  };
}

function riskLabel(risk: AlgorithmOption["risk"], t: ReturnType<typeof useTranslation>["t"]) {
  if (risk === "insecure") return t("dialog.algorithmRiskInsecure");
  if (risk === "legacy") return t("dialog.algorithmRiskLegacy");
  return t("dialog.algorithmRiskModern");
}

function riskClassName(risk: AlgorithmOption["risk"]) {
  if (risk === "insecure") {
    return "border-destructive/40 bg-destructive/10 text-destructive";
  }
  if (risk === "legacy") {
    return "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300";
  }
  return "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300";
}

function moveItem(items: string[], index: number, direction: -1 | 1) {
  const nextIndex = index + direction;
  if (nextIndex < 0 || nextIndex >= items.length) return items;
  const next = [...items];
  [next[index], next[nextIndex]] = [next[nextIndex], next[index]];
  return next;
}

function AlgorithmOrderList({
  options,
  value,
  onChange,
}: {
  options: AlgorithmOption[];
  value: string[];
  onChange: (value: string[]) => void;
}) {
  const { t } = useTranslation();
  const selected = new Set(value);
  const optionById = new Map(options.map((option) => [option.id, option]));
  const rows = [
    ...value
      .map((id) => optionById.get(id))
      .filter((option): option is AlgorithmOption => Boolean(option)),
    ...options.filter((option) => !selected.has(option.id)),
  ];

  return (
    <div className="max-h-56 space-y-1 overflow-y-auto pr-1">
      {rows.map((option) => {
        const enabled = selected.has(option.id);
        const enabledIndex = value.indexOf(option.id);
        const isLastEnabled = enabled && value.length <= 1;
        return (
          <div
            key={option.id}
            className={cn(
              "grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-md border px-2 py-1.5",
              enabled ? "bg-accent/50" : "bg-muted/20 opacity-70",
            )}
          >
            <Checkbox
              className="size-3.5"
              checked={enabled}
              disabled={isLastEnabled}
              onCheckedChange={(checked) => {
                if (checked === true) {
                  onChange([...value, option.id]);
                } else {
                  onChange(value.filter((id) => id !== option.id));
                }
              }}
              aria-label={option.label}
            />
            <div className="min-w-0">
              <div className="truncate font-mono text-[0.6875rem]">{option.label}</div>
              <span
                className={cn(
                  "mt-0.5 inline-flex rounded border px-1.5 py-0.5 text-[0.5625rem] leading-none",
                  riskClassName(option.risk),
                )}
              >
                {riskLabel(option.risk, t)}
              </span>
            </div>
            <div className="flex items-center gap-0.5">
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                disabled={!enabled || enabledIndex <= 0}
                onClick={() => onChange(moveItem(value, enabledIndex, -1))}
                title={t("dialog.moveUp")}
              >
                <MdKeyboardArrowUp className="text-sm" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                disabled={!enabled || enabledIndex < 0 || enabledIndex >= value.length - 1}
                onClick={() => onChange(moveItem(value, enabledIndex, 1))}
                title={t("dialog.moveDown")}
              >
                <MdKeyboardArrowDown className="text-sm" />
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function SshForm({
  host,
  setHost,
  port,
  setPort,
  username,
  setUsername,
  authType,
  setAuthType,
  passwordId,
  setPasswordId,
  password,
  setPassword,
  hasPassword,
  setHasPassword,
  keyId,
  setKeyId,
  proxyId,
  setProxyId,
  proxies,
  jumpHostId,
  setJumpHostId,
  jumpHostOptions,
  otpId,
  setOtpId,
  autoFillOtp,
  setAutoFillOtp,
  otpEntries,
  postLoginEnabled,
  setPostLoginEnabled,
  postLoginCommand,
  setPostLoginCommand,
  postLoginDelayMs,
  setPostLoginDelayMs,
  minPostLoginDelayMs,
  maxPostLoginDelayMs,
  backspaceMode,
  setBackspaceMode,
  x11Forwarding,
  setX11Forwarding,
  authAgentEndpoint,
  setAuthAgentEndpoint,
  authAgentEndpointError,
  agentForwardingConfig,
  setAgentForwardingConfig,
  agentForwardingEndpointError,
  sshAlgorithms,
  setSshAlgorithms,
  sshProfile,
  setSshProfile,
  sshTerminalType,
  setSshTerminalType,
  sftpSettings,
  setSftpSettings,
  remoteDynamicTabTitle,
  setRemoteDynamicTabTitle,
  recordingUseGlobal,
  setRecordingUseGlobal,
  recordingAutoStart,
  setRecordingAutoStart,
  recordingMode,
  setRecordingMode,
  connectionId,
  encoding,
  setEncoding,
  passwordSecretsUnlocked = false,
  onUnlockPasswordSecrets,
  onLockPasswordSecrets,
}: SshFormProps) {
  const { t } = useTranslation();
  const [sshKeys, setSshKeys] = useState<SshKey[]>([]);
  const [savedPasswords, setSavedPasswords] = useState<SavedPassword[]>([]);
  const [showKeyDropdown, setShowKeyDropdown] = useState(false);
  const [showPasswordDropdown, setShowPasswordDropdown] = useState(false);
  const [showKeyManagement, setShowKeyManagement] = useState(false);
  const [showPasswordManagement, setShowPasswordManagement] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [showDirectPassword, setShowDirectPassword] = useState(false);
  const [directPasswordLoading, setDirectPasswordLoading] = useState(false);
  const [showAgentAllowAllWarning, setShowAgentAllowAllWarning] = useState(false);
  const [showAgentIdentityPicker, setShowAgentIdentityPicker] = useState(false);
  const [agentIdentities, setAgentIdentities] = useState<SshAgentForwardingIdentity[]>([]);
  const [agentEndpointErrors, setAgentEndpointErrors] = useState<SshAgentForwardingEndpointError[]>(
    [],
  );
  const [agentIdentityLoadError, setAgentIdentityLoadError] = useState("");
  const [agentIdentityLoading, setAgentIdentityLoading] = useState(false);
  const [agentIdentityTruncated, setAgentIdentityTruncated] = useState(false);
  const agentIdentityRequestGeneration = useRef(0);
  const [supportedAlgorithms, setSupportedAlgorithms] = useState<SupportedSshAlgorithms | null>(
    null,
  );
  const [passwordSource, setPasswordSource] = useState<PasswordSource>(
    passwordId ? "saved" : password || hasPassword ? "direct" : "ask",
  );

  const loadSshKeys = useCallback(async () => {
    try {
      const keys = await invoke<SshKey[]>("get_ssh_keys");
      setSshKeys(keys);
      if (keyId && !keys.some((key) => key.id === keyId)) {
        setKeyId("");
      }
    } catch {
      /* ignore */
    }
  }, [keyId, setKeyId]);

  const loadPasswords = useCallback(async () => {
    try {
      const passwords = await invoke<SavedPassword[]>("get_saved_passwords");
      setSavedPasswords(passwords);
      if (passwordId && !passwords.some((p) => p.id === passwordId)) {
        setPasswordId("");
      }
    } catch {
      /* ignore */
    }
  }, [passwordId, setPasswordId]);

  const loadAgentIdentities = useCallback(async () => {
    const generation = ++agentIdentityRequestGeneration.current;
    if (
      !agentForwardingConfig.enabled ||
      (!agentForwardingConfig.sources.external_agent && !agentForwardingConfig.sources.stored_keys)
    ) {
      if (generation !== agentIdentityRequestGeneration.current) return;
      setAgentIdentities([]);
      setAgentEndpointErrors([]);
      setAgentIdentityTruncated(false);
      setAgentIdentityLoadError("");
      return;
    }

    setAgentIdentityLoading(true);
    setAgentIdentityLoadError("");
    try {
      const response = await invoke<SshAgentForwardingIdentityResponse>(
        "get_ssh_agent_forwarding_identities",
        {
          forwardingConfig: {
            enabled: agentForwardingConfig.enabled,
            sources: agentForwardingConfig.sources,
            // Identity preview enumerates candidates; the saved policy is applied only when forwarding.
            policy: { mode: "all" },
          },
        },
      );
      if (generation !== agentIdentityRequestGeneration.current) return;
      setAgentIdentities(response.identities);
      setAgentEndpointErrors(response.endpoint_errors);
      setAgentIdentityTruncated(response.truncated);
    } catch (error) {
      if (generation !== agentIdentityRequestGeneration.current) return;
      setAgentIdentities([]);
      setAgentEndpointErrors([]);
      setAgentIdentityTruncated(false);
      setAgentIdentityLoadError(getErrorMessage(error));
    } finally {
      if (generation === agentIdentityRequestGeneration.current) {
        setAgentIdentityLoading(false);
      }
    }
  }, [agentForwardingConfig.enabled, agentForwardingConfig.sources]);

  useEffect(() => {
    if (showAgentIdentityPicker) {
      void loadAgentIdentities();
    } else {
      agentIdentityRequestGeneration.current += 1;
      setAgentIdentityLoading(false);
    }
  }, [loadAgentIdentities, showAgentIdentityPicker]);

  useEffect(() => {
    if (passwordId) {
      setPasswordSource("saved");
    } else if (password || hasPassword) {
      setPasswordSource("direct");
    }
  }, [hasPassword, password, passwordId]);

  useEffect(() => {
    let unlisten: () => void;
    getCurrentWindow()
      .onFocusChanged((event) => {
        if (event.payload) {
          void loadSshKeys();
          void loadPasswords();
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    void loadSshKeys();
    void loadPasswords();
    return () => {
      if (unlisten) unlisten();
    };
  }, [loadSshKeys, loadPasswords]);

  useEffect(() => {
    invoke<SupportedSshAlgorithms>("get_supported_ssh_algorithms")
      .then(setSupportedAlgorithms)
      .catch(() => {
        /* ignore */
      });
  }, []);

  useEffect(() => {
    if (!supportedAlgorithms || sshAlgorithms.mode !== "custom") return;
    const next = withDefaultAlgorithms(sshAlgorithms, supportedAlgorithms.compatible);
    if (
      next.kex !== sshAlgorithms.kex ||
      next.ciphers !== sshAlgorithms.ciphers ||
      next.macs !== sshAlgorithms.macs ||
      next.host_keys !== sshAlgorithms.host_keys
    ) {
      setSshAlgorithms(next);
    }
  }, [setSshAlgorithms, sshAlgorithms, supportedAlgorithms]);

  useEffect(() => {
    if (!isSupportedSshAgentEndpoint(authAgentEndpoint.type)) {
      setAuthAgentEndpoint({ type: "auto" });
    }
  }, [authAgentEndpoint.type, setAuthAgentEndpoint]);

  const selectedKeyName = sshKeys.find((k) => k.id === keyId)?.name;
  const selectedPasswordName = savedPasswords.find((p) => p.id === passwordId)?.name;
  const availableAgentEndpointTypes: SshAgentEndpoint["type"][] = isWindows
    ? ["auto", "pageant", "windows_open_ssh"]
    : isMacOS || isLinux
      ? ["auto", "environment", "unix_socket"]
      : ["auto"];
  const availableForwardingEndpointTypes = availableAgentEndpointTypes;
  const forwardingEndpointCandidates: SshAgentEndpoint[] = isWindows
    ? [{ type: "windows_open_ssh" }, { type: "pageant" }, { type: "auto" }]
    : [
        { type: "environment", variable: "SSH_AUTH_SOCK" },
        { type: "unix_socket", path: "" },
        { type: "auto" },
      ];
  const forwardingEndpointKeys = new Set(
    agentForwardingConfig.sources.external_agent_endpoints.map((endpoint) =>
      JSON.stringify(endpoint),
    ),
  );
  const canAddForwardingEndpoint = forwardingEndpointCandidates.some(
    (endpoint) => !forwardingEndpointKeys.has(JSON.stringify(endpoint)),
  );
  const visibleAgentEndpointType = isSupportedSshAgentEndpoint(authAgentEndpoint.type)
    ? authAgentEndpoint.type
    : "auto";
  const updateForwardingSources = (patch: Partial<SshAgentForwardingConfig["sources"]>) => {
    setAgentForwardingConfig({
      ...agentForwardingConfig,
      sources: { ...agentForwardingConfig.sources, ...patch },
    });
  };
  const updateForwardingEndpoint = (index: number, endpoint: SshAgentEndpoint) => {
    const endpoints = [...agentForwardingConfig.sources.external_agent_endpoints];
    endpoints[index] = endpoint;
    updateForwardingSources({ external_agent_endpoints: endpoints });
  };
  const addForwardingEndpoint = () => {
    const endpoint = forwardingEndpointCandidates.find(
      (candidate) => !forwardingEndpointKeys.has(JSON.stringify(candidate)),
    );
    if (!endpoint) return;
    updateForwardingSources({
      external_agent: true,
      external_agent_endpoints: [
        ...agentForwardingConfig.sources.external_agent_endpoints,
        endpoint,
      ],
    });
  };
  const removeForwardingEndpoint = (index: number) => {
    updateForwardingSources({
      external_agent_endpoints: agentForwardingConfig.sources.external_agent_endpoints.filter(
        (_, endpointIndex) => endpointIndex !== index,
      ),
    });
  };
  const moveForwardingEndpoint = (index: number, direction: -1 | 1) => {
    const nextIndex = index + direction;
    const endpoints = [...agentForwardingConfig.sources.external_agent_endpoints];
    if (nextIndex < 0 || nextIndex >= endpoints.length) return;
    [endpoints[index], endpoints[nextIndex]] = [endpoints[nextIndex], endpoints[index]];
    updateForwardingSources({ external_agent_endpoints: endpoints });
  };
  const otpOptions = otpEntries.map((entry) => ({
    id: entry.id,
    label: formatOtpLabel(entry),
    searchText: [entry.issuer, entry.username, entry.otp_type, entry.algorithm]
      .filter(Boolean)
      .join(" "),
    subtitle: formatOtpSubtitle(entry),
  }));
  const setAlgorithmMode = (mode: SshAlgorithmPreferences["mode"]) => {
    if (mode === "custom" && supportedAlgorithms) {
      setSshAlgorithms(
        withDefaultAlgorithms({ ...sshAlgorithms, mode }, supportedAlgorithms.compatible),
      );
      return;
    }
    setSshAlgorithms(emptyAlgorithmPreferences(mode));
  };
  const setAlgorithmList = (
    key: keyof Pick<SshAlgorithmPreferences, "kex" | "ciphers" | "macs" | "host_keys">,
    value: string[],
  ) => {
    setSshAlgorithms({
      ...sshAlgorithms,
      mode: "custom",
      [key]: value,
    });
  };
  const networkDeviceProfile = sshProfile === "network_device";
  const sftpDisabled = !sftpSettings.enabled || networkDeviceProfile;
  const defaultTerminalType = networkDeviceProfile ? "vt100" : "xterm-256color";

  const toggleDirectPasswordVisibility = async () => {
    if (showDirectPassword) {
      setShowDirectPassword(false);
      return;
    }

    if (!password && hasPassword && connectionId) {
      setDirectPasswordLoading(true);
      try {
        const value = await invoke<string | null>("get_connection_password_value", {
          id: connectionId,
        });
        if (value) {
          setPassword(value);
          setHasPassword(false);
        }
      } catch {
        return;
      } finally {
        setDirectPasswordLoading(false);
      }
    }

    setShowDirectPassword(true);
  };

  return (
    <div className="space-y-3 w-full">
      <div className="flex flex-col gap-3 sm:flex-row">
        <div className="min-w-0 flex-1">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.host")}
            <RequiredMark />
          </Label>
          <Input
            className="mt-1 text-xs h-8"
            placeholder="192.168.1.100"
            value={host}
            onChange={(e) => setHost(e.target.value)}
          />
        </div>
        <div className="w-full sm:w-32">
          <Label className="text-xs font-medium text-foreground/80">
            {t("dialog.port")}
            <RequiredMark />
          </Label>
          <NumberInput
            className="mt-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
            value={port}
            onChange={setPort}
            min={1}
            max={65535}
          />
        </div>
      </div>
      <div>
        <Label className="text-xs font-medium text-foreground/80">
          {t("dialog.username")}
          <RequiredMark />
        </Label>
        <Input
          className="mt-1 text-xs h-8"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />
      </div>
      <div>
        <Label className="text-xs font-medium text-foreground/80">
          {t("dialog.authentication")}
        </Label>
        <Tabs
          value={authType}
          onValueChange={(v) => {
            const nextAuthType = v as SshAuthMode;
            setAuthType(nextAuthType);
            if (nextAuthType === "none") {
              setPasswordId("");
              setPassword("");
              setHasPassword(false);
              setKeyId("");
            }
          }}
          className="w-full mt-1"
        >
          <TabsList className="grid h-8 w-full grid-cols-4 pointer-events-auto">
            <TabsTrigger value="none" className="text-xs">
              {t("dialog.noAuthentication", "None")}
            </TabsTrigger>
            <TabsTrigger value="password" className="text-xs">
              {t("dialog.password")}
            </TabsTrigger>
            <TabsTrigger value="key" className="text-xs">
              {t("dialog.privateKey")}
            </TabsTrigger>
            <TabsTrigger value="agent" className="text-xs">
              {t("dialog.sshAgent", "SSH Agent")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="none" className="mt-3 border-0 outline-none">
            <div className="rounded-md border border-dashed bg-accent/25 px-3 py-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
              {t(
                "dialog.noAuthenticationDescription",
                "Connect without a password or private key. Use this only for SSH servers that allow none authentication or will request credentials interactively.",
              )}
            </div>
          </TabsContent>

          <TabsContent value="password" className="mt-3 border-0 outline-none">
            <Label className="text-xs font-medium text-foreground/80">
              {t("dialog.passwordSource")}
            </Label>
            <Tabs
              value={passwordSource}
              onValueChange={(value) => {
                const nextSource = value as PasswordSource;
                setPasswordSource(nextSource);
                if (nextSource === "direct") {
                  setPasswordId("");
                } else if (nextSource === "saved") {
                  setPassword("");
                  setHasPassword(false);
                } else {
                  setPasswordId("");
                  setPassword("");
                  setHasPassword(false);
                }
              }}
              className="mt-1 w-full"
            >
              <TabsList className="grid h-8 w-full grid-cols-3 pointer-events-auto">
                <TabsTrigger value="ask" className="text-xs">
                  {t("dialog.askWhenConnecting")}
                </TabsTrigger>
                <TabsTrigger value="direct" className="text-xs">
                  {t("dialog.directPassword")}
                </TabsTrigger>
                <TabsTrigger value="saved" className="text-xs">
                  {t("dialog.savedPassword")}
                </TabsTrigger>
              </TabsList>

              <TabsContent value="ask" className="mt-3 border-0 outline-none">
                <div className="rounded-md border border-dashed bg-accent/25 px-3 py-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                  {t("dialog.askPasswordDescription")}
                </div>
              </TabsContent>

              <TabsContent value="direct" className="mt-3 border-0 outline-none">
                <Label className="text-xs font-medium text-foreground/80">
                  {t("dialog.password")}
                </Label>
                <div className="relative mt-1">
                  <Input
                    type={showDirectPassword ? "text" : "password"}
                    className="text-xs h-8 pr-16"
                    placeholder={
                      hasPassword && !password
                        ? MASKED_PASSWORD_PLACEHOLDER
                        : t("dialog.passwordPlaceholder")
                    }
                    value={password}
                    onChange={(e) => {
                      setPassword(e.target.value);
                      setPasswordId("");
                      if (e.target.value) {
                        setHasPassword(false);
                      }
                    }}
                    disabled={directPasswordLoading}
                  />
                  {(password || hasPassword) && (
                    <button
                      type="button"
                      className="absolute right-7 top-1/2 -translate-y-1/2 p-0.5 rounded-sm text-muted-foreground hover:text-foreground transition-colors disabled:opacity-50"
                      title={
                        showDirectPassword ? t("dialog.hidePassword") : t("dialog.showPassword")
                      }
                      disabled={directPasswordLoading}
                      onClick={() => {
                        void toggleDirectPasswordVisibility();
                      }}
                    >
                      {showDirectPassword ? (
                        <EyeOff className="h-3.5 w-3.5" />
                      ) : (
                        <Eye className="h-3.5 w-3.5" />
                      )}
                    </button>
                  )}
                  {(password || hasPassword) && (
                    <button
                      type="button"
                      className="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 rounded-sm text-muted-foreground hover:text-foreground transition-colors"
                      title={t("dialog.clearPassword", "Clear password")}
                      onClick={() => {
                        setPassword("");
                        setHasPassword(false);
                        setShowDirectPassword(false);
                      }}
                    >
                      <MdClose className="text-sm" />
                    </button>
                  )}
                </div>
              </TabsContent>

              <TabsContent value="saved" className="mt-3 border-0 outline-none">
                <Label className="text-xs font-medium text-foreground/80">
                  {t("dialog.savedPassword")}
                </Label>
                <Popover open={showPasswordDropdown} onOpenChange={setShowPasswordDropdown}>
                  <PopoverTrigger asChild>
                    <Button
                      type="button"
                      variant="outline"
                      className="mt-1 h-8 w-full justify-between text-xs font-normal"
                    >
                      <span className={`truncate ${passwordId ? "" : "text-muted-foreground"}`}>
                        {selectedPasswordName || t("dialog.selectPassword")}
                      </span>
                      <MdExpandMore className="shrink-0 text-xs text-muted-foreground" />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent
                    align="start"
                    side="bottom"
                    sideOffset={4}
                    collisionPadding={16}
                    className="w-(--radix-popover-trigger-width) min-w-56 overflow-hidden p-0"
                  >
                    <div className="max-h-40 overflow-y-auto overflow-x-hidden">
                      <button
                        type="button"
                        className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent ${!passwordId ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                        onClick={() => {
                          setPasswordId("");
                          setShowPasswordDropdown(false);
                        }}
                      >
                        {t("dialog.none")}
                      </button>
                      {savedPasswords.map((p) => (
                        <button
                          key={p.id}
                          type="button"
                          className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent ${passwordId === p.id ? "bg-primary/15 text-primary" : ""}`}
                          onClick={() => {
                            setPasswordId(p.id);
                            setPassword("");
                            setHasPassword(false);
                            setShowPasswordDropdown(false);
                          }}
                        >
                          {p.name}
                        </button>
                      ))}
                      {savedPasswords.length === 0 && (
                        <div className="px-3 py-2 text-xs text-muted-foreground">
                          {t("dialog.noPasswords")}
                        </div>
                      )}
                    </div>
                    <button
                      type="button"
                      className="flex w-full shrink-0 items-center gap-1.5 border-t bg-popover px-3 py-1.5 text-left text-xs text-primary transition-colors hover:bg-accent"
                      onClick={() => {
                        setShowPasswordDropdown(false);
                        setShowPasswordManagement(true);
                      }}
                    >
                      <MdSettings className="text-sm" />
                      {t("dialog.managePasswords")}
                    </button>
                  </PopoverContent>
                </Popover>
              </TabsContent>
            </Tabs>
          </TabsContent>

          <TabsContent value="key" className="mt-3 border-0 outline-none">
            <Label className="text-xs font-medium text-foreground/80">
              {t("dialog.privateKey")}
            </Label>
            <Popover open={showKeyDropdown} onOpenChange={setShowKeyDropdown}>
              <PopoverTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  className="mt-1 h-8 w-full justify-between text-xs font-normal"
                >
                  <span className={`truncate ${keyId ? "" : "text-muted-foreground"}`}>
                    {selectedKeyName || t("dialog.selectKey")}
                  </span>
                  <MdExpandMore className="shrink-0 text-xs text-muted-foreground" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="start"
                side="bottom"
                sideOffset={4}
                collisionPadding={16}
                className="w-(--radix-popover-trigger-width) min-w-56 overflow-hidden p-0"
              >
                <div className="max-h-40 overflow-y-auto overflow-x-hidden">
                  <button
                    type="button"
                    className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent ${!keyId ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                    onClick={() => {
                      setKeyId("");
                      setShowKeyDropdown(false);
                    }}
                  >
                    {t("dialog.none")}
                  </button>
                  {sshKeys.map((k) => (
                    <button
                      key={k.id}
                      type="button"
                      className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent ${keyId === k.id ? "bg-primary/15 text-primary" : ""}`}
                      onClick={() => {
                        setKeyId(k.id);
                        setShowKeyDropdown(false);
                      }}
                    >
                      {k.name}
                    </button>
                  ))}
                  {sshKeys.length === 0 && (
                    <div className="px-3 py-2 text-xs text-muted-foreground">
                      {t("dialog.noKeys")}
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  className="flex w-full shrink-0 items-center gap-1.5 border-t bg-popover px-3 py-1.5 text-left text-xs text-primary transition-colors hover:bg-accent"
                  onClick={() => {
                    setShowKeyDropdown(false);
                    setShowKeyManagement(true);
                  }}
                >
                  <MdSettings className="text-sm" />
                  {t("dialog.manageKeys")}
                </button>
              </PopoverContent>
            </Popover>
          </TabsContent>
          <TabsContent value="agent" className="mt-3 border-0 outline-none">
            <div className="space-y-3 rounded-lg border bg-accent/25 p-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.sshAgentEndpoint", "Agent endpoint")}
                  </Label>
                  <p className="mt-1 text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t(
                      "dialog.sshAgentAuthDesc",
                      "Use an identity managed by the local SSH Agent.",
                    )}
                  </p>
                </div>
                <Select
                  value={visibleAgentEndpointType}
                  onValueChange={(type) => {
                    if (type === "auto") setAuthAgentEndpoint({ type: "auto" });
                    if (type === "environment")
                      setAuthAgentEndpoint({ type: "environment", variable: "SSH_AUTH_SOCK" });
                    if (type === "unix_socket")
                      setAuthAgentEndpoint({ type: "unix_socket", path: "" });
                    if (type === "pageant") setAuthAgentEndpoint({ type: "pageant" });
                    if (type === "windows_open_ssh")
                      setAuthAgentEndpoint({ type: "windows_open_ssh" });
                  }}
                >
                  <SelectTrigger className="h-8 w-44 shrink-0 text-xs font-normal">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">{t("dialog.sshAgentAuto", "Automatic")}</SelectItem>
                    {availableAgentEndpointTypes.includes("environment") && (
                      <SelectItem value="environment">
                        {t("dialog.sshAgentEnvironment", "Environment variable")}
                      </SelectItem>
                    )}
                    {availableAgentEndpointTypes.includes("unix_socket") && (
                      <SelectItem value="unix_socket">
                        {t("dialog.sshAgentUnixSocket", "Unix domain socket")}
                      </SelectItem>
                    )}
                    {availableAgentEndpointTypes.includes("pageant") && (
                      <SelectItem value="pageant">
                        {t("dialog.sshAgentPageant", "Pageant")}
                      </SelectItem>
                    )}
                    {availableAgentEndpointTypes.includes("windows_open_ssh") && (
                      <SelectItem value="windows_open_ssh">
                        {t("dialog.sshAgentWindowsOpenSsh", "Windows OpenSSH Agent")}
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
              </div>
              {authAgentEndpoint.type === "environment" && (
                <Input
                  value={authAgentEndpoint.variable}
                  onChange={(event) =>
                    setAuthAgentEndpoint({ type: "environment", variable: event.target.value })
                  }
                  placeholder="SSH_AUTH_SOCK"
                  className="h-8 text-xs"
                />
              )}
              {authAgentEndpoint.type === "unix_socket" && (
                <Input
                  value={authAgentEndpoint.path}
                  onChange={(event) =>
                    setAuthAgentEndpoint({ type: "unix_socket", path: event.target.value })
                  }
                  placeholder="/tmp/ssh-XXXXXX/agent.YYYY"
                  className="h-8 text-xs"
                />
              )}
              {authAgentEndpointError && (
                <p className="text-[0.6875rem] text-destructive" role="alert">
                  {authAgentEndpointError}
                </p>
              )}
            </div>
          </TabsContent>
        </Tabs>
      </div>

      <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
        <CollapsibleTrigger className="group flex w-full items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground">
          <MdChevronRight
            className={`text-sm transition-transform duration-200 ${advancedOpen ? "rotate-90" : ""}`}
          />
          <span>{t("dialog.advancedConfig")}</span>
        </CollapsibleTrigger>
        <CollapsibleContent className="mt-3 space-y-3">
          <Tabs defaultValue="network" className="w-full">
            <TabsList className="grid h-8 w-full grid-cols-3 pointer-events-auto">
              <TabsTrigger value="network" className="text-xs">
                {t("dialog.proxySelect")}
              </TabsTrigger>
              <TabsTrigger value="two-factor" className="text-xs">
                {t("dialog.twoFactorAuth")}
              </TabsTrigger>
              <TabsTrigger value="agent" className="text-xs">
                {t("dialog.sshAgent", "SSH Agent")}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="network" className="mt-3 border-0 outline-none">
              <SessionNetworkSection
                proxyId={proxyId}
                setProxyId={setProxyId}
                proxies={proxies}
                jumpHostId={jumpHostId}
                setJumpHostId={setJumpHostId}
                jumpHostOptions={jumpHostOptions}
              />
            </TabsContent>

            <TabsContent value="agent" className="mt-3 border-0 outline-none">
              <div className="space-y-3 rounded-lg border bg-accent/25 p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-xs font-medium">
                      {t("dialog.sshAgentForwarding", "Allow agent forwarding")}
                    </div>
                    <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t(
                        "dialog.sshAgentForwardingDesc",
                        "Allow remote processes to use signing capabilities from the selected forwarding sources.",
                      )}
                    </p>
                  </div>
                  <Switch
                    checked={agentForwardingConfig.enabled}
                    onCheckedChange={(enabled) =>
                      setAgentForwardingConfig({ ...agentForwardingConfig, enabled })
                    }
                  />
                </div>
                {agentForwardingConfig.enabled && (
                  <>
                    <div className="space-y-2 border-t pt-3">
                      <div className="text-xs font-medium">
                        {t("dialog.sshAgentForwardingSources", "Forwarding sources")}
                      </div>
                      <label className="flex items-center justify-between gap-3 text-xs">
                        <span>
                          {t("dialog.sshAgentExternalSource", "External SSH Agent")}
                          {t("dialog.sshAgentEndpointList", " (custom endpoints)")}
                        </span>
                        <Switch
                          checked={agentForwardingConfig.sources.external_agent}
                          onCheckedChange={(external_agent) =>
                            setAgentForwardingConfig({
                              ...agentForwardingConfig,
                              sources: {
                                ...agentForwardingConfig.sources,
                                external_agent,
                                external_agent_endpoints:
                                  external_agent &&
                                  agentForwardingConfig.sources.external_agent_endpoints.length ===
                                    0
                                    ? [defaultForwardingEndpoint()]
                                    : agentForwardingConfig.sources.external_agent_endpoints,
                              },
                            })
                          }
                        />
                      </label>
                      {agentForwardingConfig.sources.external_agent && (
                        <div className="space-y-2 rounded-md border border-dashed p-2">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[0.6875rem] text-muted-foreground">
                              {t(
                                "dialog.sshAgentEndpointListDesc",
                                "Use more than one local Agent endpoint; identities are merged in this order.",
                              )}
                              <span className="mt-0.5 block">
                                {t(
                                  "dialog.sshAgentEndpointGpgHint",
                                  "For gpg-agent, use its SSH-compatible socket, not a regular GPG Assuan socket.",
                                )}
                              </span>
                            </span>
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              className="h-7 shrink-0 px-2 text-xs"
                              disabled={
                                agentForwardingConfig.sources.external_agent_endpoints.length >=
                                  MAX_SSH_AGENT_FORWARDING_ENDPOINTS ||
                                !!agentForwardingEndpointError ||
                                !canAddForwardingEndpoint
                              }
                              title={
                                agentForwardingEndpointError ||
                                (agentForwardingConfig.sources.external_agent_endpoints.length >=
                                MAX_SSH_AGENT_FORWARDING_ENDPOINTS
                                  ? t(
                                      "dialog.sshAgentEndpointLimit",
                                      "A maximum of 16 custom endpoints is supported.",
                                    )
                                  : undefined)
                              }
                              onClick={addForwardingEndpoint}
                            >
                              <MdAdd className="mr-1 text-sm" />
                              {t("dialog.sshAgentAddEndpoint", "Add endpoint")}
                            </Button>
                          </div>
                          {agentForwardingConfig.sources.external_agent_endpoints.length === 0 ? (
                            <div className="rounded border border-dashed px-2 py-2 text-[0.6875rem] text-muted-foreground">
                              {t(
                                "dialog.sshAgentEndpointListEmpty",
                                "No custom endpoints are configured.",
                              )}
                            </div>
                          ) : (
                            agentForwardingConfig.sources.external_agent_endpoints.map(
                              (endpoint, index) => (
                                <div
                                  key={`${index}-${endpoint.type}`}
                                  className="space-y-2 rounded-md border bg-background/40 p-2"
                                >
                                  <div className="flex items-center gap-1.5">
                                    <Select
                                      value={endpoint.type}
                                      onValueChange={(type) => {
                                        if (type === "auto") {
                                          updateForwardingEndpoint(index, { type: "auto" });
                                        } else if (type === "environment") {
                                          updateForwardingEndpoint(index, {
                                            type: "environment",
                                            variable: "SSH_AUTH_SOCK",
                                          });
                                        } else if (type === "unix_socket") {
                                          updateForwardingEndpoint(index, {
                                            type: "unix_socket",
                                            path: "",
                                          });
                                        } else if (type === "pageant") {
                                          updateForwardingEndpoint(index, { type: "pageant" });
                                        } else if (type === "windows_open_ssh") {
                                          updateForwardingEndpoint(index, {
                                            type: "windows_open_ssh",
                                          });
                                        }
                                      }}
                                    >
                                      <SelectTrigger className="h-8 min-w-0 flex-1 text-xs font-normal">
                                        <SelectValue />
                                      </SelectTrigger>
                                      <SelectContent>
                                        {availableAgentEndpointTypes.includes("auto") && (
                                          <SelectItem value="auto">
                                            {t("dialog.sshAgentAuto", "Automatic")}
                                          </SelectItem>
                                        )}
                                        {availableForwardingEndpointTypes.includes(
                                          "environment",
                                        ) && (
                                          <SelectItem value="environment">
                                            {t(
                                              "dialog.sshAgentEnvironment",
                                              "Environment variable",
                                            )}
                                          </SelectItem>
                                        )}
                                        {availableForwardingEndpointTypes.includes(
                                          "unix_socket",
                                        ) && (
                                          <SelectItem value="unix_socket">
                                            {t("dialog.sshAgentUnixSocket", "Unix domain socket")}
                                          </SelectItem>
                                        )}
                                        {availableForwardingEndpointTypes.includes("pageant") && (
                                          <SelectItem value="pageant">
                                            {t("dialog.sshAgentPageant", "Pageant")}
                                          </SelectItem>
                                        )}
                                        {availableForwardingEndpointTypes.includes(
                                          "windows_open_ssh",
                                        ) && (
                                          <SelectItem value="windows_open_ssh">
                                            {t(
                                              "dialog.sshAgentWindowsOpenSsh",
                                              "Windows OpenSSH Agent",
                                            )}
                                          </SelectItem>
                                        )}
                                      </SelectContent>
                                    </Select>
                                    <Button
                                      type="button"
                                      variant="ghost"
                                      size="icon"
                                      className="h-7 w-7"
                                      disabled={index === 0}
                                      onClick={() => moveForwardingEndpoint(index, -1)}
                                      aria-label={t(
                                        "dialog.sshAgentMoveEndpointUp",
                                        "Move endpoint up",
                                      )}
                                    >
                                      <MdKeyboardArrowUp className="text-base" />
                                    </Button>
                                    <Button
                                      type="button"
                                      variant="ghost"
                                      size="icon"
                                      className="h-7 w-7"
                                      disabled={
                                        index ===
                                        agentForwardingConfig.sources.external_agent_endpoints
                                          .length -
                                          1
                                      }
                                      onClick={() => moveForwardingEndpoint(index, 1)}
                                      aria-label={t(
                                        "dialog.sshAgentMoveEndpointDown",
                                        "Move endpoint down",
                                      )}
                                    >
                                      <MdKeyboardArrowDown className="text-base" />
                                    </Button>
                                    <Button
                                      type="button"
                                      variant="ghost"
                                      size="icon"
                                      className="h-7 w-7 text-destructive hover:text-destructive"
                                      onClick={() => removeForwardingEndpoint(index)}
                                      aria-label={t(
                                        "dialog.sshAgentRemoveEndpoint",
                                        "Remove endpoint",
                                      )}
                                    >
                                      <MdDeleteOutline className="text-base" />
                                    </Button>
                                  </div>
                                  {endpoint.type === "environment" && (
                                    <Input
                                      value={endpoint.variable}
                                      onChange={(event) =>
                                        updateForwardingEndpoint(index, {
                                          type: "environment",
                                          variable: event.target.value,
                                        })
                                      }
                                      placeholder="SSH_AUTH_SOCK"
                                      className="h-8 text-xs"
                                    />
                                  )}
                                  {endpoint.type === "unix_socket" && (
                                    <Input
                                      value={endpoint.path}
                                      onChange={(event) =>
                                        updateForwardingEndpoint(index, {
                                          type: "unix_socket",
                                          path: event.target.value,
                                        })
                                      }
                                      placeholder="/tmp/ssh-XXXXXX/agent.YYYY"
                                      className="h-8 text-xs"
                                    />
                                  )}
                                </div>
                              ),
                            )
                          )}
                        </div>
                      )}
                      {agentForwardingEndpointError && (
                        <p className="text-[0.6875rem] text-destructive" role="alert">
                          {agentForwardingEndpointError}
                        </p>
                      )}
                      <label className="flex items-center justify-between gap-3 text-xs">
                        <span>{t("dialog.sshAgentStoredKeysSource", "NyaTerm stored keys")}</span>
                        <Switch
                          checked={agentForwardingConfig.sources.stored_keys}
                          onCheckedChange={(stored_keys) =>
                            updateForwardingSources({ stored_keys })
                          }
                        />
                      </label>
                    </div>
                    <div className="space-y-2 border-t pt-3">
                      <Label className="text-xs font-medium text-foreground/80">
                        {t("dialog.sshAgentForwardingPolicy", "Identity policy")}
                      </Label>
                      <Select
                        value={agentForwardingConfig.policy.mode}
                        onValueChange={(mode) => {
                          if (mode === "allowlist") {
                            setAgentForwardingConfig({
                              ...agentForwardingConfig,
                              policy: { mode: "allowlist", fingerprints: [] },
                            });
                          }
                          if (mode === "all") {
                            setShowAgentAllowAllWarning(true);
                          }
                        }}
                      >
                        <SelectTrigger className="h-8 text-xs font-normal">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="allowlist">
                            {t("dialog.sshAgentAllowlist", "Allowlist")}
                          </SelectItem>
                          <SelectItem value="all">
                            {t("dialog.sshAgentAllowAll", "Allow all identities")}
                          </SelectItem>
                        </SelectContent>
                      </Select>
                      {agentForwardingConfig.policy.mode === "allowlist" && (
                        <div className="space-y-2">
                          <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                            {agentForwardingConfig.policy.fingerprints.length === 0
                              ? t(
                                  "dialog.sshAgentAllowlistEmpty",
                                  "The allowlist is empty. Forwarding remains enabled but exposes no identities until you authorize fingerprints.",
                                )
                              : t("dialog.sshAgentAllowlistCount", {
                                  count: agentForwardingConfig.policy.fingerprints.length,
                                  defaultValue: `${agentForwardingConfig.policy.fingerprints.length} identities are authorized.`,
                                })}
                          </p>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={
                              !agentForwardingConfig.sources.external_agent &&
                              !agentForwardingConfig.sources.stored_keys
                            }
                            onClick={() => setShowAgentIdentityPicker(true)}
                          >
                            <MdSettings className="mr-1.5 text-sm" />
                            {t("dialog.sshAgentManageAllowlist", "Choose identities")}
                          </Button>
                        </div>
                      )}
                    </div>
                  </>
                )}
                <p className="mt-2 text-[0.6875rem] leading-relaxed text-amber-600 dark:text-amber-300">
                  {t(
                    "dialog.sshAgentForwardingWarning",
                    "Agent forwarding exposes signing capability from the selected local Agent endpoints and NyaTerm stored keys to remote processes. Enable it only for trusted servers. Private key material is never sent to the remote server; endpoint and forwarding policy remain device-local.",
                  )}
                </p>
              </div>
            </TabsContent>

            <TabsContent value="two-factor" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="space-y-3">
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.selectOtp")}
                    </Label>
                    <div className="mt-1">
                      <AdvancedCombobox
                        value={otpId}
                        options={otpOptions}
                        placeholder={t("dialog.noOtp")}
                        searchPlaceholder={t("dialog.searchOtpEntries")}
                        emptyText={t("dialog.noOtpEntries")}
                        missingSelectionLabel={t("dialog.selectedItemMissing")}
                        clearLabel={t("dialog.noOtp")}
                        onChange={(id) => {
                          setOtpId(id);
                          if (!id) setAutoFillOtp(false);
                        }}
                      />
                    </div>
                  </div>

                  <div className="rounded-md border border-dashed bg-background/70 px-3 py-2">
                    <div className="flex items-center justify-between gap-3">
                      <div className="min-w-0">
                        <div className="text-xs font-medium">{t("dialog.autoFillOtp")}</div>
                        <div className="text-[0.625rem] text-muted-foreground">
                          {otpId ? t("dialog.twoFactorAuth") : t("dialog.noOtp")}
                        </div>
                      </div>
                      <Switch
                        checked={otpId ? autoFillOtp : false}
                        onCheckedChange={setAutoFillOtp}
                        disabled={!otpId}
                      />
                    </div>
                  </div>
                </div>
              </div>
            </TabsContent>
          </Tabs>
          <Tabs defaultValue="post-login" className="w-full">
            <TabsList className="grid h-8 w-full grid-cols-5 pointer-events-auto">
              <TabsTrigger value="post-login" className="text-xs">
                {t("dialog.commandExecution")}
              </TabsTrigger>
              <TabsTrigger value="terminal" className="text-xs">
                {t("dialog.encodingSettings")}
              </TabsTrigger>
              <TabsTrigger value="sftp" className="text-xs">
                SFTP
              </TabsTrigger>
              <TabsTrigger value="x11" className="text-xs">
                {t("dialog.x11Forwarding")}
              </TabsTrigger>
              <TabsTrigger value="backspace" className="text-xs">
                {t("dialog.backspaceMode", "Backspace Mode")}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="post-login" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 space-y-0.5">
                    <div className="text-xs font-medium">{t("dialog.postLoginCommand")}</div>
                    <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t("dialog.postLoginCommandDesc")}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Switch checked={postLoginEnabled} onCheckedChange={setPostLoginEnabled} />
                    <span className="text-xs text-muted-foreground">
                      {t("dialog.enabled", "Enabled")}
                    </span>
                  </div>
                </div>

                <div
                  className={cn(
                    "mt-3 grid gap-3 sm:grid-cols-[minmax(0,1fr)_8rem]",
                    !postLoginEnabled && "pointer-events-none opacity-50",
                  )}
                >
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.postLoginCommandContent")}
                    </Label>
                    <Textarea
                      rows={4}
                      className="mt-1 min-h-24 resize-y font-mono text-xs"
                      placeholder={"cd /opt/app\nclear"}
                      value={postLoginCommand}
                      onChange={(event) => setPostLoginCommand(event.target.value)}
                      disabled={!postLoginEnabled}
                    />
                  </div>
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.postLoginDelay")}
                    </Label>
                    <div className="mt-1 flex items-center gap-2">
                      <NumberInput
                        className="min-w-0 flex-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
                        value={postLoginDelayMs}
                        onChange={setPostLoginDelayMs}
                        min={minPostLoginDelayMs}
                        max={maxPostLoginDelayMs}
                        step={100}
                        disabled={!postLoginEnabled}
                      />
                      <span className="shrink-0 text-[0.625rem] text-muted-foreground">ms</span>
                    </div>
                  </div>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="terminal" className="mt-3 border-0 outline-none">
              <div className="space-y-3 rounded-lg border bg-accent/25 p-3">
                <div className="grid gap-3 sm:grid-cols-2">
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.sshProfile")}
                    </Label>
                    <Select
                      value={sshProfile}
                      onValueChange={(value) => setSshProfile(value as SshProfile)}
                    >
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="standard">{t("dialog.sshProfileStandard")}</SelectItem>
                        <SelectItem value="network_device">
                          {t("dialog.sshProfileNetworkDevice")}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {networkDeviceProfile
                        ? t("dialog.sshProfileNetworkDeviceDesc")
                        : t("dialog.sshProfileStandardDesc")}
                    </p>
                  </div>
                  <div>
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.sshTerminalType")}
                    </Label>
                    <Select
                      value={sshTerminalType}
                      onValueChange={(value) =>
                        setSshTerminalType(value as SshTerminalTypeSelection)
                      }
                    >
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="default">
                          {t("dialog.sshTerminalTypeDefault", { value: defaultTerminalType })}
                        </SelectItem>
                        <SelectItem value="xterm-256color">xterm-256color</SelectItem>
                        <SelectItem value="xterm">xterm</SelectItem>
                        <SelectItem value="vt100">vt100</SelectItem>
                        <SelectItem value="vt220">vt220</SelectItem>
                        <SelectItem value="ansi">ansi</SelectItem>
                        <SelectItem value="linux">linux</SelectItem>
                      </SelectContent>
                    </Select>
                    <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t("dialog.sshTerminalTypeDesc")}
                    </p>
                  </div>
                </div>
                {networkDeviceProfile && (
                  <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[0.6875rem] leading-relaxed text-amber-800 dark:text-amber-200">
                    {t("dialog.sshProfileNetworkDeviceRuntimeDesc")}
                  </div>
                )}
                <div className="max-w-md">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("connection.encoding")}
                  </Label>
                  <Select value={encoding} onValueChange={setEncoding}>
                    <SelectTrigger className="mt-1 h-8 w-full text-xs">
                      <SelectValue placeholder={t("connection.encodingFollowGlobal")} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="global">{t("connection.encodingFollowGlobal")}</SelectItem>
                      <SelectItem value="UTF-8">UTF-8</SelectItem>
                      <SelectItem value="GBK">GBK</SelectItem>
                      <SelectItem value="GB2312">GB2312</SelectItem>
                      <SelectItem value="GB18030">GB18030</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 space-y-0.5">
                    <div className="text-xs font-medium">{t("dialog.remoteDynamicTabTitle")}</div>
                    <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t("dialog.remoteDynamicTabTitleDesc")}
                    </p>
                  </div>
                  <Switch
                    className="mt-0.5"
                    size="sm"
                    checked={remoteDynamicTabTitle}
                    onCheckedChange={setRemoteDynamicTabTitle}
                  />
                </div>
                <ConnectionRecordingSettings
                  useGlobal={recordingUseGlobal}
                  onUseGlobalChange={setRecordingUseGlobal}
                  autoStart={recordingAutoStart}
                  onAutoStartChange={setRecordingAutoStart}
                  mode={recordingMode}
                  onModeChange={setRecordingMode}
                />
              </div>
            </TabsContent>

            <TabsContent value="sftp" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 space-y-0.5">
                    <div className="text-xs font-medium">{t("dialog.sftpAdvanced")}</div>
                    <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t("dialog.sftpAdvancedDesc")}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Switch
                      checked={sftpSettings.enabled}
                      disabled={networkDeviceProfile}
                      onCheckedChange={(enabled) =>
                        setSftpSettings({
                          ...sftpSettings,
                          enabled,
                        })
                      }
                    />
                    <span className="text-xs text-muted-foreground">
                      {t("dialog.enabled", "Enabled")}
                    </span>
                  </div>
                </div>

                <div className="mt-3 max-w-md">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.sftpCwdFollowMode")}
                  </Label>
                  <Select
                    disabled={sftpDisabled}
                    value={sftpSettings.cwd_follow_mode}
                    onValueChange={(cwd_follow_mode) =>
                      setSftpSettings({
                        ...sftpSettings,
                        cwd_follow_mode: cwd_follow_mode as SftpSettings["cwd_follow_mode"],
                      })
                    }
                  >
                    <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="off">{t("dialog.sftpCwdFollowOff")}</SelectItem>
                      <SelectItem value="shell_integration">
                        {t("dialog.sftpCwdFollowShellIntegration")}
                      </SelectItem>
                      <SelectItem value="rc_file">{t("dialog.sftpCwdFollowRcFile")}</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {sftpSettings.cwd_follow_mode === "off"
                      ? t("dialog.sftpCwdFollowOffDesc")
                      : sftpSettings.cwd_follow_mode === "rc_file"
                        ? t("dialog.sftpCwdFollowRcFileDesc")
                        : t("dialog.sftpCwdFollowShellIntegrationDesc")}
                  </p>
                </div>
                <div className="mt-3 max-w-xs">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.sftpShellDetectionTimeout")}
                  </Label>
                  <div className="mt-1 flex items-center gap-2">
                    <NumberInput
                      className="min-w-0 flex-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
                      value={
                        sftpSettings.shell_detection_timeout_ms ??
                        DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS
                      }
                      onChange={(shell_detection_timeout_ms) =>
                        setSftpSettings({
                          ...sftpSettings,
                          shell_detection_timeout_ms,
                        })
                      }
                      min={MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS}
                      max={MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS}
                      step={100}
                      disabled={sftpDisabled || sftpSettings.cwd_follow_mode === "off"}
                    />
                    <span className="shrink-0 text-[0.625rem] text-muted-foreground">ms</span>
                  </div>
                  <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t("dialog.sftpShellDetectionTimeoutDesc")}
                  </p>
                </div>
                <div className="mt-3 max-w-md">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.sftpFilenameEncoding")}
                  </Label>
                  <Select
                    disabled={sftpDisabled}
                    value={sftpSettings.filename_encoding || "terminal"}
                    onValueChange={(filename_encoding) =>
                      setSftpSettings({
                        ...sftpSettings,
                        filename_encoding:
                          filename_encoding === "terminal" ? "" : filename_encoding,
                      })
                    }
                  >
                    <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="terminal">
                        {t("dialog.sftpFilenameEncodingFollowTerminal")}
                      </SelectItem>
                      <SelectItem value="UTF-8">UTF-8</SelectItem>
                      <SelectItem value="GBK">GBK</SelectItem>
                      <SelectItem value="GB2312">GB2312</SelectItem>
                      <SelectItem value="GB18030">GB18030</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t("dialog.sftpFilenameEncodingDesc")}
                  </p>
                </div>
                <div className="mt-3 max-w-md">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.sftpPipelineDepth")}
                  </Label>
                  <Select
                    disabled={sftpDisabled}
                    value={sftpSettings.pipeline_depth?.toString() ?? "auto"}
                    onValueChange={(value) =>
                      setSftpSettings({
                        ...sftpSettings,
                        pipeline_depth: value === "auto" ? undefined : Number(value),
                      })
                    }
                  >
                    <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="auto">{t("dialog.sftpPipelineDepthAuto")}</SelectItem>
                      {[4, 8, 16, 32, 64].map((depth) => (
                        <SelectItem key={depth} value={depth.toString()}>
                          {depth}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t("dialog.sftpPipelineDepthDesc")}
                  </p>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="x11" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 space-y-0.5">
                    <div className="text-xs font-medium">{t("dialog.x11Forwarding")}</div>
                    <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                      {t("dialog.x11ForwardingDesc")}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Switch checked={x11Forwarding} onCheckedChange={setX11Forwarding} />
                    <span className="text-xs text-muted-foreground">
                      {t("dialog.enabled", "Enabled")}
                    </span>
                  </div>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="backspace" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">
                    {t("dialog.backspaceMode", "Backspace Mode")}
                  </div>
                  <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t("dialog.sshBackspaceModeDesc")}
                  </p>
                </div>
                <div className="mt-3 max-w-xs">
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.backspaceMode", "Backspace Mode")}
                  </Label>
                  <Select value={backspaceMode} onValueChange={setBackspaceMode}>
                    <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="del">{t("dialog.backspaceDel", "DEL (0x7F)")}</SelectItem>
                      <SelectItem value="ctrl_h">
                        {t("dialog.backspaceCtrlH", "Ctrl+H (BS)")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
            </TabsContent>
          </Tabs>
          <div className="rounded-lg border bg-accent/25 p-3">
            <div className="space-y-0.5">
              <div className="text-xs font-medium">{t("dialog.sshAlgorithms")}</div>
              <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                {t("dialog.sshAlgorithmsDesc")}
              </p>
            </div>
            <div className="mt-3 max-w-xs">
              <Label className="text-xs font-medium text-foreground/80">
                {t("dialog.algorithmMode")}
              </Label>
              <Select
                value={sshAlgorithms.mode}
                onValueChange={(value) =>
                  setAlgorithmMode(value as SshAlgorithmPreferences["mode"])
                }
              >
                <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="compatible">{t("dialog.algorithmModeCompatible")}</SelectItem>
                  <SelectItem value="secure">{t("dialog.algorithmModeSecure")}</SelectItem>
                  <SelectItem value="custom">{t("dialog.algorithmModeCustom")}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <p className="mt-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
              {sshAlgorithms.mode === "secure"
                ? t("dialog.algorithmModeSecureDesc")
                : sshAlgorithms.mode === "custom"
                  ? t("dialog.algorithmModeCustomDesc")
                  : t("dialog.algorithmModeCompatibleDesc")}
            </p>
            {sshAlgorithms.mode === "custom" && supportedAlgorithms && (
              <Tabs defaultValue="kex" className="mt-3 w-full">
                <TabsList className="grid h-8 w-full grid-cols-4 pointer-events-auto">
                  <TabsTrigger value="kex" className="min-w-0 px-1 text-[0.6875rem]">
                    <span className="truncate">{t("dialog.algorithmKexTab")}</span>
                  </TabsTrigger>
                  <TabsTrigger value="ciphers" className="min-w-0 px-1 text-[0.6875rem]">
                    <span className="truncate">{t("dialog.algorithmCiphersTab")}</span>
                  </TabsTrigger>
                  <TabsTrigger value="macs" className="min-w-0 px-1 text-[0.6875rem]">
                    <span className="truncate">{t("dialog.algorithmMacsTab")}</span>
                  </TabsTrigger>
                  <TabsTrigger value="host-keys" className="min-w-0 px-1 text-[0.6875rem]">
                    <span className="truncate">{t("dialog.algorithmHostKeysTab")}</span>
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="kex" className="mt-1 border-0 outline-none">
                  <AlgorithmOrderList
                    options={supportedAlgorithms.kex}
                    value={sshAlgorithms.kex}
                    onChange={(value) => setAlgorithmList("kex", value)}
                  />
                </TabsContent>
                <TabsContent value="ciphers" className="mt-1 border-0 outline-none">
                  <AlgorithmOrderList
                    options={supportedAlgorithms.ciphers}
                    value={sshAlgorithms.ciphers}
                    onChange={(value) => setAlgorithmList("ciphers", value)}
                  />
                </TabsContent>
                <TabsContent value="macs" className="mt-1 border-0 outline-none">
                  <AlgorithmOrderList
                    options={supportedAlgorithms.macs}
                    value={sshAlgorithms.macs}
                    onChange={(value) => setAlgorithmList("macs", value)}
                  />
                </TabsContent>
                <TabsContent value="host-keys" className="mt-1 border-0 outline-none">
                  <AlgorithmOrderList
                    options={supportedAlgorithms.host_keys}
                    value={sshAlgorithms.host_keys}
                    onChange={(value) => setAlgorithmList("host_keys", value)}
                  />
                </TabsContent>
              </Tabs>
            )}
            {sshAlgorithms.mode === "custom" && !supportedAlgorithms && (
              <div className="mt-3 rounded-md border border-dashed bg-background/70 px-3 py-2 text-[0.6875rem] text-muted-foreground">
                {t("dialog.algorithmLoading")}
              </div>
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>

      <Dialog
        disablePointerDismissal
        open={showKeyManagement}
        onOpenChange={(open) => {
          setShowKeyManagement(open);
          if (!open) {
            void loadSshKeys();
          }
        }}
      >
        <DialogContent
          className="flex! w-[min(42rem,calc(100vw-3rem))] max-w-none max-h-[76vh]! min-h-0 flex-col overflow-hidden!"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>{t("settings.keyManagement")}</DialogTitle>
            <DialogDescription className="sr-only">{t("settings.keyManagement")}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-1 terminal-scroll">
            <KeyManagementTab />
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        disablePointerDismissal
        open={showPasswordManagement}
        onOpenChange={(open) => {
          setShowPasswordManagement(open);
          if (!open) {
            void loadPasswords();
          }
        }}
      >
        <DialogContent
          className="flex! w-[min(27rem,calc(100vw-3rem))] max-w-none max-h-[76vh]! min-h-0 flex-col overflow-hidden!"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>{t("passwordManager.title")}</DialogTitle>
            <DialogDescription className="sr-only">{t("passwordManager.title")}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-1 terminal-scroll">
            <PasswordManagementTab
              secretsUnlocked={passwordSecretsUnlocked}
              onUnlockSecrets={onUnlockPasswordSecrets}
              onLockSecrets={onLockPasswordSecrets}
            />
          </div>
        </DialogContent>
      </Dialog>
      <Dialog open={showAgentIdentityPicker} onOpenChange={setShowAgentIdentityPicker}>
        <DialogContent className="w-[min(42rem,calc(100vw-2rem))]">
          <DialogHeader>
            <DialogTitle>{t("dialog.sshAgentManageAllowlist", "Choose identities")}</DialogTitle>
            <DialogDescription>
              {t(
                "dialog.sshAgentAllowlistDescription",
                "Select fingerprints from the merged external Agent and NyaTerm stored-key identities. An unchecked identity is never exposed to the remote server.",
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[min(30rem,50vh)] space-y-2 overflow-y-auto pr-1 terminal-scroll">
            {agentIdentityLoading ? (
              <div className="rounded-md border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
                {t("dialog.sshAgentIdentitiesLoading", "Loading Agent identities…")}
              </div>
            ) : agentIdentityLoadError ? (
              <div className="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                {t(
                  "dialog.sshAgentIdentityLoadError",
                  "Could not load forwarding identities. Check the Agent endpoint configuration and application environment.",
                )}
                <div className="mt-1 break-words font-mono text-[0.625rem] opacity-80">
                  {agentIdentityLoadError}
                </div>
              </div>
            ) : agentIdentities.length === 0 && agentEndpointErrors.length === 0 ? (
              <div className="rounded-md border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
                {t(
                  "dialog.sshAgentIdentitiesEmpty",
                  "No identities are currently available. Check the selected Agent endpoint or add a stored key.",
                )}
              </div>
            ) : (
              <>
                {agentEndpointErrors.map((error) => (
                  <div
                    key={`${error.custom_endpoint_index}-${error.endpoint_type}-${error.code}`}
                    className="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive"
                  >
                    {t(
                      `dialog.sshAgentEndpointError.${error.code}`,
                      error.code === "connect_failed"
                        ? "Could not connect to forwarding Agent endpoint."
                        : "Could not enumerate identities from forwarding Agent endpoint.",
                    )}{" "}
                    <span className="font-mono text-[0.625rem]">
                      {`${t("dialog.sshAgentCustomEndpointLabel", "Custom endpoint")} #${
                        error.custom_endpoint_index + 1
                      }`}{" "}
                      ({error.endpoint_type})
                    </span>
                  </div>
                ))}
                {agentIdentityTruncated && (
                  <div className="rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-600 dark:text-amber-400">
                    {t(
                      "dialog.sshAgentIdentityPreviewTruncated",
                      "Only the first 1,024 identities that fit the SSH Agent protocol limits are shown and available for forwarding.",
                    )}
                  </div>
                )}
                {agentIdentities.map((identity) => {
                  const selected =
                    agentForwardingConfig.policy.mode === "allowlist" &&
                    agentForwardingConfig.policy.fingerprints.includes(identity.fingerprint);
                  return (
                    <label
                      key={identity.fingerprint}
                      className="flex cursor-pointer items-start gap-3 rounded-md border px-3 py-2 hover:bg-accent/40"
                    >
                      <Checkbox
                        checked={selected}
                        onCheckedChange={(checked) => {
                          if (agentForwardingConfig.policy.mode !== "allowlist") return;
                          const fingerprints = new Set(agentForwardingConfig.policy.fingerprints);
                          if (checked === true) {
                            if (fingerprints.size >= MAX_SSH_AGENT_FORWARDING_IDENTITIES) return;
                            fingerprints.add(identity.fingerprint);
                          } else {
                            fingerprints.delete(identity.fingerprint);
                          }
                          setAgentForwardingConfig({
                            ...agentForwardingConfig,
                            policy: {
                              mode: "allowlist",
                              fingerprints: [...fingerprints],
                            },
                          });
                        }}
                        disabled={
                          !selected &&
                          agentForwardingConfig.policy.mode === "allowlist" &&
                          agentForwardingConfig.policy.fingerprints.length >=
                            MAX_SSH_AGENT_FORWARDING_IDENTITIES
                        }
                        className="mt-0.5"
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-xs font-medium">
                          {identity.comment || identity.fingerprint}
                        </span>
                        <span className="mt-0.5 block break-all font-mono text-[0.625rem] text-muted-foreground">
                          {identity.fingerprint}
                        </span>
                        <span className="mt-1 block text-[0.625rem] text-muted-foreground">
                          {identity.source === "stored_key"
                            ? t("dialog.sshAgentStoredKeysSource", "NyaTerm stored keys")
                            : t("dialog.sshAgentExternalSource", "External SSH Agent")}
                          {identity.custom_endpoint_index !== undefined
                            ? ` #${identity.custom_endpoint_index + 1}`
                            : ""}
                        </span>
                      </span>
                    </label>
                  );
                })}
              </>
            )}
          </div>
          <DialogFooter className="sm:justify-between">
            <Button type="button" onClick={() => setShowAgentIdentityPicker(false)}>
              {t("dialog.done", "Done")}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={agentIdentityLoading}
              onClick={() => void loadAgentIdentities()}
            >
              <MdRefresh className="mr-1.5 text-sm" />
              {t("dialog.refresh", "Refresh")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <AlertDialog open={showAgentAllowAllWarning} onOpenChange={setShowAgentAllowAllWarning}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("dialog.sshAgentAllowAllWarningTitle", "Allow all Agent identities?")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                "dialog.sshAgentAllowAllWarning",
                "Remote processes may use every current and future identity provided by the selected forwarding sources to create signatures. This can include external hardware Agents and NyaTerm stored keys. Private keys are not transferred, but signing capability is exposed.",
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("dialog.cancel", "Cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setAgentForwardingConfig({
                  ...agentForwardingConfig,
                  policy: { mode: "all" },
                });
              }}
            >
              {t("dialog.sshAgentAllowAllConfirm", "Allow all")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

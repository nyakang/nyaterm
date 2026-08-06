import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronsUpDownIcon, Eye, EyeOff } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdCheck,
  MdChevronRight,
  MdClose,
  MdExpandMore,
  MdKeyboardArrowDown,
  MdKeyboardArrowUp,
  MdSettings,
} from "react-icons/md";
import { ConnectionCombobox, type ConnectionOption } from "@/components/network/shared";
import { KeyManagementTab } from "@/components/panel/security-auth/KeyManagementTab";
import { PasswordManagementTab } from "@/components/panel/security-auth/PasswordManagementTab";
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
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type {
  AlgorithmOption,
  OtpEntry,
  ProxyConfig,
  SavedPassword,
  SftpSettings,
  SshAlgorithmDefaults,
  SshAlgorithmPreferences,
  SshKey,
  SupportedSshAlgorithms,
} from "@/types/global";

const MASKED_PASSWORD_PLACEHOLDER = "••••••••";
const DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS = 3000;
const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS = 100;
const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS = 60_000;
export type SshAuthMode = "none" | "password" | "key";
type PasswordSource = "ask" | "direct" | "saved";

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
  sshAlgorithms: SshAlgorithmPreferences;
  setSshAlgorithms: (v: SshAlgorithmPreferences) => void;
  sftpSettings: SftpSettings;
  setSftpSettings: (v: SftpSettings) => void;
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

function formatProxySubtitle(proxy: ProxyConfig) {
  if (proxy.protocol === "proxycommand") {
    return [proxy.protocol.toUpperCase(), proxy.command].filter(Boolean).join(" · ");
  }
  return [`${proxy.protocol.toUpperCase()} ${proxy.host}:${proxy.port}`, proxy.username]
    .filter(Boolean)
    .join(" · ");
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
  sshAlgorithms,
  setSshAlgorithms,
  sftpSettings,
  setSftpSettings,
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

  const selectedKeyName = sshKeys.find((k) => k.id === keyId)?.name;
  const selectedPasswordName = savedPasswords.find((p) => p.id === passwordId)?.name;
  const proxyOptions = proxies.map((proxy) => ({
    id: proxy.id,
    label: proxy.name,
    searchText: [proxy.name, proxy.protocol, proxy.host, proxy.port, proxy.username, proxy.command]
      .filter(Boolean)
      .join(" "),
    subtitle: formatProxySubtitle(proxy),
  }));
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
          <TabsList className="grid w-full grid-cols-3 h-8 pointer-events-auto">
            <TabsTrigger value="none" className="text-xs">
              {t("dialog.noAuthentication", "None")}
            </TabsTrigger>
            <TabsTrigger value="password" className="text-xs">
              {t("dialog.password")}
            </TabsTrigger>
            <TabsTrigger value="key" className="text-xs">
              {t("dialog.privateKey")}
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
          <Tabs defaultValue="proxy" className="w-full">
            <TabsList className="grid h-8 w-full grid-cols-3 pointer-events-auto">
              <TabsTrigger value="proxy" className="text-xs">
                {t("dialog.proxySelect")}
              </TabsTrigger>
              <TabsTrigger value="jump-host" className="text-xs">
                {t("dialog.proxyJump")}
              </TabsTrigger>
              <TabsTrigger value="two-factor" className="text-xs">
                {t("dialog.twoFactorAuth")}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="proxy" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div>
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.proxySelect")}
                  </Label>
                  <div className="mt-1">
                    <AdvancedCombobox
                      value={proxyId}
                      options={proxyOptions}
                      placeholder={t("dialog.noProxy")}
                      searchPlaceholder={t("network.searchProxies")}
                      emptyText={t("network.noProxyConfigs")}
                      missingSelectionLabel={t("dialog.selectedItemMissing")}
                      clearLabel={t("dialog.noProxy")}
                      onChange={setProxyId}
                    />
                  </div>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="jump-host" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div>
                  <Label className="text-xs font-medium text-foreground/80">
                    {t("dialog.selectProxyJump")}
                  </Label>
                  <div className="mt-1">
                    <ConnectionCombobox
                      value={jumpHostId}
                      options={jumpHostOptions}
                      placeholder={t("dialog.noProxyJump")}
                      searchPlaceholder={t("network.searchConnections")}
                      emptyText={t("dialog.proxyJumpSshOnly")}
                      missingSelectionLabel={t("network.connectionMissing")}
                      clearLabel={t("dialog.noProxyJump")}
                      onChange={setJumpHostId}
                    />
                  </div>
                </div>
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
              <div className="rounded-lg border bg-accent/25 p-3">
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
                      disabled={sftpSettings.cwd_follow_mode === "off"}
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
    </div>
  );
}

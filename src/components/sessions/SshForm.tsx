import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronsUpDownIcon, Eye, EyeOff } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdCheck, MdChevronRight, MdClose, MdExpandMore, MdSettings } from "react-icons/md";
import { ConnectionCombobox, type ConnectionOption } from "@/components/dialog/network/shared";
import { KeyManagementTab } from "@/components/panel/security-auth/KeyManagementTab";
import { PasswordManagementTab } from "@/components/panel/security-auth/PasswordManagementTab";
import { Button } from "@/components/ui/button";
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
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type { OtpEntry, ProxyConfig, SavedPassword, SshKey } from "@/types/global";

const MASKED_PASSWORD_PLACEHOLDER = "••••••••";
export type SshAuthMode = "none" | "password" | "key";

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
  connectionId?: string;
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
  connectionId,
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
  const [passwordSource, setPasswordSource] = useState<"direct" | "saved">(
    passwordId ? "saved" : "direct",
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
    }
  }, [passwordId]);

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

  const selectedKeyName = sshKeys.find((k) => k.id === keyId)?.name;
  const selectedPasswordName = savedPasswords.find((p) => p.id === passwordId)?.name;
  const selectedProxy = proxies.find((proxy) => proxy.id === proxyId);
  const selectedJumpHost = jumpHostOptions.find((option) => option.connection.id === jumpHostId);
  const selectedOtpEntry = otpEntries.find((entry) => entry.id === otpId);
  const proxyOptions = proxies.map((proxy) => ({
    id: proxy.id,
    label: proxy.name,
    searchText: [proxy.name, proxy.protocol, proxy.host, proxy.port, proxy.username]
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
                const nextSource = value as "direct" | "saved";
                setPasswordSource(nextSource);
                if (nextSource === "direct") {
                  setPasswordId("");
                } else {
                  setPassword("");
                  setHasPassword(false);
                }
              }}
              className="mt-1 w-full"
            >
              <TabsList className="grid h-8 w-full grid-cols-2 pointer-events-auto">
                <TabsTrigger value="direct" className="text-xs">
                  {t("dialog.directPassword")}
                </TabsTrigger>
                <TabsTrigger value="saved" className="text-xs">
                  {t("dialog.savedPassword")}
                </TabsTrigger>
              </TabsList>

              <TabsContent value="direct" className="mt-3 border-0 outline-none">
                <Label className="text-xs font-medium text-foreground/80">
                  {t("dialog.password")}
                  <RequiredMark />
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
                    className="w-[var(--radix-popover-trigger-width)] min-w-[14rem] overflow-hidden p-0"
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
                className="w-[var(--radix-popover-trigger-width)] min-w-[14rem] overflow-hidden p-0"
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
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">{t("dialog.proxySelect")}</div>
                  <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {proxyId
                      ? (selectedProxy?.name ?? t("dialog.proxySelect"))
                      : t("dialog.noProxy")}
                  </p>
                </div>
                <div className="mt-3">
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
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">{t("dialog.proxyJump")}</div>
                  <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {jumpHostId
                      ? (selectedJumpHost?.subtitle ??
                        selectedJumpHost?.connection.name ??
                        t("dialog.selectProxyJump"))
                      : t("dialog.noProxyJump")}
                  </p>
                </div>
                <div className="mt-3">
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
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">{t("dialog.twoFactorAuth")}</div>
                  <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {otpId && selectedOtpEntry
                      ? formatOtpLabel(selectedOtpEntry)
                      : t("dialog.noOtp")}
                  </p>
                </div>
                <div className="mt-3 space-y-3">
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
        </CollapsibleContent>
      </Collapsible>

      <Dialog
        open={showKeyManagement}
        onOpenChange={(open) => {
          setShowKeyManagement(open);
          if (!open) {
            void loadSshKeys();
          }
        }}
      >
        <DialogContent className="max-w-2xl max-h-[80vh] overflow-hidden">
          <DialogHeader>
            <DialogTitle>{t("settings.keyManagement")}</DialogTitle>
            <DialogDescription className="sr-only">{t("settings.keyManagement")}</DialogDescription>
          </DialogHeader>
          <div className="overflow-y-auto pr-1">
            <KeyManagementTab />
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        open={showPasswordManagement}
        onOpenChange={(open) => {
          setShowPasswordManagement(open);
          if (!open) {
            void loadPasswords();
          }
        }}
      >
        <DialogContent
          className="w-[min(27rem,calc(100vw-3rem))] max-w-none max-h-[76vh] overflow-hidden"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogHeader>
            <DialogTitle>{t("passwordManager.title")}</DialogTitle>
            <DialogDescription className="sr-only">{t("passwordManager.title")}</DialogDescription>
          </DialogHeader>
          <div className="overflow-y-auto px-1 pb-1">
            <PasswordManagementTab />
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

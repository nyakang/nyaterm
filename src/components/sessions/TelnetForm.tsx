import { getCurrentWindow } from "@tauri-apps/api/window";
import { Eye, EyeOff } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdChevronRight, MdClose, MdSettings } from "react-icons/md";
import { PasswordManagementTab } from "@/components/panel/security-auth/PasswordManagementTab";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type { SavedPassword } from "@/types/global";

const MASKED_PASSWORD_PLACEHOLDER = "••••••••";
type TelnetEnterMode = "crlf" | "cr" | "lf";
type TelnetAuthMode = "none" | "password";
type PasswordSource = "direct" | "saved";

interface TelnetFormProps {
  host: string;
  setHost: (v: string) => void;
  port: number;
  setPort: (v: number) => void;
  username: string;
  setUsername: (v: string) => void;
  authType: TelnetAuthMode;
  setAuthType: (v: TelnetAuthMode) => void;
  passwordId: string;
  setPasswordId: (v: string) => void;
  password: string;
  setPassword: (v: string) => void;
  hasPassword: boolean;
  setHasPassword: (v: boolean) => void;
  backspaceMode: string;
  setBackspaceMode: (v: string) => void;
  rawTcpCli: boolean;
  setRawTcpCli: (v: boolean) => void;
  enterMode: TelnetEnterMode;
  setEnterMode: (v: TelnetEnterMode) => void;
  localEcho: boolean;
  setLocalEcho: (v: boolean) => void;
  localLineEdit: boolean;
  setLocalLineEdit: (v: boolean) => void;
  forceCharacterAtATime: boolean;
  setForceCharacterAtATime: (v: boolean) => void;
  sendNaws: boolean;
  setSendNaws: (v: boolean) => void;
  sendSga: boolean;
  setSendSga: (v: boolean) => void;
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

export function TelnetForm({
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
  backspaceMode,
  setBackspaceMode,
  rawTcpCli,
  setRawTcpCli,
  enterMode,
  setEnterMode,
  localEcho,
  setLocalEcho,
  localLineEdit,
  setLocalLineEdit,
  forceCharacterAtATime,
  setForceCharacterAtATime,
  sendNaws,
  setSendNaws,
  sendSga,
  setSendSga,
  connectionId,
  encoding,
  setEncoding,
  passwordSecretsUnlocked = false,
  onUnlockPasswordSecrets,
  onLockPasswordSecrets,
}: TelnetFormProps) {
  const { t } = useTranslation();
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [savedPasswords, setSavedPasswords] = useState<SavedPassword[]>([]);
  const [showPasswordManagement, setShowPasswordManagement] = useState(false);
  const [showDirectPassword, setShowDirectPassword] = useState(false);
  const [directPasswordLoading, setDirectPasswordLoading] = useState(false);
  const [passwordSource, setPasswordSource] = useState<PasswordSource>(
    passwordId ? "saved" : "direct",
  );

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
    } else {
      setPasswordSource("direct");
    }
  }, [passwordId]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged((event) => {
        if (event.payload) void loadPasswords();
      })
      .then((fn) => {
        unlisten = fn;
      });
    void loadPasswords();
    return () => {
      unlisten?.();
    };
  }, [loadPasswords]);

  const selectedPasswordName = savedPasswords.find((p) => p.id === passwordId)?.name;

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

  const renderSwitchRow = (
    label: string,
    description: string,
    checked: boolean,
    onCheckedChange: (checked: boolean) => void,
    disabled = false,
  ) => (
    <div className={cn("rounded-md border bg-background/70 px-3 py-2", disabled && "opacity-55")}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-0.5">
          <div className="text-xs font-medium">{label}</div>
          <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">{description}</p>
        </div>
        <Switch
          className="mt-0.5"
          checked={checked}
          onCheckedChange={onCheckedChange}
          disabled={disabled}
        />
      </div>
    </div>
  );

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
        <Label className="text-xs font-medium text-foreground/80">{t("dialog.username")}</Label>
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
          onValueChange={(value) => {
            const next = value as TelnetAuthMode;
            setAuthType(next);
            if (next === "none") {
              setPasswordId("");
              setPassword("");
              setHasPassword(false);
            }
          }}
          className="mt-1 w-full"
        >
          <TabsList className="grid h-8 w-full grid-cols-2 pointer-events-auto">
            <TabsTrigger value="none" className="text-xs">
              {t("dialog.noAuthentication", "None")}
            </TabsTrigger>
            <TabsTrigger value="password" className="text-xs">
              {t("dialog.password")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="none" className="mt-3 border-0 outline-none">
            <div className="rounded-md border border-dashed bg-accent/25 px-3 py-2 text-[0.6875rem] leading-relaxed text-muted-foreground">
              {t("dialog.telnetNoAuthenticationDescription")}
            </div>
          </TabsContent>

          <TabsContent value="password" className="mt-3 border-0 outline-none">
            <Label className="text-xs font-medium text-foreground/80">
              {t("dialog.passwordSource")}
            </Label>
            <Tabs
              value={passwordSource}
              onValueChange={(value) => {
                const next = value as PasswordSource;
                setPasswordSource(next);
                if (next === "direct") {
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
                      if (e.target.value) setHasPassword(false);
                    }}
                    disabled={directPasswordLoading}
                  />
                  {(password || hasPassword) && (
                    <button
                      type="button"
                      className="absolute right-7 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
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
                      className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground transition-colors hover:text-foreground"
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
                <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto]">
                  <div className="min-w-0">
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.savedPassword")}
                    </Label>
                    <Select
                      value={passwordId || "__none__"}
                      onValueChange={(value) => {
                        setPasswordId(value === "__none__" ? "" : value);
                        setPassword("");
                        setHasPassword(false);
                      }}
                    >
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue placeholder={t("dialog.selectPassword")}>
                          {selectedPasswordName || t("dialog.none")}
                        </SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__none__">{t("dialog.none")}</SelectItem>
                        {savedPasswords.map((entry) => (
                          <SelectItem key={entry.id} value={entry.id}>
                            {entry.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="mt-6 h-8 gap-1.5 text-xs"
                    onClick={() => setShowPasswordManagement(true)}
                  >
                    <MdSettings className="text-sm" />
                    {t("dialog.managePasswords")}
                  </Button>
                </div>
              </TabsContent>
            </Tabs>
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
        <CollapsibleContent className="mt-3">
          <Tabs defaultValue="input" className="w-full">
            <TabsList className="grid h-8 w-full grid-cols-3 pointer-events-auto">
              <TabsTrigger value="input" className="text-xs">
                {t("dialog.telnetInputSettings", "Input")}
              </TabsTrigger>
              <TabsTrigger value="terminal" className="text-xs">
                {t("dialog.encodingSettings")}
              </TabsTrigger>
              <TabsTrigger value="telnet" className="text-xs">
                {t("dialog.telnetCompatibility", "Compatibility")}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="input" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">
                    {t("dialog.telnetInputBehavior", "Key input")}
                  </div>
                </div>
                <div className="mt-3 grid grid-cols-2 gap-2 sm:gap-3">
                  <div className="min-w-0">
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.backspaceMode", "Backspace Mode")}
                    </Label>
                    <Select value={backspaceMode} onValueChange={setBackspaceMode}>
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="ctrl_h">
                          {t("dialog.backspaceCtrlH", "Ctrl+H (BS)")}
                        </SelectItem>
                        <SelectItem value="del">
                          {t("dialog.backspaceDel", "DEL (0x7F)")}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="min-w-0">
                    <Label className="text-xs font-medium text-foreground/80">
                      {t("dialog.telnetEnterMode", "Enter sends")}
                    </Label>
                    <Select
                      value={enterMode}
                      onValueChange={(value) => setEnterMode(value as TelnetEnterMode)}
                    >
                      <SelectTrigger className="mt-1 h-8 text-xs font-normal">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="crlf">CRLF (\r\n)</SelectItem>
                        <SelectItem value="cr">CR (\r)</SelectItem>
                        <SelectItem value="lf">LF (\n)</SelectItem>
                      </SelectContent>
                    </Select>
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

            <TabsContent value="telnet" className="mt-3 border-0 outline-none">
              <div className="rounded-lg border bg-accent/25 p-3">
                <div className="space-y-0.5">
                  <div className="text-xs font-medium">
                    {t("dialog.telnetCompatibility", "Compatibility")}
                  </div>
                  <p className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                    {t("dialog.telnetRawTcpCliDesc")}
                  </p>
                </div>

                <div className="mt-3 grid gap-2">
                  {renderSwitchRow(
                    t("dialog.telnetRawTcpCli", "Embedded debug port / Raw TCP CLI"),
                    t("dialog.telnetRawTcpCliLongDesc"),
                    rawTcpCli,
                    (checked) => {
                      setRawTcpCli(checked);
                      if (checked) setEnterMode("cr");
                    },
                  )}

                  <div className="grid gap-2 md:grid-cols-2">
                    {renderSwitchRow(
                      t("dialog.telnetLocalEcho", "Local Echo"),
                      t("dialog.telnetLocalEchoDesc"),
                      localEcho,
                      setLocalEcho,
                    )}
                    {renderSwitchRow(
                      t("dialog.telnetLocalLineEdit", "Local line editing / Send line on Enter"),
                      t("dialog.telnetLocalLineEditDesc"),
                      localLineEdit,
                      setLocalLineEdit,
                    )}
                    {renderSwitchRow(
                      t("dialog.telnetForceCharAtATime", "Force character-at-a-time"),
                      t("dialog.telnetForceCharAtATimeDesc"),
                      forceCharacterAtATime,
                      setForceCharacterAtATime,
                    )}
                    {renderSwitchRow(
                      t("dialog.telnetSendNaws", "Send NAWS"),
                      t("dialog.telnetSendNawsDesc"),
                      sendNaws,
                      setSendNaws,
                      rawTcpCli,
                    )}
                    {renderSwitchRow(
                      t("dialog.telnetSendSga", "Send SGA"),
                      t("dialog.telnetSendSgaDesc"),
                      sendSga,
                      setSendSga,
                      rawTcpCli,
                    )}
                  </div>
                </div>
              </div>
            </TabsContent>
          </Tabs>
        </CollapsibleContent>
      </Collapsible>

      <Dialog
        disablePointerDismissal
        open={showPasswordManagement}
        onOpenChange={(open) => {
          setShowPasswordManagement(open);
          if (!open) void loadPasswords();
        }}
      >
        <DialogContent
          className="!flex w-[min(27rem,calc(100vw-3rem))] max-w-none !max-h-[76vh] min-h-0 flex-col !overflow-hidden"
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

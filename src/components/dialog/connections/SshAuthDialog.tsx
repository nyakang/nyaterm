import { Eye, EyeOff, KeyRound, Plus, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { KeyManagementTab } from "@/components/panel/security-auth/KeyManagementTab";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import type { SavedPassword, SshKey } from "@/types/global";

export type SshAuthPromptReason =
  | "missing_password"
  | "password_rejected"
  | "key_passphrase_required"
  | "key_rejected_password_fallback"
  | "publickey_rejected"
  | "publickey_required";

export type SshAuthPromptKind = "password" | "passphrase" | "publickey" | "auth_method";

export interface SshAuthRequest {
  requestId: string;
  connectionId?: string | null;
  connectionName: string;
  host: string;
  port: number;
  username: string;
  reason: SshAuthPromptReason;
  promptKind: SshAuthPromptKind;
  availableMethods?: string[];
  currentAuthMode?: string;
  attempt: number;
  canSave: boolean;
  passwordId?: string | null;
  targetWindowLabel?: string | null;
}

type SaveMode = "none" | "connection" | "saved_password" | "key_passphrase";
type AuthMethod = "password" | "key";
type PasswordSource = "manual" | "saved";

interface SshAuthDialogProps {
  request: SshAuthRequest | null;
  onDone: (requestId: string) => void;
}

function defaultSaveMode(request: SshAuthRequest | null): SaveMode {
  if (!request?.canSave) return "none";
  return "none";
}

export function SshAuthDialog({ request, onDone }: SshAuthDialogProps) {
  const { t } = useTranslation();
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [saveMode, setSaveMode] = useState<SaveMode>("none");
  const [saveName, setSaveName] = useState("");
  const [authMethod, setAuthMethod] = useState<AuthMethod>("password");
  const [passwordSource, setPasswordSource] = useState<PasswordSource>("manual");
  const [sshKeys, setSshKeys] = useState<SshKey[]>([]);
  const [savedPasswords, setSavedPasswords] = useState<SavedPassword[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState("");
  const [selectedPasswordId, setSelectedPasswordId] = useState("");
  const [keyManagementOpen, setKeyManagementOpen] = useState(false);
  const [loadingKeys, setLoadingKeys] = useState(false);
  const [loadingPasswords, setLoadingPasswords] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const isPassphrase = request?.promptKind === "passphrase";
  const availableMethods = request?.availableMethods ?? [];
  const passwordAvailable =
    isPassphrase ||
    request?.promptKind === "password" ||
    availableMethods.includes("password") ||
    availableMethods.includes("keyboard-interactive");
  const publickeyAvailable =
    request?.promptKind === "publickey" ||
    request?.promptKind === "auth_method" ||
    availableMethods.includes("publickey");
  const showMethodTabs = !isPassphrase && passwordAvailable && publickeyAvailable;
  const keyOptions = useMemo(() => sshKeys.filter((key) => key.has_key_data !== false), [sshKeys]);
  const passwordOptions = useMemo(
    () => savedPasswords.filter((password) => password.has_password !== false),
    [savedPasswords],
  );

  const loadSshKeys = useCallback(async () => {
    setLoadingKeys(true);
    try {
      const keys = await invoke<SshKey[]>("get_ssh_keys");
      setSshKeys(keys);
      const firstUsableKey = keys.find((key) => key.has_key_data !== false);
      setSelectedKeyId((current) =>
        current && keys.some((key) => key.id === current) ? current : firstUsableKey?.id || "",
      );
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "ssh_auth.keys_load_failed",
        message: "Failed to load SSH keys for runtime auth",
        ids: request ? { request_id: request.requestId } : undefined,
        error,
      });
      setSshKeys([]);
      setSelectedKeyId("");
    } finally {
      setLoadingKeys(false);
    }
  }, [request]);

  const loadSavedPasswords = useCallback(async () => {
    setLoadingPasswords(true);
    try {
      const passwords = await invoke<SavedPassword[]>("get_saved_passwords");
      const usablePasswords = passwords.filter((password) => password.has_password !== false);
      setSavedPasswords(passwords);
      setSelectedPasswordId((current) => {
        if (current && usablePasswords.some((password) => password.id === current)) return current;
        if (
          request?.passwordId &&
          usablePasswords.some((password) => password.id === request.passwordId)
        ) {
          return request.passwordId;
        }
        return usablePasswords[0]?.id || "";
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "ssh_auth.passwords_load_failed",
        message: "Failed to load saved passwords for runtime auth",
        ids: request ? { request_id: request.requestId } : undefined,
        error,
      });
      setSavedPasswords([]);
      setSelectedPasswordId("");
    } finally {
      setLoadingPasswords(false);
    }
  }, [request]);

  useEffect(() => {
    if (!request) return;
    setSecret("");
    setShowSecret(false);
    setSaveMode(defaultSaveMode(request));
    setSaveName(`${request.connectionName} ${t("dialog.password")}`);
    setPasswordSource("manual");
    const nextMethod = publickeyAvailable && !passwordAvailable ? "key" : "password";
    setAuthMethod(nextMethod);
    setSelectedKeyId("");
    setSelectedPasswordId("");
    setSubmitting(false);
    if (publickeyAvailable) void loadSshKeys();
    if (passwordAvailable && !isPassphrase) void loadSavedPasswords();
    const timer = window.setTimeout(() => {
      if (nextMethod === "password" || isPassphrase) inputRef.current?.focus();
    }, 100);
    return () => window.clearTimeout(timer);
  }, [
    isPassphrase,
    loadSavedPasswords,
    loadSshKeys,
    passwordAvailable,
    publickeyAvailable,
    request,
    t,
  ]);

  const reasonText = useMemo(() => {
    if (!request) return "";
    if (request.reason === "password_rejected") return t("sshAuth.passwordRejected");
    if (request.reason === "key_passphrase_required") return t("sshAuth.keyPassphraseRequired");
    if (request.reason === "key_rejected_password_fallback")
      return t("sshAuth.keyRejectedFallback");
    if (request.reason === "publickey_rejected") return t("sshAuth.publickeyRejected");
    if (request.reason === "publickey_required") return t("sshAuth.publickeyRequired");
    return t("sshAuth.missingPassword");
  }, [request, t]);

  const activeMethod = isPassphrase ? "password" : authMethod;
  const usingSavedPassword =
    !isPassphrase && activeMethod === "password" && passwordSource === "saved";
  const canSubmit =
    !!request &&
    !submitting &&
    (activeMethod === "key"
      ? !!selectedKeyId
      : usingSavedPassword
        ? !!selectedPasswordId
        : !!secret);

  const handleSubmit = async () => {
    if (!request || !canSubmit) return;
    setSubmitting(true);
    try {
      const save =
        activeMethod === "key" || saveMode === "none"
          ? null
          : usingSavedPassword
            ? { kind: "connection" }
            : saveMode === "connection"
              ? { kind: "connection" }
              : saveMode === "key_passphrase"
                ? { kind: "key_passphrase" }
                : {
                    kind: "saved_password",
                    name: saveName.trim() || `${request.connectionName} ${t("dialog.password")}`,
                    passwordId: request.passwordId || undefined,
                  };
      const response =
        activeMethod === "key"
          ? { method: "key", keyId: selectedKeyId }
          : usingSavedPassword
            ? {
                method: "saved_password",
                passwordId: selectedPasswordId,
                save,
              }
            : {
                method: isPassphrase ? "passphrase" : "password",
                secret,
                save,
              };

      await invoke("submit_ssh_auth_response", {
        requestId: request.requestId,
        response,
      });
      logger.info({
        domain: "security.flow",
        event: "ssh_auth.response_submitted",
        message: "Submitted SSH credential response",
        ids: { request_id: request.requestId },
        data: {
          prompt_kind: request.promptKind,
          method: usingSavedPassword ? "saved_password" : activeMethod,
          save_mode: activeMethod === "key" ? "none" : saveMode,
        },
      });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "ssh_auth.response_submit_failed",
        message: "Failed to submit SSH credential response",
        ids: { request_id: request.requestId },
        error,
      });
    }
    onDone(request.requestId);
  };

  const handleCancel = async () => {
    if (!request) return;
    try {
      await invoke("cancel_ssh_auth_request", { requestId: request.requestId });
    } catch (error) {
      logger.error({
        domain: "security.flow",
        event: "ssh_auth.request_cancel_failed",
        message: "Failed to cancel SSH credential request",
        ids: { request_id: request.requestId },
        error,
      });
    }
    onDone(request.requestId);
  };

  return (
    <>
      <Dialog
        disablePointerDismissal
        open={!!request}
        onOpenChange={(open) => {
          if (!open) void handleCancel();
        }}
      >
        <DialogContent
          className="w-[min(28rem,calc(100vw-2rem))] max-w-none overflow-x-hidden"
          onKeyDown={(event) => {
            if (event.key === "Enter" && canSubmit) void handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle className="pr-6 text-sm">{t("sshAuth.title")}</DialogTitle>
            <DialogDescription className="break-all pr-6 text-xs leading-relaxed">
              {t("sshAuth.description", { name: request?.connectionName })}
            </DialogDescription>
          </DialogHeader>

          <div className="min-w-0 space-y-3 py-2">
            <div className="min-w-0 rounded-md border bg-muted/30 px-3 py-2">
              <div
                className="break-all font-mono text-xs font-medium leading-relaxed"
                title={request ? `${request.username}@${request.host}:${request.port}` : undefined}
              >
                {request ? `${request.username}@${request.host}:${request.port}` : ""}
              </div>
              <div className="mt-1 text-xs leading-relaxed text-muted-foreground">{reasonText}</div>
            </div>

            {isPassphrase ? (
              <SecretInput
                inputRef={inputRef}
                label={t("sshAuth.passphrase")}
                value={secret}
                showValue={showSecret}
                onChange={setSecret}
                onToggleShow={() => setShowSecret((value) => !value)}
              />
            ) : showMethodTabs ? (
              <Tabs
                className="min-w-0"
                value={authMethod}
                onValueChange={(value) => setAuthMethod(value as AuthMethod)}
              >
                <TabsList className="grid w-full grid-cols-2">
                  <TabsTrigger value="password" className="min-w-0 text-xs">
                    <span className="truncate">{t("sshAuth.passwordMethod")}</span>
                  </TabsTrigger>
                  <TabsTrigger value="key" className="min-w-0 text-xs">
                    <span className="truncate">{t("sshAuth.keyMethod")}</span>
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="password" className="mt-3 min-w-0">
                  <PasswordAuthInput
                    inputRef={inputRef}
                    source={passwordSource}
                    onSourceChange={(source) => {
                      setPasswordSource(source);
                      setSaveMode("none");
                      if (source === "saved") {
                        setSecret("");
                        setShowSecret(false);
                      }
                    }}
                    secret={secret}
                    showSecret={showSecret}
                    onSecretChange={setSecret}
                    onToggleShow={() => setShowSecret((value) => !value)}
                    passwords={passwordOptions}
                    selectedPasswordId={selectedPasswordId}
                    loadingPasswords={loadingPasswords}
                    onPasswordChange={setSelectedPasswordId}
                    onRefreshPasswords={loadSavedPasswords}
                  />
                </TabsContent>
                <TabsContent value="key" className="mt-3 min-w-0">
                  <KeySelector
                    keys={keyOptions}
                    value={selectedKeyId}
                    loading={loadingKeys}
                    onChange={setSelectedKeyId}
                    onRefresh={loadSshKeys}
                    onAddKey={() => setKeyManagementOpen(true)}
                  />
                </TabsContent>
              </Tabs>
            ) : publickeyAvailable && !passwordAvailable ? (
              <KeySelector
                keys={keyOptions}
                value={selectedKeyId}
                loading={loadingKeys}
                onChange={setSelectedKeyId}
                onRefresh={loadSshKeys}
                onAddKey={() => setKeyManagementOpen(true)}
              />
            ) : (
              <PasswordAuthInput
                inputRef={inputRef}
                source={passwordSource}
                onSourceChange={(source) => {
                  setPasswordSource(source);
                  setSaveMode("none");
                  if (source === "saved") {
                    setSecret("");
                    setShowSecret(false);
                  }
                }}
                secret={secret}
                showSecret={showSecret}
                onSecretChange={setSecret}
                onToggleShow={() => setShowSecret((value) => !value)}
                passwords={passwordOptions}
                selectedPasswordId={selectedPasswordId}
                loadingPasswords={loadingPasswords}
                onPasswordChange={setSelectedPasswordId}
                onRefreshPasswords={loadSavedPasswords}
              />
            )}

            {request?.canSave && activeMethod !== "key" && (
              <div className="min-w-0 space-y-2 rounded-md border border-dashed px-3 py-2">
                <div className="flex min-w-0 items-center gap-2">
                  <Checkbox
                    checked={saveMode !== "none"}
                    onCheckedChange={(checked) => {
                      setSaveMode(
                        checked
                          ? isPassphrase
                            ? "key_passphrase"
                            : usingSavedPassword
                              ? "connection"
                              : request.passwordId
                                ? "saved_password"
                                : "connection"
                          : "none",
                      );
                    }}
                  />
                  <span className="min-w-0 text-xs">{t("sshAuth.rememberCredential")}</span>
                </div>
                {saveMode !== "none" && !isPassphrase && !usingSavedPassword && (
                  <div className="min-w-0 space-y-2">
                    <Select
                      value={saveMode}
                      onValueChange={(value) => setSaveMode(value as SaveMode)}
                    >
                      <SelectTrigger className="h-8 w-full text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="connection">{t("sshAuth.saveToConnection")}</SelectItem>
                        <SelectItem value="saved_password">
                          {request.passwordId
                            ? t("sshAuth.updateSavedPassword")
                            : t("sshAuth.createSavedPassword")}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    {saveMode === "saved_password" && !request.passwordId && (
                      <Input
                        className="h-8 text-xs"
                        value={saveName}
                        onChange={(event) => setSaveName(event.target.value)}
                        placeholder={t("sshAuth.savedPasswordName")}
                      />
                    )}
                  </div>
                )}
              </div>
            )}
          </div>

          <DialogFooter className="w-full gap-2 sm:gap-0">
            <Button
              variant="ghost"
              size="sm"
              className="text-xs"
              onClick={() => void handleCancel()}
              disabled={submitting}
            >
              {t("common.cancel")}
            </Button>
            <Button
              size="sm"
              className="text-xs"
              onClick={() => void handleSubmit()}
              disabled={!canSubmit}
            >
              {t("sshAuth.submit")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        disablePointerDismissal
        open={keyManagementOpen}
        onOpenChange={(open) => {
          setKeyManagementOpen(open);
          if (!open) void loadSshKeys();
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
    </>
  );
}

interface SecretInputProps {
  inputRef: React.RefObject<HTMLInputElement | null>;
  label: string;
  value: string;
  showValue: boolean;
  onChange: (value: string) => void;
  onToggleShow: () => void;
}

interface PasswordAuthInputProps {
  inputRef: React.RefObject<HTMLInputElement | null>;
  source: PasswordSource;
  onSourceChange: (value: PasswordSource) => void;
  secret: string;
  showSecret: boolean;
  onSecretChange: (value: string) => void;
  onToggleShow: () => void;
  passwords: SavedPassword[];
  selectedPasswordId: string;
  loadingPasswords: boolean;
  onPasswordChange: (value: string) => void;
  onRefreshPasswords: () => void;
}

function PasswordAuthInput({
  inputRef,
  source,
  onSourceChange,
  secret,
  showSecret,
  onSecretChange,
  onToggleShow,
  passwords,
  selectedPasswordId,
  loadingPasswords,
  onPasswordChange,
  onRefreshPasswords,
}: PasswordAuthInputProps) {
  const { t } = useTranslation();
  return (
    <Tabs
      value={source}
      className="min-w-0"
      onValueChange={(value) => onSourceChange(value as PasswordSource)}
    >
      <TabsList className="grid h-8 w-full grid-cols-2">
        <TabsTrigger value="manual" className="min-w-0 text-xs">
          <span className="truncate">{t("dialog.directPassword")}</span>
        </TabsTrigger>
        <TabsTrigger value="saved" className="min-w-0 text-xs">
          <span className="truncate">{t("dialog.savedPassword")}</span>
        </TabsTrigger>
      </TabsList>
      <TabsContent value="manual" className="mt-3 min-w-0">
        <SecretInput
          inputRef={inputRef}
          label={t("dialog.password")}
          value={secret}
          showValue={showSecret}
          onChange={onSecretChange}
          onToggleShow={onToggleShow}
        />
      </TabsContent>
      <TabsContent value="saved" className="mt-3 min-w-0">
        <PasswordSelector
          passwords={passwords}
          value={selectedPasswordId}
          loading={loadingPasswords}
          onChange={onPasswordChange}
          onRefresh={onRefreshPasswords}
        />
      </TabsContent>
    </Tabs>
  );
}

function SecretInput({
  inputRef,
  label,
  value,
  showValue,
  onChange,
  onToggleShow,
}: SecretInputProps) {
  const { t } = useTranslation();
  return (
    <div className="min-w-0">
      <Label className="text-xs">{label}</Label>
      <div className="relative mt-1">
        <Input
          ref={inputRef}
          type={showValue ? "text" : "password"}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          className="h-9 pr-9 text-sm"
        />
        <button
          type="button"
          className="absolute right-2 top-1/2 -translate-y-1/2 rounded-sm p-1 text-muted-foreground hover:text-foreground"
          onClick={onToggleShow}
          title={showValue ? t("dialog.hidePassword") : t("dialog.showPassword")}
        >
          {showValue ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
        </button>
      </div>
    </div>
  );
}

interface PasswordSelectorProps {
  passwords: SavedPassword[];
  value: string;
  loading: boolean;
  onChange: (value: string) => void;
  onRefresh: () => void;
}

function PasswordSelector({
  passwords,
  value,
  loading,
  onChange,
  onRefresh,
}: PasswordSelectorProps) {
  const { t } = useTranslation();
  return (
    <div className="min-w-0 space-y-2">
      <div className="flex min-w-0 items-center justify-between gap-2">
        <Label className="text-xs">{t("dialog.savedPassword")}</Label>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={() => void onRefresh()}
          disabled={loading}
          title={t("sshAuth.refreshPasswords")}
        >
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
      </div>
      {passwords.length > 0 ? (
        <Select value={value} onValueChange={onChange}>
          <SelectTrigger className="h-9 w-full text-xs">
            <SelectValue placeholder={t("dialog.selectPassword")} />
          </SelectTrigger>
          <SelectContent>
            {passwords.map((password) => (
              <SelectItem key={password.id} value={password.id}>
                <span className="flex min-w-0 items-center gap-2">
                  <KeyRound className="h-3.5 w-3.5 shrink-0" />
                  <span className="truncate">{password.name}</span>
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <div className="min-w-0 rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground">
          {loading ? t("sshAuth.loadingPasswords") : t("dialog.noPasswords")}
        </div>
      )}
    </div>
  );
}

interface KeySelectorProps {
  keys: SshKey[];
  value: string;
  loading: boolean;
  onChange: (value: string) => void;
  onRefresh: () => void;
  onAddKey: () => void;
}

function KeySelector({ keys, value, loading, onChange, onRefresh, onAddKey }: KeySelectorProps) {
  const { t } = useTranslation();
  return (
    <div className="min-w-0 space-y-2">
      <div className="flex min-w-0 items-center justify-between gap-2">
        <Label className="text-xs">{t("sshAuth.savedKey")}</Label>
        <div className="flex items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => void onRefresh()}
            disabled={loading}
            title={t("sshAuth.refreshKeys")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={onAddKey}
            title={t("sshAuth.addKey")}
          >
            <Plus className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
      {keys.length > 0 ? (
        <Select value={value} onValueChange={onChange}>
          <SelectTrigger className="h-9 w-full text-xs">
            <SelectValue placeholder={t("sshAuth.selectKey")} />
          </SelectTrigger>
          <SelectContent>
            {keys.map((key) => (
              <SelectItem key={key.id} value={key.id}>
                <span className="flex min-w-0 items-center gap-2">
                  <KeyRound className="h-3.5 w-3.5 shrink-0" />
                  <span className="truncate">{key.name}</span>
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <div className="min-w-0 rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground">
          {loading ? t("sshAuth.loadingKeys") : t("sshAuth.noKeys")}
        </div>
      )}
      <div className="text-xs leading-relaxed text-muted-foreground">
        {t("sshAuth.keyOnlyHint")}
      </div>
    </div>
  );
}

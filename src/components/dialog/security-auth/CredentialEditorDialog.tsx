import { CheckCircle2, Eye, EyeOff, KeyRound, UserRound } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
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
import { Switch } from "@/components/ui/switch";
import { validatePromptRegex } from "@/lib/credentialAutofill";
import type { SavedCredential } from "@/types/global";

interface CredentialEditorDialogProps {
  entry: Partial<SavedCredential>;
  isEditing: boolean;
  open: boolean;
  passwordLoading: boolean;
  onCancel: () => void;
  onChange: (patch: Partial<SavedCredential>) => void;
  onSave: () => void;
  saveDisabled: boolean;
}

export function CredentialEditorDialog({
  entry,
  isEditing,
  open,
  passwordLoading,
  onCancel,
  onChange,
  onSave,
  saveDisabled,
}: CredentialEditorDialogProps) {
  const { t } = useTranslation();
  const [showPassword, setShowPassword] = useState(false);
  const usernamePromptRegex = entry.username_prompt_regex ?? "";
  const passwordPromptRegex = entry.password_prompt_regex ?? "";
  const usernameRegexValid =
    !usernamePromptRegex.trim() || validatePromptRegex(usernamePromptRegex);
  const passwordRegexValid =
    !passwordPromptRegex.trim() || validatePromptRegex(passwordPromptRegex);
  const showUsernameRegexValid = Boolean(usernamePromptRegex.trim() && usernameRegexValid);
  const showPasswordRegexValid = Boolean(passwordPromptRegex.trim() && passwordRegexValid);
  const regexError = (value: string) => (value.trim() ? t("credentialManager.invalidRegex") : "");

  useEffect(() => {
    if (!open) setShowPassword(false);
  }, [open]);

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(nextOpen) => !nextOpen && onCancel()}>
      <DialogContent className="w-[min(640px,calc(100vw-2rem))] max-w-none gap-0 overflow-hidden p-0">
        <DialogHeader className="border-b px-5 py-3 pr-12">
          <DialogTitle className="text-sm">
            {isEditing
              ? t("credentialManager.editorEditTitle")
              : t("credentialManager.editorAddTitle")}
          </DialogTitle>
          <DialogDescription>{t("credentialManager.editorDescription")}</DialogDescription>
        </DialogHeader>
        <div className="terminal-scroll max-h-[calc(100vh-12rem)] overflow-y-auto px-5 py-4">
          <div className="space-y-3">
            <div className="flex min-w-0 items-start justify-between gap-4 border-b pb-3">
              <div className="min-w-0 space-y-1">
                <div className="text-xs font-medium">{t("credentialManager.enabled")}</div>
                <div className="text-[0.6875rem] leading-relaxed text-muted-foreground">
                  {t("credentialManager.enabledDesc")}
                </div>
              </div>
              <Switch
                size="sm"
                checked={entry.enabled ?? true}
                onCheckedChange={(enabled) => onChange({ enabled })}
                aria-label={t("credentialManager.enabled")}
              />
            </div>

            <div className="min-w-0 space-y-1.5">
              <Label className="text-[0.6875rem] text-muted-foreground">
                {t("credentialManager.nameLabel")}
              </Label>
              <Input
                placeholder={t("credentialManager.namePlaceholder")}
                className="h-8 text-xs"
                value={entry.name ?? ""}
                onChange={(event) => onChange({ name: event.target.value })}
                autoFocus
              />
            </div>
          </div>

          <div className="space-y-3">
            <div className="space-y-2 border-t pt-3">
              <div className="flex items-center gap-1.5 text-xs font-medium">
                <UserRound className="h-3.5 w-3.5 text-primary" />
                {t("credentialManager.usernameOptionalLabel")}
              </div>
              <div className="space-y-2">
                <div className="space-y-1.5">
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("credentialManager.promptRegexOptionalLabel")}
                  </Label>
                  <div className="relative">
                    <Input
                      placeholder={t("credentialManager.usernameRegexPlaceholder")}
                      className="h-8 pr-8 font-mono text-[0.6875rem]"
                      value={usernamePromptRegex}
                      onChange={(event) => onChange({ username_prompt_regex: event.target.value })}
                      aria-invalid={!usernameRegexValid}
                    />
                    {showUsernameRegexValid ? (
                      <CheckCircle2 className="pointer-events-none absolute top-1/2 right-2 h-3.5 w-3.5 -translate-y-1/2 text-emerald-500" />
                    ) : null}
                  </div>
                  {!usernameRegexValid ? (
                    <div className="text-[0.6875rem] text-destructive">
                      {regexError(usernamePromptRegex)}
                    </div>
                  ) : null}
                </div>

                <div className="space-y-1.5">
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("credentialManager.sendValueLabel")}
                  </Label>
                  <Input
                    placeholder={t("credentialManager.usernamePlaceholder")}
                    className="h-8 text-xs"
                    value={entry.username ?? ""}
                    onChange={(event) => onChange({ username: event.target.value })}
                  />
                </div>
              </div>
            </div>

            <div className="space-y-2 border-t pt-3">
              <div className="flex items-center gap-1.5 text-xs font-medium">
                <KeyRound className="h-3.5 w-3.5 text-primary" />
                {t("credentialManager.passwordLabel")}
              </div>
              <div className="space-y-2">
                <div className="space-y-1.5">
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("credentialManager.promptRegexOptionalLabel")}
                  </Label>
                  <div className="relative">
                    <Input
                      placeholder={t("credentialManager.passwordRegexPlaceholder")}
                      className="h-8 pr-8 font-mono text-[0.6875rem]"
                      value={passwordPromptRegex}
                      onChange={(event) => onChange({ password_prompt_regex: event.target.value })}
                      aria-invalid={!passwordRegexValid}
                    />
                    {showPasswordRegexValid ? (
                      <CheckCircle2 className="pointer-events-none absolute top-1/2 right-2 h-3.5 w-3.5 -translate-y-1/2 text-emerald-500" />
                    ) : null}
                  </div>
                  {!passwordRegexValid ? (
                    <div className="text-[0.6875rem] text-destructive">
                      {regexError(passwordPromptRegex)}
                    </div>
                  ) : null}
                </div>

                <div className="space-y-1.5">
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("credentialManager.sendValueLabel")}
                  </Label>
                  <div className="relative">
                    <Input
                      type={showPassword ? "text" : "password"}
                      placeholder={
                        passwordLoading
                          ? t("common.loading")
                          : isEditing && entry.has_password
                            ? t("credentialManager.passwordUnchanged")
                            : t("credentialManager.passwordPlaceholder")
                      }
                      className="h-8 pr-8 text-xs"
                      value={entry.password ?? ""}
                      onChange={(event) => onChange({ password: event.target.value })}
                      disabled={passwordLoading}
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="absolute top-0.5 right-0.5 h-7 w-7 text-muted-foreground hover:text-foreground"
                      onClick={() => setShowPassword((value) => !value)}
                      disabled={passwordLoading}
                      aria-label={
                        showPassword
                          ? t("credentialManager.hidePassword")
                          : t("credentialManager.showPassword")
                      }
                    >
                      {showPassword ? (
                        <EyeOff className="h-3.5 w-3.5" />
                      ) : (
                        <Eye className="h-3.5 w-3.5" />
                      )}
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
        <DialogFooter className="border-t px-5 py-3">
          <Button variant="outline" size="sm" className="h-7 px-3 text-xs" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button size="sm" className="h-7 px-3 text-xs" onClick={onSave} disabled={saveDisabled}>
            {t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

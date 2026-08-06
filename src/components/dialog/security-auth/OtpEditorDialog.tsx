import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdChevronRight } from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { OtpEntry } from "@/types/global";

interface OtpEditorDialogProps {
  entry: Partial<OtpEntry>;
  isEditing: boolean;
  open: boolean;
  onCancel: () => void;
  onChange: (patch: Partial<OtpEntry>) => void;
  onSave: () => void;
  saveDisabled: boolean;
  secretLoading: boolean;
}

export function OtpEditorDialog({
  entry,
  isEditing,
  open,
  onCancel,
  onChange,
  onSave,
  saveDisabled,
  secretLoading,
}: OtpEditorDialogProps) {
  const { t } = useTranslation();
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const otpType = entry.otp_type ?? "totp";

  useEffect(() => {
    if (!open) setAdvancedOpen(false);
  }, [open]);

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(nextOpen) => !nextOpen && onCancel()}>
      <DialogContent className="w-[min(560px,calc(100vw-2rem))] max-w-none gap-0 overflow-hidden p-0">
        <DialogHeader className="border-b px-5 py-3 pr-12">
          <DialogTitle className="text-sm">
            {isEditing ? t("otpManager.editorEditTitle") : t("otpManager.editorAddTitle")}
          </DialogTitle>
          <DialogDescription>{t("otpManager.editorDescription")}</DialogDescription>
        </DialogHeader>
        <div className="terminal-scroll max-h-[calc(100vh-12rem)] overflow-y-auto px-5 py-4">
          <div className="space-y-2.5">
            <Tabs
              value={otpType}
              onValueChange={(value) => onChange({ otp_type: value })}
              className="w-full"
            >
              <TabsList className="grid h-7 w-full grid-cols-2">
                <TabsTrigger value="totp" className="text-xs">
                  TOTP
                </TabsTrigger>
                <TabsTrigger value="hotp" className="text-xs">
                  HOTP
                </TabsTrigger>
              </TabsList>
            </Tabs>

            <Input
              placeholder={t("otpManager.issuerPlaceholder")}
              className="h-8 text-xs"
              value={entry.issuer ?? ""}
              onChange={(event) => onChange({ issuer: event.target.value })}
              autoFocus
            />
            <Input
              placeholder={t("otpManager.usernamePlaceholder")}
              className="h-8 text-xs"
              value={entry.username ?? ""}
              onChange={(event) => onChange({ username: event.target.value })}
            />
            <Input
              type="password"
              placeholder={secretLoading ? t("common.loading") : t("otpManager.secretPlaceholder")}
              className="h-8 text-xs"
              value={entry.secret ?? ""}
              onChange={(event) => onChange({ secret: event.target.value })}
              disabled={secretLoading}
            />

            <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
              <CollapsibleTrigger className="group flex w-full items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground">
                <MdChevronRight
                  className={`text-sm transition-transform duration-200 ${advancedOpen ? "rotate-90" : ""}`}
                />
                <span>{t("otpManager.advanced")}</span>
              </CollapsibleTrigger>
              <CollapsibleContent className="mt-2 space-y-2.5">
                <div>
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("otpManager.algorithm")}
                  </Label>
                  <Select
                    value={entry.algorithm ?? "SHA1"}
                    onValueChange={(value) => onChange({ algorithm: value })}
                  >
                    <SelectTrigger className="mt-1 h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="SHA1">SHA-1</SelectItem>
                      <SelectItem value="SHA256">SHA-256</SelectItem>
                      <SelectItem value="SHA512">SHA-512</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div>
                  <Label className="text-[0.6875rem] text-muted-foreground">
                    {t("otpManager.digits")}
                  </Label>
                  <NumberInput
                    className="mt-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
                    value={entry.digits ?? 6}
                    onChange={(value) => onChange({ digits: value })}
                    min={4}
                    max={10}
                  />
                </div>

                {otpType === "totp" ? (
                  <div>
                    <Label className="text-[0.6875rem] text-muted-foreground">
                      {t("otpManager.period")}
                    </Label>
                    <NumberInput
                      className="mt-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
                      value={entry.period ?? 30}
                      onChange={(value) => onChange({ period: value })}
                      min={10}
                      max={300}
                    />
                  </div>
                ) : (
                  <div>
                    <Label className="text-[0.6875rem] text-muted-foreground">
                      {t("otpManager.counter")}
                    </Label>
                    <NumberInput
                      className="mt-1 [&_button]:h-8 [&_button]:w-8 [&_input]:h-8 [&_input]:text-xs"
                      value={entry.counter ?? 0}
                      onChange={(value) => onChange({ counter: value })}
                      min={0}
                      max={999999999}
                    />
                  </div>
                )}
              </CollapsibleContent>
            </Collapsible>
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

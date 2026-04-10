import { invoke } from "@tauri-apps/api/core";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdChevronRight,
  MdDelete,
  MdEdit,
  MdQrCodeScanner,
  MdRefresh,
  MdVisibility,
  MdVisibilityOff,
} from "react-icons/md";
import { toast } from "sonner";
import { OtpCodePanel } from "@/components/panel/OtpCodePanel";
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

interface OtpManagementTabProps {
  onCountChange?: (count: number) => void;
}

function otpTypeTagClass(otpType: string) {
  return otpType === "hotp" ? "text-amber-600 dark:text-amber-400" : "text-primary";
}

interface OtpEditorProps {
  entry: Partial<OtpEntry>;
  onCancel: () => void;
  onChange: (patch: Partial<OtpEntry>) => void;
  onSave: () => void;
  saveDisabled: boolean;
  secretLoading: boolean;
  t: ReturnType<typeof useTranslation>["t"];
}

function OtpEditor({
  entry,
  onCancel,
  onChange,
  onSave,
  saveDisabled,
  secretLoading,
  t,
}: OtpEditorProps) {
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const otpType = entry.otp_type ?? "totp";

  return (
    <div className="space-y-2.5 border-b bg-accent/30 p-3">
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

      <div className="flex justify-end gap-1.5 pt-0.5">
        <Button variant="outline" size="sm" className="h-7 px-3 text-xs" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button size="sm" className="h-7 px-3 text-xs" onClick={onSave} disabled={saveDisabled}>
          {t("common.save")}
        </Button>
      </div>
    </div>
  );
}

export function OtpManagementTab({ onCountChange }: OtpManagementTabProps) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<OtpEntry[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editEntry, setEditEntry] = useState<Partial<OtpEntry>>({});
  const [secretLoading, setSecretLoading] = useState(false);
  const [isNew, setIsNew] = useState(false);
  const [qrImporting, setQrImporting] = useState(false);
  const [deletingEntry, setDeletingEntry] = useState<OtpEntry | null>(null);
  const [visibleOtpIds, setVisibleOtpIds] = useState<Set<string>>(() => new Set());
  const editRequestRef = useRef(0);

  const loadEntries = useCallback(async () => {
    try {
      const result = await invoke<OtpEntry[]>("get_otp_entries");
      setEntries(result);
      setVisibleOtpIds((prev) => {
        const next = new Set<string>();
        for (const entry of result) {
          if (prev.has(entry.id)) next.add(entry.id);
        }
        return next;
      });
      onCountChange?.(result.length);
    } catch {
      /* ignore */
    }
  }, [onCountChange]);

  useEffect(() => {
    void loadEntries();
  }, [loadEntries]);

  const resetEdit = useCallback(() => {
    editRequestRef.current += 1;
    setEditingId(null);
    setEditEntry({});
    setSecretLoading(false);
    setIsNew(false);
  }, []);

  const handleAdd = useCallback(() => {
    resetEdit();
    setEditingId("__new__");
    setEditEntry({
      algorithm: "SHA1",
      counter: 0,
      digits: 6,
      otp_type: "totp",
      period: 30,
    });
    setIsNew(true);
  }, [resetEdit]);

  const handleImportQr = useCallback(async () => {
    if (qrImporting || editingId !== null) return;

    setQrImporting(true);
    try {
      const selected = await openFileDialog({
        multiple: false,
        title: t("otpManager.selectQrImage"),
        filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "bmp", "gif", "webp"] }],
      });
      const selectedPath = Array.isArray(selected) ? selected[0] : selected;
      if (!selectedPath) return;

      const parsed = await invoke<OtpEntry>("import_otp_from_qr", { path: selectedPath });
      resetEdit();
      setEditingId("__new__");
      setEditEntry({
        algorithm: parsed.algorithm,
        counter: parsed.counter,
        digits: parsed.digits,
        issuer: parsed.issuer,
        otp_type: parsed.otp_type,
        period: parsed.period,
        secret: parsed.secret ?? "",
        username: parsed.username,
      });
      setIsNew(true);
    } catch (error) {
      toast.error(t("otpManager.qrImportFailed"), { description: String(error) });
    } finally {
      setQrImporting(false);
    }
  }, [editingId, qrImporting, resetEdit, t]);

  const handleEdit = useCallback(async (entry: OtpEntry) => {
    const requestId = ++editRequestRef.current;
    setEditingId(entry.id);
    setEditEntry({
      ...entry,
      secret: "",
    });
    setSecretLoading(true);
    setIsNew(false);

    try {
      const secret = await invoke<string | null>("get_otp_secret_value", { id: entry.id });
      if (editRequestRef.current !== requestId) return;
      setEditEntry((prev) => ({
        ...prev,
        secret: secret ?? "",
      }));
    } catch {
      if (editRequestRef.current !== requestId) return;
      setEditEntry((prev) => ({
        ...prev,
        secret: "",
      }));
    } finally {
      if (editRequestRef.current === requestId) {
        setSecretLoading(false);
      }
    }
  }, []);

  const handleSave = useCallback(async () => {
    if (!editEntry.issuer?.trim()) return;
    if (isNew && !editEntry.secret) return;

    try {
      await invoke("save_otp_entry", {
        entry: {
          algorithm: editEntry.algorithm ?? "SHA1",
          counter: editEntry.counter ?? 0,
          digits: editEntry.digits ?? 6,
          id: isNew ? "" : editingId,
          issuer: editEntry.issuer.trim(),
          otp_type: editEntry.otp_type ?? "totp",
          period: editEntry.period ?? 30,
          secret: editEntry.secret || undefined,
          username: editEntry.username?.trim() ?? "",
        },
      });
      resetEdit();
      await loadEntries();
    } catch {
      /* ignore */
    }
  }, [editEntry, editingId, isNew, loadEntries, resetEdit]);

  const handleDeleteConfirm = useCallback(async () => {
    if (!deletingEntry) return;

    try {
      await invoke("delete_otp_entry", { id: deletingEntry.id });
      await loadEntries();
    } catch {
      /* ignore */
    }

    setDeletingEntry(null);
  }, [deletingEntry, loadEntries]);

  const handleChange = useCallback((patch: Partial<OtpEntry>) => {
    setEditEntry((prev) => ({ ...prev, ...patch }));
  }, []);

  const toggleCodeVisibility = useCallback((id: string) => {
    setVisibleOtpIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const actionsDisabled = editingId !== null || qrImporting;

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <Label className="text-sm font-medium">{t("otpManager.title")}</Label>
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-primary"
              onClick={() => void handleImportQr()}
              disabled={actionsDisabled}
              title={t("otpManager.scanQr")}
            >
              {qrImporting ? (
                <MdRefresh className="mr-1 text-base animate-spin" />
              ) : (
                <MdQrCodeScanner className="mr-1 text-base" />
              )}
              {qrImporting ? t("otpManager.scanningQr") : t("otpManager.scanQr")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-primary"
              onClick={handleAdd}
              disabled={actionsDisabled}
            >
              <MdAdd className="mr-1 text-base" /> {t("otpManager.add")}
            </Button>
          </div>
        </div>

        <div className="overflow-hidden rounded-md border">
          {isNew && editingId === "__new__" ? (
            <OtpEditor
              entry={editEntry}
              onCancel={resetEdit}
              onChange={handleChange}
              onSave={handleSave}
              saveDisabled={secretLoading || !editEntry.issuer?.trim() || !editEntry.secret}
              secretLoading={secretLoading}
              t={t}
            />
          ) : null}

          {entries.map((entry) => {
            const codeVisible = visibleOtpIds.has(entry.id);

            return (
              <div key={entry.id}>
                {editingId === entry.id && !isNew ? (
                  <OtpEditor
                    entry={editEntry}
                    onCancel={resetEdit}
                    onChange={handleChange}
                    onSave={handleSave}
                    saveDisabled={secretLoading || !editEntry.issuer?.trim()}
                    secretLoading={secretLoading}
                    t={t}
                  />
                ) : (
                  <div className="border-b px-3 py-3 last:border-0 transition-colors hover:bg-accent/35">
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center justify-between gap-3">
                          <div className="truncate text-sm font-medium">{entry.issuer}</div>
                          <span
                            className={`shrink-0 font-mono text-[0.625rem] font-semibold uppercase tracking-[0.18em] ${otpTypeTagClass(entry.otp_type)}`}
                          >
                            [{entry.otp_type.toUpperCase()}]
                          </span>
                        </div>
                        <div className="mt-1 truncate text-[0.6875rem] text-muted-foreground">
                          {entry.username}
                        </div>

                        <div className="mt-2 flex flex-wrap items-center gap-1">
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-7 px-2 text-[0.6875rem] font-medium text-muted-foreground hover:text-foreground"
                            onClick={() => toggleCodeVisibility(entry.id)}
                            disabled={editingId !== null}
                            title={
                              codeVisible ? t("otpManager.hideCodes") : t("otpManager.showCodes")
                            }
                          >
                            {codeVisible ? (
                              <MdVisibilityOff className="mr-1 text-sm" />
                            ) : (
                              <MdVisibility className="mr-1 text-sm" />
                            )}
                            {codeVisible ? t("otpManager.hide") : t("otpManager.view")}
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-7 px-2 text-[0.6875rem] font-medium text-muted-foreground hover:text-foreground"
                            onClick={() => {
                              void handleEdit(entry);
                            }}
                            disabled={editingId !== null}
                          >
                            <MdEdit className="mr-1 text-sm" />
                            {t("common.edit")}
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-7 px-2 text-[0.6875rem] font-medium text-destructive hover:bg-destructive/10 hover:text-destructive"
                            onClick={() => setDeletingEntry(entry)}
                            disabled={editingId !== null}
                          >
                            <MdDelete className="mr-1 text-sm" />
                            {t("common.delete")}
                          </Button>
                        </div>
                      </div>
                    </div>

                    {codeVisible ? (
                      <OtpCodePanel
                        className="mt-3"
                        otpEntryId={entry.id}
                        otpType={entry.otp_type}
                        period={entry.period}
                        variant="list"
                      />
                    ) : null}
                  </div>
                )}
              </div>
            );
          })}

          {entries.length === 0 && !isNew ? (
            <div className="py-6 text-center text-xs text-muted-foreground">
              {t("otpManager.noEntries")}
            </div>
          ) : null}
        </div>
      </div>

      <Dialog
        open={deletingEntry !== null}
        onOpenChange={(open) => !open && setDeletingEntry(null)}
      >
        <DialogContent showCloseButton={false} className="max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("otpManager.deleteTitle")}</DialogTitle>
            <DialogDescription>
              {t("otpManager.deleteConfirm", { name: deletingEntry?.issuer })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeletingEntry(null)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={handleDeleteConfirm}>
              {t("common.delete")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdRefresh } from "react-icons/md";
import { toast } from "sonner";
import {
  type FileExplorerBackendKind,
  joinExplorerPath,
} from "@/components/panel/file-explorer/model";
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
import { invoke } from "@/lib/invoke";

export interface NewItemDialogData {
  sessionId: string;
  backend: FileExplorerBackendKind;
  currentDirPath: string;
  /** "file" or "folder" */
  type: "file" | "folder";
}

interface NewItemDialogProps {
  data: NewItemDialogData;
  onClose: () => void;
  onSuccess: (result: { name: string; openAfterCreate: boolean; is_dir: boolean }) => void;
}

export default function NewItemDialog({ data, onClose, onSuccess }: NewItemDialogProps) {
  const { t } = useTranslation();
  const isFile = data.type === "file";

  const [name, setName] = useState("");
  const [octal, setOctal] = useState<string>(isFile ? "0644" : "0755");
  const [openAfterCreate, setOpenAfterCreate] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const octalRef = useRef<HTMLInputElement>(null);

  const titleKey = isFile ? "fileExplorer.newFile" : "fileExplorer.newFolder";
  const labelKey = isFile ? "fileExplorer.fileLabel" : "fileExplorer.folderLabel";
  const openLabelKey = isFile
    ? "fileExplorer.openAfterCreateFile"
    : "fileExplorer.openAfterCreateFolder";
  const canEditMode = data.backend === "remote";
  const command =
    data.backend === "local"
      ? isFile
        ? "create_local_file"
        : "create_local_dir"
      : isFile
        ? "create_remote_file"
        : "create_remote_dir";

  const updateBit = (index: number, bit: number, checked: boolean) => {
    const chars = octal.padStart(4, "0").split("");
    let val = parseInt(chars[index], 8);
    if (Number.isNaN(val)) val = 0;
    if (checked) val |= bit;
    else val &= ~bit;
    chars[index] = val.toString(8);
    setOctal(chars.join(""));
  };

  const hasBit = (index: number, bit: number) => {
    const chars = octal.padStart(4, "0").split("");
    const val = parseInt(chars[index], 8);
    return (val & bit) === bit;
  };

  const handleSubmit = async () => {
    const trimmed = name.trim();
    if (!trimmed) {
      onClose();
      return;
    }

    try {
      setIsSubmitting(true);
      const path = joinExplorerPath(data.currentDirPath, trimmed, data.backend);
      await invoke(command, { sessionId: data.sessionId, path, mode: canEditMode ? octal : null });
      onSuccess({ name: trimmed, openAfterCreate, is_dir: !isFile });
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <Dialog disablePointerDismissal open onOpenChange={(v) => !v && !isSubmitting && onClose()}>
      <DialogContent className="w-[min(500px,calc(100vw-2rem))] sm:max-w-[500px] p-0 gap-0">
        <DialogHeader className="pl-5 pr-12 py-3 border-b">
          <DialogTitle className="text-sm">{t(titleKey)}</DialogTitle>
          <DialogDescription className="sr-only">{t(titleKey)}</DialogDescription>
        </DialogHeader>

        <div className="p-5 space-y-4">
          <div className="flex min-w-0 items-center gap-3">
            <Label className="text-xs w-16 shrink-0">{t(labelKey)}</Label>
            <Input
              ref={inputRef}
              className="text-sm flex-1 h-8"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !isSubmitting && handleSubmit()}
              disabled={isSubmitting}
              autoFocus
            />
          </div>

          {canEditMode && (
            <div className="flex min-w-0 items-start gap-3">
              <Label className="text-xs w-16 shrink-0 mt-3">{t("fileExplorer.permissions")}:</Label>
              <div className="min-w-0 flex-1 space-y-3">
                {/* Permission Grid */}
                <div className="grid grid-cols-[auto_1fr_1fr_1fr_1fr] gap-x-2 gap-y-3 text-xs items-center">
                  <span className="text-muted-foreground mr-2">{t("fileExplorer.permUser")}</span>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(1, 4)}
                      onCheckedChange={(v) => updateBit(1, 4, v === true)}
                    />{" "}
                    R
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(1, 2)}
                      onCheckedChange={(v) => updateBit(1, 2, v === true)}
                    />{" "}
                    W
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(1, 1)}
                      onCheckedChange={(v) => updateBit(1, 1, v === true)}
                    />{" "}
                    X
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(0, 4)}
                      onCheckedChange={(v) => updateBit(0, 4, v === true)}
                    />{" "}
                    UID
                  </label>

                  <span className="text-muted-foreground mr-2">{t("fileExplorer.permGroup")}</span>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(2, 4)}
                      onCheckedChange={(v) => updateBit(2, 4, v === true)}
                    />{" "}
                    R
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(2, 2)}
                      onCheckedChange={(v) => updateBit(2, 2, v === true)}
                    />{" "}
                    W
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(2, 1)}
                      onCheckedChange={(v) => updateBit(2, 1, v === true)}
                    />{" "}
                    X
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(0, 2)}
                      onCheckedChange={(v) => updateBit(0, 2, v === true)}
                    />{" "}
                    GID
                  </label>

                  <span className="text-muted-foreground mr-2">{t("fileExplorer.permOther")}</span>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(3, 4)}
                      onCheckedChange={(v) => updateBit(3, 4, v === true)}
                    />{" "}
                    R
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(3, 2)}
                      onCheckedChange={(v) => updateBit(3, 2, v === true)}
                    />{" "}
                    W
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(3, 1)}
                      onCheckedChange={(v) => updateBit(3, 1, v === true)}
                    />{" "}
                    X
                  </label>
                  <label className="flex items-center gap-1.5 cursor-pointer">
                    <Checkbox
                      checked={hasBit(0, 1)}
                      onCheckedChange={(v) => updateBit(0, 1, v === true)}
                    />{" "}
                    {t("fileExplorer.permSticky")}
                  </label>
                </div>

                {/* Octal input */}
                <div className="flex items-center pt-2 gap-3">
                  <Label className="text-xs w-20 shrink-0">{t("fileExplorer.octal")}</Label>
                  <div className="flex items-center border rounded pl-2 pr-1 h-8 flex-1 bg-background mr-[20%]">
                    <input
                      ref={octalRef}
                      type="text"
                      className="flex-1 bg-transparent outline-none font-mono text-xs w-full"
                      value={octal}
                      onChange={(e) => {
                        let val = e.target.value.replace(/[^0-7]/g, "");
                        if (val.length > 4) val = val.substring(0, 4);
                        setOctal(val);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !isSubmitting) handleSubmit();
                      }}
                      onBlur={() => {
                        if (!octal) setOctal(isFile ? "0644" : "0755");
                        else setOctal(octal.padStart(4, "0"));
                      }}
                      autoCapitalize="none"
                      autoCorrect="off"
                      spellCheck={false}
                      autoComplete="off"
                    />
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="px-5 py-3 border-t flex flex-col-reverse items-stretch justify-between sm:flex-row sm:items-center sm:justify-between">
          <label className="flex min-w-0 items-center gap-1.5 cursor-pointer text-xs">
            <Checkbox
              checked={openAfterCreate}
              onCheckedChange={(v) => setOpenAfterCreate(v === true)}
            />
            <span className="min-w-0 break-words">{t(openLabelKey)}</span>
          </label>
          <div className="flex shrink-0 justify-end gap-2">
            <Button variant="outline" size="sm" onClick={onClose} disabled={isSubmitting}>
              {t("dialog.cancel")}
            </Button>
            <Button size="sm" onClick={handleSubmit} disabled={isSubmitting || !name.trim()}>
              {isSubmitting && <MdRefresh className="text-[0.875rem] animate-spin h-4 w-4 mr-1" />}
              {t("common.confirm")}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

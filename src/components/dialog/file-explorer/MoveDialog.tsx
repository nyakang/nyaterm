import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdRefresh } from "react-icons/md";
import { toast } from "sonner";
import type { FileExplorerBackendKind } from "@/components/panel/file-explorer/model";
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
import { invoke } from "@/lib/invoke";

export interface MoveDialogData {
  sessionId: string;
  backend: FileExplorerBackendKind;
  oldPath: string;
  oldRawPathToken?: string;
  name: string;
}

interface MoveDialogProps {
  data: MoveDialogData;
  onClose: () => void;
  onSuccess: () => void;
}

export default function MoveDialog({ data, onClose, onSuccess }: MoveDialogProps) {
  const { t } = useTranslation();
  const [dialogInput, setDialogInput] = useState(data.oldPath);
  const [isSubmitting, setIsSubmitting] = useState(false);

  useEffect(() => {
    setDialogInput(data.oldPath);
  }, [data.oldPath]);

  const handleMoveSubmit = async () => {
    if (!dialogInput || dialogInput === data.oldPath) {
      onClose();
      return;
    }

    try {
      setIsSubmitting(true);
      if (data.backend === "local") {
        await invoke("rename_local_file", {
          sessionId: data.sessionId,
          oldPath: data.oldPath,
          newPath: dialogInput,
        });
      } else {
        await invoke("rename_remote_file", {
          sessionId: data.sessionId,
          oldPath: data.oldPath,
          newPath: dialogInput,
          oldRawPathToken: data.oldRawPathToken,
        });
      }
      onSuccess();
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <Dialog disablePointerDismissal open onOpenChange={(v) => !v && !isSubmitting && onClose()}>
      <DialogContent className="w-[min(24rem,calc(100vw-2rem))] sm:max-w-96">
        <DialogHeader className="min-w-0 pr-8">
          <DialogTitle
            className="w-full text-sm truncate"
            title={t("fileExplorer.moveTo", { name: data.name })}
          >
            {t("fileExplorer.moveTo", { name: data.name })}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t("fileExplorer.moveTo", { name: data.name })}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-2">
          <Input
            className="text-sm"
            value={dialogInput}
            onChange={(e) => setDialogInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !isSubmitting && handleMoveSubmit()}
            disabled={isSubmitting}
            autoFocus
          />
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose} disabled={isSubmitting}>
            {t("dialog.cancel")}
          </Button>
          <Button size="sm" onClick={handleMoveSubmit} disabled={isSubmitting}>
            {isSubmitting && <MdRefresh className="text-[0.875rem] animate-spin" />}
            {t("dialog.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

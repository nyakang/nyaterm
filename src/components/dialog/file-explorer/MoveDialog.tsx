import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdRefresh } from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { invoke } from "@/lib/invoke";

export interface MoveDialogData {
  sessionId: string;
  oldPath: string;
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
      await invoke("rename_remote_file", {
        sessionId: data.sessionId,
        oldPath: data.oldPath,
        newPath: dialogInput,
      });
      onSuccess();
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <Dialog open onOpenChange={(v) => !v && !isSubmitting && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-96 sm:max-w-96">
        <DialogHeader>
          <DialogTitle className="text-sm">
            {t("fileExplorer.moveTo", { name: data.name })}
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-2">
          <Label className="text-xs">{t("fileExplorer.moveTo", { name: data.name })}</Label>
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

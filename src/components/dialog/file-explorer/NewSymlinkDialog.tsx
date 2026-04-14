import { useState } from "react";
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

export interface NewSymlinkDialogData {
  sessionId: string;
  currentDirPath: string;
}

interface NewSymlinkDialogProps {
  data: NewSymlinkDialogData;
  onClose: () => void;
  onSuccess: () => void;
}

export default function NewSymlinkDialog({ data, onClose, onSuccess }: NewSymlinkDialogProps) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleSubmit = async () => {
    const trimmedName = name.trim();
    const trimmedTarget = target.trim();
    if (!trimmedName || !trimmedTarget) return;

    try {
      setIsSubmitting(true);
      const linkPath =
        data.currentDirPath === "/" ? `/${trimmedName}` : `${data.currentDirPath}/${trimmedName}`;
      await invoke("create_remote_symlink", {
        sessionId: data.sessionId,
        linkPath,
        targetPath: trimmedTarget,
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
      <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px] p-0 gap-0">
        <DialogHeader className="px-5 py-3 border-b">
          <DialogTitle className="text-sm">{t("fileExplorer.newSymlink")}</DialogTitle>
        </DialogHeader>

        <div className="p-5 space-y-4">
          <div className="flex items-center gap-3">
            <Label className="text-xs w-20 shrink-0">{t("fileExplorer.symlinkName")}</Label>
            <Input
              className="text-sm flex-1 h-8"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !isSubmitting && handleSubmit()}
              disabled={isSubmitting}
              autoFocus
            />
          </div>
          <div className="flex items-center gap-3">
            <Label className="text-xs w-20 shrink-0">{t("fileExplorer.symlinkTarget")}</Label>
            <Input
              className="text-sm flex-1 h-8"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !isSubmitting && handleSubmit()}
              disabled={isSubmitting}
              placeholder="/path/to/target"
            />
          </div>
        </div>

        <DialogFooter className="px-5 py-3 border-t">
          <Button variant="outline" size="sm" onClick={onClose} disabled={isSubmitting}>
            {t("dialog.cancel")}
          </Button>
          <Button
            size="sm"
            onClick={handleSubmit}
            disabled={isSubmitting || !name.trim() || !target.trim()}
          >
            {isSubmitting && <MdRefresh className="text-[0.875rem] animate-spin h-4 w-4 mr-1" />}
            OK
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

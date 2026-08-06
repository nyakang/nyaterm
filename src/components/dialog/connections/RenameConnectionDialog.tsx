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

interface RenameConnectionDialogProps {
  open: boolean;
  value: string;
  onValueChange: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
}

export default function RenameConnectionDialog({
  open,
  value,
  onValueChange,
  onSubmit,
  onCancel,
}: RenameConnectionDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(v) => !v && onCancel()}>
      <DialogContent showCloseButton={false} className="max-w-xs">
        <DialogHeader>
          <DialogTitle className="text-sm">{t("savedConnections.rename")}</DialogTitle>
          <DialogDescription className="sr-only">{t("savedConnections.rename")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-2">
          <Input
            className="text-sm"
            value={value}
            onChange={(e) => onValueChange(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && onSubmit()}
            autoFocus
          />
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onCancel}>
            {t("dialog.cancel")}
          </Button>
          <Button size="sm" onClick={onSubmit} disabled={!value.trim()}>
            {t("dialog.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

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

interface TabRenameDialogProps {
  open: boolean;
  value: string;
  onOpenChange: (open: boolean) => void;
  onValueChange: (value: string) => void;
  onSubmit: () => void | Promise<void>;
}

export default function TabRenameDialog({
  open,
  value,
  onOpenChange,
  onValueChange,
  onSubmit,
}: TabRenameDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-xs">
        <DialogHeader>
          <DialogTitle className="text-sm">{t("tabCtx.renameTitle")}</DialogTitle>
          <DialogDescription className="sr-only">{t("tabCtx.renameTitle")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-2">
          <Input
            className="text-sm"
            value={value}
            onChange={(event) => onValueChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void onSubmit();
              }
            }}
            maxLength={64}
            autoFocus
          />
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            {t("dialog.cancel")}
          </Button>
          <Button size="sm" onClick={() => void onSubmit()} disabled={!value.trim()}>
            {t("dialog.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

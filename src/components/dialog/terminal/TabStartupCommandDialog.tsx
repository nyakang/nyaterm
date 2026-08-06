import { useEffect, useRef } from "react";
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

interface TabStartupCommandDialogProps {
  delayMs: number;
  open: boolean;
  value: string;
  onDelayMsChange: (delayMs: number) => void;
  onOpenChange: (open: boolean) => void;
  onSubmit: () => void | Promise<void>;
  onValueChange: (value: string) => void;
}

export default function TabStartupCommandDialog({
  delayMs,
  open,
  value,
  onDelayMsChange,
  onOpenChange,
  onSubmit,
  onValueChange,
}: TabStartupCommandDialogProps) {
  const { t } = useTranslation();
  const commandInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) return;
    requestAnimationFrame(() => {
      commandInputRef.current?.focus();
      commandInputRef.current?.select();
    });
  }, [open]);

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-md">
        <DialogHeader>
          <DialogTitle className="text-sm">{t("tabCtx.runCommandTitle")}</DialogTitle>
          <DialogDescription className="sr-only">{t("tabCtx.runCommandTitle")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              {t("tabCtx.commandInput")}
            </label>
            <Input
              ref={commandInputRef}
              className="text-sm"
              value={value}
              onChange={(event) => onValueChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void onSubmit();
                }
              }}
            />
          </div>
          <div className="space-y-1.5">
            <label className="text-xs font-medium text-muted-foreground">
              {t("tabCtx.commandDelay")}
            </label>
            <Input
              className="text-sm"
              type="number"
              min={0}
              max={60000}
              step={100}
              value={delayMs}
              onChange={(event) => {
                const parsed = Number.parseInt(event.target.value, 10);
                onDelayMsChange(Number.isFinite(parsed) ? parsed : 0);
              }}
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            {t("dialog.cancel")}
          </Button>
          <Button size="sm" onClick={() => void onSubmit()}>
            {t("common.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

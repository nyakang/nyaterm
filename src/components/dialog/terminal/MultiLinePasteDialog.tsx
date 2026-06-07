import { useRef } from "react";
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

interface MultiLinePasteDialogProps {
  open: boolean;
  text: string | null;
  onClose: () => void;
  onDirectPaste: () => void;
  onSendLineByLine: () => void;
}

function normalizePasteNewlines(text: string): string {
  return text.replace(/\r\n|\r/gu, "\n");
}

function countPasteLines(text: string): number {
  return normalizePasteNewlines(text).split("\n").length;
}

function countPasteCharacters(text: string): number {
  return Array.from(text).length;
}

export default function MultiLinePasteDialog({
  open,
  text,
  onClose,
  onDirectPaste,
  onSendLineByLine,
}: MultiLinePasteDialogProps) {
  const { t } = useTranslation();
  const directPasteButtonRef = useRef<HTMLButtonElement>(null);

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent
        className="sm:max-w-xl"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          directPasteButtonRef.current?.focus();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("terminal.multiLinePasteTitle")}</DialogTitle>
          <DialogDescription>
            {text
              ? t("terminal.multiLinePasteStats", "{{lines}} lines, {{chars}} characters", {
                  lines: countPasteLines(text),
                  chars: countPasteCharacters(text),
                })
              : ""}
          </DialogDescription>
        </DialogHeader>
        <pre className="max-h-56 min-h-32 overflow-auto whitespace-pre-wrap rounded-md border bg-muted/30 p-3 font-mono text-xs leading-5">
          {text}
        </pre>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button ref={directPasteButtonRef} onClick={onDirectPaste}>
            {t("terminal.multiLinePasteDirect")}
          </Button>
          <Button variant="secondary" onClick={onSendLineByLine}>
            {t("terminal.multiLinePasteSendLineByLine")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

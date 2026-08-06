import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MultiLinePasteDialogProps } from "@/components/terminal/xterminalTypes";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";

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
  const [draftText, setDraftText] = useState(text ?? "");
  const canSend = draftText.length > 0;
  const stats = useMemo(
    () =>
      draftText
        ? t("terminal.multiLinePasteStats", "{{lines}} lines, {{chars}} characters", {
            lines: countPasteLines(draftText),
            chars: countPasteCharacters(draftText),
          })
        : "",
    [draftText, t],
  );

  useEffect(() => {
    if (open) {
      setDraftText(text ?? "");
    }
  }, [open, text]);

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent
        className="sm:max-w-xl"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          requestAnimationFrame(() => {
            directPasteButtonRef.current?.focus();
          });
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("terminal.multiLinePasteTitle")}</DialogTitle>
          <DialogDescription>{stats}</DialogDescription>
        </DialogHeader>
        <Textarea
          className="max-h-72 min-h-32 resize-y overflow-auto font-mono text-xs leading-5 md:text-xs"
          value={draftText}
          onChange={(event) => setDraftText(event.target.value)}
          aria-label={t("terminal.multiLinePasteTitle")}
        />
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            ref={directPasteButtonRef}
            disabled={!canSend}
            onClick={() => onDirectPaste(draftText)}
          >
            {t("terminal.multiLinePasteDirect")}
          </Button>
          <Button
            variant="secondary"
            disabled={!canSend}
            onClick={() => onSendLineByLine(draftText)}
          >
            {t("terminal.multiLinePasteSendLineByLine")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

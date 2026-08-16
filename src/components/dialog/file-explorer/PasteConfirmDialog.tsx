import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  type PasteConfirmRequest,
  resolvePasteConfirm,
  subscribePasteConfirm,
} from "@/lib/pasteConfirmPrompt";

export function PasteConfirmDialog() {
  const { t } = useTranslation();
  const [request, setRequest] = useState<PasteConfirmRequest | null>(null);

  useEffect(() => subscribePasteConfirm(setRequest), []);

  const handleConfirm = () => {
    resolvePasteConfirm(true);
  };

  const handleCancel = () => {
    resolvePasteConfirm(false);
  };

  const titleKey =
    request?.action === "upload"
      ? "fileExplorer.pasteConfirmUploadTitle"
      : request?.action === "copy"
        ? "fileExplorer.pasteConfirmCopyTitle"
        : "fileExplorer.pasteConfirmMoveTitle";
  const descriptionKey =
    request?.action === "upload"
      ? "fileExplorer.pasteConfirmUploadDesc"
      : request?.action === "copy"
        ? "fileExplorer.pasteConfirmCopyDesc"
        : "fileExplorer.pasteConfirmMoveDesc";
  const actionKey =
    request?.action === "upload"
      ? "fileExplorer.pasteConfirmUploadAction"
      : request?.action === "copy"
        ? "fileExplorer.pasteConfirmCopyAction"
        : "fileExplorer.pasteConfirmMoveAction";

  return (
    <AlertDialog
      open={!!request}
      onOpenChange={(open) => {
        if (!open && request) {
          handleCancel();
        }
      }}
    >
      <AlertDialogContent
        size="sm"
        className="w-[min(22rem,calc(100vw-2rem))] sm:max-w-md"
        onKeyDown={(event) => {
          if (event.key === "Enter" && request) {
            if (event.target instanceof Element && event.target.closest("button")) {
              // Let the focused button run its own action (e.g. Cancel).
              return;
            }
            event.preventDefault();
            handleConfirm();
          }
        }}
      >
        <AlertDialogHeader className="min-w-0 text-left">
          <AlertDialogTitle className="text-sm">{t(titleKey)}</AlertDialogTitle>
          <AlertDialogDescription className="text-xs leading-relaxed">
            {t(descriptionKey, { count: request?.count ?? 0 })}
          </AlertDialogDescription>
        </AlertDialogHeader>

        {request?.fileNames && request.fileNames.length > 0 && (
          <div
            className="max-h-28 overflow-y-auto rounded-md border px-2 py-1.5 font-mono text-[0.6875rem]"
            style={{ borderColor: "var(--df-border)", color: "var(--df-text-dimmed)" }}
          >
            {request.fileNames.map((name) => (
              <div key={name} className="truncate leading-5">
                {name}
              </div>
            ))}
          </div>
        )}

        {request?.targetDir && (
          <div
            className="rounded-md border px-2 py-1.5 font-mono text-[0.6875rem] break-all"
            style={{ borderColor: "var(--df-border)", color: "var(--df-text-dimmed)" }}
          >
            {request.targetDir}
          </div>
        )}

        <AlertDialogFooter className="grid grid-cols-2 gap-3 sm:grid sm:justify-stretch">
          <AlertDialogCancel
            className="w-full text-xs"
            onClick={(event) => {
              event.preventDefault();
              handleCancel();
            }}
          >
            {t("common.cancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            className="w-full text-xs"
            onClick={(event) => {
              event.preventDefault();
              handleConfirm();
            }}
          >
            {t(actionKey)}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { MdRefresh } from "react-icons/md";
import { toast } from "sonner";
import type { FileExplorerBackendKind } from "@/components/panel/file-explorer/model";
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
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";

export type DeleteDialogMode = "native" | "rm";

export interface DeleteDialogData {
  sessionId: string;
  backend: FileExplorerBackendKind;
  mode: DeleteDialogMode;
  items: DeleteDialogItem[];
}

export interface DeleteDialogItem {
  path: string;
  name: string;
  rawPathToken?: string;
}

interface DeleteDialogProps {
  data: DeleteDialogData;
  onClose: () => void;
  onSuccess: () => void;
}

export default function DeleteDialog({ data, onClose, onSuccess }: DeleteDialogProps) {
  const { t } = useTranslation();
  const [isSubmitting, setIsSubmitting] = useState(false);
  const previewItems = data.items.slice(0, 6);
  const remainingItems = data.items.length - previewItems.length;
  const usesRm = data.mode === "rm" && data.backend === "remote";

  const handleDeleteSubmit = async () => {
    try {
      setIsSubmitting(true);

      const results = await Promise.allSettled(
        data.items.map((item) => {
          if (data.backend === "local") {
            return invoke("delete_local_file", {
              sessionId: data.sessionId,
              path: item.path,
            });
          }

          return invoke(usesRm ? "delete_remote_file_with_rm" : "delete_remote_file", {
            sessionId: data.sessionId,
            path: item.path,
            rawPathToken: item.rawPathToken,
          });
        }),
      );

      const failedResults = results.filter(
        (result): result is PromiseRejectedResult => result.status === "rejected",
      );
      const failedCount = failedResults.length;
      const successCount = results.length - failedCount;

      if (successCount > 0) {
        onSuccess();
      }

      if (failedCount > 0) {
        toast.error(
          usesRm && failedCount === 1
            ? t("fileExplorer.deleteWithRmFailed", {
                error: getErrorMessage(failedResults[0]?.reason),
              })
            : failedCount === 1
              ? t("fileExplorer.deleteFailedItem")
              : t("fileExplorer.deleteFailedCount", { count: failedCount }),
        );
      }

      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open && !isSubmitting) {
          onClose();
        }
      }}
    >
      <AlertDialogContent
        size="sm"
        className="w-[min(20rem,calc(100vw-2rem))] sm:max-w-80"
        onKeyDown={(event) => {
          if (event.key === "Enter" && !isSubmitting && data.items.length > 0) {
            event.preventDefault();
            void handleDeleteSubmit();
          }
        }}
      >
        <AlertDialogHeader className="min-w-0 text-left">
          <AlertDialogTitle className="text-sm break-all leading-relaxed">
            {data.items.length === 1
              ? t(usesRm ? "fileExplorer.sureDeleteWithRm" : "fileExplorer.sureDelete", {
                  name: data.items[0]?.name ?? "",
                })
              : t(
                  usesRm
                    ? "fileExplorer.sureDeleteWithRmMultiple"
                    : "fileExplorer.sureDeleteMultiple",
                  { count: data.items.length },
                )}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t(usesRm ? "fileExplorer.deleteWithRmConfirmHint" : "fileExplorer.deleteConfirmHint")}
          </AlertDialogDescription>
        </AlertDialogHeader>

        {data.items.length > 1 && (
          <div
            className="terminal-scroll max-h-40 overflow-y-auto rounded-md border px-2 py-1.5 text-xs"
            style={{ borderColor: "var(--df-border)", color: "var(--df-text-dimmed)" }}
          >
            {previewItems.map((item) => (
              <div key={item.path} className="truncate py-0.5" title={item.path}>
                {item.name}
              </div>
            ))}
            {remainingItems > 0 && (
              <div className="pt-1" style={{ color: "var(--df-text)" }}>
                {t("fileExplorer.moreItems", { count: remainingItems })}
              </div>
            )}
          </div>
        )}

        <AlertDialogFooter className="mt-2 min-w-0">
          <AlertDialogCancel disabled={isSubmitting}>{t("dialog.cancel")}</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            autoFocus
            disabled={isSubmitting || data.items.length === 0}
            onClick={(event) => {
              event.preventDefault();
              void handleDeleteSubmit();
            }}
          >
            {isSubmitting && <MdRefresh className="mr-1 text-[0.875rem] animate-spin" />}
            {t(usesRm ? "fileExplorer.deleteWithRm" : "fileExplorer.cmDelete")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

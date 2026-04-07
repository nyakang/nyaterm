import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { MdCloudSync } from "react-icons/md";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { Button } from "@/components/ui/button";
import { parseJsonSearchParam } from "@/lib/utils";

export interface AutoUploadDialogData {
  sessionId: string;
  localPath: string;
  remotePath: string;
}

export default function AutoUploadPage() {
  const { t } = useTranslation();
  const params = new URLSearchParams(window.location.search);
  const dataParam = params.get("data");
  const data = parseJsonSearchParam<AutoUploadDialogData>(dataParam);

  const handleClose = () => getCurrentWindow().close();

  const handleUpload = async (always: boolean) => {
    if (!data) return;

    // We emit an event to the main window to update its 'alwaysUploadFilesRef'
    if (always) {
      await emit("auto-upload-decision", {
        sessionId: data.sessionId,
        localPath: data.localPath,
        remotePath: data.remotePath,
        always,
      });
    }

    try {
      await invoke("upload_local_file", {
        sessionId: data.sessionId,
        localPath: data.localPath,
        remotePath: data.remotePath,
      });
    } catch (e) {
      console.error("Upload failed", e);
    }

    handleClose();
  };

  if (!data) return null;

  return (
    <div className="h-screen flex flex-col overflow-hidden bg-background text-foreground">
      <ChildWindowHeader
        title={t("fileExplorer.fileModified")}
        icon={<MdCloudSync className="text-base" />}
        onClose={handleClose}
      />

      <div className="flex-1 p-5 space-y-4">
        <div className="flex items-center gap-3 pointer-events-none">
          <div
            className="flex items-center justify-center w-8 h-8 rounded-full shrink-0"
            style={{ backgroundColor: "color-mix(in srgb, var(--df-primary) 15%, transparent)" }}
          >
            <MdCloudSync className="text-[1.125rem] text-primary shrink-0" />
          </div>
          <h2 className="text-sm font-semibold truncate shrink-0">
            {t("fileExplorer.fileModified")}
          </h2>
        </div>
        <p className="text-xs leading-relaxed min-w-0 mt-1 pointer-events-none text-muted-foreground">
          {t("fileExplorer.uploadPrompt")}
        </p>
        <div
          className="font-mono bg-black/20 px-2 py-1.5 rounded border text-[11px] truncate min-w-0 mt-2 pointer-events-none"
          style={{ color: "var(--df-text)", borderColor: "var(--df-border)" }}
          title={data.remotePath}
        >
          {data.remotePath}
        </div>
      </div>

      <div className="px-5 py-4 border-t bg-muted/20 flex gap-2 shrink-0 justify-end">
        <Button variant="ghost" size="sm" className="text-xs" onClick={handleClose}>
          {t("dialog.cancel")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="text-xs flex-1"
          onClick={() => handleUpload(true)}
        >
          {t("fileExplorer.alwaysUpload")}
        </Button>
        <Button size="sm" className="text-xs flex-1" onClick={() => handleUpload(false)}>
          {t("fileExplorer.uploadOnce")}
        </Button>
      </div>
    </div>
  );
}

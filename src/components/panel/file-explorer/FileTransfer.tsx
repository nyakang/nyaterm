import { downloadDir } from "@tauri-apps/api/path";
import { type ElementType, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdCancel,
  MdCheckCircle,
  MdDelete,
  MdDownload,
  MdError,
  MdFolder,
  MdFolderOff,
  MdPause,
  MdPlayArrow,
  MdPlaylistRemove,
  MdRefresh,
  MdSwapHoriz,
  MdSync,
  MdUpload,
} from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { type TransferItem, useTransfer } from "../../../context/TransferContext";

interface FileTransferProps {
  activeSessionId: string | null;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function HeaderActionButton({
  label,
  icon: Icon,
  onClick,
  disabled,
}: {
  label: string;
  icon: ElementType;
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex">
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground hover:text-foreground"
            aria-label={label}
            onClick={onClick}
            disabled={disabled}
          >
            <Icon className="size-4" />
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">{label}</TooltipContent>
    </Tooltip>
  );
}

function TransferRow({
  item,
  onPause,
  onResume,
  onRetry,
  onCancel,
  onDelete,
  onOpenTargetDirectory,
}: {
  item: TransferItem;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRetry: (item: TransferItem) => void;
  onCancel: (id: string) => void;
  onDelete: (id: string) => void;
  onOpenTargetDirectory: (id: string) => void;
}) {
  const { t } = useTranslation();
  const DirIcon =
    item.kind === "directory" ? MdFolder : item.direction === "upload" ? MdUpload : MdDownload;
  const dirColor = item.direction === "upload" ? "#4ade80" : "#60a5fa";
  const progress =
    item.kind === "directory"
      ? item.itemCountTotal && item.itemCountTotal > 0
        ? Math.min(100, Math.round(((item.itemCountCompleted ?? 0) / item.itemCountTotal) * 100))
        : item.status === "completed"
          ? 100
          : 0
      : item.totalSize > 0
        ? Math.min(100, Math.round((item.bytesTransferred / item.totalSize) * 100))
        : 0;
  const canPause = item.status === "transferring";
  const canResume = item.status === "paused";
  const canRetry = item.status === "error" || item.status === "cancelled";
  const canCancel = item.status === "transferring" || item.status === "paused";
  const canDelete = !canCancel;

  let statusIcon: ElementType = MdSync;
  let statusColor = "#facc15";
  let statusText = `${progress}%`;

  if (item.status === "paused") {
    statusIcon = MdPause;
    statusColor = "#fb923c";
    statusText = t("fileTransfer.paused");
  } else if (item.status === "completed") {
    statusIcon = MdCheckCircle;
    statusColor = "#4ade80";
    statusText = t("fileTransfer.completed");
  } else if (item.status === "error") {
    statusIcon = MdError;
    statusColor = "#f87171";
    statusText = t("fileTransfer.error");
  } else if (item.status === "cancelled") {
    statusIcon = MdCancel;
    statusColor = "#a1a1aa";
    statusText = t("fileTransfer.cancelled");
  }

  const StatusIcon = statusIcon;

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          className="rounded transition-colors px-2 py-1.5"
          style={{ backgroundColor: "var(--df-bg-panel)" }}
          onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = "var(--df-bg-hover)")}
          onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "var(--df-bg-panel)")}
          title={item.error || `${item.fileName} — ${statusText}`}
        >
          <div className="flex items-center gap-2">
            <DirIcon className="text-sm shrink-0" style={{ color: dirColor }} />

            <div className="flex-1 min-w-0">
              <div className="text-xs truncate" style={{ color: "var(--df-text)" }}>
                {item.fileName}
              </div>
              <div
                className="flex items-center gap-1 text-[0.625rem]"
                style={{ color: "var(--df-text-dimmed)" }}
              >
                <span>{formatTime(item.timestamp)}</span>
                {item.kind === "directory" ? (
                  item.itemCountTotal !== undefined && (
                    <>
                      <span>·</span>
                      <span>
                        {t("fileTransfer.directoryProgress", {
                          completed: item.itemCountCompleted ?? 0,
                          total: item.itemCountTotal,
                        })}
                      </span>
                    </>
                  )
                ) : item.totalSize > 0 ? (
                  <>
                    <span>·</span>
                    <span>
                      {formatSize(item.bytesTransferred)} / {formatSize(item.totalSize)}
                    </span>
                  </>
                ) : item.status === "completed" && item.size > 0 && item.totalSize === 0 ? (
                  <>
                    <span>·</span>
                    <span>{formatSize(item.size)}</span>
                  </>
                ) : null}
                {item.error && (
                  <>
                    <span>·</span>
                    <span className="truncate" style={{ color: "#f87171" }}>
                      {item.error}
                    </span>
                  </>
                )}
              </div>
            </div>

            {item.status === "transferring" ? (
              <span
                className="text-[0.625rem] font-mono font-bold shrink-0"
                style={{ color: statusColor }}
              >
                {statusText}
              </span>
            ) : (
              <StatusIcon className="text-sm shrink-0" style={{ color: statusColor }} />
            )}
          </div>

          {(item.status === "transferring" || item.status === "paused") &&
            (item.kind === "directory" ? (item.itemCountTotal ?? 0) > 0 : item.totalSize > 0) && (
              <div
                className="mt-1 h-1 rounded-full overflow-hidden"
                style={{ backgroundColor: "var(--df-border)" }}
              >
                <div
                  className="h-full rounded-full transition-all duration-200"
                  style={{
                    width: `${progress}%`,
                    backgroundColor: dirColor,
                    opacity: item.status === "paused" ? 0.45 : 0.8,
                  }}
                />
              </div>
            )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-[180px]">
        <ContextMenuItem onClick={() => onPause(item.id)} disabled={!canPause}>
          <MdPause className="mr-2 text-[0.875rem]" />
          {t("fileTransfer.pause")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onResume(item.id)} disabled={!canResume}>
          <MdPlayArrow className="mr-2 text-[0.875rem]" />
          {t("fileTransfer.resume")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onRetry(item)} disabled={!canRetry}>
          <MdRefresh className="mr-2 text-[0.875rem]" />
          {t("fileTransfer.retry")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onCancel(item.id)} disabled={!canCancel}>
          <MdCancel className="mr-2 text-[0.875rem]" />
          {t("fileTransfer.cancel")}
        </ContextMenuItem>
        {item.direction === "download" && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem onClick={() => onOpenTargetDirectory(item.id)}>
              <MdFolder className="mr-2 text-[0.875rem]" />
              {t("fileTransfer.openTargetDirectory")}
            </ContextMenuItem>
          </>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem
          variant="destructive"
          onClick={() => onDelete(item.id)}
          disabled={!canDelete}
        >
          <MdDelete className="mr-2 text-[0.875rem]" />
          {t("fileTransfer.delete")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

export default function FileTransfer({ activeSessionId }: FileTransferProps) {
  const { t } = useTranslation();
  const {
    transfers,
    clearCompleted,
    removeTransfer,
    pauseTransfer,
    resumeTransfer,
    cancelTransfer,
    retryTransfer,
  } = useTransfer();
  const { appSettings } = useApp();
  const [resolvedDownloadDir, setResolvedDownloadDir] = useState("");

  useEffect(() => {
    downloadDir()
      .then(setResolvedDownloadDir)
      .catch(() => {});
  }, []);

  const displayPath = appSettings.transfer.download_path || resolvedDownloadDir;
  const visibleTransfers = useMemo(() => {
    const topLevelTransfers = transfers.filter((transfer) => !transfer.parentId);
    const filteredTransfers = activeSessionId
      ? topLevelTransfers.filter(
          (transfer) =>
            transfer.sessionId === activeSessionId ||
            transfer.status === "transferring" ||
            transfer.status === "paused",
        )
      : topLevelTransfers;

    return [...filteredTransfers].sort((a, b) => b.timestamp - a.timestamp);
  }, [activeSessionId, transfers]);

  const hasRunning = visibleTransfers.some((transfer) => transfer.status === "transferring");
  const hasPaused = visibleTransfers.some((transfer) => transfer.status === "paused");
  const hasActive = visibleTransfers.some(
    (transfer) => transfer.status === "transferring" || transfer.status === "paused",
  );
  const hasCompleted = visibleTransfers.some((transfer) => transfer.status === "completed");

  const handlePauseAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter((transfer) => transfer.status === "transferring")
        .map((transfer) => pauseTransfer(transfer.id)),
    );
  }, [pauseTransfer, visibleTransfers]);

  const handleResumeAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter((transfer) => transfer.status === "paused")
        .map((transfer) => resumeTransfer(transfer.id)),
    );
  }, [resumeTransfer, visibleTransfers]);

  const handleCancelAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter((transfer) => transfer.status === "transferring" || transfer.status === "paused")
        .map((transfer) => cancelTransfer(transfer.id)),
    );
  }, [cancelTransfer, visibleTransfers]);

  const handleOpenDownloadDir = useCallback(async () => {
    try {
      await invoke("open_download_dir");
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const handleOpenTargetDirectory = useCallback(async (transferId: string) => {
    try {
      await invoke("open_transfer_target_directory", { transferId });
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  return (
    <aside
      className="h-full flex flex-col overflow-hidden"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
    >
      <PanelHeader
        title={t("panel.fileTransfer")}
        titleClassName="shrink-0 whitespace-nowrap"
        actions={
          <>
            <HeaderActionButton
              label={t("fileTransfer.pauseAll")}
              icon={MdPause}
              onClick={handlePauseAll}
              disabled={!hasRunning}
            />
            <HeaderActionButton
              label={t("fileTransfer.resumeAll")}
              icon={MdPlayArrow}
              onClick={handleResumeAll}
              disabled={!hasPaused}
            />
            <HeaderActionButton
              label={t("fileTransfer.cancelAll")}
              icon={MdCancel}
              onClick={handleCancelAll}
              disabled={!hasActive}
            />
            <HeaderActionButton
              label={t("fileTransfer.clearCompleted")}
              icon={MdPlaylistRemove}
              onClick={clearCompleted}
              disabled={!hasCompleted}
            />
          </>
        }
      />

      <div className="flex-1 overflow-y-auto p-1 text-sm terminal-scroll">
        {!activeSessionId ? (
          <div className="text-center py-8 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
            <MdFolderOff className="text-xl block mx-auto mb-2" />
            <div className="text-sm block mb-2">{t("fileExplorer.connectToSession")}</div>
          </div>
        ) : visibleTransfers.length === 0 ? (
          <div className="text-center py-8 text-xs" style={{ color: "var(--df-text-dimmed)" }}>
            <MdSwapHoriz className="text-xl block mx-auto mb-2" />
            <div className="text-sm block mb-2">{t("fileTransfer.noTransfers")}</div>
          </div>
        ) : (
          <div className="space-y-0.5">
            {visibleTransfers.map((item) => (
              <TransferRow
                key={item.id}
                item={item}
                onPause={(id) => void pauseTransfer(id)}
                onResume={(id) => void resumeTransfer(id)}
                onRetry={(transfer) => void retryTransfer(transfer)}
                onCancel={(id) => void cancelTransfer(id)}
                onDelete={removeTransfer}
                onOpenTargetDirectory={(id) => void handleOpenTargetDirectory(id)}
              />
            ))}
          </div>
        )}
      </div>

      {displayPath && (
        <Tooltip>
          <TooltipTrigger asChild>
            <div
              className="shrink-0 border-t px-2 py-1.5 font-mono text-[0.75rem] leading-tight"
              style={{
                borderColor: "var(--df-border)",
                color: "var(--df-text-dimmed)",
              }}
              onClick={() => void handleOpenDownloadDir()}
            >
              <div className="truncate">{displayPath}</div>
            </div>
          </TooltipTrigger>
          <TooltipContent side="top">{t("fileTransfer.downloadPath")}</TooltipContent>
        </Tooltip>
      )}
    </aside>
  );
}

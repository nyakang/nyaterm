import { downloadDir } from "@tauri-apps/api/path";
import {
  type ElementType,
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  MdBlock,
  MdContentCopy,
  MdDelete,
  MdDeleteSweep,
  MdDownload,
  MdFolder,
  MdFolderOff,
  MdPause,
  MdPlayArrow,
  MdPlaylistRemove,
  MdRefresh,
  MdSwapHoriz,
  MdUpload,
} from "react-icons/md";
import { toast } from "sonner";
import DeleteTransferDialog from "@/components/dialog/file-explorer/DeleteTransferDialog";
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

function formatRate(bytesPerSecond: number): string {
  if (bytesPerSecond <= 0) return "0 B/s";
  if (bytesPerSecond < 1024) return `${Math.round(bytesPerSecond)} B/s`;
  if (bytesPerSecond < 1024 * 1024) return `${(bytesPerSecond / 1024).toFixed(1)} KB/s`;
  if (bytesPerSecond < 1024 * 1024 * 1024) {
    return `${(bytesPerSecond / (1024 * 1024)).toFixed(1)} MB/s`;
  }
  return `${(bytesPerSecond / (1024 * 1024 * 1024)).toFixed(1)} GB/s`;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function getTransferDisplayRank(transfer: TransferItem): number {
  if (transfer.status === "transferring") return 0;
  if (transfer.status === "queued") return 1;
  return 2;
}

function isBackendModemTransfer(transfer: TransferItem): boolean {
  return transfer.source === "zmodem" || transfer.source === "serial_modem";
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
  selected,
  onSelect,
  onPause,
  onResume,
  onRetry,
  onCancel,
  onDelete,
  onOpenTargetDirectory,
}: {
  item: TransferItem;
  selected: boolean;
  onSelect: (id: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRetry: (item: TransferItem) => void;
  onCancel: (id: string) => void;
  onDelete: (item: TransferItem) => void;
  onOpenTargetDirectory: (id: string) => void;
}) {
  const { t } = useTranslation();
  const DirIcon =
    item.kind === "directory"
      ? MdFolder
      : item.direction === "copy"
        ? MdContentCopy
        : item.direction === "upload"
          ? MdUpload
          : MdDownload;
  const dirColor =
    item.direction === "copy" ? "#a78bfa" : item.direction === "upload" ? "#4ade80" : "#60a5fa";
  const hasByteProgress = item.totalSize > 0;
  const byteProgress = hasByteProgress
    ? Math.min(100, Math.round((item.bytesTransferred / item.totalSize) * 100))
    : 0;
  const progress =
    item.kind === "directory"
      ? hasByteProgress
        ? byteProgress
        : item.itemCountTotal && item.itemCountTotal > 0
          ? Math.min(100, Math.round(((item.itemCountCompleted ?? 0) / item.itemCountTotal) * 100))
          : item.status === "completed"
            ? 100
            : 0
      : item.totalSize > 0
        ? Math.min(100, Math.round((item.bytesTransferred / item.totalSize) * 100))
        : 0;
  const isModemTransfer = isBackendModemTransfer(item);
  const canPause = !isModemTransfer && item.status === "transferring";
  const canPauseQueued = !isModemTransfer && item.status === "queued";
  const canResume = !isModemTransfer && item.status === "paused";
  const canRetry = !isModemTransfer && (item.status === "error" || item.status === "cancelled");
  const canCancel =
    !isModemTransfer &&
    (item.status === "queued" || item.status === "transferring" || item.status === "paused");
  const canDelete = isModemTransfer
    ? item.status !== "transferring"
    : !canCancel || item.status === "queued" || item.queueState === "pending";

  let statusColor = "#facc15";
  let statusText = formatRate(item.speedBytesPerSec ?? 0);

  if (
    item.source === "serial_modem" &&
    item.status === "transferring" &&
    item.bytesTransferred === 0
  ) {
    statusText = t("terminal.serialModemWaiting");
  }

  if (item.status === "queued") {
    statusColor = "#a1a1aa";
    statusText = t("fileTransfer.queued");
  } else if (item.status === "paused") {
    statusColor = "#fb923c";
    statusText = t("fileTransfer.paused");
  } else if (item.status === "completed") {
    statusColor = "#4ade80";
    statusText = t("fileTransfer.completed");
  } else if (item.status === "error") {
    statusColor = "#f87171";
    statusText = t("fileTransfer.error");
  } else if (item.status === "cancelled") {
    statusColor = "#a1a1aa";
    statusText = t("fileTransfer.cancelled");
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          className="rounded transition-colors px-2 py-1.5"
          style={{
            backgroundColor: selected ? "var(--df-bg-hover)" : "var(--df-bg-panel)",
            outline: selected ? "1px solid var(--df-primary)" : undefined,
          }}
          onMouseDown={() => onSelect(item.id)}
          onMouseEnter={(e) => {
            if (!selected) {
              e.currentTarget.style.backgroundColor = "var(--df-bg-hover)";
            }
          }}
          onMouseLeave={(e) => {
            if (!selected) {
              e.currentTarget.style.backgroundColor = "var(--df-bg-panel)";
            }
          }}
          title={item.error || `${item.fileName} — ${statusText}`}
        >
          <div className="flex items-center gap-2">
            <DirIcon className="text-sm shrink-0" style={{ color: dirColor }} />

            <div className="flex-1 min-w-0">
              <div className="text-xs truncate" style={{ color: "var(--df-text)" }}>
                {item.fileName}
              </div>
              <div
                className="flex min-w-0 items-center gap-1 overflow-hidden whitespace-nowrap text-[0.625rem]"
                style={{ color: "var(--df-text-dimmed)" }}
              >
                <span className="shrink-0">{formatTime(item.timestamp)}</span>
                {item.kind === "directory" ? (
                  item.itemCountTotal !== undefined && (
                    <>
                      <span className="shrink-0">·</span>
                      <span className="truncate">
                        {t("fileTransfer.directoryProgress", {
                          completed: item.itemCountCompleted ?? 0,
                          total: item.itemCountTotal,
                        })}
                      </span>
                    </>
                  )
                ) : item.totalSize > 0 ? (
                  <>
                    <span className="shrink-0">·</span>
                    <span className="truncate">
                      {formatSize(item.bytesTransferred)} / {formatSize(item.totalSize)}
                    </span>
                  </>
                ) : item.status === "completed" && item.size > 0 && item.totalSize === 0 ? (
                  <>
                    <span className="shrink-0">·</span>
                    <span className="truncate">{formatSize(item.size)}</span>
                  </>
                ) : null}
                {item.error && (
                  <>
                    <span className="shrink-0">·</span>
                    <span className="truncate" style={{ color: "#f87171" }}>
                      {item.error}
                    </span>
                  </>
                )}
              </div>
            </div>

            {item.status === "transferring" ? (
              <span
                className="w-[4.25rem] shrink-0 text-right font-mono text-[0.625rem] font-bold tabular-nums"
                style={{ color: statusColor }}
              >
                {statusText}
              </span>
            ) : (
              <span
                className="shrink-0 rounded px-1.5 py-0.5 text-[0.625rem]"
                style={{
                  color: statusColor,
                  backgroundColor: "color-mix(in srgb, currentColor 12%, transparent)",
                }}
              >
                {statusText}
              </span>
            )}
          </div>

          {(item.status === "transferring" || item.status === "paused") &&
            (item.kind === "directory"
              ? hasByteProgress || (item.itemCountTotal ?? 0) > 0
              : item.totalSize > 0) && (
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
        {!isModemTransfer && (
          <>
            <ContextMenuItem
              onClick={() => onPause(item.id)}
              disabled={!canPause && !canPauseQueued}
            >
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
              <MdBlock className="mr-2 text-[0.875rem]" />
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
          </>
        )}
        <ContextMenuItem variant="destructive" onClick={() => onDelete(item)} disabled={!canDelete}>
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
  const [selectedTransferId, setSelectedTransferId] = useState<string | null>(null);
  const [pendingDeleteTransfer, setPendingDeleteTransfer] = useState<TransferItem | null>(null);
  const listContainerRef = useRef<HTMLDivElement>(null);
  const previousActiveSessionIdRef = useRef(activeSessionId);

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
            transfer.sourceSessionId === activeSessionId ||
            transfer.targetSessionId === activeSessionId,
        )
      : topLevelTransfers;

    return [...filteredTransfers].sort((a, b) => {
      const rankDiff = getTransferDisplayRank(a) - getTransferDisplayRank(b);
      if (rankDiff !== 0) return rankDiff;
      return b.timestamp - a.timestamp;
    });
  }, [activeSessionId, transfers]);

  useEffect(() => {
    if (previousActiveSessionIdRef.current === activeSessionId) return;
    previousActiveSessionIdRef.current = activeSessionId;
    setSelectedTransferId(null);
    setPendingDeleteTransfer(null);
  });

  useEffect(() => {
    if (
      selectedTransferId &&
      !visibleTransfers.some((transfer) => transfer.id === selectedTransferId)
    ) {
      setSelectedTransferId(null);
    }
  }, [selectedTransferId, visibleTransfers]);

  const selectedTransfer = useMemo(
    () => visibleTransfers.find((transfer) => transfer.id === selectedTransferId) ?? null,
    [selectedTransferId, visibleTransfers],
  );

  const canDeleteTransfer = useCallback((transfer: TransferItem) => {
    if (isBackendModemTransfer(transfer)) {
      return transfer.status !== "transferring";
    }
    const canCancel =
      transfer.status === "queued" ||
      transfer.status === "transferring" ||
      transfer.status === "paused";
    return !canCancel || transfer.status === "queued" || transfer.queueState === "pending";
  }, []);

  const requestDeleteTransfer = useCallback(
    (transfer: TransferItem) => {
      if (!canDeleteTransfer(transfer)) return;
      setSelectedTransferId(transfer.id);
      setPendingDeleteTransfer(transfer);
    },
    [canDeleteTransfer],
  );

  const handleConfirmDeleteTransfer = useCallback(() => {
    if (!pendingDeleteTransfer) return;
    removeTransfer(pendingDeleteTransfer.id);
    setPendingDeleteTransfer(null);
    setSelectedTransferId(null);
  }, [pendingDeleteTransfer, removeTransfer]);

  const handleListKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (pendingDeleteTransfer) return;

      if (
        event.key !== "Delete" ||
        event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey
      ) {
        return;
      }

      const target = event.target;
      if (
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT")
      ) {
        return;
      }

      const transfer = selectedTransfer ?? visibleTransfers[0];
      if (!transfer || !canDeleteTransfer(transfer)) {
        return;
      }

      event.preventDefault();
      event.stopPropagation();
      requestDeleteTransfer(transfer);
    },
    [
      canDeleteTransfer,
      pendingDeleteTransfer,
      requestDeleteTransfer,
      selectedTransfer,
      visibleTransfers,
    ],
  );

  const hasRunning = visibleTransfers.some(
    (transfer) =>
      !isBackendModemTransfer(transfer) &&
      (transfer.status === "transferring" || transfer.status === "queued"),
  );
  const hasPaused = visibleTransfers.some(
    (transfer) => !isBackendModemTransfer(transfer) && transfer.status === "paused",
  );
  const hasActive = visibleTransfers.some(
    (transfer) =>
      !isBackendModemTransfer(transfer) &&
      (transfer.status === "queued" ||
        transfer.status === "transferring" ||
        transfer.status === "paused"),
  );
  const hasCompleted = visibleTransfers.some((transfer) => transfer.status === "completed");
  const hasClearable = visibleTransfers.some(
    (transfer) =>
      transfer.status !== "queued" &&
      transfer.status !== "transferring" &&
      transfer.status !== "paused",
  );

  const handlePauseAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter(
          (transfer) =>
            !isBackendModemTransfer(transfer) &&
            (transfer.status === "transferring" || transfer.status === "queued"),
        )
        .map((transfer) => pauseTransfer(transfer.id)),
    );
  }, [pauseTransfer, visibleTransfers]);

  const handleClearAll = useCallback(() => {
    visibleTransfers
      .filter(
        (transfer) =>
          transfer.status !== "queued" &&
          transfer.status !== "transferring" &&
          transfer.status !== "paused",
      )
      .forEach((transfer) => {
        removeTransfer(transfer.id);
      });
  }, [removeTransfer, visibleTransfers]);

  const handleResumeAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter((transfer) => !isBackendModemTransfer(transfer) && transfer.status === "paused")
        .map((transfer) => resumeTransfer(transfer.id)),
    );
  }, [resumeTransfer, visibleTransfers]);

  const handleCancelAll = useCallback(() => {
    void Promise.all(
      visibleTransfers
        .filter(
          (transfer) =>
            !isBackendModemTransfer(transfer) &&
            (transfer.status === "queued" ||
              transfer.status === "transferring" ||
              transfer.status === "paused"),
        )
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
      className="nyaterm-wallpaper-transparent-surface h-full flex flex-col overflow-hidden"
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
              icon={MdBlock}
              onClick={handleCancelAll}
              disabled={!hasActive}
            />
            <HeaderActionButton
              label={t("fileTransfer.clearCompleted")}
              icon={MdPlaylistRemove}
              onClick={clearCompleted}
              disabled={!hasCompleted}
            />
            <HeaderActionButton
              label={t("fileTransfer.clearAll")}
              icon={MdDeleteSweep}
              onClick={handleClearAll}
              disabled={!hasClearable}
            />
          </>
        }
      />

      <div
        ref={listContainerRef}
        className="flex-1 overflow-y-auto p-1 text-sm terminal-scroll outline-none"
        tabIndex={activeSessionId ? 0 : -1}
        onMouseDown={() => {
          if (activeSessionId) {
            listContainerRef.current?.focus();
          }
        }}
        onKeyDown={handleListKeyDown}
      >
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
                selected={selectedTransferId === item.id}
                onSelect={setSelectedTransferId}
                onPause={(id) => void pauseTransfer(id)}
                onResume={(id) => void resumeTransfer(id)}
                onRetry={(transfer) => void retryTransfer(transfer)}
                onCancel={(id) => void cancelTransfer(id)}
                onDelete={requestDeleteTransfer}
                onOpenTargetDirectory={(id) => void handleOpenTargetDirectory(id)}
              />
            ))}
          </div>
        )}
      </div>

      <DeleteTransferDialog
        transfer={pendingDeleteTransfer}
        onCancel={() => setPendingDeleteTransfer(null)}
        onConfirm={handleConfirmDeleteTransfer}
      />

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

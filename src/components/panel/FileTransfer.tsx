import { useTranslation } from "react-i18next";
import {
  MdCheckCircle,
  MdDeleteSweep,
  MdDownload,
  MdError,
  MdFolderOff,
  MdPlaylistRemove,
  MdSwapHoriz,
  MdSync,
  MdUpload,
} from "react-icons/md";
import PanelHeader from "@/components/layout/PanelHeader";
import { type TransferItem, useTransfer } from "../../context/TransferContext";

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

function TransferRow({ item }: { item: TransferItem }) {
  const { t } = useTranslation();

  const DirIcon = item.direction === "upload" ? MdUpload : MdDownload;
  const dirColor = item.direction === "upload" ? "#4ade80" : "#60a5fa";

  const iconMap: Record<string, React.ElementType> = {
    sync: MdSync,
    check_circle: MdCheckCircle,
    error: MdError,
  };

  let statusIcon: string;
  let statusColor: string;
  let statusText: string;
  const progress =
    item.totalSize > 0
      ? Math.min(100, Math.round((item.bytesTransferred / item.totalSize) * 100))
      : 0;

  switch (item.status) {
    case "transferring":
      statusIcon = "sync";
      statusColor = "#facc15";
      statusText = `${progress}%`;
      break;
    case "completed":
      statusIcon = "check_circle";
      statusColor = "#4ade80";
      statusText = t("fileTransfer.completed");
      break;
    case "error":
      statusIcon = "error";
      statusColor = "#f87171";
      statusText = t("fileTransfer.error");
      break;
  }

  return (
    <div
      className="rounded transition-colors px-2 py-1.5"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
      onMouseEnter={(e) => (e.currentTarget.style.backgroundColor = "var(--df-bg-hover)")}
      onMouseLeave={(e) => (e.currentTarget.style.backgroundColor = "var(--df-bg-panel)")}
      title={item.error || `${item.fileName} — ${statusText}`}
    >
      <div className="flex items-center gap-2">
        {/* Direction icon */}
        <DirIcon className="text-sm shrink-0" style={{ color: dirColor }} />

        {/* File name + info */}
        <div className="flex-1 min-w-0">
          <div className="text-xs truncate" style={{ color: "var(--df-text)" }}>
            {item.fileName}
          </div>
          <div
            className="flex items-center gap-1 text-[0.625rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            <span>{formatTime(item.timestamp)}</span>
            {item.status === "transferring" && item.totalSize > 0 && (
              <>
                <span>·</span>
                <span>
                  {formatSize(item.bytesTransferred)} / {formatSize(item.totalSize)}
                </span>
              </>
            )}
            {item.status === "completed" && item.size > 0 && (
              <>
                <span>·</span>
                <span>{formatSize(item.size)}</span>
              </>
            )}
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

        {/* Status icon or percentage */}
        {item.status === "transferring" ? (
          <span
            className="text-[0.625rem] font-mono font-bold shrink-0"
            style={{ color: statusColor }}
          >
            {statusText}
          </span>
        ) : (
          (() => {
            const Icon = iconMap[statusIcon];
            return Icon ? (
              <Icon className="text-sm shrink-0" style={{ color: statusColor }} />
            ) : null;
          })()
        )}
      </div>

      {/* Progress bar */}
      {item.status === "transferring" && (
        <div
          className="mt-1 h-1 rounded-full overflow-hidden"
          style={{ backgroundColor: "var(--df-border)" }}
        >
          <div
            className="h-full rounded-full transition-all duration-200"
            style={{
              width: `${progress}%`,
              backgroundColor: dirColor,
              opacity: 0.8,
            }}
          />
        </div>
      )}
    </div>
  );
}

export default function FileTransfer({ activeSessionId }: FileTransferProps) {
  const { t } = useTranslation();
  const { transfers, clearCompleted, clearAll } = useTransfer();

  // Filter transfers: show all if no active session, else show session-specific + active
  const visibleTransfers = activeSessionId
    ? transfers.filter((tr) => tr.sessionId === activeSessionId || tr.status === "transferring")
    : transfers;

  const hasCompleted = visibleTransfers.some((tr) => tr.status !== "transferring");

  return (
    <aside
      className="h-full flex flex-col overflow-hidden"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
    >
      <PanelHeader
        title={t("panel.fileTransfer")}
        meta={visibleTransfers.length}
        actions={
          <>
            {hasCompleted && (
              <MdPlaylistRemove
                className="text-sm cursor-pointer transition-opacity hover:opacity-80"
                style={{ color: "var(--df-text-muted)" }}
                onClick={clearCompleted}
                title={t("fileTransfer.clearCompleted")}
              />
            )}
            {visibleTransfers.length > 0 && (
              <MdDeleteSweep
                className="text-sm cursor-pointer transition-opacity hover:opacity-80"
                style={{ color: "var(--df-text-muted)" }}
                onClick={clearAll}
                title={t("fileTransfer.clearAll")}
              />
            )}
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
              <TransferRow key={item.id} item={item} />
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}

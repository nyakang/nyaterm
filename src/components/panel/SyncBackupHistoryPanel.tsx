import { listen } from "@tauri-apps/api/event";
import type { TFunction } from "i18next";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdCloudDownload,
  MdCloudUpload,
  MdContentCopy,
  MdExpandLess,
  MdExpandMore,
  MdHistory,
  MdRefresh,
  MdWarning,
} from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  DEFAULT_CLOUD_SYNC_STATUS,
  formatCloudProvider,
  formatDuration,
  formatTimestamp,
  shortValue,
} from "@/lib/cloudSync";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type { CloudConflictPreview, CloudSyncHistoryEntry, CloudSyncStatus } from "@/types/global";

type SyncState = "idle" | "running" | "success" | "failed" | "conflict" | "disabled";
type EntryKind = "sync" | "backup";
type EntryStatus = "success" | "failed" | "conflict" | "running";

function statusDotClass(status: string): string {
  switch (status) {
    case "running":
      return "bg-blue-500";
    case "success":
      return "bg-emerald-500";
    case "failed":
      return "bg-red-500";
    case "conflict":
      return "bg-amber-500";
    default:
      return "bg-muted-foreground/40";
  }
}

function statusTextClass(status: string): string {
  switch (status) {
    case "running":
      return "text-blue-500";
    case "success":
      return "text-emerald-500";
    case "failed":
      return "text-red-500";
    case "conflict":
      return "text-amber-500";
    default:
      return "text-muted-foreground";
  }
}

function kindTextClass(kind: string): string {
  switch (kind) {
    case "sync":
      return "text-blue-500";
    case "backup":
      return "text-violet-500";
    default:
      return "text-muted-foreground";
  }
}

function normalizeHistoryMessage(value?: string | null) {
  return (value ?? "").replace(/\s+/g, " ").trim();
}

function extractFirstSentence(value: string) {
  const normalized = normalizeHistoryMessage(value);
  if (!normalized) return "";
  const match = normalized.match(/^(.{1,120}?[.!?])(?:\s|$)/);
  return match?.[1]?.trim() ?? "";
}

function extractHttpStatus(value: string) {
  const normalized = normalizeHistoryMessage(value);
  const match = normalized.match(/\b([45]\d{2}\s+[A-Z][A-Za-z]+(?:\s+[A-Z][A-Za-z]+)*)\b/);
  return match?.[1] ?? null;
}

function buildHistorySummary(
  entry: CloudSyncHistoryEntry,
  kindLabels: Record<string, string>,
  statusLabels: Record<string, string>,
  t: TFunction,
) {
  const normalized = normalizeHistoryMessage(entry.message);
  if (!normalized) {
    return t("settings.historySummaryKindStatus", {
      kind: kindLabels[entry.kind as EntryKind] ?? entry.kind,
      status: statusLabels[entry.status as EntryStatus] ?? entry.status,
    });
  }

  const firstSentence = extractFirstSentence(entry.message);
  if (firstSentence && firstSentence.length <= 110) {
    return firstSentence;
  }

  if (!entry.message.includes("\n") && normalized.length <= 110) {
    return normalized;
  }

  const genericSummary = t("settings.historySummaryKindStatus", {
    kind: kindLabels[entry.kind as EntryKind] ?? entry.kind,
    status: statusLabels[entry.status as EntryStatus] ?? entry.status,
  });
  const httpStatus = extractHttpStatus(entry.message);

  if (!httpStatus) {
    return genericSummary;
  }

  return t("settings.historySummaryWithStatus", {
    summary: genericSummary,
    status: httpStatus,
  });
}

interface StatRowProps {
  label: string;
  value: string;
}

function StatRow({ label, value }: StatRowProps) {
  return (
    <div className="min-w-0 rounded-md border border-border/50 bg-background/30 px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-0.5 truncate font-mono text-xs text-foreground/80">{value}</div>
    </div>
  );
}

interface HistoryDetailFieldProps {
  label: string;
  value: string;
  monospace?: boolean;
}

function HistoryDetailField({ label, value, monospace = false }: HistoryDetailFieldProps) {
  return (
    <div className="rounded-md border border-border/50 bg-background/30 px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className={cn("mt-0.5 text-sm text-foreground/85", monospace && "font-mono text-xs")}>
        {value}
      </div>
    </div>
  );
}

function SyncBackupHistoryPanel() {
  const { t } = useTranslation();
  const [history, setHistory] = useState<CloudSyncHistoryEntry[]>([]);
  const [status, setStatus] = useState<CloudSyncStatus>(DEFAULT_CLOUD_SYNC_STATUS);
  const [loading, setLoading] = useState(true);
  const [runningAction, setRunningAction] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [nextHistory, nextStatus] = await Promise.all([
        invoke<CloudSyncHistoryEntry[]>("list_cloud_sync_history"),
        invoke<CloudSyncStatus>("get_cloud_sync_status"),
      ]);
      setHistory(nextHistory);
      setStatus(nextStatus);
    } catch (error) {
      toast.error(getErrorMessage(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const unsubs = [
      listen<CloudSyncHistoryEntry[]>("cloud-sync-history-changed", (event) => {
        setHistory(event.payload);
      }),
      listen<CloudSyncStatus>("cloud-sync-status-changed", (event) => {
        setStatus(event.payload);
      }),
      listen<CloudConflictPreview | null>("cloud-sync-conflict", (event) => {
        const conflict = event.payload;
        if (!conflict) return;

        setStatus((current) => ({
          ...current,
          state: "conflict",
          message: conflict.message,
          conflict,
        }));
      }),
    ];

    return () => {
      unsubs.forEach((promise) => {
        promise.then((unlisten) => unlisten());
      });
    };
  }, []);

  const handleResolveConflict = useCallback(
    async (action: "download_remote" | "upload_local") => {
      setRunningAction(action);
      try {
        await invoke("resolve_cloud_sync_conflict", { action });
        await refresh();
        toast.success(
          action === "download_remote"
            ? t("settings.syncResolveDownloadSuccess")
            : t("settings.syncResolveUploadSuccess"),
        );
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setRunningAction(null);
      }
    },
    [refresh, t],
  );

  const runSyncAction = useCallback(
    async (actionKey: string, successMessage: string, task: () => Promise<void>) => {
      if (!status.enabled) {
        toast.error(t("settings.syncEnableFirst"));
        return;
      }

      setRunningAction(actionKey);
      try {
        await task();
        await refresh();
        toast.success(successMessage);
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setRunningAction(null);
      }
    },
    [refresh, status.enabled, t],
  );

  const syncActionDisabled = loading || runningAction !== null || status.state === "running";
  const canRunSyncAction = status.enabled && !syncActionDisabled;

  const kindLabels = useMemo(
    () => ({
      sync: t("settings.historyKindSync"),
      backup: t("settings.historyKindBackup"),
    }),
    [t],
  );

  const statusLabels = useMemo(
    () => ({
      success: t("settings.syncState.success"),
      conflict: t("settings.syncState.conflict"),
      running: t("settings.syncState.running"),
      failed: t("settings.syncState.failed"),
      idle: t("settings.syncState.idle"),
      disabled: t("settings.syncState.disabled"),
    }),
    [t],
  );

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <PanelHeader
        title={t("panel.syncBackupHistory")}
        actions={
          <>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() =>
                void runSyncAction("push", t("settings.syncPushSuccess"), () =>
                  invoke("sync_push_now"),
                )
              }
              disabled={!canRunSyncAction}
              title={t("settings.syncPushNow")}
              aria-busy={runningAction === "push"}
            >
              <MdCloudUpload
                className={cn("h-3.5 w-3.5", runningAction === "push" && "opacity-60")}
              />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() =>
                void runSyncAction("pull", t("settings.syncPullSuccess"), () =>
                  invoke("sync_pull_now"),
                )
              }
              disabled={!canRunSyncAction}
              title={t("settings.syncPullNow")}
              aria-busy={runningAction === "pull"}
            >
              <MdCloudDownload
                className={cn("h-3.5 w-3.5", runningAction === "pull" && "opacity-60")}
              />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={() => void refresh()}
              disabled={loading}
              title={t("resourceMonitor.refresh")}
            >
              <MdRefresh className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            </Button>
          </>
        }
      />

      <div className="terminal-scroll flex-1 overflow-y-auto">
        <div className="border-b border-border/60 px-3 py-2.5">
          <div className="flex min-w-0 items-center gap-2 text-sm">
            <span
              className={cn(
                "h-2 w-2 shrink-0 rounded-full",
                statusDotClass(status.state as SyncState),
              )}
            />
            <span className="shrink-0 text-muted-foreground">
              {t("settings.historyCurrentState")}
            </span>
            <span className={cn("truncate font-medium", statusTextClass(status.state))}>
              {t(`settings.syncState.${status.state}`, status.state)}
            </span>
            <span className="text-muted-foreground/50">·</span>
            <span className="shrink-0 text-muted-foreground">
              {formatCloudProvider(status.provider)}
            </span>
          </div>

          {status.message && !status.conflict ? (
            <div className="mt-1.5 break-words pl-4 text-sm leading-5 text-muted-foreground">
              {status.message}
            </div>
          ) : null}
        </div>

        {status.conflict ? (
          <div className="m-2 rounded-md border border-amber-500/30 bg-amber-500/5">
            <div className="flex items-center gap-2 border-b border-amber-500/20 px-3 py-2.5">
              <MdWarning className="shrink-0 text-base text-amber-500" />
              <span className="flex-1 text-sm font-medium text-amber-500">
                {t("settings.syncConflictTitle")}
              </span>
            </div>

            <div className="px-3 py-3 text-sm leading-6 text-muted-foreground">
              {status.conflict.message}
            </div>

            <div className="grid grid-cols-1 gap-2 px-3 pb-3">
              <StatRow
                label={t("settings.remoteSnapshot")}
                value={shortValue(status.conflict.remote_revision, 10)}
              />
              <StatRow
                label={t("settings.remoteDeviceLabel")}
                value={status.conflict.remote_device_id}
              />
              <StatRow
                label={t("settings.payloadHashLabel")}
                value={shortValue(status.conflict.remote_payload_hash, 10)}
              />
            </div>

            <div className="flex gap-2 px-3 pb-3">
              <Button
                variant="outline"
                size="sm"
                className="flex-1 text-xs"
                onClick={() => void handleResolveConflict("download_remote")}
                disabled={runningAction !== null}
              >
                {t("settings.downloadRemoteVersion")}
              </Button>
              <Button
                size="sm"
                className="flex-1 text-xs"
                onClick={() => void handleResolveConflict("upload_local")}
                disabled={runningAction !== null}
              >
                {t("settings.uploadLocalVersion")}
              </Button>
            </div>
          </div>
        ) : null}

        <div className="p-2">
          <div className="overflow-hidden rounded-md border border-border/60 bg-background/20">
            {loading ? (
              <div className="flex flex-col items-center justify-center gap-2 py-10 text-muted-foreground/60">
                <MdRefresh className="animate-spin text-2xl" />
                <span className="text-sm">{t("common.loading")}</span>
              </div>
            ) : history.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-2 py-10 text-center">
                <MdHistory className="text-3xl text-muted-foreground/25" />
                <span className="text-sm text-muted-foreground">{t("settings.noSyncHistory")}</span>
              </div>
            ) : (
              history.map((entry) => (
                <HistoryEntryRow
                  key={entry.id}
                  entry={entry}
                  kindLabels={kindLabels}
                  statusLabels={statusLabels}
                  t={t}
                />
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

interface HistoryEntryRowProps {
  entry: CloudSyncHistoryEntry;
  kindLabels: Record<string, string>;
  statusLabels: Record<string, string>;
  t: TFunction;
}

const HistoryEntryRow = memo(function HistoryEntryRow({
  entry,
  kindLabels,
  statusLabels,
  t,
}: HistoryEntryRowProps) {
  const [detailsOpen, setDetailsOpen] = useState(false);

  const summary = buildHistorySummary(entry, kindLabels, statusLabels, t);
  const normalizedMessage = normalizeHistoryMessage(entry.message);
  const isProblemEntry = entry.status === "failed" || entry.status === "conflict";
  const hasMessageDetails =
    Boolean(normalizedMessage) &&
    (isProblemEntry || normalizeHistoryMessage(summary) !== normalizedMessage);
  const hasExpandableDetails = hasMessageDetails || Boolean(entry.revision);

  const handleCopyMessage = useCallback(() => {
    navigator.clipboard
      .writeText(entry.message)
      .then(() => {
        toast.success(t("settings.historyCopyErrorSuccess"));
      })
      .catch((error) => {
        toast.error(getErrorMessage(error));
      });
  }, [entry.message, t]);

  return (
    <div className="border-b border-border/50 px-3 py-3 last:border-b-0">
      <div className="flex items-start gap-2.5">
        <span
          className={cn("mt-2 h-1.5 w-1.5 shrink-0 rounded-full", statusDotClass(entry.status))}
        />

        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className={cn("shrink-0 text-xs font-medium", kindTextClass(entry.kind))}>
              {kindLabels[entry.kind as EntryKind] ?? entry.kind}
            </span>

            <span className={cn("shrink-0 text-xs", statusTextClass(entry.status))}>
              {statusLabels[entry.status as EntryStatus] ?? entry.status}
            </span>

            <span className="ml-auto shrink-0 text-xs text-muted-foreground/70">
              {formatTimestamp(entry.timestamp_ms) ?? t("settings.never")}
            </span>
          </div>

          <div className="mt-1 truncate text-sm font-medium text-foreground/90">{summary}</div>

          <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <span>
              {t("settings.triggerLabel")} {entry.trigger}
            </span>

            {entry.provider ? (
              <span>
                {t("settings.providerLabel")} {formatCloudProvider(entry.provider)}
              </span>
            ) : null}

            {entry.duration_ms != null ? (
              <span>
                {t("settings.durationLabel")}{" "}
                {formatDuration(entry.duration_ms) ?? t("settings.none")}
              </span>
            ) : null}
          </div>

          {hasExpandableDetails ? (
            <Collapsible open={detailsOpen} onOpenChange={setDetailsOpen}>
              <div className="mt-2 flex flex-wrap gap-3">
                <CollapsibleTrigger asChild>
                  <Button
                    variant="ghost"
                    size="xs"
                    className="h-6 px-0 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground"
                  >
                    {detailsOpen ? <MdExpandLess /> : <MdExpandMore />}
                    {detailsOpen
                      ? t("settings.historyHideDetails")
                      : t("settings.historyViewDetails")}
                  </Button>
                </CollapsibleTrigger>

                {hasMessageDetails ? (
                  <Button
                    variant="ghost"
                    size="xs"
                    className="h-6 px-0 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground"
                    onClick={handleCopyMessage}
                  >
                    <MdContentCopy />
                    {t("settings.historyCopyError")}
                  </Button>
                ) : null}
              </div>

              <CollapsibleContent className="mt-2 space-y-2 overflow-hidden data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0">
                {hasMessageDetails ? (
                  <pre
                    className={cn(
                      "whitespace-pre-wrap break-words rounded-md p-2 font-mono text-xs leading-5",
                      isProblemEntry
                        ? "bg-red-500/5 text-red-300"
                        : "bg-muted/25 text-muted-foreground",
                    )}
                  >
                    {entry.message}
                  </pre>
                ) : null}

                {entry.revision ? (
                  <div className="grid gap-2">
                    <HistoryDetailField
                      label={t("settings.revisionLabel")}
                      value={entry.revision}
                      monospace
                    />
                  </div>
                ) : null}
              </CollapsibleContent>
            </Collapsible>
          ) : null}
        </div>
      </div>
    </div>
  );
});

export default memo(SyncBackupHistoryPanel);

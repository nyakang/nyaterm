import { listen } from "@tauri-apps/api/event";
import type { TFunction } from "i18next";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdCheckCircle,
  MdContentCopy,
  MdError,
  MdExpandLess,
  MdExpandMore,
  MdFilterList,
  MdHistory,
  MdHourglassEmpty,
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

function stateConfig(state: SyncState): {
  icon: React.ReactNode;
  dot: string;
  badge: string;
} {
  switch (state) {
    case "running":
      return {
        icon: <MdRefresh className="animate-spin" />,
        dot: "bg-blue-500",
        badge: "bg-blue-500/15 text-blue-500 ring-1 ring-blue-500/30",
      };
    case "success":
      return {
        icon: <MdCheckCircle />,
        dot: "bg-emerald-500",
        badge: "bg-emerald-500/15 text-emerald-500 ring-1 ring-emerald-500/30",
      };
    case "failed":
      return {
        icon: <MdError />,
        dot: "bg-red-500",
        badge: "bg-red-500/15 text-red-500 ring-1 ring-red-500/30",
      };
    case "conflict":
      return {
        icon: <MdWarning />,
        dot: "bg-amber-500",
        badge: "bg-amber-500/15 text-amber-500 ring-1 ring-amber-500/30",
      };
    case "disabled":
      return {
        icon: <MdHourglassEmpty />,
        dot: "bg-muted-foreground/40",
        badge: "bg-muted/60 text-muted-foreground ring-1 ring-border/50",
      };
    default:
      return {
        icon: <MdHourglassEmpty />,
        dot: "bg-muted-foreground/30",
        badge: "bg-muted/60 text-muted-foreground ring-1 ring-border/50",
      };
  }
}

function entryStatusBadge(status: string): string {
  switch (status) {
    case "success":
      return "bg-emerald-500/15 text-emerald-500 ring-1 ring-emerald-500/30";
    case "failed":
      return "bg-red-500/15 text-red-500 ring-1 ring-red-500/30";
    case "conflict":
      return "bg-amber-500/15 text-amber-500 ring-1 ring-amber-500/30";
    case "running":
      return "bg-blue-500/15 text-blue-500 ring-1 ring-blue-500/30";
    default:
      return "bg-muted/60 text-muted-foreground ring-1 ring-border/50";
  }
}

function kindBadge(kind: string): string {
  switch (kind) {
    case "sync":
      return "bg-primary/10 text-primary ring-1 ring-primary/25";
    case "backup":
      return "bg-violet-500/15 text-violet-500 ring-1 ring-violet-500/30";
    default:
      return "bg-muted/60 text-muted-foreground ring-1 ring-border/50";
  }
}

function historyCardTone(status: string) {
  switch (status) {
    case "failed":
      return "border-red-500/20 bg-red-500/5 hover:border-red-500/35";
    case "conflict":
      return "border-amber-500/20 bg-amber-500/5 hover:border-amber-500/35";
    default:
      return "border-border/50 bg-card/40 hover:border-border/80 hover:bg-card/60";
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
    <div className="rounded-lg border border-border/40 bg-muted/15 px-3 py-2.5">
      <div className="text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground/80">
        {label}
      </div>
      <div className="mt-1 truncate text-sm font-medium text-foreground/85">{value}</div>
    </div>
  );
}

interface FilterChipProps {
  active: boolean;
  count: number;
  label: string;
  onClick: () => void;
}

function FilterChip({ active, count, label, onClick }: FilterChipProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "inline-flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs font-medium transition-colors",
        active
          ? "border-primary bg-primary text-primary-foreground"
          : "border-border/60 bg-muted/30 text-muted-foreground hover:border-border hover:bg-muted/55 hover:text-foreground",
      )}
    >
      <span>{label}</span>
      <span
        className={cn(
          "rounded-full px-1.5 py-0.5 text-xs font-semibold",
          active
            ? "bg-primary-foreground/15 text-primary-foreground"
            : "bg-background/70 text-foreground/70",
        )}
      >
        {count}
      </span>
    </button>
  );
}

interface HistoryMetaChipProps {
  label: string;
  value: string;
  monospace?: boolean;
}

function HistoryMetaChip({ label, value, monospace = false }: HistoryMetaChipProps) {
  return (
    <span className="inline-flex max-w-full items-center gap-1 rounded-full bg-muted/45 px-2 py-1 text-xs text-muted-foreground">
      <span className="shrink-0 text-muted-foreground/70">{label}</span>
      <span className={cn("truncate text-foreground/75", monospace && "font-mono")}>{value}</span>
    </span>
  );
}

interface HistoryDetailFieldProps {
  label: string;
  value: string;
  monospace?: boolean;
}

function HistoryDetailField({ label, value, monospace = false }: HistoryDetailFieldProps) {
  return (
    <div className="rounded-md border border-border/40 bg-background/35 px-3 py-2">
      <div className="text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground/75">
        {label}
      </div>
      <div className={cn("mt-1 text-sm text-foreground/85", monospace && "font-mono text-xs")}>
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
  const [filterKind, setFilterKind] = useState<string>("all");
  const [filterStatus, setFilterStatus] = useState<string>("all");
  const [searchText, setSearchText] = useState("");

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

  const stateCfg = stateConfig(status.state as SyncState);

  const counts = useMemo(() => {
    const next = {
      total: history.length,
      sync: 0,
      backup: 0,
      success: 0,
      failed: 0,
      conflict: 0,
    };

    for (const entry of history) {
      if (entry.kind === "sync") next.sync += 1;
      if (entry.kind === "backup") next.backup += 1;
      if (entry.status === "success") next.success += 1;
      if (entry.status === "failed") next.failed += 1;
      if (entry.status === "conflict") next.conflict += 1;
    }

    return next;
  }, [history]);

  const filtered = useMemo(() => {
    const query = searchText.trim().toLowerCase();
    return history.filter((entry) => {
      if (filterKind !== "all" && entry.kind !== filterKind) return false;
      if (filterStatus !== "all" && entry.status !== filterStatus) return false;
      if (!query) return true;

      const haystack = [
        entry.message,
        entry.trigger,
        entry.provider,
        entry.revision,
        entry.kind,
        entry.status,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();

      return haystack.includes(query);
    });
  }, [filterKind, filterStatus, history, searchText]);

  const hasFilters = filterKind !== "all" || filterStatus !== "all" || searchText.trim().length > 0;

  const clearFilters = useCallback(() => {
    setFilterKind("all");
    setFilterStatus("all");
    setSearchText("");
  }, []);

  const kindFilterOptions = useMemo(
    () => [
      { value: "all", label: t("settings.historyAll"), count: counts.total },
      { value: "sync", label: kindLabels.sync, count: counts.sync },
      { value: "backup", label: kindLabels.backup, count: counts.backup },
    ],
    [counts.backup, counts.sync, counts.total, kindLabels, t],
  );

  const statusFilterOptions = useMemo(
    () => [
      { value: "all", label: t("settings.historyAll"), count: counts.total },
      { value: "success", label: statusLabels.success, count: counts.success },
      { value: "failed", label: statusLabels.failed, count: counts.failed },
      { value: "conflict", label: statusLabels.conflict, count: counts.conflict },
    ],
    [counts.conflict, counts.failed, counts.success, counts.total, statusLabels, t],
  );

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <PanelHeader
        title={t("panel.syncBackupHistory")}
        actions={
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => void refresh()}
            disabled={loading}
            title={t("resourceMonitor.refresh")}
          >
            <MdRefresh className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
          </Button>
        }
      />

      <div className="terminal-scroll flex-1 overflow-y-auto">
        <div className="px-2 pt-2">
          <div className="overflow-hidden rounded-xl border border-border/60 bg-card/50">
            <div className="flex items-start gap-3 px-3 py-3">
              <span className={cn("mt-1 h-2 w-2 rounded-full shrink-0", stateCfg.dot)} />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground">
                    {t("settings.historyCurrentState")}
                  </span>
                  <span
                    className={cn(
                      "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-semibold",
                      stateCfg.badge,
                    )}
                  >
                    <span className="text-sm">{stateCfg.icon}</span>
                    {t(`settings.syncState.${status.state}`, status.state)}
                  </span>
                  <span className="rounded-full bg-muted/45 px-2 py-1 text-xs text-muted-foreground">
                    {formatCloudProvider(status.provider)}
                  </span>
                </div>

                {status.message && !status.conflict ? (
                  <div className="mt-2 rounded-lg border border-border/40 bg-muted/20 px-3 py-2 text-sm leading-5 text-muted-foreground">
                    {status.message}
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        </div>

        {status.conflict ? (
          <div className="px-2 pt-2">
            <div className="overflow-hidden rounded-xl border border-amber-500/35 bg-amber-500/8">
              <div className="flex items-center gap-2 border-b border-amber-500/20 px-3 py-3">
                <MdWarning className="shrink-0 text-lg text-amber-500" />
                <span className="flex-1 text-sm font-semibold text-amber-500">
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
          </div>
        ) : null}

        {history.length > 0 ? (
          <div className="px-2 pt-2">
            <div className="rounded-xl border border-border/60 bg-card/45 p-3">
              <div className="relative">
                <MdFilterList className="absolute left-3 top-1/2 -translate-y-1/2 text-base text-muted-foreground/50" />
                <input
                  type="text"
                  value={searchText}
                  onChange={(event) => setSearchText(event.target.value)}
                  placeholder={t("settings.historySearchPlaceholder")}
                  className="h-10 w-full rounded-lg border border-border/60 bg-muted/25 pl-9 pr-3 text-sm placeholder:text-muted-foreground/45 focus:outline-none focus:ring-1 focus:ring-primary/50"
                />
              </div>

              <div className="mt-3 space-y-3">
                <div>
                  <div className="mb-2 text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground/80">
                    {t("settings.historyFilterKind")}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {kindFilterOptions.map((option) => (
                      <FilterChip
                        key={option.value}
                        active={filterKind === option.value}
                        count={option.count}
                        label={option.label}
                        onClick={() => setFilterKind(option.value)}
                      />
                    ))}
                  </div>
                </div>

                <div>
                  <div className="mb-2 text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground/80">
                    {t("settings.historyFilterStatus")}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {statusFilterOptions.map((option) => (
                      <FilterChip
                        key={option.value}
                        active={filterStatus === option.value}
                        count={option.count}
                        label={option.label}
                        onClick={() => setFilterStatus(option.value)}
                      />
                    ))}
                  </div>
                </div>

                {hasFilters ? (
                  <div className="flex justify-end">
                    <button
                      type="button"
                      className="text-xs font-medium text-primary underline underline-offset-2"
                      onClick={clearFilters}
                    >
                      {t("settings.historyClearFilters")}
                    </button>
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        ) : null}

        <div className="space-y-2 p-2 pb-4">
          {loading ? (
            <div className="flex flex-col items-center justify-center gap-2 py-10 text-muted-foreground/60">
              <MdRefresh className="animate-spin text-2xl" />
              <span className="text-sm">{t("common.loading")}</span>
            </div>
          ) : history.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border/60 py-10 text-center">
              <MdHistory className="text-3xl text-muted-foreground/25" />
              <span className="text-sm text-muted-foreground">{t("settings.noSyncHistory")}</span>
            </div>
          ) : filtered.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border/60 py-8 text-center">
              <MdFilterList className="text-2xl text-muted-foreground/25" />
              <div className="text-sm font-medium text-foreground/80">
                {t("settings.historyNoResultsTitle")}
              </div>
              <div className="max-w-[18rem] text-sm text-muted-foreground">
                {t("settings.noSyncHistoryMatchFilters")}
              </div>
              {hasFilters ? (
                <button
                  type="button"
                  className="text-xs font-medium text-primary underline underline-offset-2"
                  onClick={clearFilters}
                >
                  {t("settings.historyClearFilters")}
                </button>
              ) : null}
            </div>
          ) : (
            <>
              {hasFilters ? (
                <div className="px-1 text-right text-xs text-muted-foreground/70">
                  {t("settings.historyFilteredCount", {
                    shown: filtered.length,
                    total: history.length,
                  })}
                </div>
              ) : null}

              {filtered.map((entry) => (
                <HistoryEntryCard
                  key={entry.id}
                  entry={entry}
                  kindLabels={kindLabels}
                  statusLabels={statusLabels}
                  t={t}
                />
              ))}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

interface HistoryEntryCardProps {
  entry: CloudSyncHistoryEntry;
  kindLabels: Record<string, string>;
  statusLabels: Record<string, string>;
  t: TFunction;
}

const HistoryEntryCard = memo(function HistoryEntryCard({
  entry,
  kindLabels,
  statusLabels,
  t,
}: HistoryEntryCardProps) {
  const [detailsOpen, setDetailsOpen] = useState(false);

  const summary = buildHistorySummary(entry, kindLabels, statusLabels, t);
  const normalizedMessage = normalizeHistoryMessage(entry.message);
  const hasMessageDetails =
    Boolean(normalizedMessage) && normalizeHistoryMessage(summary) !== normalizedMessage;
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
    <div
      className={cn(
        "overflow-hidden rounded-xl border transition-colors",
        historyCardTone(entry.status),
      )}
    >
      <div className="flex items-center gap-2 px-3 pt-3">
        <span
          className={cn(
            "rounded-full px-2 py-1 text-xs font-semibold uppercase tracking-[0.12em]",
            kindBadge(entry.kind),
          )}
        >
          {kindLabels[entry.kind as EntryKind] ?? entry.kind}
        </span>
        <span
          className={cn(
            "rounded-full px-2 py-1 text-xs font-semibold",
            entryStatusBadge(entry.status),
          )}
        >
          {statusLabels[entry.status as EntryStatus] ?? entry.status}
        </span>
        <span className="ml-auto shrink-0 text-xs font-mono text-muted-foreground/70">
          {formatTimestamp(entry.timestamp_ms) ?? t("settings.never")}
        </span>
      </div>

      <div className="space-y-2.5 px-3 pb-3 pt-2.5">
        <div className="text-sm font-medium leading-6 text-foreground/90">{summary}</div>

        <div className="flex flex-wrap gap-1.5">
          <HistoryMetaChip label={t("settings.triggerLabel")} value={entry.trigger} />
          {entry.provider ? (
            <HistoryMetaChip
              label={t("settings.providerLabel")}
              value={formatCloudProvider(entry.provider)}
            />
          ) : null}
          {entry.duration_ms != null ? (
            <HistoryMetaChip
              label={t("settings.durationLabel")}
              value={formatDuration(entry.duration_ms) ?? t("settings.none")}
            />
          ) : null}
        </div>

        {hasExpandableDetails ? (
          <Collapsible open={detailsOpen} onOpenChange={setDetailsOpen}>
            <div className="flex flex-wrap gap-1.5">
              <CollapsibleTrigger asChild>
                <Button
                  variant="ghost"
                  size="xs"
                  className="h-7 px-2.5 text-xs text-muted-foreground hover:text-foreground"
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
                  className="h-7 px-2.5 text-xs text-muted-foreground hover:text-foreground"
                  onClick={handleCopyMessage}
                >
                  <MdContentCopy />
                  {t("settings.historyCopyError")}
                </Button>
              ) : null}
            </div>

            <CollapsibleContent className="mt-2 space-y-2 overflow-hidden data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0">
              {hasMessageDetails ? (
                <div className="rounded-lg border border-border/40 bg-muted/20 p-3">
                  <pre className="whitespace-pre-wrap break-words font-mono text-xs leading-5 text-foreground/80">
                    {entry.message}
                  </pre>
                </div>
              ) : null}

              {entry.revision ? (
                <div className="grid gap-2 sm:grid-cols-2">
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
  );
});

export default memo(SyncBackupHistoryPanel);

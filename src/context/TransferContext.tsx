import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import { filterEnqueueUploadRequests } from "@/lib/transferDuplicateResolution";
import { pruneRetainedTransfers as pruneTransferMap } from "@/lib/transferRetention";

export type TransferDirection = "upload" | "download" | "copy";
export type TransferKind = "file" | "directory";
export type TransferSource = "sftp" | "zmodem" | "serial_modem";
export type TransferStatus =
  | "queued"
  | "transferring"
  | "paused"
  | "completed"
  | "error"
  | "cancelled";

export interface EnqueueUploadRequest {
  sessionId: string;
  fileName: string;
  localPath: string;
  remotePath: string;
  kind: TransferKind;
  duplicateStrategyOverride?: string;
}

export interface EnqueueDownloadRequest {
  sessionId: string;
  fileName: string;
  localPath: string;
  remotePath: string;
  kind: TransferKind;
}

export interface CopyEndpointRequest {
  sessionId: string;
  kind: "local" | "remote";
  path: string;
}

export interface EnqueueCopyRequest {
  fileName: string;
  kind: TransferKind;
  source: CopyEndpointRequest;
  target: CopyEndpointRequest;
  duplicateStrategyOverride?: string;
}

interface QueuedTransferRequest {
  sessionId: string;
  fileName: string;
  localPath: string;
  remotePath: string;
  kind: TransferKind;
  direction: TransferDirection;
  sourceEndpoint?: CopyEndpointRequest;
  targetEndpoint?: CopyEndpointRequest;
  duplicateStrategyOverride?: string;
}

export interface ExternalTransferProgress {
  id: string;
  sessionId: string;
  fileName: string;
  direction: TransferDirection;
  bytesTransferred: number;
  totalSize: number;
  localPath?: string;
  remotePath?: string;
  source?: Exclude<TransferSource, "sftp">;
}

export interface TransferItem {
  id: string;
  sessionId: string;
  fileName: string;
  remotePath: string;
  localPath: string;
  direction: TransferDirection;
  kind: TransferKind;
  sourceSessionId?: string;
  sourceKind?: "local" | "remote";
  sourcePath?: string;
  targetSessionId?: string;
  targetKind?: "local" | "remote";
  targetPath?: string;
  parentId?: string;
  status: TransferStatus;
  size: number;
  bytesTransferred: number;
  speedBytesPerSec?: number;
  totalSize: number;
  itemCountTotal?: number;
  itemCountCompleted?: number;
  error?: string;
  timestamp: number;
  queueState?: "pending" | "running";
  source?: TransferSource;
}

interface TransferContextValue {
  transfers: TransferItem[];
  clearCompleted: () => void;
  clearAll: () => void;
  removeTransfer: (id: string) => void;
  pauseTransfer: (id: string) => Promise<void>;
  resumeTransfer: (id: string) => Promise<void>;
  cancelTransfer: (id: string) => Promise<void>;
  retryTransfer: (item: TransferItem) => Promise<void>;
  enqueueUploads: (uploads: EnqueueUploadRequest[]) => string[];
  enqueueDownloads: (downloads: EnqueueDownloadRequest[]) => string[];
  enqueueCopies: (copies: EnqueueCopyRequest[]) => string[];
  upsertExternalTransferProgress: (progress: ExternalTransferProgress) => void;
  completeExternalTransfer: (id: string) => void;
  failExternalTransfer: (id: string, reason: string) => void;
}

const TransferContext = createContext<TransferContextValue | null>(null);

/** Backend event payload shape. */
interface TransferEventPayload {
  id: string;
  session_id: string;
  file_name: string;
  remote_path: string;
  local_path: string;
  direction: string;
  kind?: string;
  parent_id?: string;
  status: string;
  size: number;
  bytes_transferred: number;
  total_size: number;
  item_count_total?: number;
  item_count_completed?: number;
  error_msg?: string;
}

function createTransferId() {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `transfer-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function clampTransferConcurrency(value: number) {
  const normalized = Number.isFinite(value) ? Math.floor(value) : 1;
  return Math.min(10, Math.max(1, normalized));
}

function getBackgroundTransferConcurrency(value: number) {
  const configured = clampTransferConcurrency(value);
  return configured > 1 ? configured - 1 : 1;
}

const TRANSFER_SPEED_WINDOW_MS = 3000;

interface TransferSpeedSample {
  bytesTransferred: number;
  timestamp: number;
}

function calculateTransferSpeed(
  samples: TransferSpeedSample[] | undefined,
  bytesTransferred: number,
  timestamp: number,
): { samples: TransferSpeedSample[]; speedBytesPerSec?: number } {
  const previousSamples = samples ?? [];
  const previousSample =
    previousSamples.length > 0 ? previousSamples[previousSamples.length - 1] : undefined;
  const currentSample = { bytesTransferred, timestamp };

  if (previousSample && bytesTransferred < previousSample.bytesTransferred) {
    return { samples: [currentSample], speedBytesPerSec: 0 };
  }

  if (previousSample && bytesTransferred === previousSample.bytesTransferred) {
    const isStale = timestamp - previousSample.timestamp > TRANSFER_SPEED_WINDOW_MS;
    return {
      samples: previousSamples,
      speedBytesPerSec: isStale ? 0 : undefined,
    };
  }

  const windowStart = timestamp - TRANSFER_SPEED_WINDOW_MS;
  const windowSamples = [...previousSamples, currentSample].filter(
    (sample) => sample.timestamp >= windowStart,
  );
  const firstSample = windowSamples[0];
  const elapsedMs = firstSample ? timestamp - firstSample.timestamp : 0;
  const byteDelta = firstSample ? bytesTransferred - firstSample.bytesTransferred : 0;

  return {
    samples: windowSamples,
    speedBytesPerSec:
      elapsedMs > 0 && byteDelta >= 0 ? Math.round((byteDelta * 1000) / elapsedMs) : 0,
  };
}

export function TransferProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const { appSettings } = useApp();
  const [transferMap, setTransferMap] = useState<Map<string, TransferItem>>(() => new Map());
  const transferMapRef = useRef(transferMap);
  const queuedTransfersRef = useRef<Map<string, QueuedTransferRequest>>(new Map());
  const parkedTransferIdsRef = useRef<Set<string>>(new Set());
  const uploadFolderToastIdsRef = useRef<Map<string, string | number>>(new Map());
  const transferSpeedSamplesRef = useRef<Map<string, TransferSpeedSample[]>>(new Map());
  const [queueRevision, setQueueRevision] = useState(0);

  const transfers = useMemo(() => Array.from(transferMap.values()), [transferMap]);

  const cleanupTransferArtifacts = useCallback((id: string) => {
    transferSpeedSamplesRef.current.delete(id);
    queuedTransfersRef.current.delete(id);
    parkedTransferIdsRef.current.delete(id);
    const folderToastId = uploadFolderToastIdsRef.current.get(id);
    if (folderToastId !== undefined) {
      toast.dismiss(folderToastId);
      uploadFolderToastIdsRef.current.delete(id);
    }
  }, []);

  const pruneRetainedTransfers = useCallback(
    (map: Map<string, TransferItem>, now = Date.now()) => {
      return pruneTransferMap(map, cleanupTransferArtifacts, now);
    },
    [cleanupTransferArtifacts],
  );

  useEffect(() => {
    transferMapRef.current = transferMap;
  }, [transferMap]);

  useEffect(() => {
    const unlisten = listen<TransferEventPayload>("transfer-event", (e) => {
      const p = e.payload;
      const kind = (p.kind ?? "file") as TransferKind;
      const now = Date.now();

      if (p.status === "started") {
        if (p.parent_id) {
          setTransferMap((prev) => pruneRetainedTransfers(prev, now));
          return;
        }
        transferSpeedSamplesRef.current.set(p.id, [
          {
            bytesTransferred: p.bytes_transferred,
            timestamp: now,
          },
        ]);
        setTransferMap((prev) => {
          const next = new Map(prev);
          const existing = next.get(p.id);
          next.set(p.id, {
            ...existing,
            id: p.id,
            sessionId: p.session_id,
            fileName: p.file_name,
            remotePath: p.remote_path,
            localPath: p.local_path,
            direction: p.direction as TransferDirection,
            kind,
            parentId: p.parent_id,
            status: "transferring",
            size: 0,
            bytesTransferred: 0,
            speedBytesPerSec: 0,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total,
            itemCountCompleted: p.item_count_completed,
            timestamp: existing?.timestamp ?? now,
            queueState:
              existing?.queueState ??
              (queuedTransfersRef.current.has(p.id) ? "running" : undefined),
            source: existing?.source ?? "sftp",
          });
          return pruneRetainedTransfers(next, now);
        });
        return;
      }

      setTransferMap((prev) => {
        const existing = prev.get(p.id);
        if (!existing) return pruneRetainedTransfers(prev, now);
        const next = new Map(prev);
        let updated: TransferItem;

        if (p.status === "progress") {
          const speed = calculateTransferSpeed(
            transferSpeedSamplesRef.current.get(p.id),
            p.bytes_transferred,
            now,
          );
          transferSpeedSamplesRef.current.set(p.id, speed.samples);
          updated = {
            ...existing,
            status: "transferring",
            bytesTransferred: p.bytes_transferred,
            speedBytesPerSec: speed.speedBytesPerSec ?? existing.speedBytesPerSec ?? 0,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total ?? existing.itemCountTotal,
            itemCountCompleted: p.item_count_completed ?? existing.itemCountCompleted,
          };
        } else if (p.status === "paused") {
          transferSpeedSamplesRef.current.delete(p.id);
          updated = {
            ...existing,
            status: "paused",
            bytesTransferred: p.bytes_transferred,
            speedBytesPerSec: undefined,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total ?? existing.itemCountTotal,
            itemCountCompleted: p.item_count_completed ?? existing.itemCountCompleted,
          };
        } else if (p.status === "resumed") {
          transferSpeedSamplesRef.current.set(p.id, [
            {
              bytesTransferred: p.bytes_transferred,
              timestamp: now,
            },
          ]);
          updated = {
            ...existing,
            status: "transferring",
            bytesTransferred: p.bytes_transferred,
            speedBytesPerSec: 0,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total ?? existing.itemCountTotal,
            itemCountCompleted: p.item_count_completed ?? existing.itemCountCompleted,
          };
        } else if (p.status === "cancelled") {
          transferSpeedSamplesRef.current.delete(p.id);
          updated = {
            ...existing,
            status: "cancelled",
            bytesTransferred: p.bytes_transferred,
            speedBytesPerSec: undefined,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total ?? existing.itemCountTotal,
            itemCountCompleted: p.item_count_completed ?? existing.itemCountCompleted,
            queueState: undefined,
            error: undefined,
            timestamp: now,
          };
        } else {
          if (p.status === "completed" || p.status === "error") {
            transferSpeedSamplesRef.current.delete(p.id);
          }
          updated = {
            ...existing,
            status: p.status as TransferStatus,
            size: p.size,
            bytesTransferred: p.bytes_transferred,
            speedBytesPerSec: undefined,
            totalSize: p.total_size,
            itemCountTotal: p.item_count_total ?? existing.itemCountTotal,
            itemCountCompleted: p.item_count_completed ?? existing.itemCountCompleted,
            queueState:
              p.status === "completed" || p.status === "error" ? undefined : existing.queueState,
            error: p.error_msg,
            timestamp: p.status === "completed" || p.status === "error" ? now : existing.timestamp,
          };
        }

        next.set(p.id, updated);
        return pruneRetainedTransfers(next, now);
      });

      if (
        p.status === "paused" ||
        p.status === "cancelled" ||
        p.status === "completed" ||
        p.status === "error"
      ) {
        if (p.status !== "paused") {
          parkedTransferIdsRef.current.delete(p.id);
          queuedTransfersRef.current.delete(p.id);
        }
        setQueueRevision((revision) => revision + 1);
      }

      if (p.status === "completed" && !p.parent_id) {
        if (p.direction === "download") {
          toast.success(
            kind === "directory"
              ? t("fileTransfer.downloadFolderCompleted")
              : t("fileTransfer.downloadCompleted"),
            {
              description: p.local_path,
            },
          );
          return;
        }

        if (p.direction === "upload") {
          const folderToastId = uploadFolderToastIdsRef.current.get(p.id);
          if (folderToastId !== undefined) {
            toast.dismiss(folderToastId);
            uploadFolderToastIdsRef.current.delete(p.id);
          }
          if (kind === "directory" && p.error_msg) {
            toast.warning(t("fileTransfer.uploadFolderCompleted"), {
              description: p.error_msg,
            });
            return;
          }
          toast.success(
            kind === "directory"
              ? t("fileTransfer.uploadFolderCompleted")
              : t("fileTransfer.uploadCompleted"),
            {
              description: p.remote_path,
            },
          );
        }
        return;
      }

      if (
        p.status === "started" &&
        p.direction === "upload" &&
        !p.parent_id &&
        kind === "directory"
      ) {
        const toastId = toast.message(t("fileTransfer.uploadFolderStarted", { name: p.file_name }));
        uploadFolderToastIdsRef.current.set(p.id, toastId);
        return;
      }

      if (p.status === "error" && p.direction === "upload" && !p.parent_id) {
        const folderToastId = uploadFolderToastIdsRef.current.get(p.id);
        if (folderToastId !== undefined) {
          toast.dismiss(folderToastId);
          uploadFolderToastIdsRef.current.delete(p.id);
        }
        toast.error(
          kind === "directory"
            ? t("fileTransfer.uploadFolderFailed", { name: p.file_name })
            : t("fileTransfer.uploadFailed", { name: p.file_name }),
          {
            description: p.error_msg,
          },
        );
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [pruneRetainedTransfers, t]);

  useEffect(() => {
    void queueRevision;
    const copyConcurrency = Math.max(
      1,
      Math.min(
        getBackgroundTransferConcurrency(appSettings.transfer.download_threads),
        getBackgroundTransferConcurrency(appSettings.transfer.upload_threads),
      ),
    );
    const maxRunningByDirection: Record<TransferDirection, number> = {
      download: getBackgroundTransferConcurrency(appSettings.transfer.download_threads),
      upload: getBackgroundTransferConcurrency(appSettings.transfer.upload_threads),
      copy: copyConcurrency,
    };
    const runningByDirection: Record<TransferDirection, number> = {
      download: 0,
      upload: 0,
      copy: 0,
    };

    for (const transfer of transferMap.values()) {
      if (transfer.queueState === "running" && transfer.status === "transferring") {
        runningByDirection[transfer.direction] += 1;
      }
    }

    const queuedCandidates = Array.from(transferMap.values())
      .filter(
        (transfer) =>
          queuedTransfersRef.current.has(transfer.id) ||
          parkedTransferIdsRef.current.has(transfer.id),
      )
      .filter((transfer) => transfer.status === "queued");

    for (const nextQueued of queuedCandidates) {
      const direction = nextQueued.direction;
      if (runningByDirection[direction] >= maxRunningByDirection[direction]) {
        continue;
      }

      if (parkedTransferIdsRef.current.has(nextQueued.id)) {
        parkedTransferIdsRef.current.delete(nextQueued.id);
        runningByDirection[direction] += 1;
        setTransferMap((prev) => {
          const existing = prev.get(nextQueued.id);
          if (!existing || existing.status !== "queued") {
            return prev;
          }
          const next = new Map(prev);
          next.set(nextQueued.id, {
            ...existing,
            status: "transferring",
            queueState: "running",
            error: undefined,
          });
          return next;
        });

        void invoke("resume_transfer", { transferId: nextQueued.id }).catch((error) => {
          toast.error(String(error));
          setTransferMap((prev) => {
            const existing = prev.get(nextQueued.id);
            if (!existing || existing.status === "completed" || existing.status === "cancelled") {
              return prev;
            }
            const next = new Map(prev);
            next.set(nextQueued.id, {
              ...existing,
              status: "error",
              queueState: undefined,
              error: String(error),
              timestamp: Date.now(),
            });
            return pruneRetainedTransfers(next);
          });
          setQueueRevision((revision) => revision + 1);
        });
        continue;
      }

      const request = queuedTransfersRef.current.get(nextQueued.id);
      if (!request) {
        continue;
      }

      queuedTransfersRef.current.delete(nextQueued.id);
      runningByDirection[direction] += 1;
      setTransferMap((prev) => {
        const existing = prev.get(nextQueued.id);
        if (!existing || existing.status !== "queued") {
          return prev;
        }
        const next = new Map(prev);
        next.set(nextQueued.id, {
          ...existing,
          status: "transferring",
          queueState: "running",
          error: undefined,
        });
        return next;
      });

      void (async () => {
        try {
          if (request.direction === "copy") {
            if (!request.sourceEndpoint || !request.targetEndpoint) {
              throw new Error("Copy transfer is missing source or target endpoint");
            }
            await invoke("copy_file_entry", {
              request: {
                source: request.sourceEndpoint,
                target: request.targetEndpoint,
                fileName: request.fileName,
                isDirectory: request.kind === "directory",
                transferId: nextQueued.id,
                duplicateStrategyOverride: request.duplicateStrategyOverride,
              },
            });
          } else if (request.direction === "upload" && request.kind === "directory") {
            await invoke("upload_local_directory", {
              sessionId: request.sessionId,
              localPath: request.localPath,
              remotePath: request.remotePath,
              transferId: nextQueued.id,
              duplicateStrategyOverride: request.duplicateStrategyOverride,
            });
          } else if (request.direction === "upload") {
            await invoke("upload_local_file", {
              sessionId: request.sessionId,
              localPath: request.localPath,
              remotePath: request.remotePath,
              transferId: nextQueued.id,
              duplicateStrategyOverride: request.duplicateStrategyOverride,
            });
          } else if (request.kind === "directory") {
            await invoke("download_remote_directory", {
              sessionId: request.sessionId,
              remotePath: request.remotePath,
              localPath: request.localPath,
              transferId: nextQueued.id,
            });
          } else {
            await invoke("download_remote_file", {
              sessionId: request.sessionId,
              remotePath: request.remotePath,
              localPath: request.localPath,
              transferId: nextQueued.id,
            });
          }
        } catch (error) {
          setTransferMap((prev) => {
            const existing = prev.get(nextQueued.id);
            if (
              !existing ||
              existing.status === "completed" ||
              existing.status === "cancelled" ||
              existing.status === "error"
            ) {
              return prev;
            }
            const next = new Map(prev);
            next.set(nextQueued.id, {
              ...existing,
              status: "error",
              queueState: undefined,
              error: String(error),
              timestamp: Date.now(),
            });
            return pruneRetainedTransfers(next);
          });
        } finally {
          setQueueRevision((revision) => revision + 1);
        }
      })();
    }
  }, [
    appSettings.transfer.download_threads,
    appSettings.transfer.upload_threads,
    pruneRetainedTransfers,
    queueRevision,
    transferMap,
  ]);

  const clearCompleted = useCallback(() => {
    setTransferMap((prev) => {
      const next = new Map(prev);
      for (const [id, t] of prev) {
        if (t.status === "completed") {
          cleanupTransferArtifacts(id);
          next.delete(id);
        }
      }
      if (next.size === prev.size) return pruneRetainedTransfers(prev);
      return pruneRetainedTransfers(next);
    });
  }, [cleanupTransferArtifacts, pruneRetainedTransfers]);

  const clearAll = useCallback(() => {
    for (const toastId of uploadFolderToastIdsRef.current.values()) {
      toast.dismiss(toastId);
    }
    queuedTransfersRef.current.clear();
    parkedTransferIdsRef.current.clear();
    transferSpeedSamplesRef.current.clear();
    uploadFolderToastIdsRef.current.clear();
    setTransferMap(new Map());
  }, []);

  const removeTransfer = useCallback(
    (id: string) => {
      if (parkedTransferIdsRef.current.has(id)) {
        cleanupTransferArtifacts(id);
        void invoke("cancel_transfer", { transferId: id }).catch((error) => {
          toast.error(String(error));
        });
      }
      cleanupTransferArtifacts(id);
      setTransferMap((prev) => {
        if (!prev.has(id)) return prev;
        const next = new Map(prev);
        next.delete(id);
        return next;
      });
    },
    [cleanupTransferArtifacts],
  );

  const pauseTransfer = useCallback(async (id: string) => {
    const existing = transferMapRef.current.get(id);
    if (queuedTransfersRef.current.has(id) && existing?.status === "queued") {
      setTransferMap((prev) => {
        const queued = prev.get(id);
        if (!queued || queued.status !== "queued") return prev;
        const next = new Map(prev);
        transferSpeedSamplesRef.current.delete(id);
        next.set(id, { ...queued, status: "paused", speedBytesPerSec: undefined });
        return next;
      });
      return;
    }

    if (parkedTransferIdsRef.current.has(id) && existing?.status === "queued") {
      setTransferMap((prev) => {
        const queued = prev.get(id);
        if (!queued || queued.status !== "queued") return prev;
        const next = new Map(prev);
        transferSpeedSamplesRef.current.delete(id);
        next.set(id, {
          ...queued,
          status: "paused",
          speedBytesPerSec: undefined,
          queueState: "running",
        });
        return next;
      });
      setQueueRevision((revision) => revision + 1);
      return;
    }

    try {
      await invoke("pause_transfer", { transferId: id });
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const resumeTransfer = useCallback(async (id: string) => {
    const existing = transferMapRef.current.get(id);
    if (queuedTransfersRef.current.has(id) && existing?.status === "paused") {
      setTransferMap((prev) => {
        const queued = prev.get(id);
        if (!queued || queued.status !== "paused") return prev;
        const next = new Map(prev);
        transferSpeedSamplesRef.current.delete(id);
        next.set(id, { ...queued, status: "queued", speedBytesPerSec: undefined });
        return next;
      });
      setQueueRevision((revision) => revision + 1);
      return;
    }

    if (existing?.queueState === "running" && existing.status === "paused") {
      parkedTransferIdsRef.current.add(id);
      setTransferMap((prev) => {
        const paused = prev.get(id);
        if (!paused || paused.status !== "paused") return prev;
        const next = new Map(prev);
        transferSpeedSamplesRef.current.delete(id);
        next.set(id, {
          ...paused,
          status: "queued",
          speedBytesPerSec: undefined,
          queueState: "pending",
        });
        return next;
      });
      setQueueRevision((revision) => revision + 1);
      return;
    }

    try {
      await invoke("resume_transfer", { transferId: id });
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const cancelTransfer = useCallback(
    async (id: string) => {
      const existing = transferMapRef.current.get(id);
      if (queuedTransfersRef.current.has(id)) {
        cleanupTransferArtifacts(id);
        setTransferMap((prev) => {
          const queued = prev.get(id);
          if (!queued || queued.status === "completed" || queued.status === "error") return prev;
          const next = new Map(prev);
          next.set(id, {
            ...queued,
            status: "cancelled",
            speedBytesPerSec: undefined,
            queueState: undefined,
            error: undefined,
            timestamp: Date.now(),
          });
          return pruneRetainedTransfers(next);
        });
        setQueueRevision((revision) => revision + 1);
        return;
      }

      if (parkedTransferIdsRef.current.has(id)) {
        cleanupTransferArtifacts(id);
        try {
          await invoke("cancel_transfer", { transferId: id });
        } catch (error) {
          toast.error(String(error));
        }
        setTransferMap((prev) => {
          const parked = prev.get(id);
          if (!parked || parked.status === "completed" || parked.status === "error") return prev;
          const next = new Map(prev);
          next.set(id, {
            ...parked,
            status: "cancelled",
            speedBytesPerSec: undefined,
            queueState: undefined,
            error: undefined,
            timestamp: Date.now(),
          });
          return pruneRetainedTransfers(next);
        });
        setQueueRevision((revision) => revision + 1);
        return;
      }

      if (!existing) return;

      try {
        await invoke("cancel_transfer", { transferId: id });
        cleanupTransferArtifacts(id);
        setTransferMap((prev) => {
          const existing = prev.get(id);
          if (!existing || existing.status === "completed" || existing.status === "error") {
            return prev;
          }
          const next = new Map(prev);
          next.set(id, {
            ...existing,
            status: "cancelled",
            speedBytesPerSec: undefined,
            queueState: undefined,
            error: undefined,
            timestamp: Date.now(),
          });
          return pruneRetainedTransfers(next);
        });
        setQueueRevision((revision) => revision + 1);
      } catch (error) {
        toast.error(String(error));
      }
    },
    [cleanupTransferArtifacts, pruneRetainedTransfers],
  );

  const retryTransfer = useCallback(async (item: TransferItem) => {
    parkedTransferIdsRef.current.delete(item.id);
    transferSpeedSamplesRef.current.delete(item.id);
    queuedTransfersRef.current.set(item.id, {
      sessionId: item.sessionId,
      fileName: item.fileName,
      localPath: item.localPath,
      remotePath: item.remotePath,
      kind: item.kind,
      direction: item.direction,
      sourceEndpoint:
        item.direction === "copy" && item.sourceSessionId && item.sourceKind && item.sourcePath
          ? {
              sessionId: item.sourceSessionId,
              kind: item.sourceKind,
              path: item.sourcePath,
            }
          : undefined,
      targetEndpoint:
        item.direction === "copy" && item.targetSessionId && item.targetKind && item.targetPath
          ? {
              sessionId: item.targetSessionId,
              kind: item.targetKind,
              path: item.targetPath,
            }
          : undefined,
    });
    setTransferMap((prev) => {
      const next = new Map(prev);
      next.set(item.id, {
        ...item,
        status: "queued",
        bytesTransferred: 0,
        speedBytesPerSec: undefined,
        totalSize: 0,
        itemCountCompleted: undefined,
        itemCountTotal: undefined,
        queueState: "pending",
        error: undefined,
        timestamp: Date.now(),
      });
      return next;
    });
    setQueueRevision((revision) => revision + 1);
  }, []);

  const enqueueTransfers = useCallback((transfers: QueuedTransferRequest[]) => {
    const normalizedTransfers = transfers.filter(
      (transfer) =>
        transfer.sessionId && transfer.localPath && transfer.remotePath && transfer.fileName,
    );
    if (normalizedTransfers.length === 0) {
      return [];
    }

    const ids = normalizedTransfers.map(() => createTransferId());
    setTransferMap((prev) => {
      const next = new Map(prev);
      normalizedTransfers.forEach((transfer, index) => {
        const id = ids[index];
        queuedTransfersRef.current.set(id, transfer);
        next.set(id, {
          id,
          sessionId: transfer.sessionId,
          fileName: transfer.fileName,
          remotePath: transfer.remotePath,
          localPath: transfer.localPath,
          direction: transfer.direction,
          kind: transfer.kind,
          sourceSessionId: transfer.sourceEndpoint?.sessionId,
          sourceKind: transfer.sourceEndpoint?.kind,
          sourcePath: transfer.sourceEndpoint?.path,
          targetSessionId: transfer.targetEndpoint?.sessionId,
          targetKind: transfer.targetEndpoint?.kind,
          targetPath: transfer.targetEndpoint?.path,
          status: "queued",
          size: 0,
          bytesTransferred: 0,
          speedBytesPerSec: undefined,
          totalSize: 0,
          timestamp: Date.now() + index,
          queueState: "pending",
          source: "sftp",
        });
      });
      return next;
    });
    setQueueRevision((revision) => revision + 1);
    return ids;
  }, []);

  const enqueueUploads = useCallback(
    (uploads: EnqueueUploadRequest[]) => {
      void (async () => {
        const filtered = await filterEnqueueUploadRequests(
          uploads,
          appSettings.transfer.duplicate_strategy,
        );
        if (filtered.length === 0) {
          return;
        }
        enqueueTransfers(filtered.map((upload) => ({ ...upload, direction: "upload" as const })));
      })();
      return [];
    },
    [appSettings.transfer.duplicate_strategy, enqueueTransfers],
  );

  const enqueueDownloads = useCallback(
    (downloads: EnqueueDownloadRequest[]) =>
      enqueueTransfers(
        downloads.map((download) => ({ ...download, direction: "download" as const })),
      ),
    [enqueueTransfers],
  );

  const enqueueCopies = useCallback(
    (copies: EnqueueCopyRequest[]) =>
      enqueueTransfers(
        copies.map((copy) => ({
          sessionId: copy.target.sessionId,
          fileName: copy.fileName,
          localPath: copy.target.kind === "local" ? copy.target.path : copy.source.path,
          remotePath: copy.target.kind === "remote" ? copy.target.path : copy.source.path,
          kind: copy.kind,
          direction: "copy" as const,
          sourceEndpoint: copy.source,
          targetEndpoint: copy.target,
          duplicateStrategyOverride: copy.duplicateStrategyOverride,
        })),
      ),
    [enqueueTransfers],
  );

  const upsertExternalTransferProgress = useCallback(
    (progress: ExternalTransferProgress) => {
      const now = Date.now();
      setTransferMap((prev) => {
        const existing = prev.get(progress.id);
        const next = new Map(prev);
        let speedBytesPerSec = existing?.speedBytesPerSec ?? 0;

        if (!existing) {
          transferSpeedSamplesRef.current.set(progress.id, [
            {
              bytesTransferred: progress.bytesTransferred,
              timestamp: now,
            },
          ]);
        } else {
          const speed = calculateTransferSpeed(
            transferSpeedSamplesRef.current.get(progress.id),
            progress.bytesTransferred,
            now,
          );
          transferSpeedSamplesRef.current.set(progress.id, speed.samples);
          speedBytesPerSec = speed.speedBytesPerSec ?? speedBytesPerSec;
        }

        next.set(progress.id, {
          ...existing,
          id: progress.id,
          sessionId: progress.sessionId,
          fileName: progress.fileName,
          remotePath: progress.remotePath ?? existing?.remotePath ?? "",
          localPath: progress.localPath ?? existing?.localPath ?? "",
          direction: progress.direction,
          kind: "file",
          status: "transferring",
          size: existing?.size ?? 0,
          bytesTransferred: progress.bytesTransferred,
          speedBytesPerSec,
          totalSize: progress.totalSize,
          timestamp: existing?.timestamp ?? now,
          queueState: undefined,
          error: undefined,
          source: progress.source ?? "zmodem",
        });
        return pruneRetainedTransfers(next, now);
      });
    },
    [pruneRetainedTransfers],
  );

  const completeExternalTransfer = useCallback(
    (id: string) => {
      transferSpeedSamplesRef.current.delete(id);
      const now = Date.now();
      setTransferMap((prev) => {
        const existing = prev.get(id);
        if (!existing) return pruneRetainedTransfers(prev, now);
        const next = new Map(prev);
        const completedBytes =
          existing.totalSize > 0
            ? Math.max(existing.bytesTransferred, existing.totalSize)
            : existing.bytesTransferred;
        next.set(id, {
          ...existing,
          status: "completed",
          size: existing.totalSize > 0 ? existing.totalSize : existing.size,
          bytesTransferred: completedBytes,
          speedBytesPerSec: undefined,
          queueState: undefined,
          error: undefined,
          timestamp: now,
        });
        return pruneRetainedTransfers(next, now);
      });
    },
    [pruneRetainedTransfers],
  );

  const failExternalTransfer = useCallback(
    (id: string, reason: string) => {
      transferSpeedSamplesRef.current.delete(id);
      const now = Date.now();
      setTransferMap((prev) => {
        const existing = prev.get(id);
        if (!existing) return pruneRetainedTransfers(prev, now);
        const next = new Map(prev);
        next.set(id, {
          ...existing,
          status: "error",
          speedBytesPerSec: undefined,
          queueState: undefined,
          error: reason,
          timestamp: now,
        });
        return pruneRetainedTransfers(next, now);
      });
    },
    [pruneRetainedTransfers],
  );

  const contextValue = useMemo(
    () => ({
      transfers,
      clearCompleted,
      clearAll,
      removeTransfer,
      pauseTransfer,
      resumeTransfer,
      cancelTransfer,
      retryTransfer,
      enqueueUploads,
      enqueueDownloads,
      enqueueCopies,
      upsertExternalTransferProgress,
      completeExternalTransfer,
      failExternalTransfer,
    }),
    [
      transfers,
      clearCompleted,
      clearAll,
      removeTransfer,
      pauseTransfer,
      resumeTransfer,
      cancelTransfer,
      retryTransfer,
      enqueueUploads,
      enqueueDownloads,
      enqueueCopies,
      upsertExternalTransferProgress,
      completeExternalTransfer,
      failExternalTransfer,
    ],
  );

  return <TransferContext.Provider value={contextValue}>{children}</TransferContext.Provider>;
}

export function useTransfer(): TransferContextValue {
  const ctx = useContext(TransferContext);
  if (!ctx) throw new Error("useTransfer must be used within TransferProvider");
  return ctx;
}

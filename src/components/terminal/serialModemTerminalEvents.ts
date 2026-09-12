import { toast } from "sonner";

type SerialModemProtocol = "xmodem" | "ymodem";

export type SerialModemEventPayload =
  | {
      type: "progress";
      protocol: SerialModemProtocol;
      fileName: string;
      fileIndex: number;
      bytesTransferred: number;
      totalSize: number;
    }
  | {
      type: "fileComplete";
      protocol: SerialModemProtocol;
      fileIndex: number;
      fileName: string;
    }
  | { type: "complete"; protocol: SerialModemProtocol; fileCount: number }
  | {
      type: "failed";
      protocol: SerialModemProtocol;
      reason: string;
      fileIndex: number;
      fileName?: string;
    };

type Translate = (key: string, options?: Record<string, unknown>) => string;

export interface SerialModemTransferProgressSink {
  upsertProgress: (progress: {
    id: string;
    sessionId: string;
    fileName: string;
    direction: "upload";
    bytesTransferred: number;
    totalSize: number;
    source: "serial_modem";
  }) => void;
  complete: (id: string) => void;
  fail: (id: string, reason: string) => void;
}

export interface SerialModemEventHandler {
  handle(payload: SerialModemEventPayload): void;
  dispose(): void;
}

export function createSerialModemEventHandler(
  sessionId: string,
  getT: () => Translate,
  progressSink: SerialModemTransferProgressSink,
): SerialModemEventHandler {
  let batchSequence = 0;
  let batchActive = false;
  let lastCompletedId: string | null = null;
  const activeIds = new Map<number, string>();

  const transferId = (payload: Extract<SerialModemEventPayload, { type: "progress" }>) => {
    let id = activeIds.get(payload.fileIndex);
    if (id) return id;
    if (!batchActive) {
      batchSequence += 1;
      batchActive = true;
    }
    id = `serial-modem-${sessionId}-${batchSequence}-${payload.fileIndex}-${encodeURIComponent(payload.fileName)}`;
    activeIds.set(payload.fileIndex, id);
    return id;
  };

  return {
    handle(payload) {
      if (payload.type === "progress") {
        progressSink.upsertProgress({
          id: transferId(payload),
          sessionId,
          fileName: payload.fileName,
          direction: "upload",
          bytesTransferred: payload.bytesTransferred,
          totalSize: payload.totalSize,
          source: "serial_modem",
        });
        return;
      }

      if (payload.type === "fileComplete") {
        const id = activeIds.get(payload.fileIndex);
        if (id) {
          progressSink.complete(id);
          activeIds.delete(payload.fileIndex);
          lastCompletedId = id;
        }
        return;
      }

      if (payload.type === "complete") {
        for (const id of activeIds.values()) {
          progressSink.complete(id);
        }
        activeIds.clear();
        batchActive = false;
        lastCompletedId = null;
        return;
      }

      const failedId = activeIds.get(payload.fileIndex);
      if (failedId) {
        progressSink.fail(failedId, payload.reason);
      } else if (lastCompletedId) {
        progressSink.fail(lastCompletedId, payload.reason);
      }
      activeIds.clear();
      batchActive = false;
      lastCompletedId = null;
      toast.error(getT()("terminal.serialModemUploadFailed"), {
        description: payload.reason,
      });
    },
    dispose() {
      activeIds.clear();
      batchActive = false;
      lastCompletedId = null;
    },
  };
}

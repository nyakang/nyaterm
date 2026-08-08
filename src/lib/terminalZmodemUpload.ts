import { toast } from "sonner";
import { getLocalPathName } from "@/components/panel/file-explorer/model";
import i18n from "@/i18n";
import { invoke } from "@/lib/invoke";
import { buildTerminalCommandInput, sendSessionInput } from "@/lib/sessionInput";
import { showTransferDuplicatePrompt } from "@/lib/transferDuplicatePrompt";

const ZMODEM_UPLOAD_TIMEOUT_MS = 60_000;
const ZMODEM_ACTIVE_STALL_TIMEOUT_MS = 60_000;

type PendingUploadPhase = "waiting" | "active";
export type ZmodemUploadConflictMode = "overwrite" | "skip";

type PendingUpload = {
  sessionId: string;
  filePaths: string[];
  conflictMode: ZmodemUploadConflictMode;
  resolve: () => void;
  reject: (error: Error) => void;
  timeoutId: number | null;
  phase: PendingUploadPhase;
};

let pendingUpload: PendingUpload | null = null;
let preparingUploadToastId: string | number | null = null;

function clearWaitingTimeout() {
  if (pendingUpload?.timeoutId !== null && pendingUpload?.timeoutId !== undefined) {
    window.clearTimeout(pendingUpload.timeoutId);
    pendingUpload.timeoutId = null;
  }
}

function clearPendingUpload() {
  clearWaitingTimeout();
  pendingUpload = null;
  if (preparingUploadToastId !== null) {
    toast.dismiss(preparingUploadToastId);
    preparingUploadToastId = null;
  }
}

export function getPendingZmodemUploadPaths(sessionId: string): string[] | null {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return null;
  }
  return pendingUpload.filePaths;
}

export function getPendingZmodemUploadConflictMode(sessionId: string): ZmodemUploadConflictMode {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return "overwrite";
  }
  return pendingUpload.conflictMode;
}

export function hasPendingZmodemUpload(sessionId: string): boolean {
  return pendingUpload?.sessionId === sessionId;
}

function armActiveUploadTimeout(sessionId: string) {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return;
  }
  clearWaitingTimeout();
  pendingUpload.timeoutId = window.setTimeout(() => {
    if (
      !pendingUpload ||
      pendingUpload.sessionId !== sessionId ||
      pendingUpload.phase !== "active"
    ) {
      return;
    }
    const { reject } = pendingUpload;
    clearPendingUpload();
    void invoke("zmodem_cancel", { sessionId }).catch(() => {});
    reject(new Error("zmodem stalled"));
  }, ZMODEM_ACTIVE_STALL_TIMEOUT_MS);
}

export function markPendingZmodemUploadActive(sessionId: string) {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return;
  }
  clearWaitingTimeout();
  pendingUpload.phase = "active";
  armActiveUploadTimeout(sessionId);
  if (preparingUploadToastId !== null) {
    toast.dismiss(preparingUploadToastId);
    preparingUploadToastId = null;
  }
}

export function completePendingZmodemUpload(sessionId: string) {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return;
  }
  const { resolve } = pendingUpload;
  clearPendingUpload();
  resolve();
}

export function failPendingZmodemUpload(sessionId: string, reason: string) {
  if (!pendingUpload || pendingUpload.sessionId !== sessionId) {
    return;
  }
  const { reject } = pendingUpload;
  clearPendingUpload();
  reject(new Error(reason));
}

export function cancelPendingZmodemUpload(sessionId?: string) {
  if (!pendingUpload) return;
  if (sessionId && pendingUpload.sessionId !== sessionId) return;
  failPendingZmodemUpload(pendingUpload.sessionId, "cancelled");
}

const SFTP_PROBE_TIMEOUT_MS = 3_000;

function withTimeout<T>(promise: Promise<T>, ms: number, fallback: T): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((resolve) => setTimeout(() => resolve(fallback), ms)),
  ]);
}

function buildZmodemReceiveCommand(conflictMode: ZmodemUploadConflictMode): string {
  return conflictMode === "overwrite" ? "rz -y" : "rz";
}

export interface ConflictProbeResult {
  paths: string[];
  probeSkipped: boolean;
  conflictMode: ZmodemUploadConflictMode;
}

/**
 * Probe the remote working directory for files that match the local file
 * names. Resolves conflicts according to the configured duplicate strategy.
 *
 * - "overwrite" / "rename": delete remote files silently, include all local paths
 * - "skip": silently exclude conflicting local paths
 * - "ask" (default): prompt the user per conflict
 *
 * SFTP operations are time-boxed — if the SFTP channel is busy (e.g. the
 * file explorer panel is open), the probe is skipped, uploads fall back to
 * the selected ZMODEM conflict mode, and `probeSkipped` is set to true.
 */
export async function probeAndResolveRemoteConflicts(
  sessionId: string,
  filePaths: string[],
  duplicateStrategy = "ask",
): Promise<ConflictProbeResult> {
  if (filePaths.length === 0) {
    return { paths: [], probeSkipped: false, conflictMode: "overwrite" };
  }

  // Resolve remote upload directory (terminal CWD is a shell operation, not SFTP).
  const cwd = await withTimeout(
    invoke<string | null>("try_get_terminal_cwd", { sessionId }).catch(() => null),
    SFTP_PROBE_TIMEOUT_MS,
    null,
  );

  if (!cwd) {
    return resolveUnverifiedZmodemUpload(sessionId, filePaths, duplicateStrategy);
  }

  const remoteDir = cwd.replace(/\/+$/, "") || "/";

  // List remote directory (SFTP, time-boxed).
  const entries = await withTimeout(
    invoke<{ name: string; is_dir: boolean }[] | null>("list_remote_dir", {
      sessionId,
      path: remoteDir,
    }).catch(() => null),
    SFTP_PROBE_TIMEOUT_MS,
    null as { name: string; is_dir: boolean }[] | null,
  );

  // If the SFTP probe timed out, skip conflict detection entirely.
  if (entries === null) {
    return resolveUnverifiedZmodemUpload(sessionId, filePaths, duplicateStrategy);
  }

  const existingNames = new Set(entries.map((e) => e.name));

  // Separate conflicting and clean files.
  const conflicts: string[] = [];
  const clean: string[] = [];
  for (const fp of filePaths) {
    const name = getLocalPathName(fp, "file");
    if (existingNames.has(name)) {
      conflicts.push(fp);
    } else {
      clean.push(fp);
    }
  }

  if (conflicts.length === 0) {
    return { paths: filePaths, probeSkipped: false, conflictMode: "overwrite" };
  }

  // Auto-resolve based on configured strategy.
  if (duplicateStrategy === "skip") {
    return { paths: clean, probeSkipped: false, conflictMode: "overwrite" };
  }

  if (duplicateStrategy === "overwrite" || duplicateStrategy === "rename") {
    for (const fp of conflicts) {
      const name = getLocalPathName(fp, "file");
      const remotePath = remoteDir === "/" ? `/${name}` : `${remoteDir}/${name}`;
      await withTimeout(
        invoke("delete_remote_file", { sessionId, path: remotePath }).catch(() => {}),
        SFTP_PROBE_TIMEOUT_MS,
        undefined,
      );
    }
    return { paths: filePaths, probeSkipped: false, conflictMode: "overwrite" };
  }

  // "ask" — prompt the user for each conflict.
  const resolved: string[] = [...clean];

  for (const fp of conflicts) {
    const name = getLocalPathName(fp, "file");
    const remotePath = remoteDir === "/" ? `/${name}` : `${remoteDir}/${name}`;

    const choice = await showTransferDuplicatePrompt({
      requestId: crypto.randomUUID(),
      sessionId,
      remotePath,
      fileName: name,
      isDirectory: false,
      allowApplyToTask: conflicts.length > 1,
    });

    if (choice === "overwrite" || choice === "overwriteAllForTask") {
      await withTimeout(
        invoke("delete_remote_file", { sessionId, path: remotePath }).catch(() => {}),
        SFTP_PROBE_TIMEOUT_MS,
        undefined,
      );
      resolved.push(fp);
    }
    if (choice === "overwriteAllForTask") {
      const remaining = conflicts.slice(conflicts.indexOf(fp) + 1);
      for (const remainingPath of remaining) {
        const remainingName = getLocalPathName(remainingPath, "file");
        const remainingRemotePath =
          remoteDir === "/" ? `/${remainingName}` : `${remoteDir}/${remainingName}`;
        await withTimeout(
          invoke("delete_remote_file", { sessionId, path: remainingRemotePath }).catch(() => {}),
          SFTP_PROBE_TIMEOUT_MS,
          undefined,
        );
        resolved.push(remainingPath);
      }
      break;
    }
    // "skip" → don't add.
  }

  return { paths: resolved, probeSkipped: false, conflictMode: "overwrite" };
}

async function resolveUnverifiedZmodemUpload(
  sessionId: string,
  filePaths: string[],
  duplicateStrategy: string,
): Promise<ConflictProbeResult> {
  if (duplicateStrategy === "skip") {
    toast.message(i18n.t("zmodem.sftpProbeSkipped"));
    return { paths: [], probeSkipped: true, conflictMode: "skip" };
  }

  if (duplicateStrategy === "overwrite" || duplicateStrategy === "rename") {
    toast.message(i18n.t("zmodem.sftpProbeUnavailable"));
    return { paths: filePaths, probeSkipped: true, conflictMode: "overwrite" };
  }

  const fileName =
    filePaths.length === 1
      ? getLocalPathName(filePaths[0] ?? "", "file")
      : i18n.t("zmodem.multipleFiles", { count: filePaths.length });
  const choice = await showTransferDuplicatePrompt({
    requestId: crypto.randomUUID(),
    sessionId,
    remotePath: "",
    fileName,
    isDirectory: false,
    unverified: true,
  });

  if (choice === "overwrite" || choice === "overwriteAllForTask") {
    return { paths: filePaths, probeSkipped: true, conflictMode: "overwrite" };
  }

  return { paths: [], probeSkipped: true, conflictMode: "skip" };
}

export async function uploadFilesViaZmodem(
  sessionId: string,
  filePaths: string[],
  duplicateStrategy = "ask",
): Promise<void> {
  if (filePaths.length === 0) {
    return Promise.resolve();
  }

  // Cancel any previous pending upload.
  if (pendingUpload?.sessionId === sessionId && pendingUpload.phase === "waiting") {
    clearPendingUpload();
    await invoke("zmodem_cancel", { sessionId }).catch(() => {});
  } else {
    cancelPendingZmodemUpload(sessionId);
  }

  // Show immediate feedback so the user knows the drop was received,
  // even while the SFTP probe may be slow (e.g. file explorer panel open).
  if (preparingUploadToastId !== null) {
    toast.dismiss(preparingUploadToastId);
  }
  preparingUploadToastId = toast.message(
    i18n.t("zmodem.preparingUpload", { count: filePaths.length }),
  );

  // BEFORE starting remote rz, so the receiver never sees an existing file to
  // prompt about when the target directory can be verified. If the probe is
  // unavailable, fall back to rz flags plus the ZFILE conflict mode.
  const conflict = await probeAndResolveRemoteConflicts(sessionId, filePaths, duplicateStrategy);

  if (conflict.paths.length === 0) {
    if (preparingUploadToastId !== null) {
      toast.dismiss(preparingUploadToastId);
      preparingUploadToastId = null;
    }
    return;
  }

  return new Promise((resolve, reject) => {
    const timeoutId = window.setTimeout(() => {
      if (pendingUpload?.sessionId !== sessionId || pendingUpload.phase !== "waiting") {
        return;
      }
      clearPendingUpload();
      void invoke("zmodem_cancel", { sessionId }).catch(() => {});
      reject(new Error("zmodem timeout"));
    }, ZMODEM_UPLOAD_TIMEOUT_MS);

    pendingUpload = {
      sessionId,
      filePaths: conflict.paths,
      conflictMode: conflict.conflictMode,
      resolve,
      reject,
      timeoutId,
      phase: "waiting",
    };

    sendSessionInput(
      sessionId,
      buildTerminalCommandInput(buildZmodemReceiveCommand(conflict.conflictMode), true),
    ).catch((error) => {
      if (pendingUpload?.sessionId !== sessionId) return;
      clearPendingUpload();
      reject(error instanceof Error ? error : new Error(String(error)));
    });
  });
}

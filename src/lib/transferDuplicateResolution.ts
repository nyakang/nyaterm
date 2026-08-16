import { getRemoteParentDirectory, joinExplorerPath } from "@/components/panel/file-explorer/model";
import type { EnqueueUploadRequest } from "@/context/TransferContext";
import { invoke } from "@/lib/invoke";
import {
  showTransferDuplicatePrompt,
  type TransferDuplicatePromptChoice,
} from "@/lib/transferDuplicatePrompt";
import type { FileEntry } from "@/types/global";

async function remotePathExists(
  sessionId: string,
  path: string,
): Promise<{ exists: boolean; isDirectory: boolean }> {
  const parentDir = getRemoteParentDirectory(path);
  const fileName = path.split("/").filter(Boolean).pop() ?? "";
  if (!fileName) {
    return { exists: false, isDirectory: false };
  }

  try {
    const entries = await invoke<FileEntry[]>("list_remote_dir", {
      sessionId,
      path: parentDir,
    });
    const entry = entries.find((item) => item.name === fileName);
    if (entry) {
      return { exists: true, isDirectory: entry.is_dir };
    }
  } catch {
    // Fall through to "does not exist".
  }

  return { exists: false, isDirectory: false };
}

async function resolveDuplicateChoice(params: {
  sessionId: string;
  remotePath: string;
  fileName: string;
  isDirectory: boolean;
  duplicateStrategy: string;
  allowApplyToTask?: boolean;
}): Promise<TransferDuplicatePromptChoice | "proceed" | "skip"> {
  const { sessionId, remotePath, fileName, isDirectory, duplicateStrategy, allowApplyToTask } =
    params;

  switch (duplicateStrategy) {
    case "skip":
      return "skip";
    case "overwrite":
    case "rename":
      return "proceed";
    case "ask":
      return showTransferDuplicatePrompt({
        requestId: crypto.randomUUID(),
        sessionId,
        remotePath,
        fileName,
        isDirectory,
        allowApplyToTask,
      });
    default:
      return "proceed";
  }
}

async function resolveRemoteUploadConflict(params: {
  sessionId: string;
  remotePath: string;
  fileName: string;
  isDirectory: boolean;
  duplicateStrategy: string;
  allowApplyToTask?: boolean;
}): Promise<"include" | "skip" | "includeAndOverwriteRemaining"> {
  const { exists, isDirectory } = await remotePathExists(params.sessionId, params.remotePath);
  if (!exists) {
    return "include";
  }

  const choice = await resolveDuplicateChoice({
    sessionId: params.sessionId,
    remotePath: params.remotePath,
    fileName: params.fileName,
    isDirectory: exists ? isDirectory : params.isDirectory,
    duplicateStrategy: params.duplicateStrategy,
    allowApplyToTask: params.allowApplyToTask,
  });

  if (choice === "skip") {
    return "skip";
  }
  if (choice === "overwriteAllForTask") {
    return "includeAndOverwriteRemaining";
  }

  return "include";
}

export async function filterEnqueueUploadRequests(
  requests: EnqueueUploadRequest[],
  duplicateStrategy: string,
): Promise<EnqueueUploadRequest[]> {
  if (duplicateStrategy !== "ask" && duplicateStrategy !== "skip") {
    return requests;
  }

  const filtered: EnqueueUploadRequest[] = [];
  const allowApplyToTask = duplicateStrategy === "ask" && requests.length > 1;
  let overwriteRemainingForTask = false;

  for (const request of requests) {
    if (overwriteRemainingForTask) {
      filtered.push({ ...request, duplicateStrategyOverride: "overwrite" });
      continue;
    }

    const decision = await resolveRemoteUploadConflict({
      sessionId: request.sessionId,
      remotePath: request.remotePath,
      fileName: request.fileName,
      isDirectory: request.kind === "directory",
      duplicateStrategy,
      allowApplyToTask,
    });

    if (decision === "skip") {
      continue;
    }
    if (decision === "includeAndOverwriteRemaining") {
      overwriteRemainingForTask = true;
    }

    filtered.push(
      duplicateStrategy === "ask"
        ? { ...request, duplicateStrategyOverride: "overwrite" }
        : request,
    );
  }

  return filtered;
}

export interface ResolvedRemoteMove {
  oldPath: string;
  newPath: string;
}

export interface RemoteMoveTargetParams {
  sessionId: string;
  targetDir: string;
  entries: Array<{ name: string; path: string; isDirectory: boolean }>;
  duplicateStrategy: string;
}

/**
 * Resolve a set of cut→paste moves against the target directory, applying the
 * configured duplicate strategy (skip / overwrite / rename / ask). Returns the
 * moves that should actually be performed.
 */
export async function resolveRemoteMoveTargets(
  params: RemoteMoveTargetParams,
): Promise<ResolvedRemoteMove[]> {
  const { sessionId, targetDir, entries, duplicateStrategy } = params;
  if (entries.length === 0) {
    return [];
  }

  if (duplicateStrategy !== "ask" && duplicateStrategy !== "skip") {
    const moves: ResolvedRemoteMove[] = entries.map((entry) => ({
      oldPath: entry.path,
      newPath: joinExplorerPath(targetDir, entry.name, "remote"),
    }));
    if (duplicateStrategy === "rename") {
      const existingNames = await listRemoteDirNames(sessionId, targetDir);
      const used = new Set(existingNames);
      for (const move of moves) {
        if (used.has(move.newPath.split("/").filter(Boolean).pop() ?? "")) {
          const uniqueName = nextAvailableRemoteName(
            move.newPath.split("/").filter(Boolean).pop() ?? "",
            used,
          );
          used.add(uniqueName);
          move.newPath = joinExplorerPath(targetDir, uniqueName, "remote");
        }
      }
    }
    return moves;
  }

  const moves: ResolvedRemoteMove[] = [];
  const allowApplyToTask = duplicateStrategy === "ask" && entries.length > 1;
  let overwriteRemainingForTask = false;

  for (const entry of entries) {
    if (overwriteRemainingForTask) {
      moves.push({
        oldPath: entry.path,
        newPath: joinExplorerPath(targetDir, entry.name, "remote"),
      });
      continue;
    }

    const targetPath = joinExplorerPath(targetDir, entry.name, "remote");
    const { exists, isDirectory } = await remotePathExists(sessionId, targetPath);
    if (!exists) {
      moves.push({ oldPath: entry.path, newPath: targetPath });
      continue;
    }

    const choice = await resolveDuplicateChoice({
      sessionId,
      remotePath: targetPath,
      fileName: entry.name,
      isDirectory: exists ? isDirectory : entry.isDirectory,
      duplicateStrategy,
      allowApplyToTask,
    });

    if (choice === "skip") {
      continue;
    }
    if (choice === "overwriteAllForTask") {
      overwriteRemainingForTask = true;
    }
    moves.push({ oldPath: entry.path, newPath: targetPath });
  }

  return moves;
}

/**
 * Check which of the given remote entries no longer exist on the source
 * session. A failed `list_remote_dir` (e.g. transient network error) is treated
 * as "cannot verify" and the entry is NOT reported missing, to avoid blocking a
 * paste on a flaky connection. Returns entries whose parent directory was
 * listed successfully but that were not found in it.
 */
export async function findMissingRemoteEntries(
  sessionId: string,
  entries: Array<{ name: string; path: string }>,
): Promise<Array<{ name: string; path: string }>> {
  if (entries.length === 0) {
    return [];
  }

  const missing: Array<{ name: string; path: string }> = [];
  for (const entry of entries) {
    const parentDir = getRemoteParentDirectory(entry.path);
    try {
      const listed = await invoke<FileEntry[]>("list_remote_dir", {
        sessionId,
        path: parentDir,
      });
      if (!listed.some((item) => item.name === entry.name)) {
        missing.push(entry);
      }
    } catch {
      // Cannot verify existence: do not report as missing.
    }
  }

  return missing;
}

async function listRemoteDirNames(sessionId: string, dirPath: string): Promise<Set<string>> {
  try {
    const entries = await invoke<FileEntry[]>("list_remote_dir", {
      sessionId,
      path: dirPath,
    });
    return new Set(entries.map((entry) => entry.name));
  } catch {
    return new Set();
  }
}

function nextAvailableRemoteName(baseName: string, used: Set<string>): string {
  const dot = baseName.lastIndexOf(".");
  const stem = dot > 0 ? baseName.slice(0, dot) : baseName;
  const ext = dot > 0 ? baseName.slice(dot) : "";
  for (let i = 1; i <= 999; i++) {
    const candidate = `${stem} (${i})${ext}`;
    if (!used.has(candidate)) {
      return candidate;
    }
  }
  return `${stem} (${Date.now()})${ext}`;
}

/**
 * In-memory SFTP clipboard shared across file browser panes and windows.
 *
 * Copy/cut stores a lightweight reference (session + path list) without
 * touching the OS clipboard, so pasting behaves like `cp` (copy) or `mv`
 * (cut) on the same remote endpoint.
 */

import { readClipboardFilePaths } from "@/lib/clipboard";

export type SftpClipboardMode = "copy" | "cut";

export interface SftpClipboardEntry {
  name: string;
  path: string;
  isDirectory: boolean;
}

export interface SftpClipboardState {
  sessionId: string;
  mode: SftpClipboardMode;
  entries: SftpClipboardEntry[];
}

let currentState: SftpClipboardState | null = null;
const listeners = new Set<() => void>();
/** When the current SFTP clipboard entry was last copied/cut (for "last copy wins"). */
let sftpClipboardSetAt = 0;

export function getSftpClipboard(): SftpClipboardState | null {
  return currentState;
}

export function setSftpClipboard(state: SftpClipboardState | null): void {
  currentState = state;
  sftpClipboardSetAt = Date.now();
  for (const listener of listeners) {
    listener();
  }
}

export function clearSftpClipboard(): void {
  setSftpClipboard(null);
}

export function subscribeSftpClipboard(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Tracks whether the OS clipboard currently holds file paths (e.g. files
 * copied/cut in the system file manager), so paste actions can be disabled
 * when there is nothing to paste.
 *
 * Polling only runs while at least one subscriber is active and the document
 * has focus, to avoid contending with the system clipboard while the user is
 * copying in another application.
 */

export interface OsClipboardObservation {
  hasFiles: boolean;
  paths: string[];
  isNewerThanSftp: boolean;
}

const OS_CLIPBOARD_POLL_INTERVAL_MS = 1500;

let osClipboardHasFiles = false;
const osClipboardListeners = new Set<() => void>();
let osClipboardPollTimer: ReturnType<typeof setInterval> | null = null;
let osClipboardRefreshInFlight = false;
/** Key of the last observed OS clipboard file paths, used to detect new copies. */
let osPathsKey = "";
/** When the current OS clipboard file paths were first observed. */
let osPathsObservedAt = 0;

export function getOsClipboardHasFiles(): boolean {
  return osClipboardHasFiles;
}

export function subscribeOsClipboard(listener: () => void): () => void {
  osClipboardListeners.add(listener);
  if (osClipboardListeners.size === 1) {
    void refreshOsClipboardHasFiles();
    osClipboardPollTimer = setInterval(() => {
      if (document.hasFocus()) {
        void refreshOsClipboardHasFiles();
      }
    }, OS_CLIPBOARD_POLL_INTERVAL_MS);
    window.addEventListener("focus", handleOsClipboardWindowFocus);
  }
  return () => {
    osClipboardListeners.delete(listener);
    if (osClipboardListeners.size === 0) {
      if (osClipboardPollTimer) {
        clearInterval(osClipboardPollTimer);
        osClipboardPollTimer = null;
      }
      window.removeEventListener("focus", handleOsClipboardWindowFocus);
    }
  };
}

function handleOsClipboardWindowFocus(): void {
  void refreshOsClipboardHasFiles();
}

/**
 * Read the OS clipboard and record the observation so paste can decide which
 * clipboard is newer. Safe to call from the paste handler directly (always
 * performs a fresh read).
 */
export async function observeOsClipboard(): Promise<OsClipboardObservation> {
  let paths: string[] = [];
  try {
    paths = await readClipboardFilePaths();
  } catch {
    /* transient clipboard read failure: treat as empty */
    return { hasFiles: false, paths: [], isNewerThanSftp: false };
  }

  const key = buildOsPathsKey(paths);
  if (key !== osPathsKey) {
    osPathsKey = key;
    osPathsObservedAt = Date.now();
  }
  const hasFiles = paths.length > 0;
  if (hasFiles !== osClipboardHasFiles) {
    osClipboardHasFiles = hasFiles;
    for (const listener of osClipboardListeners) {
      listener();
    }
  }
  return {
    hasFiles,
    paths,
    isNewerThanSftp: hasFiles && osPathsObservedAt > sftpClipboardSetAt,
  };
}

function buildOsPathsKey(paths: string[]): string {
  return paths
    .map((path) => path.replace(/[\\/]+$/, ""))
    .filter(Boolean)
    .sort()
    .join("\n");
}

async function refreshOsClipboardHasFiles(): Promise<void> {
  if (osClipboardRefreshInFlight) return;
  osClipboardRefreshInFlight = true;
  try {
    await observeOsClipboard();
  } finally {
    osClipboardRefreshInFlight = false;
  }
}

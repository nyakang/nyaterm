import { emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  getCurrentWindow,
  type Window as TauriWindow,
  UserAttentionType,
} from "@tauri-apps/api/window";
import i18n from "../i18n";
import { invoke } from "./invoke";
import { isMacOS } from "./platform";

interface ChildWindowOptions {
  label: string;
  title: string;
  url: string;
  parentLabel?: string;
  width?: number;
  height?: number;
  resizable?: boolean;
}

const MAIN_WINDOW_LABEL = "main";
const MAIN_WINDOW_PREFIX = "main-";
const AUTO_UPLOAD_WINDOW_PREFIX = "auto-upload-";
const AUTO_UPLOAD_OWNER_SEPARATOR = "--";
const MODAL_CHILD_BASE_LABELS = new Set(["settings", "new-session", "quick-command"]);
const registeredDestroyedHandlers = new Set<string>();
let ownerMainWindowLabel = MAIN_WINDOW_LABEL;

export function isMainWindowLabel(label: string) {
  return label === MAIN_WINDOW_LABEL || label.startsWith(MAIN_WINDOW_PREFIX);
}

export function setOwnerMainWindowLabel(label: string) {
  if (isMainWindowLabel(label)) {
    ownerMainWindowLabel = label;
  }
}

export function getOwnerMainWindowLabel() {
  return ownerMainWindowLabel;
}

export function isPrimaryMainWindow() {
  return ownerMainWindowLabel === MAIN_WINDOW_LABEL;
}

function scopedModalLabel(baseLabel: string, ownerLabel = ownerMainWindowLabel) {
  return ownerLabel === MAIN_WINDOW_LABEL ? baseLabel : `${baseLabel}-${ownerLabel}`;
}

function ownerToken(ownerLabel = ownerMainWindowLabel) {
  return btoa(ownerLabel).replace(/[^a-zA-Z0-9]/g, "");
}

function modalOwnerLabel(label: string) {
  if (MODAL_CHILD_BASE_LABELS.has(label)) return MAIN_WINDOW_LABEL;
  for (const baseLabel of MODAL_CHILD_BASE_LABELS) {
    const prefix = `${baseLabel}-`;
    if (label.startsWith(prefix)) {
      return label.slice(prefix.length);
    }
  }
  return null;
}

function autoUploadOwnerLabel(label: string) {
  if (!label.startsWith(AUTO_UPLOAD_WINDOW_PREFIX)) return null;
  const rest = label.slice(AUTO_UPLOAD_WINDOW_PREFIX.length);
  const separatorIndex = rest.indexOf(AUTO_UPLOAD_OWNER_SEPARATOR);
  if (separatorIndex === -1) return null;
  const token = rest.slice(0, separatorIndex);
  try {
    return atob(token);
  } catch {
    return null;
  }
}

export function isModalChildLabel(label: string) {
  return modalOwnerLabel(label) !== null || label.startsWith(AUTO_UPLOAD_WINDOW_PREFIX);
}

export function isOwnedModalChildLabel(label: string, ownerLabel = ownerMainWindowLabel) {
  if (label.startsWith(AUTO_UPLOAD_WINDOW_PREFIX)) {
    return autoUploadOwnerLabel(label) === ownerLabel;
  }
  return modalOwnerLabel(label) === ownerLabel;
}

function needsAlwaysOnTop(label: string) {
  return label.startsWith(AUTO_UPLOAD_WINDOW_PREFIX);
}

async function getMainWindow() {
  return (await WebviewWindow.getByLabel(ownerMainWindowLabel)) ?? getCurrentWindow();
}

async function getOpenModalChildWindows() {
  const windows = await WebviewWindow.getAll();
  const modalWindows = windows.filter(
    (window) => window.label !== ownerMainWindowLabel && isOwnedModalChildLabel(window.label),
  );
  const visibleStates = await Promise.all(
    modalWindows.map((window) => window.isVisible().catch(() => false)),
  );
  return modalWindows.filter((_, index) => visibleStates[index]);
}

export async function getOpenModalChildWindowLabels() {
  const windows = await getOpenModalChildWindows();
  return windows.map((window) => window.label);
}

async function setMainWindowModalBlocking(mainWindow: TauriWindow, hasModalChild: boolean) {
  if (isMacOS) {
    // AppKit child windows inherit disabled/dimmed behavior from their parent window.
    await mainWindow.setEnabled(true).catch(() => {});
    await mainWindow.setFocusable(true).catch(() => {});
    return;
  }

  await mainWindow.setEnabled(!hasModalChild).catch(() => {});
  await mainWindow.setFocusable(!hasModalChild).catch(() => {});
}

async function applyModalWindowState(excludedLabel?: string) {
  const [mainWindow, modalWindows] = await Promise.all([
    getMainWindow(),
    getOpenModalChildWindows(),
  ]);
  const remainingModalWindows = excludedLabel
    ? modalWindows.filter((window) => window.label !== excludedLabel)
    : modalWindows;
  const hasModalChild = remainingModalWindows.length > 0;

  await setMainWindowModalBlocking(mainWindow, hasModalChild);

  if (hasModalChild) {
    const topModalWindow = remainingModalWindows[remainingModalWindows.length - 1];
    await topModalWindow.setAlwaysOnTop(needsAlwaysOnTop(topModalWindow.label)).catch(() => {});
    const isVisible = await topModalWindow.isVisible().catch(() => false);
    if (isVisible) {
      await topModalWindow.setFocus().catch(() => {});
    }
    return;
  }

  await mainWindow.show().catch(() => {});
  await mainWindow.setFocus().catch(() => {});
}

function attachChildWindowDestroyedHandler(label: string, win: WebviewWindow) {
  if (registeredDestroyedHandlers.has(label)) return;
  registeredDestroyedHandlers.add(label);

  win.once("tauri://destroyed", () => {
    registeredDestroyedHandlers.delete(label);
    emit("child-window-closed", { label });
    void prepareForModalChildClose(label);
  });
}

export async function syncMainWindowModalState() {
  await applyModalWindowState();
}

export async function prepareForModalChildClose(closingLabel: string) {
  await applyModalWindowState(closingLabel);
}

export async function bounceTopModalWindow() {
  const modalWindows = await getOpenModalChildWindows();
  const topModalWindow = modalWindows[modalWindows.length - 1];
  if (!topModalWindow) return;

  const isVisible = await topModalWindow.isVisible().catch(() => false);
  if (!isVisible) return;

  await topModalWindow.requestUserAttention(UserAttentionType.Critical).catch(() => {});
  await topModalWindow.setAlwaysOnTop(needsAlwaysOnTop(topModalWindow.label)).catch(() => {});
  await topModalWindow.setFocus().catch(() => {});
}

export async function openChildWindow(opts: ChildWindowOptions) {
  const existing = await WebviewWindow.getByLabel(opts.label);
  if (existing) {
    await existing.setTitle(opts.title).catch(() => {});
    await existing.setAlwaysOnTop(needsAlwaysOnTop(opts.label)).catch(() => {});
    await existing.show().catch(() => {});
    await existing.setFocus().catch(() => {});
    emit("child-window-opened", { label: opts.label });
    await syncMainWindowModalState().catch(() => {});
    return existing;
  }

  await invoke("open_child_window", {
    options: {
      label: opts.label,
      title: opts.title,
      url: opts.url,
      parentLabel: opts.parentLabel ?? ownerMainWindowLabel,
      width: opts.width ?? 720,
      height: opts.height ?? 560,
      resizable: opts.resizable ?? true,
      alwaysOnTop: needsAlwaysOnTop(opts.label),
    },
  });

  const win = await WebviewWindow.getByLabel(opts.label);
  if (!win) {
    throw new Error(`Failed to create child window: ${opts.label}`);
  }

  await win.setAlwaysOnTop(needsAlwaysOnTop(opts.label)).catch(() => {});
  attachChildWindowDestroyedHandler(opts.label, win);
  await win.show().catch(() => {});
  await win.setFocus().catch(() => {});
  emit("child-window-opened", { label: opts.label });
  await syncMainWindowModalState().catch(() => {});
  return win;
}

export async function openSettings(tab?: string) {
  const url = tab
    ? `index.html?window=settings&owner=${encodeURIComponent(ownerMainWindowLabel)}&tab=${encodeURIComponent(tab)}`
    : `index.html?window=settings&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  const win = await openChildWindow({
    label: scopedModalLabel("settings"),
    title: i18n.t("settings.title"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 800,
    height: 560,
  });
  if (tab) {
    const payload = { tab, targetWindowLabel: ownerMainWindowLabel };
    emit("settings-open-tab", payload);
    window.setTimeout(() => {
      void win.show().catch(() => {});
      void win.setFocus().catch(() => {});
      emit("settings-open-tab", payload);
    }, 120);
  }
  return win;
}

export interface NewSessionTarget {
  targetLeafId?: string;
  anchorTabId?: string | null;
  sourceTabId?: string;
  sourcePaneId?: string;
  initialGroupId?: string;
}

export function openNewSession(editId?: string, autoConnect?: boolean, target?: NewSessionTarget) {
  return openNewSessionWithTarget(editId, autoConnect, target);
}

export function openNewSessionWithTarget(
  editId?: string,
  autoConnect?: boolean,
  target?: NewSessionTarget,
) {
  let url = editId
    ? `index.html?window=new-session&owner=${encodeURIComponent(ownerMainWindowLabel)}&edit=${encodeURIComponent(editId)}`
    : `index.html?window=new-session&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  if (autoConnect) url += "&autoConnect=1";
  if (target?.targetLeafId) {
    url += `&targetLeafId=${encodeURIComponent(target.targetLeafId)}`;
  }
  if (target?.anchorTabId) {
    url += `&anchorTabId=${encodeURIComponent(target.anchorTabId)}`;
  }
  if (target?.sourceTabId) {
    url += `&sourceTabId=${encodeURIComponent(target.sourceTabId)}`;
  }
  if (target?.sourcePaneId) {
    url += `&sourcePaneId=${encodeURIComponent(target.sourcePaneId)}`;
  }
  if (!editId && target?.initialGroupId) {
    url += `&groupId=${encodeURIComponent(target.initialGroupId)}`;
  }
  return openChildWindow({
    label: scopedModalLabel("new-session"),
    title: i18n.t(editId ? "dialog.editConnection" : "dialog.newConnection"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 520,
    height: 620,
  });
}

export function openQuickCommand(editJson?: string) {
  const url = editJson
    ? `index.html?window=quick-command&owner=${encodeURIComponent(ownerMainWindowLabel)}&data=${encodeURIComponent(editJson)}`
    : `index.html?window=quick-command&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  return openChildWindow({
    label: scopedModalLabel("quick-command"),
    title: i18n.t(editJson ? "quickCommands.editCommand" : "quickCommands.addCommand"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 540,
    height: 640,
  });
}

export function openAutoUpload(data: { sessionId: string; localPath: string; remotePath: string }) {
  // Use a unique label for each upload dialog so multiple files modifying simultaneously don't conflict
  // We use the local path base64 (or just random) to make it unique per file
  const safePath = btoa(encodeURIComponent(data.localPath)).replace(/[^a-zA-Z0-9]/g, "");
  const label = `auto-upload-${ownerToken()}${AUTO_UPLOAD_OWNER_SEPARATOR}${safePath}`;
  const url = `index.html?window=auto-upload&owner=${encodeURIComponent(ownerMainWindowLabel)}&data=${encodeURIComponent(JSON.stringify(data))}`;
  return openChildWindow({
    label,
    title: i18n.t("fileExplorer.fileModified"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 440,
    height: 240,
    resizable: false,
  });
}

import { emit, listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  availableMonitors,
  getCurrentWindow,
  PhysicalPosition,
  primaryMonitor,
  type Window as TauriWindow,
  UserAttentionType,
} from "@tauri-apps/api/window";
import i18n from "../i18n";
import { ChildWindowCommandQueue } from "./childWindowCommandQueue";
import {
  CHILD_WINDOW_COMMANDS,
  CHILD_WINDOW_LIFECYCLE_EVENT,
  CHILD_WINDOW_READY_TOKEN_PARAM,
  type ChildWindowCommandName,
  type ChildWindowLifecyclePayload,
} from "./childWindowProtocol";
import { invoke } from "./invoke";
import { logger } from "./logger";
import { isMacOS } from "./platform";

type ChildWindowStateKey =
  | "settings"
  | "new-session"
  | "quick-command"
  | "proxy"
  | "tunnel"
  | "file-editor"
  | "file-preview"
  | "note-editor";

interface ChildWindowOptions {
  label: string;
  title: string;
  url: string;
  kind?: "modal" | "modeless";
  parentLabel?: string;
  width?: number;
  height?: number;
  resizable?: boolean;
  stateKey?: ChildWindowStateKey;
}

const MAIN_WINDOW_LABEL = "main";
const MAIN_WINDOW_PREFIX = "main-";
const AUTO_UPLOAD_WINDOW_PREFIX = "auto-upload-";
const FILE_EDITOR_WINDOW_PREFIX = "file-editor-";
const FILE_PREVIEW_WINDOW_PREFIX = "file-preview-";
const NOTE_EDITOR_WINDOW_PREFIX = "note-editor-";
const AUTO_UPLOAD_OWNER_SEPARATOR = "--";
const MODAL_CHILD_BASE_LABELS = new Set([
  "settings",
  "new-session",
  "quick-command",
  "proxy",
  "tunnel",
]);
const MODAL_GROUP_RAISE_SUPPRESS_MS = 250;
const MODAL_TOPMOST_PULSE_MS = 120;
const CHILD_WINDOW_READY_TIMEOUT_MS = 5_000;
const INIT_URL_ONLY_WINDOW_TYPES = new Set(["new-session", "quick-command"]);
const registeredDestroyedHandlers = new Map<string, string>();
const pendingChildWindowOpens = new Map<string, PendingChildWindowOpen>();
const childWindowCommands = new ChildWindowCommandQueue();
const childWindowTokens = new Map<string, string>();
const childWindowShellWaiters = new Map<string, ChildWindowLifecycleWaiter>();
const failedChildWindowClosures = new Map<string, string>();
let childWindowLifecycleListenerPromise: Promise<void> | undefined;
let ownerMainWindowLabel = MAIN_WINDOW_LABEL;
let modalGroupRaiseInFlight = false;
let suppressChildFocusSyncUntil = 0;
let modalTopmostPulseId = 0;

type ModalGroupRaiseReason = "open" | "main-focus" | "child-focus" | "backdrop" | "close";

interface ModalGroupRaiseOptions {
  focusLabel?: string;
  excludedLabel?: string;
  requestAttention?: boolean;
  reason?: ModalGroupRaiseReason;
}

interface ChildWindowLifecycleWaiter {
  token: string;
  promise: Promise<void>;
  resolve: () => void;
  cancel: () => void;
  fail: () => void;
  failed: () => boolean;
}

interface PendingChildWindowOpen {
  url: string;
  promise: Promise<WebviewWindow>;
}

interface WorkAreaLike {
  position: { x: number; y: number };
  size: { width: number; height: number };
}

interface WindowRectLike {
  x: number;
  y: number;
  width: number;
  height: number;
}

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

function childWindowKind(opts: ChildWindowOptions) {
  return opts.kind ?? "modal";
}

function childWindowTypeFromUrl(url: string) {
  try {
    const query = url.includes("?") ? url.slice(url.indexOf("?") + 1) : "";
    return new URLSearchParams(query).get("window") ?? undefined;
  } catch {
    return undefined;
  }
}

export function childWindowCommandForUrl(url: string): ChildWindowCommandName | undefined {
  switch (childWindowTypeFromUrl(url)) {
    case "settings":
      return CHILD_WINDOW_COMMANDS.settingsOpenTab;
    case "file-editor":
      return CHILD_WINDOW_COMMANDS.remoteFileEditorOpen;
    case "file-preview":
      return CHILD_WINDOW_COMMANDS.filePreviewOpen;
    default:
      return undefined;
  }
}

function appendChildWindowReadyToken(url: string, token: string) {
  const separator = url.includes("?") ? "&" : "?";
  return `${url}${separator}${CHILD_WINDOW_READY_TOKEN_PARAM}=${encodeURIComponent(token)}`;
}

function createChildWindowReadyToken() {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function shouldWarnPendingOpenConflict(existingUrl: string, requestedUrl: string) {
  const existingWindowType = childWindowTypeFromUrl(existingUrl);
  const requestedWindowType = childWindowTypeFromUrl(requestedUrl);
  return {
    existingWindowType,
    requestedWindowType,
    shouldWarn:
      existingWindowType === requestedWindowType &&
      existingWindowType !== undefined &&
      INIT_URL_ONLY_WINDOW_TYPES.has(existingWindowType),
  };
}

async function getMainWindow() {
  return (await WebviewWindow.getByLabel(ownerMainWindowLabel)) ?? getCurrentWindow();
}

export function rectOverlapsWorkArea(rect: WindowRectLike, workArea: WorkAreaLike) {
  const windowRight = rect.x + rect.width;
  const windowBottom = rect.y + rect.height;
  const areaRight = workArea.position.x + workArea.size.width;
  const areaBottom = workArea.position.y + workArea.size.height;

  return (
    rect.x < areaRight &&
    windowRight > workArea.position.x &&
    rect.y < areaBottom &&
    windowBottom > workArea.position.y
  );
}

export function centerWindowRectInWorkArea(
  size: Pick<WindowRectLike, "width" | "height">,
  workArea: WorkAreaLike,
) {
  return {
    x: workArea.position.x + Math.max(0, Math.round((workArea.size.width - size.width) / 2)),
    y: workArea.position.y + Math.max(0, Math.round((workArea.size.height - size.height) / 2)),
  };
}

function findMonitorForRect<T extends { workArea: WorkAreaLike }>(
  rect: WindowRectLike,
  monitors: T[],
) {
  return monitors.find((monitor) => rectOverlapsWorkArea(rect, monitor.workArea)) ?? null;
}

async function getWindowRect(win: TauriWindow): Promise<WindowRectLike> {
  const [position, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
  return {
    x: position.x,
    y: position.y,
    width: size.width,
    height: size.height,
  };
}

async function ensureChildWindowVisible(win: WebviewWindow, opts: ChildWindowOptions) {
  try {
    const [childRect, monitors] = await Promise.all([getWindowRect(win), availableMonitors()]);
    if (monitors.length === 0) return;
    if (findMonitorForRect(childRect, monitors)) return;

    const parentWindow = await getMainWindow();
    const parentRect = await getWindowRect(parentWindow).catch(() => null);
    const parentMonitor = parentRect ? findMonitorForRect(parentRect, monitors) : null;
    const fallbackMonitor = await primaryMonitor().catch(() => null);
    const targetMonitor = parentMonitor ?? fallbackMonitor ?? monitors[0];
    const nextPosition = centerWindowRectInWorkArea(childRect, targetMonitor.workArea);

    await win.setPosition(new PhysicalPosition(nextPosition.x, nextPosition.y));
    logger.warn({
      domain: "window.lifecycle",
      event: "window.child.repositioned_from_disconnected_monitor",
      message: "Repositioned child window from disconnected monitor",
      data: {
        label: opts.label,
        from_x: childRect.x,
        from_y: childRect.y,
        width: childRect.width,
        height: childRect.height,
        to_x: nextPosition.x,
        to_y: nextPosition.y,
      },
    });
  } catch (error) {
    logger.warn({
      domain: "window.lifecycle",
      event: "child_window_visibility_check_failed",
      message: "Failed to verify child window visibility",
      data: { label: opts.label },
      error,
    });
  }
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
    await raiseModalChildWindowGroup({
      excludedLabel,
      reason: excludedLabel ? "close" : "open",
    });
    return;
  }

  await mainWindow.show().catch(() => {});
  await mainWindow.setFocus().catch(() => {});
}

function orderedModalWindowsForFocus(windows: WebviewWindow[], focusLabel?: string) {
  const focusWindow = focusLabel
    ? windows.find((window) => window.label === focusLabel)
    : undefined;
  const topModalWindow = focusWindow ?? windows[windows.length - 1];
  if (!topModalWindow) return { orderedWindows: windows, topModalWindow: undefined };

  return {
    orderedWindows: windows
      .filter((window) => window.label !== topModalWindow.label)
      .concat(topModalWindow),
    topModalWindow,
  };
}

function restoreModalTopmostStates(windows: WebviewWindow[], pulseId: number) {
  window.setTimeout(() => {
    if (pulseId !== modalTopmostPulseId) return;
    windows.forEach((modalWindow) => {
      void modalWindow.setAlwaysOnTop(needsAlwaysOnTop(modalWindow.label)).catch(() => {});
    });
  }, MODAL_TOPMOST_PULSE_MS);
}

export function shouldSuppressModalChildFocusSync() {
  return Date.now() < suppressChildFocusSyncUntil;
}

export async function raiseModalChildWindowGroup(options: ModalGroupRaiseOptions = {}) {
  if (modalGroupRaiseInFlight) return;
  if (options.reason === "child-focus" && shouldSuppressModalChildFocusSync()) return;

  modalGroupRaiseInFlight = true;
  suppressChildFocusSyncUntil = Date.now() + MODAL_GROUP_RAISE_SUPPRESS_MS;

  try {
    const modalWindows = (await getOpenModalChildWindows()).filter(
      (modalWindow) => modalWindow.label !== options.excludedLabel,
    );
    const { orderedWindows, topModalWindow } = orderedModalWindowsForFocus(
      modalWindows,
      options.focusLabel,
    );
    if (!topModalWindow) return;

    modalTopmostPulseId += 1;
    const pulseId = modalTopmostPulseId;

    await Promise.all(
      orderedWindows.map(async (modalWindow) => {
        await modalWindow.show().catch(() => {});
        await modalWindow.setAlwaysOnTop(true).catch(() => {});
      }),
    );

    for (const modalWindow of orderedWindows) {
      await modalWindow.setFocus().catch(() => {});
    }

    if (options.requestAttention) {
      await topModalWindow.requestUserAttention(UserAttentionType.Critical).catch(() => {});
    }

    restoreModalTopmostStates(orderedWindows, pulseId);
  } finally {
    window.setTimeout(() => {
      modalGroupRaiseInFlight = false;
      suppressChildFocusSyncUntil = Math.max(suppressChildFocusSyncUntil, 0);
    }, MODAL_GROUP_RAISE_SUPPRESS_MS);
  }
}

async function attachChildWindowDestroyedHandler(label: string, win: WebviewWindow) {
  const lifecycleToken = childWindowTokens.get(label);
  // 回调绑定注册时的窗口代际；旧实例迟到的 destroyed 不得清理同 label 新实例。
  const registrationId =
    lifecycleToken ?? registeredDestroyedHandlers.get(label) ?? createChildWindowReadyToken();
  if (registeredDestroyedHandlers.get(label) === registrationId) return;
  registeredDestroyedHandlers.set(label, registrationId);

  try {
    await win.once("tauri://destroyed", () => {
      if (registeredDestroyedHandlers.get(label) !== registrationId) return;
      const currentToken = childWindowTokens.get(label);
      if (currentToken && currentToken !== lifecycleToken) return;

      registeredDestroyedHandlers.delete(label);
      clearChildWindowLifecycle(label, lifecycleToken, true);
      void emit("child-window-closed", { label }).catch(() => {});
      if (isModalChildLabel(label)) {
        void prepareForModalChildClose(label).catch(() => {});
      }
    });
  } catch (error) {
    if (registeredDestroyedHandlers.get(label) === registrationId) {
      registeredDestroyedHandlers.delete(label);
    }
    throw error;
  }
}

export async function syncMainWindowModalState() {
  await applyModalWindowState();
}

export async function prepareForModalChildClose(closingLabel: string) {
  await applyModalWindowState(closingLabel);
}

export async function bounceTopModalWindow() {
  await raiseModalChildWindowGroup({ requestAttention: true, reason: "backdrop" });
}

function emitChildWindowCommands(commands: ReturnType<ChildWindowCommandQueue["dispatch"]>) {
  for (const command of commands) {
    void emit(command.event, command.payload).catch((error) => {
      logger.warn({
        domain: "window.lifecycle",
        event: "child_command_emit_failed",
        message: "Failed to emit a command to a child window",
        data: { command: command.event },
        error,
      });
    });
  }
}

function dispatchChildWindowCommand(
  label: string,
  event: ChildWindowCommandName,
  payload: unknown,
) {
  emitChildWindowCommands(childWindowCommands.dispatch(label, event, payload));
}

async function closeFailedChildWindow(label: string, token: string) {
  failedChildWindowClosures.set(label, token);
  try {
    const win = await WebviewWindow.getByLabel(label).catch(() => null);
    await win?.close().catch((error) => {
      logger.warn({
        domain: "window.lifecycle",
        event: "child_failed_window_close_failed",
        message: "Failed to close a child window after load failure",
        data: { label },
        error,
      });
    });
  } finally {
    if (failedChildWindowClosures.get(label) === token) {
      failedChildWindowClosures.delete(label);
    }
    clearChildWindowLifecycle(label, token, true);
  }
}

function handleChildWindowLifecycle(payload: ChildWindowLifecyclePayload) {
  if (!payload.token || childWindowTokens.get(payload.label) !== payload.token) return;

  if (payload.phase === "load-started") {
    childWindowCommands.markLoading(payload.label, payload.token);
    return;
  }

  switch (payload.phase) {
    case "shell-ready": {
      const waiter = childWindowShellWaiters.get(payload.label);
      if (waiter?.token === payload.token) waiter.resolve();
      break;
    }
    case "command-ready":
      emitChildWindowCommands(
        childWindowCommands.markReady(payload.label, payload.token, payload.command),
      );
      break;
    case "load-failed":
      childWindowCommands.markFailed(payload.label, payload.token);
      {
        const waiter = childWindowShellWaiters.get(payload.label);
        if (waiter?.token === payload.token) waiter.fail();
      }
      logger.warn({
        domain: "window.lifecycle",
        event: "child_load_failed",
        message: "Child window failed to finish loading",
        data: { label: payload.label, stage: payload.stage },
      });
      void closeFailedChildWindow(payload.label, payload.token);
      break;
  }
}

async function ensureChildWindowLifecycleListener() {
  if (!childWindowLifecycleListenerPromise) {
    childWindowLifecycleListenerPromise = listen<ChildWindowLifecyclePayload>(
      CHILD_WINDOW_LIFECYCLE_EVENT,
      ({ payload }) => handleChildWindowLifecycle(payload),
    )
      .then(() => undefined)
      .catch((error) => {
        childWindowLifecycleListenerPromise = undefined;
        logger.warn({
          domain: "window.lifecycle",
          event: "child_lifecycle_listener_failed",
          message: "Failed to listen for child window lifecycle events",
          error,
        });
        throw error;
      });
  }
  await childWindowLifecycleListenerPromise;
}

function clearChildWindowLifecycle(label: string, token?: string, failWaiter = false) {
  if (token && childWindowTokens.get(label) !== token) return;

  childWindowTokens.delete(label);
  childWindowCommands.clear(label);
  const waiter = childWindowShellWaiters.get(label);
  if (failWaiter) waiter?.fail();
  else waiter?.cancel();
}

async function createChildWindowLifecycleWaiter(
  label: string,
  token: string,
  expectedCommand: ChildWindowCommandName | undefined,
): Promise<ChildWindowLifecycleWaiter> {
  await ensureChildWindowLifecycleListener();

  childWindowShellWaiters.get(label)?.cancel();
  childWindowTokens.set(label, token);
  if (expectedCommand) {
    childWindowCommands.register(label, token, expectedCommand);
  }

  let settled = false;
  let timeoutId: number | undefined;
  let failed = false;
  let resolveReady: () => void = () => {};
  const promise = new Promise<void>((resolve) => {
    resolveReady = resolve;
  });

  const settle = (didFail: boolean) => {
    if (settled) return;
    settled = true;
    failed = didFail;
    if (timeoutId !== undefined) {
      window.clearTimeout(timeoutId);
    }
    if (childWindowShellWaiters.get(label)?.token === token) {
      childWindowShellWaiters.delete(label);
    }
    resolveReady();
  };

  const waiter: ChildWindowLifecycleWaiter = {
    token,
    promise,
    resolve: () => settle(false),
    cancel: () => settle(false),
    fail: () => settle(true),
    failed: () => failed,
  };
  childWindowShellWaiters.set(label, waiter);

  timeoutId = window.setTimeout(() => {
    logger.warn({
      domain: "window.lifecycle",
      event: "child_ready_timeout",
      message: "Child window did not signal shell ready before timeout",
      data: { label },
    });
    settle(true);
  }, CHILD_WINDOW_READY_TIMEOUT_MS);

  return waiter;
}

async function revealChildWindow(
  win: WebviewWindow,
  opts: ChildWindowOptions,
  isModal: boolean,
  isNewWindow = false,
  onShown?: () => void,
) {
  // The Rust builder already sets the title, always-on-top state, and position for a new window.
  // Repeating those IPC calls would delay show(), especially during the first macOS open.
  if (!isNewWindow) {
    await win.setTitle(opts.title).catch(() => {});
    await win.setAlwaysOnTop(needsAlwaysOnTop(opts.label)).catch(() => {});
  }
  await attachChildWindowDestroyedHandler(opts.label, win);
  if (!isNewWindow) {
    await ensureChildWindowVisible(win, opts);
  }
  // Keep the child hidden until the ready handshake, then restore interactivity before showing.
  await win.setFocusable(true).catch(() => {});
  await win.show();
  onShown?.();
  await win.setFocus().catch(() => {});
  emit("child-window-opened", { label: opts.label });
  if (isModal) {
    await syncMainWindowModalState().catch(() => {});
  }
  return win;
}

async function openChildWindowInternal(opts: ChildWindowOptions) {
  const startedAt = performance.now();
  const logTiming = (data: Record<string, unknown>) => {
    logger.info({
      domain: "window.lifecycle",
      event: "child_window_open_timing",
      message: "Child window open timing",
      data: {
        label: opts.label,
        total_ms: Math.round(performance.now() - startedAt),
        ...data,
      },
    });
  };
  const kind = childWindowKind(opts);
  const isModal = kind === "modal";
  const existing = await WebviewWindow.getByLabel(opts.label);
  if (existing) {
    const existingToken = childWindowTokens.get(opts.label);
    const existingFailed =
      existingToken !== undefined &&
      (childWindowCommands.isFailed(opts.label, existingToken) ||
        failedChildWindowClosures.get(opts.label) === existingToken);
    if (existingFailed) {
      await closeFailedChildWindow(opts.label, existingToken);
    } else {
      let shownMs: number | undefined;
      const revealed = await revealChildWindow(existing, opts, isModal, false, () => {
        shownMs = Math.round(performance.now() - startedAt);
      });
      logTiming({ existing: true, shown_ms: shownMs });
      return revealed;
    }
  }

  const readyToken = createChildWindowReadyToken();
  const lifecycleWaiter = await createChildWindowLifecycleWaiter(
    opts.label,
    readyToken,
    childWindowCommandForUrl(opts.url),
  );
  const listenerReadyMs = Math.round(performance.now() - startedAt);
  try {
    await invoke("open_child_window", {
      options: {
        label: opts.label,
        title: opts.title,
        url: appendChildWindowReadyToken(opts.url, readyToken),
        kind,
        parentLabel: opts.parentLabel ?? ownerMainWindowLabel,
        width: opts.width ?? 720,
        height: opts.height ?? 560,
        resizable: opts.resizable ?? true,
        alwaysOnTop: needsAlwaysOnTop(opts.label),
        stateKey: opts.stateKey,
      },
    });
    const invokeMs = Math.round(performance.now() - startedAt);

    const win = await WebviewWindow.getByLabel(opts.label);
    if (!win) {
      throw new Error(`Failed to create child window: ${opts.label}`);
    }
    const handleMs = Math.round(performance.now() - startedAt);

    const destroyedListenerPromise = attachChildWindowDestroyedHandler(opts.label, win);
    await Promise.all([lifecycleWaiter.promise, destroyedListenerPromise]);
    if (lifecycleWaiter.failed()) {
      throw new Error(`Child window did not finish rendering: ${opts.label}`);
    }
    const readyMs = Math.round(performance.now() - startedAt);
    let shownMs: number | undefined;
    const revealed = await revealChildWindow(win, opts, isModal, true, () => {
      shownMs = Math.round(performance.now() - startedAt);
    });
    logTiming({
      existing: false,
      listener_ready_ms: listenerReadyMs,
      invoke_ms: invokeMs,
      handle_ms: handleMs,
      ready_ms: readyMs,
      shown_ms: shownMs,
    });
    return revealed;
  } catch (error) {
    lifecycleWaiter.cancel();
    clearChildWindowLifecycle(opts.label, readyToken);
    // Destroy a failed first-open window promptly so it cannot remain as a background orphan.
    const orphan = await WebviewWindow.getByLabel(opts.label).catch(() => null);
    await orphan?.close().catch(() => {});
    throw error;
  }
}

export function openChildWindow(opts: ChildWindowOptions): Promise<WebviewWindow> {
  const pending = pendingChildWindowOpens.get(opts.label);
  if (pending) {
    if (pending.url !== opts.url) {
      const { existingWindowType, requestedWindowType, shouldWarn } = shouldWarnPendingOpenConflict(
        pending.url,
        opts.url,
      );
      if (shouldWarn) {
        logger.warn({
          domain: "window.lifecycle",
          event: "child_window_open_conflict",
          message: "Ignored child window open request while the same label is already opening",
          data: {
            label: opts.label,
            existingWindowType,
            requestedWindowType,
          },
        });
      }
    }
    return pending.promise;
  }

  const operation = openChildWindowInternal(opts);
  pendingChildWindowOpens.set(opts.label, { url: opts.url, promise: operation });
  const clearPending = () => {
    if (pendingChildWindowOpens.get(opts.label)?.promise === operation) {
      pendingChildWindowOpens.delete(opts.label);
    }
  };
  operation.then(clearPending, clearPending);
  return operation;
}

export async function openSettings(tab?: string) {
  const label = scopedModalLabel("settings");
  const url = tab
    ? `index.html?window=settings&owner=${encodeURIComponent(ownerMainWindowLabel)}&tab=${encodeURIComponent(tab)}`
    : `index.html?window=settings&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  const win = await openChildWindow({
    label,
    title: i18n.t("settings.title"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 800,
    height: 560,
    stateKey: "settings",
  });
  if (tab) {
    const payload = { tab, targetWindowLabel: ownerMainWindowLabel };
    dispatchChildWindowCommand(label, CHILD_WINDOW_COMMANDS.settingsOpenTab, payload);
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
    stateKey: "new-session",
  });
}

export function openQuickCommand(editJson?: string, options?: { categoryId?: string | null }) {
  const params = new URLSearchParams({
    window: "quick-command",
    owner: ownerMainWindowLabel,
  });
  if (editJson) {
    params.set("data", editJson);
  } else if (options?.categoryId) {
    params.set("category_id", options.categoryId);
  }
  let isEdit = false;
  if (editJson) {
    try {
      isEdit = !!JSON.parse(editJson).id;
    } catch {
      isEdit = false;
    }
  }
  const url = `index.html?${params.toString()}`;
  return openChildWindow({
    label: scopedModalLabel("quick-command"),
    title: i18n.t(isEdit ? "quickCommands.editCommand" : "quickCommands.addCommand"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 540,
    height: 640,
    stateKey: "quick-command",
  });
}

export function openProxyConfig(editId?: string) {
  const url = editId
    ? `index.html?window=proxy&owner=${encodeURIComponent(ownerMainWindowLabel)}&edit=${encodeURIComponent(editId)}`
    : `index.html?window=proxy&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  return openChildWindow({
    label: scopedModalLabel("proxy"),
    title: i18n.t(editId ? "network.editProxy" : "network.newProxy"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 520,
    height: 560,
    stateKey: "proxy",
  });
}

export function openTunnelConfig(editId?: string) {
  const url = editId
    ? `index.html?window=tunnel&owner=${encodeURIComponent(ownerMainWindowLabel)}&edit=${encodeURIComponent(editId)}`
    : `index.html?window=tunnel&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  return openChildWindow({
    label: scopedModalLabel("tunnel"),
    title: i18n.t(editId ? "network.editTunnel" : "network.newTunnel"),
    url,
    parentLabel: ownerMainWindowLabel,
    width: 680,
    height: 640,
    stateKey: "tunnel",
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

export interface FileWindowTarget {
  kind: "remote" | "local";
  label: string;
  detail?: string;
}

export interface RemoteFileEditorWindowData {
  sessionId: string;
  backend?: "remote" | "local";
  path?: string;
  remotePath?: string;
  name: string;
  size: number;
  mtime: number;
  target?: FileWindowTarget;
}

export function openRemoteFileEditor(data: RemoteFileEditorWindowData) {
  const label = `${FILE_EDITOR_WINDOW_PREFIX}${ownerToken()}`;
  const url = `index.html?window=file-editor&owner=${encodeURIComponent(ownerMainWindowLabel)}&data=${encodeURIComponent(JSON.stringify(data))}`;
  return openChildWindow({
    label,
    title: i18n.t("fileEditor.title"),
    url,
    kind: "modeless",
    parentLabel: ownerMainWindowLabel,
    width: 980,
    height: 720,
    stateKey: "file-editor",
  }).then((win) => {
    const payload = { targetLabel: label, data };
    dispatchChildWindowCommand(label, CHILD_WINDOW_COMMANDS.remoteFileEditorOpen, payload);
    return win;
  });
}

export interface FilePreviewWindowData {
  sessionId: string;
  backend?: "remote" | "local";
  path: string;
  name: string;
  size: number;
  mtime: number;
  target?: FileWindowTarget;
}

export function openFilePreview(data: FilePreviewWindowData) {
  const label = `${FILE_PREVIEW_WINDOW_PREFIX}${ownerToken()}`;
  const url = `index.html?window=file-preview&owner=${encodeURIComponent(ownerMainWindowLabel)}&data=${encodeURIComponent(JSON.stringify(data))}`;
  return openChildWindow({
    label,
    title: i18n.t("filePreview.title"),
    url,
    kind: "modeless",
    parentLabel: ownerMainWindowLabel,
    width: 1080,
    height: 760,
    stateKey: "file-preview",
  }).then((win) => {
    const payload = { targetLabel: label, data };
    dispatchChildWindowCommand(label, CHILD_WINDOW_COMMANDS.filePreviewOpen, payload);
    return win;
  });
}

export function openNoteEditor(noteId: string, noteTitle: string) {
  const label = `${NOTE_EDITOR_WINDOW_PREFIX}${ownerToken()}-${noteId}`;
  const title = `${noteTitle || i18n.t("notes.untitled")} - ${i18n.t("notes.title")} - NyaTerm`;
  const url = `index.html?window=note-editor&noteId=${encodeURIComponent(noteId)}&owner=${encodeURIComponent(ownerMainWindowLabel)}`;
  return openChildWindow({
    label,
    title,
    url,
    kind: "modeless",
    parentLabel: ownerMainWindowLabel,
    width: 980,
    height: 760,
    resizable: true,
    stateKey: "note-editor",
  });
}

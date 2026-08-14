import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { downloadDir } from "@tauri-apps/api/path";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import AppLayout from "./components/app/AppLayout";
import AppPanelContent, {
  type OneClickRuntime,
} from "./components/app/AppPanelContent";
import ExternalConnectionMatchDialog from "./components/dialog/connections/ExternalConnectionMatchDialog";
import type { HostKeyVerifyRequest } from "./components/dialog/connections/HostKeyVerifyDialog";
import type { OtpRequest } from "./components/dialog/connections/OtpDialog";
import type { RdpCertificateVerifyRequest } from "./components/dialog/connections/RdpCertificateVerifyDialog";
import type { SshAgentAuthRequest } from "./components/dialog/connections/SshAgentAuthDialog";
import type { SshAuthRequest } from "./components/dialog/connections/SshAuthDialog";
import TemporarySshLinkDialog from "./components/dialog/connections/TemporarySshLinkDialog";
import type { DockerSudoPasswordRequest } from "./components/dialog/docker/DockerSudoPasswordDialog";
import SessionQuickSwitcher, {
  type QuickSwitcherSession,
} from "./components/dialog/terminal/SessionQuickSwitcherDialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./components/ui/alert-dialog";
import { useApp } from "./context/AppContext";
import { TransferProvider } from "./context/TransferContext";
import { useActivityBarController } from "./hooks/useActivityBarController";
import {
  type ExternalOpenRequest,
  useExternalOpenRequests,
} from "./hooks/useExternalOpenRequests";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { useMacSelectionGuard } from "./hooks/useMacSelectionGuard";
import { useModalChildWindows } from "./hooks/useModalChildWindows";
import { useRemoteGpuOverview } from "./hooks/useRemoteGpuOverview";
import { useRemoteNpuOverview } from "./hooks/useRemoteNpuOverview";
import { useRemoteStats } from "./hooks/useRemoteStats";
import { resolveDisplayKeys } from "./hooks/useShortcutMap";
import { useTerminalZoom } from "./hooks/useTerminalZoom";
import { useTabStatusIndicators } from "./hooks/useUnreadTabs";

type SecurityPrompt =
  | { kind: "host-key"; request: HostKeyVerifyRequest }
  | { kind: "ssh-agent"; request: SshAgentAuthRequest }
  | { kind: "otp"; request: OtpRequest }
  | { kind: "ssh-auth"; request: SshAuthRequest };

function upsertSecurityPrompt(
  current: SecurityPrompt[],
  prompt: SecurityPrompt,
): SecurityPrompt[] {
  const index = current.findIndex(
    (item) => item.request.requestId === prompt.request.requestId,
  );
  if (index < 0) return [...current, prompt];
  const next = [...current];
  next[index] = prompt;
  return next;
}

import { AI_OPEN_EVENT, type AIOpenIntent } from "./lib/aiEvents";
import {
  assertMatchingTemporaryConfig,
  buildPanelOpenUpdate,
  canCreateSessionFromPane,
  collectActiveNonSerialSessionIds,
  EXCLUSIVE_PANEL_IDS,
  getSideOpenPanels,
  getSideOverlayPanel,
  getVisibleActivityIds,
  hasLiveSession,
  isActivityItemVisible,
  isNonSerialSessionType,
  NON_PANEL_IDS,
  type TrayAction,
} from "./lib/appWorkspace";
import {
  type AssetMonitoringCacheEntry,
  buildAssetPatchFromGpuOverview,
  buildAssetPatchFromNpuOverview,
  buildAssetPatchFromRemoteStats,
  recordAssetMonitoringPatch,
} from "./lib/assetMonitoring";
import { updateConnectionAutoIconAfterSessionStart } from "./lib/connectionAutoIcon";
import {
  getErrorMessage,
  shouldPromptConnectionEditOnFailure,
} from "./lib/errors";
import {
  type ExternalConnectionResolution,
  findExternalConnectionMatches,
  parseExternalOpenUrl,
} from "./lib/externalOpen";
import { normalizeHeaderStatusMode } from "./lib/headerStatus";
import { invoke } from "./lib/invoke";
import { logger } from "./lib/logger";
import {
  listenOpenSendCommandPanel,
  type SendCommandPanelDraft,
} from "./lib/sendCommandPanelEvents";
import {
  buildTerminalCommandInput,
  clearSessionCommandHistory,
  sendSessionInput,
  sendSessionInputWithSync,
} from "./lib/sessionInput";
import { buildSmartSplitLayout, type SmartSplitMode } from "./lib/smartSplit";
import {
  getSessionInputPeerIds,
  purgeSessionFromGroups,
} from "./lib/syncInputGroups";
import {
  findTerminalWindowLeafById,
  findTerminalWindowLeafByTabId,
  flattenTerminalWindows,
  insertTabAfterInLeaf,
  insertTabIntoLeaf,
  moveTabBetweenLeaves,
  reconcileTerminalWindows,
  reorderTabsInLeaf,
  restoreTerminalWindowLayout,
  type SplitEdgeDirection,
  serializeTerminalWindowLayout,
  setLeafActiveTab,
  splitLeafWithTab,
  splitTerminalWindowForTab,
  type TerminalWindowNode,
  updateTerminalWindowSplitRatio,
} from "./lib/tabWindows";
import type { TemporaryLinkConfig } from "./lib/temporaryLink";
import {
  captureTerminalReconnectContent,
  preserveTerminalReconnectContent,
} from "./lib/terminalReconnectHistory";
import { setBackendTransferDuplicatePrompt } from "./lib/transferDuplicatePrompt";
import { checkForUpdate, type UpdateInfo } from "./lib/updater";
import {
  getOwnerMainWindowLabel,
  type NewSessionTarget,
  openNewSession,
  openNewSessionWithTarget,
  openSettings,
  setOwnerMainWindowLabel,
} from "./lib/windowManager";
import {
  collectSessionPanes,
  findPaneBySessionId,
  findSessionPaneById,
  findTabBySessionId,
  getActivePane,
  getTabDisplayName,
} from "./lib/workspaceTabs";
import type {
  AppSettings,
  AssetMetadata,
  CloudConflictPreview,
  PaneSplitDirection,
  RecordingMode,
  RecordingStatus,
  SavedConnection,
  SessionInfo,
  SessionPane,
  SessionType,
  Tab,
  WorkspaceSessionType,
} from "./types/global";

const CONNECTION_SESSION_TYPES: Record<
  SavedConnection["type"],
  WorkspaceSessionType
> = {
  ssh: "SSH",
  local_terminal: "Local",
  telnet: "Telnet",
  serial: "Serial",
  rdp: "RDP",
  vnc: "VNC",
};

function getConnectionSessionType(
  connection: Pick<SavedConnection, "type"> | null | undefined,
): WorkspaceSessionType {
  return connection ? CONNECTION_SESSION_TYPES[connection.type] : "SSH";
}

function getRemoteDesktopPaneDisplay(
  connection: SavedConnection | null | undefined,
) {
  if (connection?.type === "rdp") {
    return {
      remoteWidth: connection.display?.width ?? 1920,
      remoteHeight: connection.display?.height ?? 1080,
      scaleMode: connection.display?.mode === "fit-window" ? "fit" : "actual",
    } as const;
  }
  if (connection?.type === "vnc") {
    return {
      scaleMode: connection.display?.scale_mode ?? "fit",
      viewOnly: connection.view_only ?? false,
      clipboardEnabled: connection.clipboard?.enabled ?? true,
    } as const;
  }
  return undefined;
}

function isSessionCreationCancelled(error: unknown) {
  return getErrorMessage(error)
    .toLowerCase()
    .includes("session creation cancelled");
}

async function attachSessionBeforeClose(sessionId: string) {
  await tauriInvoke("attach_session", { sessionId }).catch((error) => {
    logger.debug({
      domain: "session.lifecycle",
      event: "session.attach_before_close_failed",
      message: "Best-effort attach before close failed",
      ids: { session_id: sessionId },
      error,
    });
  });
}

async function closeStaleCreatedSession(sessionId: string) {
  try {
    await attachSessionBeforeClose(sessionId);
    await invoke("close_session", { sessionId });
    clearSessionCommandHistory(sessionId);
  } catch (error) {
    logger.error({
      domain: "session.lifecycle",
      event: "session.stale_close_failed",
      message: "Failed to close stale created session",
      ids: { session_id: sessionId },
      error,
    });
  }
}

type StartupCommandRequest = {
  command: string;
  delayMs: number;
};

type ExternalConnectionChoice =
  | { kind: "saved"; connection: SavedConnection }
  | { kind: "temporary"; config: TemporaryLinkConfig }
  | { kind: "cancelled" };

type ExternalMatchDialogState = {
  connections: SavedConnection[];
  temporary: TemporaryLinkConfig;
  resolve: (choice: ExternalConnectionChoice) => void;
};

type PostLoginConfirmState = {
  connection: SavedConnection;
  command: string;
  resolve: (confirmed: boolean) => void;
};

async function createSessionForConnection(
  connection: Pick<SavedConnection, "id" | "type">,
  createRequestId?: string,
  startupCommand?: StartupCommandRequest,
) {
  switch (connection.type) {
    case "local_terminal":
      return invoke<string>("create_local_session", {
        connectionId: connection.id,
        createRequestId,
      });
    case "telnet":
      return invoke<string>("create_telnet_session", {
        connectionId: connection.id,
        createRequestId,
        startupCommand: buildStartupCommandPayload(startupCommand),
      });
    case "serial":
      return invoke<string>("create_serial_session", {
        connectionId: connection.id,
        createRequestId,
      });
    case "vnc":
      return invoke<string>("create_vnc_session", {
        connectionId: connection.id,
        createRequestId,
      });
    case "rdp":
      return invoke<string>("create_rdp_session", {
        connectionId: connection.id,
        createRequestId,
      });
    default:
      return invoke<string>("create_ssh_session", {
        connectionId: connection.id,
        createRequestId,
        startupCommand: buildStartupCommandPayload(startupCommand),
      });
  }
}

async function createTemporarySession(
  config: TemporaryLinkConfig,
  createRequestId?: string,
  startupCommand?: StartupCommandRequest,
) {
  switch (config.protocol) {
    case "telnet":
      return invoke<string>("create_telnet_session", {
        connectionId: null,
        host: config.host,
        port: config.port,
        name: config.name,
        createRequestId,
        startupCommand: buildStartupCommandPayload(startupCommand),
      });
    case "serial":
      return invoke<string>("create_serial_session", {
        connectionId: null,
        portName: config.portName,
        baudRate: config.baudRate,
        name: config.name,
        createRequestId,
      });
    default: {
      const { protocol: _protocol, ...sshConfig } = config;
      return invoke<string>("create_temporary_ssh_session", {
        config: sshConfig,
        createRequestId,
        startupCommand: buildStartupCommandPayload(startupCommand),
      });
    }
  }
}

function getTemporaryLinkSessionType(config: TemporaryLinkConfig): SessionType {
  switch (config.protocol) {
    case "telnet":
      return "Telnet";
    case "serial":
      return "Serial";
    default:
      return "SSH";
  }
}

function buildStartupCommandPayload(startupCommand?: StartupCommandRequest) {
  if (!startupCommand || !startupCommand.command.trim()) return null;
  return {
    command: startupCommand.command,
    delayMs: Math.max(0, Math.min(60000, Math.round(startupCommand.delayMs))),
  };
}

async function sendStartupCommandToSession(
  sessionId: string,
  startupCommand: StartupCommandRequest,
) {
  const delayMs = Math.max(
    0,
    Math.min(60000, Math.round(startupCommand.delayMs)),
  );
  if (delayMs > 0) {
    await new Promise((resolve) => window.setTimeout(resolve, delayMs));
  }
  await sendSessionInput(
    sessionId,
    buildTerminalCommandInput(startupCommand.command),
    {
      preview: { kind: "reset" },
      registerSubmission: startupCommand.command,
      origin: "startup_command",
    },
  );
}

function safeRecordingName(name: string) {
  return (
    name.normalize("NFC").replace(/[^\p{L}\p{M}\p{N}._-]+/gu, "_") || "session"
  );
}

function joinPath(dir: string, fileName: string) {
  return `${dir}${dir.endsWith("\\") || dir.endsWith("/") ? "" : "/"}${fileName}`;
}

function eventTargetsCurrentWindow(targetWindowLabel?: string | null) {
  return !targetWindowLabel || targetWindowLabel === getOwnerMainWindowLabel();
}

function focusTerminalSession(sessionId?: string | null) {
  if (!sessionId) return;
  requestAnimationFrame(() => {
    void emit(`focus-terminal-${sessionId}`);
  });
}

function capturePaneReconnectContent(pane: SessionPane) {
  if (pane.connecting || pane.connectError) return null;
  return captureTerminalReconnectContent(pane.sessionId);
}

/** Root layout: header, activity bars, sidebars, terminal area, dialogs. */
function App() {
  useMacSelectionGuard();

  const {
    tabs,
    activeTabId,
    setActiveTabId,
    setActivePane,
    addTab,
    addPendingTab,
    updateTabSession,
    markTabConnectionFailed,
    updatePaneSession,
    markPaneConnectionFailed,
    markPaneConnecting,
    hasTab,
    hasPane,
    closePane,
    updateSplitRatio,
    persistTabsNow,
    updateUi,
    updateAppSettings,
    replaceAppSettings,
    appSettings,
    closeTabs,
    savedConnections,
    savedGroups,
    recordRecentConnection,
    syncGroups,
    setSyncGroups,
    broadcastToAll,
    setBroadcastToAll,
    isLocked,
    setIsLocked,
    settingsLoaded,
    startupRestoreComplete,
    runtimeInfo,
    runtimeInfoLoaded,
  } = useApp();
  const uiConfig = appSettings.ui;
  const portable = runtimeInfo.portable;
  const remoteStatsEnabled = uiConfig.show_remote_stats ?? true;
  const updateAutoIconForSessionStart = useCallback(
    (connectionId: string | null | undefined, sessionId: string) => {
      void updateConnectionAutoIconAfterSessionStart({
        connectionId,
        sessionId,
        remoteStatsEnabled,
      });
    },
    [remoteStatsEnabled],
  );
  const multiPanelOpen = appSettings.appearance.panel_multi_open;
  const { t, i18n } = useTranslation();

  useEffect(() => {
    if (!settingsLoaded) return;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      const currentWindow = getCurrentWindow();
      setOwnerMainWindowLabel(currentWindow.label);
      currentWindow.show();
    });
  }, [settingsLoaded]);

  useEffect(() => {
    if (appSettings.ui.language && appSettings.ui.language !== i18n.language) {
      i18n.changeLanguage(appSettings.ui.language);
    }
  }, [appSettings.ui.language, i18n]);

  // Mobile state
  const [mobileLeftOpen, setMobileLeftOpen] = useState(false);
  const [mobileRightOpen, setMobileRightOpen] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [showUpdateDialog, setShowUpdateDialog] = useState(false);
  const [showSyncGroupDialog, setShowSyncGroupDialog] = useState(false);
  const [showQuitConfirm, setShowQuitConfirm] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [helpDotVisible, setHelpDotVisible] = useState(false);
  const [sendCommandDraft, setSendCommandDraft] =
    useState<SendCommandPanelDraft | null>(null);
  const [showSessionQuickSwitcher, setShowSessionQuickSwitcher] =
    useState(false);
  const [showTemporarySshLink, setShowTemporarySshLink] = useState(false);
  const [externalMatchDialog, setExternalMatchDialog] =
    useState<ExternalMatchDialogState | null>(null);
  const [postLoginConfirm, setPostLoginConfirm] =
    useState<PostLoginConfirmState | null>(null);
  const allowProgrammaticWindowCloseRef = useRef(false);
  const handleSendCommandDraftConsumed = useCallback(() => {
    setSendCommandDraft(null);
  }, []);

  useEffect(() => {
    return listenOpenSendCommandPanel((draft) => {
      setSendCommandDraft(draft);
      updateUi({ show_serial_send_panel: true });
    });
  }, [updateUi]);

  // Recording state: active file recording statuses reported by the backend.
  const [recordingStatuses, setRecordingStatuses] = useState<RecordingStatus[]>(
    [],
  );
  const recordingSessions = useMemo(
    () => new Set(recordingStatuses.map((status) => status.sessionId)),
    [recordingStatuses],
  );
  const [liveSessionIds, setLiveSessionIds] = useState<Set<string> | null>(
    null,
  );
  const [liveSessionsById, setLiveSessionsById] = useState<Map<
    string,
    SessionInfo
  > | null>(null);
  const assetMonitoringCacheRef = useRef<
    Map<string, AssetMonitoringCacheEntry>
  >(new Map());
  const assetMonitoringFlushesRef = useRef<Set<string>>(new Set());

  const refreshRecordingStatuses = useCallback(async () => {
    try {
      const statuses = await invoke<RecordingStatus[]>(
        "list_recording_statuses",
      );
      setRecordingStatuses(statuses);
    } catch (error) {
      logger.error({
        domain: "session.lifecycle",
        event: "recording.list_failed",
        message: "Failed to list recording sessions",
        error,
      });
    }
  }, []);

  useEffect(() => {
    void refreshRecordingStatuses();
    const unlistenSessions = listen("sessions-changed", () => {
      void refreshRecordingStatuses();
    });
    const unlistenRecording = listen<RecordingStatus>(
      "recording-status-changed",
      () => {
        void refreshRecordingStatuses();
      },
    );
    return () => {
      unlistenSessions.then((dispose) => dispose());
      unlistenRecording.then((dispose) => dispose());
    };
  }, [refreshRecordingStatuses]);

  useEffect(() => {
    let disposed = false;

    const refreshLiveSessions = async () => {
      try {
        const sessions = await invoke<SessionInfo[]>("list_sessions");
        if (!disposed) {
          setLiveSessionIds(new Set(sessions.map((session) => session.id)));
          setLiveSessionsById(
            new Map(sessions.map((session) => [session.id, session])),
          );
        }
      } catch (error) {
        logger.error({
          domain: "session.lifecycle",
          event: "session.live_list_failed",
          message: "Failed to refresh live session ids",
          error,
        });
      }
    };

    void refreshLiveSessions();
    const unlisten = listen("sessions-changed", () => {
      void refreshLiveSessions();
    });

    return () => {
      disposed = true;
      unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    if (!settingsLoaded) return;
    void invoke("set_recording_memory_limit", {
      maxBytes: Math.max(
        1,
        appSettings.recording.memory_limit_bytes || 5 * 1024 * 1024,
      ),
    }).catch((error) => {
      logger.error({
        domain: "settings.persistence",
        event: "recording.memory_limit_sync_failed",
        message: "Failed to sync recording memory limit",
        error,
      });
    });
  }, [appSettings.recording.memory_limit_bytes, settingsLoaded]);

  const [securityPromptQueue, setSecurityPromptQueue] = useState<
    SecurityPrompt[]
  >([]);
  const [dockerSudoPasswordRequest, setDockerSudoPasswordRequest] =
    useState<DockerSudoPasswordRequest | null>(null);
  const [rdpCertificateRequests, setRdpCertificateRequests] = useState<
    RdpCertificateVerifyRequest[]
  >([]);
  const lastCloudConflictRevisionRef = useRef<string | null>(null);
  const modalChildWindowCount = useModalChildWindows();

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_screen_lock
      ? appSettings.security.idle_lock_minutes
      : 0,
    isLocked,
    () => setIsLocked(true),
  );

  // Background update check on startup
  useEffect(() => {
    if (!runtimeInfoLoaded) return;

    const timer = setTimeout(() => {
      checkForUpdate(portable)
        .then((info) => {
          if (info) {
            setUpdateInfo(info);
            setHelpDotVisible(true);
          }
        })
        .catch(() => {});
    }, 3000);
    return () => clearTimeout(timer);
  }, [portable, runtimeInfoLoaded]);

  const handleOpenPanel = useCallback(
    (panelId: "activeSessions" | "syncBackupHistory") => {
      updateUi((prev) => buildPanelOpenUpdate(prev, panelId, multiPanelOpen));
    },
    [multiPanelOpen, updateUi],
  );

  // Cross-window event listeners
  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];

    unsubs.push(
      listen<AppSettings>("settings-changed", () => {
        invoke<AppSettings>("get_app_settings").then((cfg) => {
          replaceAppSettings(cfg);
        });
      }),
    );

    unsubs.push(
      listen<{
        sessionId: string;
        name: string;
        type: WorkspaceSessionType;
        targetLeafId?: string;
        anchorTabId?: string | null;
        targetWindowLabel?: string | null;
      }>("session-created", (event) => {
        const {
          sessionId,
          name: sessionName,
          type,
          targetLeafId,
          anchorTabId,
          targetWindowLabel,
        } = event.payload;
        if (!eventTargetsCurrentWindow(targetWindowLabel)) return;
        const tabId = addTab(
          sessionId,
          sessionName,
          type,
          undefined,
          undefined,
          anchorTabId ? { afterTabId: anchorTabId } : undefined,
        );
        if (targetLeafId) {
          setTerminalWindows((current) =>
            current
              ? insertTabIntoLeaf(current, targetLeafId, tabId, {
                  afterTabId: anchorTabId,
                  activeTabId: tabId,
                })
              : current,
          );
        }
        focusTerminalSession(sessionId);
      }),
    );

    unsubs.push(
      listen<OtpRequest>("otp-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setSecurityPromptQueue((current) =>
          upsertSecurityPrompt(current, {
            kind: "otp",
            request: event.payload,
          }),
        );
      }),
    );

    unsubs.push(
      listen<SshAuthRequest>("ssh-auth-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setSecurityPromptQueue((current) =>
          upsertSecurityPrompt(current, {
            kind: "ssh-auth",
            request: event.payload,
          }),
        );
      }),
    );

    unsubs.push(
      listen<SshAgentAuthRequest>("ssh-agent-auth-pending", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setSecurityPromptQueue((current) =>
          upsertSecurityPrompt(current, {
            kind: "ssh-agent",
            request: event.payload,
          }),
        );
      }),
    );
    unsubs.push(
      listen<SshAgentAuthRequest>("ssh-agent-auth-failed", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setSecurityPromptQueue((current) =>
          upsertSecurityPrompt(current, {
            kind: "ssh-agent",
            request: event.payload,
          }),
        );
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("ssh-agent-auth-resolved", (event) => {
        setSecurityPromptQueue((current) =>
          current.filter(
            (item) => item.request.requestId !== event.payload.requestId,
          ),
        );
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("host-key-verify-resolved", (event) => {
        setSecurityPromptQueue((current) =>
          current.filter(
            (item) => item.request.requestId !== event.payload.requestId,
          ),
        );
      }),
    );
    unsubs.push(
      listen<{ requestId: string }>("security-prompt-resolved", (event) => {
        setSecurityPromptQueue((current) =>
          current.filter(
            (item) => item.request.requestId !== event.payload.requestId,
          ),
        );
      }),
    );

    unsubs.push(
      listen<DockerSudoPasswordRequest>(
        "docker-sudo-password-request",
        (event) => {
          if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel))
            return;
          setDockerSudoPasswordRequest(event.payload);
        },
      ),
    );

    unsubs.push(
      listen<HostKeyVerifyRequest>("host-key-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setSecurityPromptQueue((current) =>
          upsertSecurityPrompt(current, {
            kind: "host-key",
            request: event.payload,
          }),
        );
      }),
    );

    unsubs.push(
      listen<RdpCertificateVerifyRequest>("rdp-certificate-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setRdpCertificateRequests((current) => {
          if (
            current.some((item) => item.requestId === event.payload.requestId)
          )
            return current;
          return [...current, event.payload];
        });
      }),
    );

    unsubs.push(
      listen<{
        requestId: string;
        sessionId: string;
        remotePath: string;
        fileName: string;
        isDirectory: boolean;
        targetWindowLabel?: string | null;
      }>("transfer-duplicate-request", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setBackendTransferDuplicatePrompt({
          requestId: event.payload.requestId,
          sessionId: event.payload.sessionId,
          remotePath: event.payload.remotePath,
          fileName: event.payload.fileName,
          isDirectory: event.payload.isDirectory,
          targetWindowLabel: event.payload.targetWindowLabel,
          respondViaBackend: true,
        });
      }),
    );

    unsubs.push(
      listen<{
        connectionId: string;
        targetLeafId?: string;
        anchorTabId?: string | null;
        sourceTabId?: string;
        sourcePaneId?: string;
        targetWindowLabel?: string | null;
      }>("session-connect-after-edit", async (event) => {
        const {
          connectionId,
          targetLeafId,
          anchorTabId,
          sourceTabId,
          sourcePaneId,
          targetWindowLabel,
        } = event.payload;
        if (!eventTargetsCurrentWindow(targetWindowLabel)) return;
        try {
          const conns = await invoke<SavedConnection[]>(
            "get_saved_connections",
          );
          const conn = conns.find((c) => c.id === connectionId);
          const connName = conn?.name ?? connectionId;
          const sessionType = getConnectionSessionType(conn);
          const sourceTab = sourceTabId
            ? (tabsRef.current.find((item) => item.id === sourceTabId) ?? null)
            : null;
          const sourcePane =
            sourceTab &&
            ((sourcePaneId
              ? findSessionPaneById(sourceTab.root, sourcePaneId)
              : null) ??
              getActivePane(sourceTab));
          let tabId: string;
          let paneId: string | undefined;
          let createRequestId: string | null = null;

          if (sourceTab && sourcePane) {
            tabId = sourceTab.id;
            paneId = sourcePane.id;
            setActiveTabId(tabId);
            setActivePane(tabId, paneId);
            createRequestId = markPaneConnecting(tabId, paneId, {
              name: connName,
              type: sessionType,
              connectionId,
              display: getRemoteDesktopPaneDisplay(conn),
            });
          } else {
            const pending = addPendingTab(
              connName,
              sessionType,
              connectionId,
              undefined,
              anchorTabId ? { afterTabId: anchorTabId } : undefined,
              { display: getRemoteDesktopPaneDisplay(conn) },
            );
            tabId = pending.tabId;
            createRequestId = pending.createRequestId;
            if (targetLeafId) {
              setTerminalWindows((current) =>
                current
                  ? insertTabIntoLeaf(current, targetLeafId, tabId, {
                      afterTabId: anchorTabId,
                      activeTabId: tabId,
                    })
                  : current,
              );
            }
          }

          try {
            let sessionId: string;
            switch (conn?.type) {
              case "local_terminal":
                sessionId = await invoke<string>("create_local_session", {
                  connectionId,
                  createRequestId,
                });
                break;
              case "telnet":
                sessionId = await invoke<string>("create_telnet_session", {
                  connectionId,
                  createRequestId,
                });
                break;
              case "serial":
                sessionId = await invoke<string>("create_serial_session", {
                  connectionId,
                  createRequestId,
                });
                break;
              case "rdp":
                sessionId = await invoke<string>("create_rdp_session", {
                  connectionId,
                  createRequestId,
                });
                break;
              default:
                sessionId = await invoke<string>("create_ssh_session", {
                  connectionId,
                  createRequestId,
                });
                break;
            }
            if (paneId ? !hasPane(tabId, paneId) : !hasTab(tabId)) {
              await closeStaleCreatedSession(sessionId);
              return;
            }
            if (paneId) {
              updatePaneSession(tabId, paneId, sessionId);
            } else {
              updateTabSession(tabId, sessionId);
            }
            focusTerminalSession(sessionId);
            recordRecentConnection(connectionId);
            updateAutoIconForSessionStart(connectionId, sessionId);
          } catch (error) {
            if (
              isSessionCreationCancelled(error) ||
              (paneId ? !hasPane(tabId, paneId) : !hasTab(tabId))
            ) {
              return;
            }
            const errorMessage = getErrorMessage(error);
            if (paneId) {
              markPaneConnectionFailed(tabId, paneId, errorMessage);
            } else {
              markTabConnectionFailed(tabId, errorMessage);
            }
            if (shouldPromptConnectionEditOnFailure(conn, errorMessage)) {
              openNewSession(connectionId, true, {
                sourceTabId: tabId,
                sourcePaneId: paneId,
              });
            }
          }
        } catch {
          /* ignore */
        }
      }),
    );

    return () => {
      unsubs.forEach((p) => {
        p.then((unsub) => unsub());
      });
    };
  }, [
    addTab,
    addPendingTab,
    hasPane,
    hasTab,
    markPaneConnecting,
    markPaneConnectionFailed,
    markTabConnectionFailed,
    recordRecentConnection,
    setActivePane,
    setActiveTabId,
    updateAutoIconForSessionStart,
    updatePaneSession,
    updateTabSession,
    replaceAppSettings,
  ]);

  useEffect(() => {
    const unlisten = listen<CloudConflictPreview | null>(
      "cloud-sync-conflict",
      (event) => {
        const conflict = event.payload;
        if (!conflict) return;
        if (lastCloudConflictRevisionRef.current === conflict.remote_revision) {
          return;
        }

        lastCloudConflictRevisionRef.current = conflict.remote_revision;
        toast.error(conflict.message);
        handleOpenPanel("syncBackupHistory");
      },
    );

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [handleOpenPanel]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
  const activeTabName = activeTab ? getTabDisplayName(activeTab).trim() : "";
  const windowTitle = activeTabName ? `${activeTabName} - NyaTerm` : "NyaTerm";
  const activePane = activeTab ? getActivePane(activeTab) : null;
  const activeConnection = activePane?.connectionId
    ? (savedConnections.find(
        (connection) => connection.id === activePane.connectionId,
      ) ?? null)
    : null;
  const [aiIntent, setAiIntent] = useState<AIOpenIntent | null>(null);
  const [terminalWindows, setTerminalWindows] =
    useState<TerminalWindowNode | null>(null);
  const previousActiveTabIdRef = useRef<string | null>(null);
  const terminalWindowsRef = useRef<TerminalWindowNode | null>(null);
  const terminalWindowsRestoredRef = useRef(false);
  const terminalWindowsHydratedRef = useRef(false);
  const preserveRestoredLeafActiveTabsRef = useRef(false);
  const restoredGlobalActiveTabIdRef = useRef<string | null>(null);
  const lastPersistedTerminalWindowLayoutKeyRef = useRef(
    JSON.stringify(uiConfig.terminal_window_layout ?? null),
  );
  const tabsRef = useRef(tabs);
  const tabsById = useMemo(
    () => new Map(tabs.map((tab) => [tab.id, tab])),
    [tabs],
  );
  const savedSshConnectionIdBySessionId = useMemo(() => {
    const sshConnectionIds = new Set(
      savedConnections
        .filter((connection) => connection.type === "ssh")
        .map((connection) => connection.id),
    );
    const result = new Map<string, string>();

    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (
          !pane.connecting &&
          !pane.connectError &&
          pane.type === "SSH" &&
          pane.connectionId &&
          sshConnectionIds.has(pane.connectionId)
        ) {
          result.set(pane.sessionId, pane.connectionId);
        }
      }
    }

    return result;
  }, [savedConnections, tabs]);

  const handleAssetMonitoringPatch = useCallback(
    (sessionId: string, patch: AssetMetadata) => {
      const connectionId = savedSshConnectionIdBySessionId.get(sessionId);
      if (!connectionId) return;

      recordAssetMonitoringPatch(
        assetMonitoringCacheRef.current,
        sessionId,
        connectionId,
        patch,
      );
    },
    [savedSshConnectionIdBySessionId],
  );

  const flushAssetMonitoringCache = useCallback(async (sessionId: string) => {
    const entry = assetMonitoringCacheRef.current.get(sessionId);
    if (!entry || assetMonitoringFlushesRef.current.has(sessionId)) return;

    assetMonitoringFlushesRef.current.add(sessionId);
    try {
      await invoke("update_connection_asset_from_monitoring", {
        connectionId: entry.connectionId,
        assetPatch: {
          ...entry.lastAssetPatch,
          updated_at: new Date().toISOString(),
        },
      });
      assetMonitoringCacheRef.current.delete(sessionId);
    } catch (error) {
      const message = getErrorMessage(error).toLowerCase();
      if (message.includes("not found")) {
        assetMonitoringCacheRef.current.delete(sessionId);
      }
      logger.error({
        domain: "session.lifecycle",
        event: "asset.flush_failed",
        message: "Failed to save monitored asset snapshot",
        ids: { connection_id: entry.connectionId, session_id: sessionId },
        error,
      });
    } finally {
      assetMonitoringFlushesRef.current.delete(sessionId);
    }
  }, []);

  useEffect(() => {
    if (liveSessionIds === null) return;

    for (const sessionId of assetMonitoringCacheRef.current.keys()) {
      if (!liveSessionIds.has(sessionId)) {
        void flushAssetMonitoringCache(sessionId);
      }
    }
  }, [flushAssetMonitoringCache, liveSessionIds]);

  useEffect(() => {
    tabsRef.current = tabs;
  }, [tabs]);

  useEffect(() => {
    terminalWindowsRef.current = terminalWindows;
  }, [terminalWindows]);

  useEffect(() => {
    if (terminalWindowsRestoredRef.current) return;
    lastPersistedTerminalWindowLayoutKeyRef.current = JSON.stringify(
      uiConfig.terminal_window_layout ?? null,
    );
  }, [uiConfig.terminal_window_layout]);

  useEffect(() => {
    let cancelled = false;

    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        if (cancelled) return;
        return getCurrentWindow().setTitle(windowTitle);
      })
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [windowTitle]);

  const handleNewSession = useCallback((parentGroupId?: string) => {
    openNewSession(
      undefined,
      undefined,
      parentGroupId ? { initialGroupId: parentGroupId } : undefined,
    );
  }, []);

  const handleEditConnection = useCallback(
    (
      conn: SavedConnection,
      autoConnect?: boolean,
      target?: NewSessionTarget,
    ) => {
      openNewSession(conn.id, autoConnect, target);
    },
    [],
  );

  const maybePromptConnectionEdit = useCallback(
    (
      connectionId: string | undefined,
      errorMessage: string,
      target?: Pick<NewSessionTarget, "sourceTabId" | "sourcePaneId">,
    ) => {
      if (!connectionId) return;
      const connection = savedConnections.find(
        (item) => item.id === connectionId,
      );
      if (shouldPromptConnectionEditOnFailure(connection, errorMessage)) {
        openNewSession(connectionId, true, target);
      }
    },
    [savedConnections],
  );

  const connectSavedConnection = useCallback(
    async (
      connection: SavedConnection,
      options?: {
        failureContext?: string;
        startupCommand?: StartupCommandRequest;
      },
    ) => {
      const pending = addPendingTab(
        connection.name,
        getConnectionSessionType(connection),
        connection.id,
        undefined,
        undefined,
        { display: getRemoteDesktopPaneDisplay(connection) },
      );
      const { tabId, createRequestId } = pending;

      try {
        const sessionId = await createSessionForConnection(
          connection,
          createRequestId,
          options?.startupCommand,
        );
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        focusTerminalSession(sessionId);
        recordRecentConnection(connection.id);
        updateAutoIconForSessionStart(connection.id, sessionId);
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "connection.open_failed",
          message: options?.failureContext ?? "Connection failed",
          ids: { connection_id: connection.id },
          error,
        });
        markTabConnectionFailed(tabId, errorMessage);
        maybePromptConnectionEdit(connection.id, errorMessage, {
          sourceTabId: tabId,
        });
        toast.error(
          t("savedConnections.connectionFailed", { error: errorMessage }),
        );
      }
    },
    [
      addPendingTab,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const connectTemporaryConnection = useCallback(
    async (config: TemporaryLinkConfig) => {
      const pending = addPendingTab(
        config.name,
        getTemporaryLinkSessionType(config),
        undefined,
        undefined,
        undefined,
        { temporaryConfig: config },
      );
      const { tabId, createRequestId } = pending;

      try {
        const sessionId = await createTemporarySession(config, createRequestId);
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        focusTerminalSession(sessionId);
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "temporary_link.open_failed",
          message: "Temporary connection failed",
          error,
        });
        markTabConnectionFailed(tabId, errorMessage);
        toast.error(
          t("savedConnections.connectionFailed", { error: errorMessage }),
        );
      }
    },
    [addPendingTab, hasTab, markTabConnectionFailed, t, updateTabSession],
  );

  const chooseExternalConnection = useCallback(
    (
      resolution: Extract<ExternalConnectionResolution, { kind: "ambiguous" }>,
    ) =>
      new Promise<ExternalConnectionChoice>((resolve) => {
        setExternalMatchDialog({
          connections: resolution.connections,
          temporary: resolution.temporary,
          resolve,
        });
      }),
    [],
  );

  const confirmExternalPostLogin = useCallback(
    (connection: SavedConnection) => {
      const command = connection.post_login?.command?.trim() ?? "";
      if (!connection.post_login?.enabled || !command)
        return Promise.resolve(true);

      return new Promise<boolean>((resolve) => {
        setPostLoginConfirm({ connection, command, resolve });
      });
    },
    [],
  );

  const handleExternalOpenRequest = useCallback(
    async (request: ExternalOpenRequest) => {
      logger.info({
        domain: "app.lifecycle",
        event: "external_open.request_received",
        message: "Received external open request in target window",
        ids: { request_id: request.id },
        data: {
          source: request.source,
          target_window_label: request.targetWindowLabel,
        },
      });

      const parsed = parseExternalOpenUrl(request.rawUrl);
      if (!parsed.ok) {
        logger.warn({
          domain: "app.lifecycle",
          event: "external_open.request_rejected",
          message: "Rejected external open request in frontend",
          ids: { request_id: request.id },
          data: {
            scheme: parsed.scheme,
            source: request.source,
            target_window_label: request.targetWindowLabel,
            error_type: parsed.errorType,
          },
        });
        toast.error(t(parsed.errorKey));
        return;
      }

      const latestConnections = await invoke<SavedConnection[]>(
        "get_saved_connections",
      );
      const resolution = findExternalConnectionMatches(
        latestConnections,
        parsed.intent,
      );

      if (resolution.kind === "saved") {
        logger.info({
          domain: "app.lifecycle",
          event: "external_open.connection_matched",
          message: "External open matched a saved connection",
          ids: { request_id: request.id },
          data: {
            scheme: parsed.intent.protocol,
            source: request.source,
            target_window_label: request.targetWindowLabel,
            candidate_count: 1,
          },
        });
        if (await confirmExternalPostLogin(resolution.connection)) {
          await connectSavedConnection(resolution.connection, {
            failureContext: "External open saved connection failed",
          });
        }
        return;
      }

      if (resolution.kind === "ambiguous") {
        logger.info({
          domain: "app.lifecycle",
          event: "external_open.connection_ambiguous",
          message: "External open matched multiple saved connections",
          ids: { request_id: request.id },
          data: {
            scheme: parsed.intent.protocol,
            source: request.source,
            target_window_label: request.targetWindowLabel,
            candidate_count: resolution.connections.length,
          },
        });
        const choice = await chooseExternalConnection(resolution);
        if (choice.kind === "saved") {
          if (await confirmExternalPostLogin(choice.connection)) {
            await connectSavedConnection(choice.connection, {
              failureContext: "External open saved connection failed",
            });
          }
        } else if (choice.kind === "temporary") {
          await connectTemporaryConnection(choice.config);
        }
        return;
      }

      if (resolution.kind === "temporary") {
        logger.info({
          domain: "app.lifecycle",
          event: "external_open.temporary_connection_created",
          message: "External open will create a temporary connection",
          ids: { request_id: request.id },
          data: {
            scheme: parsed.intent.protocol,
            source: request.source,
            target_window_label: request.targetWindowLabel,
            candidate_count: 0,
          },
        });
        await connectTemporaryConnection(resolution.config);
      }
    },
    [
      chooseExternalConnection,
      confirmExternalPostLogin,
      connectSavedConnection,
      connectTemporaryConnection,
      t,
    ],
  );

  useExternalOpenRequests({
    ready: settingsLoaded && startupRestoreComplete && !isLocked,
    onRequest: handleExternalOpenRequest,
  });

  const persistTerminalWindowLayout = useCallback(
    (layout: TerminalWindowNode | null, nextTabs: Tab[] = tabsRef.current) => {
      if (
        !settingsLoaded ||
        !startupRestoreComplete ||
        !appSettings.general.startup_restore
      )
        return;
      const terminalWindowLayout =
        appSettings.general.startup_restore_window_layout === false
          ? null
          : serializeTerminalWindowLayout(layout, nextTabs);
      const layoutKey = JSON.stringify(terminalWindowLayout ?? null);
      if (layoutKey === lastPersistedTerminalWindowLayoutKeyRef.current) return;
      lastPersistedTerminalWindowLayoutKeyRef.current = layoutKey;
      updateUi({ terminal_window_layout: terminalWindowLayout });
    },
    [
      appSettings.general.startup_restore,
      appSettings.general.startup_restore_window_layout,
      settingsLoaded,
      startupRestoreComplete,
      updateUi,
    ],
  );

  useEffect(() => {
    if (!preserveRestoredLeafActiveTabsRef.current) return;
    if (restoredGlobalActiveTabIdRef.current === null) return;
    if (activeTabId === restoredGlobalActiveTabIdRef.current) return;
    preserveRestoredLeafActiveTabsRef.current = false;
    restoredGlobalActiveTabIdRef.current = null;
  }, [activeTabId]);

  useEffect(() => {
    if (!settingsLoaded || !startupRestoreComplete) return;

    setTerminalWindows((current) => {
      let next = current;
      let preserveRestoredLeafActiveTabs =
        preserveRestoredLeafActiveTabsRef.current;

      if (!terminalWindowsRestoredRef.current) {
        terminalWindowsRestoredRef.current = true;
        if (
          appSettings.general.startup_restore &&
          appSettings.general.startup_restore_window_layout !== false &&
          tabs.length > 0
        ) {
          const restored = restoreTerminalWindowLayout(
            uiConfig.terminal_window_layout,
            tabs,
          );
          if (restored) {
            next = restored;
            preserveRestoredLeafActiveTabs = true;
            preserveRestoredLeafActiveTabsRef.current = true;
            restoredGlobalActiveTabIdRef.current = activeTabId;
          }
        }
      }

      const reconciled = reconcileTerminalWindows(
        next,
        tabs,
        preserveRestoredLeafActiveTabs ? null : activeTabId,
        previousActiveTabIdRef.current,
      );
      terminalWindowsHydratedRef.current = true;
      terminalWindowsRef.current = reconciled;
      return reconciled;
    });
    previousActiveTabIdRef.current = activeTabId;
  }, [
    activeTabId,
    appSettings.general.startup_restore,
    appSettings.general.startup_restore_window_layout,
    settingsLoaded,
    startupRestoreComplete,
    tabs,
    uiConfig.terminal_window_layout,
  ]);

  useEffect(() => {
    if (!settingsLoaded) return;
    if (!startupRestoreComplete) return;
    if (!terminalWindowsRestoredRef.current) return;
    if (!terminalWindowsHydratedRef.current) return;
    if (tabs.length > 0 && !terminalWindows) return;
    persistTerminalWindowLayout(terminalWindows, tabs);
  }, [
    persistTerminalWindowLayout,
    settingsLoaded,
    startupRestoreComplete,
    tabs,
    terminalWindows,
  ]);

  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIOpenIntent>).detail;
      if (!detail) return;
      setAiIntent(detail);
      updateUi((prev) =>
        buildPanelOpenUpdate(prev, "aiAssistant", multiPanelOpen, "right"),
      );
    };
    window.addEventListener(AI_OPEN_EVENT, handler);
    return () => window.removeEventListener(AI_OPEN_EVENT, handler);
  }, [multiPanelOpen, updateUi]);

  const { unreadTabIds, disconnectedTabIds } = useTabStatusIndicators(
    tabs,
    activeTabId,
  );

  const handleSelectLeafTab = useCallback(
    (leafId: string, tabId: string) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) =>
        current ? setLeafActiveTab(current, leafId, tabId) : current,
      );
      setActiveTabId(tabId);
    },
    [setActiveTabId],
  );

  const handleAddTabFromLeaf = useCallback(
    (leafId: string) => {
      const targetLeaf = terminalWindows
        ? findTerminalWindowLeafById(terminalWindows, leafId)
        : null;
      if (targetLeaf?.activeTabId) {
        handleSelectLeafTab(leafId, targetLeaf.activeTabId);
      }
      openNewSessionWithTarget(undefined, undefined, {
        targetLeafId: leafId,
        anchorTabId:
          targetLeaf?.activeTabId ??
          targetLeaf?.tabIds[targetLeaf.tabIds.length - 1] ??
          null,
      });
    },
    [handleSelectLeafTab, terminalWindows],
  );

  const handleConnectConnectionFromLeaf = useCallback(
    async (leafId: string, connection: SavedConnection) => {
      const targetLeaf = terminalWindows
        ? findTerminalWindowLeafById(terminalWindows, leafId)
        : null;
      const anchorTabId =
        targetLeaf?.activeTabId ??
        targetLeaf?.tabIds[targetLeaf.tabIds.length - 1] ??
        null;

      if (targetLeaf?.activeTabId) {
        handleSelectLeafTab(leafId, targetLeaf.activeTabId);
      }

      const pending = addPendingTab(
        connection.name,
        getConnectionSessionType(connection),
        connection.id,
        undefined,
        anchorTabId ? { afterTabId: anchorTabId } : undefined,
        { display: getRemoteDesktopPaneDisplay(connection) },
      );
      const { tabId, createRequestId } = pending;

      if (targetLeaf) {
        setTerminalWindows((current) =>
          current
            ? insertTabIntoLeaf(current, leafId, tabId, {
                afterTabId: anchorTabId,
                activeTabId: tabId,
              })
            : current,
        );
      }

      try {
        const sessionId = await createSessionForConnection(
          connection,
          createRequestId,
        );
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        recordRecentConnection(connection.id);
        updateAutoIconForSessionStart(connection.id, sessionId);
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "connection.open_failed",
          message: "Connection failed from tab menu",
          ids: { connection_id: connection.id },
          error,
        });
        markTabConnectionFailed(tabId, errorMessage);
        maybePromptConnectionEdit(connection.id, errorMessage, {
          sourceTabId: tabId,
        });
        toast.error(
          t("savedConnections.connectionFailed", { error: errorMessage }),
        );
      }
    },
    [
      addPendingTab,
      hasTab,
      handleSelectLeafTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      terminalWindows,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const handleReorderTabsInLeaf = useCallback(
    (_: string, fromTabId: string, toIndex: number) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) =>
        current ? reorderTabsInLeaf(current, fromTabId, toIndex) : current,
      );
    },
    [],
  );

  const handleMoveTabToLeaf = useCallback(
    (fromTabId: string, targetLeafId: string, toIndex: number) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) => {
        if (!current) return current;
        const next = moveTabBetweenLeaves(
          current,
          fromTabId,
          targetLeafId,
          toIndex,
        );
        return next ?? current;
      });
      setActiveTabId(fromTabId);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
    },
    [setActiveTabId],
  );

  const handleSplitTabToLeaf = useCallback(
    (
      fromTabId: string,
      targetLeafId: string,
      direction: SplitEdgeDirection,
    ) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setTerminalWindows((current) => {
        if (!current) return current;
        const next = splitLeafWithTab(
          current,
          fromTabId,
          targetLeafId,
          direction,
        );
        return next ?? current;
      });
      setActiveTabId(fromTabId);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
    },
    [setActiveTabId],
  );

  const handleUnsplit = useCallback(() => {
    preserveRestoredLeafActiveTabsRef.current = false;
    restoredGlobalActiveTabIdRef.current = null;
    setTerminalWindows((current) => {
      if (!current) return current;
      return flattenTerminalWindows(current, activeTabId);
    });
    requestAnimationFrame(() => {
      window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
    });
  }, [activeTabId]);

  const handleUpdateWindowSplitRatio = useCallback(
    (splitId: string, ratio: number) => {
      setTerminalWindows((current) =>
        current
          ? updateTerminalWindowSplitRatio(current, splitId, ratio)
          : current,
      );
    },
    [],
  );

  const handleActivatePane = useCallback(
    (tabId: string, paneId: string) => {
      preserveRestoredLeafActiveTabsRef.current = false;
      restoredGlobalActiveTabIdRef.current = null;
      setActiveTabId(tabId);
      setActivePane(tabId, paneId);
    },
    [setActivePane, setActiveTabId],
  );

  const handleUpdatePaneSplitRatio = useCallback(
    (tabId: string, splitId: string, ratio: number) => {
      updateSplitRatio(tabId, splitId, ratio);
    },
    [updateSplitRatio],
  );

  const createSessionForPane = useCallback(
    async (
      pane: Pick<SessionPane, "type" | "connectionId" | "temporaryConfig">,
      createRequestId?: string,
      startupCommand?: StartupCommandRequest,
    ) => {
      switch (pane.type) {
        case "Local":
          return invoke<string>("create_local_session", {
            connectionId: pane.connectionId || null,
            createRequestId,
          });
        case "Telnet":
          if (pane.connectionId) {
            return invoke<string>("create_telnet_session", {
              connectionId: pane.connectionId,
              createRequestId,
              startupCommand: buildStartupCommandPayload(startupCommand),
            });
          }
          assertMatchingTemporaryConfig(pane);
          if (pane.temporaryConfig?.protocol === "telnet") {
            return invoke<string>("create_telnet_session", {
              connectionId: null,
              host: pane.temporaryConfig.host,
              port: pane.temporaryConfig.port,
              name: pane.temporaryConfig.name,
              createRequestId,
              startupCommand: buildStartupCommandPayload(startupCommand),
            });
          }
          throw new Error("Missing Telnet connection id");
        case "Serial":
          if (pane.connectionId) {
            return invoke<string>("create_serial_session", {
              connectionId: pane.connectionId,
              createRequestId,
            });
          }
          assertMatchingTemporaryConfig(pane);
          if (pane.temporaryConfig?.protocol === "serial") {
            return invoke<string>("create_serial_session", {
              connectionId: null,
              portName: pane.temporaryConfig.portName,
              baudRate: pane.temporaryConfig.baudRate,
              name: pane.temporaryConfig.name,
              createRequestId,
            });
          }
          throw new Error("Missing Serial connection id");
        case "VNC":
          if (!pane.connectionId) throw new Error("Missing VNC connection id");
          return invoke<string>("create_vnc_session", {
            connectionId: pane.connectionId,
            createRequestId,
          });
        case "RDP":
          if (!pane.connectionId) throw new Error("Missing RDP connection id");
          return invoke<string>("create_rdp_session", {
            connectionId: pane.connectionId,
            createRequestId,
          });
        default:
          if (pane.connectionId) {
            return invoke<string>("create_ssh_session", {
              connectionId: pane.connectionId,
              createRequestId,
              startupCommand: buildStartupCommandPayload(startupCommand),
            });
          }
          assertMatchingTemporaryConfig(pane);
          if (pane.temporaryConfig?.protocol === "ssh") {
            const { protocol: _protocol, ...sshConfig } = pane.temporaryConfig;
            return invoke<string>("create_temporary_ssh_session", {
              config: sshConfig,
              createRequestId,
              startupCommand: buildStartupCommandPayload(startupCommand),
            });
          }
          throw new Error("Missing SSH connection id");
      }
    },
    [],
  );

  const closePaneBackendSession = useCallback(
    async (
      pane: Pick<
        SessionPane,
        "connecting" | "connectError" | "sessionId" | "createRequestId" | "type"
      >,
    ) => {
      if (pane.connecting) {
        if (pane.type === "RDP") {
          await invoke("close_rdp_session", {
            sessionId: pane.sessionId,
          }).catch(() => {});
          return true;
        }
        if (pane.type === "VNC") {
          await invoke("close_vnc_session", {
            sessionId: pane.sessionId,
          }).catch(() => {});
          return true;
        }
        if (pane.createRequestId) {
          try {
            await invoke("cancel_session_creation", {
              createRequestId: pane.createRequestId,
            });
          } catch (error) {
            logger.error({
              domain: "session.lifecycle",
              event: "session.creation_cancel_failed",
              message: "Failed to cancel session creation",
              data: { create_request_id: pane.createRequestId },
              error,
            });
          }
        }
        return true;
      }

      if (pane.connectError) {
        return true;
      }

      try {
        if (pane.type === "RDP") {
          await invoke("close_rdp_session", { sessionId: pane.sessionId });
          return true;
        }
        if (pane.type === "VNC") {
          await invoke("close_vnc_session", { sessionId: pane.sessionId });
          return true;
        }
        await flushAssetMonitoringCache(pane.sessionId);
        await attachSessionBeforeClose(pane.sessionId);
        await invoke("close_session", { sessionId: pane.sessionId });
        clearSessionCommandHistory(pane.sessionId);
        setSyncGroups((prev) => purgeSessionFromGroups(pane.sessionId, prev));
        return true;
      } catch (error) {
        logger.error({
          domain: "session.lifecycle",
          event: "session.close_failed",
          message: "Failed to close session",
          ids: { session_id: pane.sessionId },
          error,
        });
        return false;
      }
    },
    [flushAssetMonitoringCache, setSyncGroups],
  );

  const closeWorkspaceTabSessions = useCallback(
    async (tab: Tab) => {
      const results = await Promise.all(
        collectSessionPanes(tab.root).map((pane) =>
          closePaneBackendSession(pane),
        ),
      );
      return results.every(Boolean);
    },
    [closePaneBackendSession],
  );

  const persistWorkspaceNow = useCallback(
    async (message: string) => {
      try {
        await persistTabsNow();
        return true;
      } catch (error) {
        logger.error({
          domain: "settings.persistence",
          event: "workspace_tabs.persist_failed",
          message: "Failed to persist workspace tabs",
          error,
        });
        toast.error(message);
        return false;
      }
    },
    [persistTabsNow],
  );

  const notifyLockedTabCloseBlocked = useCallback(() => {
    toast.info(t("tabCtx.lockedCloseBlocked"));
  }, [t]);

  const handleCloseWorkspaceTab = useCallback(
    async (tab: Tab) => {
      if (tab.locked) {
        notifyLockedTabCloseBlocked();
        return;
      }

      const allClosed = await closeWorkspaceTabSessions(tab);
      if (!allClosed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }
      closeTabs([tab.id]);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [
      closeTabs,
      closeWorkspaceTabSessions,
      notifyLockedTabCloseBlocked,
      persistWorkspaceNow,
      t,
    ],
  );

  const handleCloseDisconnectedPane = useCallback(
    async (tabId: string, paneId: string) => {
      const tab = tabs.find((item) => item.id === tabId);
      const pane = tab ? findSessionPaneById(tab.root, paneId) : null;
      if (!tab || !pane) return;
      if (tab.locked) {
        notifyLockedTabCloseBlocked();
        return;
      }

      const closed = await closePaneBackendSession(pane);
      if (!closed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }

      closePane(tab.id, pane.id);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [
      closePane,
      closePaneBackendSession,
      notifyLockedTabCloseBlocked,
      persistWorkspaceNow,
      t,
      tabs,
    ],
  );

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      if (tab && pane) {
        setTerminalWindows((current) => {
          const leaf = current
            ? findTerminalWindowLeafByTabId(current, tab.id)
            : null;
          return current && leaf
            ? setLeafActiveTab(current, leaf.id, tab.id)
            : current;
        });
        setActiveTabId(tab.id);
        setActivePane(tab.id, pane.id);
      }
    },
    [tabs, setActivePane, setActiveTabId],
  );

  const getQuickCommandPeerSessionIds = useCallback(
    (sessionId: string) => {
      return getSessionInputPeerIds(
        sessionId,
        syncGroups,
        tabs,
        broadcastToAll,
      );
    },
    [broadcastToAll, syncGroups, tabs],
  );

  const handleHistoryCommand = useCallback(
    (command: string, execute: boolean = true) => {
      if (!hasLiveSession(activePane)) return;

      const { sessionId } = activePane;
      const data = buildTerminalCommandInput(command, execute);
      const options = {
        preview: execute
          ? ({ kind: "reset" } as const)
          : ({ kind: "data", data: command } as const),
        registerSubmission: execute ? command : null,
      };
      const peerSessionIds = getQuickCommandPeerSessionIds(sessionId);
      const sendInput =
        peerSessionIds.length > 0
          ? sendSessionInputWithSync(sessionId, data, peerSessionIds, options)
          : sendSessionInput(sessionId, data, options);

      void sendInput.catch(() => {});
      import("@tauri-apps/api/event").then(({ emit }) => {
        emit(`focus-terminal-${sessionId}`);
      });
    },
    [activePane, getQuickCommandPeerSessionIds],
  );

  const handleSendToAllSessions = useCallback(
    (command: string, execute: boolean = true) => {
      const data = buildTerminalCommandInput(command, execute);
      for (const tab of tabs) {
        for (const pane of collectSessionPanes(tab.root)) {
          if (!hasLiveSession(pane) || !isNonSerialSessionType(pane.type))
            continue;
          const { sessionId } = pane;
          void sendSessionInput(sessionId, data, {
            preview: execute
              ? { kind: "reset" }
              : { kind: "data", data: command },
            registerSubmission: execute ? command : null,
          }).catch(() => {});
        }
      }
    },
    [tabs],
  );

  const handleReconnected = useCallback(
    (oldSessionId: string, newSessionId: string) => {
      const tab = findTabBySessionId(tabs, oldSessionId);
      const pane = tab ? findPaneBySessionId(tab, oldSessionId) : null;
      if (tab && pane) {
        updatePaneSession(tab.id, pane.id, newSessionId);
        updateAutoIconForSessionStart(pane.connectionId, newSessionId);
      }
    },
    [tabs, updateAutoIconForSessionStart, updatePaneSession],
  );

  const handleConnectionError = useCallback(
    (tabId: string, paneId: string, sessionId: string, error: string) => {
      const tab = tabs.find((item) => item.id === tabId);
      const pane = tab ? findSessionPaneById(tab.root, paneId) : null;
      if (!pane || pane.sessionId !== sessionId || pane.connectError) return;
      markPaneConnectionFailed(tabId, paneId, error);
    },
    [markPaneConnectionFailed, tabs],
  );

  // --- Shortcut callbacks ---

  const handleNewLocalTerminal = useCallback(() => {
    invoke<string>("create_local_session")
      .then((sessionId) => {
        addTab(sessionId, t("menu.newLocalTerminal"), "Local");
      })
      .catch((e) =>
        logger.error({
          domain: "session.lifecycle",
          event: "session.create_failed",
          message: "Failed to create local session",
          data: { session_type: "Local" },
          error: e,
        }),
      );
  }, [addTab, t]);

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTab) return;
    if (activeTab.locked) {
      notifyLockedTabCloseBlocked();
      return;
    }
    void handleCloseWorkspaceTab(activeTab);
  }, [activeTab, handleCloseWorkspaceTab, notifyLockedTabCloseBlocked]);

  const getActiveLeafTabIds = useCallback(() => {
    const leaf =
      terminalWindows && activeTabId
        ? findTerminalWindowLeafByTabId(terminalWindows, activeTabId)
        : null;
    return leaf?.tabIds ?? tabs.map((tab) => tab.id);
  }, [activeTabId, tabs, terminalWindows]);

  const handleNextTab = useCallback(() => {
    if (!activeTabId) return;
    const tabIds = getActiveLeafTabIds();
    if (tabIds.length < 2) return;
    const idx = tabIds.indexOf(activeTabId);
    if (idx === -1) return;
    setActiveTabId(tabIds[(idx + 1) % tabIds.length]);
  }, [activeTabId, getActiveLeafTabIds, setActiveTabId]);

  const handlePrevTab = useCallback(() => {
    if (!activeTabId) return;
    const tabIds = getActiveLeafTabIds();
    if (tabIds.length < 2) return;
    const idx = tabIds.indexOf(activeTabId);
    if (idx === -1) return;
    setActiveTabId(tabIds[(idx - 1 + tabIds.length) % tabIds.length]);
  }, [activeTabId, getActiveLeafTabIds, setActiveTabId]);

  const handleSwitchTab = useCallback(
    (index: number) => {
      const tabIds = getActiveLeafTabIds();
      const targetTabId =
        index === -1 ? tabIds[tabIds.length - 1] : tabIds[index];
      if (targetTabId) setActiveTabId(targetTabId);
    },
    [getActiveLeafTabIds, setActiveTabId],
  );

  const handleToggleLeftSidebar = useCallback(() => {
    updateUi((prev) => {
      if (multiPanelOpen) {
        if (
          (prev.left_open_panels?.length ?? 0) > 0 ||
          prev.active_left_panel
        ) {
          return { left_open_panels: [], active_left_panel: null };
        }
        const first = getVisibleActivityIds(
          [
            ...prev.activity_bar_layout.left_top,
            ...prev.activity_bar_layout.left_bottom,
          ],
          prev,
        ).find((id) => !NON_PANEL_IDS.has(id));
        if (!first) return {};
        return EXCLUSIVE_PANEL_IDS.has(first)
          ? { active_left_panel: first }
          : { left_open_panels: [first], active_left_panel: first };
      }
      if (prev.active_left_panel) return { active_left_panel: null };
      const first = getVisibleActivityIds(
        [
          ...prev.activity_bar_layout.left_top,
          ...prev.activity_bar_layout.left_bottom,
        ],
        prev,
      ).find((id) => !NON_PANEL_IDS.has(id));
      return { active_left_panel: first ?? null };
    });
  }, [multiPanelOpen, updateUi]);

  const handleToggleRightSidebar = useCallback(() => {
    updateUi((prev) => {
      if (multiPanelOpen) {
        if (
          (prev.right_open_panels?.length ?? 0) > 0 ||
          prev.active_right_panel
        ) {
          return { right_open_panels: [], active_right_panel: null };
        }
        const first = getVisibleActivityIds(
          [
            ...prev.activity_bar_layout.right_top,
            ...prev.activity_bar_layout.right_bottom,
          ],
          prev,
        ).find((id) => !NON_PANEL_IDS.has(id));
        if (!first) return {};
        return EXCLUSIVE_PANEL_IDS.has(first)
          ? { active_right_panel: first }
          : { right_open_panels: [first], active_right_panel: first };
      }
      if (prev.active_right_panel) return { active_right_panel: null };
      const first = getVisibleActivityIds(
        [
          ...prev.activity_bar_layout.right_top,
          ...prev.activity_bar_layout.right_bottom,
        ],
        prev,
      ).find((id) => !NON_PANEL_IDS.has(id));
      return { active_right_panel: first ?? null };
    });
  }, [multiPanelOpen, updateUi]);

  const { handleZoomIn, handleZoomOut, handleResetZoom } = useTerminalZoom(
    updateAppSettings,
    appSettings.keybindings,
    appSettings.interaction.terminal_zoom_enabled,
  );

  const handleOpenSettings = useCallback(() => {
    openSettings();
  }, []);

  const handleLockScreen = useCallback(() => {
    if (appSettings.security.enable_screen_lock) {
      setIsLocked(true);
    }
  }, [appSettings.security.enable_screen_lock, setIsLocked]);

  const persistWorkspaceLayoutNow = useCallback(async () => {
    if (
      !settingsLoaded ||
      !startupRestoreComplete ||
      !terminalWindowsRestoredRef.current
    ) {
      await persistTabsNow();
      return;
    }

    const terminalWindowLayout =
      appSettings.general.startup_restore_window_layout === false
        ? null
        : serializeTerminalWindowLayout(
            terminalWindowsRef.current,
            tabsRef.current,
          );
    lastPersistedTerminalWindowLayoutKeyRef.current = JSON.stringify(
      terminalWindowLayout ?? null,
    );
    await persistTabsNow({ terminal_window_layout: terminalWindowLayout });
  }, [
    appSettings.general.startup_restore_window_layout,
    persistTabsNow,
    settingsLoaded,
    startupRestoreComplete,
  ]);

  const handleQuitApplication = useCallback(() => {
    setShowQuitConfirm(false);
    void persistWorkspaceLayoutNow()
      .catch((error) => {
        logger.error({
          domain: "settings.persistence",
          event: "workspace_layout.persist_before_quit_failed",
          message: "Failed to persist workspace layout before quit",
          error,
        });
      })
      .finally(() => {
        void invoke<void>("quit_application");
      });
  }, [persistWorkspaceLayoutNow]);

  useEffect(() => {
    if (!settingsLoaded) return;
    let unlistenCloseRequested: (() => void) | undefined;

    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        const currentWindow = getCurrentWindow();
        return currentWindow.onCloseRequested(async (event) => {
          if (allowProgrammaticWindowCloseRef.current) return;

          event.preventDefault();

          try {
            await persistWorkspaceLayoutNow();
          } catch (error) {
            logger.error({
              domain: "settings.persistence",
              event: "workspace_layout.persist_before_close_failed",
              message: "Failed to persist workspace layout before close",
              error,
            });
          }

          if (appSettings.general.minimize_to_tray) {
            await invoke<void>("hide_main_window").catch(() => {});
            return;
          }

          allowProgrammaticWindowCloseRef.current = true;
          await currentWindow.close().catch(() => {
            allowProgrammaticWindowCloseRef.current = false;
          });
          window.setTimeout(() => {
            allowProgrammaticWindowCloseRef.current = false;
          }, 1000);
        });
      })
      .then((unlisten) => {
        unlistenCloseRequested = unlisten;
      })
      .catch(() => {});

    return () => {
      unlistenCloseRequested?.();
    };
  }, [
    appSettings.general.minimize_to_tray,
    persistWorkspaceLayoutNow,
    settingsLoaded,
  ]);

  const handleRequestQuit = useCallback(() => {
    if (tabs.length > 0 && appSettings.general.confirm_on_close !== false) {
      setShowQuitConfirm(true);
      return;
    }

    handleQuitApplication();
  }, [
    appSettings.general.confirm_on_close,
    handleQuitApplication,
    tabs.length,
  ]);

  const handleRequestWindowClose = useCallback(() => {
    if (
      !appSettings.general.minimize_to_tray &&
      tabs.length > 0 &&
      appSettings.general.confirm_on_close !== false
    ) {
      setShowQuitConfirm(true);
      return;
    }

    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        allowProgrammaticWindowCloseRef.current = true;
        return getCurrentWindow()
          .close()
          .catch((error) => {
            allowProgrammaticWindowCloseRef.current = false;
            throw error;
          });
      })
      .catch(() => {})
      .finally(() => {
        window.setTimeout(() => {
          allowProgrammaticWindowCloseRef.current = false;
        }, 1000);
      });
  }, [
    appSettings.general.confirm_on_close,
    appSettings.general.minimize_to_tray,
    tabs.length,
  ]);

  useEffect(() => {
    const unlisten = listen<TrayAction>("tray-action", ({ payload }) => {
      if (!eventTargetsCurrentWindow(payload.targetWindowLabel)) return;
      if (
        isLocked &&
        payload.type !== "lock_screen" &&
        payload.type !== "request_quit"
      ) {
        return;
      }

      switch (payload.type) {
        case "open_new_session":
          handleNewSession();
          break;
        case "focus_session":
          handleSessionClick(payload.sessionId);
          break;
        case "open_panel":
          handleOpenPanel(payload.panelId);
          break;
        case "open_settings":
          handleOpenSettings();
          break;
        case "lock_screen":
          handleLockScreen();
          break;
        case "check_updates":
          setShowUpdateDialog(true);
          break;
        case "request_quit":
          handleRequestQuit();
          break;
      }
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [
    handleLockScreen,
    handleNewSession,
    handleOpenPanel,
    handleOpenSettings,
    handleRequestQuit,
    handleSessionClick,
    isLocked,
  ]);

  // --- Tab context-menu callbacks ---

  const handleDuplicateSession = useCallback(
    async (tab: Tab, startupCommand?: StartupCommandRequest) => {
      const pane = getActivePane(tab);
      if (!canCreateSessionFromPane(pane)) return;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
          { temporaryConfig: pane.temporaryConfig },
        );
        const { tabId, createRequestId } = pending;
        setTerminalWindows((current) =>
          current
            ? insertTabAfterInLeaf(current, tab.id, tabId, tabId)
            : current,
        );
        try {
          const sessionId = await createSessionForPane(
            pane,
            createRequestId,
            startupCommand,
          );
          if (!hasTab(tabId)) {
            await closeStaleCreatedSession(sessionId);
            return;
          }
          updateTabSession(tabId, sessionId);
          if (startupCommand && pane.type !== "SSH" && pane.type !== "Telnet") {
            void sendStartupCommandToSession(sessionId, startupCommand).catch(
              (error) => {
                logger.error({
                  domain: "session.lifecycle",
                  event: "session.startup_command_failed",
                  message:
                    "Failed to send startup command to duplicated session",
                  ids: { session_id: sessionId },
                  error,
                });
                toast.error(t("tabCtx.duplicateFailed"));
              },
            );
          }
          if (pane.connectionId) {
            recordRecentConnection(pane.connectionId);
            updateAutoIconForSessionStart(pane.connectionId, sessionId);
          }
        } catch (error) {
          if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
            return;
          }
          const errorMessage = getErrorMessage(error);
          logger.error({
            domain: "session.lifecycle",
            event: "session.duplicate_failed",
            message: "Failed to duplicate session",
            ids: pane.connectionId
              ? { connection_id: pane.connectionId }
              : undefined,
            error,
          });
          markTabConnectionFailed(tabId, errorMessage);
          maybePromptConnectionEdit(pane.connectionId, errorMessage, {
            sourceTabId: tabId,
          });
          toast.error(t("tabCtx.duplicateFailed"));
        }
      } catch (error) {
        logger.error({
          domain: "ui.error",
          event: "tab.duplicate_failed",
          message: "Failed to create duplicated tab",
          error,
        });
        toast.error(t("tabCtx.duplicateFailed"));
      }
    },
    [
      addPendingTab,
      createSessionForPane,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const handleMultiplexSshSession = useCallback(
    async (tab: Tab, startupCommand?: StartupCommandRequest) => {
      const pane = getActivePane(tab);
      if (!pane || pane.type !== "SSH" || pane.connecting || pane.connectError)
        return;

      let tabId: string | undefined;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
          { temporaryConfig: pane.temporaryConfig },
        );
        tabId = pending.tabId;
        setTerminalWindows((current) =>
          current && tabId
            ? insertTabAfterInLeaf(current, tab.id, tabId, tabId)
            : current,
        );

        const sessionId = await invoke<string>(
          "create_multiplexed_ssh_session",
          {
            sourceSessionId: pane.sessionId,
            startupCommand: buildStartupCommandPayload(startupCommand),
          },
        );
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
          updateAutoIconForSessionStart(pane.connectionId, sessionId);
        }
      } catch (error) {
        if ((tabId && !hasTab(tabId)) || isSessionCreationCancelled(error)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "session.multiplex_failed",
          message: "Failed to create multiplexed SSH session",
          ids: pane.connectionId
            ? { connection_id: pane.connectionId, session_id: pane.sessionId }
            : { session_id: pane.sessionId },
          error,
        });
        if (tabId) {
          markTabConnectionFailed(tabId, errorMessage);
        }
        toast.error(t("tabCtx.multiplexSshFailed"));
      }
    },
    [
      addPendingTab,
      hasTab,
      markTabConnectionFailed,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const handleDuplicateSessionWithCommand = useCallback(
    (tab: Tab, command: string, delayMs: number) =>
      handleDuplicateSession(tab, { command, delayMs }),
    [handleDuplicateSession],
  );

  const handleMultiplexSshSessionWithCommand = useCallback(
    (tab: Tab, command: string, delayMs: number) =>
      handleMultiplexSshSession(tab, { command, delayMs }),
    [handleMultiplexSshSession],
  );

  const getActiveTab = useCallback(
    () => tabs.find((tab) => tab.id === activeTabId) ?? null,
    [activeTabId, tabs],
  );

  const handleDuplicateActiveSession = useCallback(() => {
    const tab = getActiveTab();
    if (tab) void handleDuplicateSession(tab);
  }, [getActiveTab, handleDuplicateSession]);

  const handleMultiplexActiveSshSession = useCallback(() => {
    const tab = getActiveTab();
    if (tab) void handleMultiplexSshSession(tab);
  }, [getActiveTab, handleMultiplexSshSession]);

  const handleDuplicateActiveSessionWithCommand = useCallback(() => {
    const tab = getActiveTab();
    if (!tab) return;
    window.dispatchEvent(
      new CustomEvent("nyaterm:open-tab-startup-command-dialog", {
        detail: { tabId: tab.id, action: "duplicate" },
      }),
    );
  }, [getActiveTab]);

  const handleMultiplexActiveSshSessionWithCommand = useCallback(() => {
    const tab = getActiveTab();
    if (!tab) return;
    window.dispatchEvent(
      new CustomEvent("nyaterm:open-tab-startup-command-dialog", {
        detail: { tabId: tab.id, action: "multiplex" },
      }),
    );
  }, [getActiveTab]);

  const handleReconnectSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;

      toast.info(t("tabCtx.reconnecting"));

      try {
        if (pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(pane);
        if (!hasPane(tab.id, pane.id)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        preserveTerminalReconnectContent(newSessionId, reconnectContent);
        updatePaneSession(tab.id, pane.id, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
          updateAutoIconForSessionStart(pane.connectionId, newSessionId);
        }
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tab.id, pane.id)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        if (pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect session",
          ids: pane.connectionId
            ? { connection_id: pane.connectionId }
            : undefined,
          error,
        });
        maybePromptConnectionEdit(pane.connectionId, errorMessage, {
          sourceTabId: tab.id,
          sourcePaneId: pane.id,
        });
        toast.error(t("tabCtx.reconnectFailed"));
      }
    },
    [
      closePaneBackendSession,
      createSessionForPane,
      hasPane,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updatePaneSession,
    ],
  );

  const handleDisconnectSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.connecting || pane.connectError) return;

      const closed = await closePaneBackendSession(pane);
      if (!closed) {
        toast.error(t("tabCtx.disconnectFailed"));
        return;
      }

      toast.success(t("tabCtx.disconnectSuccess"));
    },
    [closePaneBackendSession, t],
  );

  const handleReconnectSessionById = useCallback(
    async (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      if (!tab || !pane || pane.connecting || !canCreateSessionFromPane(pane))
        return;

      toast.info(t("tabCtx.reconnecting"));

      try {
        if (pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(pane);
        if (!hasPane(tab.id, pane.id)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        preserveTerminalReconnectContent(newSessionId, reconnectContent);
        updatePaneSession(tab.id, pane.id, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
          updateAutoIconForSessionStart(pane.connectionId, newSessionId);
        }
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tab.id, pane.id)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        if (pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect session from active sessions panel",
          ids: pane.connectionId
            ? { connection_id: pane.connectionId }
            : undefined,
          error,
        });
        maybePromptConnectionEdit(pane.connectionId, errorMessage, {
          sourceTabId: tab.id,
          sourcePaneId: pane.id,
        });
        toast.error(t("tabCtx.reconnectFailed"));
      }
    },
    [
      closePaneBackendSession,
      createSessionForPane,
      hasPane,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      tabs,
      updateAutoIconForSessionStart,
      updatePaneSession,
    ],
  );

  const handleSplitSession = useCallback(
    async (tab: Tab, direction: PaneSplitDirection) => {
      const pane = getActivePane(tab);
      if (!pane || !canCreateSessionFromPane(pane)) return;
      const leaf = terminalWindows
        ? findTerminalWindowLeafByTabId(terminalWindows, tab.id)
        : null;
      if (!leaf) {
        toast.error(t("tabCtx.splitFailed"));
        return;
      }

      if (leaf.tabIds.length > 1) {
        setTerminalWindows((current) =>
          current
            ? splitTerminalWindowForTab(current, tab.id, direction)
            : current,
        );
        setActiveTabId(tab.id);
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
        return;
      }

      let newTabId: string | undefined;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
          { temporaryConfig: pane.temporaryConfig },
        );
        newTabId = pending.tabId;
        setTerminalWindows((current) =>
          current
            ? splitTerminalWindowForTab(current, tab.id, direction, newTabId)
            : current,
        );
        const sessionId = await createSessionForPane(
          pane,
          pending.createRequestId,
        );
        if (newTabId) {
          if (!hasTab(newTabId)) {
            await closeStaleCreatedSession(sessionId);
            return;
          }
          updateTabSession(newTabId, sessionId);
        }
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
          updateAutoIconForSessionStart(pane.connectionId, sessionId);
        }
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      } catch (error) {
        if (
          (newTabId && !hasTab(newTabId)) ||
          isSessionCreationCancelled(error)
        ) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "session.split_failed",
          message: "Failed to create split session",
          ids: pane.connectionId
            ? { connection_id: pane.connectionId }
            : undefined,
          error,
        });
        if (newTabId) {
          markTabConnectionFailed(newTabId, errorMessage);
        }
        maybePromptConnectionEdit(
          pane.connectionId,
          errorMessage,
          newTabId ? { sourceTabId: newTabId } : undefined,
        );
        toast.error(t("tabCtx.splitFailed"));
      }
    },
    [
      addPendingTab,
      createSessionForPane,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      setActiveTabId,
      t,
      terminalWindows,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const handleSmartSplit = useCallback(
    (mode: SmartSplitMode) => {
      const tabIds = tabs.map((tab) => tab.id);
      if (tabIds.length === 0) return;

      const layout = buildSmartSplitLayout(tabIds, mode);
      if (layout) {
        setTerminalWindows(layout);
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      }
    },
    [tabs],
  );

  const handleReconnectPane = useCallback(
    async (tabId: string, paneId: string) => {
      const tab = tabs.find((item) => item.id === tabId);
      const pane = tab
        ? (collectSessionPanes(tab.root).find((item) => item.id === paneId) ??
          null)
        : null;
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;

      try {
        if (pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(pane);
        if (!hasPane(tabId, paneId)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        preserveTerminalReconnectContent(newSessionId, reconnectContent);
        updatePaneSession(tabId, paneId, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
          updateAutoIconForSessionStart(pane.connectionId, newSessionId);
        }
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tabId, paneId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        if (pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect pane",
          ids: pane.connectionId
            ? { connection_id: pane.connectionId }
            : undefined,
          error,
        });
        if (pane.connectError) {
          markPaneConnectionFailed(tabId, paneId, errorMessage);
        }
        maybePromptConnectionEdit(pane.connectionId, errorMessage, {
          sourceTabId: tabId,
          sourcePaneId: paneId,
        });
      }
    },
    [
      closePaneBackendSession,
      createSessionForPane,
      hasPane,
      markPaneConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      tabs,
      updateAutoIconForSessionStart,
      updatePaneSession,
    ],
  );

  const handleCloseSession = useCallback(
    async (tab: Tab) => {
      if (tab.locked) {
        notifyLockedTabCloseBlocked();
        return;
      }

      const pane = getActivePane(tab);
      if (!pane) return;

      const closed = await closePaneBackendSession(pane);
      if (!closed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }

      closePane(tab.id, pane.id);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [
      closePane,
      closePaneBackendSession,
      notifyLockedTabCloseBlocked,
      persistWorkspaceNow,
      t,
    ],
  );

  const handleDisconnectSessionById = useCallback(
    async (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;

      if (!tab || !pane) {
        try {
          await attachSessionBeforeClose(sessionId);
          await invoke("close_session", { sessionId });
          clearSessionCommandHistory(sessionId);
        } catch (error) {
          logger.error({
            domain: "session.lifecycle",
            event: "session.close_failed",
            message: "Failed to disconnect session outside workspace",
            ids: { session_id: sessionId },
            error,
          });
          toast.error(t("tabCtx.closeFailed"));
        }
        return;
      }

      const closed = await closePaneBackendSession(pane);
      if (!closed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }

      closePane(tab.id, pane.id);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [closePane, closePaneBackendSession, persistWorkspaceNow, t, tabs],
  );

  const canReconnectSessionById = useCallback(
    (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      return !!pane && !pane.connecting && canCreateSessionFromPane(pane);
    },
    [tabs],
  );

  const handleCloseAllTabs = useCallback(async () => {
    const tabsToClose = tabs.filter((tab) => !tab.locked);
    const skippedLockedCount = tabs.length - tabsToClose.length;
    const results = await Promise.all(
      tabsToClose.map((tab) => closeWorkspaceTabSessions(tab)),
    );
    const successfulTabIds = tabsToClose
      .filter((_, index) => results[index])
      .map((tab) => tab.id);

    if (successfulTabIds.length > 0) {
      closeTabs(successfulTabIds);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    }

    if (skippedLockedCount > 0) {
      toast.info(t("tabCtx.lockedTabsSkipped"));
    }

    if (successfulTabIds.length !== tabsToClose.length) {
      toast.error(t("tabCtx.closeFailed"));
    }
  }, [closeTabs, closeWorkspaceTabSessions, persistWorkspaceNow, t, tabs]);

  const handleCloseInactiveTabs = useCallback(
    async (keepTabId: string) => {
      const leaf = terminalWindows
        ? findTerminalWindowLeafByTabId(terminalWindows, keepTabId)
        : null;
      const targetTabs = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const targetTabsToClose = tabs.filter(
        (tab) => targetTabs.includes(tab.id) && tab.id !== keepTabId,
      );
      const tabsToClose = targetTabsToClose.filter((tab) => !tab.locked);
      const skippedLockedCount = targetTabsToClose.length - tabsToClose.length;
      const results = await Promise.all(
        tabsToClose.map((tab) => closeWorkspaceTabSessions(tab)),
      );

      const successfulTabIds = tabsToClose
        .filter((_, index) => results[index])
        .map((tab) => tab.id);

      if (successfulTabIds.length > 0) {
        closeTabs(successfulTabIds, { nextActiveTabId: keepTabId });
        await persistWorkspaceNow(t("tabCtx.closeFailed"));
      }

      if (!successfulTabIds.length && activeTabId !== keepTabId) {
        setActiveTabId(keepTabId);
      }

      if (skippedLockedCount > 0) {
        toast.info(t("tabCtx.lockedTabsSkipped"));
      }

      if (successfulTabIds.length !== tabsToClose.length) {
        toast.error(t("tabCtx.closeFailed"));
      }
    },
    [
      activeTabId,
      closeTabs,
      closeWorkspaceTabSessions,
      persistWorkspaceNow,
      setActiveTabId,
      t,
      tabs,
      terminalWindows,
    ],
  );

  const handleCloseRightTabs = useCallback(
    async (tabId: string) => {
      const leaf = terminalWindows
        ? findTerminalWindowLeafByTabId(terminalWindows, tabId)
        : null;
      const tabOrder = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const idx = tabOrder.indexOf(tabId);
      if (idx === -1) return;

      const rightTabIds = tabOrder.slice(idx + 1);
      const targetTabsToClose = tabs.filter((tab) =>
        rightTabIds.includes(tab.id),
      );
      const tabsToClose = targetTabsToClose.filter((tab) => !tab.locked);
      const skippedLockedCount = targetTabsToClose.length - tabsToClose.length;
      const results = await Promise.all(
        tabsToClose.map((tab) => closeWorkspaceTabSessions(tab)),
      );

      const successfulTabIds = tabsToClose
        .filter((_, index) => results[index])
        .map((tab) => tab.id);

      if (successfulTabIds.length > 0) {
        closeTabs(successfulTabIds);
        await persistWorkspaceNow(t("tabCtx.closeFailed"));
      }

      if (skippedLockedCount > 0) {
        toast.info(t("tabCtx.lockedTabsSkipped"));
      }

      if (successfulTabIds.length !== tabsToClose.length) {
        toast.error(t("tabCtx.closeFailed"));
      }
    },
    [
      closeTabs,
      closeWorkspaceTabSessions,
      persistWorkspaceNow,
      t,
      tabs,
      terminalWindows,
    ],
  );

  const handleSessionInfo = useCallback((tab: Tab) => {
    const pane = getActivePane(tab);
    if (pane?.connectionId) {
      openNewSession(pane.connectionId);
    }
  }, []);

  const handleOpenChat = useCallback(() => {
    if (!isLocked) {
      updateUi((prev) =>
        buildPanelOpenUpdate(prev, "aiAssistant", multiPanelOpen, "right"),
      );
    }
  }, [isLocked, multiPanelOpen, updateUi]);

  const handleShowAllCommands = useCallback(() => {
    if (!isLocked) {
      updateUi((prev) => ({
        show_quick_cmd_bar: !prev.show_quick_cmd_bar,
        ...(prev.show_serial_send_panel
          ? { show_serial_send_panel: false }
          : {}),
      }));
    }
  }, [isLocked, updateUi]);

  const handleOpenSessionSwitcher = useCallback(() => {
    if (!isLocked) {
      setShowSessionQuickSwitcher(true);
    }
  }, [isLocked]);

  const handleOpenTemporarySshLink = useCallback(() => {
    if (!isLocked) {
      setShowTemporarySshLink(true);
    }
  }, [isLocked]);

  const handleTemporarySshConnect = useCallback(
    async (config: TemporaryLinkConfig) => {
      await connectTemporaryConnection(config);
    },
    [connectTemporaryConnection],
  );

  const buildRecordingFilePath = useCallback(
    async (prefix: "recording" | "session", sessionName: string) => {
      const dir = appSettings.recording.base_path || (await downloadDir());
      const timestamp = new Date()
        .toISOString()
        .replace(/[:.]/g, "-")
        .slice(0, 19);
      return joinPath(
        dir,
        `${prefix}-${safeRecordingName(sessionName)}-${timestamp}.log`,
      );
    },
    [appSettings.recording.base_path],
  );

  const handleToggleSessionRecordingById = useCallback(
    async (sessionId: string, mode: RecordingMode = "transcript") => {
      const isActive = recordingSessions.has(sessionId);

      if (isActive) {
        try {
          const savedPath = await invoke<string>("stop_recording", {
            sessionId,
          });
          await refreshRecordingStatuses();
          toast.success(t("recording.saved", { path: savedPath }));
        } catch (error) {
          logger.error({
            domain: "session.lifecycle",
            event: "recording.stop_failed",
            message: "Failed to stop recording",
            ids: { session_id: sessionId },
            error,
          });
          toast.error(t("recording.stopFailed"));
        }
        return;
      }

      try {
        await invoke<string>("start_recording", {
          request: {
            sessionId,
            mode,
            explicitPath: null,
          },
        });
        await refreshRecordingStatuses();
        toast.success(t("recording.started"));
      } catch (error) {
        logger.error({
          domain: "session.lifecycle",
          event: "recording.start_failed",
          message: "Failed to start recording",
          ids: { session_id: sessionId },
          error,
        });
        toast.error(t("recording.startFailed"));
      }
    },
    [refreshRecordingStatuses, recordingSessions, t],
  );

  const handleToggleSessionRecording = useCallback(
    async (session: SessionInfo, mode: RecordingMode = "transcript") => {
      await handleToggleSessionRecordingById(session.id, mode);
    },
    [handleToggleSessionRecordingById],
  );

  const handleToggleActiveSessionRecording = useCallback(() => {
    if (
      isLocked ||
      !activePane ||
      activePane.connecting ||
      activePane.connectError
    ) {
      return;
    }
    void handleToggleSessionRecordingById(activePane.sessionId, "transcript");
  }, [activePane, handleToggleSessionRecordingById, isLocked]);

  useGlobalShortcuts(
    {
      onNewSession: () => handleNewSession(),
      onTemporarySshLink: handleOpenTemporarySshLink,
      onOpenSessionSwitcher: handleOpenSessionSwitcher,
      onNewLocalTerminal: handleNewLocalTerminal,
      onCloseTab: handleCloseActiveTab,
      onDuplicateSession: handleDuplicateActiveSession,
      onMultiplexSsh: handleMultiplexActiveSshSession,
      onDuplicateSessionWithCommand: handleDuplicateActiveSessionWithCommand,
      onMultiplexSshWithCommand: handleMultiplexActiveSshSessionWithCommand,
      onNextTab: handleNextTab,
      onPrevTab: handlePrevTab,
      onSwitchTab: handleSwitchTab,
      onToggleLeftSidebar: handleToggleLeftSidebar,
      onToggleRightSidebar: handleToggleRightSidebar,
      onZoomIn: handleZoomIn,
      onZoomOut: handleZoomOut,
      onResetZoom: handleResetZoom,
      onOpenSettings: handleOpenSettings,
      onOpenChat: handleOpenChat,
      onShowAllCommands: handleShowAllCommands,
      onLockScreen: handleLockScreen,
      onManageSyncGroups: () => setShowSyncGroupDialog(true),
      onClearTerminal: () =>
        window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
      onToggleRecording: handleToggleActiveSessionRecording,
    },
    appSettings.keybindings,
  );

  const handleSaveSessionTranscript = useCallback(
    async (session: SessionInfo) => {
      try {
        const filePath = await buildRecordingFilePath("session", session.name);
        const savedPath = await invoke<string>("save_session_transcript", {
          sessionId: session.id,
          filePath,
          includeIoLabels: appSettings.recording.include_io_labels,
          includeTimestamps: appSettings.recording.include_timestamps ?? true,
        });
        toast.success(t("recording.transcriptSaved", { path: savedPath }));
      } catch (error) {
        logger.error({
          domain: "session.lifecycle",
          event: "recording.transcript_save_failed",
          message: "Failed to save session transcript",
          ids: { session_id: session.id },
          error,
        });
        toast.error(t("recording.saveFailed"));
      }
    },
    [
      appSettings.recording.include_io_labels,
      appSettings.recording.include_timestamps,
      buildRecordingFilePath,
      t,
    ],
  );

  const handleSaveSessionTranscriptById = useCallback(
    async (sessionId: string, sessionName?: string) => {
      const session = liveSessionsById?.get(sessionId);
      await handleSaveSessionTranscript({
        id: sessionId,
        name: session?.name ?? sessionName ?? sessionId,
        session_type: session?.session_type ?? "Local",
        started_at: session?.started_at ?? new Date().toISOString(),
        connection_id: session?.connection_id ?? null,
        connected: session?.connected ?? true,
        owner_window_label: session?.owner_window_label ?? null,
        ai_execution_profile: session?.ai_execution_profile ?? "auto",
        injection_active: session?.injection_active ?? false,
        remote_file_browser_enabled:
          session?.remote_file_browser_enabled ?? false,
        remote_stats_enabled: session?.remote_stats_enabled ?? false,
        ssh_profile: session?.ssh_profile ?? null,
      });
    },
    [handleSaveSessionTranscript, liveSessionsById],
  );

  // Resize handlers
  const handleLeftResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        left_width: Math.max(
          160,
          Math.min(720, (prev.left_width || 256) + delta),
        ),
      }));
    },
    [updateUi],
  );

  const handleRightResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        right_width: Math.max(
          200,
          Math.min(720, (prev.right_width || 288) - delta),
        ),
      }));
    },
    [updateUi],
  );

  const handleQuickCmdResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        quick_cmd_height: Math.max(
          36,
          Math.min(520, (prev.quick_cmd_height || 180) - delta),
        ),
      }));
    },
    [updateUi],
  );

  const handleSerialSendResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        serial_send_height: Math.max(
          60,
          Math.min(520, (prev.serial_send_height || 180) - delta),
        ),
      }));
    },
    [updateUi],
  );

  const {
    leftTopItems,
    leftBottomItems,
    rightTopItems,
    rightBottomItems,
    showLabels,
    toggleActiveIds,
    handleItemSelect,
    handleReorder,
    handleMoveItem,
    handleToggleLabel,
  } = useActivityBarController({
    uiConfig,
    recordingSessions,
    multiPanelOpen,
    updateUi,
    setIsLocked,
    t,
  });

  // --- Panel content rendering (side-independent) ---

  const activeSessionId =
    activePane && !activePane.connecting && !activePane.connectError
      ? activePane.sessionId
      : null;
  const activeSshSessionId =
    activePane &&
    !activePane.connecting &&
    !activePane.connectError &&
    activePane.type === "SSH"
      ? activePane.sessionId
      : null;
  const activeLiveSshSessionId =
    activeSshSessionId &&
    (liveSessionIds === null || liveSessionIds.has(activeSshSessionId))
      ? activeSshSessionId
      : null;
  const activeLiveSshSessionInfo = activeLiveSshSessionId
    ? liveSessionsById?.get(activeLiveSshSessionId)
    : null;
  const activeStatsSessionId =
    activeLiveSshSessionId &&
    (activeLiveSshSessionInfo?.remote_stats_enabled ?? true)
      ? activeLiveSshSessionId
      : null;
  const activeRemoteStatsEnabled =
    remoteStatsEnabled && Boolean(activeStatsSessionId);
  const remoteStats = useRemoteStats(
    activeLiveSshSessionId,
    activeRemoteStatsEnabled,
    uiConfig.remote_stats_interval ?? 3,
  );
  const headerStatusMode = normalizeHeaderStatusMode(
    uiConfig.header_status_mode,
  );
  const headerStatusVisible = uiConfig.header_status_visible !== false;
  const gpuOverviewEnabled =
    (uiConfig.show_gpu_monitor ?? false) ||
    (headerStatusVisible && headerStatusMode === "gpu");
  const npuOverviewEnabled =
    (uiConfig.show_ascend_npu_monitor ?? false) ||
    (headerStatusVisible && headerStatusMode === "npu");
  const gpuOverviewState = useRemoteGpuOverview(
    activeLiveSshSessionId,
    gpuOverviewEnabled && Boolean(activeStatsSessionId),
    uiConfig.gpu_monitor_interval ?? 3,
  );
  const npuOverviewState = useRemoteNpuOverview(
    activeLiveSshSessionId,
    npuOverviewEnabled && Boolean(activeStatsSessionId),
    uiConfig.ascend_npu_monitor_interval ?? 3,
  );

  useEffect(() => {
    if (!activeStatsSessionId || !remoteStats.stats) return;

    const patch = buildAssetPatchFromRemoteStats(remoteStats.stats);
    if (patch) {
      handleAssetMonitoringPatch(activeStatsSessionId, patch);
    }
  }, [activeStatsSessionId, handleAssetMonitoringPatch, remoteStats.stats]);
  useEffect(() => {
    if (!activeStatsSessionId || !gpuOverviewState.overview) return;

    const patch = buildAssetPatchFromGpuOverview(gpuOverviewState.overview);
    if (patch) {
      handleAssetMonitoringPatch(activeStatsSessionId, patch);
    }
  }, [
    activeStatsSessionId,
    gpuOverviewState.overview,
    handleAssetMonitoringPatch,
  ]);
  useEffect(() => {
    if (!activeStatsSessionId || !npuOverviewState.overview) return;

    const patch = buildAssetPatchFromNpuOverview(npuOverviewState.overview);
    if (patch) {
      handleAssetMonitoringPatch(activeStatsSessionId, patch);
    }
  }, [
    activeStatsSessionId,
    handleAssetMonitoringPatch,
    npuOverviewState.overview,
  ]);

  const activeSerialSessionId =
    activePane &&
    !activePane.connecting &&
    !activePane.connectError &&
    activePane.type === "Serial"
      ? activePane.sessionId
      : null;
  const activeNonSerialSessionId =
    activePane &&
    !activePane.connecting &&
    !activePane.connectError &&
    isNonSerialSessionType(activePane.type)
      ? activePane.sessionId
      : null;
  const activeNonSerialSessionIds = useMemo(
    () => collectActiveNonSerialSessionIds(terminalWindows, tabsById),
    [tabsById, terminalWindows],
  );
  const sendCommandSessionTargets = useMemo(() => {
    if (!terminalWindows) return [];

    const targets: {
      id: string;
      name: string;
      tabName: string;
      type: SessionType;
    }[] = [];
    const seen = new Set<string>();

    const visit = (node: TerminalWindowNode) => {
      if (node.kind === "split") {
        visit(node.first);
        visit(node.second);
        return;
      }

      for (const tabId of node.tabIds) {
        const tab = tabsById.get(tabId);
        if (!tab) continue;

        for (const pane of collectSessionPanes(tab.root)) {
          if (
            pane.paneKind !== "terminal" ||
            !hasLiveSession(pane) ||
            seen.has(pane.sessionId)
          ) {
            continue;
          }
          seen.add(pane.sessionId);
          targets.push({
            id: pane.sessionId,
            name: pane.name,
            tabName: getTabDisplayName(tab),
            type: pane.type,
          });
        }
      }
    };

    visit(terminalWindows);
    return targets;
  }, [tabsById, terminalWindows]);

  const activeBottomPanel = uiConfig.show_serial_send_panel
    ? "serialSend"
    : uiConfig.show_quick_cmd_bar
      ? "quickCmdBar"
      : null;
  const temporarySshShortcut = resolveDisplayKeys(
    "tab.temporarySshLink",
    appSettings.keybindings,
  );
  const openChatShortcut = resolveDisplayKeys(
    "view.openChat",
    appSettings.keybindings,
  );
  const showCommandsShortcut = resolveDisplayKeys(
    "view.showAllCommands",
    appSettings.keybindings,
  );
  const switchTerminalShortcut = resolveDisplayKeys(
    "tab.quickSwitch",
    appSettings.keybindings,
  );

  const quickSwitcherSessions = useMemo<QuickSwitcherSession[]>(() => {
    const connectionsById = new Map(
      savedConnections.map((connection) => [connection.id, connection]),
    );
    const sessions: QuickSwitcherSession[] = [];
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (pane.paneKind !== "terminal") continue;
        const connection = pane.connectionId
          ? connectionsById.get(pane.connectionId)
          : undefined;
        sessions.push({
          id: pane.sessionId,
          name: pane.name,
          sessionType: pane.type,
          connectionName: connection?.name,
          tabName: getTabDisplayName(tab),
          connecting: pane.connecting,
          connectError: pane.connectError,
        });
      }
    }
    return sessions;
  }, [savedConnections, tabs]);

  const handleCloseSessionQuickSwitcher = useCallback(() => {
    setShowSessionQuickSwitcher(false);
    focusTerminalSession(activeSessionId);
  }, [activeSessionId]);

  const handleQuickSwitchSession = useCallback(
    (sessionId: string) => {
      handleSessionClick(sessionId);
      setShowSessionQuickSwitcher(false);
      focusTerminalSession(sessionId);
    },
    [handleSessionClick],
  );

  const handleQuickOpenConnection = useCallback(
    async (connection: SavedConnection) => {
      setShowSessionQuickSwitcher(false);
      await connectSavedConnection(connection, {
        failureContext: "Connection failed from quick switcher",
      });
    },
    [connectSavedConnection],
  );

  const handleQuickSwitcherNewSshSession = useCallback(() => {
    setShowSessionQuickSwitcher(false);
    openNewSession(undefined, true);
  }, []);

  const handleTransferResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        transfer_height: Math.max(
          60,
          Math.min(600, (prev.transfer_height || 180) - delta),
        ),
      }));
    },
    [updateUi],
  );

  const leftPanelIds = useMemo(
    () => getSideOpenPanels(uiConfig, "left", multiPanelOpen),
    [multiPanelOpen, uiConfig],
  );
  const rightPanelIds = useMemo(
    () => getSideOpenPanels(uiConfig, "right", multiPanelOpen),
    [multiPanelOpen, uiConfig],
  );
  const leftOverlayPanelId = useMemo(
    () => getSideOverlayPanel(uiConfig, "left", multiPanelOpen),
    [multiPanelOpen, uiConfig],
  );
  const rightOverlayPanelId = useMemo(
    () => getSideOverlayPanel(uiConfig, "right", multiPanelOpen),
    [multiPanelOpen, uiConfig],
  );
  const leftActiveIds = useMemo(
    () => (multiPanelOpen ? new Set(leftPanelIds) : undefined),
    [leftPanelIds, multiPanelOpen],
  );
  const rightActiveIds = useMemo(
    () => (multiPanelOpen ? new Set(rightPanelIds) : undefined),
    [multiPanelOpen, rightPanelIds],
  );

  // 一键命令面板的运行时上下文：聚合执行命令所需的全部依赖。
  const oneClickRuntime = useMemo<OneClickRuntime>(
    () => ({
      tabs,
      savedConnections,
      // biome-ignore lint/suspicious/noExplicitAny: workspaceTabs.collectSessionPanes 的 root 是递归联合类型
      collectSessionPanes: (root: any) => collectSessionPanes(root),
      sendToCurrent: handleHistoryCommand,
      connectAndRun: (connection, startupCommand) =>
        connectSavedConnection(connection, {
          startupCommand: {
            command: startupCommand.command,
            delayMs: startupCommand.delay_ms ?? 0,
          },
        }),
      incrementUseCount: (id) => {
        invoke("increment_quick_command_use_count", { id }).catch(() => {});
      },
    }),
    [connectSavedConnection, handleHistoryCommand, savedConnections, tabs],
  );

  // When multi-open mode is first enabled, seed the stacks from the active panels.
  useEffect(() => {
    if (!settingsLoaded || !multiPanelOpen) return;
    updateUi((prev) => ({
      ...((prev.left_open_panels?.length ?? 0) === 0 &&
      prev.active_left_panel &&
      isActivityItemVisible(prev.active_left_panel, prev) &&
      !EXCLUSIVE_PANEL_IDS.has(prev.active_left_panel)
        ? { left_open_panels: [prev.active_left_panel] }
        : {}),
      ...((prev.right_open_panels?.length ?? 0) === 0 &&
      prev.active_right_panel &&
      isActivityItemVisible(prev.active_right_panel, prev) &&
      !EXCLUSIVE_PANEL_IDS.has(prev.active_right_panel)
        ? { right_open_panels: [prev.active_right_panel] }
        : {}),
    }));
  }, [multiPanelOpen, settingsLoaded, updateUi]);

  const handlePanelStackResize = useCallback(
    (
      side: "left" | "right",
      aboveId: string,
      belowId: string,
      delta: number,
      containerHeight: number,
    ) => {
      updateUi((prev) => {
        const openIds = getSideOpenPanels(prev, side, true);
        const sizes = prev.panel_stack_sizes ?? {};
        const totalWeight = openIds.reduce(
          (sum, id) => sum + (sizes[id] ?? 1),
          0,
        );
        if (containerHeight <= 0 || totalWeight <= 0 || delta === 0) return {};
        const pxPerWeight = containerHeight / totalWeight;
        const aboveWeight = sizes[aboveId] ?? 1;
        const belowWeight = sizes[belowId] ?? 1;
        const pairWeight = aboveWeight + belowWeight;
        const minWeight = Math.min(pairWeight / 2, 48 / pxPerWeight);
        const nextAbove = Math.max(
          minWeight,
          Math.min(pairWeight - minWeight, aboveWeight + delta / pxPerWeight),
        );
        return {
          panel_stack_sizes: {
            ...sizes,
            [aboveId]: nextAbove,
            [belowId]: pairWeight - nextAbove,
          },
        };
      });
    },
    [updateUi],
  );

  const renderPanelContent = useCallback(
    (panelId: string | null) => (
      <AppPanelContent
        panelId={panelId}
        activePane={activePane}
        activeConnection={activeConnection}
        activeSessionId={activeSessionId}
        activeSshSessionId={activeLiveSshSessionId}
        remoteStatsEnabled={activeRemoteStatsEnabled}
        remoteStats={remoteStats}
        gpuMonitorEnabled={uiConfig.show_gpu_monitor ?? false}
        gpuOverviewState={gpuOverviewState}
        npuMonitorEnabled={uiConfig.show_ascend_npu_monitor ?? false}
        npuOverviewState={npuOverviewState}
        recordingStatuses={recordingStatuses}
        aiIntent={aiIntent}
        transferHeight={uiConfig.transfer_height || 180}
        onTransferResize={handleTransferResize}
        onTemporarySshLink={handleOpenTemporarySshLink}
        onNewConnection={handleNewSession}
        onEditConnection={handleEditConnection}
        onConnectConnection={connectSavedConnection}
        onSessionClick={handleSessionClick}
        onSessionReconnect={handleReconnectSessionById}
        onSessionDisconnect={handleDisconnectSessionById}
        canReconnect={canReconnectSessionById}
        onCommandSend={handleHistoryCommand}
        onToggleSessionRecording={handleToggleSessionRecording}
        onSaveSessionTranscript={handleSaveSessionTranscript}
        oneClickRuntime={oneClickRuntime}
      />
    ),
    [
      activeConnection,
      activeLiveSshSessionId,
      activePane,
      activeSessionId,
      aiIntent,
      activeRemoteStatsEnabled,
      canReconnectSessionById,
      remoteStats,
      gpuOverviewState,
      npuOverviewState,
      handleSaveSessionTranscript,
      handleDisconnectSessionById,
      handleEditConnection,
      handleHistoryCommand,
      handleNewSession,
      handleOpenTemporarySshLink,
      handleReconnectSessionById,
      handleSessionClick,
      handleToggleSessionRecording,
      handleTransferResize,
      connectSavedConnection,
      oneClickRuntime,
      recordingStatuses,
      uiConfig.show_ascend_npu_monitor,
      uiConfig.show_gpu_monitor,
      uiConfig.transfer_height,
    ],
  );

  const handleExternalMatchOpenChange = useCallback((open: boolean) => {
    if (open) return;
    setExternalMatchDialog((current) => {
      current?.resolve({ kind: "cancelled" });
      return null;
    });
  }, []);

  const handleExternalMatchConnection = useCallback(
    (connection: SavedConnection) => {
      setExternalMatchDialog((current) => {
        current?.resolve({ kind: "saved", connection });
        return null;
      });
    },
    [],
  );

  const handleExternalMatchTemporary = useCallback(
    (config: TemporaryLinkConfig) => {
      setExternalMatchDialog((current) => {
        current?.resolve({ kind: "temporary", config });
        return null;
      });
    },
    [],
  );

  const handlePostLoginConfirmOpenChange = useCallback((open: boolean) => {
    if (open) return;
    setPostLoginConfirm((current) => {
      current?.resolve(false);
      return null;
    });
  }, []);

  const handlePostLoginContinue = useCallback(() => {
    setPostLoginConfirm((current) => {
      current?.resolve(true);
      return null;
    });
  }, []);

  const activeSecurityPrompt = securityPromptQueue[0] ?? null;
  const activeHostKeyRequest =
    activeSecurityPrompt?.kind === "host-key"
      ? activeSecurityPrompt.request
      : null;
  const activeSshAgentRequest =
    activeSecurityPrompt?.kind === "ssh-agent"
      ? activeSecurityPrompt.request
      : null;
  const activeOtpRequest =
    activeSecurityPrompt?.kind === "otp" ? activeSecurityPrompt.request : null;
  const activeSshAuthRequest =
    activeSecurityPrompt?.kind === "ssh-auth"
      ? activeSecurityPrompt.request
      : null;
  const removeSecurityPrompt = (requestId: string) => {
    setSecurityPromptQueue((current) =>
      current.filter((item) => item.request.requestId !== requestId),
    );
  };

  return (
    <TransferProvider>
      <AppLayout
        t={t}
        uiConfig={uiConfig}
        appearance={appSettings.appearance}
        header={{
          onNewSession: () => handleNewSession(),
          onAbout: () => setShowAbout(true),
          onCheckForUpdates: () => setShowUpdateDialog(true),
          hasUpdate: updateInfo !== null,
          showUpdateDot: helpDotVisible,
          onHelpMenuOpen: () => setHelpDotVisible(false),
          activeTab,
          savedConnections,
          remoteStatsEnabled: activeRemoteStatsEnabled,
          remoteStats,
          gpuOverviewState,
          npuOverviewState,
          onSmartSplit: handleSmartSplit,
          onUnsplit: handleUnsplit,
          canUnsplit: terminalWindows?.kind === "split",
          onManageSyncGroups: () => setShowSyncGroupDialog(true),
          onBroadcastToAll: () => setBroadcastToAll((prev) => !prev),
          broadcastToAll,
          onOpenCommandPalette: handleOpenSessionSwitcher,
          onClearTerminal: () =>
            window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
          onRefitTerminals: () =>
            window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals")),
          locked: isLocked,
          onRequestQuit: handleRequestQuit,
        }}
        mobile={{
          leftOpen: mobileLeftOpen,
          rightOpen: mobileRightOpen,
          setLeftOpen: setMobileLeftOpen,
          setRightOpen: setMobileRightOpen,
        }}
        leftActivityBar={{
          items: leftTopItems,
          bottomItems: leftBottomItems,
          activeId: uiConfig.active_left_panel,
          activeIds: leftActiveIds,
          activeBottomIds: toggleActiveIds,
          onSelect: handleItemSelect,
          onReorder: (zoneKey, ids) => handleReorder("left", zoneKey, ids),
          onMoveItem: handleMoveItem,
          onToggleLabel: handleToggleLabel,
          showLabels,
        }}
        rightActivityBar={{
          items: rightTopItems,
          bottomItems: rightBottomItems,
          activeId: uiConfig.active_right_panel,
          activeIds: rightActiveIds,
          activeBottomIds: toggleActiveIds,
          onSelect: handleItemSelect,
          onReorder: (zoneKey, ids) => handleReorder("right", zoneKey, ids),
          onMoveItem: handleMoveItem,
          onToggleLabel: handleToggleLabel,
          showLabels,
        }}
        onLeftResize={handleLeftResize}
        onRightResize={handleRightResize}
        panelContent={renderPanelContent}
        leftPanelIds={leftPanelIds}
        rightPanelIds={rightPanelIds}
        leftOverlayPanelId={leftOverlayPanelId}
        rightOverlayPanelId={rightOverlayPanelId}
        panelStackSizes={uiConfig.panel_stack_sizes ?? {}}
        onPanelStackResize={handlePanelStackResize}
        workspace={{
          layout: terminalWindows,
          tabsById,
          focusedTabId: activeTabId,
          unreadTabIds,
          disconnectedTabIds,
          onSelectTab: handleSelectLeafTab,
          onAddTab: handleAddTabFromLeaf,
          onConnectConnection: handleConnectConnectionFromLeaf,
          onTabClose: handleCloseWorkspaceTab,
          onDuplicateSession: handleDuplicateSession,
          onMultiplexSshSession: handleMultiplexSshSession,
          onDuplicateSessionWithCommand: handleDuplicateSessionWithCommand,
          onMultiplexSshSessionWithCommand:
            handleMultiplexSshSessionWithCommand,
          onReconnectSession: handleReconnectSession,
          onDisconnectSession: handleDisconnectSession,
          onSplitSession: handleSplitSession,
          onUnsplit: handleUnsplit,
          onCloseSession: handleCloseSession,
          onCloseAll: handleCloseAllTabs,
          onCloseInactive: handleCloseInactiveTabs,
          onCloseRight: handleCloseRightTabs,
          onSessionInfo: handleSessionInfo,
          onReorderTabs: handleReorderTabsInLeaf,
          onMoveTabToLeaf: handleMoveTabToLeaf,
          onSplitTabToLeaf: handleSplitTabToLeaf,
          onActivatePane: handleActivatePane,
          onUpdatePaneSplitRatio: handleUpdatePaneSplitRatio,
          onUpdateWindowSplitRatio: handleUpdateWindowSplitRatio,
          onReconnectPane: handleReconnectPane,
          onReconnected: handleReconnected,
          onDisconnectedCloseRequested: handleCloseDisconnectedPane,
          onConnectionError: handleConnectionError,
          recordingStatuses,
          onToggleSessionRecording: handleToggleSessionRecordingById,
          onSaveSessionTranscript: handleSaveSessionTranscriptById,
        }}
        tabsCount={tabs.length}
        emptyWorkspace={{
          temporarySshShortcut,
          openChatShortcut,
          showCommandsShortcut,
          switchTerminalShortcut,
          onTemporarySshLink: handleOpenTemporarySshLink,
          onOpenChat: handleOpenChat,
          onShowCommands: handleShowAllCommands,
          onSwitchTerminal: handleOpenSessionSwitcher,
          onConnectConnection: connectSavedConnection,
          onEditConnection: handleEditConnection,
        }}
        bottomPanel={{
          activePanel: activeBottomPanel,
          quickCmdHeight: uiConfig.quick_cmd_height || 180,
          serialSendHeight: uiConfig.serial_send_height || 180,
          activeSerialSessionId,
          activeNonSerialSessionId,
          activeNonSerialSessionIds,
          syncGroups,
          sessionTargets: sendCommandSessionTargets,
          sendCommandDraft,
          onSendCommandDraftConsumed: handleSendCommandDraftConsumed,
          onQuickCmdResize: handleQuickCmdResize,
          onSerialSendResize: handleSerialSendResize,
          onCommandSend: handleHistoryCommand,
          onSendToAllSessions: handleSendToAllSessions,
        }}
        dialogs={{
          aboutOpen: showAbout,
          onAboutOpenChange: setShowAbout,
          syncGroupOpen: showSyncGroupDialog,
          onSyncGroupOpenChange: setShowSyncGroupDialog,
          updateOpen: showUpdateDialog,
          onUpdateOpenChange: setShowUpdateDialog,
          onUpdateFound: setUpdateInfo,
          quitConfirmOpen: showQuitConfirm,
          onQuitConfirmOpenChange: setShowQuitConfirm,
          onQuitConfirm: handleQuitApplication,
          otpRequest: activeOtpRequest,
          onOtpDone: removeSecurityPrompt,
          sshAuthRequest: activeSshAuthRequest,
          onSshAuthDone: removeSecurityPrompt,
          sshAgentAuthRequest: activeSshAgentRequest,
          onSshAgentAuthDone: removeSecurityPrompt,
          dockerSudoPasswordRequest,
          onDockerSudoPasswordDone: (requestId) =>
            setDockerSudoPasswordRequest((current) =>
              current?.requestId === requestId ? null : current,
            ),
          hostKeyVerifyRequest: activeHostKeyRequest,
          onHostKeyVerifyDone: removeSecurityPrompt,
          rdpCertificateVerifyRequest: rdpCertificateRequests[0] ?? null,
          onRdpCertificateVerifyDone: (requestId) =>
            setRdpCertificateRequests((current) =>
              current.filter((item) => item.requestId !== requestId),
            ),
          modalChildWindowCount,
          locked: isLocked,
          hasMasterPassword: !!appSettings.security.master_password,
          onUnlock: () => setIsLocked(false),
          onRequestClose: handleRequestWindowClose,
        }}
      />
      <SessionQuickSwitcher
        open={showSessionQuickSwitcher}
        activeSessionId={activePane?.sessionId ?? null}
        workspaceSessions={quickSwitcherSessions}
        savedConnections={savedConnections}
        onClose={handleCloseSessionQuickSwitcher}
        onSelectSession={handleQuickSwitchSession}
        onOpenConnection={handleQuickOpenConnection}
        onNewSshSession={handleQuickSwitcherNewSshSession}
      />
      <TemporarySshLinkDialog
        open={showTemporarySshLink}
        onOpenChange={setShowTemporarySshLink}
        onConnect={handleTemporarySshConnect}
      />
      <ExternalConnectionMatchDialog
        open={externalMatchDialog !== null}
        connections={externalMatchDialog?.connections ?? []}
        groups={savedGroups}
        temporary={externalMatchDialog?.temporary ?? null}
        onOpenChange={handleExternalMatchOpenChange}
        onSelectConnection={handleExternalMatchConnection}
        onUseTemporary={handleExternalMatchTemporary}
      />
      <AlertDialog
        open={postLoginConfirm !== null}
        onOpenChange={handlePostLoginConfirmOpenChange}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("externalOpen.postLoginConfirmTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("externalOpen.postLoginConfirmDescription", {
                name: postLoginConfirm?.connection.name ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <pre className="max-h-40 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs whitespace-pre-wrap">
            {postLoginConfirm?.command ?? ""}
          </pre>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handlePostLoginContinue}>
              {t("externalOpen.continueConnection")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </TransferProvider>
  );
}

export default App;

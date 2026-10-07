import { supports } from "@/lib/backend/runtime";
import { listen } from "@/lib/backend/api";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import AppLayout from "./components/app/AppLayout";
import AppPanelContent from "./components/app/AppPanelContent";
import ActivityBarResetDialog from "./components/dialog/app/ActivityBarResetDialog";
import AppOverlayDialogs from "./components/dialog/app/AppOverlayDialogs";
import { McpApprovalHost } from "./components/dialog/app/McpApprovalHost";
import type { VncServerKeyVerifyRequest } from "./components/dialog/connections/VncServerKeyVerifyDialog";
import type { RdpCertificateVerifyRequest } from "./components/dialog/connections/RdpCertificateVerifyDialog";
import type { DockerSudoPasswordRequest } from "./components/dialog/docker/DockerSudoPasswordDialog";
import type { QuickSwitcherSession } from "./components/dialog/terminal/SessionQuickSwitcherDialog";
import { useApp } from "./context/AppContext";
import { usePlugins } from "./context/PluginContext";
import { activeManifest, parsePluginPanelId } from "./lib/plugins";
import { TransferProvider } from "./context/TransferContext";
import { useActivityBarController } from "./hooks/useActivityBarController";
import { useAppWindowEvents } from "./hooks/useAppWindowEvents";
import { useAssetMonitoringCache } from "./hooks/useAssetMonitoringCache";
import { useConnectAfterEdit } from "./hooks/useConnectAfterEdit";
import { type ExternalOpenRequest, useExternalOpenRequests } from "./hooks/useExternalOpenRequests";
import { useFileDocumentCloseGuard } from "./hooks/useFileDocumentCloseGuard";
import { useFileEditorZoom } from "./hooks/useFileEditorZoom";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { useMacSelectionGuard } from "./hooks/useMacSelectionGuard";
import { useMcpActiveSession } from "./hooks/useMcpActiveSession";
import { useModalChildWindows } from "./hooks/useModalChildWindows";
import { useSecurityPromptQueue } from "./hooks/useSecurityPromptQueue";
import { useSessionRecordingActions } from "./hooks/useSessionRecordingActions";
import { useSessionRuntimeState } from "./hooks/useSessionRuntimeState";
import { resolveDisplayKeys } from "./hooks/useShortcutMap";
import { useTerminalWindowLayout } from "./hooks/useTerminalWindowLayout";
import { useTerminalZoom } from "./hooks/useTerminalZoom";
import { useTabStatusIndicators } from "./hooks/useUnreadTabs";
import { useWorkspaceMonitoring } from "./hooks/useWorkspaceMonitoring";
import { AI_OPEN_EVENT, type AIOpenIntent } from "./lib/aiEvents";
import type {
  ExternalConnectionChoice,
  ExternalMatchDialogState,
  PostLoginConfirmState,
} from "./lib/appExternalDialogs";
import {
  attachSessionBeforeClose,
  buildStartupCommandPayload,
  capturePaneReconnectContent,
  closeStaleCreatedSession,
  createExternalLocalSession,
  createSessionForConnection,
  createSessionForPane,
  createTemporarySession,
  focusTerminalSession,
  getConnectionSessionType,
  getRemoteDesktopPaneDisplay,
  getTemporaryLinkSessionType,
  isSessionCreationCancelled,
  launchSavedRdpWithSystemClient,
  type StartupCommandRequest,
  sendStartupCommandToSession,
} from "./lib/appSessionFactory";
import {
  buildPanelOpenUpdate,
  canMultiplexSshPane,
  canCreateSessionFromPane,
  canUseStartupCommandForPane,
  canUseFloatingPanel,
  clearUnavailableFloatingPanels,
  collectActiveNonSerialSessionIds,
  EXCLUSIVE_PANEL_IDS,
  type FloatingPanelsState,
  getItemSide,
  getSideOpenPanels,
  getSideOverlayPanel,
  getVisibleActivityIds,
  hasLiveSession,
  isActivityItemAvailable,
  isNonSerialSessionType,
  moveFloatingPanelSide,
  NON_PANEL_IDS,
  normalizePanelOpenMode,
  type PanelOpenMode,
  reduceFloatingPanelSelect,
  type TrayAction,
} from "./lib/appWorkspace";
import { collectFileDocumentPaneIds, removePaneFromTabs } from "./lib/appWorkspaceClose";
import { updateConnectionAutoIconAfterSessionStart } from "./lib/connectionAutoIcon";
import { getDynamicTitle, startDynamicTitles, useDynamicTitles } from "./lib/dynamicTabTitles";
import { getErrorMessage, shouldPromptConnectionEditOnFailure } from "./lib/errors";
import {
  type ExternalConnectionResolution,
  findExternalConnectionMatches,
  parseExternalOpenUrl,
} from "./lib/externalOpen";
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
import { getSessionInputPeerIds, purgeSessionFromGroups } from "./lib/syncInputGroups";
import {
  findTerminalWindowLeafById,
  findTerminalWindowLeafByTabId,
  insertTabAfterInLeaf,
  insertTabIntoLeaf,
  setLeafActiveTab,
  splitTerminalWindowForTab,
  type TerminalWindowNode,
} from "./lib/tabWindows";
import type { TemporaryLinkConfig } from "./lib/temporaryLink";
import { preserveTerminalReconnectContent } from "./lib/terminalReconnectHistory";
import {
  buildDirectoryChangeCommand,
  buildReconnectCwdStartupCommand,
  carryOverSessionCwd,
  isTerminalDirectoryPath,
} from "./lib/terminalSessionCwd";
import { checkForUpdate, type UpdateInfo } from "./lib/updater";
import {
  eventTargetsCurrentWindow,
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
  findSessionPaneBySessionId,
  findTabBySessionId,
  getActivePane,
  getActiveSessionTabDisplayName,
  getReleasedSessionIds,
} from "./lib/workspaceTabs";
import type {
  McpSessionOpenCancel,
  McpSessionOpenRequest,
  PaneSplitDirection,
  SavedConnection,
  SessionInfo,
  SessionPane,
  SessionType,
  SshRuntimeMode,
  Tab,
} from "./types/global";

function isSftpOnlyPane(
  pane: SessionPane | null | undefined,
  sessionsById: Map<string, SessionInfo> | null | undefined,
) {
  return (
    pane?.paneKind === "terminal" &&
    pane.type === "SSH" &&
    (pane.sshRuntimeMode === "sftp" ||
      sessionsById?.get(pane.sessionId)?.ssh_runtime_mode === "sftp")
  );
}

function isSftpOnlySession(
  sessionId: string,
  sessionsById: Map<string, SessionInfo> | null | undefined,
) {
  return sessionsById?.get(sessionId)?.ssh_runtime_mode === "sftp";
}

/** Root layout: header, activity bars, sidebars, terminal area, dialogs. */
function App() {
  const { plugins } = usePlugins();
  useMacSelectionGuard();
  // Keep dynamic session titles (local PTY shell integration) flowing for the
  // main window's tab labels and window title.
  const dynamicTitles = useDynamicTitles();
  useEffect(() => {
    startDynamicTitles();
  }, []);

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
    replaceSessionReferences,
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
  const remoteStatsEnabled =
    supports("remoteMonitoring") && (uiConfig.show_remote_stats ?? true);
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
  const panelOpenMode = normalizePanelOpenMode(uiConfig.panel_open_mode);
  const { t, i18n } = useTranslation();

  useEffect(() => {
    if (!settingsLoaded) return;
    import("@/lib/backend/platform/window").then(({ getCurrentWindow }) => {
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
  const [paneFocusMode, setPaneFocusMode] = useState(false);
  const [nativeFullscreen, setNativeFullscreen] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [showUpdateDialog, setShowUpdateDialog] = useState(false);
  const [showSyncGroupDialog, setShowSyncGroupDialog] = useState(false);
  const [showQuitConfirm, setShowQuitConfirm] = useState(false);
  const [showActivityBarResetConfirm, setShowActivityBarResetConfirm] = useState(false);
  const [floatingPanels, setFloatingPanels] = useState<FloatingPanelsState>({
    left: null,
    right: null,
  });
  const [lastFloatingSide, setLastFloatingSide] = useState<"left" | "right" | null>(null);
  const {
    pendingFileDocumentClose,
    savingFileDocuments,
    requestFileDocumentClose,
    handleSaveFileDocumentsAndClose,
    handleDiscardFileDocumentsAndClose,
    handlePendingFileDocumentCloseOpenChange,
  } = useFileDocumentCloseGuard();
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [helpDotVisible, setHelpDotVisible] = useState(false);
  const [sendCommandDraft, setSendCommandDraft] = useState<SendCommandPanelDraft | null>(null);
  const [showSessionQuickSwitcher, setShowSessionQuickSwitcher] = useState(false);
  const [showTemporarySshLink, setShowTemporarySshLink] = useState(false);
  const [externalMatchDialog, setExternalMatchDialog] = useState<ExternalMatchDialogState | null>(
    null,
  );
  const [postLoginConfirm, setPostLoginConfirm] = useState<PostLoginConfirmState | null>(null);
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

  const {
    recordingStatuses,
    recordingSessions,
    liveSessionIds,
    liveSessionsById,
    refreshRecordingStatuses,
  } = useSessionRuntimeState(appSettings.recording.memory_limit_bytes, settingsLoaded);
  const {
    activeHostKeyRequest,
    activeSshAgentRequest,
    activeOtpRequest,
    activeSshAuthRequest,
    queueSecurityPrompt,
    removeSecurityPrompt,
  } = useSecurityPromptQueue();
  const [dockerSudoPasswordRequest, setDockerSudoPasswordRequest] =
    useState<DockerSudoPasswordRequest | null>(null);
  const [rdpCertificateRequests, setRdpCertificateRequests] = useState<
    RdpCertificateVerifyRequest[]
  >([]);
  const [vncServerKeyRequests, setVncServerKeyRequests] = useState<VncServerKeyVerifyRequest[]>([]);
  const modalChildWindowCount = useModalChildWindows();

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_idle_lock ? appSettings.security.idle_lock_minutes : 0,
    isLocked,
    () => setIsLocked(true),
  );

  // Background update check on startup
  useEffect(() => {
    if (!runtimeInfoLoaded || runtimeInfo.packageManager) return;

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
  }, [portable, runtimeInfo.packageManager, runtimeInfoLoaded]);

  const {
    terminalWindows,
    setTerminalWindows,
    handleSelectLeafTab,
    handleReorderTabsInLeaf,
    handleMoveTabToLeaf,
    handleSplitTabToLeaf,
    handleUnsplit,
    handleUpdateWindowSplitRatio,
    handleActivatePane,
    handleUpdatePaneSplitRatio,
    persistWorkspaceLayoutNow,
  } = useTerminalWindowLayout({
    tabs,
    activeTabId,
    setActiveTabId,
    setActivePane,
    updateSplitRatio,
    updateUi,
    persistTabsNow,
    settingsLoaded,
    startupRestoreComplete,
    general: appSettings.general,
    savedLayout: uiConfig.terminal_window_layout,
  });

  const closeFloatingPanel = useCallback(
    (side: "left" | "right") => {
      if (!floatingPanels[side]) return;
      const next = { ...floatingPanels, [side]: null };
      setFloatingPanels(next);
      if (lastFloatingSide !== side) return;
      setLastFloatingSide(
        side === "left"
          ? next.right
            ? "right"
            : null
          : next.left
            ? "left"
            : null,
      );
    },
    [floatingPanels, lastFloatingSide],
  );

  const handleFloatingPanelSelect = useCallback(
    (panelId: string, side: "left" | "right") => {
      const next = reduceFloatingPanelSelect(floatingPanels, panelId, side);
      const otherSide = side === "left" ? "right" : "left";
      setFloatingPanels(next);
      setLastFloatingSide(next[side] ? side : next[otherSide] ? otherSide : null);
    },
    [floatingPanels],
  );

  const handleFloatingPanelMove = useCallback(
    (panelId: string, targetSide: "left" | "right") => {
      const next = moveFloatingPanelSide(floatingPanels, panelId, targetSide);
      if (next === floatingPanels) return;
      setFloatingPanels(next);
      setLastFloatingSide(targetSide);
    },
    [floatingPanels],
  );

  const handlePanelOpenModeChange = useCallback(
    (mode: PanelOpenMode) => {
      const nextMode = normalizePanelOpenMode(mode);
      setFloatingPanels({ left: null, right: null });
      setLastFloatingSide(null);
      updateUi(
        nextMode === "floating"
          ? {
              active_left_panel: null,
              active_right_panel: null,
              left_open_panels: [],
              right_open_panels: [],
              panel_open_mode: "floating",
            }
          : { panel_open_mode: "docked" },
      );
    },
    [updateUi],
  );

  const openPanel = useCallback(
    (panelId: string, fallbackSide: "left" | "right" = "left") => {
      const side = getItemSide(panelId, uiConfig.activity_bar_layout) ?? fallbackSide;
      if (panelOpenMode === "floating" && canUseFloatingPanel(panelId)) {
        handleFloatingPanelSelect(panelId, side);
        return;
      }
      updateUi((prev) => buildPanelOpenUpdate(prev, panelId, multiPanelOpen, fallbackSide));
    },
    [
      handleFloatingPanelSelect,
      multiPanelOpen,
      panelOpenMode,
      uiConfig.activity_bar_layout,
      updateUi,
    ],
  );

  const handleOpenPanel = useCallback(
    (panelId: "activeSessions" | "syncBackupHistory") => {
      openPanel(panelId);
    },
    [openPanel],
  );

  useEffect(() => {
    if (!settingsLoaded || panelOpenMode !== "floating") return;
    updateUi((prev) => {
      if (
        !prev.active_left_panel &&
        !prev.active_right_panel &&
        (prev.left_open_panels?.length ?? 0) === 0 &&
        (prev.right_open_panels?.length ?? 0) === 0
      ) {
        return {};
      }
      return {
        active_left_panel: null,
        active_right_panel: null,
        left_open_panels: [],
        right_open_panels: [],
      };
    });
  }, [panelOpenMode, settingsLoaded, updateUi]);

  const handleConnectAfterEdit = useConnectAfterEdit({
    tabs,
    addPendingTab,
    hasPane,
    hasTab,
    markPaneConnecting,
    markPaneConnectionFailed,
    markTabConnectionFailed,
    recordRecentConnection,
    setActivePane,
    setActiveTabId,
    updatePaneSession,
    updateTabSession,
    rdpClientMode: appSettings.general.rdp_client_mode,
    setTerminalWindows,
    updateAutoIconForSessionStart,
  });
  useAppWindowEvents({
    addTab,
    replaceAppSettings,
    setTerminalWindows,
    queueSecurityPrompt,
    removeSecurityPrompt,
    setDockerSudoPasswordRequest,
    setRdpCertificateRequests,
    setVncServerKeyRequests,
    handleConnectAfterEdit,
    handleOpenPanel,
  });

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
  const activeTabName = activeTab
    ? getActiveSessionTabDisplayName(activeTab, getDynamicTitle).trim()
    : "";
  const windowTitle = activeTabName ? `${activeTabName} - NyaTerm` : "NyaTerm";
  const activePane = activeTab ? getActivePane(activeTab) : null;
  const activeSessionId =
    activePane &&
    activePane.paneKind === "terminal" &&
    !activePane.connecting &&
    !activePane.connectError
      ? activePane.sessionId
      : null;
  const activeConnection = activePane?.connectionId
    ? (savedConnections.find((connection) => connection.id === activePane.connectionId) ?? null)
    : null;

  useEffect(() => {
    if (paneFocusMode && !activePane) {
      setPaneFocusMode(false);
    }
  }, [activePane, paneFocusMode]);

  useEffect(() => {
    if (!paneFocusMode) return;
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      event.stopPropagation();
      setPaneFocusMode(false);
    };
    window.addEventListener("keydown", handleEscape, true);
    return () => window.removeEventListener("keydown", handleEscape, true);
  }, [paneFocusMode]);

  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      window.dispatchEvent(
        new CustomEvent("nyaterm:refresh-terminals", {
          detail: { nativeFullscreen, paneFocusMode },
        }),
      );
    });
    return () => cancelAnimationFrame(frame);
  }, [paneFocusMode, nativeFullscreen]);

  useEffect(() => {
    let disposed = false;
    let unlistenResize: (() => void) | undefined;
    let unlistenFocus: (() => void) | undefined;
    void import("@/lib/backend/platform/window").then(
      async ({ getCurrentWindow }) => {
        if (disposed) return;
        const currentWindow = getCurrentWindow();
        const syncFullscreen = async () => {
          const fullscreen = await currentWindow
            .isFullscreen()
            .catch(() => false);
          if (!disposed) setNativeFullscreen(fullscreen);
        };
        await syncFullscreen();
        unlistenResize = await currentWindow.onResized(
          () => void syncFullscreen(),
        );
        unlistenFocus = await currentWindow.onFocusChanged(
          () => void syncFullscreen(),
        );
      },
    );
    return () => {
      disposed = true;
      unlistenResize?.();
      unlistenFocus?.();
    };
  }, []);
  const [aiIntent, setAiIntent] = useState<AIOpenIntent | null>(null);
  const { handleAssetMonitoringPatch, flushAssetMonitoringCache } = useAssetMonitoringCache({
    tabs,
    savedConnections,
    liveSessionIds,
  });
  const tabsById = useMemo(() => new Map(tabs.map((tab) => [tab.id, tab])), [tabs]);
  useEffect(() => {
    let cancelled = false;

    import("@/lib/backend/platform/window")
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
    (conn: SavedConnection, autoConnect?: boolean, target?: NewSessionTarget) => {
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
      const connection = savedConnections.find((item) => item.id === connectionId);
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
        runtimeModeOverride?: SshRuntimeMode;
        propagateError?: boolean;
        onPending?: (pending: {
          tabId: string;
          createRequestId: string;
        }) => void;
        onSuccess?: (sessionId: string) => void;
      },
    ) => {
      try {
        if (
          await launchSavedRdpWithSystemClient(
            connection,
            appSettings.general.rdp_client_mode,
          )
        ) {
          recordRecentConnection(connection.id);
          return;
        }
      } catch (error) {
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "connection.external_rdp_open_failed",
          message:
            options?.failureContext ?? "Windows Remote Desktop launch failed",
          ids: { connection_id: connection.id },
          error,
        });
        toast.error(
          t("savedConnections.connectionFailed", { error: errorMessage }),
        );
        if (options?.propagateError) throw error;
        return;
      }

      const pending = addPendingTab(
        connection.name,
        getConnectionSessionType(connection),
        connection.id,
        undefined,
        undefined,
        {
          display: getRemoteDesktopPaneDisplay(connection),
          sshRuntimeMode: options?.runtimeModeOverride,
        },
      );
      const { tabId, createRequestId } = pending;
      options?.onPending?.({ tabId, createRequestId });

      try {
        const sessionId = await createSessionForConnection(
          connection,
          createRequestId,
          undefined,
          options?.runtimeModeOverride,
          pending.paneId,
        );
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        focusTerminalSession(sessionId);
        recordRecentConnection(connection.id);
        updateAutoIconForSessionStart(connection.id, sessionId);
        options?.onSuccess?.(sessionId);
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
          if (options?.propagateError) throw error;
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
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
        if (options?.propagateError) throw error;
      }
    },
    [
      addPendingTab,
      appSettings.general.rdp_client_mode,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
  );

  const openSavedConnectionWithSftp = useCallback(
    (connection: SavedConnection) =>
      connectSavedConnection(connection, {
        runtimeModeOverride: "sftp",
        failureContext: "SFTP-only connection failed",
      }),
    [connectSavedConnection],
  );

  const mcpSessionOpenRequestsRef = useRef(
    new Map<string, { tabId: string; createRequestId: string }>(),
  );
  const cancelledMcpSessionOpenRequestsRef = useRef(new Set<string>());
  useEffect(() => {
    let disposed = false;
    let unlistenOpen: (() => void) | undefined;
    let unlistenCancel: (() => void) | undefined;

    void listen<McpSessionOpenRequest>(
      "mcp-session-open-request",
      ({ payload }) => {
        if (disposed || !eventTargetsCurrentWindow(payload.targetWindowLabel))
          return;
        void (async () => {
          const connections = savedConnections.some(
            (item) => item.id === payload.connectionId,
          )
            ? savedConnections
            : await invoke<SavedConnection[]>("get_saved_connections");
          if (
            cancelledMcpSessionOpenRequestsRef.current.delete(payload.requestId)
          )
            return;
          const connection = connections.find(
            (item) => item.id === payload.connectionId,
          );
          if (
            !connection ||
            connection.type === "rdp" ||
            connection.type === "vnc"
          ) {
            await invoke("respond_mcp_session_open", {
              requestId: payload.requestId,
              sessionId: null,
              error:
                "The saved connection does not exist or is not a supported terminal connection.",
            });
            return;
          }

          let openedSessionId: string | null = null;
          await connectSavedConnection(connection, {
            failureContext: "MCP session open failed",
            propagateError: true,
            onPending: (pending) => {
              mcpSessionOpenRequestsRef.current.set(payload.requestId, pending);
            },
            onSuccess: (sessionId) => {
              openedSessionId = sessionId;
            },
          });
          await invoke("respond_mcp_session_open", {
            requestId: payload.requestId,
            sessionId: openedSessionId,
            error: openedSessionId
              ? null
              : "The MCP session-open request did not create a session.",
          });
        })()
          .catch((error) => {
            void invoke("respond_mcp_session_open", {
              requestId: payload.requestId,
              sessionId: null,
              error: getErrorMessage(error),
            }).catch(() => {});
          })
          .finally(() => {
            mcpSessionOpenRequestsRef.current.delete(payload.requestId);
            cancelledMcpSessionOpenRequestsRef.current.delete(
              payload.requestId,
            );
          });
      },
    ).then((dispose) => {
      if (disposed) dispose();
      else unlistenOpen = dispose;
    });

    void listen<McpSessionOpenCancel>("mcp-session-open-cancel", ({ payload }) => {
      if (disposed || !eventTargetsCurrentWindow(payload.targetWindowLabel)) return;
      const pending = mcpSessionOpenRequestsRef.current.get(payload.requestId);
      if (!pending) {
        cancelledMcpSessionOpenRequestsRef.current.add(payload.requestId);
        return;
      }
      mcpSessionOpenRequestsRef.current.delete(payload.requestId);
      closeTabs([pending.tabId]);
      void invoke("cancel_session_creation", {
        createRequestId: pending.createRequestId,
      }).catch(() => {});
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenCancel = dispose;
    });

    return () => {
      disposed = true;
      unlistenOpen?.();
      unlistenCancel?.();
    };
  }, [closeTabs, connectSavedConnection, savedConnections]);

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
        const sessionId = await createTemporarySession(
          config,
          createRequestId,
          undefined,
          pending.paneId,
        );
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
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
      }
    },
    [addPendingTab, hasTab, markTabConnectionFailed, t, updateTabSession],
  );

  const connectExternalLocalSession = useCallback(
    async (workingDir: string | null) => {
      const pending = addPendingTab(t("menu.newLocalTerminal"), "Local", undefined);
      const { tabId, createRequestId } = pending;

      try {
        const sessionId = await createExternalLocalSession(
          workingDir,
          createRequestId,
          pending.paneId,
        );
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
          event: "external_local.open_failed",
          message: "External local terminal failed",
          error,
        });
        markTabConnectionFailed(tabId, errorMessage);
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
      }
    },
    [addPendingTab, hasTab, markTabConnectionFailed, t, updateTabSession],
  );

  const chooseExternalConnection = useCallback(
    (resolution: Extract<ExternalConnectionResolution, { kind: "ambiguous" }>) =>
      new Promise<ExternalConnectionChoice>((resolve) => {
        setExternalMatchDialog({
          connections: resolution.connections,
          temporary: resolution.temporary,
          runtimeModeOverride: resolution.runtimeModeOverride,
          resolve,
        });
      }),
    [],
  );

  const confirmExternalPostLogin = useCallback((connection: SavedConnection) => {
    const command = connection.post_login?.command?.trim() ?? "";
    if (!connection.post_login?.enabled || !command) return Promise.resolve(true);

    return new Promise<boolean>((resolve) => {
      setPostLoginConfirm({ connection, command, resolve });
    });
  }, []);

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

      if (parsed.intent.protocol === "local") {
        logger.info({
          domain: "app.lifecycle",
          event: "external_open.local_terminal_created",
          message: "External open will create a local terminal",
          ids: { request_id: request.id },
          data: {
            scheme: parsed.intent.protocol,
            source: request.source,
            target_window_label: request.targetWindowLabel,
            has_working_dir: parsed.intent.workingDir !== null,
          },
        });
        await connectExternalLocalSession(parsed.intent.workingDir);
        return;
      }

      const latestConnections = await invoke<SavedConnection[]>("get_saved_connections");
      const resolution = findExternalConnectionMatches(latestConnections, parsed.intent);

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
            runtimeModeOverride: resolution.runtimeModeOverride,
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
              runtimeModeOverride: choice.runtimeModeOverride,
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
      connectExternalLocalSession,
      connectSavedConnection,
      connectTemporaryConnection,
      t,
    ],
  );

  useExternalOpenRequests({
    ready: settingsLoaded && startupRestoreComplete && !isLocked,
    onRequest: handleExternalOpenRequest,
  });

  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIOpenIntent>).detail;
      if (!detail) return;
      setAiIntent(detail);
      openPanel("aiAssistant", "right");
    };
    window.addEventListener(AI_OPEN_EVENT, handler);
    return () => window.removeEventListener(AI_OPEN_EVENT, handler);
  }, [openPanel]);

  const { unreadTabIds, disconnectedTabIds } = useTabStatusIndicators(tabs, activeTabId);

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
          targetLeaf?.activeTabId ?? targetLeaf?.tabIds[targetLeaf.tabIds.length - 1] ?? null,
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
        targetLeaf?.activeTabId ?? targetLeaf?.tabIds[targetLeaf.tabIds.length - 1] ?? null;

      try {
        if (
          await launchSavedRdpWithSystemClient(
            connection,
            appSettings.general.rdp_client_mode,
          )
        ) {
          recordRecentConnection(connection.id);
          return;
        }
      } catch (error) {
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "connection.external_rdp_open_failed",
          message: "Windows Remote Desktop launch failed from tab menu",
          ids: { connection_id: connection.id },
          error,
        });
        toast.error(
          t("savedConnections.connectionFailed", { error: errorMessage }),
        );
        return;
      }

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
          undefined,
          undefined,
          pending.paneId,
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
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
      }
    },
    [
      addPendingTab,
      appSettings.general.rdp_client_mode,
      hasTab,
      handleSelectLeafTab,
      setTerminalWindows,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      terminalWindows,
      updateAutoIconForSessionStart,
      updateTabSession,
    ],
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

  const closeReleasedSessions = useCallback(
    async (previousTabs: Tab[], nextTabs: Tab[]) => {
      const retainedPaneIds = new Set(
        nextTabs.flatMap((tab) => collectSessionPanes(tab.root).map((pane) => pane.id)),
      );
      const releasedPanes = previousTabs
        .flatMap((tab) => collectSessionPanes(tab.root))
        .filter((pane) => !retainedPaneIds.has(pane.id));
      const releasedSessionIds = getReleasedSessionIds(previousTabs, nextTabs);
      const results = await Promise.all(
        releasedSessionIds.map((sessionId) => {
          const pane = previousTabs
            .flatMap((tab) => collectSessionPanes(tab.root))
            .find((candidate) => candidate.sessionId === sessionId);
          return pane ? closePaneBackendSession(pane) : Promise.resolve(true);
        }),
      );
      if (!results.every(Boolean)) return false;
      await Promise.all(
        releasedPanes
          .filter((pane) => pane.paneKind !== "file")
          .map((pane) => invoke("finish_recording_scope", { scopeId: pane.id })),
      );
      return true;
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

  const executeCloseTabs = useCallback(
    async (tabIds: string[], options?: { nextActiveTabId?: string | null }) => {
      const targetIds = new Set(tabIds);
      const nextTabs = tabs.filter((tab) => !targetIds.has(tab.id));
      const allClosed = await closeReleasedSessions(tabs, nextTabs);
      if (!allClosed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }
      closeTabs(tabIds, options);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [closeReleasedSessions, closeTabs, persistWorkspaceNow, t, tabs],
  );

  const requestCloseTabs = useCallback(
    async (tabsToClose: Tab[], options?: { nextActiveTabId?: string | null }) => {
      const tabIds = tabsToClose.map((tab) => tab.id);
      const paneIds = collectFileDocumentPaneIds(tabsToClose);
      await requestFileDocumentClose(paneIds, () => executeCloseTabs(tabIds, options));
    },
    [executeCloseTabs, requestFileDocumentClose],
  );

  const executeClosePane = useCallback(
    async (tabId: string, paneId: string) => {
      const nextTabs = removePaneFromTabs(tabs, tabId, paneId);
      const allClosed = await closeReleasedSessions(tabs, nextTabs);
      if (!allClosed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }
      closePane(tabId, paneId);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [closePane, closeReleasedSessions, persistWorkspaceNow, t, tabs],
  );

  const requestClosePane = useCallback(
    async (tab: Tab, pane: SessionPane) => {
      const paneIds = pane.paneKind === "file" ? [pane.id] : [];
      await requestFileDocumentClose(paneIds, () => executeClosePane(tab.id, pane.id));
    },
    [executeClosePane, requestFileDocumentClose],
  );

  const handleCloseWorkspaceTab = useCallback(
    async (tab: Tab) => {
      if (tab.locked) {
        notifyLockedTabCloseBlocked();
        return;
      }
      await requestCloseTabs([tab]);
    },
    [notifyLockedTabCloseBlocked, requestCloseTabs],
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
      await requestClosePane(tab, pane);
    },
    [notifyLockedTabCloseBlocked, requestClosePane, tabs],
  );

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      if (tab && pane) {
        setTerminalWindows((current) => {
          const leaf = current ? findTerminalWindowLeafByTabId(current, tab.id) : null;
          return current && leaf ? setLeafActiveTab(current, leaf.id, tab.id) : current;
        });
        setActiveTabId(tab.id);
        setActivePane(tab.id, pane.id);
      }
    },
    [tabs, setActivePane, setActiveTabId, setTerminalWindows],
  );

  const getQuickCommandPeerSessionIds = useCallback(
    (sessionId: string) => {
      return getSessionInputPeerIds(sessionId, syncGroups, tabs, broadcastToAll).filter(
        (peerSessionId) => !isSftpOnlySession(peerSessionId, liveSessionsById),
      );
    },
    [broadcastToAll, liveSessionsById, syncGroups, tabs],
  );

  const handleHistoryCommand = useCallback(
    (command: string, execute: boolean = true) => {
      if (
        activePane?.paneKind !== "terminal" ||
        !hasLiveSession(activePane) ||
        isSftpOnlyPane(activePane, liveSessionsById)
      ) {
        return;
      }

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
      import("@/lib/backend/api").then(({ emit }) => {
        emit(`focus-terminal-${sessionId}`);
      });
    },
    [activePane, getQuickCommandPeerSessionIds, liveSessionsById],
  );

  const handleSendToAllSessions = useCallback(
    (command: string, execute: boolean = true) => {
      const data = buildTerminalCommandInput(command, execute);
      for (const tab of tabs) {
        for (const pane of collectSessionPanes(tab.root)) {
          if (
            pane.paneKind !== "terminal" ||
            !hasLiveSession(pane) ||
            !isNonSerialSessionType(pane.type) ||
            isSftpOnlyPane(pane, liveSessionsById)
          ) {
            continue;
          }
          const { sessionId } = pane;
          void sendSessionInput(sessionId, data, {
            preview: execute ? { kind: "reset" } : { kind: "data", data: command },
            registerSubmission: execute ? command : null,
          }).catch(() => {});
        }
      }
    },
    [liveSessionsById, tabs],
  );

  const handleReconnected = useCallback(
    (oldSessionId: string, newSessionId: string) => {
      const tab = findTabBySessionId(tabs, oldSessionId);
      const pane = tab ? findPaneBySessionId(tab, oldSessionId) : null;
      if (!pane) return;
      replaceSessionReferences(oldSessionId, newSessionId);
      updateAutoIconForSessionStart(pane.connectionId, newSessionId);
    },
    [replaceSessionReferences, tabs, updateAutoIconForSessionStart],
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
    const pending = addPendingTab(t("menu.newLocalTerminal"), "Local");
    invoke<string>("create_local_session", {
      recordingScopeId: pending.paneId,
      createRequestId: pending.createRequestId,
    })
      .then((sessionId) => {
        if (!hasTab(pending.tabId)) {
          void closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(pending.tabId, sessionId);
      })
      .catch((e) => {
        logger.error({
          domain: "session.lifecycle",
          event: "session.create_failed",
          message: "Failed to create local session",
          data: { session_type: "Local" },
          error: e,
        });
        markTabConnectionFailed(pending.tabId, getErrorMessage(e));
      });
  }, [addPendingTab, hasTab, markTabConnectionFailed, t, updateTabSession]);

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
      const targetTabId = index === -1 ? tabIds[tabIds.length - 1] : tabIds[index];
      if (targetTabId) setActiveTabId(targetTabId);
    },
    [getActiveLeafTabIds, setActiveTabId],
  );

  const handleToggleLeftSidebar = useCallback(() => {
    if (panelOpenMode === "floating") {
      if (floatingPanels.left) {
        closeFloatingPanel("left");
        return;
      }
      const first = getVisibleActivityIds(
        [...uiConfig.activity_bar_layout.left_top, ...uiConfig.activity_bar_layout.left_bottom],
        uiConfig,
      ).find(canUseFloatingPanel);
      if (first) handleFloatingPanelSelect(first, "left");
      return;
    }
    updateUi((prev) => {
      if (multiPanelOpen) {
        if ((prev.left_open_panels?.length ?? 0) > 0 || prev.active_left_panel) {
          return { left_open_panels: [], active_left_panel: null };
        }
        const first = getVisibleActivityIds(
          [...prev.activity_bar_layout.left_top, ...prev.activity_bar_layout.left_bottom],
          prev,
        ).find((id) => !NON_PANEL_IDS.has(id));
        if (!first) return {};
        return EXCLUSIVE_PANEL_IDS.has(first)
          ? { active_left_panel: first }
          : { left_open_panels: [first], active_left_panel: first };
      }
      if (prev.active_left_panel) return { active_left_panel: null };
      const first = getVisibleActivityIds(
        [...prev.activity_bar_layout.left_top, ...prev.activity_bar_layout.left_bottom],
        prev,
      ).find((id) => !NON_PANEL_IDS.has(id));
      return { active_left_panel: first ?? null };
    });
  }, [
    closeFloatingPanel,
    floatingPanels.left,
    handleFloatingPanelSelect,
    multiPanelOpen,
    panelOpenMode,
    uiConfig,
    updateUi,
  ]);

  const handleToggleRightSidebar = useCallback(() => {
    if (panelOpenMode === "floating") {
      if (floatingPanels.right) {
        closeFloatingPanel("right");
        return;
      }
      const first = getVisibleActivityIds(
        [...uiConfig.activity_bar_layout.right_top, ...uiConfig.activity_bar_layout.right_bottom],
        uiConfig,
      ).find(canUseFloatingPanel);
      if (first) handleFloatingPanelSelect(first, "right");
      return;
    }
    updateUi((prev) => {
      if (multiPanelOpen) {
        if ((prev.right_open_panels?.length ?? 0) > 0 || prev.active_right_panel) {
          return { right_open_panels: [], active_right_panel: null };
        }
        const first = getVisibleActivityIds(
          [...prev.activity_bar_layout.right_top, ...prev.activity_bar_layout.right_bottom],
          prev,
        ).find((id) => !NON_PANEL_IDS.has(id));
        if (!first) return {};
        return EXCLUSIVE_PANEL_IDS.has(first)
          ? { active_right_panel: first }
          : { right_open_panels: [first], active_right_panel: first };
      }
      if (prev.active_right_panel) return { active_right_panel: null };
      const first = getVisibleActivityIds(
        [...prev.activity_bar_layout.right_top, ...prev.activity_bar_layout.right_bottom],
        prev,
      ).find((id) => !NON_PANEL_IDS.has(id));
      return { active_right_panel: first ?? null };
    });
  }, [
    closeFloatingPanel,
    floatingPanels.right,
    handleFloatingPanelSelect,
    multiPanelOpen,
    panelOpenMode,
    uiConfig,
    updateUi,
  ]);

  const { handleZoomIn, handleZoomOut, handleResetZoom } = useTerminalZoom(
    updateAppSettings,
    appSettings.keybindings,
    appSettings.interaction.terminal_zoom_enabled,
  );

  useFileEditorZoom(updateAppSettings);

  const handleOpenSettings = useCallback(() => {
    openSettings();
  }, []);

  const handleLockScreen = useCallback(() => {
    if (appSettings.security.enable_startup_lock || appSettings.security.enable_idle_lock) {
      setIsLocked(true);
    }
  }, [
    appSettings.security.enable_idle_lock,
    appSettings.security.enable_startup_lock,
    setIsLocked,
  ]);

  const handleQuitApplication = useCallback(() => {
    setShowQuitConfirm(false);
    void requestFileDocumentClose(
      collectFileDocumentPaneIds(tabs),
      async () => {
        await persistWorkspaceLayoutNow().catch((error) => {
          logger.error({
            domain: "settings.persistence",
            event: "workspace_layout.persist_before_quit_failed",
            message: "Failed to persist workspace layout before quit",
            error,
          });
        });
        await invoke<void>("quit_application");
      },
    );
  }, [persistWorkspaceLayoutNow, requestFileDocumentClose, tabs]);

  useEffect(() => {
    if (!settingsLoaded) return;
    let unlistenCloseRequested: (() => void) | undefined;

    import("@/lib/backend/platform/window")
      .then(({ getCurrentWindow }) => {
        const currentWindow = getCurrentWindow();
        return currentWindow.onCloseRequested(async (event) => {
          if (allowProgrammaticWindowCloseRef.current) return;

          event.preventDefault();

          if (appSettings.general.minimize_to_tray) {
            await persistWorkspaceLayoutNow().catch((error) => {
              logger.error({
                domain: "settings.persistence",
                event: "workspace_layout.persist_before_close_failed",
                message: "Failed to persist workspace layout before close",
                error,
              });
            });
            await invoke<void>("hide_main_window").catch(() => {});
            return;
          }

          if (tabs.length > 0 && appSettings.general.confirm_on_close !== false) {
            setShowQuitConfirm(true);
            return;
          }

          await requestFileDocumentClose(
            collectFileDocumentPaneIds(tabs),
            async () => {
              await persistWorkspaceLayoutNow().catch((error) => {
                logger.error({
                  domain: "settings.persistence",
                  event: "workspace_layout.persist_before_close_failed",
                  message: "Failed to persist workspace layout before close",
                  error,
                });
              });
              allowProgrammaticWindowCloseRef.current = true;
              await currentWindow.close().catch(() => {
                allowProgrammaticWindowCloseRef.current = false;
              });
              window.setTimeout(() => {
                allowProgrammaticWindowCloseRef.current = false;
              }, 1000);
            },
          );
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
    appSettings.general.confirm_on_close,
    appSettings.general.minimize_to_tray,
    persistWorkspaceLayoutNow,
    requestFileDocumentClose,
    settingsLoaded,
    tabs,
  ]);

  const handleRequestQuit = useCallback(() => {
    if (tabs.length > 0 && appSettings.general.confirm_on_close !== false) {
      setShowQuitConfirm(true);
      return;
    }

    handleQuitApplication();
  }, [appSettings.general.confirm_on_close, handleQuitApplication, tabs.length]);

  const handleRequestWindowClose = useCallback(() => {
    if (
      !appSettings.general.minimize_to_tray &&
      tabs.length > 0 &&
      appSettings.general.confirm_on_close !== false
    ) {
      setShowQuitConfirm(true);
      return;
    }

    void import("@/lib/backend/platform/window").then(({ getCurrentWindow }) =>
      getCurrentWindow().close(),
    );
  }, [appSettings.general.confirm_on_close, appSettings.general.minimize_to_tray, tabs.length]);

  useEffect(() => {
    const unlisten = listen<TrayAction>("tray-action", ({ payload }) => {
      if (!eventTargetsCurrentWindow(payload.targetWindowLabel)) return;
      if (isLocked && payload.type !== "lock_screen" && payload.type !== "request_quit") {
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
    async (
      tab: Tab,
      startupCommand?: StartupCommandRequest,
      sourcePane?: SessionPane,
      workingDir?: string,
    ) => {
      const pane = sourcePane ?? getActivePane(tab);
      if (!canCreateSessionFromPane(pane)) return;
      if (
        startupCommand &&
        (isSftpOnlyPane(pane, liveSessionsById) ||
          !canUseStartupCommandForPane(pane, savedConnections))
      )
        return;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
          {
            temporaryConfig: pane.temporaryConfig,
            sshRuntimeMode: pane.sshRuntimeMode,
          },
        );
        const { tabId, createRequestId } = pending;
        setTerminalWindows((current) =>
          current ? insertTabAfterInLeaf(current, tab.id, tabId, tabId) : current,
        );
        try {
          const sessionId = await createSessionForPane(
            { ...pane, id: pending.paneId },
            createRequestId,
            startupCommand,
            workingDir,
          );
          if (!hasTab(tabId)) {
            await closeStaleCreatedSession(sessionId);
            return;
          }
          updateTabSession(tabId, sessionId);
          if (startupCommand && pane.type !== "SSH" && pane.type !== "Telnet") {
            void sendStartupCommandToSession(sessionId, startupCommand).catch((error) => {
              logger.error({
                domain: "session.lifecycle",
                event: "session.startup_command_failed",
                message: "Failed to send startup command to duplicated session",
                ids: { session_id: sessionId },
                error,
              });
              toast.error(t("tabCtx.duplicateFailed"));
            });
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
            ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
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
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updateTabSession,
      liveSessionsById,
      savedConnections,
      setTerminalWindows,
    ],
  );

  const handleMultiplexSshSession = useCallback(
    async (tab: Tab, startupCommand?: StartupCommandRequest) => {
      const pane = getActivePane(tab);
      if (
        !pane ||
        pane.paneKind !== "terminal" ||
        pane.type !== "SSH" ||
        !canMultiplexSshPane(pane, liveSessionsById) ||
        isSftpOnlyPane(pane, liveSessionsById) ||
        pane.connecting ||
        pane.connectError
      ) {
        return;
      }

      let tabId: string | undefined;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
          {
            temporaryConfig: pane.temporaryConfig,
            sshRuntimeMode: pane.sshRuntimeMode,
          },
        );
        tabId = pending.tabId;
        setTerminalWindows((current) =>
          current && tabId ? insertTabAfterInLeaf(current, tab.id, tabId, tabId) : current,
        );

        const sessionId = await invoke<string>("create_multiplexed_ssh_session", {
          sourceSessionId: pane.sessionId,
          startupCommand: buildStartupCommandPayload(startupCommand),
        });
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
      liveSessionsById,
      setTerminalWindows,
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
    if (!canUseStartupCommandForPane(getActivePane(tab), savedConnections)) return;
    window.dispatchEvent(
      new CustomEvent("nyaterm:open-tab-startup-command-dialog", {
        detail: { tabId: tab.id, action: "duplicate" },
      }),
    );
  }, [getActiveTab, savedConnections]);

  const handleMultiplexActiveSshSessionWithCommand = useCallback(() => {
    const tab = getActiveTab();
    if (!tab) return;
    if (!canMultiplexSshPane(getActivePane(tab), liveSessionsById)) return;
    window.dispatchEvent(
      new CustomEvent("nyaterm:open-tab-startup-command-dialog", {
        detail: { tabId: tab.id, action: "multiplex" },
      }),
    );
  }, [getActiveTab, liveSessionsById]);

  const hasFileDocumentDependency = useCallback(
    (sessionId: string) =>
      tabs.some((tab) =>
        collectSessionPanes(tab.root).some(
          (pane) => pane.paneKind === "file" && pane.sessionId === sessionId,
        ),
      ),
    [tabs],
  );

  const notifyFileDocumentDependency = useCallback(() => {
    toast.info(t("tabCtx.fileSessionInUse"));
  }, [t]);

  const buildPaneReconnectCwdStartupCommand = useCallback(
    (pane: Pick<SessionPane, "sessionId">) =>
      (appSettings.terminal.reconnect_restore_cwd ?? false)
        ? buildReconnectCwdStartupCommand(
            pane.sessionId,
            appSettings.interaction.duplicate_session_command_delay_ms,
          )
        : undefined,
    [
      appSettings.terminal.reconnect_restore_cwd,
      appSettings.interaction.duplicate_session_command_delay_ms,
    ],
  );

  const handleReconnectSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;
      if (hasFileDocumentDependency(pane.sessionId)) {
        notifyFileDocumentDependency();
        return;
      }

      toast.info(t("tabCtx.reconnecting"));

      try {
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const reconnectCwdStartupCommand = buildPaneReconnectCwdStartupCommand(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(
          pane,
          undefined,
          reconnectCwdStartupCommand,
        );
        carryOverSessionCwd(pane.sessionId, newSessionId);
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
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect session",
          ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
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
      buildPaneReconnectCwdStartupCommand,
      closePaneBackendSession,
      hasFileDocumentDependency,
      hasPane,
      maybePromptConnectionEdit,
      notifyFileDocumentDependency,
      recordRecentConnection,
      t,
      updateAutoIconForSessionStart,
      updatePaneSession,
    ],
  );

  const handleDisconnectSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.connecting || pane.connectError || pane.paneKind === "file") return;
      if (hasFileDocumentDependency(pane.sessionId)) {
        notifyFileDocumentDependency();
        return;
      }

      const closed = await closePaneBackendSession(pane);
      if (!closed) {
        toast.error(t("tabCtx.disconnectFailed"));
        return;
      }

      toast.success(t("tabCtx.disconnectSuccess"));
    },
    [closePaneBackendSession, hasFileDocumentDependency, notifyFileDocumentDependency, t],
  );

  const handleReconnectSessionById = useCallback(
    async (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      if (!tab || !pane || pane.connecting || !canCreateSessionFromPane(pane)) return;
      if (hasFileDocumentDependency(pane.sessionId)) {
        notifyFileDocumentDependency();
        return;
      }

      toast.info(t("tabCtx.reconnecting"));

      try {
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const reconnectCwdStartupCommand = buildPaneReconnectCwdStartupCommand(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(
          pane,
          undefined,
          reconnectCwdStartupCommand,
        );
        carryOverSessionCwd(pane.sessionId, newSessionId);
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
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect session from active sessions panel",
          ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
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
      buildPaneReconnectCwdStartupCommand,
      closePaneBackendSession,
      hasFileDocumentDependency,
      hasPane,
      maybePromptConnectionEdit,
      notifyFileDocumentDependency,
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
      if (!pane || pane.paneKind === "file" || !canCreateSessionFromPane(pane)) return;
      const leaf = terminalWindows ? findTerminalWindowLeafByTabId(terminalWindows, tab.id) : null;
      if (!leaf) {
        toast.error(t("tabCtx.splitFailed"));
        return;
      }

      if (leaf.tabIds.length > 1) {
        setTerminalWindows((current) =>
          current ? splitTerminalWindowForTab(current, tab.id, direction) : current,
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
          current ? splitTerminalWindowForTab(current, tab.id, direction, newTabId) : current,
        );
        const sessionId = await createSessionForPane(
          { ...pane, id: pending.paneId },
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
        if ((newTabId && !hasTab(newTabId)) || isSessionCreationCancelled(error)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "session.split_failed",
          message: "Failed to create split session",
          ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
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
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      setActiveTabId,
      t,
      terminalWindows,
      setTerminalWindows,
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
    [tabs, setTerminalWindows],
  );

  const handleReconnectPane = useCallback(
    async (tabId: string, paneId: string) => {
      const tab = tabs.find((item) => item.id === tabId);
      const pane = tab
        ? (collectSessionPanes(tab.root).find((item) => item.id === paneId) ?? null)
        : null;
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;
      if (hasFileDocumentDependency(pane.sessionId)) {
        notifyFileDocumentDependency();
        return;
      }

      try {
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_reconnecting_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        const reconnectContent = capturePaneReconnectContent(pane);
        const reconnectCwdStartupCommand = buildPaneReconnectCwdStartupCommand(pane);
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const newSessionId = await createSessionForPane(
          pane,
          undefined,
          reconnectCwdStartupCommand,
        );
        carryOverSessionCwd(pane.sessionId, newSessionId);
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
        if (supports("nativeWindows") && pane.connectionId) {
          await invoke("mark_tunnels_disconnected_for_connection", {
            connectionId: pane.connectionId,
          }).catch(() => {});
        }
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect pane",
          ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
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
      buildPaneReconnectCwdStartupCommand,
      closePaneBackendSession,
      hasFileDocumentDependency,
      hasPane,
      markPaneConnectionFailed,
      maybePromptConnectionEdit,
      notifyFileDocumentDependency,
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
      await requestClosePane(tab, pane);
    },
    [notifyLockedTabCloseBlocked, requestClosePane],
  );

  const handleDisconnectSessionById = useCallback(
    async (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;

      if (pane && hasFileDocumentDependency(sessionId)) {
        notifyFileDocumentDependency();
        return;
      }

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

      await requestClosePane(tab, pane);
    },
    [hasFileDocumentDependency, notifyFileDocumentDependency, requestClosePane, t, tabs],
  );

  const canReconnectSessionById = useCallback(
    (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;
      return (
        !!pane &&
        pane.paneKind !== "file" &&
        !pane.connecting &&
        !hasFileDocumentDependency(sessionId) &&
        canCreateSessionFromPane(pane)
      );
    },
    [hasFileDocumentDependency, tabs],
  );

  const handleCloseAllTabs = useCallback(async () => {
    const tabsToClose = tabs.filter((tab) => !tab.locked);
    const skippedLockedCount = tabs.length - tabsToClose.length;
    if (tabsToClose.length > 0) await requestCloseTabs(tabsToClose);
    if (skippedLockedCount > 0) toast.info(t("tabCtx.lockedTabsSkipped"));
  }, [requestCloseTabs, t, tabs]);

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

      if (tabsToClose.length > 0) {
        await requestCloseTabs(tabsToClose, { nextActiveTabId: keepTabId });
      } else if (activeTabId !== keepTabId) {
        setActiveTabId(keepTabId);
      }

      if (skippedLockedCount > 0) toast.info(t("tabCtx.lockedTabsSkipped"));
    },
    [activeTabId, requestCloseTabs, setActiveTabId, t, tabs, terminalWindows],
  );

  const handleCloseRightTabs = useCallback(
    async (tabId: string) => {
      const leaf = terminalWindows ? findTerminalWindowLeafByTabId(terminalWindows, tabId) : null;
      const tabOrder = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const idx = tabOrder.indexOf(tabId);
      if (idx === -1) return;

      const rightTabIds = tabOrder.slice(idx + 1);
      const targetTabsToClose = tabs.filter((tab) => rightTabIds.includes(tab.id));
      const tabsToClose = targetTabsToClose.filter((tab) => !tab.locked);
      const skippedLockedCount = targetTabsToClose.length - tabsToClose.length;

      if (tabsToClose.length > 0) await requestCloseTabs(tabsToClose);
      if (skippedLockedCount > 0) toast.info(t("tabCtx.lockedTabsSkipped"));
    },
    [requestCloseTabs, t, tabs, terminalWindows],
  );

  const handleSessionInfo = useCallback((tab: Tab) => {
    const pane = getActivePane(tab);
    if (pane?.connectionId) {
      openNewSession(pane.connectionId);
    }
  }, []);

  const handleOpenChat = useCallback(() => {
    if (!isLocked) {
      openPanel("aiAssistant", "right");
    }
  }, [isLocked, openPanel]);

  const handleShowAllCommands = useCallback(() => {
    if (!isLocked) {
      updateUi((prev) => ({
        show_quick_cmd_bar: !prev.show_quick_cmd_bar,
        ...(prev.show_serial_send_panel ? { show_serial_send_panel: false } : {}),
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

  const {
    handleToggleSessionRecordingById,
    handleToggleSessionRecording,
    handleSaveSessionTranscript,
    handleSaveSessionTranscriptById,
  } = useSessionRecordingActions({
    recording: appSettings.recording,
    recordingSessions,
    liveSessionsById,
    refreshRecordingStatuses,
    t,
  });

  const handleToggleActiveSessionRecording = useCallback(() => {
    if (isLocked || !activePane || activePane.connecting || activePane.connectError) {
      return;
    }
    void handleToggleSessionRecordingById(activePane.sessionId, "transcript");
  }, [activePane, handleToggleSessionRecordingById, isLocked]);

  const handleTogglePaneFocus = useCallback(() => {
    if (!activePane) return;
    setPaneFocusMode((current) => !current);
  }, [activePane]);

  const handleToggleNativeFullscreen = useCallback(() => {
    void import("@/lib/backend/platform/window").then(
      async ({ getCurrentWindow }) => {
        const currentWindow = getCurrentWindow();
        const fullscreen = await currentWindow.isFullscreen();
        await currentWindow.setFullscreen(!fullscreen);
        setNativeFullscreen(!fullscreen);
      },
    );
  }, []);

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
      onTogglePaneFocus: handleTogglePaneFocus,
      onToggleNativeFullscreen: handleToggleNativeFullscreen,
      onZoomIn: handleZoomIn,
      onZoomOut: handleZoomOut,
      onResetZoom: handleResetZoom,
      onOpenSettings: handleOpenSettings,
      onOpenChat: handleOpenChat,
      onShowAllCommands: handleShowAllCommands,
      onLockScreen: handleLockScreen,
      onManageSyncGroups: () => setShowSyncGroupDialog(true),
      onClearTerminal: () => window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
      onToggleRecording: handleToggleActiveSessionRecording,
    },
    appSettings.keybindings,
  );

  // Resize handlers
  const handleLeftResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        left_width: Math.max(160, Math.min(720, (prev.left_width || 256) + delta)),
      }));
    },
    [updateUi],
  );

  const handleRightResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        right_width: Math.max(200, Math.min(720, (prev.right_width || 288) - delta)),
      }));
    },
    [updateUi],
  );

  const handleQuickCmdResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        quick_cmd_height: Math.max(36, Math.min(520, (prev.quick_cmd_height || 180) - delta)),
      }));
    },
    [updateUi],
  );

  const handleSerialSendResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        serial_send_height: Math.max(60, Math.min(520, (prev.serial_send_height || 180) - delta)),
      }));
    },
    [updateUi],
  );

  const {
    leftTopItems,
    leftBottomItems,
    rightTopItems,
    rightBottomItems,
    leftHiddenItems,
    rightHiddenItems,
    showLabels,
    toggleActiveIds,
    handleItemSelect,
    handleReorder,
    handleMoveItem,
    handleToggleLabel,
    handleHideItem,
    handleShowItem,
    handleToggleItemVisibility,
    handleResetActivityBarLayout,
  } = useActivityBarController({
    uiConfig,
    activeSessionId,
    recordingSessions,
    multiPanelOpen,
    panelOpenMode,
    onFloatingPanelSelect: handleFloatingPanelSelect,
    onFloatingPanelMove: handleFloatingPanelMove,
    updateUi,
    setIsLocked,
    t,
  });

  // --- Panel content rendering (side-independent) ---

  const activeSftpOnly = isSftpOnlyPane(activePane, liveSessionsById);
  useMcpActiveSession(activeSftpOnly ? null : activeSessionId);
  const {
    activeStatsSessionId,
    activeRemoteStatsEnabled,
    remoteStats,
    networkHistoryStore,
    gpuOverviewState,
    npuOverviewState,
  } = useWorkspaceMonitoring({
    activePane,
    activeConnection,
    liveSessionIds,
    liveSessionsById,
    uiConfig,
    handleAssetMonitoringPatch,
  });

  const handleOpenDirectoryInNewTerminal = useCallback(
    (sessionId: string, path: string) => {
      const source = tabs
        .map((tab) => ({
          tab,
          pane: findSessionPaneBySessionId(tab.root, sessionId),
        }))
        .find(({ pane }) => pane?.paneKind === "terminal");
      const tab = source?.tab;
      const pane = source?.pane;
      const session = liveSessionsById?.get(sessionId);
      if (
        !tab ||
        !pane ||
        pane.paneKind !== "terminal" ||
        !session?.connected ||
        isSftpOnlyPane(pane, liveSessionsById) ||
        (pane.type !== "SSH" && pane.type !== "Local")
      ) {
        toast.error(t("fileExplorer.directoryTerminalUnavailable"));
        return;
      }
      if (pane.type === "Local") {
        if (!isTerminalDirectoryPath(path)) {
          toast.error(t("fileExplorer.directoryTerminalInvalidPath"));
          return;
        }
        void handleDuplicateSession(tab, undefined, pane, path);
        return;
      }
      const command = buildDirectoryChangeCommand(path, "posix");
      if (!command) {
        toast.error(t("fileExplorer.directoryTerminalInvalidPath"));
        return;
      }
      void handleDuplicateSession(
        tab,
        {
          command,
          delayMs: appSettings.interaction.duplicate_session_command_delay_ms,
        },
        pane,
      );
    },
    [
      appSettings.interaction.duplicate_session_command_delay_ms,
      handleDuplicateSession,
      liveSessionsById,
      t,
      tabs,
    ],
  );
  const activeSerialSessionId =
    activePane &&
    activePane.paneKind === "terminal" &&
    !activePane.connecting &&
    !activePane.connectError &&
    activePane.type === "Serial"
      ? activePane.sessionId
      : null;
  const activeNonSerialSessionId =
    activePane &&
    activePane.paneKind === "terminal" &&
    !activePane.connecting &&
    !activePane.connectError &&
    isNonSerialSessionType(activePane.type) &&
    !activeSftpOnly
      ? activePane.sessionId
      : null;
  const activeNonSerialSessionIds = useMemo(
    () =>
      collectActiveNonSerialSessionIds(terminalWindows, tabsById).filter(
        (sessionId) => !isSftpOnlySession(sessionId, liveSessionsById),
      ),
    [liveSessionsById, tabsById, terminalWindows],
  );
  const sendCommandSessionTargets = useMemo(() => {
    const currentWindowLabel = getOwnerMainWindowLabel();
    const targetsById = new Map<
      string,
      {
        id: string;
        name: string;
        tabName: string;
        type: SessionType;
        ownerWindowLabel?: string | null;
      }
    >();

    if (terminalWindows) {
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
              isSftpOnlyPane(pane, liveSessionsById)
            ) {
              continue;
            }
            targetsById.set(pane.sessionId, {
              id: pane.sessionId,
              name: pane.name,
              tabName: getActiveSessionTabDisplayName(
                tab,
                (sessionId) => dynamicTitles.get(sessionId ?? "")?.effectiveTitle ?? null,
              ),
              type: pane.type,
              ownerWindowLabel: currentWindowLabel,
            });
          }
        }
      };

      visit(terminalWindows);
    }

    for (const session of liveSessionsById?.values() ?? []) {
      if (!["SSH", "Local", "Telnet", "Serial"].includes(session.session_type)) continue;
      if (session.ssh_runtime_mode === "sftp") continue;
      if (targetsById.has(session.id)) continue;
      targetsById.set(session.id, {
        id: session.id,
        name: session.name,
        tabName: session.name,
        type: session.session_type as SessionType,
        ownerWindowLabel: session.owner_window_label ?? null,
      });
    }

    return [...targetsById.values()];
  }, [liveSessionsById, tabsById, terminalWindows, dynamicTitles]);

  const activeBottomPanel = uiConfig.show_serial_send_panel
    ? "serialSend"
    : uiConfig.show_quick_cmd_bar
      ? "quickCmdBar"
      : null;
  const temporarySshShortcut = resolveDisplayKeys("tab.temporarySshLink", appSettings.keybindings);
  const openChatShortcut = resolveDisplayKeys("view.openChat", appSettings.keybindings);
  const showCommandsShortcut = resolveDisplayKeys("view.showAllCommands", appSettings.keybindings);
  const switchTerminalShortcut = resolveDisplayKeys("tab.quickSwitch", appSettings.keybindings);

  const quickSwitcherSessions = useMemo<QuickSwitcherSession[]>(() => {
    const connectionsById = new Map(
      savedConnections.map((connection) => [connection.id, connection]),
    );
    const sessions: QuickSwitcherSession[] = [];
    for (const tab of tabs) {
      for (const pane of collectSessionPanes(tab.root)) {
        if (pane.paneKind !== "terminal") continue;
        const connection = pane.connectionId ? connectionsById.get(pane.connectionId) : undefined;
        sessions.push({
          id: pane.sessionId,
          name: pane.name,
          sessionType: pane.type,
          connectionName: connection?.name,
          tabName: getActiveSessionTabDisplayName(
            tab,
            (sessionId) => dynamicTitles.get(sessionId ?? "")?.effectiveTitle ?? null,
          ),
          connecting: pane.connecting,
          connectError: pane.connectError,
        });
      }
    }
    return sessions;
  }, [savedConnections, tabs, dynamicTitles]);

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
        transfer_height: Math.max(60, Math.min(600, (prev.transfer_height || 180) - delta)),
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
  const dockedLeftPanelIds = panelOpenMode === "floating" ? [] : leftPanelIds;
  const dockedRightPanelIds = panelOpenMode === "floating" ? [] : rightPanelIds;
  const dockedLeftOverlayPanelId = panelOpenMode === "floating" ? null : leftOverlayPanelId;
  const dockedRightOverlayPanelId = panelOpenMode === "floating" ? null : rightOverlayPanelId;
  const visibleFloatingPanels =
    panelOpenMode === "floating" ? floatingPanels : { left: null, right: null };
  const leftActivityActiveIds = useMemo(() => {
    if (panelOpenMode !== "floating") return leftActiveIds;
    return floatingPanels.left ? new Set([floatingPanels.left]) : undefined;
  }, [floatingPanels.left, leftActiveIds, panelOpenMode]);
  const rightActivityActiveIds = useMemo(() => {
    if (panelOpenMode !== "floating") return rightActiveIds;
    return floatingPanels.right ? new Set([floatingPanels.right]) : undefined;
  }, [floatingPanels.right, panelOpenMode, rightActiveIds]);

  useEffect(() => {
    if (panelOpenMode !== "floating") return;
    const next = clearUnavailableFloatingPanels(floatingPanels, uiConfig);
    if (next === floatingPanels) return;
    setFloatingPanels(next);
    setLastFloatingSide((current) => {
      if (current && next[current]) return current;
      return next.right ? "right" : next.left ? "left" : null;
    });
  }, [floatingPanels, panelOpenMode, uiConfig]);

  useEffect(() => {
    if (panelOpenMode !== "floating") return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (isLocked || modalChildWindowCount > 0) return;
      if (
        showAbout ||
        showUpdateDialog ||
        showSyncGroupDialog ||
        showQuitConfirm ||
        showActivityBarResetConfirm ||
        showSessionQuickSwitcher ||
        showTemporarySshLink ||
        Boolean(externalMatchDialog) ||
        Boolean(postLoginConfirm) ||
        Boolean(pendingFileDocumentClose) ||
        Boolean(activeHostKeyRequest) ||
        Boolean(activeSshAgentRequest) ||
        Boolean(activeOtpRequest) ||
        Boolean(activeSshAuthRequest) ||
        Boolean(dockerSudoPasswordRequest) ||
        rdpCertificateRequests.length > 0 ||
        vncServerKeyRequests.length > 0
      ) {
        return;
      }

      const sideToClose =
        lastFloatingSide && floatingPanels[lastFloatingSide]
          ? lastFloatingSide
          : floatingPanels.right
            ? "right"
            : floatingPanels.left
              ? "left"
              : null;
      if (!sideToClose) return;
      event.preventDefault();
      closeFloatingPanel(sideToClose);
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [
    activeHostKeyRequest,
    activeOtpRequest,
    activeSshAgentRequest,
    activeSshAuthRequest,
    closeFloatingPanel,
    dockerSudoPasswordRequest,
    externalMatchDialog,
    floatingPanels,
    isLocked,
    lastFloatingSide,
    modalChildWindowCount,
    panelOpenMode,
    pendingFileDocumentClose,
    postLoginConfirm,
    rdpCertificateRequests.length,
    vncServerKeyRequests.length,
    showAbout,
    showActivityBarResetConfirm,
    showQuitConfirm,
    showSessionQuickSwitcher,
    showSyncGroupDialog,
    showTemporarySshLink,
    showUpdateDialog,
  ]);

  // When multi-open mode is first enabled, seed the stacks from the active panels.
  useEffect(() => {
    if (!settingsLoaded || !multiPanelOpen || panelOpenMode === "floating") return;
    updateUi((prev) => ({
      ...((prev.left_open_panels?.length ?? 0) === 0 &&
      prev.active_left_panel &&
      isActivityItemAvailable(prev.active_left_panel, prev) &&
      !EXCLUSIVE_PANEL_IDS.has(prev.active_left_panel)
        ? { left_open_panels: [prev.active_left_panel] }
        : {}),
      ...((prev.right_open_panels?.length ?? 0) === 0 &&
      prev.active_right_panel &&
      isActivityItemAvailable(prev.active_right_panel, prev) &&
      !EXCLUSIVE_PANEL_IDS.has(prev.active_right_panel)
        ? { right_open_panels: [prev.active_right_panel] }
        : {}),
    }));
  }, [multiPanelOpen, panelOpenMode, settingsLoaded, updateUi]);

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
        const totalWeight = openIds.reduce((sum, id) => sum + (sizes[id] ?? 1), 0);
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

  const getPanelTitle = useCallback(
    (panelId: string) => {
      const pluginPanel = parsePluginPanelId(panelId);
      if (pluginPanel) {
        const plugin = plugins.find(
          (plugin) => plugin.id === pluginPanel.pluginId,
        );
        return (
          (plugin &&
            activeManifest(plugin)?.contributions.panels.find(
              (panel) => panel.id === pluginPanel.panelId,
            )?.title) ||
          t("plugins.title")
        );
      }
      switch (panelId) {
        case "plugins":
          return t("plugins.title");
        case "securityAuth":
          return t("securityAuth.title");
        case "aiAssistant":
          return t("ai.title");
        case "recording":
          return t("recording.panelTitle");
        default:
          return t(`panel.${panelId}`);
      }
    },
    [t, plugins],
  );

  const renderPanelContent = useCallback(
    (panelId: string | null) => (
      <AppPanelContent
        panelId={panelId}
        activePane={activePane}
        activeConnection={activeConnection}
        activeSessionId={activeSessionId}
        shellInputEnabled={!activeSftpOnly}
        activeStatsSessionId={activeStatsSessionId}
        remoteStatsEnabled={activeRemoteStatsEnabled}
        remoteStats={remoteStats}
        networkHistoryStore={networkHistoryStore}
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
        onOpenSftpConnection={openSavedConnectionWithSftp}
        onSessionClick={handleSessionClick}
        onSessionReconnect={handleReconnectSessionById}
        onSessionDisconnect={handleDisconnectSessionById}
        canReconnect={canReconnectSessionById}
        onCommandSend={handleHistoryCommand}
        onOpenDirectoryInNewTerminal={handleOpenDirectoryInNewTerminal}
        onToggleSessionRecording={handleToggleSessionRecording}
        onSaveSessionTranscript={handleSaveSessionTranscript}
      />
    ),
    [
      activeConnection,
      activeStatsSessionId,
      activePane,
      activeSessionId,
      activeSftpOnly,
      aiIntent,
      activeRemoteStatsEnabled,
      canReconnectSessionById,
      remoteStats,
      networkHistoryStore,
      gpuOverviewState,
      npuOverviewState,
      handleSaveSessionTranscript,
      handleDisconnectSessionById,
      handleEditConnection,
      handleHistoryCommand,
      handleOpenDirectoryInNewTerminal,
      handleNewSession,
      handleOpenTemporarySshLink,
      handleReconnectSessionById,
      handleSessionClick,
      handleToggleSessionRecording,
      handleTransferResize,
      connectSavedConnection,
      openSavedConnectionWithSftp,
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
        current?.resolve({
          kind: "saved",
          connection,
          runtimeModeOverride: current.runtimeModeOverride,
        });
        return null;
      });
    },
    [],
  );

  const handleExternalMatchTemporary = useCallback((config: TemporaryLinkConfig) => {
    setExternalMatchDialog((current) => {
      current?.resolve({ kind: "temporary", config });
      return null;
    });
  }, []);

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

  return (
    <TransferProvider>
      <AppLayout
        t={t}
        uiConfig={uiConfig}
        appearance={appSettings.appearance}
        paneFocusMode={paneFocusMode}
        nativeFullscreen={nativeFullscreen}
        onExitPaneFocus={() => setPaneFocusMode(false)}
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
          onClearTerminal: () => window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
          onRefitTerminals: () =>
            window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals")),
          locked: isLocked,
          onRequestQuit: handleRequestQuit,
          onToggleActivityBarItemVisibility: handleToggleItemVisibility,
          onRequestActivityBarReset: () => setShowActivityBarResetConfirm(true),
          onPanelOpenModeChange: handlePanelOpenModeChange,
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
          hiddenItems: leftHiddenItems,
          activeId: panelOpenMode === "floating" ? null : uiConfig.active_left_panel,
          activeIds: leftActivityActiveIds,
          activeBottomIds: toggleActiveIds,
          onSelect: handleItemSelect,
          onReorder: (zoneKey, ids) => handleReorder("left", zoneKey, ids),
          onMoveItem: handleMoveItem,
          onHideItem: handleHideItem,
          onShowItem: handleShowItem,
          onToggleLabel: handleToggleLabel,
          onRequestResetLayout: () => setShowActivityBarResetConfirm(true),
          panelOpenMode,
          onPanelOpenModeChange: handlePanelOpenModeChange,
          showLabels,
        }}
        rightActivityBar={{
          items: rightTopItems,
          bottomItems: rightBottomItems,
          hiddenItems: rightHiddenItems,
          activeId: panelOpenMode === "floating" ? null : uiConfig.active_right_panel,
          activeIds: rightActivityActiveIds,
          activeBottomIds: toggleActiveIds,
          onSelect: handleItemSelect,
          onReorder: (zoneKey, ids) => handleReorder("right", zoneKey, ids),
          onMoveItem: handleMoveItem,
          onHideItem: handleHideItem,
          onShowItem: handleShowItem,
          onToggleLabel: handleToggleLabel,
          onRequestResetLayout: () => setShowActivityBarResetConfirm(true),
          panelOpenMode,
          onPanelOpenModeChange: handlePanelOpenModeChange,
          showLabels,
        }}
        onLeftResize={handleLeftResize}
        onRightResize={handleRightResize}
        panelContent={renderPanelContent}
        panelTitle={getPanelTitle}
        leftPanelIds={dockedLeftPanelIds}
        rightPanelIds={dockedRightPanelIds}
        floatingPanelIds={visibleFloatingPanels}
        onCloseFloatingPanel={closeFloatingPanel}
        leftOverlayPanelId={dockedLeftOverlayPanelId}
        rightOverlayPanelId={dockedRightOverlayPanelId}
        panelStackSizes={uiConfig.panel_stack_sizes ?? {}}
        onPanelStackResize={handlePanelStackResize}
        workspace={{
          layout: terminalWindows,
          tabsById,
          focusedTabId: activeTabId,
          paneFocusMode,
          unreadTabIds,
          disconnectedTabIds,
          sessionInfoById: liveSessionsById,
          onSelectTab: handleSelectLeafTab,
          onAddTab: handleAddTabFromLeaf,
          onConnectConnection: handleConnectConnectionFromLeaf,
          onTabClose: handleCloseWorkspaceTab,
          onDuplicateSession: handleDuplicateSession,
          onMultiplexSshSession: handleMultiplexSshSession,
          onDuplicateSessionWithCommand: handleDuplicateSessionWithCommand,
          onMultiplexSshSessionWithCommand: handleMultiplexSshSessionWithCommand,
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
          clearAfterSend: uiConfig.serial_send_clear_after_send ?? false,
          activeSerialSessionId,
          activeNonSerialSessionId,
          activeNonSerialSessionIds,
          quickCommandsDisabled: activeSftpOnly,
          syncGroups,
          currentWindowLabel: getOwnerMainWindowLabel(),
          sessionTargets: sendCommandSessionTargets,
          sendCommandDraft,
          onSendCommandDraftConsumed: handleSendCommandDraftConsumed,
          onQuickCmdResize: handleQuickCmdResize,
          onSerialSendResize: handleSerialSendResize,
          onClearAfterSendChange: (enabled) => updateUi({ serial_send_clear_after_send: enabled }),
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
          vncServerKeyVerifyRequest: vncServerKeyRequests[0] ?? null,
          onVncServerKeyVerifyDone: (requestId) =>
            setVncServerKeyRequests((current) =>
              current.filter((item) => item.requestId !== requestId),
            ),
          modalChildWindowCount,
          locked: isLocked,
          hasMasterPassword: !!appSettings.security.master_password,
          onUnlock: () => setIsLocked(false),
          onRequestClose: handleRequestWindowClose,
        }}
      />
      <McpApprovalHost />
      <AppOverlayDialogs
        t={t}
        showSessionQuickSwitcher={showSessionQuickSwitcher}
        activeSessionId={activeSessionId}
        quickSwitcherSessions={quickSwitcherSessions}
        savedConnections={savedConnections}
        onCloseSessionQuickSwitcher={handleCloseSessionQuickSwitcher}
        onQuickSwitchSession={handleQuickSwitchSession}
        onQuickOpenConnection={handleQuickOpenConnection}
        onQuickSwitcherNewSshSession={handleQuickSwitcherNewSshSession}
        showTemporarySshLink={showTemporarySshLink}
        onTemporarySshLinkOpenChange={setShowTemporarySshLink}
        onTemporarySshConnect={handleTemporarySshConnect}
        externalMatchDialog={externalMatchDialog}
        savedGroups={savedGroups}
        onExternalMatchOpenChange={handleExternalMatchOpenChange}
        onExternalMatchConnection={handleExternalMatchConnection}
        onExternalMatchTemporary={handleExternalMatchTemporary}
        pendingFileDocumentClose={pendingFileDocumentClose}
        savingFileDocuments={savingFileDocuments}
        onPendingFileDocumentCloseOpenChange={handlePendingFileDocumentCloseOpenChange}
        onSaveFileDocumentsAndClose={handleSaveFileDocumentsAndClose}
        onDiscardFileDocumentsAndClose={handleDiscardFileDocumentsAndClose}
        postLoginConfirm={postLoginConfirm}
        onPostLoginConfirmOpenChange={handlePostLoginConfirmOpenChange}
        onPostLoginContinue={handlePostLoginContinue}
      />
      <ActivityBarResetDialog
        t={t}
        open={showActivityBarResetConfirm}
        onOpenChange={setShowActivityBarResetConfirm}
        onConfirm={() => {
          setShowActivityBarResetConfirm(false);
          handleResetActivityBarLayout();
        }}
      />
    </TransferProvider>
  );
}

export default App;

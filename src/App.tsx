import { emit, listen } from "@tauri-apps/api/event";
import { downloadDir } from "@tauri-apps/api/path";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import AppLayout from "./components/app/AppLayout";
import AppPanelContent from "./components/app/AppPanelContent";
import type { HostKeyVerifyRequest } from "./components/dialog/connections/HostKeyVerifyDialog";
import type { OtpRequest } from "./components/dialog/connections/OtpDialog";
import SessionQuickSwitcher, {
  type QuickSwitcherSession,
} from "./components/terminal/SessionQuickSwitcher";
import { useApp } from "./context/AppContext";
import { TransferProvider } from "./context/TransferContext";
import { useActivityBarController } from "./hooks/useActivityBarController";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { useModalChildWindows } from "./hooks/useModalChildWindows";
import { resolveDisplayKeys } from "./hooks/useShortcutMap";
import { useTerminalZoom } from "./hooks/useTerminalZoom";
import { useUnreadTabs } from "./hooks/useUnreadTabs";
import { AI_OPEN_EVENT, type AIOpenIntent } from "./lib/aiEvents";
import {
  canCreateSessionFromPane,
  collectActiveNonSerialSessionIds,
  getItemSide,
  hasLiveSession,
  isNonSerialSessionType,
  NON_PANEL_IDS,
  type TrayAction,
} from "./lib/appWorkspace";
import { getErrorMessage, shouldPromptConnectionEditOnFailure } from "./lib/errors";
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
} from "./lib/sessionInput";
import { buildSmartSplitLayout, type SmartSplitMode } from "./lib/smartSplit";
import { purgeSessionFromGroups } from "./lib/syncInputGroups";
import {
  findTerminalWindowLeafById,
  findTerminalWindowLeafByTabId,
  flattenTerminalWindows,
  insertTabAfterInLeaf,
  insertTabIntoLeaf,
  moveTabBetweenLeaves,
  reconcileTerminalWindows,
  reorderTabsInLeaf,
  setLeafActiveTab,
  splitTerminalWindowForTab,
  type TerminalWindowNode,
  updateTerminalWindowSplitRatio,
} from "./lib/tabWindows";
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
  CloudConflictPreview,
  PaneSplitDirection,
  SavedConnection,
  SessionInfo,
  SessionPane,
  SessionType,
  Tab,
} from "./types/global";

const CONNECTION_SESSION_TYPES: Record<SavedConnection["type"], SessionType> = {
  ssh: "SSH",
  local_terminal: "Local",
  telnet: "Telnet",
  serial: "Serial",
};

function getConnectionSessionType(
  connection: Pick<SavedConnection, "type"> | null | undefined,
): SessionType {
  return connection ? CONNECTION_SESSION_TYPES[connection.type] : "SSH";
}

function isSessionCreationCancelled(error: unknown) {
  return getErrorMessage(error).toLowerCase().includes("session creation cancelled");
}

async function closeStaleCreatedSession(sessionId: string) {
  try {
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

async function createSessionForConnection(
  connection: Pick<SavedConnection, "id" | "type">,
  createRequestId?: string,
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
      });
    case "serial":
      return invoke<string>("create_serial_session", {
        connectionId: connection.id,
        createRequestId,
      });
    default:
      return invoke<string>("create_ssh_session", {
        connectionId: connection.id,
        createRequestId,
      });
  }
}

function safeRecordingName(name: string) {
  return name.normalize("NFC").replace(/[^\p{L}\p{M}\p{N}._-]+/gu, "_") || "session";
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

/** Root layout: header, activity bars, sidebars, terminal area, dialogs. */
function App() {
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
    recordRecentConnection,
    setSyncGroups,
    broadcastToAll,
    setBroadcastToAll,
    isLocked,
    setIsLocked,
    settingsLoaded,
    runtimeInfo,
    runtimeInfoLoaded,
  } = useApp();
  const uiConfig = appSettings.ui;
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
  const [sendCommandDraft, setSendCommandDraft] = useState<SendCommandPanelDraft | null>(null);
  const [showSessionQuickSwitcher, setShowSessionQuickSwitcher] = useState(false);
  const handleSendCommandDraftConsumed = useCallback(() => {
    setSendCommandDraft(null);
  }, []);

  useEffect(() => {
    return listenOpenSendCommandPanel((draft) => {
      setSendCommandDraft(draft);
      updateUi({ show_serial_send_panel: true });
    });
  }, [updateUi]);

  // Recording state: tracks which sessions are currently being recorded
  const [recordingSessions, setRecordingSessions] = useState<Set<string>>(new Set());

  const refreshRecordingSessions = useCallback(async () => {
    try {
      const sessionIds = await invoke<string[]>("list_recording_sessions");
      setRecordingSessions(new Set(sessionIds));
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
    void refreshRecordingSessions();
    const unlisten = listen("sessions-changed", () => {
      void refreshRecordingSessions();
    });
    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [refreshRecordingSessions]);

  useEffect(() => {
    if (!settingsLoaded) return;
    void invoke("set_recording_memory_limit", {
      maxBytes: Math.max(1, appSettings.transfer.recording_memory_limit_bytes || 5 * 1024 * 1024),
    }).catch((error) => {
      logger.error({
        domain: "settings.persistence",
        event: "recording.memory_limit_sync_failed",
        message: "Failed to sync recording memory limit",
        error,
      });
    });
  }, [appSettings.transfer.recording_memory_limit_bytes, settingsLoaded]);

  // OTP / 2FA dialog state
  const [otpRequest, setOtpRequest] = useState<OtpRequest | null>(null);
  const [hostKeyVerifyRequest, setHostKeyVerifyRequest] = useState<HostKeyVerifyRequest | null>(
    null,
  );
  const lastCloudConflictRevisionRef = useRef<string | null>(null);
  const modalChildWindowCount = useModalChildWindows();

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_screen_lock ? appSettings.security.idle_lock_minutes : 0,
    () => setIsLocked(true),
  );

  // Background update check on startup
  useEffect(() => {
    if (!runtimeInfoLoaded || runtimeInfo.portable) return;

    const timer = setTimeout(() => {
      checkForUpdate()
        .then((info) => {
          if (info) {
            setUpdateInfo(info);
            setHelpDotVisible(true);
          }
        })
        .catch(() => {});
    }, 3000);
    return () => clearTimeout(timer);
  }, [runtimeInfo.portable, runtimeInfoLoaded]);

  const handleOpenPanel = useCallback(
    (panelId: "activeSessions" | "syncBackupHistory") => {
      updateUi((prev) => {
        const side = getItemSide(panelId, prev.activity_bar_layout);
        if (side === "right") {
          return {
            active_right_panel: panelId,
            ...(prev.active_left_panel === panelId ? { active_left_panel: null } : {}),
          };
        }
        return {
          active_left_panel: panelId,
          ...(prev.active_right_panel === panelId ? { active_right_panel: null } : {}),
        };
      });
    },
    [updateUi],
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
        type: "SSH" | "Local" | "Telnet" | "Serial";
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
        setOtpRequest(event.payload);
      }),
    );

    unsubs.push(
      listen<HostKeyVerifyRequest>("host-key-verify", (event) => {
        if (!eventTargetsCurrentWindow(event.payload.targetWindowLabel)) return;
        setHostKeyVerifyRequest(event.payload);
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
          const conns = await invoke<SavedConnection[]>("get_saved_connections");
          const conn = conns.find((c) => c.id === connectionId);
          const connName = conn?.name ?? connectionId;
          const sessionType = getConnectionSessionType(conn);
          const sourceTab = sourceTabId
            ? (tabsRef.current.find((item) => item.id === sourceTabId) ?? null)
            : null;
          const sourcePane =
            sourceTab &&
            ((sourcePaneId ? findSessionPaneById(sourceTab.root, sourcePaneId) : null) ??
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
            });
          } else {
            const pending = addPendingTab(
              connName,
              sessionType,
              connectionId,
              undefined,
              anchorTabId ? { afterTabId: anchorTabId } : undefined,
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
    updatePaneSession,
    updateTabSession,
    replaceAppSettings,
  ]);

  useEffect(() => {
    const unlisten = listen<CloudConflictPreview | null>("cloud-sync-conflict", (event) => {
      const conflict = event.payload;
      if (!conflict) return;
      if (lastCloudConflictRevisionRef.current === conflict.remote_revision) {
        return;
      }

      lastCloudConflictRevisionRef.current = conflict.remote_revision;
      toast.error(conflict.message);
      handleOpenPanel("syncBackupHistory");
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [handleOpenPanel]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
  const activeTabName = activeTab ? getTabDisplayName(activeTab).trim() : "";
  const windowTitle = activeTabName ? `${activeTabName} - NyaTerm` : "NyaTerm";
  const activePane = activeTab ? getActivePane(activeTab) : null;
  const activeConnection = activePane?.connectionId
    ? (savedConnections.find((connection) => connection.id === activePane.connectionId) ?? null)
    : null;
  const [aiIntent, setAiIntent] = useState<AIOpenIntent | null>(null);
  const [terminalWindows, setTerminalWindows] = useState<TerminalWindowNode | null>(null);
  const previousActiveTabIdRef = useRef<string | null>(null);
  const tabsRef = useRef(tabs);
  const tabsById = useMemo(() => new Map(tabs.map((tab) => [tab.id, tab])), [tabs]);

  useEffect(() => {
    tabsRef.current = tabs;
  }, [tabs]);

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

  useEffect(() => {
    setTerminalWindows((current) =>
      reconcileTerminalWindows(current, tabs, activeTabId, previousActiveTabIdRef.current),
    );
    previousActiveTabIdRef.current = activeTabId;
  }, [activeTabId, tabs]);

  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<AIOpenIntent>).detail;
      if (!detail) return;
      setAiIntent(detail);
      updateUi({ active_right_panel: "aiAssistant" });
    };
    window.addEventListener(AI_OPEN_EVENT, handler);
    return () => window.removeEventListener(AI_OPEN_EVENT, handler);
  }, [updateUi]);

  const unreadTabIds = useUnreadTabs(tabs, activeTabId);

  const handleSelectLeafTab = useCallback(
    (leafId: string, tabId: string) => {
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

      if (targetLeaf?.activeTabId) {
        handleSelectLeafTab(leafId, targetLeaf.activeTabId);
      }

      const pending = addPendingTab(
        connection.name,
        getConnectionSessionType(connection),
        connection.id,
        undefined,
        anchorTabId ? { afterTabId: anchorTabId } : undefined,
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
        const sessionId = await createSessionForConnection(connection, createRequestId);
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        recordRecentConnection(connection.id);
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
        maybePromptConnectionEdit(connection.id, errorMessage, { sourceTabId: tabId });
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
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
      updateTabSession,
    ],
  );

  const handleReorderTabsInLeaf = useCallback((_: string, fromTabId: string, toIndex: number) => {
    setTerminalWindows((current) =>
      current ? reorderTabsInLeaf(current, fromTabId, toIndex) : current,
    );
  }, []);

  const handleMoveTabToLeaf = useCallback(
    (fromTabId: string, targetLeafId: string, toIndex: number) => {
      setTerminalWindows((current) => {
        if (!current) return current;
        const next = moveTabBetweenLeaves(current, fromTabId, targetLeafId, toIndex);
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
    setTerminalWindows((current) => {
      if (!current) return current;
      return flattenTerminalWindows(current, activeTabId);
    });
    requestAnimationFrame(() => {
      window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
    });
  }, [activeTabId]);

  const handleUpdateWindowSplitRatio = useCallback((splitId: string, ratio: number) => {
    setTerminalWindows((current) =>
      current ? updateTerminalWindowSplitRatio(current, splitId, ratio) : current,
    );
  }, []);

  const handleActivatePane = useCallback(
    (tabId: string, paneId: string) => {
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
    async (pane: Pick<SessionPane, "type" | "connectionId">, createRequestId?: string) => {
      switch (pane.type) {
        case "Local":
          return invoke<string>("create_local_session", {
            connectionId: pane.connectionId || null,
            createRequestId,
          });
        case "Telnet":
          if (!pane.connectionId) throw new Error("Missing Telnet connection id");
          return invoke<string>("create_telnet_session", {
            connectionId: pane.connectionId,
            createRequestId,
          });
        case "Serial":
          if (!pane.connectionId) throw new Error("Missing Serial connection id");
          return invoke<string>("create_serial_session", {
            connectionId: pane.connectionId,
            createRequestId,
          });
        default:
          if (!pane.connectionId) throw new Error("Missing SSH connection id");
          return invoke<string>("create_ssh_session", {
            connectionId: pane.connectionId,
            createRequestId,
          });
      }
    },
    [],
  );

  const closePaneBackendSession = useCallback(
    async (
      pane: Pick<SessionPane, "connecting" | "connectError" | "sessionId" | "createRequestId">,
    ) => {
      if (pane.connecting) {
        if (pane.createRequestId) {
          try {
            await invoke("cancel_session_creation", { createRequestId: pane.createRequestId });
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
    [setSyncGroups],
  );

  const closeWorkspaceTabSessions = useCallback(
    async (tab: Tab) => {
      const results = await Promise.all(
        collectSessionPanes(tab.root).map((pane) => closePaneBackendSession(pane)),
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

  const handleCloseWorkspaceTab = useCallback(
    async (tab: Tab) => {
      const allClosed = await closeWorkspaceTabSessions(tab);
      if (!allClosed) {
        toast.error(t("tabCtx.closeFailed"));
        return;
      }
      closeTabs([tab.id]);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    },
    [closeTabs, closeWorkspaceTabSessions, persistWorkspaceNow, t],
  );

  const handleCloseDisconnectedPane = useCallback(
    async (tabId: string, paneId: string) => {
      const tab = tabs.find((item) => item.id === tabId);
      const pane = tab ? findSessionPaneById(tab.root, paneId) : null;
      if (!tab || !pane) return;

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
    [tabs, setActivePane, setActiveTabId],
  );

  const handleHistoryCommand = useCallback(
    (command: string, execute: boolean = true) => {
      if (activePane && !activePane.connecting) {
        if (activePane.connectError) return;
        const { sessionId } = activePane;
        void sendSessionInput(sessionId, buildTerminalCommandInput(command, execute), {
          preview: execute ? { kind: "reset" } : { kind: "data", data: command },
          registerSubmission: execute ? command : null,
        }).catch(() => {});
        import("@tauri-apps/api/event").then(({ emit }) => {
          emit(`focus-terminal-${sessionId}`);
        });
      }
    },
    [activePane],
  );

  const handleSendToAllSessions = useCallback(
    (command: string) => {
      for (const tab of tabs) {
        for (const pane of collectSessionPanes(tab.root)) {
          if (!hasLiveSession(pane) || pane.type !== "SSH") continue;
          const { sessionId } = pane;
          void sendSessionInput(sessionId, buildTerminalCommandInput(command), {
            preview: { kind: "reset" },
            registerSubmission: command,
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
      }
    },
    [tabs, updatePaneSession],
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
    void handleCloseWorkspaceTab(activeTab);
  }, [activeTab, handleCloseWorkspaceTab]);

  const handleNextTab = useCallback(() => {
    if (tabs.length < 2 || !activeTabId) return;
    const idx = tabs.findIndex((t) => t.id === activeTabId);
    setActiveTabId(tabs[(idx + 1) % tabs.length].id);
  }, [tabs, activeTabId, setActiveTabId]);

  const handlePrevTab = useCallback(() => {
    if (tabs.length < 2 || !activeTabId) return;
    const idx = tabs.findIndex((t) => t.id === activeTabId);
    setActiveTabId(tabs[(idx - 1 + tabs.length) % tabs.length].id);
  }, [tabs, activeTabId, setActiveTabId]);

  const handleSwitchTab = useCallback(
    (index: number) => {
      const leaf =
        terminalWindows && activeTabId
          ? findTerminalWindowLeafByTabId(terminalWindows, activeTabId)
          : null;
      const tabIds = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const targetTabId = index === -1 ? tabIds[tabIds.length - 1] : tabIds[index];
      if (targetTabId) setActiveTabId(targetTabId);
    },
    [activeTabId, setActiveTabId, tabs, terminalWindows],
  );

  const handleToggleLeftSidebar = useCallback(() => {
    updateUi((prev) => {
      if (prev.active_left_panel) return { active_left_panel: null };
      const first = [
        ...prev.activity_bar_layout.left_top,
        ...prev.activity_bar_layout.left_bottom,
      ].find((id) => !NON_PANEL_IDS.has(id));
      return { active_left_panel: first ?? null };
    });
  }, [updateUi]);

  const handleToggleRightSidebar = useCallback(() => {
    updateUi((prev) => {
      if (prev.active_right_panel) return { active_right_panel: null };
      const first = [
        ...prev.activity_bar_layout.right_top,
        ...prev.activity_bar_layout.right_bottom,
      ].find((id) => !NON_PANEL_IDS.has(id));
      return { active_right_panel: first ?? null };
    });
  }, [updateUi]);

  const { handleZoomIn, handleZoomOut, handleResetZoom } = useTerminalZoom(updateAppSettings);

  const handleOpenSettings = useCallback(() => {
    openSettings();
  }, []);

  const handleLockScreen = useCallback(() => {
    if (appSettings.security.enable_screen_lock) {
      setIsLocked(true);
    }
  }, [appSettings.security.enable_screen_lock, setIsLocked]);

  const handleQuitApplication = useCallback(() => {
    setShowQuitConfirm(false);
    void invoke<void>("quit_application");
  }, []);

  const handleRequestQuit = useCallback(() => {
    if (tabs.length > 0 && appSettings.general.confirm_on_close !== false) {
      setShowQuitConfirm(true);
      return;
    }

    handleQuitApplication();
  }, [appSettings.general.confirm_on_close, handleQuitApplication, tabs.length]);

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
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!canCreateSessionFromPane(pane)) return;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
        );
        const { tabId, createRequestId } = pending;
        setTerminalWindows((current) =>
          current ? insertTabAfterInLeaf(current, tab.id, tabId, tabId) : current,
        );
        try {
          const sessionId = await createSessionForPane(pane, createRequestId);
          if (!hasTab(tabId)) {
            await closeStaleCreatedSession(sessionId);
            return;
          }
          updateTabSession(tabId, sessionId);
          if (pane.connectionId) {
            recordRecentConnection(pane.connectionId);
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
          maybePromptConnectionEdit(pane.connectionId, errorMessage, { sourceTabId: tabId });
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
      updateTabSession,
    ],
  );

  const handleMultiplexSshSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.type !== "SSH" || pane.connecting || pane.connectError) return;

      let tabId: string | undefined;

      try {
        const pending = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
        );
        tabId = pending.tabId;
        setTerminalWindows((current) =>
          current && tabId ? insertTabAfterInLeaf(current, tab.id, tabId, tabId) : current,
        );

        const sessionId = await invoke<string>("create_multiplexed_ssh_session", {
          sourceSessionId: pane.sessionId,
        });
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
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
    [addPendingTab, hasTab, markTabConnectionFailed, recordRecentConnection, t, updateTabSession],
  );

  const handleReconnectSession = useCallback(
    async (tab: Tab) => {
      const pane = getActivePane(tab);
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;

      toast.info(t("tabCtx.reconnecting"));

      try {
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const createRequestId = markPaneConnecting(tab.id, pane.id);
        if (!createRequestId) return;
        const newSessionId = await createSessionForPane(pane, createRequestId);
        if (!hasPane(tab.id, pane.id)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        updatePaneSession(tab.id, pane.id, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
        }
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tab.id, pane.id)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
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
      closePaneBackendSession,
      createSessionForPane,
      hasPane,
      markPaneConnecting,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
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
      if (!tab || !pane || pane.connecting || !canCreateSessionFromPane(pane)) return;

      toast.info(t("tabCtx.reconnecting"));

      try {
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const createRequestId = markPaneConnecting(tab.id, pane.id);
        if (!createRequestId) return;
        const newSessionId = await createSessionForPane(pane, createRequestId);
        if (!hasPane(tab.id, pane.id)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        updatePaneSession(tab.id, pane.id, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
        }
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tab.id, pane.id)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
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
      closePaneBackendSession,
      createSessionForPane,
      hasPane,
      markPaneConnecting,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      tabs,
      updatePaneSession,
    ],
  );

  const handleSplitSession = useCallback(
    async (tab: Tab, direction: PaneSplitDirection) => {
      const pane = getActivePane(tab);
      if (!pane || !canCreateSessionFromPane(pane)) return;
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
        );
        newTabId = pending.tabId;
        setTerminalWindows((current) =>
          current ? splitTerminalWindowForTab(current, tab.id, direction, newTabId) : current,
        );
        const sessionId = await createSessionForPane(pane, pending.createRequestId);
        if (newTabId) {
          if (!hasTab(newTabId)) {
            await closeStaleCreatedSession(sessionId);
            return;
          }
          updateTabSession(newTabId, sessionId);
        }
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
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
      createSessionForPane,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      setActiveTabId,
      t,
      terminalWindows,
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
        ? (collectSessionPanes(tab.root).find((item) => item.id === paneId) ?? null)
        : null;
      if (!pane || pane.connecting || !canCreateSessionFromPane(pane)) return;

      try {
        const closed = await closePaneBackendSession(pane);
        if (!closed) {
          throw new Error("close_session_failed");
        }

        const createRequestId = markPaneConnecting(tabId, paneId);
        if (!createRequestId) return;
        const newSessionId = await createSessionForPane(pane, createRequestId);
        if (!hasPane(tabId, paneId)) {
          await closeStaleCreatedSession(newSessionId);
          return;
        }
        updatePaneSession(tabId, paneId, newSessionId);
        if (pane.connectionId) {
          recordRecentConnection(pane.connectionId);
        }
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasPane(tabId, paneId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "session.reconnect_failed",
          message: "Failed to reconnect pane",
          ids: pane.connectionId ? { connection_id: pane.connectionId } : undefined,
          error,
        });
        markPaneConnectionFailed(tabId, paneId, errorMessage);
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
      markPaneConnecting,
      maybePromptConnectionEdit,
      recordRecentConnection,
      tabs,
      updatePaneSession,
    ],
  );

  const handleCloseSession = useCallback(
    async (tab: Tab) => {
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
    [closePane, closePaneBackendSession, persistWorkspaceNow, t],
  );

  const handleDisconnectSessionById = useCallback(
    async (sessionId: string) => {
      const tab = findTabBySessionId(tabs, sessionId);
      const pane = tab ? findPaneBySessionId(tab, sessionId) : null;

      if (!tab || !pane) {
        try {
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
    if (!window.confirm(t("tabCtx.closeAllConfirm"))) return;

    const results = await Promise.all(tabs.map((tab) => closeWorkspaceTabSessions(tab)));
    const successfulTabIds = tabs.filter((_, index) => results[index]).map((tab) => tab.id);

    if (successfulTabIds.length > 0) {
      closeTabs(successfulTabIds);
      await persistWorkspaceNow(t("tabCtx.closeFailed"));
    }

    if (successfulTabIds.length !== tabs.length) {
      toast.error(t("tabCtx.closeFailed"));
    }
  }, [closeTabs, closeWorkspaceTabSessions, persistWorkspaceNow, t, tabs]);

  const handleCloseInactiveTabs = useCallback(
    async (keepTabId: string) => {
      const leaf = terminalWindows
        ? findTerminalWindowLeafByTabId(terminalWindows, keepTabId)
        : null;
      const targetTabs = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const tabsToClose = tabs.filter((tab) => targetTabs.includes(tab.id) && tab.id !== keepTabId);
      const results = await Promise.all(tabsToClose.map((tab) => closeWorkspaceTabSessions(tab)));

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
      const leaf = terminalWindows ? findTerminalWindowLeafByTabId(terminalWindows, tabId) : null;
      const tabOrder = leaf?.tabIds ?? tabs.map((tab) => tab.id);
      const idx = tabOrder.indexOf(tabId);
      if (idx === -1) return;

      const rightTabIds = tabOrder.slice(idx + 1);
      const tabsToClose = tabs.filter((tab) => rightTabIds.includes(tab.id));
      const results = await Promise.all(tabsToClose.map((tab) => closeWorkspaceTabSessions(tab)));

      const successfulTabIds = tabsToClose
        .filter((_, index) => results[index])
        .map((tab) => tab.id);

      if (successfulTabIds.length > 0) {
        closeTabs(successfulTabIds);
        await persistWorkspaceNow(t("tabCtx.closeFailed"));
      }

      if (successfulTabIds.length !== tabsToClose.length) {
        toast.error(t("tabCtx.closeFailed"));
      }
    },
    [closeTabs, closeWorkspaceTabSessions, persistWorkspaceNow, t, tabs, terminalWindows],
  );

  const handleSessionInfo = useCallback((tab: Tab) => {
    const pane = getActivePane(tab);
    if (pane?.connectionId) {
      openNewSession(pane.connectionId);
    }
  }, []);

  const handleOpenChat = useCallback(() => {
    if (!isLocked) {
      updateUi({ active_right_panel: "aiAssistant" });
    }
  }, [isLocked, updateUi]);

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

  useGlobalShortcuts(
    {
      onNewSession: () => handleNewSession(),
      onOpenSessionSwitcher: handleOpenSessionSwitcher,
      onNewLocalTerminal: handleNewLocalTerminal,
      onCloseTab: handleCloseActiveTab,
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
      onClearTerminal: () => window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
    },
    appSettings.keybindings,
  );

  const buildRecordingFilePath = useCallback(
    async (prefix: "recording" | "session", sessionName: string) => {
      const dir = appSettings.transfer.recording_path || (await downloadDir());
      const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
      return joinPath(dir, `${prefix}-${safeRecordingName(sessionName)}-${timestamp}.log`);
    },
    [appSettings.transfer.recording_path],
  );

  const handleToggleSessionRecording = useCallback(
    async (session: SessionInfo) => {
      const sessionId = session.id;
      const isActive = recordingSessions.has(sessionId);

      if (isActive) {
        try {
          const savedPath = await invoke<string>("stop_recording", { sessionId });
          setRecordingSessions((prev) => {
            const next = new Set(prev);
            next.delete(sessionId);
            return next;
          });
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
        const filePath = await buildRecordingFilePath("recording", session.name);
        await invoke("start_recording", {
          sessionId,
          filePath,
          includeIoLabels: appSettings.transfer.recording_include_io_labels,
          includeTimestamps: appSettings.transfer.recording_include_timestamps ?? true,
        });
        setRecordingSessions((prev) => {
          const next = new Set(prev);
          next.add(sessionId);
          return next;
        });
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
    [
      appSettings.transfer.recording_include_io_labels,
      appSettings.transfer.recording_include_timestamps,
      buildRecordingFilePath,
      recordingSessions,
      t,
    ],
  );

  const handleSaveSessionTranscript = useCallback(
    async (session: SessionInfo) => {
      try {
        const filePath = await buildRecordingFilePath("session", session.name);
        const savedPath = await invoke<string>("save_session_transcript", {
          sessionId: session.id,
          filePath,
          includeIoLabels: appSettings.transfer.recording_include_io_labels,
          includeTimestamps: appSettings.transfer.recording_include_timestamps ?? true,
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
      appSettings.transfer.recording_include_io_labels,
      appSettings.transfer.recording_include_timestamps,
      buildRecordingFilePath,
      t,
    ],
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
    showLabels,
    toggleActiveIds,
    handleItemSelect,
    handleReorder,
    handleMoveItem,
    handleToggleLabel,
  } = useActivityBarController({
    uiConfig,
    recordingSessions,
    updateUi,
    setIsLocked,
    t,
  });

  // --- Panel content rendering (side-independent) ---

  const activeSessionId =
    activePane && !activePane.connecting && !activePane.connectError ? activePane.sessionId : null;
  const activeSshSessionId =
    activePane && !activePane.connecting && !activePane.connectError && activePane.type === "SSH"
      ? activePane.sessionId
      : null;
  const activeSerialSessionId =
    activePane && !activePane.connecting && !activePane.connectError && activePane.type === "Serial"
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
  const activeBottomPanel = uiConfig.show_serial_send_panel
    ? "serialSend"
    : uiConfig.show_quick_cmd_bar
      ? "quickCmdBar"
      : null;
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
        const connection = pane.connectionId ? connectionsById.get(pane.connectionId) : undefined;
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

      const pending = addPendingTab(
        connection.name,
        getConnectionSessionType(connection),
        connection.id,
      );
      const { tabId, createRequestId } = pending;

      try {
        const sessionId = await createSessionForConnection(connection, createRequestId);
        if (!hasTab(tabId)) {
          await closeStaleCreatedSession(sessionId);
          return;
        }
        updateTabSession(tabId, sessionId);
        focusTerminalSession(sessionId);
        recordRecentConnection(connection.id);
      } catch (error) {
        if (isSessionCreationCancelled(error) || !hasTab(tabId)) {
          return;
        }
        const errorMessage = getErrorMessage(error);
        logger.error({
          domain: "session.lifecycle",
          event: "connection.open_failed",
          message: "Connection failed from quick switcher",
          ids: { connection_id: connection.id },
          error,
        });
        markTabConnectionFailed(tabId, errorMessage);
        maybePromptConnectionEdit(connection.id, errorMessage, { sourceTabId: tabId });
        toast.error(t("savedConnections.connectionFailed", { error: errorMessage }));
      }
    },
    [
      addPendingTab,
      hasTab,
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      recordRecentConnection,
      t,
      updateTabSession,
    ],
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

  const renderPanelContent = useCallback(
    (panelId: string | null) => (
      <AppPanelContent
        panelId={panelId}
        activePane={activePane}
        activeConnection={activeConnection}
        activeSessionId={activeSessionId}
        activeSshSessionId={activeSshSessionId}
        recordingSessions={recordingSessions}
        aiIntent={aiIntent}
        transferHeight={uiConfig.transfer_height || 180}
        onTransferResize={handleTransferResize}
        onNewConnection={handleNewSession}
        onEditConnection={handleEditConnection}
        onSessionClick={handleSessionClick}
        onSessionReconnect={handleReconnectSessionById}
        onSessionDisconnect={handleDisconnectSessionById}
        canReconnect={canReconnectSessionById}
        onCommandSend={handleHistoryCommand}
        onToggleSessionRecording={handleToggleSessionRecording}
        onSaveSessionTranscript={handleSaveSessionTranscript}
      />
    ),
    [
      activeConnection,
      activePane,
      activeSessionId,
      activeSshSessionId,
      aiIntent,
      canReconnectSessionById,
      handleSaveSessionTranscript,
      handleDisconnectSessionById,
      handleEditConnection,
      handleHistoryCommand,
      handleNewSession,
      handleReconnectSessionById,
      handleSessionClick,
      handleToggleSessionRecording,
      handleTransferResize,
      recordingSessions,
      uiConfig.transfer_height,
    ],
  );

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
          onSmartSplit: handleSmartSplit,
          onManageSyncGroups: () => setShowSyncGroupDialog(true),
          onBroadcastToAll: () => setBroadcastToAll((prev) => !prev),
          broadcastToAll,
          onClearTerminal: () => window.dispatchEvent(new CustomEvent("nyaterm:clear-terminal")),
          onResetTerminalSize: () =>
            window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals")),
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
        workspace={{
          layout: terminalWindows,
          tabsById,
          focusedTabId: activeTabId,
          unreadTabIds,
          onSelectTab: handleSelectLeafTab,
          onAddTab: handleAddTabFromLeaf,
          onConnectConnection: handleConnectConnectionFromLeaf,
          onTabClose: handleCloseWorkspaceTab,
          onDuplicateSession: handleDuplicateSession,
          onMultiplexSshSession: handleMultiplexSshSession,
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
          onActivatePane: handleActivatePane,
          onUpdatePaneSplitRatio: handleUpdatePaneSplitRatio,
          onUpdateWindowSplitRatio: handleUpdateWindowSplitRatio,
          onReconnectPane: handleReconnectPane,
          onReconnected: handleReconnected,
          onDisconnectedCloseRequested: handleCloseDisconnectedPane,
        }}
        tabsCount={tabs.length}
        emptyWorkspace={{
          openChatShortcut,
          showCommandsShortcut,
          switchTerminalShortcut,
          onOpenChat: handleOpenChat,
          onShowCommands: handleShowAllCommands,
          onSwitchTerminal: handleOpenSessionSwitcher,
        }}
        bottomPanel={{
          activePanel: activeBottomPanel,
          quickCmdHeight: uiConfig.quick_cmd_height || 180,
          serialSendHeight: uiConfig.serial_send_height || 180,
          activeSerialSessionId,
          activeNonSerialSessionId,
          activeNonSerialSessionIds,
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
          otpRequest,
          onOtpDone: () => setOtpRequest(null),
          hostKeyVerifyRequest,
          onHostKeyVerifyDone: () => setHostKeyVerifyRequest(null),
          modalChildWindowCount,
          locked: isLocked,
          hasMasterPassword: !!appSettings.security.master_password,
          onUnlock: () => setIsLocked(false),
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
    </TransferProvider>
  );
}

export default App;

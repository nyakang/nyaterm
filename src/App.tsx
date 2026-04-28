import { listen } from "@tauri-apps/api/event";
import { downloadDir } from "@tauri-apps/api/path";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BiServer } from "react-icons/bi";
import { FaRegFolder } from "react-icons/fa";
import { LuKeyRound } from "react-icons/lu";
import {
  MdAutoAwesome,
  MdBackup,
  MdBolt,
  MdClose,
  MdHistory,
  MdLan,
  MdLink,
  MdLock,
  MdOutlineMonitorHeart,
  MdSend,
  MdSettings,
  MdTerminal,
} from "react-icons/md";
import { PiRecordFill } from "react-icons/pi";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import AboutDialog from "./components/dialog/app/AboutDialog";
import LockScreen from "./components/dialog/app/LockScreen";
import QuitConfirmDialog from "./components/dialog/app/QuitConfirmDialog";
import UpdateDialog from "./components/dialog/app/UpdateDialog";
import {
  HostKeyVerifyDialog,
  type HostKeyVerifyRequest,
} from "./components/dialog/connections/HostKeyVerifyDialog";
import { OtpDialog, type OtpRequest } from "./components/dialog/connections/OtpDialog";
import SyncGroupDialog from "./components/dialog/terminal/SyncGroupDialog";
import type { ActivityBarItem } from "./components/layout/ActivityBar";
import ActivityBar from "./components/layout/ActivityBar";
import Header from "./components/layout/Header";
import ResizeHandle from "./components/layout/ResizeHandle";
import ActiveSessions from "./components/panel/ActiveSessions";
import AIAssistantPanel from "./components/panel/AIAssistantPanel";
import CommandHistory from "./components/panel/CommandHistory";
import FileExplorer from "./components/panel/file-explorer";
import FileTransfer from "./components/panel/file-explorer/FileTransfer";
import NetworkPanel from "./components/panel/NetworkPanel";
import QuickCommands from "./components/panel/QuickCommands";
import ResourceMonitor from "./components/panel/ResourceMonitor";
import SerialSendPanel from "./components/panel/SendCommandPanel";
import SyncBackupHistoryPanel from "./components/panel/SyncBackupHistoryPanel";
import SavedConnections from "./components/panel/saved-connections";
import SecurityAuthPanel from "./components/panel/security-auth";
import TabWindowsWorkspace from "./components/terminal/TabWindowsWorkspace";
import { useApp } from "./context/AppContext";
import { TransferProvider } from "./context/TransferContext";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { AI_OPEN_EVENT, type AIOpenIntent } from "./lib/aiEvents";
import { getErrorMessage, shouldPromptConnectionEditOnFailure } from "./lib/errors";
import { invoke } from "./lib/invoke";
import { logger } from "./lib/logger";
import { clearSessionCommandHistory, sendSessionInput } from "./lib/sessionInput";
import { buildSmartSplitLayout, type SmartSplitMode } from "./lib/smartSplit";
import { purgeSessionFromGroups } from "./lib/syncInputGroups";
import {
  findTerminalWindowLeafById,
  findTerminalWindowLeafByTabId,
  insertTabAfterInLeaf,
  insertTabIntoLeaf,
  reconcileTerminalWindows,
  reorderTabsInLeaf,
  setLeafActiveTab,
  splitTerminalWindowForTab,
  type TerminalWindowNode,
  updateTerminalWindowSplitRatio,
} from "./lib/tabWindows";
import {
  DEFAULT_TERMINAL_FONT_SIZE,
  decreaseTerminalFontSize,
  increaseTerminalFontSize,
} from "./lib/terminalFontSize";
import { checkForUpdate, type UpdateInfo } from "./lib/updater";
import {
  bounceTopModalWindow,
  isModalChildLabel,
  type NewSessionTarget,
  openNewSession,
  openNewSessionWithTarget,
  openSettings,
  syncMainWindowModalState,
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
  ActivityBarLayout,
  ActivityBarZone,
  AppSettings,
  CloudConflictPreview,
  PaneSplitDirection,
  SavedConnection,
  SessionPane,
  Tab,
} from "./types/global";

/** Item IDs that are not regular panels — they have special action on click. */
const NON_PANEL_IDS = new Set(["settings", "lock", "quickCmdBar", "serialSend", "recording"]);

type TrayAction =
  | { type: "open_new_session" }
  | { type: "focus_session"; sessionId: string }
  | { type: "open_panel"; panelId: "activeSessions" | "syncBackupHistory" }
  | { type: "open_settings" }
  | { type: "lock_screen" }
  | { type: "check_updates" }
  | { type: "request_quit" };

function canCreateSessionFromPane(
  pane: Pick<SessionPane, "type" | "connectionId"> | null | undefined,
): pane is Pick<SessionPane, "type" | "connectionId"> {
  return !!pane && (pane.type === "Local" || !!pane.connectionId);
}

function hasLiveSession<T extends Pick<SessionPane, "connecting" | "connectError">>(
  pane: T | null | undefined,
): pane is T {
  return !!pane && !pane.connecting && !pane.connectError;
}

/** Determine which visual side (left/right) a given item currently lives on. */
function getItemSide(id: string, layout: ActivityBarLayout): "left" | "right" | null {
  if (layout.left_top.includes(id) || layout.left_bottom.includes(id)) return "left";
  if (layout.right_top.includes(id) || layout.right_bottom.includes(id)) return "right";
  return null;
}

function collectActiveShellSessionIds(
  layout: TerminalWindowNode | null,
  tabsById: Map<string, Tab>,
) {
  if (!layout) return [];

  const sessionIds = new Set<string>();

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
        if (hasLiveSession(pane) && pane.type === "SSH") {
          sessionIds.add(pane.sessionId);
        }
      }
    }
  };

  visit(layout);
  return [...sessionIds];
}

const CTRL_WHEEL_ZOOM_THROTTLE_MS = 50;

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
    closePane,
    updateSplitRatio,
    persistTabsNow,
    updateUi,
    updateAppSettings,
    replaceAppSettings,
    appSettings,
    closeTabs,
    savedConnections,
    setSyncGroups,
    broadcastToAll,
    setBroadcastToAll,
    isLocked,
    setIsLocked,
    settingsLoaded,
  } = useApp();
  const uiConfig = appSettings.ui;
  const { t, i18n } = useTranslation();

  useEffect(() => {
    if (!settingsLoaded) return;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow().show();
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
  const lastCtrlWheelZoomAtRef = useRef(0);

  // Recording state: tracks which sessions are currently being recorded
  const [recordingSessions, setRecordingSessions] = useState<Set<string>>(new Set());

  // Unread output tracking: session IDs with unread terminal output
  const [unreadSessionIds, setUnreadSessionIds] = useState<Set<string>>(new Set());

  // Track only modal child windows that should block the main window.
  const [modalChildWindowCount, setModalChildWindowCount] = useState(0);

  // OTP / 2FA dialog state
  const [otpRequest, setOtpRequest] = useState<OtpRequest | null>(null);
  const [hostKeyVerifyRequest, setHostKeyVerifyRequest] = useState<HostKeyVerifyRequest | null>(
    null,
  );
  const lastCloudConflictRevisionRef = useRef<string | null>(null);

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_screen_lock ? appSettings.security.idle_lock_minutes : 0,
    () => setIsLocked(true),
  );

  // Background update check on startup
  useEffect(() => {
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
  }, []);

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
      }>("session-created", (event) => {
        const { sessionId, name: sessionName, type, targetLeafId, anchorTabId } = event.payload;
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
      }),
    );

    unsubs.push(
      listen<OtpRequest>("otp-request", (event) => {
        setOtpRequest(event.payload);
      }),
    );

    unsubs.push(
      listen<HostKeyVerifyRequest>("host-key-verify", (event) => {
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
      }>("session-connect-after-edit", async (event) => {
        const { connectionId, targetLeafId, anchorTabId, sourceTabId, sourcePaneId } =
          event.payload;
        try {
          const conns = await invoke<SavedConnection[]>("get_saved_connections");
          const conn = conns.find((c) => c.id === connectionId);
          const connName = conn?.name ?? connectionId;
          const typeMap: Record<string, "SSH" | "Local" | "Telnet" | "Serial"> = {
            ssh: "SSH",
            local_terminal: "Local",
            telnet: "Telnet",
            serial: "Serial",
          };
          const sessionType = typeMap[conn?.type ?? "ssh"] ?? "SSH";
          const sourceTab = sourceTabId
            ? (tabsRef.current.find((item) => item.id === sourceTabId) ?? null)
            : null;
          const sourcePane =
            sourceTab &&
            ((sourcePaneId ? findSessionPaneById(sourceTab.root, sourcePaneId) : null) ??
              getActivePane(sourceTab));
          let tabId: string;
          let paneId: string | undefined;

          if (sourceTab && sourcePane) {
            tabId = sourceTab.id;
            paneId = sourcePane.id;
            setActiveTabId(tabId);
            setActivePane(tabId, paneId);
            markPaneConnecting(tabId, paneId, {
              name: connName,
              type: sessionType,
              connectionId,
            });
          } else {
            tabId = addPendingTab(
              connName,
              sessionType,
              connectionId,
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
          }

          try {
            let sessionId: string;
            switch (conn?.type) {
              case "local_terminal":
                sessionId = await invoke<string>("create_local_session", { connectionId });
                break;
              case "telnet":
                sessionId = await invoke<string>("create_telnet_session", { connectionId });
                break;
              case "serial":
                sessionId = await invoke<string>("create_serial_session", { connectionId });
                break;
              default:
                sessionId = await invoke<string>("create_ssh_session", { connectionId });
                break;
            }
            if (paneId) {
              updatePaneSession(tabId, paneId, sessionId);
            } else {
              updateTabSession(tabId, sessionId);
            }
          } catch (error) {
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
    markPaneConnecting,
    markPaneConnectionFailed,
    markTabConnectionFailed,
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

  // Track modal child window open/close for overlay and focus enforcement.
  useEffect(() => {
    const unsubs = [
      listen<{ label: string }>("child-window-opened", ({ payload }) => {
        if (!isModalChildLabel(payload.label)) return;
        setModalChildWindowCount((c) => c + 1);
        void syncMainWindowModalState();
      }),
      listen<{ label: string }>("child-window-closed", ({ payload }) => {
        if (!isModalChildLabel(payload.label)) return;
        setModalChildWindowCount((c) => Math.max(0, c - 1));
        void syncMainWindowModalState();
      }),
    ];
    return () => {
      unsubs.forEach((p) => {
        p.then((unsub) => unsub());
      });
    };
  }, []);

  useEffect(() => {
    let unlistenFocusChanged: (() => void) | undefined;

    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow()
        .onFocusChanged(({ payload: focused }) => {
          if (!focused || modalChildWindowCount === 0) return;
          void syncMainWindowModalState();
          void bounceTopModalWindow();
        })
        .then((unlisten) => {
          unlistenFocusChanged = unlisten;
        })
        .catch(() => {});
    });

    return () => {
      unlistenFocusChanged?.();
    };
  }, [modalChildWindowCount]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
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

  const handleNewSession = useCallback((_parentGroupId?: string) => {
    openNewSession();
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

  // Listen for background session output and mark as unread
  useEffect(() => {
    const handler = (e: Event) => {
      const { sessionId } = (e as CustomEvent<{ sessionId: string }>).detail;
      const tab = findTabBySessionId(tabs, sessionId);
      if (tab && tab.id !== activeTabId) {
        setUnreadSessionIds((prev) => {
          if (prev.has(sessionId)) return prev;
          const next = new Set(prev);
          next.add(sessionId);
          return next;
        });
      }
    };
    window.addEventListener("dragonfly:session-output", handler);
    return () => window.removeEventListener("dragonfly:session-output", handler);
  }, [tabs, activeTabId]);

  // Clear unread state when switching to a tab
  useEffect(() => {
    if (!activeTabId) return;
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab) return;
    const panes = collectSessionPanes(tab.root);
    const paneSessionIds = new Set(panes.map((p) => p.sessionId));
    setUnreadSessionIds((prev) => {
      const hasAny = [...prev].some((id) => paneSessionIds.has(id));
      if (!hasAny) return prev;
      const next = new Set(prev);
      for (const id of paneSessionIds) next.delete(id);
      return next;
    });
  }, [activeTabId, tabs]);

  const unreadTabIds = useMemo(() => {
    const result = new Set<string>();
    for (const sessionId of unreadSessionIds) {
      const tab = findTabBySessionId(tabs, sessionId);
      if (tab) result.add(tab.id);
    }
    return result;
  }, [unreadSessionIds, tabs]);

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

  const handleReorderTabsInLeaf = useCallback((_: string, fromTabId: string, toIndex: number) => {
    setTerminalWindows((current) =>
      current ? reorderTabsInLeaf(current, fromTabId, toIndex) : current,
    );
  }, []);

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
    async (pane: Pick<SessionPane, "type" | "connectionId">) => {
      switch (pane.type) {
        case "Local":
          return invoke<string>("create_local_session", {
            connectionId: pane.connectionId || null,
          });
        case "Telnet":
          if (!pane.connectionId) throw new Error("Missing Telnet connection id");
          return invoke<string>("create_telnet_session", { connectionId: pane.connectionId });
        case "Serial":
          if (!pane.connectionId) throw new Error("Missing Serial connection id");
          return invoke<string>("create_serial_session", { connectionId: pane.connectionId });
        default:
          if (!pane.connectionId) throw new Error("Missing SSH connection id");
          return invoke<string>("create_ssh_session", { connectionId: pane.connectionId });
      }
    },
    [],
  );

  const closePaneBackendSession = useCallback(
    async (pane: Pick<SessionPane, "connecting" | "connectError" | "sessionId">) => {
      if (pane.connecting || pane.connectError) {
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
    [],
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
        void sendSessionInput(sessionId, execute ? `${command}\r` : command, {
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
          void sendSessionInput(sessionId, `${command}\r`, {
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
      if (tabs.length === 0) return;
      const target = index === -1 ? tabs[tabs.length - 1] : tabs[index];
      if (target) setActiveTabId(target.id);
    },
    [tabs, setActiveTabId],
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

  const handleZoomIn = useCallback(() => {
    updateAppSettings((prev) => ({
      appearance: {
        ...prev.appearance,
        font_size: increaseTerminalFontSize(prev.appearance.font_size),
      },
    }));
  }, [updateAppSettings]);

  const handleZoomOut = useCallback(() => {
    updateAppSettings((prev) => ({
      appearance: {
        ...prev.appearance,
        font_size: decreaseTerminalFontSize(prev.appearance.font_size),
      },
    }));
  }, [updateAppSettings]);

  const handleResetZoom = useCallback(() => {
    updateAppSettings((prev) => ({
      appearance: { ...prev.appearance, font_size: DEFAULT_TERMINAL_FONT_SIZE },
    }));
  }, [updateAppSettings]);

  useEffect(() => {
    const handleCtrlWheelZoom = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      if (event.deltaY === 0) return;

      event.preventDefault();
      const now = Date.now();
      if (now - lastCtrlWheelZoomAtRef.current < CTRL_WHEEL_ZOOM_THROTTLE_MS) return;
      lastCtrlWheelZoomAtRef.current = now;

      if (event.deltaY < 0) {
        handleZoomIn();
      } else {
        handleZoomOut();
      }
    };

    window.addEventListener("wheel", handleCtrlWheelZoom, { passive: false, capture: true });
    return () => {
      window.removeEventListener("wheel", handleCtrlWheelZoom, true);
    };
  }, [handleZoomIn, handleZoomOut]);

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
        const tabId = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
        );
        setTerminalWindows((current) =>
          current ? insertTabAfterInLeaf(current, tab.id, tabId, tabId) : current,
        );
        try {
          const sessionId = await createSessionForPane(pane);
          updateTabSession(tabId, sessionId);
        } catch (error) {
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
      markTabConnectionFailed,
      maybePromptConnectionEdit,
      t,
      updateTabSession,
    ],
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

        const newSessionId = await createSessionForPane(pane);
        updatePaneSession(tab.id, pane.id, newSessionId);
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
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
      maybePromptConnectionEdit,
      t,
      updatePaneSession,
    ],
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

        const newSessionId = await createSessionForPane(pane);
        updatePaneSession(tab.id, pane.id, newSessionId);
        toast.success(t("tabCtx.reconnectSuccess"));
      } catch (error) {
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
      maybePromptConnectionEdit,
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
        window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"));
        return;
      }

      let newTabId: string | undefined;

      try {
        newTabId = addPendingTab(
          pane.name,
          pane.type,
          pane.connectionId,
          { customName: tab.customName, tabColor: tab.tabColor },
          { afterTabId: tab.id },
        );
        setTerminalWindows((current) =>
          current ? splitTerminalWindowForTab(current, tab.id, direction, newTabId) : current,
        );
        const sessionId = await createSessionForPane(pane);
        if (newTabId) {
          updateTabSession(newTabId, sessionId);
        }
        window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"));
      } catch (error) {
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
      markTabConnectionFailed,
      maybePromptConnectionEdit,
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
        window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"));
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

        const newSessionId = await createSessionForPane(pane);
        updatePaneSession(tabId, paneId, newSessionId);
      } catch (error) {
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
      markPaneConnectionFailed,
      maybePromptConnectionEdit,
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

  useGlobalShortcuts({
    onNewSession: () => handleNewSession(),
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
    onLockScreen: handleLockScreen,
    onManageSyncGroups: () => setShowSyncGroupDialog(true),
    onClearTerminal: () => window.dispatchEvent(new CustomEvent("dragonfly:clear-terminal")),
  });

  // Recording toggle
  const handleToggleRecording = useCallback(async () => {
    if (!activeTab || !activePane || activePane.connecting || activePane.connectError) return;
    const sessionId = activePane.sessionId;
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
      } catch (e) {
        logger.error({
          domain: "session.lifecycle",
          event: "recording.stop_failed",
          message: "Failed to stop recording",
          ids: { session_id: sessionId },
          error: e,
        });
      }
    } else {
      try {
        const dir = appSettings.transfer.recording_path || (await downloadDir());
        const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
        const safeName = getTabDisplayName(activeTab).replace(/[^\w.-]/g, "_");
        const filePath = `${dir}${dir.endsWith("\\") || dir.endsWith("/") ? "" : "/"}recording-${safeName}-${timestamp}.log`;
        await invoke("start_recording", { sessionId, filePath });
        setRecordingSessions((prev) => {
          const next = new Set(prev);
          next.add(sessionId);
          return next;
        });
      } catch (e) {
        logger.error({
          domain: "session.lifecycle",
          event: "recording.start_failed",
          message: "Failed to start recording",
          ids: { session_id: sessionId },
          error: e,
        });
      }
    }
  }, [activePane, activeTab, appSettings.transfer.recording_path, recordingSessions, t]);

  // Resize handlers
  const handleLeftResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        left_width: Math.max(160, Math.min(480, (prev.left_width || 256) + delta)),
      }));
    },
    [updateUi],
  );

  const handleRightResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        right_width: Math.max(200, Math.min(480, (prev.right_width || 288) - delta)),
      }));
    },
    [updateUi],
  );

  const handleQuickCmdResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        quick_cmd_height: Math.max(36, Math.min(300, (prev.quick_cmd_height || 36) - delta)),
      }));
    },
    [updateUi],
  );

  const handleSerialSendResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        serial_send_height: Math.max(60, Math.min(300, (prev.serial_send_height || 120) - delta)),
      }));
    },
    [updateUi],
  );

  // --- Activity bar item registry & dynamic zone arrays ---

  const itemRegistry = useMemo<Record<string, { icon: ReactNode; tooltip: string }>>(
    () => ({
      fileExplorer: { icon: <FaRegFolder />, tooltip: t("panel.fileExplorer") },
      network: { icon: <MdLan />, tooltip: t("panel.network") },
      securityAuth: { icon: <LuKeyRound />, tooltip: t("securityAuth.title") },
      syncBackupHistory: { icon: <MdBackup />, tooltip: t("panel.syncBackupHistory") },
      settings: { icon: <MdSettings />, tooltip: t("settings.title") },
      savedConnections: { icon: <BiServer />, tooltip: t("panel.savedConnections") },
      aiAssistant: { icon: <MdAutoAwesome />, tooltip: t("ai.title") },
      activeSessions: { icon: <MdLink />, tooltip: t("panel.activeSessions") },
      commandHistory: { icon: <MdHistory />, tooltip: t("panel.commandHistory") },
      resourceMonitor: { icon: <MdOutlineMonitorHeart />, tooltip: t("panel.resourceMonitor") },
      quickCmdBar: { icon: <MdBolt />, tooltip: t("panel.quickCommands") },
      serialSend: { icon: <MdSend />, tooltip: t("panel.serialSend", "Command Send") },
      recording: {
        icon: (
          <PiRecordFill
            className={
              activePane && recordingSessions.has(activePane.sessionId)
                ? "animate-pulse"
                : undefined
            }
          />
        ),
        tooltip:
          activePane && recordingSessions.has(activePane.sessionId)
            ? t("recording.stop")
            : t("recording.start"),
      },
      lock: { icon: <MdLock />, tooltip: t("statusBar.lock") },
    }),
    [activePane, recordingSessions, t],
  );

  const layout = uiConfig.activity_bar_layout;

  useEffect(() => {
    const allIds = [
      ...layout.left_top,
      ...layout.left_bottom,
      ...layout.right_top,
      ...layout.right_bottom,
    ];
    const needsSyncBackupHistory = !allIds.includes("syncBackupHistory");
    const needsAiAssistant = !allIds.includes("aiAssistant");
    const needsSerialSend = !allIds.includes("serialSend");
    const needsRecording = !allIds.includes("recording");
    if (!needsSyncBackupHistory && !needsAiAssistant && !needsSerialSend && !needsRecording) return;

    updateUi((prev) => {
      const nextLeftBottom = [...prev.activity_bar_layout.left_bottom];
      const nextRightTop = [...prev.activity_bar_layout.right_top];
      const nextRightBottom = [...prev.activity_bar_layout.right_bottom];

      if (!nextLeftBottom.includes("syncBackupHistory")) {
        const settingsIndex = nextLeftBottom.indexOf("settings");
        if (settingsIndex !== -1) {
          nextLeftBottom.splice(settingsIndex, 0, "syncBackupHistory");
        } else {
          nextLeftBottom.push("syncBackupHistory");
        }
      }

      if (!nextRightTop.includes("aiAssistant")) {
        const savedConnectionsIndex = nextRightTop.indexOf("savedConnections");
        if (savedConnectionsIndex !== -1) {
          nextRightTop.splice(savedConnectionsIndex + 1, 0, "aiAssistant");
        } else {
          nextRightTop.unshift("aiAssistant");
        }
      }

      if (!nextRightBottom.includes("serialSend")) {
        const quickCmdIndex = nextRightBottom.indexOf("quickCmdBar");
        const recordingIndex = nextRightBottom.indexOf("recording");
        const lockIndex = nextRightBottom.indexOf("lock");
        if (quickCmdIndex !== -1) {
          nextRightBottom.splice(quickCmdIndex + 1, 0, "serialSend");
        } else if (recordingIndex !== -1) {
          nextRightBottom.splice(recordingIndex, 0, "serialSend");
        } else if (lockIndex !== -1) {
          nextRightBottom.splice(lockIndex, 0, "serialSend");
        } else {
          nextRightBottom.push("serialSend");
        }
      }

      if (!nextRightBottom.includes("recording")) {
        const serialSendIndex = nextRightBottom.indexOf("serialSend");
        const lockIndex = nextRightBottom.indexOf("lock");
        if (serialSendIndex !== -1) {
          nextRightBottom.splice(serialSendIndex + 1, 0, "recording");
        } else if (lockIndex !== -1) {
          nextRightBottom.splice(lockIndex, 0, "recording");
        } else {
          nextRightBottom.push("recording");
        }
      }

      return {
        activity_bar_layout: {
          ...prev.activity_bar_layout,
          left_bottom: nextLeftBottom,
          right_top: nextRightTop,
          right_bottom: nextRightBottom,
        },
      };
    });
  }, [layout.left_bottom, layout.left_top, layout.right_bottom, layout.right_top, updateUi]);

  const buildItems = useCallback(
    (ids: string[]): ActivityBarItem[] =>
      ids.filter((id) => id in itemRegistry).map((id) => ({ id, ...itemRegistry[id] })),
    [itemRegistry],
  );

  const leftTopItems = useMemo(() => buildItems(layout.left_top), [buildItems, layout.left_top]);
  const leftBottomItems = useMemo(
    () => buildItems(layout.left_bottom),
    [buildItems, layout.left_bottom],
  );
  const rightTopItems = useMemo(() => buildItems(layout.right_top), [buildItems, layout.right_top]);
  const rightBottomItems = useMemo(
    () => buildItems(layout.right_bottom),
    [buildItems, layout.right_bottom],
  );

  const showLabels = layout.show_labels;

  const toggleActiveIds = useMemo(() => {
    const s = new Set<string>();
    if (uiConfig.show_quick_cmd_bar) s.add("quickCmdBar");
    if (uiConfig.show_serial_send_panel) s.add("serialSend");
    if (activePane && recordingSessions.has(activePane.sessionId)) s.add("recording");
    return s;
  }, [activePane, recordingSessions, uiConfig.show_quick_cmd_bar, uiConfig.show_serial_send_panel]);

  useEffect(() => {
    if (!uiConfig.show_quick_cmd_bar || !uiConfig.show_serial_send_panel) return;
    updateUi({ show_quick_cmd_bar: false });
  }, [uiConfig.show_quick_cmd_bar, uiConfig.show_serial_send_panel, updateUi]);

  // Unified item select — routes to left or right panel based on current layout position
  const handleItemSelect = useCallback(
    (id: string) => {
      if (id === "settings") {
        openSettings();
        return;
      }
      if (id === "lock") {
        setIsLocked(true);
        return;
      }
      if (id === "quickCmdBar") {
        updateUi((prev) => ({
          show_quick_cmd_bar: !prev.show_quick_cmd_bar,
          ...(prev.show_serial_send_panel ? { show_serial_send_panel: false } : {}),
        }));
        return;
      }
      if (id === "serialSend") {
        updateUi((prev) => ({
          show_serial_send_panel: !prev.show_serial_send_panel,
          ...(prev.show_quick_cmd_bar ? { show_quick_cmd_bar: false } : {}),
        }));
        return;
      }
      if (id === "recording") {
        if (!activePane || activePane.connecting || activePane.connectError) {
          toast.error(t("panel.noActiveSessions"));
          return;
        }
        void handleToggleRecording();
        return;
      }
      const side = getItemSide(id, layout);
      if (side === "left") {
        updateUi((prev) => ({ active_left_panel: prev.active_left_panel === id ? null : id }));
      } else if (side === "right") {
        updateUi((prev) => ({ active_right_panel: prev.active_right_panel === id ? null : id }));
      }
    },
    [activePane, handleToggleRecording, layout, setIsLocked, t, updateUi],
  );

  // Reorder within a zone — uses prev to avoid stale closure
  const handleReorder = useCallback(
    (side: "left" | "right", zoneKey: "top" | "bottom", orderedIds: string[]) => {
      const layoutKey = `${side}_${zoneKey}` as keyof ActivityBarLayout;
      updateUi((prev) => ({
        activity_bar_layout: { ...prev.activity_bar_layout, [layoutKey]: orderedIds },
      }));
    },
    [updateUi],
  );

  // Move item between zones; clear its active-panel state if it crosses sides
  const handleMoveItem = useCallback(
    (itemId: string, targetZone: ActivityBarZone) => {
      updateUi((prev) => {
        const zones = ["left_top", "left_bottom", "right_top", "right_bottom"] as const;
        const newLayout = { ...prev.activity_bar_layout };
        for (const z of zones) {
          newLayout[z] = newLayout[z].filter((id) => id !== itemId);
        }
        newLayout[targetZone] = [...newLayout[targetZone], itemId];
        const isMovingToRight = targetZone === "right_top" || targetZone === "right_bottom";
        const isMovingToLeft = targetZone === "left_top" || targetZone === "left_bottom";
        return {
          activity_bar_layout: newLayout,
          ...(prev.active_left_panel === itemId && isMovingToRight
            ? { active_left_panel: null }
            : {}),
          ...(prev.active_right_panel === itemId && isMovingToLeft
            ? { active_right_panel: null }
            : {}),
        };
      });
    },
    [updateUi],
  );

  // Toggle global "show labels" setting
  const handleToggleLabel = useCallback(() => {
    updateUi((prev) => ({
      activity_bar_layout: {
        ...prev.activity_bar_layout,
        show_labels: !prev.activity_bar_layout.show_labels,
      },
    }));
  }, [updateUi]);

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
  const activeShellSessionIds = useMemo(
    () => collectActiveShellSessionIds(terminalWindows, tabsById),
    [tabsById, terminalWindows],
  );
  const activeBottomPanel = uiConfig.show_serial_send_panel
    ? "serialSend"
    : uiConfig.show_quick_cmd_bar
      ? "quickCmdBar"
      : null;

  const handleTransferResize = useCallback(
    (delta: number) => {
      updateUi((prev) => ({
        transfer_height: Math.max(60, Math.min(400, (prev.transfer_height || 180) - delta)),
      }));
    },
    [updateUi],
  );

  function renderPanelContent(id: string | null) {
    switch (id) {
      case "fileExplorer":
        return (
          <div className="h-full flex flex-col overflow-hidden">
            <div className="flex-1 min-h-0 overflow-hidden">
              <FileExplorer
                activeSessionId={activeSessionId}
                activeSessionType={
                  activePane && !activePane.connecting && !activePane.connectError
                    ? activePane.type
                    : null
                }
              />
            </div>
            <ResizeHandle direction="vertical" onResize={handleTransferResize} />
            <div
              style={{ height: uiConfig.transfer_height || 180 }}
              className="shrink-0 overflow-hidden"
            >
              <FileTransfer activeSessionId={activeSessionId} />
            </div>
          </div>
        );
      case "network":
        return <NetworkPanel />;
      case "securityAuth":
        return <SecurityAuthPanel />;
      case "syncBackupHistory":
        return <SyncBackupHistoryPanel />;
      case "savedConnections":
        return (
          <SavedConnections
            onNewConnection={handleNewSession}
            onEditConnection={handleEditConnection}
          />
        );
      case "aiAssistant":
        return (
          <AIAssistantPanel
            activePane={
              activePane && !activePane.connecting && !activePane.connectError ? activePane : null
            }
            activeConnection={activeConnection}
            intent={aiIntent}
          />
        );
      case "activeSessions":
        return (
          <ActiveSessions
            onSessionClick={handleSessionClick}
            onSessionReconnect={handleReconnectSessionById}
            onSessionDisconnect={handleDisconnectSessionById}
            canReconnect={canReconnectSessionById}
          />
        );
      case "commandHistory":
        return (
          <CommandHistory activeSessionId={activeSessionId} onCommandSend={handleHistoryCommand} />
        );
      case "resourceMonitor":
        return <ResourceMonitor activeSessionId={activeSshSessionId} />;
      default:
        return null;
    }
  }

  return (
    <TransferProvider>
      <div
        className="font-display h-full min-h-0 flex flex-col overflow-hidden"
        style={{ backgroundColor: "var(--df-bg)", color: "var(--df-text)" }}
      >
        {/* Header */}
        <Header
          onNewSession={() => handleNewSession()}
          onToggleLeft={() => setMobileLeftOpen(!mobileLeftOpen)}
          onToggleRight={() => setMobileRightOpen(!mobileRightOpen)}
          onAbout={() => setShowAbout(true)}
          onCheckForUpdates={() => setShowUpdateDialog(true)}
          hasUpdate={updateInfo !== null}
          showUpdateDot={helpDotVisible}
          onHelpMenuOpen={() => setHelpDotVisible(false)}
          activeTab={activeTab}
          savedConnections={savedConnections}
          onSmartSplit={handleSmartSplit}
          onManageSyncGroups={() => setShowSyncGroupDialog(true)}
          onBroadcastToAll={() => setBroadcastToAll((prev) => !prev)}
          broadcastToAll={broadcastToAll}
          onClearTerminal={() => window.dispatchEvent(new CustomEvent("dragonfly:clear-terminal"))}
          onResetTerminalSize={() =>
            window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"))
          }
        />

        {/* Main Content */}
        <main className="flex-1 flex overflow-hidden relative">
          {/* Backdrop for mobile */}
          {(mobileLeftOpen || mobileRightOpen) && (
            <div
              className="absolute inset-0 bg-black/50 z-40 lg:hidden"
              onClick={() => {
                setMobileLeftOpen(false);
                setMobileRightOpen(false);
              }}
            />
          )}

          {/* Left Activity Bar */}
          <ActivityBar
            items={leftTopItems}
            bottomItems={leftBottomItems}
            activeId={uiConfig.active_left_panel}
            activeBottomIds={toggleActiveIds}
            onSelect={handleItemSelect}
            onReorder={(zk, ids) => handleReorder("left", zk, ids)}
            onMoveItem={handleMoveItem}
            onToggleLabel={handleToggleLabel}
            showLabels={showLabels}
            side="left"
            zone={{ top: "left_top", bottom: "left_bottom" }}
          />

          {/* Left Panel */}
          {uiConfig.active_left_panel && (
            <>
              <div
                style={{ width: uiConfig.left_width, backgroundColor: "var(--df-bg-panel)" }}
                className={`
                  fixed inset-y-0 left-10 z-40 flex flex-col shadow-xl transition-transform duration-200
                  lg:relative lg:left-0 lg:translate-x-0 lg:z-0 lg:shadow-none
                  ${
                    mobileLeftOpen
                      ? "translate-x-0"
                      : "-translate-x-[calc(100%+2.5rem)] lg:translate-x-0"
                  }
                `}
              >
                <div
                  className="lg:hidden h-10 flex items-center justify-end px-2 border-b shrink-0"
                  style={{ borderColor: "var(--df-border)" }}
                >
                  <button
                    onClick={() => setMobileLeftOpen(false)}
                    style={{ color: "var(--df-text-muted)" }}
                  >
                    <MdClose />
                  </button>
                </div>

                <div className="flex-1 min-h-0 overflow-hidden">
                  {renderPanelContent(uiConfig.active_left_panel)}
                </div>
              </div>
              <ResizeHandle
                direction="horizontal"
                onResize={handleLeftResize}
                className="hidden lg:block"
              />
            </>
          )}

          {/* Center - Terminal Area */}
          <section
            className="flex-1 flex flex-col relative min-w-0 origin-top-left"
            style={{
              backgroundColor: "var(--df-bg-terminal)",
            }}
          >
            <div className="flex-1 relative overflow-hidden">
              {tabs.length === 0 ? (
                <div className="flex items-center justify-center h-full text-slate-500">
                  <div className="text-center space-y-3">
                    <MdTerminal className="text-4xl mx-auto" />
                    <p className="text-sm">{t("app.noActiveSessions")}</p>
                    <button
                      className="px-4 py-2 text-xs bg-primary hover:bg-primary/80 text-white rounded transition-colors"
                      onClick={() => handleNewSession()}
                    >
                      {t("app.newConnection")}
                    </button>
                  </div>
                </div>
              ) : terminalWindows ? (
                <TabWindowsWorkspace
                  layout={terminalWindows}
                  tabsById={tabsById}
                  focusedTabId={activeTabId}
                  unreadTabIds={unreadTabIds}
                  onSelectTab={handleSelectLeafTab}
                  onAddTab={handleAddTabFromLeaf}
                  onTabClose={handleCloseWorkspaceTab}
                  onDuplicateSession={handleDuplicateSession}
                  onReconnectSession={handleReconnectSession}
                  onSplitSession={handleSplitSession}
                  onCloseSession={handleCloseSession}
                  onCloseAll={handleCloseAllTabs}
                  onCloseInactive={handleCloseInactiveTabs}
                  onCloseRight={handleCloseRightTabs}
                  onSessionInfo={handleSessionInfo}
                  onReorderTabs={handleReorderTabsInLeaf}
                  onActivatePane={handleActivatePane}
                  onUpdatePaneSplitRatio={handleUpdatePaneSplitRatio}
                  onUpdateWindowSplitRatio={handleUpdateWindowSplitRatio}
                  onReconnectPane={handleReconnectPane}
                  onReconnected={handleReconnected}
                />
              ) : (
                <div className="flex items-center justify-center h-full text-slate-500">
                  <div className="text-center space-y-3">
                    <MdTerminal className="text-4xl mx-auto" />
                    <p className="text-sm">{t("common.loading")}</p>
                  </div>
                </div>
              )}
            </div>

            {/* Bottom Panel: only one window can be visible at a time */}
            {activeBottomPanel === "quickCmdBar" && (
              <>
                <ResizeHandle direction="vertical" onResize={handleQuickCmdResize} />
                <div
                  style={{ height: uiConfig.quick_cmd_height }}
                  className="shrink-0 overflow-hidden"
                >
                  <QuickCommands
                    onSend={handleHistoryCommand}
                    onSendToAll={handleSendToAllSessions}
                  />
                </div>
              </>
            )}

            {activeBottomPanel === "serialSend" && (
              <>
                <ResizeHandle direction="vertical" onResize={handleSerialSendResize} />
                <div
                  style={{ height: uiConfig.serial_send_height || 120 }}
                  className="shrink-0 overflow-hidden"
                >
                  <SerialSendPanel
                    serialSessionId={activeSerialSessionId}
                    shellSessionIds={activeShellSessionIds}
                  />
                </div>
              </>
            )}
          </section>

          {/* Right Panel */}
          {uiConfig.active_right_panel && (
            <>
              <ResizeHandle
                direction="horizontal"
                onResize={handleRightResize}
                className="hidden md:block"
              />
              <aside
                style={{
                  width: uiConfig.right_width,
                  backgroundColor: "var(--df-bg-panel)",
                  borderColor: "var(--df-border)",
                }}
                className={`
                  fixed inset-y-0 right-10 z-50 flex flex-col shadow-xl transition-transform duration-200 border-l
                  md:relative md:right-0 md:translate-x-0 md:z-0 md:shadow-none
                  ${
                    mobileRightOpen
                      ? "translate-x-0"
                      : "translate-x-[calc(100%+2.5rem)] md:translate-x-0"
                  }
                `}
              >
                <div
                  className="md:hidden h-10 flex items-center justify-end px-2 border-b shrink-0"
                  style={{ borderColor: "var(--df-border)" }}
                >
                  <button
                    onClick={() => setMobileRightOpen(false)}
                    style={{ color: "var(--df-text-muted)" }}
                  >
                    <MdClose />
                  </button>
                </div>

                <div className="flex-1 min-h-0 overflow-hidden">
                  {renderPanelContent(uiConfig.active_right_panel)}
                </div>
              </aside>
            </>
          )}

          {/* Right Activity Bar */}
          <ActivityBar
            items={rightTopItems}
            bottomItems={rightBottomItems}
            activeId={uiConfig.active_right_panel}
            activeBottomIds={toggleActiveIds}
            onSelect={handleItemSelect}
            onReorder={(zk, ids) => handleReorder("right", zk, ids)}
            onMoveItem={handleMoveItem}
            onToggleLabel={handleToggleLabel}
            showLabels={showLabels}
            side="right"
            zone={{ top: "right_top", bottom: "right_bottom" }}
          />
        </main>

        <AboutDialog open={showAbout} onClose={() => setShowAbout(false)} />

        <SyncGroupDialog open={showSyncGroupDialog} onClose={() => setShowSyncGroupDialog(false)} />

        <UpdateDialog
          open={showUpdateDialog}
          onClose={() => setShowUpdateDialog(false)}
          onUpdateFound={(info) => setUpdateInfo(info)}
        />

        <QuitConfirmDialog
          open={showQuitConfirm}
          onOpenChange={setShowQuitConfirm}
          onConfirm={handleQuitApplication}
        />

        <OtpDialog request={otpRequest} onDone={() => setOtpRequest(null)} />
        <HostKeyVerifyDialog
          request={hostKeyVerifyRequest}
          onDone={() => setHostKeyVerifyRequest(null)}
        />

        <Toaster position="bottom-right" />

        {/* Child Window Modal Overlay */}
        {modalChildWindowCount > 0 && (
          <div
            className="fixed inset-0 z-[9998]"
            style={{
              backgroundColor: "rgba(0, 0, 0, 0.3)",
              backdropFilter: "blur(4px)",
              WebkitBackdropFilter: "blur(4px)",
            }}
          />
        )}

        {/* Lock Screen Overlay */}
        {isLocked && (
          <LockScreen
            hasPassword={!!appSettings.security.master_password}
            onUnlock={() => setIsLocked(false)}
          />
        )}
      </div>
    </TransferProvider>
  );
}

export default App;

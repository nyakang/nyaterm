import { listen } from "@tauri-apps/api/event";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BiServer } from "react-icons/bi";
import { FaRegFolder } from "react-icons/fa";
import { IoSwapVerticalOutline } from "react-icons/io5";
import { LuKeyRound } from "react-icons/lu";
import {
  MdBolt,
  MdClose,
  MdHistory,
  MdLink,
  MdLock,
  MdOutlineMonitorHeart,
  MdSettings,
  MdTerminal,
} from "react-icons/md";
import { Toaster } from "@/components/ui/sonner";
import AboutDialog from "./components/dialog/app/AboutDialog";
import LockScreen from "./components/dialog/app/LockScreen";
import type { ActivityBarItem } from "./components/layout/ActivityBar";
import ActivityBar from "./components/layout/ActivityBar";
import Header from "./components/layout/Header";
import ResizeHandle from "./components/layout/ResizeHandle";
import ActiveSessions from "./components/panel/ActiveSessions";
import CommandHistory from "./components/panel/CommandHistory";
import FileExplorer from "./components/panel/FileExplorer";
import FileTransfer from "./components/panel/FileTransfer";
import SecurityAuthPanel from "./components/panel/SecurityAuthPanel";
import ResourceMonitor from "./components/panel/ResourceMonitor";
import SavedConnections from "./components/panel/SavedConnections";
import QuickCommands from "./components/terminal/QuickCommands";
import TabBar from "./components/terminal/TabBar";
import XTerminal from "./components/terminal/XTerminal";
import { useApp } from "./context/AppContext";
import { TransferProvider } from "./context/TransferContext";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { invoke } from "./lib/invoke";
import { logger } from "./lib/logger";
import {
  DEFAULT_TERMINAL_FONT_SIZE,
  decreaseTerminalFontSize,
  increaseTerminalFontSize,
} from "./lib/terminalFontSize";
import {
  bounceTopModalWindow,
  openNewSession,
  openSettings,
  syncMainWindowModalState,
} from "./lib/windowManager";
import type {
  ActivityBarLayout,
  ActivityBarZone,
  AppSettings,
  SavedConnection,
} from "./types/global";

/** Item IDs that are not regular panels — they have special action on click. */
const NON_PANEL_IDS = new Set(["settings", "lock", "quickCmdBar"]);

/** Determine which visual side (left/right) a given item currently lives on. */
function getItemSide(id: string, layout: ActivityBarLayout): "left" | "right" | null {
  if (layout.left_top.includes(id) || layout.left_bottom.includes(id)) return "left";
  if (layout.right_top.includes(id) || layout.right_bottom.includes(id)) return "right";
  return null;
}

const CTRL_WHEEL_ZOOM_THROTTLE_MS = 50;

/** Root layout: header, activity bars, sidebars, terminal area, dialogs. */
function App() {
  const {
    tabs,
    activeTabId,
    setActiveTabId,
    addTab,
    addPendingTab,
    updateTabSession,
    closeTab,
    updateUi,
    updateAppSettings,
    appSettings,
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
  const lastCtrlWheelZoomAtRef = useRef(0);

  // Child window modal overlay
  const [childWindowCount, setChildWindowCount] = useState(0);

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_screen_lock ? appSettings.security.idle_lock_minutes : 0,
    () => setIsLocked(true),
  );

  // Cross-window event listeners
  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];

    unsubs.push(
      listen<AppSettings>("settings-changed", () => {
        invoke<AppSettings>("get_app_settings").then((cfg) => {
          updateAppSettings(() => cfg);
        });
      }),
    );

    unsubs.push(
      listen<{ sessionId: string; name: string; type: "SSH" | "Local" }>(
        "session-created",
        (event) => {
          const { sessionId, name: sessionName, type } = event.payload;
          addTab(sessionId, sessionName, type);
        },
      ),
    );

    unsubs.push(
      listen<{ connectionId: string }>("session-connect-after-edit", async (event) => {
        const { connectionId } = event.payload;
        try {
          const conns = await invoke<SavedConnection[]>("get_saved_connections");
          const conn = conns.find((c) => c.id === connectionId);
          const connName = conn?.name ?? connectionId;
          const tabId = addPendingTab(connName, "SSH", connectionId);
          try {
            const sessionId = await invoke<string>("create_ssh_session", { connectionId });
            updateTabSession(tabId, sessionId);
          } catch {
            closeTab(tabId);
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
  }, [addTab, addPendingTab, updateTabSession, closeTab, updateAppSettings]);

  // Track child window open/close for modal overlay
  useEffect(() => {
    const unsubs = [
      listen<{ label: string }>("child-window-opened", () => {
        setChildWindowCount((c) => c + 1);
        void syncMainWindowModalState();
      }),
      listen<{ label: string }>("child-window-closed", () => {
        setChildWindowCount((c) => Math.max(0, c - 1));
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
          if (!focused || childWindowCount === 0) return;
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
  }, [childWindowCount]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;

  const handleNewSession = (_parentGroupId?: string) => {
    openNewSession();
  };

  const handleEditConnection = useCallback((conn: SavedConnection, autoConnect?: boolean) => {
    openNewSession(conn.id, autoConnect);
  }, []);

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      const tab = tabs.find((t) => t.sessionId === sessionId);
      if (tab) {
        setActiveTabId(tab.id);
      }
    },
    [tabs, setActiveTabId],
  );

  const handleHistoryCommand = useCallback(
    (command: string, execute: boolean = true) => {
      if (activeTab) {
        const { sessionId } = activeTab;
        import("@tauri-apps/api/core").then(({ invoke }) => {
          invoke("write_to_session", {
            sessionId,
            data: execute ? `${command}\r` : command,
          });
        });
        import("@tauri-apps/api/event").then(({ emit }) => {
          emit(`focus-terminal-${sessionId}`);
        });
      }
    },
    [activeTab],
  );

  const handleReconnected = useCallback(
    (oldSessionId: string, newSessionId: string) => {
      const tab = tabs.find((t) => t.sessionId === oldSessionId);
      if (tab) {
        updateTabSession(tab.id, newSessionId);
      }
    },
    [tabs, updateTabSession],
  );

  // --- Shortcut callbacks ---

  const handleNewLocalTerminal = useCallback(() => {
    invoke<string>("create_local_session")
      .then((sessionId) => {
        addTab(sessionId, t("menu.newLocalTerminal"), "Local");
      })
      .catch((e) => logger.error("Failed to create local session", e));
  }, [addTab, t]);

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTab) return;
    if (!activeTab.connecting) {
      invoke("close_session", { sessionId: activeTab.sessionId }).catch(() => {});
    }
    closeTab(activeTab.id);
  }, [activeTab, closeTab]);

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

  const handleToggleFullscreen = useCallback(() => {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(() => {});
    } else {
      document.exitFullscreen();
    }
  }, []);

  const handleOpenSettings = useCallback(() => {
    openSettings();
  }, []);

  const handleLockScreen = useCallback(() => {
    if (appSettings.security.enable_screen_lock) {
      setIsLocked(true);
    }
  }, [appSettings.security.enable_screen_lock, setIsLocked]);

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
    onToggleFullscreen: handleToggleFullscreen,
    onOpenSettings: handleOpenSettings,
    onLockScreen: handleLockScreen,
  });

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

  // --- Activity bar item registry & dynamic zone arrays ---

  const itemRegistry = useMemo<Record<string, { icon: ReactNode; tooltip: string }>>(
    () => ({
      fileExplorer: { icon: <FaRegFolder />, tooltip: t("panel.fileExplorer") },
      fileTransfer: { icon: <IoSwapVerticalOutline />, tooltip: t("panel.fileTransfer") },
      securityAuth: { icon: <LuKeyRound />, tooltip: t("securityAuth.title") },
      settings: { icon: <MdSettings />, tooltip: t("settings.title") },
      savedConnections: { icon: <BiServer />, tooltip: t("panel.savedConnections") },
      activeSessions: { icon: <MdLink />, tooltip: t("panel.activeSessions") },
      commandHistory: { icon: <MdHistory />, tooltip: t("panel.commandHistory") },
      resourceMonitor: { icon: <MdOutlineMonitorHeart />, tooltip: t("panel.resourceMonitor") },
      quickCmdBar: { icon: <MdBolt />, tooltip: t("panel.quickCommands") },
      lock: { icon: <MdLock />, tooltip: t("statusBar.lock") },
    }),
    [t],
  );

  const layout = uiConfig.activity_bar_layout;

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
    return s;
  }, [uiConfig.show_quick_cmd_bar]);

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
        updateUi((prev) => ({ show_quick_cmd_bar: !prev.show_quick_cmd_bar }));
        return;
      }
      const side = getItemSide(id, layout);
      if (side === "left") {
        updateUi((prev) => ({ active_left_panel: prev.active_left_panel === id ? null : id }));
      } else if (side === "right") {
        updateUi((prev) => ({ active_right_panel: prev.active_right_panel === id ? null : id }));
      }
    },
    [updateUi, setIsLocked, layout],
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

  const activeSessionId = activeTab?.connecting ? null : (activeTab?.sessionId ?? null);

  function renderPanelContent(id: string | null) {
    switch (id) {
      case "fileExplorer":
        return <FileExplorer activeSessionId={activeSessionId} />;
      case "fileTransfer":
        return <FileTransfer activeSessionId={activeSessionId} />;
      case "securityAuth":
        return <SecurityAuthPanel />;
      case "savedConnections":
        return (
          <SavedConnections
            onNewConnection={handleNewSession}
            onEditConnection={handleEditConnection}
          />
        );
      case "activeSessions":
        return <ActiveSessions onSessionClick={handleSessionClick} />;
      case "commandHistory":
        return <CommandHistory onCommandSend={handleHistoryCommand} />;
      case "resourceMonitor":
        return <ResourceMonitor activeSessionId={activeSessionId} />;
      default:
        return null;
    }
  }

  return (
    <TransferProvider>
      <div
        className="font-display h-screen flex flex-col overflow-hidden"
        style={{ backgroundColor: "var(--df-bg)", color: "var(--df-text)" }}
      >
        {/* Header */}
        <Header
          onNewSession={() => handleNewSession()}
          onToggleLeft={() => setMobileLeftOpen(!mobileLeftOpen)}
          onToggleRight={() => setMobileRightOpen(!mobileRightOpen)}
          onAbout={() => setShowAbout(true)}
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

          {/* Left Activity Bar (TA + TC) */}
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
                  ${mobileLeftOpen ? "translate-x-0" : "-translate-x-full lg:translate-x-0"}
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
            {/* Tab Bar */}
            <TabBar
              tabs={tabs}
              activeTabId={activeTabId}
              onTabChange={setActiveTabId}
              onTabClose={closeTab}
              onAddTab={() => handleNewSession()}
            />

            {/* Terminal Instances */}
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
              ) : (
                tabs.map((tab) =>
                  tab.connecting ? (
                    <div
                      key={tab.id}
                      className="absolute inset-0 flex items-center justify-center"
                      style={{
                        display: activeTabId === tab.id ? "flex" : "none",
                        color: "var(--df-text-dimmed)",
                      }}
                    >
                      <div className="flex flex-col items-center gap-3 text-sm">
                        <svg
                          className="animate-spin w-6 h-6"
                          style={{ color: "var(--df-primary)" }}
                          xmlns="http://www.w3.org/2000/svg"
                          fill="none"
                          viewBox="0 0 24 24"
                        >
                          <title>{t("common.loading")}</title>
                          <circle
                            className="opacity-25"
                            cx="12"
                            cy="12"
                            r="10"
                            stroke="currentColor"
                            strokeWidth="4"
                          />
                          <path
                            className="opacity-75"
                            fill="currentColor"
                            d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
                          />
                        </svg>
                        <span>{tab.name}</span>
                      </div>
                    </div>
                  ) : (
                    <XTerminal
                      key={tab.sessionId}
                      sessionId={tab.sessionId}
                      active={activeTabId === tab.id}
                      connectionId={tab.connectionId}
                      onReconnected={handleReconnected}
                    />
                  ),
                )
              )}
            </div>

            {/* Quick Commands Bar */}
            {uiConfig.show_quick_cmd_bar && (
              <>
                <ResizeHandle direction="vertical" onResize={handleQuickCmdResize} />
                <div
                  style={{ height: uiConfig.quick_cmd_height }}
                  className="shrink-0 overflow-hidden"
                >
                  <QuickCommands onSend={handleHistoryCommand} />
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
                  ${mobileRightOpen ? "translate-x-0" : "translate-x-full md:translate-x-0"}
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

          {/* Right Activity Bar (TB + TD) */}
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

        <Toaster position="bottom-right" />

        {/* Child Window Modal Overlay */}
        {childWindowCount > 0 && (
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
            hasPassword={!!appSettings.security.lock_password}
            onUnlock={() => setIsLocked(false)}
          />
        )}
      </div>
    </TransferProvider>
  );
}

export default App;

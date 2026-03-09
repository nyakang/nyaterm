import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { MdClose, MdTerminal } from "react-icons/md";
import { Toaster } from "@/components/ui/sonner";
import AboutDialog from "./components/dialog/app/AboutDialog";
import LockScreen from "./components/dialog/app/LockScreen";
import DraggablePanel from "./components/layout/DraggablePanel";
import Header from "./components/layout/Header";
import ResizeHandle from "./components/layout/ResizeHandle";
import StatusBar from "./components/layout/StatusBar";
import ActiveSessions from "./components/panel/ActiveSessions";
import CommandHistory from "./components/panel/CommandHistory";
import SavedConnections from "./components/panel/SavedConnections";
import FileExplorer from "./components/sidebar/FileExplorer";
import FileTransfer from "./components/sidebar/FileTransfer";
import QuickCommands from "./components/terminal/QuickCommands";
import TabBar from "./components/terminal/TabBar";
import XTerminal from "./components/terminal/XTerminal";
import { useApp } from "./context/AppContext";
import { TransferProvider } from "./context/TransferContext";
import { useGlobalShortcuts } from "./hooks/useGlobalShortcuts";
import { useIdleLock } from "./hooks/useIdleLock";
import { invoke } from "./lib/invoke";
import { logger } from "./lib/logger";
import { openNewSession, openSettings } from "./lib/windowManager";
import type { AppSettings, PanelId, PanelLayout, SavedConnection, UiConfig } from "./types/global";

const PANEL_VISIBILITY: Record<PanelId, keyof UiConfig & `show_${string}`> = {
  fileExplorer: "show_file_explorer",
  fileTransfer: "show_file_transfer",
  savedConnections: "show_saved_connections",
  activeSessions: "show_active_sessions",
  commandHistory: "show_command_history",
};

const PANEL_HEIGHT_KEY: Partial<Record<PanelId, keyof UiConfig>> = {
  fileTransfer: "file_transfer_height",
  savedConnections: "saved_conn_height",
  commandHistory: "history_height",
};

const PANEL_DEFAULT_HEIGHT: Partial<Record<PanelId, number>> = {
  fileTransfer: 240,
  savedConnections: 240,
  commandHistory: 200,
};

const DEFAULT_PANEL_LAYOUT: PanelLayout = {
  left: ["fileExplorer", "fileTransfer"],
  right: ["savedConnections", "activeSessions", "commandHistory"],
};

/** Root layout: header, sidebars, terminal area, dialogs. Wraps content in ToastProvider. */
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

  // Idle auto-lock
  useIdleLock(
    appSettings.security.enable_screen_lock ? appSettings.security.idle_lock_minutes : 0,
    () => setIsLocked(true)
  );

  // Cross-window event listeners (child windows emit these)
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
      listen<{ connectionId: string }>(
        "session-connect-after-edit",
        async (event) => {
          const { connectionId } = event.payload;
          try {
            const conns = await invoke<SavedConnection[]>("get_saved_connections");
            const conn = conns.find((c) => c.id === connectionId);
            const connName = conn?.name ?? connectionId;
            const tabId = addPendingTab(connName, "SSH", connectionId);
            try {
              const sessionId = await invoke<string>("create_ssh_session", { connectionId });
              updateTabSession(tabId, sessionId);
            } catch (e) {
              closeTab(tabId);
            }
          } catch { /* ignore */ }
        },
      ),
    );

    return () => {
      unsubs.forEach((p) => p.then((unsub) => unsub()));
    };
  }, [addTab, addPendingTab, updateTabSession, closeTab, updateAppSettings]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;

  const handleNewSession = (_parentGroupId?: string) => {
    openNewSession();
  };

  const handleEditConnection = useCallback(
    (conn: SavedConnection, autoConnect?: boolean) => {
      openNewSession(conn.id, autoConnect);
    },
    [],
  );

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

  const LEFT_PANELS: PanelId[] = ["fileExplorer", "fileTransfer"];
  const RIGHT_PANELS: PanelId[] = ["savedConnections", "activeSessions", "commandHistory"];

  const handleToggleLeftSidebar = useCallback(() => {
    const anyVisible = LEFT_PANELS.some((id) => uiConfig[PANEL_VISIBILITY[id]]);
    const updates: Partial<UiConfig> = {};
    for (const id of LEFT_PANELS) updates[PANEL_VISIBILITY[id]] = !anyVisible;
    updateUi(updates);
  }, [uiConfig, updateUi]);

  const handleToggleRightSidebar = useCallback(() => {
    const anyVisible = RIGHT_PANELS.some((id) => uiConfig[PANEL_VISIBILITY[id]]);
    const updates: Partial<UiConfig> = {};
    for (const id of RIGHT_PANELS) updates[PANEL_VISIBILITY[id]] = !anyVisible;
    updateUi(updates);
  }, [uiConfig, updateUi]);

  const handleZoomIn = useCallback(() => {
    updateUi((prev) => ({
      zoom_level: parseFloat(Math.min(2.0, prev.zoom_level + 0.1).toFixed(1)),
    }));
  }, [updateUi]);

  const handleZoomOut = useCallback(() => {
    updateUi((prev) => ({
      zoom_level: parseFloat(Math.max(0.5, prev.zoom_level - 0.1).toFixed(1)),
    }));
  }, [updateUi]);

  const handleResetZoom = useCallback(() => {
    updateUi({ zoom_level: 1.0 });
  }, [updateUi]);

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

  const panelLayout = uiConfig.panel_layout ?? DEFAULT_PANEL_LAYOUT;

  const isPanelVisible = useCallback(
    (id: PanelId) => uiConfig[PANEL_VISIBILITY[id]] as boolean,
    [uiConfig],
  );

  const visibleLeftPanels = useMemo(
    () => panelLayout.left.filter(isPanelVisible),
    [panelLayout.left, isPanelVisible],
  );
  const visibleRightPanels = useMemo(
    () => panelLayout.right.filter(isPanelVisible),
    [panelLayout.right, isPanelVisible],
  );

  const handlePanelDrop = useCallback(
    (
      draggedId: PanelId,
      fromSidebar: "left" | "right",
      targetId: PanelId,
      targetSidebar: "left" | "right",
      position: "before" | "after",
    ) => {
      updateUi((prev) => {
        const layout = prev.panel_layout ?? DEFAULT_PANEL_LAYOUT;
        const newLeft = [...layout.left];
        const newRight = [...layout.right];

        const sourceArr = fromSidebar === "left" ? newLeft : newRight;
        const sourceIdx = sourceArr.indexOf(draggedId);
        if (sourceIdx >= 0) sourceArr.splice(sourceIdx, 1);

        const targetArr = targetSidebar === "left" ? newLeft : newRight;
        let insertIdx = targetArr.indexOf(targetId);
        if (insertIdx < 0) insertIdx = targetArr.length;
        if (position === "after") insertIdx++;
        targetArr.splice(insertIdx, 0, draggedId);

        return { panel_layout: { left: newLeft, right: newRight } };
      });
    },
    [updateUi],
  );

  const handlePanelResize = useCallback(
    (aboveId: PanelId, belowId: PanelId, delta: number) => {
      const aboveKey = PANEL_HEIGHT_KEY[aboveId];
      const belowKey = PANEL_HEIGHT_KEY[belowId];

      if (aboveKey) {
        updateUi((prev) => ({
          [aboveKey]: Math.max(
            80,
            Math.min(500, ((prev[aboveKey] as number) || PANEL_DEFAULT_HEIGHT[aboveId] || 200) + delta),
          ),
        }));
      } else if (belowKey) {
        updateUi((prev) => ({
          [belowKey]: Math.max(
            80,
            Math.min(500, ((prev[belowKey] as number) || PANEL_DEFAULT_HEIGHT[belowId] || 200) - delta),
          ),
        }));
      }
    },
    [updateUi],
  );

  function renderPanelContent(id: PanelId) {
    switch (id) {
      case "fileExplorer":
        return <FileExplorer activeSessionId={activeTab?.connecting ? null : (activeTab?.sessionId ?? null)} />;
      case "fileTransfer":
        return <FileTransfer activeSessionId={activeTab?.connecting ? null : (activeTab?.sessionId ?? null)} />;
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
    }
  }

  function renderSidebarPanels(panels: PanelId[], sidebar: "left" | "right") {
    if (panels.length === 0) return null;

    const flexIndex = panels.findIndex((id) => !PANEL_HEIGHT_KEY[id]);
    const actualFlexIndex = flexIndex >= 0 ? flexIndex : 0;

    const elements: ReactNode[] = [];

    panels.forEach((panelId, idx) => {
      if (idx > 0) {
        const aboveId = panels[idx - 1];
        if (PANEL_HEIGHT_KEY[aboveId] || PANEL_HEIGHT_KEY[panelId]) {
          elements.push(
            <ResizeHandle
              key={`resize-${aboveId}-${panelId}`}
              direction="vertical"
              onResize={(delta) => handlePanelResize(aboveId, panelId, delta)}
            />,
          );
        }
      }

      const isFlex = panels.length === 1 || idx === actualFlexIndex;
      const heightKey = PANEL_HEIGHT_KEY[panelId];

      elements.push(
        <DraggablePanel
          key={panelId}
          panelId={panelId}
          sidebar={sidebar}
          onPanelDrop={handlePanelDrop}
          className={isFlex ? "flex-1 min-h-0 overflow-hidden" : "shrink-0 overflow-hidden"}
          style={
            !isFlex && heightKey
              ? { height: (uiConfig[heightKey] as number) || PANEL_DEFAULT_HEIGHT[panelId] }
              : undefined
          }
        >
          {renderPanelContent(panelId)}
        </DraggablePanel>,
      );
    });

    return elements;
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

          {/* Left Sidebar */}
          {visibleLeftPanels.length > 0 && (
            <>
              <div
                style={{ width: uiConfig.left_width, backgroundColor: "var(--df-bg)" }}
                className={`
                  fixed inset-y-0 left-0 z-40 flex flex-col shadow-xl transition-transform duration-200
                  lg:relative lg:translate-x-0 lg:z-0 lg:shadow-none
                  ${mobileLeftOpen ? "translate-x-0" : "-translate-x-full"}
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

                <div className="flex-1 flex flex-col min-h-0 pt-10 lg:pt-0">
                  {renderSidebarPanels(visibleLeftPanels, "left")}
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
              zoom: uiConfig.zoom_level,
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
                        <svg className="animate-spin w-6 h-6" style={{ color: "var(--df-primary)" }} xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                          <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                          <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                        </svg>
                        <span>{tab.name}</span>
                      </div>
                    </div>
                  ) : (
                    <XTerminal
                      key={tab.sessionId}
                      sessionId={tab.sessionId}
                      active={activeTabId === tab.id}
                    />
                  ),
                )
              )}
            </div>

            {/* Quick Commands Bar */}
            {uiConfig.show_quick_commands && (
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

          {/* Right Sidebar */}
          {visibleRightPanels.length > 0 && (
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
                  fixed inset-y-0 right-0 z-50 flex flex-col shadow-xl transition-transform duration-200 border-l
                  md:relative md:translate-x-0 md:z-0 md:shadow-none
                  ${mobileRightOpen ? "translate-x-0" : "translate-x-full"}
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

                {renderSidebarPanels(visibleRightPanels, "right")}
              </aside>
            </>
          )}
        </main>

        {/* Status Bar */}
        <StatusBar />

        <AboutDialog open={showAbout} onClose={() => setShowAbout(false)} />

        <Toaster position="bottom-right" />

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

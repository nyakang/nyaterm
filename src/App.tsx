import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdClose, MdTerminal } from "react-icons/md";
import { Toaster } from "@/components/ui/sonner";
import AboutDialog from "./components/dialog/AboutDialog";
import LockScreen from "./components/dialog/LockScreen";
import NewSessionDialog from "./components/dialog/NewSessionDialog";
import SettingsDialog from "./components/dialog/SettingsDialog";
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
import { useIdleLock } from "./hooks/useIdleLock";
import type { SavedConnection } from "./types";

/** Root layout: header, sidebars, terminal area, dialogs. Wraps content in ToastProvider. */
function App() {
  const {
    tabs,
    activeTabId,
    setActiveTabId,
    addTab,
    closeTab,
    uiConfig,
    updateUiConfig,
    showNewSession,
    setShowNewSession,
    editingConnection,
    setEditingConnection,
    refreshConnections,
    appSettings,
    isLocked,
    setIsLocked,
  } = useApp();
  const { t, i18n } = useTranslation();

  useEffect(() => {
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow().show();
    });
  }, []);

  useEffect(() => {
    if (uiConfig.language && uiConfig.language !== i18n.language) {
      i18n.changeLanguage(uiConfig.language);
    }
  }, [uiConfig.language, i18n]);

  // Mobile state
  const [mobileLeftOpen, setMobileLeftOpen] = useState(false);
  const [mobileRightOpen, setMobileRightOpen] = useState(false);
  const [showAbout, setShowAbout] = useState(false);

  // Idle auto-lock
  useIdleLock(appSettings.security.idle_lock_minutes, () => setIsLocked(true));

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;

  const handleNewSession = () => {
    setEditingConnection(undefined);
    setShowNewSession(true);
  };

  const handleEditConnection = useCallback(
    (conn: SavedConnection) => {
      setEditingConnection(conn);
      setShowNewSession(true);
    },
    [setEditingConnection, setShowNewSession],
  );

  // Handle connection success from Dialog or SavedConnections
  const handleSessionConnected = useCallback(
    (sessionId: string, name: string, type_: "SSH" | "Local", connectionId?: string) => {
      addTab(sessionId, name, type_, connectionId);
      setShowNewSession(false);
      setEditingConnection(undefined);
    },
    [addTab, setShowNewSession, setEditingConnection],
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
        import("@tauri-apps/api/core").then(({ invoke }) => {
          invoke("write_to_session", {
            sessionId: activeTab.sessionId,
            data: execute ? `${command}\r` : command,
          });
        });
      }
    },
    [activeTab],
  );

  // Resize handlers
  const handleLeftResize = useCallback(
    (delta: number) => {
      updateUiConfig((prev) => ({
        left_width: Math.max(160, Math.min(480, (prev.left_width || 256) + delta)),
      }));
    },
    [updateUiConfig],
  );

  const handleRightResize = useCallback(
    (delta: number) => {
      updateUiConfig((prev) => ({
        right_width: Math.max(200, Math.min(480, (prev.right_width || 288) - delta)),
      }));
    },
    [updateUiConfig],
  );

  const handleSavedConnResize = useCallback(
    (delta: number) => {
      updateUiConfig((prev) => ({
        saved_conn_height: Math.max(80, Math.min(500, (prev.saved_conn_height || 240) + delta)),
      }));
    },
    [updateUiConfig],
  );

  const handleHistoryResize = useCallback(
    (delta: number) => {
      // delta > 0 means mouse moves down → shrink history (it's at the bottom in flex col, but here it's actually growing/shrinking height)
      updateUiConfig((prev) => ({
        history_height: Math.max(80, Math.min(500, (prev.history_height || 200) - delta)),
      }));
    },
    [updateUiConfig],
  );

  const handleQuickCmdResize = useCallback(
    (delta: number) => {
      updateUiConfig((prev) => ({
        quick_cmd_height: Math.max(36, Math.min(300, (prev.quick_cmd_height || 36) - delta)),
      }));
    },
    [updateUiConfig],
  );

  const handleFileTransferResize = useCallback(
    (delta: number) => {
      updateUiConfig((prev) => ({
        file_transfer_height: Math.max(
          80,
          Math.min(500, (prev.file_transfer_height || 240) - delta),
        ),
      }));
    },
    [updateUiConfig],
  );

  return (
    <TransferProvider>
      <div
        className="font-display h-screen flex flex-col overflow-hidden"
        style={{ backgroundColor: "var(--df-bg)", color: "var(--df-text)" }}
      >
        {/* Header */}
        <Header
          onNewSession={handleNewSession}
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

          {/* Left Sidebar - File Explorer & File Transfer */}
          {(uiConfig.show_file_explorer || uiConfig.show_file_transfer) && (
            <>
              <div
                style={{ width: uiConfig.left_width, backgroundColor: "var(--df-bg)" }}
                className={`
                  fixed inset-y-0 left-0 z-40 flex flex-col shadow-xl transition-transform duration-200
                  lg:relative lg:translate-x-0 lg:z-0 lg:shadow-none
                  ${mobileLeftOpen ? "translate-x-0" : "-translate-x-full"}
                `}
              >
                {/* Mobile placeholder for header height if needed, or close button */}
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
                  {uiConfig.show_file_explorer && (
                    <div className="flex-1 min-h-0 overflow-hidden">
                      <FileExplorer activeSessionId={activeTab?.sessionId ?? null} />
                    </div>
                  )}

                  {uiConfig.show_file_transfer && (
                    <>
                      {uiConfig.show_file_explorer && (
                        <ResizeHandle direction="vertical" onResize={handleFileTransferResize} />
                      )}
                      <div
                        style={{
                          height: uiConfig.show_file_explorer
                            ? uiConfig.file_transfer_height || 240
                            : "100%",
                        }}
                        className={
                          uiConfig.show_file_explorer
                            ? "shrink-0 overflow-hidden"
                            : "flex-1 min-h-0 overflow-hidden"
                        }
                      >
                        <FileTransfer activeSessionId={activeTab?.sessionId ?? null} />
                      </div>
                    </>
                  )}
                </div>
              </div>
              {/* Only show resize handle on desktop */}
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
              onAddTab={handleNewSession}
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
                      onClick={handleNewSession}
                    >
                      {t("app.newConnection")}
                    </button>
                  </div>
                </div>
              ) : (
                tabs.map((tab) => (
                  <XTerminal
                    key={tab.sessionId}
                    sessionId={tab.sessionId}
                    active={activeTabId === tab.id}
                  />
                ))
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

          {/* Right Sidebar: three independent panels */}
          {(uiConfig.show_saved_connections ||
            uiConfig.show_active_sessions ||
            uiConfig.show_command_history) && (
            <>
              {/* Only show resize handle on desktop */}
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
                {/* Mobile placeholder for header height if needed, or close button */}
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

                {/* Saved Connections - fixed pixel height at top */}
                {uiConfig.show_saved_connections && (
                  <>
                    <div
                      style={{ height: uiConfig.saved_conn_height }}
                      className="shrink-0 overflow-hidden"
                    >
                      <SavedConnections
                        onEditConnection={handleEditConnection}
                        onSessionCreated={handleSessionConnected}
                      />
                    </div>
                    {/* Show resize handle only if there's something below it */}
                    {(uiConfig.show_active_sessions || uiConfig.show_command_history) && (
                      <ResizeHandle direction="vertical" onResize={handleSavedConnResize} />
                    )}
                  </>
                )}

                {/* Active Sessions - flexible middle */}
                {uiConfig.show_active_sessions && (
                  <div className="flex-1 min-h-0 overflow-hidden">
                    <ActiveSessions onSessionClick={handleSessionClick} />
                  </div>
                )}

                {/* Command History - fixed pixel height at bottom */}
                {uiConfig.show_command_history && (
                  <>
                    {/* Show resize handle only if there's something above it */}
                    {(uiConfig.show_saved_connections || uiConfig.show_active_sessions) && (
                      <ResizeHandle direction="vertical" onResize={handleHistoryResize} />
                    )}
                    <div
                      style={{ height: uiConfig.history_height }}
                      className="shrink-0 overflow-hidden"
                    >
                      <CommandHistory onCommandSend={handleHistoryCommand} />
                    </div>
                  </>
                )}
              </aside>
            </>
          )}
        </main>

        {/* Status Bar */}
        <StatusBar />

        {/* New Session Dialog */}
        <NewSessionDialog
          open={showNewSession}
          onClose={() => {
            setShowNewSession(false);
            setEditingConnection(undefined);
          }}
          onConnect={handleSessionConnected}
          onSaved={() => {
            setShowNewSession(false);
            setEditingConnection(undefined);
            refreshConnections();
          }}
          initialData={editingConnection}
        />

        <AboutDialog open={showAbout} onClose={() => setShowAbout(false)} />
        <SettingsDialog />

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

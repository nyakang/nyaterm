import { useCallback, useState } from "react";
import AboutDialog from "./components/dialog/AboutDialog";
import NewSessionDialog from "./components/dialog/NewSessionDialog";
import Header from "./components/layout/Header";
import ResizeHandle from "./components/layout/ResizeHandle";
import StatusBar from "./components/layout/StatusBar";
import ActiveSessions from "./components/panel/ActiveSessions";
import CommandHistory from "./components/panel/CommandHistory";
import SavedConnections from "./components/panel/SavedConnections";
import FileExplorer from "./components/sidebar/FileExplorer";
import QuickCommands from "./components/terminal/QuickCommands";
import TabBar from "./components/terminal/TabBar";
import XTerminal from "./components/terminal/XTerminal";
import ToastContainer from "./components/toast/ToastContainer";
import { ToastProvider } from "./components/toast/ToastContext";
import { useApp } from "./context/AppContext";
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
    // savedConnections, // consumed by ActiveSessions/SavedConnections if refactored
    showNewSession,
    setShowNewSession,
    editingConnection,
    setEditingConnection,
    refreshConnections,
  } = useApp();

  // Mobile state
  const [mobileLeftOpen, setMobileLeftOpen] = useState(false);
  const [mobileRightOpen, setMobileRightOpen] = useState(false);
  const [showAbout, setShowAbout] = useState(false);

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
    (sessionId: string, name: string, type_: "SSH" | "Local") => {
      addTab(sessionId, name, type_);
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
    (command: string) => {
      if (activeTab) {
        import("@tauri-apps/api/core").then(({ invoke }) => {
          invoke("write_to_session", {
            sessionId: activeTab.sessionId,
            data: `${command}\r`,
          });
        });
      }
    },
    [activeTab],
  );

  // Resize handlers
  const handleLeftResize = useCallback(
    (delta: number) => {
      updateUiConfig({ left_width: Math.max(160, Math.min(480, uiConfig.left_width + delta)) });
    },
    [updateUiConfig, uiConfig.left_width],
  );

  const handleRightResize = useCallback(
    (delta: number) => {
      updateUiConfig({ right_width: Math.max(200, Math.min(480, uiConfig.right_width - delta)) });
    },
    [updateUiConfig, uiConfig.right_width],
  );

  const handleSavedConnResize = useCallback(
    (delta: number) => {
      updateUiConfig({
        saved_conn_height: Math.max(80, Math.min(500, uiConfig.saved_conn_height + delta)),
      });
    },
    [updateUiConfig, uiConfig.saved_conn_height],
  );

  const handleHistoryResize = useCallback(
    (delta: number) => {
      // delta > 0 means mouse moves down → shrink history (it's at the bottom in flex col, but here it's actually growing/shrinking height)
      // Checking previous logic: setHistoryHeight((h) => Math.max(80, Math.min(500, h - delta)));
      updateUiConfig({
        history_height: Math.max(80, Math.min(500, uiConfig.history_height - delta)),
      });
    },
    [updateUiConfig, uiConfig.history_height],
  );

  const handleQuickCmdResize = useCallback(
    (delta: number) => {
      updateUiConfig({
        quick_cmd_height: Math.max(36, Math.min(300, uiConfig.quick_cmd_height - delta)),
      });
    },
    [updateUiConfig, uiConfig.quick_cmd_height],
  );

  return (
    <ToastProvider>
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

          {/* Left Sidebar - File Explorer */}
          {uiConfig.show_file_explorer && (
            <>
              <div
                style={{ width: uiConfig.left_width, backgroundColor: "var(--df-bg)" }}
                className={`
                  fixed inset-y-0 left-0 z-40 shadow-xl transition-transform duration-200
                  lg:relative lg:translate-x-0 lg:z-0 lg:shadow-none
                  ${mobileLeftOpen ? "translate-x-0" : "-translate-x-full"}
                `}
              >
                <div className="h-full flex flex-col pt-10 lg:pt-0">
                  <FileExplorer activeSessionId={activeTab?.sessionId ?? null} />
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
              zoom: uiConfig.zoom_level
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
                    <span className="material-icons text-4xl">terminal</span>
                    <p className="text-sm">No active sessions</p>
                    <button
                      className="px-4 py-2 text-xs bg-primary hover:bg-primary/80 text-white rounded transition-colors"
                      onClick={handleNewSession}
                    >
                      New Connection
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
                      <span className="material-icons">close</span>
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

        <ToastContainer />
      </div>
    </ToastProvider>
  );
}

export default App;

import { useRef } from "react";
import ResizeHandle from "@/components/layout/ResizeHandle";
import ActiveSessions from "@/components/panel/ActiveSessions";
import AIAssistantPanel from "@/components/panel/AIAssistantPanel";
import CommandHistory from "@/components/panel/CommandHistory";
import FileExplorer from "@/components/panel/file-explorer";
import FileTransfer from "@/components/panel/file-explorer/FileTransfer";
import NetworkPanel from "@/components/panel/NetworkPanel";
import ResourceMonitor from "@/components/panel/ResourceMonitor";
import SyncBackupHistoryPanel from "@/components/panel/SyncBackupHistoryPanel";
import SavedConnections from "@/components/panel/saved-connections";
import SecurityAuthPanel from "@/components/panel/security-auth";
import type { AIOpenIntent } from "@/lib/aiEvents";
import type { NewSessionTarget } from "@/lib/windowManager";
import type { SavedConnection, SessionPane } from "@/types/global";

interface AppPanelContentProps {
  panelId: string | null;
  activePane: SessionPane | null;
  activeConnection: SavedConnection | null;
  activeSessionId: string | null;
  activeSshSessionId: string | null;
  aiIntent: AIOpenIntent | null;
  transferHeight: number;
  onTransferResize: (delta: number) => void;
  onNewConnection: (parentGroupId?: string) => void;
  onEditConnection: (
    connection: SavedConnection,
    autoConnect?: boolean,
    target?: NewSessionTarget,
  ) => void;
  onSessionClick: (sessionId: string) => void;
  onSessionReconnect: (sessionId: string) => Promise<void> | void;
  onSessionDisconnect: (sessionId: string) => Promise<void> | void;
  canReconnect: (sessionId: string) => boolean;
  onCommandSend: (command: string, execute?: boolean) => void;
}

export default function AppPanelContent({
  panelId,
  activePane,
  activeConnection,
  activeSessionId,
  activeSshSessionId,
  aiIntent,
  transferHeight,
  onTransferResize,
  onNewConnection,
  onEditConnection,
  onSessionClick,
  onSessionReconnect,
  onSessionDisconnect,
  canReconnect,
  onCommandSend,
}: AppPanelContentProps) {
  const liveActivePane =
    activePane && !activePane.connecting && !activePane.connectError ? activePane : null;

  const aiEverMounted = useRef(false);
  if (panelId === "aiAssistant") aiEverMounted.current = true;

  const otherPanel = (() => {
    switch (panelId) {
      case "fileExplorer":
        return (
          <div className="h-full flex flex-col overflow-hidden">
            <div className="flex-1 min-h-0 overflow-hidden">
              <FileExplorer
                activeSessionId={activeSessionId}
                activeSessionType={liveActivePane ? liveActivePane.type : null}
              />
            </div>
            <ResizeHandle direction="vertical" onResize={onTransferResize} />
            <div style={{ height: transferHeight }} className="shrink-0 overflow-hidden">
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
          <SavedConnections onNewConnection={onNewConnection} onEditConnection={onEditConnection} />
        );
      case "activeSessions":
        return (
          <ActiveSessions
            onSessionClick={onSessionClick}
            onSessionReconnect={onSessionReconnect}
            onSessionDisconnect={onSessionDisconnect}
            canReconnect={canReconnect}
          />
        );
      case "commandHistory":
        return <CommandHistory activeSessionId={activeSessionId} onCommandSend={onCommandSend} />;
      case "resourceMonitor":
        return <ResourceMonitor activeSessionId={activeSshSessionId} />;
      case "aiAssistant":
        return null;
      default:
        return null;
    }
  })();

  const isAiActive = panelId === "aiAssistant";

  return (
    <>
      {otherPanel}
      {aiEverMounted.current && (
        <div className={isAiActive ? "h-full" : "hidden"}>
          <AIAssistantPanel
            activePane={liveActivePane}
            activeConnection={activeConnection}
            intent={aiIntent}
          />
        </div>
      )}
    </>
  );
}

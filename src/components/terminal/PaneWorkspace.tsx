import { memo, useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdErrorOutline } from "react-icons/md";
import ResizeHandle from "@/components/layout/ResizeHandle";
import { Button } from "@/components/ui/button";
import { useApp } from "@/context/AppContext";
import {
  getActiveGroupForSession,
  getSessionInputPeerIds,
  isSessionPausedInGroup,
  pauseSessionInGroup,
  removeSessionFromGroup,
  resumeSessionInGroup,
} from "@/lib/syncInputGroups";
import { isSplitPane } from "@/lib/workspaceTabs";
import { FilePreviewContent } from "@/components/panel/file-explorer/FilePreviewContent";
import type { PaneNode, SplitPane, Tab } from "@/types/global";
import XTerminal from "./XTerminal";

interface PaneWorkspaceProps {
  tab: Tab;
  visible: boolean;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
}

function SplitView({
  split,
  tab,
  visible,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
}: {
  split: SplitPane;
  tab: Tab;
  visible: boolean;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const isHorizontalSplit = split.direction === "horizontal";

  const handleResize = (delta: number) => {
    const size = isHorizontalSplit
      ? (containerRef.current?.clientHeight ?? 0)
      : (containerRef.current?.clientWidth ?? 0);
    if (size <= 0) return;
    onUpdateSplitRatio(split.id, split.ratio + delta / size);
  };

  return (
    <div
      ref={containerRef}
      className={`flex h-full w-full min-h-0 min-w-0 ${
        isHorizontalSplit ? "flex-col" : "flex-row"
      }`}
    >
      <div
        className="min-h-0 min-w-0 relative"
        style={{ flexBasis: `${split.ratio * 100}%`, flexGrow: 0, flexShrink: 0 }}
      >
        <PaneNodeView
          node={split.first}
          tab={tab}
          visible={visible}
          showChrome
          onActivatePane={onActivatePane}
          onUpdateSplitRatio={onUpdateSplitRatio}
          onReconnectPane={onReconnectPane}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={onDisconnectedCloseRequested}
          onConnectionError={onConnectionError}
        />
      </div>
      <ResizeHandle
        direction={isHorizontalSplit ? "vertical" : "horizontal"}
        onResize={handleResize}
      />
      <div
        className="min-h-0 min-w-0 flex-1 relative"
        style={{ flexBasis: `${(1 - split.ratio) * 100}%`, flexGrow: 1, flexShrink: 1 }}
      >
        <PaneNodeView
          node={split.second}
          tab={tab}
          visible={visible}
          showChrome
          onActivatePane={onActivatePane}
          onUpdateSplitRatio={onUpdateSplitRatio}
          onReconnectPane={onReconnectPane}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={onDisconnectedCloseRequested}
          onConnectionError={onConnectionError}
        />
      </div>
    </div>
  );
}

function PaneNodeView({
  node,
  tab,
  visible,
  showChrome,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
}: {
  node: PaneNode;
  tab: Tab;
  visible: boolean;
  showChrome: boolean;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
}) {
  const { t } = useTranslation();
  const { syncGroups, broadcastToAll } = useApp();
  const [isReconnectPending, setIsReconnectPending] = useState(false);

  const handleReconnectClick = async () => {
    if (!onReconnectPane || isReconnectPending) return;
    setIsReconnectPending(true);
    try {
      await onReconnectPane(tab.id, node.id);
    } finally {
      setIsReconnectPending(false);
    }
  };

  if (isSplitPane(node)) {
    return (
      <SplitView
        split={node}
        tab={tab}
        visible={visible}
        onActivatePane={onActivatePane}
        onUpdateSplitRatio={onUpdateSplitRatio}
        onReconnectPane={onReconnectPane}
        onReconnected={onReconnected}
        onDisconnectedCloseRequested={onDisconnectedCloseRequested}
        onConnectionError={onConnectionError}
      />
    );
  }

  const isActive = visible && tab.activePaneId === node.id;
  const showReconnectAction = !!(node.type === "Local" || node.connectionId) && !!onReconnectPane;
  const statusTitle = isReconnectPending
    ? t("tabCtx.reconnecting")
    : t("terminal.connectionFailed");
  const statusMessage = isReconnectPending
    ? t("savedConnections.connecting", { name: node.name })
    : node.connectError;

  return (
    <div
      className={`nyaterm-wallpaper-terminal-surface relative h-full w-full overflow-hidden ${
        showChrome ? "rounded-sm border" : ""
      } ${showChrome && isActive ? "ring-1 ring-primary/60" : ""}`}
      style={{
        borderColor: showChrome ? "var(--df-border)" : undefined,
        backgroundColor: "var(--df-terminal-bg, var(--df-bg-terminal))",
      }}
      onMouseDown={() => onActivatePane(node.id)}
    >
      {node.connecting ? (
        <div
          className="flex h-full w-full flex-col items-center justify-center gap-3 text-sm"
          style={{ color: "var(--df-text-dimmed)" }}
        >
          <svg
            aria-hidden="true"
            className="h-6 w-6 animate-spin"
            style={{ color: "var(--df-primary)" }}
            xmlns="http://www.w3.org/2000/svg"
            fill="none"
            viewBox="0 0 24 24"
          >
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
          <span className="max-w-[16rem] truncate px-4 text-center">{node.name}</span>
        </div>
      ) : node.connectError ? (
        <div
          className="flex h-full w-full flex-col items-center justify-center gap-3 px-6 text-center text-sm"
          style={{ color: "var(--df-text-dimmed)" }}
          aria-live="polite"
        >
          {isReconnectPending ? (
            <svg
              aria-hidden="true"
              className="h-8 w-8 animate-spin"
              style={{ color: "var(--df-primary)" }}
              xmlns="http://www.w3.org/2000/svg"
              fill="none"
              viewBox="0 0 24 24"
            >
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
          ) : (
            <MdErrorOutline className="h-8 w-8" style={{ color: "var(--destructive, #ef4444)" }} />
          )}
          <div className={`space-y-1 ${isReconnectPending ? "animate-pulse" : ""}`}>
            <div className="font-medium" style={{ color: "var(--df-text)" }}>
              {statusTitle}
            </div>
            <div className="max-w-[20rem] break-words text-xs">{statusMessage}</div>
          </div>
          {showReconnectAction ? (
            <Button
              size="sm"
              variant="outline"
              disabled={isReconnectPending}
              aria-busy={isReconnectPending}
              onClick={() => void handleReconnectClick()}
            >
              {t("tabCtx.reconnect")}
            </Button>
          ) : null}
        </div>
      ) : node.viewType === "file" && node.fileMetadata ? (
        <FilePreviewContent
          data={{
            sessionId: node.sessionId,
            backend: node.type === "Local" ? "local" : "remote",
            path: node.fileMetadata.remotePath,
            name: node.name,
            size: node.fileMetadata.size,
            mtime: 0,
          }}
          active={isActive}
          className="h-full w-full"
        />
      ) : (
        <PaneXTerminal
          sessionId={node.sessionId}
          active={isActive}
          visible={visible}
          sessionType={node.type}
          connectionId={node.connectionId}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={() => void onDisconnectedCloseRequested?.(tab.id, node.id)}
          onConnectionError={(sessionId, error) =>
            onConnectionError?.(tab.id, node.id, sessionId, error)
          }
          syncGroups={syncGroups}
          broadcastToAll={broadcastToAll}
        />
      )}
    </div>
  );
}

function PaneXTerminal({
  sessionId,
  active,
  visible,
  sessionType,
  connectionId,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  syncGroups,
  broadcastToAll,
}: {
  sessionId: string;
  active: boolean;
  visible: boolean;
  sessionType: import("@/types/global").SessionType;
  connectionId?: string;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: () => void;
  onConnectionError?: (sessionId: string, error: string) => void;
  syncGroups: import("@/types/global").SyncGroup[];
  broadcastToAll: boolean;
}) {
  const { tabs, setSyncGroups } = useApp();

  const syncPeerSessionIds = useMemo(() => {
    return getSessionInputPeerIds(sessionId, syncGroups, tabs, broadcastToAll);
  }, [sessionId, syncGroups, broadcastToAll, tabs]);

  const activeGroup = useMemo(
    () => getActiveGroupForSession(sessionId, syncGroups),
    [sessionId, syncGroups],
  );

  const isPaused = useMemo(
    () => (activeGroup ? isSessionPausedInGroup(activeGroup, sessionId) : false),
    [activeGroup, sessionId],
  );

  const handlePauseResume = useCallback(() => {
    if (!activeGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) =>
        g.id === activeGroup.id
          ? isPaused
            ? resumeSessionInGroup(g, sessionId)
            : pauseSessionInGroup(g, sessionId)
          : g,
      ),
    );
  }, [activeGroup, isPaused, sessionId, setSyncGroups]);

  const handleLeaveGroup = useCallback(() => {
    if (!activeGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) => (g.id === activeGroup.id ? removeSessionFromGroup(g, sessionId) : g)),
    );
  }, [activeGroup, sessionId, setSyncGroups]);

  const handleCloseGroup = useCallback(() => {
    if (!activeGroup) return;
    setSyncGroups((prev) =>
      prev.map((g) => (g.id === activeGroup.id ? { ...g, enabled: false } : g)),
    );
  }, [activeGroup, setSyncGroups]);

  const syncOverlay = useMemo(() => {
    if (!activeGroup?.enabled) return undefined;
    return {
      peerCount: syncPeerSessionIds.length,
      isPaused,
      groupColor: activeGroup.color,
      groupName: activeGroup.name,
      onPauseResume: handlePauseResume,
      onLeaveGroup: handleLeaveGroup,
      onCloseGroup: handleCloseGroup,
    };
  }, [
    activeGroup,
    syncPeerSessionIds.length,
    isPaused,
    handlePauseResume,
    handleLeaveGroup,
    handleCloseGroup,
  ]);

  return (
    <XTerminal
      sessionId={sessionId}
      active={active}
      visible={visible}
      sessionType={sessionType}
      connectionId={connectionId}
      onReconnected={onReconnected}
      onDisconnectedCloseRequested={onDisconnectedCloseRequested}
      onConnectionError={onConnectionError}
      syncPeerSessionIds={syncPeerSessionIds}
      syncOverlay={syncOverlay}
    />
  );
}

function PaneWorkspace({
  tab,
  visible,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
}: PaneWorkspaceProps) {
  return (
    <div className="absolute inset-0" style={{ display: visible ? "block" : "none" }}>
      <PaneNodeView
        node={tab.root}
        tab={tab}
        visible={visible}
        showChrome={isSplitPane(tab.root)}
        onActivatePane={onActivatePane}
        onUpdateSplitRatio={onUpdateSplitRatio}
        onReconnectPane={onReconnectPane}
        onReconnected={onReconnected}
        onDisconnectedCloseRequested={onDisconnectedCloseRequested}
        onConnectionError={onConnectionError}
      />
    </div>
  );
}

export default memo(PaneWorkspace);

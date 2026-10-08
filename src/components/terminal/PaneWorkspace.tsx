import { listen } from "@/lib/backend/api";
import { FolderOpen } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdErrorOutline } from "react-icons/md";
import ResizeHandle from "@/components/layout/ResizeHandle";
import { FilePreviewContent } from "@/components/panel/file-explorer/FilePreviewContent";
import RdpPaneHost from "@/components/rdp/RdpPaneHost";
import { Button } from "@/components/ui/button";
import VncPaneHost from "@/components/vnc/VncPaneHost";
import { useApp } from "@/context/AppContext";
import { hasMatchingTemporaryConfig } from "@/lib/appWorkspace";
import {
  getActiveGroupForSession,
  getSessionInputPeerIds,
  isSessionPausedInGroup,
  pauseSessionInGroup,
  removeSessionFromGroup,
  resumeSessionInGroup,
} from "@/lib/syncInputGroups";
import { findTmuxControlSessionId } from "@/lib/tmux/tree";
import {
  findPaneById,
  findPaneBySessionId,
  findTabBySessionId,
  isSplitPane,
} from "@/lib/workspaceTabs";
import type {
  PaneNode,
  RecordingMode,
  RecordingStatus,
  SessionInfo,
  SplitPane,
  Tab,
  TerminalSessionPane,
} from "@/types/global";
import TmuxBar from "./TmuxBar";
import XTerminal from "./XTerminal";

interface PaneWorkspaceProps {
  tab: Tab;
  visible: boolean;
  paneFocusMode?: boolean;
  sessionInfoById?: Map<string, SessionInfo> | null;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
  recordingStatuses?: RecordingStatus[];
  onToggleSessionRecording?: (sessionId: string, mode?: RecordingMode) => Promise<void> | void;
  onSaveSessionTranscript?: (sessionId: string, sessionName?: string) => Promise<void> | void;
}

function SplitView({
  split,
  tab,
  visible,
  paneFocusMode,
  sessionInfoById,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  recordingStatuses,
  onToggleSessionRecording,
  onSaveSessionTranscript,
}: {
  split: SplitPane;
  tab: Tab;
  visible: boolean;
  paneFocusMode: boolean;
  sessionInfoById?: Map<string, SessionInfo> | null;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
  recordingStatuses?: RecordingStatus[];
  onToggleSessionRecording?: (sessionId: string, mode?: RecordingMode) => Promise<void> | void;
  onSaveSessionTranscript?: (sessionId: string, sessionName?: string) => Promise<void> | void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const isHorizontalSplit = split.direction === "horizontal";
  const firstContainsActivePane = !!findPaneById(split.first, tab.activePaneId);
  const secondContainsActivePane = !!findPaneById(split.second, tab.activePaneId);
  // tmux-driven splits mirror a remote layout; dragging a divider would fight
  // the cell-based geometry, so the divider is fixed and non-interactive.
  const isTmuxSplit = useMemo(() => {
    const contains = (node: PaneNode): boolean =>
      node.kind === "leaf" ? Boolean(node.tmux) : contains(node.first) || contains(node.second);
    return contains(split);
  }, [split]);

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
        style={{
          display: paneFocusMode && !firstContainsActivePane ? "none" : undefined,
          flexBasis: paneFocusMode ? "100%" : `${split.ratio * 100}%`,
          flexGrow: 0,
          flexShrink: 0,
        }}
      >
        <PaneNodeView
          node={split.first}
          tab={tab}
          visible={visible && (!paneFocusMode || firstContainsActivePane)}
          paneFocusMode={paneFocusMode}
          sessionInfoById={sessionInfoById}
          showChrome
          onActivatePane={onActivatePane}
          onUpdateSplitRatio={onUpdateSplitRatio}
          onReconnectPane={onReconnectPane}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={onDisconnectedCloseRequested}
          onConnectionError={onConnectionError}
          recordingStatuses={recordingStatuses}
          onToggleSessionRecording={onToggleSessionRecording}
          onSaveSessionTranscript={onSaveSessionTranscript}
        />
      </div>
      {!paneFocusMode &&
        (isTmuxSplit ? (
          <div
            className={`shrink-0 ${isHorizontalSplit ? "h-px" : "w-px"}`}
            style={{ background: "var(--df-border)" }}
          />
        ) : (
          <ResizeHandle
            direction={isHorizontalSplit ? "vertical" : "horizontal"}
            onResize={handleResize}
          />
        ))}
      <div
        className="min-h-0 min-w-0 flex-1 relative"
        style={{
          display: paneFocusMode && !secondContainsActivePane ? "none" : undefined,
          flexBasis: paneFocusMode ? "100%" : `${(1 - split.ratio) * 100}%`,
          flexGrow: 1,
          flexShrink: 1,
        }}
      >
        <PaneNodeView
          node={split.second}
          tab={tab}
          visible={visible && (!paneFocusMode || secondContainsActivePane)}
          paneFocusMode={paneFocusMode}
          sessionInfoById={sessionInfoById}
          showChrome
          onActivatePane={onActivatePane}
          onUpdateSplitRatio={onUpdateSplitRatio}
          onReconnectPane={onReconnectPane}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={onDisconnectedCloseRequested}
          onConnectionError={onConnectionError}
          recordingStatuses={recordingStatuses}
          onToggleSessionRecording={onToggleSessionRecording}
          onSaveSessionTranscript={onSaveSessionTranscript}
        />
      </div>
    </div>
  );
}

function PaneNodeView({
  node,
  tab,
  visible,
  paneFocusMode,
  sessionInfoById,
  showChrome,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  recordingStatuses,
  onToggleSessionRecording,
  onSaveSessionTranscript,
}: {
  node: PaneNode;
  tab: Tab;
  visible: boolean;
  paneFocusMode: boolean;
  sessionInfoById?: Map<string, SessionInfo> | null;
  showChrome: boolean;
  onActivatePane: (paneId: string) => void;
  onUpdateSplitRatio: (splitId: string, ratio: number) => void;
  onReconnectPane?: (tabId: string, paneId: string) => void | Promise<void>;
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: (tabId: string, paneId: string) => void | Promise<void>;
  onConnectionError?: (tabId: string, paneId: string, sessionId: string, error: string) => void;
  recordingStatuses?: RecordingStatus[];
  onToggleSessionRecording?: (sessionId: string, mode?: RecordingMode) => Promise<void> | void;
  onSaveSessionTranscript?: (sessionId: string, sessionName?: string) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const { syncGroups, broadcastToAll } = useApp();
  const [isReconnectPending, setIsReconnectPending] = useState(false);
  const [closedSftpSessionId, setClosedSftpSessionId] = useState<string | null>(null);
  const splitNode = isSplitPane(node);
  const requestedSftp = !splitNode && node.sshRuntimeMode === "sftp";
  const effectiveSftp =
    !splitNode && sessionInfoById?.get(node.sessionId)?.ssh_runtime_mode === "sftp";
  const sessionId = splitNode ? null : node.sessionId;
  const observedSftpRef = useRef({ sessionId, enabled: requestedSftp || effectiveSftp });
  if (observedSftpRef.current.sessionId !== sessionId) {
    observedSftpRef.current = {
      sessionId,
      enabled: requestedSftp || effectiveSftp,
    };
  } else if (requestedSftp || effectiveSftp) {
    observedSftpRef.current.enabled = true;
  }
  const isSftpOnly = !splitNode && observedSftpRef.current.enabled;

  useEffect(() => {
    if (!isSftpOnly || !sessionId) return;
    setClosedSftpSessionId(null);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen(`session-closed-${sessionId}`, () => {
      if (!disposed) setClosedSftpSessionId(sessionId);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [isSftpOnly, sessionId]);

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
        paneFocusMode={paneFocusMode}
        sessionInfoById={sessionInfoById}
        onActivatePane={onActivatePane}
        onUpdateSplitRatio={onUpdateSplitRatio}
        onReconnectPane={onReconnectPane}
        onReconnected={onReconnected}
        onDisconnectedCloseRequested={onDisconnectedCloseRequested}
        onConnectionError={onConnectionError}
        recordingStatuses={recordingStatuses}
        onToggleSessionRecording={onToggleSessionRecording}
        onSaveSessionTranscript={onSaveSessionTranscript}
      />
    );
  }

  const isActive = visible && tab.activePaneId === node.id;
  const showReconnectAction =
    node.paneKind !== "file" &&
    !!(node.type === "Local" || node.connectionId || hasMatchingTemporaryConfig(node)) &&
    !!onReconnectPane;
  const statusTitle = isReconnectPending
    ? t("tabCtx.reconnecting")
    : t("terminal.connectionFailed");
  const statusMessage = isReconnectPending
    ? t("savedConnections.connecting", { name: node.name })
    : node.connectError;

  return (
    <div
      className={`nyaterm-wallpaper-terminal-surface nyaterm-terminal-surface relative h-full w-full overflow-hidden ${
        showChrome ? "rounded-sm border" : ""
      } ${showChrome && isActive ? "ring-1 ring-primary/60" : ""}`}
      style={{
        borderColor: showChrome ? "var(--df-border)" : undefined,
        backgroundColor: "var(--df-terminal-surface-bg)",
      }}
      onMouseDown={() => onActivatePane(node.id)}
    >
      {node.paneKind === "file" ? (
        <FilePreviewContent mode="edit" pane={node} active={isActive} />
      ) : node.connecting ? (
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
          <span className="max-w-[16rem] truncate px-4 text-center">
            {node.name}
          </span>
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
            <MdErrorOutline
              className="h-8 w-8"
              style={{ color: "var(--destructive, #ef4444)" }}
            />
          )}
          <div
            className={`space-y-1 ${isReconnectPending ? "animate-pulse" : ""}`}
          >
            <div className="font-medium" style={{ color: "var(--df-text)" }}>
              {statusTitle}
            </div>
            <div className="max-w-[20rem] break-words text-xs">
              {statusMessage}
            </div>
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
      ) : node.paneKind === "remote-desktop" ? (
        node.type === "RDP" ? (
          <RdpPaneHost
            pane={node}
            active={isActive}
            visible={visible}
            onConnectionError={(sessionId, error) =>
              onConnectionError?.(tab.id, node.id, sessionId, error)
            }
          />
        ) : (
          <VncPaneHost
            pane={node}
            active={isActive}
            visible={visible}
            onDisconnectedCloseRequested={() =>
              void onDisconnectedCloseRequested?.(tab.id, node.id)
            }
            onConnectionError={(sessionId, error) =>
              onConnectionError?.(tab.id, node.id, sessionId, error)
            }
          />
        )
      ) : isSftpOnly ? (
        <div
          className="flex h-full w-full flex-col items-center justify-center gap-3 px-6 text-center text-sm"
          style={{ color: "var(--df-text-dimmed)" }}
          aria-live="polite"
          data-testid="sftp-only-placeholder"
        >
          <FolderOpen className="size-9" style={{ color: "var(--df-primary)" }} />
          <div className="space-y-1">
            <div className="font-medium" style={{ color: "var(--df-text)" }}>
              {closedSftpSessionId === node.sessionId
                ? t("sftpRuntime.disconnected")
                : t("sftpRuntime.title")}
            </div>
            <div className="max-w-[24rem] text-xs">
              {t("sftpRuntime.openedWithoutShell")}
            </div>
            <div className="max-w-[24rem] text-xs">
              {t("sftpRuntime.useFileExplorer")}
            </div>
          </div>
          {closedSftpSessionId === node.sessionId && showReconnectAction ? (
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
      ) : (
        <PaneXTerminal
          sessionId={node.sessionId}
          sessionName={node.name}
          active={isActive}
          visible={visible}
          sessionType={node.type}
          connectionId={node.connectionId}
          temporaryConfig={node.temporaryConfig}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={() =>
            void onDisconnectedCloseRequested?.(tab.id, node.id)
          }
          onConnectionError={(sessionId, error) =>
            onConnectionError?.(tab.id, node.id, sessionId, error)
          }
          syncGroups={syncGroups}
          broadcastToAll={broadcastToAll}
          sessionInfoById={sessionInfoById}
          recordingStatus={recordingStatuses?.find((status) => status.sessionId === node.sessionId)}
          onToggleRecording={onToggleSessionRecording}
          onSaveTranscript={onSaveSessionTranscript}
        />
      )}
    </div>
  );
}

function PaneXTerminal({
  sessionId,
  sessionName,
  active,
  visible,
  sessionType,
  connectionId,
  temporaryConfig,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  syncGroups,
  broadcastToAll,
  sessionInfoById,
  recordingStatus,
  onToggleRecording,
  onSaveTranscript,
}: {
  sessionId: string;
  sessionName: string;
  active: boolean;
  visible: boolean;
  sessionType: TerminalSessionPane["type"];
  connectionId?: string;
  temporaryConfig?: TerminalSessionPane["temporaryConfig"];
  onReconnected?: (oldSessionId: string, newSessionId: string) => void;
  onDisconnectedCloseRequested?: () => void;
  onConnectionError?: (sessionId: string, error: string) => void;
  syncGroups: import("@/types/global").SyncGroup[];
  broadcastToAll: boolean;
  sessionInfoById?: Map<string, SessionInfo> | null;
  recordingStatus?: RecordingStatus;
  onToggleRecording?: (sessionId: string, mode?: RecordingMode) => Promise<void> | void;
  onSaveTranscript?: (sessionId: string, sessionName?: string) => Promise<void> | void;
}) {
  const { tabs, setSyncGroups, isLocked } = useApp();

  const syncPeerSessionIds = useMemo(() => {
    return getSessionInputPeerIds(sessionId, syncGroups, tabs, broadcastToAll).filter(
      (peerSessionId) => {
        if (sessionInfoById?.get(peerSessionId)?.ssh_runtime_mode === "sftp") return false;
        const peerTab = findTabBySessionId(tabs, peerSessionId);
        const peerPane = peerTab ? findPaneBySessionId(peerTab, peerSessionId) : null;
        return peerPane?.sshRuntimeMode !== "sftp";
      },
    );
  }, [broadcastToAll, sessionId, sessionInfoById, syncGroups, tabs]);

  const activeGroup = useMemo(
    () => getActiveGroupForSession(sessionId, syncGroups),
    [sessionId, syncGroups],
  );

  const isPaused = useMemo(
    () =>
      activeGroup ? isSessionPausedInGroup(activeGroup, sessionId) : false,
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
      prev.map((g) =>
        g.id === activeGroup.id ? removeSessionFromGroup(g, sessionId) : g,
      ),
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
      sessionName={sessionName}
      active={active}
      appLocked={isLocked}
      visible={visible}
      sessionType={sessionType}
      connectionId={connectionId}
      temporaryConfig={temporaryConfig}
      onReconnected={onReconnected}
      onDisconnectedCloseRequested={onDisconnectedCloseRequested}
      onConnectionError={onConnectionError}
      syncPeerSessionIds={syncPeerSessionIds}
      syncOverlay={syncOverlay}
      recordingStatus={recordingStatus}
      onToggleRecording={onToggleRecording}
      onSaveTranscript={onSaveTranscript}
    />
  );
}

function PaneWorkspace({
  tab,
  visible,
  paneFocusMode = false,
  sessionInfoById,
  onActivatePane,
  onUpdateSplitRatio,
  onReconnectPane,
  onReconnected,
  onDisconnectedCloseRequested,
  onConnectionError,
  recordingStatuses,
  onToggleSessionRecording,
  onSaveSessionTranscript,
}: PaneWorkspaceProps) {
  const tmuxControlSessionId = useMemo(
    () => findTmuxControlSessionId(tab),
    [tab],
  );
  return (
    <div
      className="absolute inset-0 flex flex-col"
      style={{ display: visible ? "flex" : "none" }}
    >
      {tmuxControlSessionId ? (
        <TmuxBar controlSessionId={tmuxControlSessionId} />
      ) : null}
      <div className="relative min-h-0 flex-1">
        <PaneNodeView
          node={tab.root}
          tab={tab}
          visible={visible}
          paneFocusMode={paneFocusMode}
          sessionInfoById={sessionInfoById}
          showChrome={!paneFocusMode && isSplitPane(tab.root)}
          onActivatePane={onActivatePane}
          onUpdateSplitRatio={onUpdateSplitRatio}
          onReconnectPane={onReconnectPane}
          onReconnected={onReconnected}
          onDisconnectedCloseRequested={onDisconnectedCloseRequested}
          onConnectionError={onConnectionError}
          recordingStatuses={recordingStatuses}
          onToggleSessionRecording={onToggleSessionRecording}
          onSaveSessionTranscript={onSaveSessionTranscript}
        />
      </div>
    </div>
  );
}

export default memo(PaneWorkspace);

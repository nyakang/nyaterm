import { emit, listen } from "@tauri-apps/api/event";
import { downloadDir, join, tempDir } from "@tauri-apps/api/path";
import {
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  type CSSProperties,
  memo,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  startTransition,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { LuClipboardPaste, LuFolderSync } from "react-icons/lu";
import {
  MdArrowDropDown,
  MdArrowDropUp,
  MdClose,
  MdContentCopy,
  MdCreateNewFolder,
  MdDriveFolderUpload,
  MdFolderOff,
  MdInfo,
  MdLink,
  MdNoteAdd,
  MdRefresh,
  MdSyncLock,
  MdUpload,
} from "react-icons/md";
import { PiColumnsPlusRightBold } from "react-icons/pi";
import { toast } from "sonner";
import type {
  DeleteDialogData,
  DeleteDialogItem,
} from "@/components/dialog/file-explorer/DeleteDialog";
import type { MoveDialogData } from "@/components/dialog/file-explorer/MoveDialog";
import type { NewItemDialogData } from "@/components/dialog/file-explorer/NewItemDialog";
import type { NewSymlinkDialogData } from "@/components/dialog/file-explorer/NewSymlinkDialog";
import type { PropertiesDialogData } from "@/components/dialog/file-explorer/PropertiesDialog";
import ExternalFileDropOverlay from "@/components/ExternalFileDropOverlay";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useTransfer } from "@/context/TransferContext";
import { resolveShortcutKeys } from "@/hooks/useShortcutMap";
import { openAIAssistant } from "@/lib/aiEvents";
import { getErrorMessage } from "@/lib/errors";
import { MAX_EDITOR_FILE_BYTES } from "@/lib/fileEditorLimits";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { sendSessionInput, sendSessionInputWithSync } from "@/lib/sessionInput";
import { matchesKeyEvent } from "@/lib/shortcutRegistry";
import { getSessionInputPeerIds } from "@/lib/syncInputGroups";
import { cn, formatSize } from "@/lib/utils";
import type { FileWindowTarget } from "@/lib/windowManager";
import { openAutoUpload, openFilePreview, openRemoteFileEditor } from "@/lib/windowManager";
import {
  findOpenFileDocument,
  findSessionPaneById,
} from "@/lib/workspaceTabs";
import type {
  AICustomActionConfig,
  FileEntry,
  FileExplorerProps,
  SavedConnection,
  SessionInfo,
  SessionType,
} from "@/types/global";
import { resolveFileEditorOpenTarget, resolveInternalEditorDisplay } from "./editorOpenMode";
import { FileExplorerDialogs } from "./FileExplorerDialogs";
import {
  clearDirectoryChildrenCacheForPath,
  clearDirectoryChildrenCacheForSession,
  FileExplorerPathBar,
} from "./FileExplorerPathBar";
import { FileExplorerToolbar } from "./FileExplorerToolbar";
import FileExplorerEntryContextMenu from "./FileExplorerEntryContextMenu";
import FileExplorerTree from "./FileExplorerTree";
import { FileListItem } from "./FileListItem";
import {
  buildRemoteUploadPath,
  buildMoveSuccessRefreshPlan,
  buildMoveTargetPath,
  buildSessionCacheSnapshot,
  canTrackTerminalCwd,
  compareFileEntries,
  DEFAULT_FILE_LIST_COLUMN_WIDTHS,
  DEFAULT_FILE_SORT_DIRECTIONS,
  type DirectoryChild,
  FILE_LIST_COLUMNS,
  FILE_LIST_HEADER_HEIGHT,
  FILE_LIST_ITEM_HEIGHT,
  FILE_LIST_OVERSCAN,
  type FileExplorerBackendKind,
  type FileListColumnWidths,
  type FileSortColumn,
  type FileSortMode,
  fileExplorerSessionCacheStore,
  getExplorerParentDirectory,
  getLocalPathName,
  type InlineRenameState,
  isParentDirectoryEntry,
  isSameExplorerDirectory,
  joinExplorerPath,
  type LoadDirectoryOptions,
  MIN_FILE_LIST_COLUMN_WIDTHS,
  matchesFileSearch,
  type MoveDialogItem,
  normalizeDirectoryPath,
  normalizeExplorerPath,
  normalizeFileExplorerViewMode,
  pathStartsWithDirectory,
  PARENT_DIRECTORY_ENTRY,
  PARENT_DIRECTORY_ENTRY_NAME,
  pushVisitedHistory,
  type RemoteTextFile,
  type ResolvedLocalDropPathEntry,
  subscribeFileExplorerSessionSnapshots,
  syncExplorerDirectoryToTerminalCwd,
  syncExplorerDirectoryToTerminalCwdChange,
  type TextFileOpenResult,
} from "./model";
import { useExternalFileDrop } from "./useExternalFileDrop";
import {
  clearFileExplorerTreeSessionCache,
  getTreeRootPath,
  type FileExplorerTreeEntry,
  type FileExplorerTreeRow,
} from "./fileExplorerTreeModel";
import { useFileExplorerTree } from "./useFileExplorerTree";

const MemoizedFileExplorer = memo(FileExplorer);

export default MemoizedFileExplorer;

type FileExplorerPaneEndpoint = {
  sessionId: string;
  kind: "local" | "remote";
  currentPath: string;
};

type FileExplorerCopyEntry = {
  name: string;
  path: string;
  isDirectory: boolean;
};

type FileExplorerSendTargetOption = {
  sessionId: string;
  label: string;
  meta: string;
};

interface FileExplorerPaneExtraProps {
  headerMeta?: ReactNode;
  headerActions?: ReactNode;
  peerEndpoint?: FileExplorerPaneEndpoint | null;
  onOpenPeerSelector?: () => void;
  onDirectoryStateChange?: (state: FileExplorerPaneEndpoint | null) => void;
  sendTargetOptions?: FileExplorerSendTargetOption[];
  onSendEntriesToTarget?: (
    source: FileExplorerPaneEndpoint,
    entries: FileExplorerCopyEntry[],
    targetSessionId: string,
  ) => void;
}

function isFileBrowsableSession(session: SessionInfo) {
  return (
    session.connected &&
    (session.session_type === "Local" ||
      (session.session_type === "SSH" && session.remote_file_browser_enabled))
  );
}

function toFileExplorerSessionType(session: SessionInfo): SessionType | null {
  return session.session_type === "Local" || session.session_type === "SSH"
    ? session.session_type
    : null;
}

function getSessionExplorerKind(session: SessionInfo): FileExplorerBackendKind {
  return session.session_type === "Local" ? "local" : "remote";
}

function formatConnectionTargetDetail(connection: SavedConnection) {
  if (connection.type === "ssh" && connection.host) {
    const hostWithPort = connection.port
      ? `${connection.host}:${connection.port}`
      : connection.host;
    return connection.username
      ? `${connection.username}@${hostWithPort}`
      : hostWithPort;
  }
  if (connection.type === "local_terminal") {
    return connection.working_dir || connection.shell_path || undefined;
  }
  return undefined;
}

function buildFileWindowTarget({
  backend,
  connection,
  sessionName,
  remoteLabel,
}: {
  backend: FileExplorerBackendKind;
  connection?: SavedConnection | null;
  sessionName?: string | null;
  remoteLabel: string;
}): FileWindowTarget | undefined {
  const fallbackLabel = sessionName?.trim() || connection?.name?.trim() || "";
  if (backend === "local") {
    return undefined;
  }

  if (connection?.name?.trim()) {
    return {
      kind: "remote",
      label: connection.name,
      detail:
        formatConnectionTargetDetail(connection) || fallbackLabel || undefined,
    };
  }

  return {
    kind: "remote",
    label: fallbackLabel || remoteLabel,
  };
}

/** Dual-pane file browser wrapper. */
function FileExplorer(props: FileExplorerProps) {
  const { t } = useTranslation();
  const { enqueueCopies } = useTransfer();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const secondaryOverlayRef = useRef<HTMLDivElement | null>(null);
  const secondaryPositionFrameRef = useRef<number | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [targetSessionId, setTargetSessionId] = useState<string | null>(null);
  const [targetSelectorOpen, setTargetSelectorOpen] = useState(false);
  const [primaryEndpoint, setPrimaryEndpoint] =
    useState<FileExplorerPaneEndpoint | null>(null);
  const [secondaryEndpoint, setSecondaryEndpoint] =
    useState<FileExplorerPaneEndpoint | null>(null);
  const [secondaryOverlayStyle, setSecondaryOverlayStyle] =
    useState<CSSProperties | null>(null);

  useEffect(() => {
    let disposed = false;
    const load = async () => {
      try {
        const next = await invoke<SessionInfo[]>("list_sessions");
        if (!disposed) setSessions(next);
      } catch {
        if (!disposed) setSessions([]);
      }
    };
    void load();
    const unlisten = listen("sessions-changed", () => {
      void load();
    });
    return () => {
      disposed = true;
      unlisten.then((dispose) => dispose());
    };
  }, []);

  const browsableSessions = sessions.filter(isFileBrowsableSession);
  const targetCandidates = browsableSessions.filter(
    (session) => session.id !== props.activeSessionId,
  );
  const selectedTarget =
    targetCandidates.find((session) => session.id === targetSessionId) ?? null;
  const currentSession =
    sessions.find((session) => session.id === props.activeSessionId) ?? null;
  const canShowDualButton =
    !!props.activeSessionId && browsableSessions.length > 1;
  const primarySendTargetOptions = targetCandidates.map((session) => ({
    sessionId: session.id,
    label: session.name,
    meta: session.session_type,
  }));
  const secondarySendTargetOptions =
    currentSession && props.activeSessionId
      ? [
          {
            sessionId: props.activeSessionId,
            label: currentSession.name,
            meta: currentSession.session_type,
          },
        ]
      : [];

  const closeSecondaryPane = useCallback(() => {
    setTargetSessionId(null);
    setSecondaryEndpoint(null);
  }, []);

  useEffect(() => {
    if (!selectedTarget && targetSessionId) {
      setTargetSessionId(null);
      setSecondaryEndpoint(null);
    }
  }, [selectedTarget, targetSessionId]);

  const measureSecondaryOverlayPosition = useCallback(() => {
    const container = containerRef.current;
    if (!container || !selectedTarget) {
      setSecondaryOverlayStyle(null);
      return;
    }

    const rect = container.getBoundingClientRect();
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const gap = 8;
    const margin = 8;
    const preferredWidth = 420;
    const minWidth = 320;
    const availableRight = viewportWidth - rect.right - gap - margin;
    const width =
      availableRight >= minWidth
        ? Math.min(preferredWidth, availableRight)
        : Math.min(
            preferredWidth,
            Math.max(minWidth, viewportWidth - margin * 2),
          );
    const left =
      availableRight >= minWidth
        ? rect.right + gap
        : Math.max(
            margin,
            Math.min(rect.right - width, viewportWidth - width - margin),
          );

    setSecondaryOverlayStyle({
      position: "fixed",
      left,
      top: Math.max(margin, rect.top),
      width,
      height: Math.max(
        240,
        Math.min(
          rect.height,
          viewportHeight - Math.max(margin, rect.top) - margin,
        ),
      ),
      zIndex: 60,
    });
  }, [selectedTarget]);

  const updateSecondaryOverlayPosition = useCallback(() => {
    if (secondaryPositionFrameRef.current !== null) return;
    secondaryPositionFrameRef.current = window.requestAnimationFrame(() => {
      secondaryPositionFrameRef.current = null;
      measureSecondaryOverlayPosition();
    });
  }, [measureSecondaryOverlayPosition]);

  useLayoutEffect(() => {
    if (!selectedTarget) {
      setSecondaryOverlayStyle(null);
      return;
    }

    measureSecondaryOverlayPosition();
    window.addEventListener("resize", updateSecondaryOverlayPosition);
    window.addEventListener("scroll", updateSecondaryOverlayPosition, true);
    const observer =
      typeof ResizeObserver === "undefined" || !containerRef.current
        ? null
        : new ResizeObserver(updateSecondaryOverlayPosition);
    if (containerRef.current) {
      observer?.observe(containerRef.current);
    }

    return () => {
      window.removeEventListener("resize", updateSecondaryOverlayPosition);
      window.removeEventListener(
        "scroll",
        updateSecondaryOverlayPosition,
        true,
      );
      observer?.disconnect();
      if (secondaryPositionFrameRef.current !== null) {
        window.cancelAnimationFrame(secondaryPositionFrameRef.current);
        secondaryPositionFrameRef.current = null;
      }
    };
  }, [
    selectedTarget,
    measureSecondaryOverlayPosition,
    updateSecondaryOverlayPosition,
  ]);

  useEffect(() => {
    if (!selectedTarget) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closeSecondaryPane();
      }
    };
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (secondaryOverlayRef.current?.contains(target)) return;
      if (containerRef.current?.contains(target)) return;
      closeSecondaryPane();
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("pointerdown", handlePointerDown, true);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("pointerdown", handlePointerDown, true);
    };
  }, [selectedTarget, closeSecondaryPane]);

  const enqueuePaneCopies = useCallback(
    (
      source: FileExplorerPaneEndpoint,
      target: FileExplorerPaneEndpoint,
      entries: FileExplorerCopyEntry[],
    ) => {
      if (!target.currentPath || entries.length === 0) return;
      enqueueCopies(
        entries.map((entry) => ({
          fileName: entry.name,
          kind: entry.isDirectory ? "directory" : "file",
          source: {
            sessionId: source.sessionId,
            kind: source.kind,
            path: entry.path,
          },
          target: {
            sessionId: target.sessionId,
            kind: target.kind,
            path: target.currentPath,
          },
        })),
      );
      toast.success(t("fileExplorer.copyQueued", { count: entries.length }));
    },
    [enqueueCopies, t],
  );

  const enqueueEntriesToSessionCwd = useCallback(
    async (
      source: FileExplorerPaneEndpoint,
      entries: FileExplorerCopyEntry[],
      targetSessionId: string,
    ) => {
      if (entries.length === 0) return;

      const targetSession = browsableSessions.find(
        (session) => session.id === targetSessionId,
      );
      if (!targetSession) {
        toast.error(t("fileExplorer.targetCwdUnavailable"));
        return;
      }

      try {
        const targetKind = getSessionExplorerKind(targetSession);
        const liveEndpoint =
          targetSessionId === primaryEndpoint?.sessionId
            ? primaryEndpoint
            : targetSessionId === secondaryEndpoint?.sessionId
              ? secondaryEndpoint
              : null;
        const cachedPath =
          fileExplorerSessionCacheStore.get(targetSessionId)?.currentPath ?? "";
        const livePath =
          liveEndpoint?.kind === targetKind
            ? normalizeExplorerPath(liveEndpoint.currentPath, targetKind)
            : "";
        let targetPath =
          livePath || normalizeExplorerPath(cachedPath, targetKind);
        if (!targetPath) {
          const cwd = await invoke<string | null>("try_get_terminal_cwd", {
            sessionId: targetSessionId,
          });
          targetPath = normalizeExplorerPath(cwd ?? "", targetKind);
        }
        if (!targetPath) {
          toast.error(t("fileExplorer.targetCwdUnavailable"));
          return;
        }

        enqueuePaneCopies(
          source,
          {
            sessionId: targetSessionId,
            kind: targetKind,
            currentPath: targetPath,
          },
          entries,
        );
      } catch (error) {
        logger.error({
          domain: "transfer.lifecycle",
          event: "copy.target_cwd_failed",
          message: "Failed to enqueue copy to target session current directory",
          ids: { session_id: targetSessionId },
          error,
        });
        toast.error(getErrorMessage(error));
      }
    },
    [
      browsableSessions,
      enqueuePaneCopies,
      primaryEndpoint,
      secondaryEndpoint,
      t,
    ],
  );

  const primaryActions = canShowDualButton ? (
    <DropdownMenu
      open={targetSelectorOpen}
      onOpenChange={setTargetSelectorOpen}
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className={cn(
                "text-muted-foreground hover:text-foreground",
                selectedTarget && "bg-primary/10 text-primary",
              )}
              aria-label={t("fileExplorer.dualPane")}
            >
              <PiColumnsPlusRightBold className="size-4" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">{t("fileExplorer.dualPane")}</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="min-w-56">
        {targetCandidates.map((session) => (
          <DropdownMenuItem
            key={session.id}
            onClick={() => {
              setTargetSessionId(session.id);
              setTargetSelectorOpen(false);
            }}
          >
            <span className="min-w-0 flex-1 truncate">{session.name}</span>
            <span className="ml-2 shrink-0 text-[0.625rem] text-muted-foreground">
              {session.session_type}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  ) : null;

  const secondaryActions = selectedTarget ? (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      className="text-muted-foreground hover:text-foreground"
      aria-label={t("common.close")}
      onClick={() => {
        closeSecondaryPane();
      }}
    >
      <MdClose className="size-4" />
    </Button>
  ) : null;

  const secondaryPane =
    selectedTarget && secondaryOverlayStyle
      ? createPortal(
          <div
            ref={secondaryOverlayRef}
            className="overflow-hidden rounded-md border shadow-xl"
            style={{
              ...secondaryOverlayStyle,
              borderColor: "var(--df-primary)",
              backgroundColor: "var(--df-bg-panel)",
              boxShadow:
                "0 0 0 1px color-mix(in srgb, var(--df-primary) 35%, transparent), 0 10px 30px rgba(0,0,0,0.35)",
            }}
          >
            <FileExplorerPane
              activeSessionId={selectedTarget.id}
              activeSessionType={toFileExplorerSessionType(selectedTarget)}
              activeConnectionId={null}
              activeSessionName={selectedTarget.name}
              headerMeta={`${selectedTarget.name} · ${
                selectedTarget.connected
                  ? t("fileExplorer.connected")
                  : t("fileExplorer.disconnected")
              }`}
              headerActions={secondaryActions}
              peerEndpoint={primaryEndpoint}
              onDirectoryStateChange={setSecondaryEndpoint}
              onSendEntries={(source, entries) => {
                if (primaryEndpoint) {
                  enqueuePaneCopies(source, primaryEndpoint, entries);
                }
              }}
              sendTargetOptions={secondarySendTargetOptions}
              onSendEntriesToTarget={(source, entries, sessionId) => {
                void enqueueEntriesToSessionCwd(source, entries, sessionId);
              }}
            />
          </div>,
          document.body,
        )
      : null;

  return (
    <div ref={containerRef} className="relative h-full min-h-0">
      <FileExplorerPane
        {...props}
        activeSessionName={
          props.activeSessionName ?? currentSession?.name ?? null
        }
        headerActions={primaryActions}
        peerEndpoint={secondaryEndpoint}
        onOpenPeerSelector={() => {
          if (!selectedTarget && targetCandidates.length > 0) {
            setTargetSelectorOpen(true);
          }
        }}
        onDirectoryStateChange={setPrimaryEndpoint}
        onSendEntries={(source, entries) => {
          if (secondaryEndpoint) {
            enqueuePaneCopies(source, secondaryEndpoint, entries);
          }
        }}
        sendTargetOptions={primarySendTargetOptions}
        onSendEntriesToTarget={(source, entries, sessionId) => {
          void enqueueEntriesToSessionCwd(source, entries, sessionId);
        }}
      />

      {secondaryPane}
    </div>
  );
}

interface FileExplorerPaneProps
  extends FileExplorerProps, FileExplorerPaneExtraProps {
  onSendEntries?: (
    source: FileExplorerPaneEndpoint,
    entries: FileExplorerCopyEntry[],
  ) => void;
}

/** Remote or local file browser pane. Lists dirs/files, supports navigation. */
function FileExplorerPane({
  activeSessionId,
  activeSessionType,
  activeConnectionId,
  activeSessionName,
  terminalInputEnabled = true,
  headerMeta,
  headerActions,
  peerEndpoint,
  onOpenPeerSelector,
  onDirectoryStateChange,
  onSendEntries,
  sendTargetOptions = [],
  onSendEntriesToTarget,
}: FileExplorerPaneProps) {
  const { t } = useTranslation();
  const {
    appSettings,
    updateUi,
    savedConnections,
    tabs,
    activeTabId,
    setActivePane,
    openFileDocument,
    syncGroups,
    broadcastToAll,
  } = useApp();
  const { enqueueDownloads, enqueueUploads } = useTransfer();
  const hasSshSession = !!activeSessionId && activeSessionType === "SSH";
  const hasLocalSession = !!activeSessionId && activeSessionType === "Local";
  const explorerBackend: FileExplorerBackendKind = hasLocalSession
    ? "local"
    : "remote";
  const [remoteFileBrowserEnabled, setRemoteFileBrowserEnabled] = useState<
    boolean | null
  >(null);
  const canBrowseFiles =
    hasLocalSession || (hasSshSession && remoteFileBrowserEnabled === true);
  const canUseRemoteTransfer =
    hasSshSession && remoteFileBrowserEnabled === true;
  const hasUnsupportedSession =
    !!activeSessionId &&
    !!activeSessionType &&
    activeSessionType !== "SSH" &&
    activeSessionType !== "Local";
  const hasRemoteFileBrowserDisabled =
    hasSshSession && remoteFileBrowserEnabled === false;
  const isResolvingRemoteFileBrowser =
    hasSshSession && remoteFileBrowserEnabled === null;

  const [files, setFiles] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState("");
  const [homeDir, setHomeDir] = useState("");
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [fileSearchQuery, setFileSearchQuery] = useState("");
  const [isFileSearchExpanded, setIsFileSearchExpanded] = useState(false);
  const [fileSortMode, setFileSortMode] = useState<FileSortMode>({
    column: "name",
    direction: "asc",
  });
  const [fileListColumnWidths, setFileListColumnWidths] =
    useState<FileListColumnWidths>(DEFAULT_FILE_LIST_COLUMN_WIDTHS);
  const lastSelectedRef = useRef<string | null>(null);
  const [isEditingPath, setIsEditingPath] = useState(false);
  const [pathInputText, setPathInputText] = useState("");
  const [directoryLoading, setDirectoryLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [inlineRenameState, setInlineRenameState] =
    useState<InlineRenameState | null>(null);
  const [deleteDialogData, setDeleteDialogData] =
    useState<DeleteDialogData | null>(null);
  const [moveDialogData, setMoveDialogData] = useState<MoveDialogData | null>(
    null,
  );
  const [newItemDialogData, setNewItemDialogData] =
    useState<NewItemDialogData | null>(null);
  const [newSymlinkDialogData, setNewSymlinkDialogData] =
    useState<NewSymlinkDialogData | null>(null);
  const [propertiesDialogData, setPropertiesDialogData] =
    useState<PropertiesDialogData | null>(null);
  const [treeContextRow, setTreeContextRow] =
    useState<FileExplorerTreeRow | null>(null);
  const [treeRevealRequest, setTreeRevealRequest] = useState<{
    id: number;
    path: string;
  } | null>(null);
  const [cwdTrackingActive, setCwdTrackingActive] = useState(false);
  const [directorySessionReadyId, setDirectorySessionReadyId] = useState<
    string | null
  >(null);
  const [visitedHistory, setVisitedHistory] = useState<string[]>([]);
  const alwaysUploadFilesRef = useRef<Set<string>>(new Set());
  const filesRef = useRef<FileEntry[]>([]);
  const activeSessionIdRef = useRef<string | null>(null);
  const canBrowseFilesRef = useRef(canBrowseFiles);
  const canUseRemoteTransferRef = useRef(canUseRemoteTransfer);
  const explorerBackendRef = useRef<FileExplorerBackendKind>(explorerBackend);
  const currentPathRef = useRef("");
  const currentPathRawTokenRef = useRef<string | undefined>(undefined);
  const directoryLoadGenerationRef = useRef(0);
  const homeDirRef = useRef("");
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const fileSearchInputRef = useRef<HTMLInputElement | null>(null);
  const preserveFileSearchCaretRef = useRef(false);
  const pathInputRef = useRef<HTMLInputElement | null>(null);
  const pendingRevealNameRef = useRef<string | null>(null);
  const inlineRenameScopeRef = useRef("");
  const historyRef = useRef<string[]>([]);
  const historyIndexRef = useRef(-1);
  const visitedHistoryRef = useRef<string[]>([]);
  const dragSelectionRef = useRef<{
    anchor: string;
    baseSelection: Set<string>;
    additive: boolean;
  } | null>(null);

  const sessionCacheRef = useRef(fileExplorerSessionCacheStore);
  const prevSessionIdRef = useRef<string | null>(null);
  const autoSyncCwdMountSyncKeyRef = useRef<string | null>(null);
  const [isExternalDropActive, setIsExternalDropActive] = useState(false);
  const [listScrollTop, setListScrollTop] = useState(0);
  const [listViewportHeight, setListViewportHeight] = useState(0);
  const refreshUploadCompletionTimerRef = useRef<ReturnType<
    typeof setTimeout
  > | null>(null);

  filesRef.current = files;
  activeSessionIdRef.current = activeSessionId;
  canBrowseFilesRef.current = canBrowseFiles;
  canUseRemoteTransferRef.current = canUseRemoteTransfer;
  explorerBackendRef.current = explorerBackend;
  currentPathRef.current = currentPath;
  homeDirRef.current = homeDir;
  visitedHistoryRef.current = visitedHistory;

  const resetExternalDropHover = useCallback(() => {
    setIsExternalDropActive(false);
  }, []);

  const beginPathEditing = useCallback(() => {
    setPathInputText(currentPathRef.current || homeDirRef.current);
    setIsEditingPath(true);
    window.requestAnimationFrame(() => pathInputRef.current?.select());
  }, []);

  const autoSyncConnectionIds =
    appSettings.ui.file_explorer_auto_sync_cwd_connection_ids ?? [];
  const autoSyncScopeId =
    activeConnectionId ?? (hasLocalSession ? "local" : null);
  const favoriteDirectoriesByConnection =
    appSettings.ui.file_explorer_favorite_dirs_by_connection_id ?? {};
  const favoriteScopeId =
    activeConnectionId ?? (hasLocalSession ? "local" : null);
  const favoriteDirectories = favoriteScopeId
    ? (favoriteDirectoriesByConnection[favoriteScopeId] ?? [])
    : [];
  const showHiddenFiles =
    appSettings.ui.file_explorer_show_hidden_files ?? true;
  const fileExplorerViewMode = normalizeFileExplorerViewMode(
    appSettings.ui.file_explorer_view_mode,
  );
  const isTreeView = fileExplorerViewMode === "tree";
  const autoSyncCwd =
    !isTreeView &&
    !!autoSyncScopeId &&
    autoSyncConnectionIds.includes(autoSyncScopeId);
  const treeRevealRequestIdRef = useRef(0);
  const requestTreeReveal = useCallback(
    (path: string) => {
      const normalizedPath = normalizeExplorerPath(path, explorerBackend);
      if (!normalizedPath) return;
      treeRevealRequestIdRef.current += 1;
      setTreeRevealRequest({
        id: treeRevealRequestIdRef.current,
        path: normalizedPath,
      });
    },
    [explorerBackend],
  );
  const listScrollResetKey = `${activeSessionId ?? ""}:${currentPath}`;
  const listFilterResetKey = `${fileSearchQuery}:${fileSortMode.column}:${fileSortMode.direction}`;
  const activeConnection = useMemo(
    () =>
      activeConnectionId
        ? (savedConnections.find(
            (connection) => connection.id === activeConnectionId,
          ) ?? null)
        : null,
    [activeConnectionId, savedConnections],
  );
  const fileWindowTarget = useMemo(
    () =>
      buildFileWindowTarget({
        backend: explorerBackend,
        connection: activeConnection,
        sessionName: activeSessionName,
        remoteLabel: t("fileEditor.remoteTarget"),
      }),
    [activeConnection, activeSessionName, explorerBackend, t],
  );
  const activeFilePath = useMemo(() => {
    const activeTab = activeTabId
      ? tabs.find((tab) => tab.id === activeTabId)
      : null;
    const activePane = activeTab
      ? findSessionPaneById(activeTab.root, activeTab.activePaneId)
      : null;
    if (
      !activePane ||
      activePane.paneKind !== "file" ||
      activePane.sessionId !== activeSessionId ||
      activePane.file.backend !== explorerBackend
    ) {
      return null;
    }
    return activePane.file.path;
  }, [activeSessionId, activeTabId, explorerBackend, tabs]);

  useEffect(() => {
    if (!onDirectoryStateChange) return;
    if (!activeSessionId || !canBrowseFiles || !currentPath) {
      onDirectoryStateChange(null);
      return;
    }
    onDirectoryStateChange({
      sessionId: activeSessionId,
      kind: explorerBackend,
      currentPath,
    });
  }, [
    activeSessionId,
    canBrowseFiles,
    currentPath,
    explorerBackend,
    onDirectoryStateChange,
  ]);

  useEffect(() => {
    if (isTreeView) {
      setListScrollTop(0);
      setListViewportHeight(0);
      return;
    }

    const container = listContainerRef.current;
    if (!container) {
      setListScrollTop(0);
      setListViewportHeight(0);
      return;
    }

    let scrollFrame = 0;
    const updateMetrics = () => {
      setListScrollTop(container.scrollTop);
      setListViewportHeight(container.clientHeight);
    };
    const handleScroll = () => {
      if (scrollFrame !== 0) {
        return;
      }

      scrollFrame = window.requestAnimationFrame(() => {
        scrollFrame = 0;
        setListScrollTop(container.scrollTop);
      });
    };

    updateMetrics();
    container.addEventListener("scroll", handleScroll, { passive: true });

    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            updateMetrics();
          });
    resizeObserver?.observe(container);

    return () => {
      container.removeEventListener("scroll", handleScroll);
      resizeObserver?.disconnect();
      if (scrollFrame !== 0) {
        window.cancelAnimationFrame(scrollFrame);
      }
    };
  }, [isTreeView]);

  useEffect(() => {
    if (isTreeView) return;
    if (!listScrollResetKey && !listContainerRef.current) {
      setListScrollTop(0);
      return;
    }

    const container = listContainerRef.current;
    if (container) {
      container.scrollTop = 0;
      container.scrollLeft = 0;
    }
    setListScrollTop(0);
  }, [isTreeView, listScrollResetKey]);

  useEffect(() => {
    if (isTreeView) return;
    if (!listFilterResetKey && !listContainerRef.current) {
      setListScrollTop(0);
      return;
    }

    const container = listContainerRef.current;
    if (container) {
      container.scrollTop = 0;
      container.scrollLeft = 0;
    }
    setListScrollTop(0);
  }, [isTreeView, listFilterResetKey]);

  useEffect(() => {
    if (!isFileSearchExpanded) {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      const input = fileSearchInputRef.current;
      if (!input) return;
      input.focus();
      if (preserveFileSearchCaretRef.current) {
        preserveFileSearchCaretRef.current = false;
        input.setSelectionRange(input.value.length, input.value.length);
      } else {
        input.select();
      }
    });

    return () => window.cancelAnimationFrame(frame);
  }, [isFileSearchExpanded]);

  const resolveUploadTarget = useCallback(
    (directoryPath = currentPathRef.current) => {
      if (!activeSessionId || !canUseRemoteTransfer) return null;

      return {
        sessionId: activeSessionId,
        remoteDir:
          normalizeDirectoryPath(directoryPath) ||
          homeDirRef.current ||
          "/",
      };
    },
    [activeSessionId, canUseRemoteTransfer],
  );

  useEffect(() => {
    return () => {
      if (!activeSessionId) return;
      const snapshot = buildSessionCacheSnapshot(
        filesRef.current,
        currentPathRef.current,
        homeDirRef.current,
        historyRef.current,
        historyIndexRef.current,
        visitedHistoryRef.current,
        explorerBackendRef.current,
      );
      if (snapshot) {
        sessionCacheRef.current.set(activeSessionId, snapshot);
      }
    };
  }, [activeSessionId]);

  useEffect(() => {
    const handleMouseUp = () => {
      dragSelectionRef.current = null;
    };

    window.addEventListener("mouseup", handleMouseUp);
    return () => {
      window.removeEventListener("mouseup", handleMouseUp);
    };
  }, []);

  // Keep the in-memory per-session cache bounded to live sessions so closed
  // sessions release their cached directory listing and history.
  useEffect(() => {
    const pruneClosedSessions = async () => {
      const cache = sessionCacheRef.current;
      if (cache.size === 0) return;
      try {
        const sessions = await invoke<SessionInfo[]>("list_sessions");
        const liveIds = new Set(sessions.map((session) => session.id));
        for (const sessionId of [...cache.keys()]) {
          if (!liveIds.has(sessionId)) {
            cache.delete(sessionId);
            clearDirectoryChildrenCacheForSession(sessionId);
            clearFileExplorerTreeSessionCache(sessionId);
          }
        }
      } catch {
        // Backend might be unavailable; keep the cache untouched until next event.
      }
    };

    void pruneClosedSessions();
    const unlisten = listen("sessions-changed", () => {
      void pruneClosedSessions();
    });
    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  // Resolve whether backend terminal-path tracking is available for this session.
  useEffect(() => {
    if ((!hasSshSession && !hasLocalSession) || !activeSessionId) {
      setCwdTrackingActive(false);
      setRemoteFileBrowserEnabled(null);
      return;
    }
    setRemoteFileBrowserEnabled(hasLocalSession ? true : null);
    return subscribeFileExplorerSessionSnapshots({
      listenSessionsChanged: (handler) => listen("sessions-changed", handler),
      readSessions: () => invoke<SessionInfo[]>("list_sessions"),
      onSessions: (sessions) => {
        const session = sessions.find((session) => session.id === activeSessionId);
        setCwdTrackingActive(canTrackTerminalCwd(session));
        setRemoteFileBrowserEnabled(
          hasLocalSession ? true : (session?.remote_file_browser_enabled ?? true),
        );
      },
      onError: () => {
        setCwdTrackingActive(false);
        setRemoteFileBrowserEnabled(true);
      },
    });
  }, [activeSessionId, hasLocalSession, hasSshSession]);

  useEffect(() => {
    const unlisten = listen<{
      session_id: string;
      local_path: string;
      remote_path: string;
    }>("file-modified", (e) => {
        const { session_id, local_path, remote_path } = e.payload;
        const watchKey = `${session_id}:${local_path}`;

        if (alwaysUploadFilesRef.current.has(watchKey)) {
          // File was marked "Always list", just upload silently
          invoke("upload_local_file", {
            sessionId: session_id,
            localPath: local_path,
            remotePath: remote_path,
          }).catch((err) =>
            logger.error({
              domain: "watcher.sync",
              event: "auto_upload.failed",
              message: "Auto upload failed",
              ids: { session_id },
              error: err,
            }),
          );
        } else {
          // Trigger the window
          openAutoUpload({
            sessionId: session_id,
            localPath: local_path,
            remotePath: remote_path,
          });
        }
    });

    const unlistenDecision = listen<{
      sessionId: string;
      localPath: string;
      always: boolean;
    }>("auto-upload-decision", (e) => {
        const { sessionId, localPath, always } = e.payload;
        if (always) {
          alwaysUploadFilesRef.current.add(`${sessionId}:${localPath}`);
        }
    });

    return () => {
      unlisten.then((fn) => fn());
      unlistenDecision.then((fn) => fn());
    };
  }, []);

  const pushDirectoryHistory = useCallback((path: string) => {
    const normalizedPath = normalizeExplorerPath(
      path,
      explorerBackendRef.current,
    );
    const currentIndex = historyIndexRef.current;
    const currentEntry =
      currentIndex >= 0 ? historyRef.current[currentIndex] : null;
    if (currentEntry === normalizedPath) {
      return;
    }

    const nextHistory = historyRef.current.slice(0, currentIndex + 1);
    nextHistory.push(normalizedPath);
    historyRef.current = nextHistory;
    historyIndexRef.current = nextHistory.length - 1;
  }, []);

  const loadDirectory = useCallback(
    async (path: string, options?: LoadDirectoryOptions) => {
      const requestSessionId = activeSessionId;
      const requestBackend = explorerBackendRef.current;
      const requestGeneration = ++directoryLoadGenerationRef.current;
      const isCurrentRequest = () =>
        activeSessionIdRef.current === requestSessionId &&
        explorerBackendRef.current === requestBackend &&
        directoryLoadGenerationRef.current === requestGeneration;

      if (!canBrowseFiles || !requestSessionId || !isCurrentRequest()) {
        return false;
      }

      const normalizedPath = normalizeExplorerPath(path, requestBackend);
      if (!normalizedPath) return false;
      const historyMode = options?.history ?? "push";
      const rawPathToken =
        options?.rawPathToken ??
        (normalizeExplorerPath(currentPathRef.current, requestBackend) ===
        normalizedPath
          ? currentPathRawTokenRef.current
          : undefined);
      setDirectoryLoading(true);
      setError(null);

      try {
        const entries =
          options?.entries ??
          (requestBackend === "local"
            ? await invoke<FileEntry[]>("list_local_dir", {
                sessionId: requestSessionId,
                path: normalizedPath,
              })
            : await invoke<FileEntry[]>("list_remote_dir", {
                sessionId: requestSessionId,
                path: normalizedPath,
                rawPathToken,
              }));

        if (!isCurrentRequest()) {
          // A newer navigation or session owns the explorer state.
          return true;
        }

        const pathChanged =
          normalizeExplorerPath(currentPathRef.current, requestBackend) !==
          normalizedPath;
        const selectEntryName = options?.selectEntryName;
        if (historyMode === "push") {
          pushDirectoryHistory(normalizedPath);
        }

        const nextVisitedHistory = pushVisitedHistory(
          visitedHistoryRef.current,
          normalizedPath,
          requestBackend,
        );
        visitedHistoryRef.current = nextVisitedHistory;

        startTransition(() => {
          setFiles(entries);
          setCurrentPath(normalizedPath);
          currentPathRawTokenRef.current = rawPathToken;
          setVisitedHistory(nextVisitedHistory);
          setSelectedFiles((prev) => {
            if (pathChanged) {
              const shouldSelectEntry =
                !!selectEntryName &&
                entries.some((entry) => entry.name === selectEntryName);
              if (shouldSelectEntry) {
                lastSelectedRef.current = selectEntryName;
                pendingRevealNameRef.current = selectEntryName;
                return new Set([selectEntryName]);
              }

              lastSelectedRef.current = null;
              pendingRevealNameRef.current = null;
              return new Set();
            }

            const entryNames = new Set(entries.map((entry) => entry.name));
            if (selectEntryName && entryNames.has(selectEntryName)) {
              lastSelectedRef.current = selectEntryName;
              pendingRevealNameRef.current = selectEntryName;
              return new Set([selectEntryName]);
            }

            const next = new Set(
              [...prev].filter((name) => entryNames.has(name)),
            );
            if (
              lastSelectedRef.current &&
              !entryNames.has(lastSelectedRef.current)
            ) {
              lastSelectedRef.current = null;
            }
            return next;
          });
        });

        if (!isCurrentRequest()) return true;

        const cached = sessionCacheRef.current.get(requestSessionId);
        const snapshot = buildSessionCacheSnapshot(
          entries,
          normalizedPath,
          cached?.homeDir ?? homeDirRef.current,
          historyRef.current,
          historyIndexRef.current,
          nextVisitedHistory,
          requestBackend,
        );
        if (snapshot) {
          sessionCacheRef.current.set(requestSessionId, snapshot);
        }
        return true;
      } catch (e) {
        if (!isCurrentRequest()) {
          return true;
        }
        if (options?.silent) {
          return false;
        }
        const msg = String(e);
        if (filesRef.current.length > 0) {
          toast.error(msg);
        } else {
          setError(msg);
        }
        return false;
      } finally {
        if (isCurrentRequest()) {
          setDirectoryLoading(false);
        }
      }
    },
    [activeSessionId, canBrowseFiles, pushDirectoryHistory],
  );

  // Tree expansion uses the existing one-directory commands so the remote
  // SFTP/SCP fallback remains unchanged. It deliberately does not call the
  // current-directory loader, which would mutate navigation history.
  const treeRootPath = getTreeRootPath(
    currentPath || homeDir,
    homeDir,
    explorerBackend,
  );
  const treeInitialRootEntries =
    treeRootPath &&
    currentPath &&
    isSameExplorerDirectory(currentPath, treeRootPath, explorerBackend)
      ? files
      : null;
  const loadTreeDirectoryEntries = useCallback(
    async (path: string, rawPathToken?: string) => {
      if (!activeSessionId || !canBrowseFiles || !isTreeView) return [];
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (!normalizedPath) return [];
      return backend === "local"
        ? invoke<FileEntry[]>("list_local_dir", {
            sessionId: activeSessionId,
            path: normalizedPath,
          })
        : invoke<FileEntry[]>("list_remote_dir", {
            sessionId: activeSessionId,
            path: normalizedPath,
            rawPathToken,
          });
    },
    [activeSessionId, canBrowseFiles, isTreeView],
  );
  const handleTreeDirectorySelected = useCallback(
    (target: FileExplorerTreeEntry, entries: FileEntry[]) => {
      void loadDirectory(target.path, {
        rawPathToken: target.rawPathToken,
        entries,
      });
    },
    [loadDirectory],
  );
  const treeState = useFileExplorerTree({
    sessionId: activeSessionId ?? "",
    enabled: canBrowseFiles && isTreeView,
    backend: explorerBackend,
    rootPath: treeRootPath,
    rootRawPathToken:
      treeInitialRootEntries !== null
        ? currentPathRawTokenRef.current
        : undefined,
    homeDir,
    currentPath,
    showHiddenFiles,
    initialRootEntries: treeInitialRootEntries,
    loadDirectory: loadTreeDirectoryEntries,
    onDirectorySelected: handleTreeDirectorySelected,
  });
  useEffect(() => {
    if (!isTreeView || !currentPath) return;
    requestTreeReveal(currentPath);
  }, [currentPath, isTreeView, requestTreeReveal]);

  const selectedTreeRows = treeState.selectedRows.filter((row) => !row.isRoot);
  const getTreeOperationDirectoryPath = () => {
    if (selectedTreeRows.length === 1) {
      const row = selectedTreeRows[0];
      return row.entry.is_dir ? row.path : row.parentPath;
    }
    return currentPathRef.current || homeDirRef.current;
  };
  const invalidateTreeDirectories = treeState.invalidateDirectories;
  const clearTreeSelection = treeState.clearSelection;
  const refreshAllTree = treeState.refreshAll;

  const handleLocatePath = useCallback(async () => {
    if (!isTreeView || !activeSessionId || !canBrowseFiles) return;
    try {
      const normalizedActiveFilePath = normalizeExplorerPath(
        activeFilePath ?? "",
        explorerBackend,
      );
      if (normalizedActiveFilePath) {
        requestTreeReveal(normalizedActiveFilePath);
        await treeState.revealPath(normalizedActiveFilePath, {
          targetType: "file",
        });
        return;
      }

      const cwd = await invoke<string | null>("try_get_terminal_cwd", {
        sessionId: activeSessionId,
      });
      const normalizedCwd = normalizeExplorerPath(cwd ?? "", explorerBackend);
      if (!normalizedCwd) {
        toast.error(t("fileExplorer.targetCwdUnavailable"));
        return;
      }
      const loaded = await loadDirectory(normalizedCwd, {
        history: "preserve",
      });
      if (!loaded) return;
      requestTreeReveal(normalizedCwd);
      void treeState.revealPath(normalizedCwd).catch(() => {});
    } catch (error) {
      toast.error(`${t("fileExplorer.syncFailed")}: ${getErrorMessage(error)}`);
    }
  }, [
    activeFilePath,
    activeSessionId,
    canBrowseFiles,
    explorerBackend,
    isTreeView,
    loadDirectory,
    requestTreeReveal,
    t,
    treeState.revealPath,
  ]);

  const refreshExplorerDirectories = useCallback(
    async (paths: string[]) => {
      const backend = explorerBackendRef.current;
      const normalizedPaths = [
        ...new Set(
          paths
            .map((path) => normalizeExplorerPath(path, backend))
            .filter((path): path is string => !!path),
        ),
      ];
      if (normalizedPaths.length === 0) return false;

      for (const path of normalizedPaths) {
        clearDirectoryChildrenCacheForPath(
          activeSessionIdRef.current,
          backend,
          path,
        );
      }
      if (isTreeView) {
        invalidateTreeDirectories(normalizedPaths);
      }

      const current = currentPathRef.current;
      const currentNeedsRefresh = normalizedPaths.some((path) =>
        isSameExplorerDirectory(path, current, backend),
      );
      if (!currentNeedsRefresh) return true;
      return loadDirectory(current, { history: "preserve" });
    },
    [isTreeView, invalidateTreeDirectories, loadDirectory],
  );

  const refreshCurrentDirectory = useCallback(() => {
    const backend = explorerBackendRef.current;
    const targetPath =
      normalizeExplorerPath(currentPathRef.current, backend) ||
      normalizeExplorerPath(homeDirRef.current, backend);
    if (!targetPath) return Promise.resolve(false);
    clearDirectoryChildrenCacheForPath(
      activeSessionIdRef.current,
      backend,
      targetPath,
    );
    return loadDirectory(targetPath);
  }, [loadDirectory]);

  const uploadLocalEntriesToTarget = useCallback(
    (
      target: { sessionId: string; remoteDir: string },
      entries: Array<{ path: string; isDir: boolean }>,
    ) => {
      if (entries.length === 0) return;

      enqueueUploads(
        entries
          .filter((entry) => !!entry.path)
          .map((entry) => {
            const fileName = getLocalPathName(
              entry.path,
              entry.isDir ? "uploaded_folder" : "uploaded_file",
            );
            return {
              sessionId: target.sessionId,
              fileName,
              localPath: entry.path,
              remotePath: buildRemoteUploadPath(target.remoteDir, fileName),
              kind: entry.isDir ? ("directory" as const) : ("file" as const),
            };
          }),
      );
    },
    [enqueueUploads],
  );

  const resolveLocalDropPaths = useCallback(async (paths: string[]) => {
    const uniquePaths = Array.from(
      new Set(paths.map((path) => path.trim()).filter((path) => !!path)),
    );
    if (uniquePaths.length === 0) {
      return [];
    }

    return invoke<ResolvedLocalDropPathEntry[]>("resolve_local_drop_paths", {
      paths: uniquePaths,
    });
  }, []);

  const processExternalDropPaths = useCallback(
    async (
      target: { sessionId: string; remoteDir: string },
      dropPaths: string[],
    ) => {
      try {
        const resolvedLocalEntries = await resolveLocalDropPaths(dropPaths);
        if (resolvedLocalEntries.length === 0) {
          logger.warn({
            domain: "ui.error",
            event: "file_explorer.external_drop_paths_unresolved",
            message:
              "Native external drop did not resolve to usable local paths",
            ids: { session_id: target.sessionId },
            data: {
              remote_dir: target.remoteDir,
              path_count: dropPaths.length,
            },
          });
          toast.error(t("fileExplorer.externalDropPathsRequired"));
          return;
        }

        await uploadLocalEntriesToTarget(
          target,
          resolvedLocalEntries.map((entry) => ({
            path: entry.path,
            isDir: entry.isDir,
          })),
        );
      } catch (error) {
        logger.error({
          domain: "ui.error",
          event: "file_explorer.external_drop_failed",
          message: "Failed to process native external drop paths",
          ids: { session_id: target.sessionId },
          data: {
            remote_dir: target.remoteDir,
            path_count: dropPaths.length,
          },
          error,
        });
        toast.error(String(error));
      }
    },
    [resolveLocalDropPaths, t, uploadLocalEntriesToTarget],
  );

  useEffect(() => {
    directoryLoadGenerationRef.current += 1;
    setDirectorySessionReadyId(null);
    resetExternalDropHover();
    const cache = sessionCacheRef.current;
    const prevId = prevSessionIdRef.current;

    if (prevId !== activeSessionId) {
      currentPathRawTokenRef.current = undefined;
      directoryLoadGenerationRef.current += 1;
    }

    if (prevId && prevId !== activeSessionId) {
      const snapshot = buildSessionCacheSnapshot(
        filesRef.current,
        currentPathRef.current,
        homeDirRef.current,
        historyRef.current,
        historyIndexRef.current,
        visitedHistoryRef.current,
        explorerBackendRef.current,
      );
      if (snapshot) {
        cache.set(prevId, snapshot);
      }
    }
    prevSessionIdRef.current = activeSessionId;

    if (!canBrowseFiles || !activeSessionId) {
      setDirectorySessionReadyId(null);
      directoryLoadGenerationRef.current += 1;
      setFiles([]);
      setCurrentPath("");
      setHomeDir("");
      setError(null);
      setSelectedFiles(new Set());
      historyRef.current = [];
      historyIndexRef.current = -1;
      visitedHistoryRef.current = [];
      setVisitedHistory([]);
      lastSelectedRef.current = null;
      return;
    }

    const cached = cache.get(activeSessionId);
    if (cached?.currentPath) {
      setFiles(cached.files);
      setCurrentPath(cached.currentPath);
      setHomeDir(cached.homeDir);
      setSelectedFiles(new Set());
      setError(null);
      historyRef.current = [...cached.history];
      historyIndexRef.current = cached.historyIndex;
      visitedHistoryRef.current = [...cached.visitedHistory];
      setVisitedHistory([...cached.visitedHistory]);
      setDirectorySessionReadyId(activeSessionId);
      lastSelectedRef.current = null;
      return;
    }

    historyRef.current = [];
    historyIndexRef.current = -1;
    visitedHistoryRef.current = [];
    setVisitedHistory([]);
    lastSelectedRef.current = null;
    setSelectedFiles(new Set());

    let cancelled = false;
    const markDirectorySessionReady = () => {
      if (!cancelled) {
        setDirectorySessionReadyId(activeSessionId);
      }
    };
    (async () => {
      const loadRootDirectory = async () => {
        if (cancelled) return false;
        homeDirRef.current = "";
        setHomeDir("");
        return loadDirectory("/");
      };

      const backend = explorerBackendRef.current;
      const cachedHome = normalizeExplorerPath(cached?.homeDir ?? "", backend);
      if (cachedHome) {
        homeDirRef.current = cachedHome;
        setHomeDir(cachedHome);
        const loaded = await loadDirectory(cachedHome);
        if (cancelled || loaded) {
          if (loaded) markDirectorySessionReady();
          return;
        }
      }

      try {
        const home = normalizeExplorerPath(
          await invoke<string>(
            backend === "local" ? "get_local_home_dir" : "get_home_dir",
            {
            sessionId: activeSessionId,
            },
          ),
          backend,
        );
        if (cancelled) return;
        if (home) {
          homeDirRef.current = home;
          setHomeDir(home);
          const loaded = await loadDirectory(home);
          if (cancelled || loaded) {
            if (loaded) markDirectorySessionReady();
            return;
          }
        }
      } catch {
        if (cancelled) {
          return;
        }
      }

      await loadRootDirectory();
      markDirectorySessionReady();
    })();
    return () => {
      cancelled = true;
    };
  }, [activeSessionId, canBrowseFiles, loadDirectory, resetExternalDropHover]);

  useEffect(() => {
    if (!activeSessionId) {
      autoSyncCwdMountSyncKeyRef.current = null;
      return;
    }

    if (!autoSyncCwd) {
      autoSyncCwdMountSyncKeyRef.current = null;
      return;
    }

    if (!cwdTrackingActive) {
      autoSyncCwdMountSyncKeyRef.current = null;
      return;
    }

    if (
      directorySessionReadyId !== activeSessionId ||
      !canBrowseFiles ||
      !currentPath
    ) {
      autoSyncCwdMountSyncKeyRef.current = null;
      return;
    }

    const syncKey = `${activeSessionId}:${autoSyncScopeId ?? ""}:${explorerBackend}`;
    if (autoSyncCwdMountSyncKeyRef.current === syncKey) {
      return;
    }

    let cancelled = false;
    autoSyncCwdMountSyncKeyRef.current = syncKey;
    void syncExplorerDirectoryToTerminalCwd({
      enabled: autoSyncCwd,
      canBrowseFiles,
      sessionId: activeSessionId,
      backend: explorerBackend,
      currentPath,
      readTerminalCwd: (sessionId) =>
        invoke<string | null>("try_get_terminal_cwd", { sessionId }),
      loadDirectory: (path, options) =>
        cancelled ? Promise.resolve(false) : loadDirectory(path, options),
    });

    return () => {
      cancelled = true;
    };
  }, [
    activeSessionId,
    autoSyncCwd,
    autoSyncScopeId,
    canBrowseFiles,
    currentPath,
    cwdTrackingActive,
    directorySessionReadyId,
    explorerBackend,
    loadDirectory,
  ]);

  useEffect(() => {
    if (isEditingPath) {
      pathInputRef.current?.focus();
    }
  }, [isEditingPath]);

  useExternalFileDrop({
    activeSessionIdRef,
    canBrowseFilesRef: canUseRemoteTransferRef,
    currentPathRef,
    homeDirRef,
    listContainerRef,
    resetExternalDropHover,
    setIsExternalDropActive,
    processExternalDropPaths,
    externalDropPathsRequiredMessage: t(
      "fileExplorer.externalDropPathsRequired",
    ),
  });

  useEffect(() => {
    const unlisten = listen<{
      session_id: string;
      remote_path: string;
      direction: string;
      status: string;
      parent_id?: string;
    }>("transfer-event", (event) => {
      const payload = event.payload;
      if (
        payload.direction !== "upload" ||
        payload.status !== "completed" ||
        payload.parent_id ||
        payload.session_id !== activeSessionIdRef.current
      ) {
        return;
      }

      const visibleDir = normalizeExplorerPath(
        currentPathRef.current,
        "remote",
      );
      const uploadedParent = getExplorerParentDirectory(
        payload.remote_path,
        "remote",
      );
      if (isTreeView) {
        invalidateTreeDirectories([uploadedParent]);
      }
      if (!visibleDir || uploadedParent !== visibleDir) {
        return;
      }

      if (refreshUploadCompletionTimerRef.current) {
        clearTimeout(refreshUploadCompletionTimerRef.current);
      }
      refreshUploadCompletionTimerRef.current = setTimeout(() => {
        refreshUploadCompletionTimerRef.current = null;
        clearDirectoryChildrenCacheForPath(
          activeSessionIdRef.current,
          "remote",
          visibleDir,
        );
        void refreshCurrentDirectory();
      }, 250);
    });

    return () => {
      unlisten.then((fn) => fn());
      if (refreshUploadCompletionTimerRef.current) {
        clearTimeout(refreshUploadCompletionTimerRef.current);
        refreshUploadCompletionTimerRef.current = null;
      }
    };
  }, [isTreeView, invalidateTreeDirectories, refreshCurrentDirectory]);

  const visibleFiles = useMemo(
    () =>
      showHiddenFiles
        ? files
        : files.filter((entry) => !entry.name.startsWith(".")),
    [files, showHiddenFiles],
  );

  const filteredSortedFiles = useMemo(
    () =>
      visibleFiles
        .filter((entry) => matchesFileSearch(entry, fileSearchQuery))
        .sort((left, right) => compareFileEntries(left, right, fileSortMode)),
    [visibleFiles, fileSearchQuery, fileSortMode],
  );

  useEffect(() => {
    if (showHiddenFiles) {
      return;
    }

    setSelectedFiles((prev) => {
      const next = new Set([...prev].filter((name) => !name.startsWith(".")));
      return next.size === prev.size ? prev : next;
    });

    if (lastSelectedRef.current?.startsWith(".")) {
      lastSelectedRef.current = null;
    }
  }, [showHiddenFiles]);

  useEffect(() => {
    const nextScope = `${activeSessionId ?? ""}:${currentPath}`;
    if (inlineRenameScopeRef.current === nextScope) {
      return;
    }
    inlineRenameScopeRef.current = nextScope;
    setInlineRenameState(null);
  }, [activeSessionId, currentPath]);

  useEffect(() => {
    if (isTreeView) return;
    setInlineRenameState((prev) => {
      if (
        !prev ||
        filteredSortedFiles.some((entry) => entry.name === prev.entryName)
      ) {
        return prev;
      }
      return null;
    });
  }, [filteredSortedFiles, isTreeView]);

  useEffect(() => {
    if (!isTreeView) return;
    setIsFileSearchExpanded(false);
    setIsEditingPath(false);
  }, [isTreeView]);

  const isFileSearchActive = fileSearchQuery.trim().length > 0;
  const fileListGridTemplate = useMemo(
    () =>
      FILE_LIST_COLUMNS.map(
        (column) => `${fileListColumnWidths[column.id]}px`,
      ).join(" "),
    [fileListColumnWidths],
  );
  const fileListTableWidth = useMemo(
    () =>
      FILE_LIST_COLUMNS.reduce(
        (sum, column) => sum + fileListColumnWidths[column.id],
        0,
      ),
    [fileListColumnWidths],
  );

  const handleSortColumn = useCallback((column: FileSortColumn) => {
    setFileSortMode((prev) => {
      if (prev.column === column) {
        return {
          column,
          direction: prev.direction === "asc" ? "desc" : "asc",
        };
      }

      return {
        column,
        direction: DEFAULT_FILE_SORT_DIRECTIONS[column],
      };
    });
  }, []);

  const handleColumnResizeMouseDown = useCallback(
    (column: FileSortColumn, event: ReactMouseEvent<HTMLSpanElement>) => {
      event.preventDefault();
      event.stopPropagation();

      const startX = event.clientX;
      const startWidth = fileListColumnWidths[column];
      const minWidth = MIN_FILE_LIST_COLUMN_WIDTHS[column];

      const handleMouseMove = (moveEvent: MouseEvent) => {
        const nextWidth = Math.max(
          minWidth,
          startWidth + moveEvent.clientX - startX,
        );
        setFileListColumnWidths((prev) =>
          prev[column] === nextWidth ? prev : { ...prev, [column]: nextWidth },
        );
      };
      const handleMouseUp = () => {
        window.removeEventListener("mousemove", handleMouseMove);
        window.removeEventListener("mouseup", handleMouseUp);
      };

      window.addEventListener("mousemove", handleMouseMove);
      window.addEventListener("mouseup", handleMouseUp);
    },
    [fileListColumnWidths],
  );

  const getRangeSelection = useCallback(
    (
      anchorName: string,
      targetName: string,
      baseSelection = new Set<string>(),
      additive = false,
    ) => {
      const names = filteredSortedFiles.map((file) => file.name);
      const anchorIndex = names.indexOf(anchorName);
      const targetIndex = names.indexOf(targetName);
      if (anchorIndex < 0 || targetIndex < 0) {
        return additive ? new Set(baseSelection) : new Set<string>();
      }

      const [start, end] =
        anchorIndex < targetIndex
          ? [anchorIndex, targetIndex]
          : [targetIndex, anchorIndex];
      const next = additive ? new Set(baseSelection) : new Set<string>();
      for (let index = start; index <= end; index += 1) {
        next.add(names[index]);
      }
      return next;
    },
    [filteredSortedFiles],
  );

  const handleSelectionStart = useCallback(
    (entry: FileEntry, event: ReactMouseEvent) => {
      if (event.button !== 0) return;

      listContainerRef.current?.focus();
      if (isParentDirectoryEntry(entry)) {
        dragSelectionRef.current = null;
        lastSelectedRef.current = null;
        setSelectedFiles(new Set([PARENT_DIRECTORY_ENTRY_NAME]));
        return;
      }

      const additive = event.ctrlKey || event.metaKey;
      setSelectedFiles((prev) => {
        const hasRangeAnchor = event.shiftKey && !!lastSelectedRef.current;
        const anchor = hasRangeAnchor
          ? (lastSelectedRef.current ?? entry.name)
          : entry.name;
        const baseSelection = additive ? new Set(prev) : new Set<string>();
        baseSelection.delete(PARENT_DIRECTORY_ENTRY_NAME);
        let next: Set<string>;

        if (hasRangeAnchor) {
          next = getRangeSelection(anchor, entry.name, baseSelection, additive);
        } else if (additive) {
          next = new Set(prev);
          next.delete(PARENT_DIRECTORY_ENTRY_NAME);
          if (next.has(entry.name)) {
            next.delete(entry.name);
          } else {
            next.add(entry.name);
          }
        } else {
          next = new Set([entry.name]);
        }

        dragSelectionRef.current = {
          anchor,
          baseSelection,
          additive,
        };
        lastSelectedRef.current = entry.name;
        return next;
      });
    },
    [getRangeSelection],
  );

  const handleSelectionDrag = useCallback(
    (entry: FileEntry, event: ReactMouseEvent) => {
      if (isParentDirectoryEntry(entry)) {
        return;
      }

      const dragSelection = dragSelectionRef.current;
      if (!dragSelection || (event.buttons & 1) !== 1) {
        return;
      }

      setSelectedFiles(
        getRangeSelection(
          dragSelection.anchor,
          entry.name,
          dragSelection.baseSelection,
          dragSelection.additive,
        ),
      );
      lastSelectedRef.current = entry.name;
    },
    [getRangeSelection],
  );

  const handleContextMenuSelection = useCallback(
    (entry: FileEntry, _event: ReactMouseEvent) => {
    listContainerRef.current?.focus();
    if (isParentDirectoryEntry(entry)) {
      dragSelectionRef.current = null;
      lastSelectedRef.current = null;
      setSelectedFiles(new Set([PARENT_DIRECTORY_ENTRY_NAME]));
      return;
    }

    setSelectedFiles((prev) => {
      if (prev.has(entry.name)) {
        return prev;
      }
      lastSelectedRef.current = entry.name;
      return new Set([entry.name]);
    });
    },
    [],
  );

  const navigateHistory = useCallback(
    async (direction: -1 | 1) => {
      const nextIndex = historyIndexRef.current + direction;
      const nextPath = historyRef.current[nextIndex];
      if (!nextPath) {
        return;
      }

      const previousIndex = historyIndexRef.current;
      historyIndexRef.current = nextIndex;
      const loaded = await loadDirectory(nextPath, { history: "preserve" });
      if (!loaded) {
        historyIndexRef.current = previousIndex;
      }
    },
    [loadDirectory],
  );

  const handleSelectHistoryPath = useCallback(
    (path: string) => {
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (
        !normalizedPath ||
        normalizedPath ===
          normalizeExplorerPath(currentPathRef.current, backend)
      ) {
        return;
      }
      setFileSearchQuery("");
      void loadDirectory(normalizedPath);
    },
    [loadDirectory],
  );

  const handleNavigateDirectory = useCallback(
    async (path: string, options?: LoadDirectoryOptions) => {
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (!normalizedPath) return false;
      setFileSearchQuery("");
      return loadDirectory(normalizedPath, options);
    },
    [loadDirectory],
  );

  const listChildDirectories = useCallback(
    async (path: string) => {
      if (!activeSessionId) return [];
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (!normalizedPath) return [];
      return backend === "local"
        ? await invoke<DirectoryChild[]>("list_local_child_directories", {
            sessionId: activeSessionId,
            path: normalizedPath,
            showHiddenFiles,
          })
        : await invoke<DirectoryChild[]>("list_remote_child_directories", {
            sessionId: activeSessionId,
            path: normalizedPath,
            rawPathToken:
              normalizedPath ===
              normalizeExplorerPath(currentPathRef.current, backend)
                ? currentPathRawTokenRef.current
                : undefined,
            showHiddenFiles,
          });
    },
    [activeSessionId, showHiddenFiles],
  );

  const handleItemClick = (entry: FileEntry) => {
    if (isParentDirectoryEntry(entry)) {
      handleGoUp();
      return;
    }

    if (entry.is_dir) {
      const newPath = joinExplorerPath(
        currentPath,
        entry.name,
        explorerBackendRef.current,
      );
      loadDirectory(newPath, { rawPathToken: entry.raw_path_token });
    } else {
      setSelectedFiles(new Set([entry.name]));
      lastSelectedRef.current = entry.name;
    }
  };

  const handleNewFile = (directoryPath = currentPath) => {
    if (!activeSessionId) return;
    setNewItemDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      currentDirPath: directoryPath,
      type: "file",
    });
  };

  const handleNewFolder = (directoryPath = currentPath) => {
    if (!activeSessionId) return;
    setNewItemDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      currentDirPath: directoryPath,
      type: "folder",
    });
  };

  const handleNewSymlink = (directoryPath = currentPath) => {
    if (!activeSessionId) return;
    setNewSymlinkDialogData({
      sessionId: activeSessionId,
      currentDirPath: directoryPath,
    });
  };

  const handleCurrentDirProperties = (
    directoryPath = currentPath,
    rawPathToken = currentPathRawTokenRef.current,
  ) => {
    if (!activeSessionId || !directoryPath) return;
    const name = getLocalPathName(directoryPath, directoryPath);
    setPropertiesDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      fullPath: directoryPath,
      rawPathToken,
      name,
      is_dir: true,
    });
  };

  const handleCopyCurrentPath = () => {
    navigator.clipboard.writeText(currentPath);
  };

  const sendTextToTerminal = useCallback(
    (text: string) => {
      if (!activeSessionId || !text || !terminalInputEnabled) return;
      const peerSessionIds = getSessionInputPeerIds(
        activeSessionId,
        syncGroups,
        tabs,
        broadcastToAll,
      );
      const sendInput =
        peerSessionIds.length > 0
          ? sendSessionInputWithSync(activeSessionId, text, peerSessionIds)
          : sendSessionInput(activeSessionId, text);

      sendInput.catch(() => {});
      emit(`focus-terminal-${activeSessionId}`).catch(() => {});
    },
    [activeSessionId, broadcastToAll, syncGroups, tabs, terminalInputEnabled],
  );

  const handleSendCurrentPathToTerminal = () => {
    sendTextToTerminal(currentPath);
  };

  const selectedRealFiles = useMemo(
    () => filteredSortedFiles.filter((file) => selectedFiles.has(file.name)),
    [filteredSortedFiles, selectedFiles],
  );
  const footerStats = useMemo(
    () => ({
      selectedFileSize: selectedRealFiles.reduce(
        (sum, file) => (file.is_dir ? sum : sum + file.size),
        0,
      ),
      selectedItemCount: selectedRealFiles.length,
      totalFileSize: visibleFiles.reduce(
        (sum, file) => (file.is_dir ? sum : sum + file.size),
        0,
      ),
      totalItemCount: visibleFiles.length,
    }),
    [selectedRealFiles, visibleFiles],
  );
  const treeFooterStats = useMemo(() => {
    const treeRows = treeState.rows.filter((row) => !row.isRoot);
    const selectedFileSize = selectedTreeRows.reduce(
      (sum, row) => (row.entry.is_dir ? sum : sum + row.entry.size),
      0,
    );
    const totalFileSize = treeRows.reduce(
      (sum, row) => (row.entry.is_dir ? sum : sum + row.entry.size),
      0,
    );
    return {
      selectedFileSize,
      selectedItemCount: selectedTreeRows.length,
      totalFileSize,
      totalItemCount: treeRows.length,
    };
  }, [selectedTreeRows, treeState.rows]);
  const activeFooterStats = isTreeView ? treeFooterStats : footerStats;
  const footerSizeText =
    activeFooterStats.selectedItemCount > 0 && activeFooterStats.selectedFileSize > 0
      ? `${formatSize(activeFooterStats.selectedFileSize)}/${formatSize(activeFooterStats.totalFileSize)}`
      : formatSize(activeFooterStats.totalFileSize);
  const fileAiActions = useMemo(
    () =>
      appSettings.ai.enabled
        ? appSettings.ai.file_ai_actions.filter(
            (action) => action.enabled && action.name.trim(),
          )
        : [],
    [appSettings.ai.enabled, appSettings.ai.file_ai_actions],
  );

  const handleDeleteSelected = () => {
    if (selectedRealFiles.length === 0) return;
    openDeleteDialog(selectedRealFiles);
  };

  const handlePreview = async (entry: FileEntry, fullPath?: string) => {
    if (!activeSessionId || entry.is_dir) return;
    const resolvedPath =
      fullPath || joinExplorerPath(currentPath, entry.name, explorerBackend);
    try {
      await openFilePreview({
        sessionId: activeSessionId,
        backend: explorerBackendRef.current,
        path: resolvedPath,
        name: entry.name,
        size: entry.size,
        mtime: entry.mtime,
        target: fileWindowTarget,
      });
    } catch (error) {
      toast.error(getErrorMessage(error) || t("filePreview.openFailed"));
    }
  };

  const handleToggleHiddenFiles = useCallback(() => {
    updateUi((prev) => ({
      file_explorer_show_hidden_files: !(
        prev.file_explorer_show_hidden_files ?? true
      ),
    }));
  }, [updateUi]);

  const handleToggleViewMode = useCallback(() => {
    updateUi({
      file_explorer_view_mode: isTreeView ? "list" : "tree",
    });
  }, [isTreeView, updateUi]);

  const handleListKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.isContentEditable ||
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT")
    ) {
      return;
    }

    if (isTreeView) {
      if (
        target instanceof HTMLElement &&
        target.closest("[data-file-tree-path]")
      ) {
        return;
      }
      if (
        (event.ctrlKey || event.metaKey) &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "a"
      ) {
        event.preventDefault();
        event.stopPropagation();
        treeState.selectAll();
      } else if (
        event.key === "Delete" &&
        !event.altKey &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.shiftKey
      ) {
        event.preventDefault();
        event.stopPropagation();
        handleTreeDelete(selectedTreeRows);
      }
      return;
    }

    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "l"
    ) {
      event.preventDefault();
      event.stopPropagation();
      beginPathEditing();
      return;
    }

    if (
      event.key.length === 1 &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.nativeEvent.isComposing &&
      !inlineRenameState
    ) {
      event.preventDefault();
      event.stopPropagation();
      preserveFileSearchCaretRef.current = true;
      setFileSearchQuery(event.key);
      setIsFileSearchExpanded(true);
      window.requestAnimationFrame(() => {
        const input = fileSearchInputRef.current;
        if (!input) return;
        input.focus();
        input.setSelectionRange(input.value.length, input.value.length);
      });
      return;
    }

    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      event.stopPropagation();
      const nextSelection = new Set(
        filteredSortedFiles.map((entry) => entry.name),
      );
      setSelectedFiles(nextSelection);
      lastSelectedRef.current = filteredSortedFiles[0]?.name ?? null;
      return;
    }

    if (
      matchesKeyEvent(
        resolveShortcutKeys("fileExplorer.rename", appSettings.keybindings),
        event.nativeEvent,
      ) &&
      selectedRealFiles.length === 1 &&
      activeSessionId &&
      !inlineRenameState
    ) {
      event.preventDefault();
      event.stopPropagation();
      beginInlineRename(selectedRealFiles[0]);
      return;
    }

    if (
      event.key !== "Delete" ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey ||
      selectedRealFiles.length === 0 ||
      deleteDialogData
    ) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    handleDeleteSelected();
  };

  const handleGoUp = () => {
    const backend = explorerBackendRef.current;
    const normalizedPath = normalizeExplorerPath(currentPath, backend);
    if (!normalizedPath) return;
    const parentPath = getExplorerParentDirectory(normalizedPath, backend);
    if (!parentPath || parentPath === normalizedPath) return;
    const exitedName = getLocalPathName(normalizedPath, normalizedPath);
    setFileSearchQuery("");
    void loadDirectory(parentPath, { selectEntryName: exitedName });
  };

  const handlePanelMouseDownCapture = useCallback(
    (event: ReactMouseEvent<HTMLElement>) => {
      if (
        !isTreeView &&
        (event.button === 3 || event.button === 4)
      ) {
        event.preventDefault();
        event.stopPropagation();
      }
    },
    [isTreeView],
  );

  const handlePanelMouseUpCapture = useCallback(
    (event: ReactMouseEvent<HTMLElement>) => {
      if (!canBrowseFiles || isTreeView) return;

      if (event.button === 3) {
        event.preventDefault();
        event.stopPropagation();
        void navigateHistory(-1);
      } else if (event.button === 4) {
        event.preventDefault();
        event.stopPropagation();
        void navigateHistory(1);
      }
    },
    [canBrowseFiles, isTreeView, navigateHistory],
  );

  const handleSyncCwd = useCallback(async () => {
    if (!activeSessionId) return;
    try {
      const cwd = await invoke<string>("get_terminal_cwd", {
        sessionId: activeSessionId,
      });
      const backend = explorerBackendRef.current;
      const normalizedCwd = normalizeExplorerPath(cwd, backend);
      if (
        normalizedCwd &&
        !isSameExplorerDirectory(normalizedCwd, currentPathRef.current, backend)
      ) {
        loadDirectory(normalizedCwd);
      }
    } catch (e) {
      toast.error(`${t("fileExplorer.syncFailed")}: ${e}`);
    }
  }, [activeSessionId, loadDirectory, t]);

  const handleToggleAutoSyncCwd = useCallback(() => {
    if (!autoSyncScopeId) return;
    updateUi((prev) => {
      const currentIds = prev.file_explorer_auto_sync_cwd_connection_ids ?? [];
      const enabled = currentIds.includes(autoSyncScopeId);
      return {
        file_explorer_auto_sync_cwd_connection_ids: enabled
          ? currentIds.filter((id) => id !== autoSyncScopeId)
          : [...currentIds, autoSyncScopeId],
      };
    });
  }, [autoSyncScopeId, updateUi]);

  const addFavoriteDirectory = useCallback(
    (path: string) => {
      if (!favoriteScopeId) return;
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (!normalizedPath) return;
      const alreadyExists = favoriteDirectories.includes(normalizedPath);

      if (alreadyExists) {
        toast.success(
          t("fileExplorer.favoriteExists", { path: normalizedPath }),
        );
        return;
      }

      updateUi((prev) => {
        const currentMap =
          prev.file_explorer_favorite_dirs_by_connection_id ?? {};
        const currentList = currentMap[favoriteScopeId] ?? [];
        if (currentList.includes(normalizedPath)) {
          return {
            file_explorer_favorite_dirs_by_connection_id: currentMap,
          };
        }

        return {
          file_explorer_favorite_dirs_by_connection_id: {
            ...currentMap,
            [favoriteScopeId]: [...currentList, normalizedPath],
          },
        };
      });

      toast.success(t("fileExplorer.favoriteAdded", { path: normalizedPath }));
    },
    [favoriteScopeId, favoriteDirectories, t, updateUi],
  );

  const handleAddCurrentDirectoryToFavorites = useCallback(() => {
    addFavoriteDirectory(currentPathRef.current || homeDirRef.current);
  }, [addFavoriteDirectory]);

  const handleSelectFavoritePath = useCallback(
    (path: string) => {
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (
        !normalizedPath ||
        normalizedPath ===
          normalizeExplorerPath(currentPathRef.current, backend)
      ) {
        return;
      }
      setFileSearchQuery("");
      void loadDirectory(normalizedPath);
    },
    [loadDirectory],
  );

  const handleRemoveFavoritePath = useCallback(
    (path: string) => {
      if (!favoriteScopeId) return;
      const backend = explorerBackendRef.current;
      const normalizedPath = normalizeExplorerPath(path, backend);
      if (!normalizedPath) return;

      updateUi((prev) => {
        const currentMap =
          prev.file_explorer_favorite_dirs_by_connection_id ?? {};
        const currentList = currentMap[favoriteScopeId] ?? [];
        return {
          file_explorer_favorite_dirs_by_connection_id: {
            ...currentMap,
            [favoriteScopeId]: currentList.filter(
              (item) => item !== normalizedPath,
            ),
          },
        };
      });
      toast.success(
        t("fileExplorer.favoriteRemoved", { path: normalizedPath }),
      );
    },
    [favoriteScopeId, t, updateUi],
  );

  const handleAddEntryToFavorites = useCallback(
    (entry: FileEntry, fullPath?: string) => {
      if (!entry.is_dir || isParentDirectoryEntry(entry)) return;
      const targetPath =
        fullPath ||
        joinExplorerPath(
          currentPathRef.current,
          entry.name,
          explorerBackendRef.current,
        );
      addFavoriteDirectory(targetPath);
    },
    [addFavoriteDirectory],
  );

  useEffect(() => {
    if (!autoSyncCwd || !activeSessionId) return;

    const sessionId = activeSessionId;
    let disposed = false;
    const unlisten = listen<string>(`cwd-changed-${sessionId}`, (event) => {
      if (disposed || activeSessionIdRef.current !== sessionId) return;

      syncExplorerDirectoryToTerminalCwdChange({
        backend: explorerBackendRef.current,
        currentPath: currentPathRef.current,
        cwd: event.payload,
        loadDirectory,
      });
    });
    return () => {
      disposed = true;
      void unlisten.then((fn) => fn());
    };
  }, [autoSyncCwd, activeSessionId, loadDirectory]);

  const getEntryFullPath = useCallback(
    (entry: FileEntry, basePath = currentPath) => {
      return joinExplorerPath(basePath, entry.name, explorerBackend);
    },
    [currentPath, explorerBackend],
  );

  const beginInlineRename = useCallback(
    (
      entry: FileEntry,
      targetPath = joinExplorerPath(currentPath, entry.name, explorerBackend),
      targetParentPath = currentPath,
    ) => {
      if (!activeSessionId || isParentDirectoryEntry(entry)) return;

      dragSelectionRef.current = null;
      lastSelectedRef.current = entry.name;
      setSelectedFiles(new Set([entry.name]));
      setInlineRenameState({
        entryName: entry.name,
        oldPath: targetPath,
        parentPath: targetParentPath,
        oldRawPathToken: entry.raw_path_token,
        initialName: entry.name,
        value: entry.name,
        isSubmitting: false,
      });
    },
    [activeSessionId, currentPath, explorerBackend],
  );

  const cancelInlineRename = useCallback(() => {
    setInlineRenameState((prev) => (prev?.isSubmitting ? prev : null));
  }, []);

  const handleInlineRenameSubmit = useCallback(async () => {
    if (
      !activeSessionId ||
      !inlineRenameState ||
      inlineRenameState.isSubmitting
    )
      return;

    const newName = inlineRenameState.value.trim();
    if (!newName || newName === inlineRenameState.initialName) {
      setInlineRenameState(null);
      return;
    }

    const backend = explorerBackendRef.current;
    const parentPath =
      normalizeExplorerPath(inlineRenameState.parentPath, backend) ||
      normalizeExplorerPath(currentPathRef.current, backend);
    const newPath = joinExplorerPath(parentPath, newName, backend);
    setInlineRenameState((prev) =>
      prev && prev.entryName === inlineRenameState.entryName
        ? { ...prev, value: newName, isSubmitting: true }
        : prev,
    );

    try {
      if (backend === "local") {
        await invoke("rename_local_file", {
          sessionId: activeSessionId,
          oldPath: inlineRenameState.oldPath,
          newPath,
        });
      } else {
        await invoke("rename_remote_file", {
          sessionId: activeSessionId,
          oldPath: inlineRenameState.oldPath,
          newPath,
          oldRawPathToken: inlineRenameState.oldRawPathToken,
        });
      }
      clearDirectoryChildrenCacheForPath(
        activeSessionIdRef.current,
        backend,
        parentPath,
      );
      if (isTreeView) {
        invalidateTreeDirectories([parentPath]);
      }
      if (
        isSameExplorerDirectory(
          parentPath,
          currentPathRef.current,
          backend,
        )
      ) {
        await loadDirectory(parentPath, {
          history: "preserve",
          selectEntryName: newName,
        });
      }
      setInlineRenameState(null);
    } catch (e) {
      toast.error(String(e));
      setInlineRenameState((prev) =>
        prev && prev.entryName === inlineRenameState.entryName
          ? { ...prev, isSubmitting: false }
          : prev,
      );
    }
  }, [
    activeSessionId,
    inlineRenameState,
    invalidateTreeDirectories,
    isTreeView,
    loadDirectory,
  ]);

  const getEntryAiActions = (entry: FileEntry) => {
    if (entry.is_dir || entry.size > appSettings.ai.max_ai_file_size_bytes) {
      return [];
    }
    return fileAiActions;
  };

  const handleFileAIAction = async (
    entry: FileEntry,
    action: AICustomActionConfig,
    fullPath = getEntryFullPath(entry),
  ) => {
    if (!activeSessionId) return;
    const backend = explorerBackendRef.current;
    const filePath = fullPath;
    try {
      const result = await invoke<RemoteTextFile>(
        backend === "local" ? "read_local_file_text" : "read_remote_file_text",
        {
          sessionId: activeSessionId,
          path: filePath,
          maxBytes: appSettings.ai.max_ai_file_size_bytes,
        },
      );
      openAIAssistant({
        action: "custom_file_action",
        userInput: action.prompt,
        selectedText: result.content,
        metadata: {
          actionId: action.id,
          actionName: action.name,
          filePath,
          fileSize: result.size,
        },
      });
    } catch (error) {
      toast.error(getErrorMessage(error) || t("ai.fileUnsupported"));
    }
  };

  const handleCopyPath = (
    entry: FileEntry,
    mode: "dir" | "name" | "full",
    targetPath?: string,
    targetParentPath = currentPath,
  ) => {
    let text = "";
    if (mode === "dir") text = targetParentPath;
    else if (mode === "name") text = entry.name;
    else text = targetPath || getEntryFullPath(entry, targetParentPath);
    navigator.clipboard.writeText(text);
  };

  const handleSendToTerminal = (
    entry: FileEntry,
    mode: "dir" | "name" | "full",
    targetPath?: string,
    targetParentPath = currentPath,
  ) => {
    if (!activeSessionId) return;
    let text = "";
    if (mode === "dir") text = targetParentPath;
    else if (mode === "name") text = entry.name;
    else text = targetPath || getEntryFullPath(entry, targetParentPath);

    sendTextToTerminal(text);
  };

  const buildDeleteItems = (
    entries: FileEntry[],
    basePath = currentPath,
    pathResolver?: (entry: FileEntry) => string,
  ): DeleteDialogItem[] => {
    return entries.map((entry) => ({
      path: pathResolver?.(entry) || getEntryFullPath(entry, basePath),
      name: entry.name,
      rawPathToken: entry.raw_path_token,
    }));
  };

  const buildMoveItems = (
    entries: FileEntry[],
    basePath = currentPath,
    pathResolver?: (entry: FileEntry) => string,
  ): MoveDialogItem[] => {
    return entries.map((entry) => ({
      oldPath: pathResolver?.(entry) || getEntryFullPath(entry, basePath),
      oldRawPathToken: entry.raw_path_token,
      name: entry.name,
      isDirectory: entry.is_dir,
    }));
  };

  const getContextMenuEntries = useCallback(
    (entry: FileEntry) => {
      if (isParentDirectoryEntry(entry)) {
        return [];
      }

      if (selectedFiles.size > 1 && selectedFiles.has(entry.name)) {
        return filteredSortedFiles.filter((file) =>
          selectedFiles.has(file.name),
        );
      }
      return [entry];
    },
    [filteredSortedFiles, selectedFiles],
  );

  const handleSendToPeer = useCallback(
    (entry: FileEntry) => {
      if (!activeSessionId || isParentDirectoryEntry(entry)) return;
      if (!peerEndpoint) {
        onOpenPeerSelector?.();
        return;
      }
      const entries = getContextMenuEntries(entry).map((item) => ({
        name: item.name,
        path: getEntryFullPath(item),
        isDirectory: item.is_dir,
      }));
      if (entries.length === 0) return;
      onSendEntries?.(
        {
          sessionId: activeSessionId,
          kind: explorerBackend,
          currentPath,
        },
        entries,
      );
    },
    [
      activeSessionId,
      currentPath,
      explorerBackend,
      getContextMenuEntries,
      getEntryFullPath,
      onOpenPeerSelector,
      onSendEntries,
      peerEndpoint,
    ],
  );

  const handleSendToTarget = useCallback(
    (entry: FileEntry, targetSessionId: string) => {
      if (!activeSessionId || isParentDirectoryEntry(entry)) return;
      const entries = getContextMenuEntries(entry).map((item) => ({
        name: item.name,
        path: getEntryFullPath(item),
        isDirectory: item.is_dir,
      }));
      if (entries.length === 0) return;
      onSendEntriesToTarget?.(
        {
          sessionId: activeSessionId,
          kind: explorerBackend,
          currentPath,
        },
        entries,
        targetSessionId,
      );
    },
    [
      activeSessionId,
      currentPath,
      explorerBackend,
      getContextMenuEntries,
      getEntryFullPath,
      onSendEntriesToTarget,
    ],
  );

  const openDeleteDialog = (entries: FileEntry[]) => {
    if (!activeSessionId || entries.length === 0) return;
    setDeleteDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      items: buildDeleteItems(entries),
    });
  };

  const openMoveDialog = (entries: FileEntry[]) => {
    if (!activeSessionId || entries.length === 0) return;
    setMoveDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      sourceDirectory: currentPath,
      initialTargetDirectory: currentPath,
      items: buildMoveItems(entries),
    });
  };

  const handleMoveFromContextMenu = (entry: FileEntry) => {
    openMoveDialog(getContextMenuEntries(entry));
  };

  const handleMoveSuccess = (targetDirectory: string) => {
    const backend = explorerBackendRef.current;
    const sourceDirectory =
      normalizeExplorerPath(
        moveDialogData?.sourceDirectory ?? currentPathRef.current,
        backend,
      ) || normalizeExplorerPath(homeDirRef.current, backend);
    const refreshPlan = buildMoveSuccessRefreshPlan(
      sourceDirectory,
      targetDirectory,
      backend,
    );
    const movedCurrentDirectory = moveDialogData?.items.find(
      (item) =>
        item.isDirectory &&
        isSameExplorerDirectory(
          item.oldPath,
          currentPathRef.current,
          backend,
        ),
    );
    const nextCurrentPath = movedCurrentDirectory
      ? buildMoveTargetPath(targetDirectory, movedCurrentDirectory.name, backend)
      : null;
    setSelectedFiles(new Set());
    lastSelectedRef.current = null;
    clearTreeSelection();
    if (refreshPlan?.shouldClearSelection) {
      const sourceDirectories = moveDialogData?.items.map((item) =>
        getExplorerParentDirectory(item.oldPath, backend),
      ) ?? [refreshPlan.sourceDirectory];
      void refreshExplorerDirectories([
        ...sourceDirectories,
        refreshPlan.targetDirectory,
      ]);
      if (nextCurrentPath) {
        void loadDirectory(nextCurrentPath);
      }
    }
  };

  const handleDeleteFromContextMenu = (entry: FileEntry) => {
    openDeleteDialog(getContextMenuEntries(entry));
  };

  const resolveDownloadDir = async (): Promise<string> => {
    const configured = appSettings.transfer.download_path;
    if (configured) return configured;
    return downloadDir();
  };

  const sanitizeDownloadFileName = async (name: string): Promise<string> =>
    invoke<string>("sanitize_download_file_name", { name });

  const downloadEntries = async (
    entries: FileEntry[],
    pathResolver?: (entry: FileEntry) => string,
  ) => {
    if (!activeSessionId || entries.length === 0) return;
    const resolveEntryPath = (entry: FileEntry) =>
      pathResolver?.(entry) ?? getEntryFullPath(entry);

    try {
      const askEach = appSettings.transfer.ask_save_location;
      const downloads: Array<{
        sessionId: string;
        fileName: string;
        localPath: string;
        remotePath: string;
        kind: "file" | "directory";
      }> = [];

      if (askEach) {
        if (entries.length === 1) {
          const entry = entries[0];
          const safeName = await sanitizeDownloadFileName(entry.name);
          if (entry.is_dir) {
            const localDir = await openDialog({ directory: true });
            if (!localDir || typeof localDir !== "string") return;
            const localPath = await join(localDir, safeName);
            downloads.push({
              sessionId: activeSessionId,
              fileName: entry.name,
              remotePath: resolveEntryPath(entry),
              localPath,
              kind: "directory",
            });
          } else {
            const localPath = await saveDialog({ defaultPath: safeName });
            if (!localPath) return;
            downloads.push({
              sessionId: activeSessionId,
              fileName: entry.name,
              remotePath: resolveEntryPath(entry),
              localPath,
              kind: "file",
            });
          }
        } else {
          const localDir = await openDialog({ directory: true });
          if (!localDir || typeof localDir !== "string") return;

          for (const entry of entries) {
            const safeName = await sanitizeDownloadFileName(entry.name);
            const localPath = await join(localDir, safeName);
            downloads.push({
              sessionId: activeSessionId,
              fileName: entry.name,
              remotePath: resolveEntryPath(entry),
              localPath,
              kind: entry.is_dir ? "directory" : "file",
            });
          }
        }
        enqueueDownloads(downloads);
        return;
      }

      const defaultDir = await resolveDownloadDir();

      for (const entry of entries) {
        const safeName = await sanitizeDownloadFileName(entry.name);
        const localPath = await join(defaultDir, safeName);
        downloads.push({
          sessionId: activeSessionId,
          fileName: entry.name,
          remotePath: resolveEntryPath(entry),
          localPath,
          kind: entry.is_dir ? "directory" : "file",
        });
      }
      enqueueDownloads(downloads);
    } catch (e) {
      logger.error({
        domain: "transfer.lifecycle",
        event: "download.failed",
        message: "Download failed",
        ids: activeSessionId ? { session_id: activeSessionId } : undefined,
        error: e,
      });
    }
  };

  const handleDownloadSelected = async () => {
    if (selectedRealFiles.length === 0) return;
    await downloadEntries(selectedRealFiles);
  };

  const handleDownload = async (entry: FileEntry) => {
    await downloadEntries([entry]);
  };

  const handleDownloadFromContextMenu = async (entry: FileEntry) => {
    if (selectedFiles.size > 1 && selectedFiles.has(entry.name)) {
      const selected = getContextMenuEntries(entry);
      await downloadEntries(selected);
      return;
    }

    await handleDownload(entry);
  };

  const handleUploadFiles = async (directoryPath = currentPath) => {
    if (!canUseRemoteTransfer) return;
    const target = resolveUploadTarget(directoryPath);
    if (!target) return;

    try {
      const localPaths = await openDialog({ multiple: true, directory: false });
      if (!localPaths) return;
      const pathList = (
        Array.isArray(localPaths) ? localPaths : [localPaths]
      ).filter(
        (localPath): localPath is string => typeof localPath === "string",
      );
      await uploadLocalEntriesToTarget(
        target,
        pathList.map((path) => ({
          path,
          isDir: false,
        })),
      );
    } catch (error) {
      logger.error({
        domain: "transfer.lifecycle",
        event: "upload.selection_failed",
        message: "Upload selection failed",
        ids: { session_id: target.sessionId },
        error,
      });
    }
  };

  const handleUploadFolder = async (directoryPath = currentPath) => {
    if (!canUseRemoteTransfer) return;
    const target = resolveUploadTarget(directoryPath);
    if (!target) return;

    try {
      const localDirs = await openDialog({ directory: true, multiple: true });
      if (!localDirs) return;
      const pathList = (
        Array.isArray(localDirs) ? localDirs : [localDirs]
      ).filter((localDir): localDir is string => typeof localDir === "string");
      await uploadLocalEntriesToTarget(
        target,
        pathList.map((path) => ({
          path,
          isDir: true,
        })),
      );
    } catch (error) {
      logger.error({
        domain: "transfer.lifecycle",
        event: "upload.folder_failed",
        message: "Upload folder failed",
        ids: { session_id: target.sessionId },
        error,
      });
    }
  };

  const handleOpenExternal = async (
    entry: FileEntry,
    fullPath = getEntryFullPath(entry),
  ) => {
    if (!activeSessionId || entry.is_dir) return;
    if (explorerBackendRef.current === "local") {
      try {
        await openPath(
          fullPath,
          appSettings.transfer.default_editor || undefined,
        );
      } catch (e) {
        toast.error(String(e));
      }
      return;
    }

    let localPath: string;
    try {
      const tDir = await tempDir();
      const downloadTimestamp = Date.now().toString();
      const safeName = await sanitizeDownloadFileName(entry.name);
      localPath = await join(
        tDir,
        "nyaterm",
        activeSessionId,
        downloadTimestamp,
        safeName,
      );
      await invoke("download_remote_file", {
        sessionId: activeSessionId,
        remotePath: fullPath,
        localPath,
      });
    } catch (e) {
      logger.error({
        domain: "transfer.lifecycle",
        event: "download.open_failed",
        message: "Download for open failed",
        ids: { session_id: activeSessionId },
        error: e,
      });
      return;
    }

    try {
      await invoke("start_file_watch", {
        sessionId: activeSessionId,
        localPath,
        remotePath: fullPath,
      });

      await openPath(
        localPath,
        appSettings.transfer.default_editor || undefined,
      );
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleOpenInternal = async (
    entry: FileEntry,
    fullPath = getEntryFullPath(entry),
  ) => {
    if (
      !activeSessionId ||
      entry.is_dir ||
      (activeSessionType !== "SSH" && activeSessionType !== "Local")
    ) {
      return;
    }

    const backend = explorerBackendRef.current;
    const path = fullPath;
    if (resolveInternalEditorDisplay(appSettings.transfer.internal_editor_display) === "window") {
      try {
        await openRemoteFileEditor({
          sessionId: activeSessionId,
          backend,
          path,
          name: entry.name,
          size: entry.size,
          mtime: entry.mtime,
          target: fileWindowTarget,
        });
      } catch (error) {
        toast.error(
          getErrorMessage(error) || t("fileExplorer.openInternalFailed"),
        );
      }
      return;
    }

    const existing = findOpenFileDocument(tabs, {
      backend,
      sessionId: activeSessionId,
      path,
    });
    if (existing) {
      setActivePane(existing.tabId, existing.paneId);
      return;
    }

    try {
      const result = await invoke<TextFileOpenResult>(
        backend === "local" ? "open_local_file_text" : "open_remote_file_text",
        { sessionId: activeSessionId, path, maxBytes: MAX_EDITOR_FILE_BYTES },
      );
      if (result.status === "unsupported") {
        toast.info(
          t(
            result.reason === "binary"
              ? "fileExplorer.binaryOpenExternal"
              : "fileExplorer.unsupportedEncodingOpenExternal",
          ),
        );
      await handleOpenExternal(entry, fullPath);
      return;
    }

      openFileDocument({
        sessionId: activeSessionId,
        name: entry.name,
        type: activeSessionType,
        connectionId: activeConnectionId ?? undefined,
        backend,
        path,
        file: {
          content: result.file.content,
          size: result.file.size,
          mtime: result.file.mtime ?? entry.mtime,
          mtimeNanos: result.file.mtimeNanos,
          contentHash: result.file.contentHash,
        },
      });
    } catch (error) {
      toast.error(
        getErrorMessage(error) || t("fileExplorer.openInternalFailed"),
      );
    }
  };

  const handleOpenDefault = async (
    entry: FileEntry,
    fullPath = getEntryFullPath(entry),
  ) => {
    if (resolveFileEditorOpenTarget(appSettings.transfer) !== "external") {
      await handleOpenInternal(entry, fullPath);
      return;
    }
    await handleOpenExternal(entry, fullPath);
  };

  const treeActionRows = useCallback(
    (target: FileExplorerTreeRow) => {
      if (target.isRoot) return [];
      const selected = treeState.selectedRows.filter((row) => !row.isRoot);
      return selected.some((row) => row.path === target.path) ? selected : [target];
    },
    [treeState.selectedRows],
  );

  const activateTreeFileParent = useCallback(
    async (row: FileExplorerTreeRow) => {
      if (row.entry.is_dir) return true;
      const parentPath = normalizeExplorerPath(
        row.parentPath,
        explorerBackendRef.current,
      );
      if (!parentPath) return false;
      if (
        isSameExplorerDirectory(
          parentPath,
          currentPathRef.current,
          explorerBackendRef.current,
        )
      ) {
        return true;
      }

      const parentSnapshot = treeState.getDirectorySnapshot(parentPath);
      return loadDirectory(
        parentSnapshot?.path ?? parentPath,
        parentSnapshot
          ? {
              rawPathToken: parentSnapshot.rawPathToken,
              entries: parentSnapshot.entries,
            }
          : undefined,
      );
    },
    [loadDirectory, treeState.getDirectorySnapshot],
  );

  const handleTreeOpenDirectory = useCallback(
    (row: FileExplorerTreeRow) => {
      if (!row.entry.is_dir) return;
      const snapshot = treeState.getDirectorySnapshot(row.path);
      if (snapshot) {
        void loadDirectory(snapshot.path, {
          rawPathToken: snapshot.rawPathToken,
          entries: snapshot.entries,
        });
      } else {
        treeState.activateDirectory(row);
      }
      if (!row.isRoot && !row.entry.is_symlink && !row.isExpanded) {
        treeState.toggleDirectory(row);
      }
    },
    [loadDirectory, treeState.activateDirectory, treeState.getDirectorySnapshot, treeState.toggleDirectory],
  );

  const handleTreePreview = (row: FileExplorerTreeRow) => {
    void (async () => {
      await activateTreeFileParent(row);
      await handlePreview(row.entry, row.path);
    })();
  };

  const handleTreeOpenDefault = (row: FileExplorerTreeRow) => {
    void (async () => {
      await activateTreeFileParent(row);
      await handleOpenDefault(row.entry, row.path);
    })();
  };

  const handleTreeOpenInternal = (row: FileExplorerTreeRow) => {
    void (async () => {
      await activateTreeFileParent(row);
      await handleOpenInternal(row.entry, row.path);
    })();
  };

  const handleTreeOpenExternal = (row: FileExplorerTreeRow) => {
    void (async () => {
      await activateTreeFileParent(row);
      await handleOpenExternal(row.entry, row.path);
    })();
  };

  const handleTreeRefresh = useCallback(
    (row?: FileExplorerTreeRow | null) => {
      const backend = explorerBackendRef.current;
      if (!row) {
        refreshAllTree();
        const current = normalizeExplorerPath(
          currentPathRef.current || homeDirRef.current,
          backend,
        );
        if (!current) return;
        clearDirectoryChildrenCacheForPath(
          activeSessionIdRef.current,
          backend,
          current,
        );
        void loadDirectory(current, { history: "preserve" });
        return;
      }

      const targetPath = row.entry.is_dir ? row.path : row.parentPath;
      const normalizedPath = normalizeExplorerPath(targetPath, backend);
      if (!normalizedPath) return;

      void refreshExplorerDirectories([normalizedPath]);
    },
    [loadDirectory, refreshAllTree, refreshExplorerDirectories],
  );

  const handleTreeDelete = (rows: FileExplorerTreeRow[]) => {
    if (!activeSessionId || rows.length === 0) return;
    const pathByEntry = new Map(rows.map((row) => [row.entry, row.path]));
    const entries = rows.map((row) => row.entry);
    setDeleteDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      items: buildDeleteItems(
        entries,
        currentPath,
        (entry) => pathByEntry.get(entry) ?? currentPath,
      ),
    });
  };

  const handleTreeMove = (rows: FileExplorerTreeRow[]) => {
    if (!activeSessionId || rows.length === 0) return;
    const pathByEntry = new Map(rows.map((row) => [row.entry, row.path]));
    const entries = rows.map((row) => row.entry);
    setMoveDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      sourceDirectory: rows[0]?.parentPath ?? currentPath,
      initialTargetDirectory: currentPath,
      items: buildMoveItems(
        entries,
        currentPath,
        (entry) => pathByEntry.get(entry) ?? currentPath,
      ),
    });
  };

  const handleTreeDownload = (rows: FileExplorerTreeRow[]) => {
    if (rows.length === 0) return;
    const pathByEntry = new Map(rows.map((row) => [row.entry, row.path]));
    void downloadEntries(
      rows.map((row) => row.entry),
      (entry) => pathByEntry.get(entry) ?? getEntryFullPath(entry),
    );
  };

  const handleTreeAddToFavorites = useCallback(
    (row: FileExplorerTreeRow) => {
      if (!row.isRoot && row.entry.is_dir) {
        addFavoriteDirectory(row.path);
      }
    },
    [addFavoriteDirectory],
  );

  const handleTreeCopyPath = (
    row: FileExplorerTreeRow,
    mode: "dir" | "name" | "full",
  ) => {
    handleCopyPath(
      row.entry,
      mode,
      row.path,
      row.isRoot ? row.path : row.parentPath,
    );
  };

  const handleTreeSendToTerminal = (
    row: FileExplorerTreeRow,
    mode: "dir" | "name" | "full",
  ) => {
    handleSendToTerminal(
      row.entry,
      mode,
      row.path,
      row.isRoot ? row.path : row.parentPath,
    );
  };

  const handleTreeProperties = useCallback((row: FileExplorerTreeRow) => {
    if (!activeSessionId) return;
    setPropertiesDialogData({
      sessionId: activeSessionId,
      backend: explorerBackend,
      fullPath: row.path,
      rawPathToken: row.rawPathToken,
      name: row.entry.name,
      is_dir: row.entry.is_dir,
    });
  }, [activeSessionId, explorerBackend]);

  const handleTreeAIAction = (
    row: FileExplorerTreeRow,
    action: AICustomActionConfig,
  ) => {
    void handleFileAIAction(row.entry, action, row.path);
  };

  const handleTreeSendToPeer = useCallback(
    (rows: FileExplorerTreeRow[]) => {
      if (!activeSessionId || rows.length === 0) return;
      if (!peerEndpoint) {
        onOpenPeerSelector?.();
        return;
      }
      onSendEntries?.(
        {
          sessionId: activeSessionId,
          kind: explorerBackend,
          currentPath: rows[0]?.parentPath ?? currentPath,
        },
        rows.map((row) => ({
          name: row.entry.name,
          path: row.path,
          isDirectory: row.entry.is_dir,
        })),
      );
    },
    [
      activeSessionId,
      currentPath,
      explorerBackend,
      onOpenPeerSelector,
      onSendEntries,
      peerEndpoint,
    ],
  );

  const handleTreeSendToTarget = useCallback(
    (rows: FileExplorerTreeRow[], targetSessionId: string) => {
      if (!activeSessionId || rows.length === 0) return;
      onSendEntriesToTarget?.(
        {
          sessionId: activeSessionId,
          kind: explorerBackend,
          currentPath: rows[0]?.parentPath ?? currentPath,
        },
        rows.map((row) => ({
          name: row.entry.name,
          path: row.path,
          isDirectory: row.entry.is_dir,
        })),
        targetSessionId,
      );
    },
    [
      activeSessionId,
      currentPath,
      explorerBackend,
      onSendEntriesToTarget,
    ],
  );

  const handleDialogRefresh = useCallback(async () => {
    const backend = explorerBackendRef.current;
    const paths: string[] = [];
    if (deleteDialogData) {
      paths.push(
        ...deleteDialogData.items.map((item) =>
          getExplorerParentDirectory(item.path, backend),
        ),
      );
    } else if (moveDialogData) {
      paths.push(moveDialogData.sourceDirectory);
    } else if (newItemDialogData) {
      paths.push(newItemDialogData.currentDirPath);
    } else if (newSymlinkDialogData) {
      paths.push(newSymlinkDialogData.currentDirPath);
    } else if (propertiesDialogData) {
      paths.push(
        getExplorerParentDirectory(propertiesDialogData.fullPath, backend),
      );
    } else {
      paths.push(currentPathRef.current || homeDirRef.current);
    }
    return refreshExplorerDirectories(paths);
  }, [
    deleteDialogData,
    moveDialogData,
    newItemDialogData,
    newSymlinkDialogData,
    propertiesDialogData,
    refreshExplorerDirectories,
  ]);

  const handleDialogDeleteSuccess = useCallback(() => {
    const backend = explorerBackendRef.current;
    const deletedItems = deleteDialogData?.items ?? [];
    const current = currentPathRef.current;
    const deletedAncestor = deletedItems.find((item) =>
      pathStartsWithDirectory(current, item.path, backend),
    );
    setSelectedFiles(new Set());
    lastSelectedRef.current = null;
    clearTreeSelection();

    if (deletedAncestor) {
      const parentPath = getExplorerParentDirectory(
        deletedAncestor.path,
        backend,
      );
      if (parentPath && parentPath !== deletedAncestor.path) {
        void refreshExplorerDirectories([parentPath]);
        void loadDirectory(parentPath, {
          selectEntryName: getLocalPathName(
            deletedAncestor.path,
            deletedAncestor.name,
          ),
        });
        return;
      }
    }

    const paths = deletedItems.map((item) =>
      getExplorerParentDirectory(item.path, backend),
    );
    void refreshExplorerDirectories(
      paths.length > 0 ? paths : [currentPathRef.current],
    );
  }, [
    clearTreeSelection,
    deleteDialogData,
    loadDirectory,
    refreshExplorerDirectories,
  ]);

  const handleDialogOpenEntry = (entry: FileEntry) => {
    if (!isTreeView || !newItemDialogData) {
      handleItemClick(entry);
      return;
    }
    const fullPath = joinExplorerPath(
      newItemDialogData.currentDirPath,
      entry.name,
      explorerBackendRef.current,
    );
    if (entry.is_dir) {
      void loadDirectory(fullPath, { rawPathToken: entry.raw_path_token });
    } else {
      void handleOpenDefault(entry, fullPath);
    }
  };

  const displayPath = currentPath || homeDir || "~";

  const displayEntries = useMemo(() => {
    const normalizedPath = normalizeExplorerPath(currentPath, explorerBackend);
    if (
      !normalizedPath ||
      getExplorerParentDirectory(normalizedPath, explorerBackend) ===
        normalizedPath
    ) {
      return filteredSortedFiles;
    }

    return [PARENT_DIRECTORY_ENTRY, ...filteredSortedFiles];
  }, [currentPath, explorerBackend, filteredSortedFiles]);
  const hasNoSearchMatches =
    isFileSearchActive && filteredSortedFiles.length === 0;

  const visibleEntries = useMemo(() => {
    if (displayEntries.length === 0) {
      return displayEntries;
    }

    const entriesScrollTop = Math.max(
      0,
      listScrollTop - FILE_LIST_HEADER_HEIGHT,
    );
    const viewportHeight =
      listViewportHeight > 0 ? listViewportHeight : FILE_LIST_ITEM_HEIGHT * 12;
    const visibleCount = Math.max(
      1,
      Math.ceil(viewportHeight / FILE_LIST_ITEM_HEIGHT),
    );
    const startIndex = Math.max(
      0,
      Math.floor(entriesScrollTop / FILE_LIST_ITEM_HEIGHT) - FILE_LIST_OVERSCAN,
    );
    const endIndex = Math.min(
      displayEntries.length,
      startIndex + visibleCount + FILE_LIST_OVERSCAN * 2,
    );

    return displayEntries.slice(startIndex, endIndex);
  }, [displayEntries, listScrollTop, listViewportHeight]);

  const virtualListPadding = useMemo(() => {
    if (visibleEntries.length === 0) {
      return { top: 0, bottom: 0 };
    }

    const startIndex = displayEntries.indexOf(visibleEntries[0]);
    const top = startIndex * FILE_LIST_ITEM_HEIGHT;
    const bottom = Math.max(
      0,
      (displayEntries.length - startIndex - visibleEntries.length) *
        FILE_LIST_ITEM_HEIGHT,
    );

    return { top, bottom };
  }, [displayEntries, visibleEntries]);

  useEffect(() => {
    if (isTreeView) return;
    const entryName = pendingRevealNameRef.current;
    const container = listContainerRef.current;
    if (!entryName || !container) {
      return;
    }

    const entryIndex = displayEntries.findIndex(
      (entry) => entry.name === entryName,
    );
    if (entryIndex < 0) {
      return;
    }

    pendingRevealNameRef.current = null;
    const nextScrollTop = Math.max(
      0,
      FILE_LIST_HEADER_HEIGHT +
        entryIndex * FILE_LIST_ITEM_HEIGHT -
        FILE_LIST_ITEM_HEIGHT,
    );
    const frame = window.requestAnimationFrame(() => {
      container.scrollTop = nextScrollTop;
      setListScrollTop(container.scrollTop);
    });

    return () => window.cancelAnimationFrame(frame);
  }, [displayEntries, isTreeView]);

  return (
    <aside
      className="nyaterm-wallpaper-transparent-surface h-full flex flex-col overflow-hidden"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
      onMouseDownCapture={handlePanelMouseDownCapture}
      onMouseUpCapture={handlePanelMouseUpCapture}
    >
      <PanelHeader
        title={t("panel.fileExplorer")}
        meta={headerMeta}
        actions={headerActions}
      />

      {canBrowseFiles && (
        <FileExplorerToolbar
          isTreeView={isTreeView}
          selectedCount={isTreeView ? selectedTreeRows.length : selectedRealFiles.length}
          isFileSearchActive={isFileSearchActive}
          isFileSearchExpanded={isFileSearchExpanded}
          showHiddenFiles={showHiddenFiles}
          showTransferActions={canUseRemoteTransfer}
          fileSearchQuery={fileSearchQuery}
          fileSearchInputRef={fileSearchInputRef}
          onNewFile={() =>
            handleNewFile(isTreeView ? getTreeOperationDirectoryPath() : undefined)
          }
          onNewFolder={() =>
            handleNewFolder(isTreeView ? getTreeOperationDirectoryPath() : undefined)
          }
          onUploadFiles={() =>
            handleUploadFiles(isTreeView ? getTreeOperationDirectoryPath() : undefined)
          }
          onUploadFolder={() =>
            handleUploadFolder(isTreeView ? getTreeOperationDirectoryPath() : undefined)
          }
          onDownloadSelected={() =>
            isTreeView
              ? handleTreeDownload(selectedTreeRows)
              : void handleDownloadSelected()
          }
          onDeleteSelected={() =>
            isTreeView ? handleTreeDelete(selectedTreeRows) : handleDeleteSelected()
          }
          onGoUp={handleGoUp}
          onRefresh={() =>
            isTreeView ? handleTreeRefresh(null) : void refreshCurrentDirectory()
          }
          onLocatePath={handleLocatePath}
          locateLabel={
            activeFilePath
              ? t("fileExplorer.locateActiveFile")
              : t("fileExplorer.locateTerminalPath")
          }
          canLocatePath={!!activeFilePath || cwdTrackingActive}
          onToggleViewMode={handleToggleViewMode}
          onToggleHiddenFiles={handleToggleHiddenFiles}
          onExpandSearch={() => setIsFileSearchExpanded(true)}
          onSearchQueryChange={setFileSearchQuery}
          onCollapseSearch={() => setIsFileSearchExpanded(false)}
        />
      )}

      {canBrowseFiles && !isTreeView && (
        <FileExplorerPathBar
          isEditingPath={isEditingPath}
          pathInputText={pathInputText}
          pathInputRef={pathInputRef}
          backend={explorerBackend}
          displayPath={displayPath}
          currentPath={currentPath}
          homeDir={homeDir}
          sessionId={activeSessionId ?? ""}
          currentDirectoryEntries={files}
          showHiddenFiles={showHiddenFiles}
          directoryHistory={visitedHistory}
          favoriteDirectories={favoriteDirectories}
          onPathInputTextChange={setPathInputText}
          onEditingPathChange={setIsEditingPath}
          onLoadDirectory={(path) => void loadDirectory(path)}
          onNavigate={handleNavigateDirectory}
          onListChildDirectories={listChildDirectories}
          onSelectHistoryPath={handleSelectHistoryPath}
          onAddCurrentDirectoryToFavorites={
            handleAddCurrentDirectoryToFavorites
          }
          onSelectFavoritePath={handleSelectFavoritePath}
          onRemoveFavoritePath={handleRemoveFavoritePath}
        />
      )}

      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div className="relative min-h-0 flex-1">
            {isExternalDropActive && canBrowseFiles && (
              <ExternalFileDropOverlay
                insetClassName="inset-3"
                title={t("fileExplorer.externalDropOverlayTitle")}
                hint={t("fileExplorer.externalDropOverlayHint")}
              />
            )}
            <div
              ref={listContainerRef}
              className="h-full overflow-auto text-sm terminal-scroll outline-none"
              tabIndex={canBrowseFiles ? 0 : -1}
              onMouseDown={(event) => {
                if (!canBrowseFiles) return;
                if (
                  isTreeView &&
                  event.target instanceof Element &&
                  event.target.closest("[data-file-tree-path]")
                ) {
                  return;
                }
                listContainerRef.current?.focus();
              }}
              onKeyDown={handleListKeyDown}
            >
              {!activeSessionId ? (
                <div
                  className="text-center py-8 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  <MdFolderOff className="text-xl block mx-auto mb-2" />
                  <div className="text-sm block mb-2">
                    {t("fileExplorer.connectToSession")}
                  </div>
                </div>
              ) : hasUnsupportedSession ? (
                <div
                  className="text-center py-8 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  <MdFolderOff className="text-xl block mx-auto mb-2" />
                  <div className="text-sm block mb-2">
                    {t("fileExplorer.unsupportedSession")}
                  </div>
                  <div>{t("fileExplorer.unsupportedSessionDesc")}</div>
                </div>
              ) : isResolvingRemoteFileBrowser ? (
                <div
                  className="text-center py-8 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  {t("fileExplorer.loading")}
                </div>
              ) : hasRemoteFileBrowserDisabled ? (
                <div
                  className="text-center py-8 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  <MdFolderOff className="text-xl block mx-auto mb-2" />
                  <div className="text-sm block mb-2">
                    {t("fileExplorer.remoteBrowserDisabled")}
                  </div>
                  <div>{t("fileExplorer.remoteBrowserDisabledDesc")}</div>
                </div>
              ) : isTreeView ? (
                <FileExplorerTree
                  key={`${activeSessionId ?? ""}:${explorerBackend}:${treeRootPath}`}
                  rows={treeState.rows}
                  selectedPaths={treeState.selectedPaths}
                  backend={explorerBackend}
                  scrollContainerRef={listContainerRef}
                  revealRequest={treeRevealRequest}
                  onRowClick={treeState.handleRowClick}
                  onRowKeyDown={treeState.handleRowKeyDown}
                  onSelectAll={treeState.selectAll}
                  onDeleteSelected={() => handleTreeDelete(selectedTreeRows)}
                  onToggleDirectory={treeState.toggleDirectory}
                  onActivateDirectory={handleTreeOpenDirectory}
                  onOpenFile={(row) => handleTreeOpenDefault(row)}
                  onRequestRename={(row) => {
                    if (!row.isRoot) {
                      beginInlineRename(row.entry, row.path, row.parentPath);
                    }
                  }}
                  onRetry={(row) => void treeState.refreshDirectory(row.path)}
                  inlineRename={
                    inlineRenameState
                      ? {
                          path: inlineRenameState.oldPath,
                          value: inlineRenameState.value,
                          isSubmitting: inlineRenameState.isSubmitting,
                        }
                      : null
                  }
                  onInlineRenameChange={(value) =>
                    setInlineRenameState((previous) =>
                      previous ? { ...previous, value } : previous,
                    )
                  }
                  onInlineRenameSubmit={() => void handleInlineRenameSubmit()}
                  onInlineRenameCancel={cancelInlineRename}
                  onContextMenuRow={setTreeContextRow}
                  onContextMenuSelect={treeState.selectContextRow}
                  labels={{
                    collapse: t("fileExplorer.collapseDirectory"),
                    expand: t("fileExplorer.expandDirectory"),
                    loading: t("fileExplorer.loading"),
                    retry: t("common.retry"),
                    emptyDirectory: t("fileExplorer.emptyDirectory"),
                    tree: t("fileExplorer.treeAriaLabel"),
                  }}
                />
              ) : (
                <>
                  <div
                    className="nyaterm-wallpaper-transparent-surface sticky top-0 z-[1] h-7 border-b"
                    style={{
                      backgroundColor: "var(--df-bg-section-header)",
                      borderColor: "var(--df-border)",
                      minWidth: fileListTableWidth,
                    }}
                  >
                    <div
                      className="grid h-full"
                      style={{
                        gridTemplateColumns: fileListGridTemplate,
                        width: fileListTableWidth,
                      }}
                    >
                      {FILE_LIST_COLUMNS.map((column, index) => {
                        const label = t(column.labelKey);
                        const isActiveSort = fileSortMode.column === column.id;
                        const SortDirectionIcon =
                          fileSortMode.direction === "asc"
                            ? MdArrowDropUp
                            : MdArrowDropDown;

                        return (
                          <div
                            key={column.id}
                            className={cn(
                              "relative min-w-0 border-r",
                              index === 0 && "border-l",
                            )}
                            style={{
                              borderColor: "var(--df-border)",
                              backgroundColor: isActiveSort
                                ? "color-mix(in srgb, var(--df-primary) 8%, var(--df-bg-section-header))"
                                : undefined,
                            }}
                          >
                            <button
                              type="button"
                              aria-label={t("fileExplorer.sortByColumn", {
                                column: label,
                              })}
                              className={cn(
                                "flex h-full w-full min-w-0 items-center gap-1 px-2 text-[0.625rem] font-medium transition-colors hover:text-foreground",
                                column.align === "right" &&
                                  "justify-end text-right",
                                isActiveSort
                                  ? "text-primary"
                                  : "text-muted-foreground",
                              )}
                              onClick={() => handleSortColumn(column.id)}
                            >
                              <span className="truncate">{label}</span>
                              {isActiveSort && (
                                <SortDirectionIcon className="h-3.5 w-3.5" />
                              )}
                            </button>
                            <span
                              title={t("fileExplorer.resizeColumn", {
                                column: label,
                              })}
                              className="absolute right-0 top-1/2 h-4 w-1.5 -translate-y-1/2 cursor-col-resize rounded-sm transition-colors hover:bg-primary/50"
                              onMouseDown={(event) =>
                                handleColumnResizeMouseDown(column.id, event)
                              }
                            />
                          </div>
                        );
                      })}
                    </div>
                  </div>

                  {directoryLoading ? (
                    <div
                      className="px-2 py-4 text-center text-xs"
                      style={{ color: "var(--df-text-dimmed)" }}
                    >
                      {t("fileExplorer.loading")}
                    </div>
                  ) : error ? (
                    <div className="px-2 py-4 text-center text-xs text-red-400">
                      {error}
                    </div>
                  ) : hasNoSearchMatches ? (
                    <div
                      className="px-2 py-4 text-center text-xs"
                      style={{ color: "var(--df-text-dimmed)" }}
                    >
                      {t("fileExplorer.noSearchResults")}
                    </div>
                  ) : displayEntries.length === 0 ? (
                    <div
                      className="px-2 py-4 text-center text-xs"
                      style={{ color: "var(--df-text-dimmed)" }}
                    >
                      {t("fileExplorer.emptyDirectory")}
                    </div>
                  ) : (
                    <ul
                      style={{
                        paddingTop: virtualListPadding.top,
                        paddingBottom: virtualListPadding.bottom + 8,
                        minWidth: fileListTableWidth,
                        width: fileListTableWidth,
                      }}
                    >
                      {visibleEntries.map((entry) => (
                        <FileListItem
                          key={entry.name}
                          entry={entry}
                          entryPath={getEntryFullPath(entry)}
                          entryParentPath={currentPath}
                          isSelected={selectedFiles.has(entry.name)}
                          selectedCount={selectedRealFiles.length}
                          isParentDirectoryEntry={isParentDirectoryEntry(entry)}
                          activeSessionId={activeSessionId}
                          editorType={
                            appSettings.transfer.editor_type || "external"
                          }
                          columnTemplate={fileListGridTemplate}
                          rowWidth={fileListTableWidth}
                          onSelectionStart={handleSelectionStart}
                          onSelectionDrag={handleSelectionDrag}
                          onContextMenuSelect={handleContextMenuSelection}
                          onItemClick={handleItemClick}
                          onOpenDefault={handleOpenDefault}
                          onPreview={(entry) => void handlePreview(entry)}
                          onOpenInternal={handleOpenInternal}
                          onOpenExternal={handleOpenExternal}
                          onRefresh={() => void refreshCurrentDirectory()}
                          showTransferActions={canUseRemoteTransfer}
                          onUpload={handleUploadFiles}
                          onUploadFolder={handleUploadFolder}
                          onDownload={handleDownloadFromContextMenu}
                          showPeerSendAction={!!peerEndpoint && !!onSendEntries}
                          onSendToPeer={handleSendToPeer}
                          sendTargetOptions={sendTargetOptions}
                          onSendToTarget={handleSendToTarget}
                          onRename={beginInlineRename}
                          onMove={handleMoveFromContextMenu}
                          onDelete={handleDeleteFromContextMenu}
                          onAddToFavorites={handleAddEntryToFavorites}
                          onCopyPath={handleCopyPath}
                          onSendToTerminal={
                            terminalInputEnabled ? handleSendToTerminal : undefined
                          }
                          onProperties={(entry) => {
                            if (activeSessionId) {
                              setPropertiesDialogData({
                                sessionId: activeSessionId,
                                backend: explorerBackend,
                                fullPath: getEntryFullPath(entry),
                                rawPathToken: entry.raw_path_token,
                                name: entry.name,
                                is_dir: entry.is_dir,
                              });
                            }
                          }}
                          aiActions={getEntryAiActions(entry)}
                          onAIAction={(entry, action) =>
                            void handleFileAIAction(entry, action)
                          }
                          inlineRename={
                            inlineRenameState?.entryName === entry.name
                              ? {
                                  value: inlineRenameState.value,
                                  isSubmitting: inlineRenameState.isSubmitting,
                                }
                              : null
                          }
                          onInlineRenameChange={(value) =>
                            setInlineRenameState((prev) =>
                              prev?.entryName === entry.name
                                ? { ...prev, value }
                                : prev,
                            )
                          }
                          onInlineRenameSubmit={() =>
                            void handleInlineRenameSubmit()
                          }
                          onInlineRenameCancel={cancelInlineRename}
                        />
                      ))}
                    </ul>
                  )}
                </>
              )}
            </div>
          </div>
        </ContextMenuTrigger>
        {canBrowseFiles && isTreeView && treeContextRow ? (
          <FileExplorerEntryContextMenu
            target={treeContextRow}
            selectedTargets={treeActionRows(treeContextRow)}
            activeSessionId={activeSessionId}
            editorType={appSettings.transfer.editor_type || "external"}
            showTransferActions={canUseRemoteTransfer}
            terminalInputEnabled={terminalInputEnabled}
            sendTargetOptions={sendTargetOptions}
            getAiActions={(row) => getEntryAiActions(row.entry)}
            onOpenDirectory={handleTreeOpenDirectory}
            onOpenDefault={handleTreeOpenDefault}
            onPreview={handleTreePreview}
            onOpenInternal={handleTreeOpenInternal}
            onOpenExternal={handleTreeOpenExternal}
            onRefresh={handleTreeRefresh}
            onNewFile={handleNewFile}
            onNewFolder={handleNewFolder}
            onNewSymlink={
              explorerBackend === "remote" ? handleNewSymlink : undefined
            }
            onUpload={(path) => void handleUploadFiles(path)}
            onUploadFolder={(path) => void handleUploadFolder(path)}
            onDownload={handleTreeDownload}
            onSendToPeer={handleTreeSendToPeer}
            onSendToTarget={handleTreeSendToTarget}
            onRename={(row) => beginInlineRename(row.entry, row.path, row.parentPath)}
            onMove={handleTreeMove}
            onDelete={handleTreeDelete}
            onAddToFavorites={handleTreeAddToFavorites}
            onCopyPath={handleTreeCopyPath}
            onSendToTerminal={handleTreeSendToTerminal}
            onProperties={handleTreeProperties}
            onAIAction={handleTreeAIAction}
          />
        ) : canBrowseFiles ? (
          <ContextMenuContent className="w-52">
            <ContextMenuItem
              onClick={() =>
                isTreeView
                  ? handleTreeRefresh(null)
                  : void refreshCurrentDirectory()
              }
            >
              <MdRefresh className="mr-2 h-4 w-4" />
              {t("fileExplorer.refresh")}
            </ContextMenuItem>
            {canUseRemoteTransfer && (
              <>
                <ContextMenuSub>
                  <ContextMenuSubTrigger>
                    <MdUpload className="mr-2 h-4 w-4" />
                    {t("fileExplorer.cmUpload")}
                  </ContextMenuSubTrigger>
                  <ContextMenuSubContent className="w-48">
                    <ContextMenuItem onClick={() => void handleUploadFiles()}>
                      <MdUpload className="mr-2 h-4 w-4" />
                      {t("fileExplorer.upload")}
                    </ContextMenuItem>
                    <ContextMenuItem onClick={() => void handleUploadFolder()}>
                      <MdDriveFolderUpload className="mr-2 h-4 w-4" />
                      {t("fileExplorer.uploadFolder")}
                    </ContextMenuItem>
                  </ContextMenuSubContent>
                </ContextMenuSub>
                <ContextMenuSeparator />
              </>
            )}
            <ContextMenuItem onClick={() => handleNewFile()}>
              <MdNoteAdd className="mr-2 h-4 w-4" />
              {t("fileExplorer.newFile")}
            </ContextMenuItem>
            <ContextMenuItem onClick={() => handleNewFolder()}>
              <MdCreateNewFolder className="mr-2 h-4 w-4" />
              {t("fileExplorer.newFolder")}
            </ContextMenuItem>
            {explorerBackend === "remote" && (
              <ContextMenuItem onClick={() => handleNewSymlink()}>
                <MdLink className="mr-2 h-4 w-4" />
                {t("fileExplorer.newSymlink")}
              </ContextMenuItem>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem onClick={handleCopyCurrentPath}>
              <MdContentCopy className="mr-2 h-4 w-4" />
              {t("fileExplorer.copyDirPath")}
            </ContextMenuItem>
            {terminalInputEnabled ? (
              <ContextMenuItem onClick={handleSendCurrentPathToTerminal}>
                <LuClipboardPaste className="mr-2 h-4 w-4" />
                {t("fileExplorer.sendDirPathToTerminal")}
              </ContextMenuItem>
            ) : null}
            <ContextMenuSeparator />
            <ContextMenuItem onClick={() => handleCurrentDirProperties()}>
              <MdInfo className="mr-2 h-4 w-4" />
              {t("fileExplorer.properties")}
            </ContextMenuItem>
          </ContextMenuContent>
        ) : null}
      </ContextMenu>

      {canBrowseFiles && (
        <div
          className="nyaterm-wallpaper-control-surface px-2 py-1.5 text-[0.6875rem] border-t flex items-center justify-between shrink-0"
          style={{
            color: "var(--df-text-dimmed)",
            borderColor: "var(--df-border)",
            backgroundColor: "var(--df-bg-panel)",
          }}
        >
          <div className="flex gap-4">
            {!directoryLoading && !error && activeFooterStats.totalItemCount > 0 && (
              <>
                <span>
                  {activeFooterStats.selectedItemCount > 0
                    ? t("fileExplorer.selectedItems", {
                        selected: activeFooterStats.selectedItemCount,
                        total: activeFooterStats.totalItemCount,
                      })
                    : t("fileExplorer.totalItems", {
                        count: activeFooterStats.totalItemCount,
                      })}
                </span>
                <span>{footerSizeText}</span>
              </>
            )}
          </div>
          {!isTreeView && (
            <div className="flex items-center gap-0.5">
              {terminalInputEnabled ? (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="inline-flex">
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground disabled:opacity-40 disabled:cursor-not-allowed"
                        onClick={handleSyncCwd}
                        disabled={!cwdTrackingActive}
                      >
                        <LuFolderSync className="h-[0.875rem] w-[0.875rem]" />
                      </Button>
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="top">
                    {cwdTrackingActive
                      ? t("fileExplorer.syncTerminalPath")
                      : t("fileExplorer.cwdTrackingUnavailable")}
                  </TooltipContent>
                </Tooltip>
              ) : null}
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="inline-flex">
                    <Button
                      variant="ghost"
                      size="icon"
                      className={`h-6 w-6 rounded-md disabled:opacity-40 disabled:cursor-not-allowed ${
                        cwdTrackingActive
                          ? autoSyncCwd
                            ? "text-primary"
                            : "text-muted-foreground hover:text-foreground"
                          : "text-muted-foreground"
                      }`}
                      onClick={handleToggleAutoSyncCwd}
                      disabled={!cwdTrackingActive || !autoSyncScopeId}
                    >
                      <MdSyncLock className="h-[0.875rem] w-[0.875rem]" />
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">
                  {cwdTrackingActive
                    ? t("fileExplorer.autoSyncTerminalPath")
                    : t("fileExplorer.cwdTrackingUnavailable")}
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="inline-flex">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground"
                      onClick={() => {
                        sendTextToTerminal(currentPath);
                      }}
                    >
                      <LuClipboardPaste className="h-[0.875rem] w-[0.875rem]" />
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">
                  {t("fileExplorer.sendToTerminal")}
                </TooltipContent>
              </Tooltip>
            </div>
          )}
        </div>
      )}

      <FileExplorerDialogs
        deleteDialogData={deleteDialogData}
        moveDialogData={moveDialogData}
        newItemDialogData={newItemDialogData}
        newSymlinkDialogData={newSymlinkDialogData}
        propertiesDialogData={propertiesDialogData}
        onDeleteClose={() => setDeleteDialogData(null)}
        onMoveClose={() => setMoveDialogData(null)}
        onNewItemClose={() => setNewItemDialogData(null)}
        onNewSymlinkClose={() => setNewSymlinkDialogData(null)}
        onPropertiesClose={() => setPropertiesDialogData(null)}
        onDeleteSuccess={handleDialogDeleteSuccess}
        onMoveSuccess={handleMoveSuccess}
        onRefresh={handleDialogRefresh}
        onOpenDirectoryEntry={handleDialogOpenEntry}
        onOpenDefault={(entry) => {
          if (isTreeView && newItemDialogData) {
            const fullPath = joinExplorerPath(
              newItemDialogData.currentDirPath,
              entry.name,
              explorerBackendRef.current,
            );
            void handleOpenDefault(entry, fullPath);
          } else {
            void handleOpenDefault(entry);
          }
        }}
      />
    </aside>
  );
}

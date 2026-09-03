import { emit } from "@tauri-apps/api/event";
import {
  type DragEvent,
  type MouseEvent,
  memo,
  type PointerEvent,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type WheelEvent,
} from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdCellTower,
  MdCheck,
  MdClose,
  MdContentCopy,
  MdDns,
  MdErrorOutline,
  MdExpandMore,
  MdFolder,
  MdHistory,
  MdLock,
  MdTerminal,
} from "react-icons/md";
import { toast } from "sonner";
import CloseAllSessionsDialog from "@/components/dialog/terminal/CloseAllSessionsDialog";
import TabRenameDialog from "@/components/dialog/terminal/TabRenameDialog";
import TabStartupCommandDialog from "@/components/dialog/terminal/TabStartupCommandDialog";
import { hasMatchingTemporaryConfig } from "@/lib/appWorkspace";
import { useFileDocumentStates } from "@/lib/fileDocumentRegistry";
import type { TabMouseAction } from "@/lib/interactionSettings";
import { normalizeTabMouseAction } from "@/lib/interactionSettings";
import {
  getActiveGroupForSession,
  isSessionPausedInGroup,
} from "@/lib/syncInputGroups";
import {
  collectSessionPanes,
  getActivePane,
  getActiveSessionTabDisplayName,
  getTabActiveSessionId,
  getTabDisplayName,
} from "@/lib/workspaceTabs";
import type {
  Group,
  PaneSplitDirection,
  SavedConnection,
  Tab,
} from "@/types/global";
import { useApp } from "../../context/AppContext";
import {
  getDynamicTitle,
  getDynamicTitleSnapshot,
  useDynamicTitles,
} from "@/lib/dynamicTabTitles";
import { resolveConnectionIcon } from "../icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import TabContextMenu from "./TabContextMenu";

interface TabBarProps {
  tabs: Tab[];
  activeTabId: string | null;
  focusedTabId?: string | null;
  unreadTabIds?: Set<string>;
  disconnectedTabIds?: Set<string>;
  onTabChange: (tabId: string) => void;
  onTabClose: (tab: Tab) => void | Promise<void>;
  onAddTab: () => void;
  onConnectConnection: (connection: SavedConnection) => void | Promise<void>;
  onDuplicateSession: (tab: Tab) => void | Promise<void>;
  onMultiplexSshSession: (tab: Tab) => void | Promise<void>;
  onDuplicateSessionWithCommand: (
    tab: Tab,
    command: string,
    delayMs: number,
  ) => void | Promise<void>;
  onMultiplexSshSessionWithCommand: (
    tab: Tab,
    command: string,
    delayMs: number,
  ) => void | Promise<void>;
  onReconnectSession: (tab: Tab) => void | Promise<void>;
  onDisconnectSession: (tab: Tab) => void | Promise<void>;
  onSplitSession: (
    tab: Tab,
    direction: PaneSplitDirection,
  ) => void | Promise<void>;
  onUnsplit?: () => void;
  onCloseSession: (tab: Tab) => void | Promise<void>;
  onCloseAll: () => void | Promise<void>;
  onCloseInactive: (keepTabId: string) => void | Promise<void>;
  onCloseRight: (tabId: string) => void | Promise<void>;
  onSessionInfo: (tab: Tab) => void | Promise<void>;
  onReorderTabs: (fromTabId: string, toIndex: number) => void;
  onMoveTabHere?: (fromTabId: string, toIndex: number) => void;
}

interface ConnectionGroupNode {
  group: Group;
  children: ConnectionGroupNode[];
  connections: SavedConnection[];
  totalCount: number;
}

const TAB_STRIP_SCROLL_DURATION_MS = 180;
const TAB_STRIP_SCROLL_PADDING = 12;
const POINTER_TAB_DRAG_THRESHOLD_PX = 4;

interface PointerTabDragState {
  tabId: string;
  pointerId: number;
  startX: number;
  startY: number;
  dragging: boolean;
}

function shouldUsePointerTabDrag() {
  if (typeof navigator === "undefined") return false;
  return (
    /Mac/.test(navigator.platform) && /AppleWebKit/.test(navigator.userAgent)
  );
}

function easeOutCubic(progress: number): number {
  return 1 - (1 - progress) ** 3;
}

function clampTabStripScrollLeft(strip: HTMLElement, value: number): number {
  const maxScrollLeft = Math.max(0, strip.scrollWidth - strip.clientWidth);
  return Math.max(0, Math.min(maxScrollLeft, value));
}

function animateTabStripScroll(
  strip: HTMLElement,
  targetScrollLeft: number,
  onComplete?: () => void,
): () => void {
  const startScrollLeft = strip.scrollLeft;
  const clampedTarget = clampTabStripScrollLeft(strip, targetScrollLeft);
  const distance = clampedTarget - startScrollLeft;

  if (Math.abs(distance) < 0.5) {
    strip.scrollLeft = clampedTarget;
    onComplete?.();
    return () => {};
  }

  const startTime = performance.now();
  let rafId = 0;

  const step = (now: number) => {
    const progress = Math.min(
      1,
      (now - startTime) / TAB_STRIP_SCROLL_DURATION_MS,
    );
    strip.scrollLeft = startScrollLeft + distance * easeOutCubic(progress);
    if (progress < 1) {
      rafId = requestAnimationFrame(step);
      return;
    }
    strip.scrollLeft = clampedTarget;
    onComplete?.();
  };

  rafId = requestAnimationFrame(step);
  return () => cancelAnimationFrame(rafId);
}

function compareSortOrder(
  left: { sort_order?: number },
  right: { sort_order?: number },
) {
  return (left.sort_order ?? 0) - (right.sort_order ?? 0);
}

function canSpawnSessionFromTab(tab: Tab): boolean {
  const pane = getActivePane(tab);
  return (
    !!pane &&
    pane.paneKind === "terminal" &&
    (pane.type === "Local" || !!pane.connectionId || hasMatchingTemporaryConfig(pane))
  );
}

function getTabConnection(tab: Tab, savedConnections: SavedConnection[]) {
  const pane = getActivePane(tab);
  return pane?.connectionId
    ? savedConnections.find((connection) => connection.id === pane.connectionId)
    : undefined;
}

function isSshTab(tab: Tab, savedConnections: SavedConnection[]): boolean {
  const pane = getActivePane(tab);
  if (pane?.type !== "SSH") return false;
  if (pane.temporaryConfig?.protocol === "ssh") return true;
  const connection = getTabConnection(tab, savedConnections);
  return connection?.type === "ssh";
}

function getTabServerIp(tab: Tab, savedConnections: SavedConnection[]): string | null {
  const pane = getActivePane(tab);
  if (pane?.paneKind !== "terminal" || pane.type !== "SSH") return null;
  if (pane.temporaryConfig?.protocol === "ssh") return pane.temporaryConfig.host;
  return getTabConnection(tab, savedConnections)?.host || null;
}

function canMultiplexTab(
  tab: Tab,
  savedConnections: SavedConnection[],
): boolean {
  const pane = getActivePane(tab);
  return (
    !!pane &&
    pane.paneKind === "terminal" &&
    isSshTab(tab, savedConnections) &&
    !pane.connecting &&
    !pane.connectError &&
    !!pane.sessionId
  );
}

function canReconnectTab(tab: Tab): boolean {
  const pane = getActivePane(tab);
  return (
    !!pane &&
    pane.paneKind === "terminal" &&
    !pane.connecting &&
    (pane.type === "Local" || !!pane.connectionId || hasMatchingTemporaryConfig(pane))
  );
}

function canDisconnectTab(tab: Tab): boolean {
  const pane = getActivePane(tab);
  return (
    !!pane &&
    pane.paneKind === "terminal" &&
    !pane.connecting &&
    !pane.connectError
  );
}

function supportsFileTabMouseAction(action: TabMouseAction) {
  return (
    action === "rename_tab" ||
    action === "copy_tab_name" ||
    action === "close_tab"
  );
}

function SyncIndicator({
  tab,
  syncGroups,
  broadcastToAll,
}: {
  tab: Tab;
  syncGroups: import("@/types/global").SyncGroup[];
  broadcastToAll: boolean;
}) {
  const pane = getActivePane(tab);
  const sessionId = pane?.sessionId;

  const activeGroup = useMemo(() => {
    if (
      pane?.paneKind !== "terminal" ||
      !sessionId ||
      pane.connecting ||
      pane.connectError
    ) {
      return null;
    }
    if (broadcastToAll) return null;
    return getActiveGroupForSession(sessionId, syncGroups);
  }, [sessionId, syncGroups, broadcastToAll, pane]);

  const isMember =
    pane?.paneKind === "terminal" && (broadcastToAll || !!activeGroup);
  const isPaused =
    activeGroup && sessionId
      ? isSessionPausedInGroup(activeGroup, sessionId)
      : false;

  if (!isMember) return null;

  return (
    <MdCellTower
      className="text-[11px] shrink-0"
      style={{
        color: activeGroup?.color ?? "var(--df-primary)",
        opacity: isPaused ? 0.4 : 1,
      }}
    />
  );
}

/** Tab strip for workspace tabs. Drag-reorder is runtime-only. */
function TabBar({
  tabs,
  activeTabId,
  focusedTabId,
  unreadTabIds,
  disconnectedTabIds,
  onTabChange,
  onTabClose,
  onAddTab,
  onConnectConnection,
  onDuplicateSession,
  onMultiplexSshSession,
  onDuplicateSessionWithCommand,
  onMultiplexSshSessionWithCommand,
  onReconnectSession,
  onDisconnectSession,
  onSplitSession,
  onUnsplit,
  onCloseSession,
  onCloseAll,
  onCloseInactive,
  onCloseRight,
  onSessionInfo,
  onReorderTabs,
  onMoveTabHere,
}: TabBarProps) {
  const { t } = useTranslation();
  useDynamicTitles();
  const {
    appSettings,
    savedConnections,
    savedGroups,
    syncGroups,
    broadcastToAll,
    updateTab,
  } = useApp();
  const fileDocumentStates = useFileDocumentStates();
  const [draggedTabId, setDraggedTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [renameTab, setRenameTab] = useState<Tab | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [closeAllDialogOpen, setCloseAllDialogOpen] = useState(false);
  const [closingAllSessions, setClosingAllSessions] = useState(false);
  const [commandDialog, setCommandDialog] = useState<{
    tab: Tab;
    action: "duplicate" | "multiplex";
  } | null>(null);
  const [commandValue, setCommandValue] = useState("");
  const [commandDelayMs, setCommandDelayMs] = useState(
    appSettings.interaction.duplicate_session_command_delay_ms,
  );
  const pendingOpenTabFocusRef = useRef<Tab | null>(null);
  const tabStripRef = useRef<HTMLDivElement | null>(null);
  const tabButtonRefs = useRef(new Map<string, HTMLDivElement>());
  const draggedTabIdRef = useRef<string | null>(null);
  const droppedTabRef = useRef(false);
  const completedDropRef = useRef(false);
  const pointerDragRef = useRef<PointerTabDragState | null>(null);
  const suppressTabClickRef = useRef(false);
  const tabStripAnimatingRef = useRef(false);
  const tabStripScrollAnimationRef = useRef<(() => void) | null>(null);
  const usePointerTabDrag = shouldUsePointerTabDrag();
  const [tabStripScroll, setTabStripScroll] = useState({
    hasOverflow: false,
  });

  const groupsById = useMemo(
    () => new Map(savedGroups.map((group) => [group.id, group])),
    [savedGroups],
  );

  const connectionTree = useMemo(() => {
    const sortedConnections = [...savedConnections].sort(compareSortOrder);
    const sortedGroups = [...savedGroups].sort(compareSortOrder);
    const nodesById = new Map<string, ConnectionGroupNode>();

    for (const group of sortedGroups) {
      nodesById.set(group.id, {
        group,
        children: [],
        connections: [],
        totalCount: 0,
      });
    }

    const ungrouped: SavedConnection[] = [];
    for (const connection of sortedConnections) {
      if (connection.group_id && nodesById.has(connection.group_id)) {
        nodesById.get(connection.group_id)?.connections.push(connection);
      } else {
        ungrouped.push(connection);
      }
    }

    const roots: ConnectionGroupNode[] = [];
    for (const group of sortedGroups) {
      const node = nodesById.get(group.id);
      if (!node) continue;
      if (group.parent_id && nodesById.has(group.parent_id)) {
        nodesById.get(group.parent_id)?.children.push(node);
      } else {
        roots.push(node);
      }
    }

    const computeTotal = (node: ConnectionGroupNode): number => {
      node.totalCount =
        node.connections.length +
        node.children.reduce((sum, child) => sum + computeTotal(child), 0);
      return node.totalCount;
    };
    roots.forEach(computeTotal);

    const pruneEmpty = (
      node: ConnectionGroupNode,
    ): ConnectionGroupNode | null => {
      const children = node.children
        .map(pruneEmpty)
        .filter((child): child is ConnectionGroupNode => !!child);
      if (node.connections.length === 0 && children.length === 0) return null;
      return { ...node, children };
    };

    return {
      roots: roots
        .map(pruneEmpty)
        .filter((node): node is ConnectionGroupNode => !!node),
      ungrouped,
    };
  }, [savedConnections, savedGroups]);

  const shellConnections = useMemo(
    () =>
      savedConnections
        .filter((connection) => connection.type === "local_terminal")
        .sort(compareSortOrder),
    [savedConnections],
  );

  const recentConnections = useMemo(() => {
    const byId = new Map(
      savedConnections.map((connection) => [connection.id, connection]),
    );
    return (appSettings.ui.recent_connection_ids ?? [])
      .map((connectionId) => byId.get(connectionId))
      .filter((connection): connection is SavedConnection => !!connection)
      .slice(0, 10);
  }, [appSettings.ui.recent_connection_ids, savedConnections]);

  const openTabsMenuItems = useMemo(
    () => tabs.map((tab, index) => ({ tab, index })).reverse(),
    [tabs],
  );

  const updateTabStripScrollState = useCallback(() => {
    const strip = tabStripRef.current;
    if (!strip) return;

    const maxScrollLeft = Math.max(0, strip.scrollWidth - strip.clientWidth);
    const nextState = {
      hasOverflow: maxScrollLeft > 1,
    };

    setTabStripScroll((current) =>
      current.hasOverflow === nextState.hasOverflow ? current : nextState,
    );
  }, []);

  const runTabStripScrollAnimation = useCallback(
    (targetScrollLeft: number) => {
      const strip = tabStripRef.current;
      if (!strip) return;

      tabStripScrollAnimationRef.current?.();
      tabStripAnimatingRef.current = true;
      tabStripScrollAnimationRef.current = animateTabStripScroll(
        strip,
        targetScrollLeft,
        () => {
        tabStripScrollAnimationRef.current = null;
        tabStripAnimatingRef.current = false;
        updateTabStripScrollState();
        },
      );
    },
    [updateTabStripScrollState],
  );

  const getTabScrollTarget = useCallback((tabId: string): number | null => {
    const strip = tabStripRef.current;
    const tabElement = tabButtonRefs.current.get(tabId);
    if (!strip || !tabElement) return null;

    const scrollLeft = strip.scrollLeft;
    const viewportRight = scrollLeft + strip.clientWidth;
    const padding = TAB_STRIP_SCROLL_PADDING;
    const tabLeft = tabElement.offsetLeft;
    const tabRight = tabLeft + tabElement.offsetWidth;

    if (
      tabLeft >= scrollLeft + padding &&
      tabRight <= viewportRight - padding
    ) {
      return null;
    }

    if (tabLeft < scrollLeft + padding) {
      return clampTabStripScrollLeft(strip, tabLeft - padding);
    }

    return clampTabStripScrollLeft(
      strip,
      tabRight - strip.clientWidth + padding,
    );
  }, []);

  const scrollTabIntoView = useCallback(
    (tabId: string | null, smooth: boolean) => {
      if (!tabId) return;
      const targetScrollLeft = getTabScrollTarget(tabId);
      if (targetScrollLeft === null) return;

      if (smooth) {
        runTabStripScrollAnimation(targetScrollLeft);
        return;
      }

      const strip = tabStripRef.current;
      if (!strip) return;
      strip.scrollLeft = targetScrollLeft;
      updateTabStripScrollState();
    },
    [getTabScrollTarget, runTabStripScrollAnimation, updateTabStripScrollState],
  );

  const handleTabStripScroll = useCallback(() => {
    if (tabStripAnimatingRef.current) return;
    updateTabStripScrollState();
  }, [updateTabStripScrollState]);

  useLayoutEffect(() => {
    updateTabStripScrollState();
  });

  useLayoutEffect(() => {
    const strip = tabStripRef.current;
    if (!strip) return;

    const observer = new ResizeObserver(() => {
      updateTabStripScrollState();
      scrollTabIntoView(activeTabId, false);
    });
    observer.observe(strip);

    return () => observer.disconnect();
  }, [activeTabId, scrollTabIntoView, updateTabStripScrollState]);

  useLayoutEffect(() => {
    scrollTabIntoView(activeTabId, true);
  }, [activeTabId, scrollTabIntoView]);

  useLayoutEffect(
    () => () => {
      tabStripScrollAnimationRef.current?.();
      tabStripScrollAnimationRef.current = null;
      tabStripAnimatingRef.current = false;
    },
    [],
  );

  const handleTabStripWheel = useCallback(
    (event: WheelEvent<HTMLDivElement>) => {
      const strip = tabStripRef.current;
      if (!strip || strip.scrollWidth <= strip.clientWidth + 1) return;

      event.preventDefault();
      event.stopPropagation();

      // Wheel: continuous proportional scroll (natural for trackpad / mouse wheel).
      tabStripScrollAnimationRef.current?.();
      tabStripScrollAnimationRef.current = null;
      tabStripAnimatingRef.current = false;

      const delta =
        Math.abs(event.deltaX) > Math.abs(event.deltaY)
          ? event.deltaX
          : event.deltaY;
      if (Math.abs(delta) < 0.5) return;

      strip.scrollLeft = clampTabStripScrollLeft(
        strip,
        strip.scrollLeft + delta,
      );
      updateTabStripScrollState();
    },
    [updateTabStripScrollState],
  );

  const handleRenameTab = useCallback((tab: Tab) => {
    setRenameValue(getActiveSessionTabDisplayName(tab, getDynamicTitle));
    setRenameTab(tab);
  }, []);

  const handleRenameSubmit = useCallback(async () => {
    if (!renameTab) return;

    const trimmed = renameValue.trim();
    if (!trimmed) {
      toast.error(t("tabCtx.renameEmpty"));
      return;
    }
    if (trimmed.length > 64) return;

    try {
      await updateTab(
        renameTab.id,
        { customName: trimmed },
        { immediatePersist: true },
      );
      setRenameTab(null);
    } catch {
      toast.error(t("tabCtx.renameFailed"));
    }
  }, [renameTab, renameValue, t, updateTab]);

  const handleCopyTabName = useCallback(
    async (tab: Tab) => {
      try {
        await navigator.clipboard.writeText(
          getActiveSessionTabDisplayName(tab, getDynamicTitle),
        );
        toast.success(t("tabCtx.nameCopied"));
      } catch {
        toast.error(t("tabCtx.copyFailed"));
      }
    },
    [t],
  );

  const handleCopyServerIp = useCallback(
    async (tab: Tab) => {
      const host = getTabServerIp(tab, savedConnections);
      if (!host) return;

      try {
        await navigator.clipboard.writeText(host);
        toast.success(t("tabCtx.ipCopied"));
      } catch {
        toast.error(t("tabCtx.copyFailed"));
      }
    },
    [savedConnections, t],
  );

  const openCommandDialog = useCallback(
    (tab: Tab, action: "duplicate" | "multiplex") => {
      setCommandDialog({ tab, action });
      setCommandValue("");
      setCommandDelayMs(
        appSettings.interaction.duplicate_session_command_delay_ms,
      );
    },
    [appSettings.interaction.duplicate_session_command_delay_ms],
  );

  const closeCommandDialog = useCallback(() => {
    setCommandDialog(null);
    setCommandValue("");
    setCommandDelayMs(
      appSettings.interaction.duplicate_session_command_delay_ms,
    );
  }, [appSettings.interaction.duplicate_session_command_delay_ms]);

  const handleCommandDialogSubmit = useCallback(() => {
    if (!commandDialog) return;
    const trimmedCommand = commandValue.trim();
    if (!trimmedCommand) {
      toast.error(t("tabCtx.commandRequired"));
      return;
    }
    const { action, tab } = commandDialog;
    const delayMs = Math.max(0, Math.min(60000, Math.round(commandDelayMs)));
    closeCommandDialog();

    if (action === "duplicate") {
      void onDuplicateSessionWithCommand(tab, trimmedCommand, delayMs);
    } else {
      void onMultiplexSshSessionWithCommand(tab, trimmedCommand, delayMs);
    }
  }, [
    closeCommandDialog,
    commandDelayMs,
    commandDialog,
    commandValue,
    onDuplicateSessionWithCommand,
    onMultiplexSshSessionWithCommand,
    t,
  ]);

  const handleConfirmCloseAll = useCallback(async () => {
    if (closingAllSessions) return;

    setClosingAllSessions(true);
    try {
      await onCloseAll();
      setCloseAllDialogOpen(false);
    } finally {
      setClosingAllSessions(false);
    }
  }, [closingAllSessions, onCloseAll]);

  useLayoutEffect(() => {
    const listener = (event: Event) => {
      const detail = (
        event as CustomEvent<{
          tabId?: string;
          action?: "duplicate" | "multiplex";
        }>
      ).detail;
      if (!detail?.tabId || !detail.action) return;
      const tab = tabs.find((item) => item.id === detail.tabId);
      if (!tab) return;
      if (
        detail.action === "multiplex" &&
        !canMultiplexTab(tab, savedConnections)
      )
        return;
      if (detail.action === "duplicate" && !canSpawnSessionFromTab(tab)) return;
      openCommandDialog(tab, detail.action);
    };

    window.addEventListener(
      "nyaterm:open-tab-startup-command-dialog",
      listener,
    );
    return () => {
      window.removeEventListener(
        "nyaterm:open-tab-startup-command-dialog",
        listener,
      );
    };
  }, [openCommandDialog, savedConnections, tabs]);

  const isTabMouseActionEnabled = useCallback(
    (tab: Tab, action: TabMouseAction) => {
      switch (action) {
        case "none":
          return false;
        case "rename_tab":
        case "copy_tab_name":
          return true;
        case "copy_server_ip":
          return !!getTabServerIp(tab, savedConnections);
        case "duplicate_session":
          return canSpawnSessionFromTab(tab);
        case "multiplex_ssh":
          return canMultiplexTab(tab, savedConnections);
        case "reconnect_session":
          return canReconnectTab(tab);
        case "disconnect_session":
          return canDisconnectTab(tab);
        case "close_tab":
          return !tab.locked;
      }
    },
    [savedConnections],
  );

  const runTabMouseAction = useCallback(
    (tab: Tab, action: TabMouseAction) => {
      if (!isTabMouseActionEnabled(tab, action)) return false;

      onTabChange(tab.id);

      switch (action) {
        case "none":
          return false;
        case "rename_tab":
          handleRenameTab(tab);
          return true;
        case "copy_tab_name":
          void handleCopyTabName(tab);
          return true;
        case "copy_server_ip":
          void handleCopyServerIp(tab);
          return true;
        case "duplicate_session":
          void onDuplicateSession(tab);
          return true;
        case "multiplex_ssh":
          void onMultiplexSshSession(tab);
          return true;
        case "reconnect_session":
          void onReconnectSession(tab);
          return true;
        case "disconnect_session":
          void onDisconnectSession(tab);
          return true;
        case "close_tab":
          void onTabClose(tab);
          return true;
      }
    },
    [
      handleCopyServerIp,
      handleCopyTabName,
      handleRenameTab,
      isTabMouseActionEnabled,
      onDisconnectSession,
      onDuplicateSession,
      onMultiplexSshSession,
      onReconnectSession,
      onTabChange,
      onTabClose,
    ],
  );

  const handleConfiguredTabMouseAction = useCallback(
    (event: MouseEvent<HTMLDivElement>, tab: Tab, action: TabMouseAction) => {
      if (action === "none") return false;
      if (!isTabMouseActionEnabled(tab, action)) return false;

      event.preventDefault();
      event.stopPropagation();
      return runTabMouseAction(tab, action);
    },
    [isTabMouseActionEnabled, runTabMouseAction],
  );

  const getInsertionIndex = useCallback(
    (event: DragEvent<HTMLDivElement>, index: number) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientX < rect.left + rect.width / 2 ? index : index + 1;
    },
    [],
  );

  const resetDragState = useCallback(() => {
    draggedTabIdRef.current = null;
    droppedTabRef.current = false;
    setDraggedTabId(null);
    setDropIndex(null);
  }, []);

  const setVisibleTabRef = useCallback(
    (tabId: string, element: HTMLDivElement | null) => {
    if (element) {
      tabButtonRefs.current.set(tabId, element);
    } else {
      tabButtonRefs.current.delete(tabId);
    }
    },
    [],
  );

  const getInsertionIndexFromClientX = useCallback(
    (clientX: number) => {
      for (const tab of tabs) {
        const element = tabButtonRefs.current.get(tab.id);
        if (!element) continue;
        const rect = element.getBoundingClientRect();
        const tabIndex = tabs.findIndex((item) => item.id === tab.id);
        if (tabIndex === -1) continue;
        if (clientX < rect.left + rect.width / 2) return tabIndex;
        if (clientX <= rect.right) return tabIndex + 1;
      }
      return tabs.length;
    },
    [tabs],
  );

  const isDragEndFallbackNearTabStrip = useCallback(
    (clientX: number, clientY: number) => {
    const strip = tabStripRef.current;
    if (!strip || clientX === 0) return false;

    const rect = strip.getBoundingClientRect();
    const horizontalTolerance = 48;
    const verticalTolerance = Math.max(96, rect.height * 3);
    const isWithinHorizontalRange =
        clientX >= rect.left - horizontalTolerance &&
        clientX <= rect.right + horizontalTolerance;
    if (!isWithinHorizontalRange) return false;

    if (clientY === 0) return true;

      return (
        clientY >= rect.top - verticalTolerance &&
        clientY <= rect.bottom + verticalTolerance
      );
    },
    [],
  );

  const handleDropAtIndex = useCallback(
    (insertionIndex: number, event?: DragEvent<HTMLDivElement>) => {
      const externalTabId = event?.dataTransfer.getData(
        "application/nyaterm-tab",
      );
      const effectiveTabId =
        draggedTabIdRef.current || draggedTabId || externalTabId;
      if (!effectiveTabId) return;

      droppedTabRef.current = true;
      completedDropRef.current = true;
      const fromIndex = tabs.findIndex((tab) => tab.id === effectiveTabId);
      if (fromIndex === -1) {
        if (onMoveTabHere) {
          onMoveTabHere(effectiveTabId, insertionIndex);
          requestAnimationFrame(() => {
            window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
          });
        }
        resetDragState();
        return;
      }

      const nextIndex =
        insertionIndex > fromIndex ? insertionIndex - 1 : insertionIndex;
      if (nextIndex === fromIndex) {
        resetDragState();
        return;
      }

      onReorderTabs(effectiveTabId, nextIndex);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
      resetDragState();
    },
    [draggedTabId, onMoveTabHere, onReorderTabs, resetDragState, tabs],
  );

  const handleDragStart = (event: DragEvent<HTMLDivElement>, tabId: string) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", tabId);
    event.dataTransfer.setData("application/nyaterm-tab", tabId);
    draggedTabIdRef.current = tabId;
    droppedTabRef.current = false;
    completedDropRef.current = false;
    setDraggedTabId(tabId);
    setDropIndex(tabs.findIndex((tab) => tab.id === tabId));
  };

  const handlePointerDown = (
    event: PointerEvent<HTMLDivElement>,
    tabId: string,
  ) => {
    if (!usePointerTabDrag) return;
    if (
      event.button !== 0 ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey ||
      event.shiftKey
    ) {
      return;
    }

    pointerDragRef.current = {
      tabId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      dragging: false,
    };
  };

  const handlePointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!usePointerTabDrag) return;
    const state = pointerDragRef.current;
    if (!state || state.pointerId !== event.pointerId) return;

    const moved =
      Math.abs(event.clientX - state.startX) >= POINTER_TAB_DRAG_THRESHOLD_PX ||
      Math.abs(event.clientY - state.startY) >= POINTER_TAB_DRAG_THRESHOLD_PX;
    if (!state.dragging) {
      if (!moved) return;
      state.dragging = true;
      suppressTabClickRef.current = true;
      draggedTabIdRef.current = state.tabId;
      droppedTabRef.current = false;
      setDraggedTabId(state.tabId);
      event.currentTarget.setPointerCapture(event.pointerId);
    }

    setDropIndex(getInsertionIndexFromClientX(event.clientX));
    event.preventDefault();
  };

  const handlePointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    if (!usePointerTabDrag) return;
    const state = pointerDragRef.current;
    if (!state || state.pointerId !== event.pointerId) return;
    pointerDragRef.current = null;

    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }

    if (!state.dragging) return;

    if (
      state.dragging &&
      isDragEndFallbackNearTabStrip(event.clientX, event.clientY)
    ) {
      handleDropAtIndex(getInsertionIndexFromClientX(event.clientX));
    } else {
      resetDragState();
    }

    event.preventDefault();
    window.setTimeout(() => {
      suppressTabClickRef.current = false;
    }, 0);
  };

  const handlePointerCancel = (event: PointerEvent<HTMLDivElement>) => {
    if (!usePointerTabDrag) return;
    const state = pointerDragRef.current;
    if (!state || state.pointerId !== event.pointerId) return;

    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    pointerDragRef.current = null;
    if (state.dragging) {
      resetDragState();
      window.setTimeout(() => {
        suppressTabClickRef.current = false;
      }, 0);
    }
  };

  const handleDragEnd = (event: DragEvent<HTMLDivElement>) => {
    if (droppedTabRef.current || completedDropRef.current) {
      completedDropRef.current = false;
      resetDragState();
      return;
    }

    const fallbackTabId = draggedTabIdRef.current;
    if (
      fallbackTabId &&
      isDragEndFallbackNearTabStrip(event.clientX, event.clientY)
    ) {
      handleDropAtIndex(getInsertionIndexFromClientX(event.clientX));
      return;
    }

    resetDragState();
  };

  const renderTabIcon = (tab: Tab) => {
    const pane = getActivePane(tab);

    if (pane?.connecting) {
      return (
        <svg
          aria-hidden="true"
          className="shrink-0 animate-spin"
          style={{
            width: "0.875rem",
            height: "0.875rem",
            color: "var(--df-primary)",
          }}
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
      );
    }

    if (pane?.connectError) {
      return (
        <MdErrorOutline
          className="text-sm shrink-0"
          style={{ color: "var(--destructive, #ef4444)" }}
        />
      );
    }

    const conn = pane?.connectionId
      ? savedConnections.find(
          (connection) => connection.id === pane.connectionId,
        )
      : undefined;
    if (conn?.icon) {
      const iconDef = resolveConnectionIcon(conn.icon);
      const IconComp = iconDef.icon;
      return (
        <IconComp
          className="text-sm shrink-0"
          style={{ color: iconDef.color }}
        />
      );
    }

    if (pane?.type === "Local") {
      return <MdTerminal className="text-sm shrink-0" />;
    }

    return <MdDns className="text-sm shrink-0" />;
  };

  const buildGroupPath = (groupId: string | undefined) => {
    const parts: string[] = [];
    let currentId = groupId;
    while (currentId) {
      const group = groupsById.get(currentId);
      if (!group) break;
      parts.unshift(group.name);
      currentId = group.parent_id;
    }
    return parts.join("/");
  };

  const getConnectionProtocol = (connection: SavedConnection) => {
    switch (connection.type) {
      case "local_terminal":
        return "shell";
      case "telnet":
        return "telnet";
      case "serial":
        return "serial";
      case "rdp":
        return "rdp";
      default:
        return "ssh";
    }
  };

  const getRecentConnectionLabel = (connection: SavedConnection) => {
    const groupPath = buildGroupPath(connection.group_id);
    const path = [groupPath, connection.name].filter(Boolean).join("/");
    return `${getConnectionProtocol(connection)}://${path || connection.name}`;
  };

  const renderConnectionIcon = (connection: SavedConnection) => {
    if (connection.icon) {
      const iconDef = resolveConnectionIcon(connection.icon);
      const IconComp = iconDef.icon;
      return (
        <IconComp
          className="text-sm shrink-0"
          style={{ color: iconDef.color }}
        />
      );
    }

    if (connection.type === "local_terminal") {
      return <MdTerminal className="text-sm shrink-0 text-emerald-500/70" />;
    }

    return <MdDns className="text-sm shrink-0 text-emerald-500/70" />;
  };

  const renderTabItem = (tab: Tab, index: number) => {
    const isActive = activeTabId === tab.id;
    const isFocused = focusedTabId === tab.id;
    const showUnreadIndicator = !isFocused && unreadTabIds?.has(tab.id);
    const isDisconnected = disconnectedTabIds?.has(tab.id) ?? false;
    const tabNameColor = isDisconnected
      ? "var(--df-danger)"
      : showUnreadIndicator
        ? "var(--df-link)"
        : undefined;
    const activePane = getActivePane(tab);
    const activeSessionId = getTabActiveSessionId(tab);
    const dynamicSnapshot = getDynamicTitleSnapshot(activeSessionId);
    const displayName = getTabDisplayName(
      tab,
      dynamicSnapshot?.effectiveTitle ?? null,
    );
    const cwdPresentation =
      activePane?.paneKind === "terminal" && dynamicSnapshot?.enabled
        ? dynamicSnapshot.cwd
        : null;
    const isDirty = collectSessionPanes(tab.root).some(
      (pane) =>
        pane.paneKind === "file" && fileDocumentStates.get(pane.id)?.dirty,
    );
    const activePaneIsFile = activePane?.paneKind === "file";
    const accentColor = tab.tabColor;
    const conn = getTabConnection(tab, savedConnections);
    const canCopyIp = !!getTabServerIp(tab, savedConnections);
    const host = canCopyIp ? conn?.host : undefined;
    const groupPath = buildGroupPath(conn?.group_id);
    const doubleClickAction = normalizeTabMouseAction(
      appSettings.interaction.tab_double_click_action,
    );
    const middleClickAction = normalizeTabMouseAction(
      appSettings.interaction.tab_middle_click_action,
    );
    const rightClickAction = normalizeTabMouseAction(
      appSettings.interaction.tab_right_click_action,
    );
    const sshAddress =
      conn?.username && conn.host && conn.port
        ? `ssh -p ${conn.port} ${conn.username}@${conn.host}`
        : null;

    const renderTooltipCopyRow = (
      value: string,
      label: string,
      copiedMessage: string,
      copyValue = value,
    ) => (
      <div className="flex min-w-0 items-center gap-2 text-[var(--df-text-muted)]">
        <span className="min-w-0 truncate">{value}</span>
        <button
          type="button"
          className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--df-text-dimmed)] transition-colors hover:bg-accent hover:text-[var(--df-primary)]"
          aria-label={label}
          onClick={(event) => {
            event.stopPropagation();
            navigator.clipboard
              .writeText(copyValue)
              .then(() => toast.success(copiedMessage))
              .catch(() => toast.error(t("tabCtx.copyFailed")));
          }}
        >
          <MdContentCopy className="text-[12px]" />
        </button>
      </div>
    );

    const tooltipContent =
      tab.locked ||
      host ||
      sshAddress ||
      groupPath ||
      isDisconnected ||
      showUnreadIndicator ||
      isDirty ||
      cwdPresentation ? (
        <div className="flex max-w-[260px] min-w-0 flex-col gap-1">
          {isDirty && (
            <div className="flex min-w-0 items-center gap-2 text-[var(--df-primary)]">
              <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-current" />
              <span className="min-w-0 truncate">
                {t("fileEditor.unsaved")}
              </span>
            </div>
          )}
          {isDisconnected && (
            <div className="flex min-w-0 items-center gap-2 text-[var(--df-danger)]">
              <MdErrorOutline className="text-[12px] shrink-0" />
              <span className="min-w-0 truncate">
                {t("tabCtx.disconnected")}
              </span>
            </div>
          )}
          {showUnreadIndicator && (
            <div className="flex min-w-0 items-center gap-2 text-[var(--df-link)]">
              <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-green-500" />
              <span className="min-w-0 truncate">
                {t("tabCtx.unreadOutput")}
              </span>
            </div>
          )}
          {tab.locked && (
            <div className="flex min-w-0 items-center gap-2 text-[var(--df-text-muted)]">
              <MdLock className="text-[12px] shrink-0" />
              <span className="min-w-0 truncate">{t("tabCtx.locked")}</span>
            </div>
          )}
          {groupPath && (
            <div className="flex min-w-0 items-center gap-2 text-[var(--df-text-muted)]">
              <MdFolder className="text-[12px] shrink-0 text-amber-500/80" />
              <span className="min-w-0 truncate">
                {t("tabCtx.group")}: {groupPath}
              </span>
            </div>
          )}
          {host &&
            renderTooltipCopyRow(
              host,
              t("tabCtx.copyIp"),
              t("tabCtx.ipCopied"),
            )}
          {sshAddress &&
            renderTooltipCopyRow(
              sshAddress,
              t("tabCtx.copySshAddress"),
              t("tabCtx.sshAddressCopied"),
            )}
          {cwdPresentation &&
            renderTooltipCopyRow(
              cwdPresentation.displayPath,
              t("tabCtx.copyWorkingDirectory"),
              t("tabCtx.workingDirectoryCopied"),
              cwdPresentation.copyValue,
            )}
        </div>
      ) : undefined;

    const tabButton = (
      <div
        draggable={!usePointerTabDrag}
        className={`group relative flex items-center gap-2 border-r pl-3 pr-2 text-xs transition-[color,background-color,opacity] duration-200 ${
          isActive ? "font-semibold" : "font-medium df-hover"
        } ${draggedTabId === tab.id ? "opacity-60" : ""}`}
        style={{
          borderColor: "var(--df-border)",
          backgroundColor: isActive
            ? accentColor
              ? `color-mix(in srgb, ${accentColor} 8%, var(--df-bg))`
              : "var(--df-bg)"
            : accentColor
              ? `color-mix(in srgb, ${accentColor} 5%, transparent)`
              : "transparent",
          color: isActive ? "var(--df-text)" : "var(--df-text-muted)",
        }}
        onClick={() => {
          if (suppressTabClickRef.current) {
            suppressTabClickRef.current = false;
            return;
          }
          onTabChange(tab.id);
          focusOpenTabTerminal(tab);
        }}
        onDoubleClick={(event) => {
          if (
            !activePaneIsFile ||
            supportsFileTabMouseAction(doubleClickAction)
          ) {
          handleConfiguredTabMouseAction(event, tab, doubleClickAction);
          }
        }}
        onMouseDown={(event) => {
          if (
            event.button === 1 &&
            middleClickAction !== "none" &&
            (!activePaneIsFile || supportsFileTabMouseAction(middleClickAction))
          ) {
            handleConfiguredTabMouseAction(event, tab, middleClickAction);
          }
        }}
        onAuxClick={(event) => {
          if (
            event.button === 1 &&
            middleClickAction !== "none" &&
            (!activePaneIsFile || supportsFileTabMouseAction(middleClickAction))
          ) {
            event.preventDefault();
            event.stopPropagation();
          }
        }}
        onContextMenu={(event) => {
          if (
            rightClickAction === "none" ||
            (activePaneIsFile && !supportsFileTabMouseAction(rightClickAction))
          ) {
            onTabChange(tab.id);
            return;
          }

          handleConfiguredTabMouseAction(event, tab, rightClickAction);
        }}
        onPointerDown={(event) => handlePointerDown(event, tab.id)}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerEnd}
        onPointerCancel={handlePointerCancel}
        onDragStart={(event) => handleDragStart(event, tab.id)}
        onDragEnd={(event) => {
          handleDragEnd(event);
        }}
        onDragOver={(event) => {
          if (
            !draggedTabId &&
            !event.dataTransfer.types.includes("application/nyaterm-tab")
          )
            return;
          event.preventDefault();
          setDropIndex(getInsertionIndex(event, index));
        }}
        onDrop={(event) => {
          event.preventDefault();
          handleDropAtIndex(getInsertionIndex(event, index), event);
        }}
      >
        {isActive && (
          <div
            className="absolute top-0 left-0 h-[2px] w-full"
            style={{
              backgroundColor: accentColor || "var(--df-primary)",
              boxShadow: `0 1px 4px ${accentColor || "var(--df-primary)"}`,
            }}
          />
        )}

        {isActive && (
          <div
            className="absolute bottom-0 left-0 z-10 h-[1px] w-full"
            style={{ backgroundColor: "var(--df-bg)" }}
          />
        )}

        {renderTabIcon(tab)}

        <span
          className="shrink-0 min-w-[1.25em] text-xs font-semibold tabular-nums leading-none"
          style={{
            color: isActive ? "var(--df-text-muted)" : "var(--df-text-dimmed)",
          }}
          aria-hidden="true"
        >
          {index + 1}
        </span>

        <span
          className="max-w-[160px] truncate whitespace-nowrap"
          style={{ color: tabNameColor }}
        >
          {displayName}
          {isDirty ? " *" : ""}
        </span>

        <SyncIndicator
          tab={tab}
          syncGroups={syncGroups}
          broadcastToAll={broadcastToAll}
        />

        <div className="relative ml-0.5 flex h-[18px] w-[18px] shrink-0 items-center justify-center">
          {tab.locked ? (
            <div
              className={`absolute inset-0 flex items-center justify-center rounded transition-all duration-200 ${
                isActive
                  ? "text-[var(--df-primary)]"
                  : "text-[var(--df-text-dimmed)] opacity-0 group-hover:opacity-100"
              } hover:!bg-accent hover:!text-[var(--df-primary)] active:scale-90`}
              title={t("tabCtx.lockedCloseBlocked")}
              onPointerDown={(event) => {
                event.stopPropagation();
              }}
              onClick={(event) => {
                event.stopPropagation();
                toast.info(t("tabCtx.lockedCloseBlocked"));
              }}
            >
              <MdLock className="text-[12px]" />
            </div>
          ) : showUnreadIndicator ? (
            <span className="h-2 w-2 rounded-full bg-green-500 animate-breathing" />
          ) : (
            <div
              className={`absolute inset-0 flex items-center justify-center rounded transition-all duration-200 ${
                isActive
                  ? "text-[var(--df-text-muted)]"
                  : "text-[var(--df-text-dimmed)] opacity-0 group-hover:opacity-100"
              } hover:!bg-red-500/10 hover:!text-red-500 active:scale-90 active:!bg-red-500/20`}
              onPointerDown={(event) => {
                event.stopPropagation();
              }}
              onClick={(event) => {
                event.stopPropagation();
                void onTabClose(tab);
              }}
            >
              <MdClose className="text-[12px]" />
            </div>
          )}
        </div>
      </div>
    );

    return (
      <div
        key={tab.id}
        ref={(element) => setVisibleTabRef(tab.id, element)}
        className="relative flex shrink-0"
      >
        {dropIndex === index && (draggedTabId || dropIndex !== null) && (
          <div
            className="pointer-events-none absolute inset-y-1 left-0 z-20 w-0.5 rounded-full"
            style={{ backgroundColor: "var(--df-primary)" }}
          />
        )}

        <TabContextMenu
          tab={tab}
          tooltipContent={tooltipContent}
          tabs={tabs}
          onDuplicateSession={onDuplicateSession}
          onMultiplexSshSession={onMultiplexSshSession}
          onDuplicateSessionWithCommand={(targetTab) =>
            openCommandDialog(targetTab, "duplicate")
          }
          onMultiplexSshSessionWithCommand={(targetTab) =>
            openCommandDialog(targetTab, "multiplex")
          }
          onReconnectSession={onReconnectSession}
          onDisconnectSession={onDisconnectSession}
          onSplitSession={onSplitSession}
          onUnsplit={onUnsplit}
          onCloseSession={onCloseSession}
          onCloseAll={() => setCloseAllDialogOpen(true)}
          onCloseInactive={onCloseInactive}
          onCloseRight={onCloseRight}
          onSessionInfo={onSessionInfo}
          onActivateTab={onTabChange}
          canCopyIp={canCopyIp}
          onRenameTab={handleRenameTab}
          onCopyTabName={handleCopyTabName}
          onCopyServerIp={handleCopyServerIp}
        >
          {tabButton}
        </TabContextMenu>
      </div>
    );
  };

  const focusOpenTabTerminal = (tab: Tab) => {
    const pane = getActivePane(tab);

    requestAnimationFrame(() => {
      scrollTabIntoView(tab.id, true);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
        if (pane?.paneKind === "terminal" && pane.sessionId) {
          void emit(`focus-terminal-${pane.sessionId}`);
        }
      });
    });
  };

  const handleSelectOpenTab = (tab: Tab) => {
    pendingOpenTabFocusRef.current = tab;
    onTabChange(tab.id);
    focusOpenTabTerminal(tab);
  };

  const renderOpenTabMenuItem = (tab: Tab, index: number) => {
    const isActive = activeTabId === tab.id;
    const displayName = getActiveSessionTabDisplayName(tab, getDynamicTitle);
    const isDirty = collectSessionPanes(tab.root).some(
      (pane) =>
        pane.paneKind === "file" && fileDocumentStates.get(pane.id)?.dirty,
    );
    const showUnreadIndicator = !isActive && unreadTabIds?.has(tab.id);
    const isDisconnected = disconnectedTabIds?.has(tab.id) ?? false;
    const tabNameColor = isDisconnected
      ? "var(--df-danger)"
      : showUnreadIndicator
        ? "var(--df-link)"
        : undefined;

    return (
      <DropdownMenuItem
        key={tab.id}
        className="w-full py-1.5"
        onSelect={() => handleSelectOpenTab(tab)}
      >
        <span className="grid w-full grid-cols-[1rem_1rem_minmax(0,1fr)_1rem] items-center gap-x-2 gap-y-1.5">
          <span className="flex h-4 w-4 items-center justify-center">
            {isActive ? (
              <MdCheck
                className="text-sm"
                style={{ color: "var(--df-primary)" }}
              />
            ) : showUnreadIndicator ? (
              <span className="h-2 w-2 rounded-full bg-green-500 animate-breathing" />
            ) : null}
          </span>
          <span className="flex h-4 w-4 items-center justify-center text-[var(--df-text-dimmed)]">
            {renderTabIcon(tab)}
          </span>
          <span className="min-w-0 truncate" style={{ color: tabNameColor }}>
            <span className="mr-1.5 text-[var(--df-text-dimmed)] tabular-nums">
              {index + 1}
            </span>
            {displayName}
            {isDirty ? " *" : ""}
          </span>
          <span className="flex h-4 w-4 items-center justify-center text-[var(--df-text-dimmed)]">
            {tab.locked ? (
              <MdLock className="text-[12px]" aria-label={t("tabCtx.locked")} />
            ) : null}
          </span>
        </span>
      </DropdownMenuItem>
    );
  };

  const renderConnectionMenuItem = (
    connection: SavedConnection,
    label = connection.name,
  ) => (
    <DropdownMenuItem
      key={connection.id}
      className="max-w-[320px]"
      onSelect={() => void onConnectConnection(connection)}
      title={label}
    >
      {renderConnectionIcon(connection)}
      <span className="min-w-0 truncate">{label}</span>
    </DropdownMenuItem>
  );

  const renderEmptyMenuItem = (label: string) => (
    <DropdownMenuItem disabled className="text-muted-foreground">
      <span className="truncate">{label}</span>
    </DropdownMenuItem>
  );

  const renderGroupNode = (node: ConnectionGroupNode) => (
    <DropdownMenuSub key={node.group.id}>
      <DropdownMenuSubTrigger className="max-w-[320px]">
        <MdFolder className="text-sm shrink-0 text-amber-500/70" />
        <span className="min-w-0 truncate">{node.group.name}</span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="min-w-[240px] max-w-[340px] max-h-[70vh] overflow-y-auto">
        {node.children.map(renderGroupNode)}
        {node.children.length > 0 && node.connections.length > 0 && (
          <DropdownMenuSeparator />
        )}
        {node.connections.map((connection) =>
          renderConnectionMenuItem(connection),
        )}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );

  return (
    <>
      <div
        className="flex h-9 shrink-0"
        style={{
          backgroundColor: "var(--df-bg-panel)",
          boxShadow: "inset 0 -1px 0 var(--df-border)",
        }}
      >
        <div
          ref={tabStripRef}
          className="tab-strip-scroll relative flex min-w-0 flex-1 overflow-x-auto overflow-y-hidden"
          onScroll={handleTabStripScroll}
          onWheel={handleTabStripWheel}
        >
          {tabs.map((tab) =>
            renderTabItem(
              tab,
              tabs.findIndex((item) => item.id === tab.id),
            ),
          )}

          <div
            className="relative flex min-w-6 flex-1 shrink-0"
            onDragOver={(event) => {
              if (
                !draggedTabId &&
                !event.dataTransfer.types.includes("application/nyaterm-tab")
              )
                return;
              event.preventDefault();
              setDropIndex(tabs.length);
            }}
            onDrop={(event) => {
              event.preventDefault();
              handleDropAtIndex(tabs.length, event);
            }}
          >
            {(draggedTabId || dropIndex !== null) &&
              dropIndex === tabs.length && (
              <div
                className="pointer-events-none absolute inset-y-1 left-0 z-20 w-0.5 rounded-full"
                style={{ backgroundColor: "var(--df-primary)" }}
              />
            )}
          </div>
        </div>

        {tabStripScroll.hasOverflow && (
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="flex h-full w-8 shrink-0 items-center justify-center border-l transition-colors df-hover"
                    style={{
                      color: "var(--df-text-muted)",
                      borderColor: "var(--df-border)",
                    }}
                    aria-label={t("terminal.openTabs")}
                  >
                    <MdExpandMore className="text-base" />
                  </button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={6} showArrow>
                {t("terminal.openTabs")}
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent
              align="end"
              className="w-64 max-w-[calc(100vw-1rem)]"
              onCloseAutoFocus={(event) => {
                event.preventDefault();
                const pendingTab = pendingOpenTabFocusRef.current;
                pendingOpenTabFocusRef.current = null;
                if (pendingTab) {
                  focusOpenTabTerminal(pendingTab);
                }
              }}
            >
              <DropdownMenuLabel className="text-muted-foreground">
                {t("terminal.openTabs")}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              {openTabsMenuItems.map(({ tab, index }) =>
                renderOpenTabMenuItem(tab, index),
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}

        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <button
                  className="flex h-full w-9 shrink-0 items-center justify-center border-l transition-colors df-hover"
                  style={{
                    color: "var(--df-text-muted)",
                    borderColor: "var(--df-border)",
                  }}
                  aria-label={t("terminal.newSession")}
                >
                  <MdAdd className="text-base" />
                </button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={6} showArrow>
              {t("terminal.newSession")}
            </TooltipContent>
          </Tooltip>
          <DropdownMenuContent
            align="end"
            className="min-w-[260px] max-w-[360px]"
          >
            <DropdownMenuGroup>
              <DropdownMenuItem onSelect={() => onAddTab()}>
                <MdAdd className="text-sm text-muted-foreground" />
                {t("terminal.newSession")}
              </DropdownMenuItem>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>
                  <MdDns className="text-sm text-muted-foreground" />
                  {t("terminal.allSessions")}
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="min-w-[260px] max-w-[360px] max-h-[70vh] overflow-y-auto">
                  {connectionTree.roots.length === 0 &&
                  connectionTree.ungrouped.length === 0 ? (
                    renderEmptyMenuItem(t("terminal.noSavedSessions"))
                  ) : (
                    <>
                      {connectionTree.roots.map(renderGroupNode)}
                      {connectionTree.roots.length > 0 &&
                        connectionTree.ungrouped.length > 0 && (
                        <DropdownMenuSeparator />
                      )}
                      {connectionTree.ungrouped.map((connection) =>
                        renderConnectionMenuItem(connection),
                      )}
                    </>
                  )}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            </DropdownMenuGroup>

            <DropdownMenuSeparator />
            <DropdownMenuLabel className="text-muted-foreground">
              {t("terminal.shellSessions")}
            </DropdownMenuLabel>
            {shellConnections.length > 0
              ? shellConnections.map((connection) =>
                  renderConnectionMenuItem(connection),
                )
              : renderEmptyMenuItem(t("terminal.noShellSessions"))}

            <DropdownMenuSeparator />
            <DropdownMenuLabel className="text-muted-foreground">
              <span className="inline-flex items-center gap-2">
                <MdHistory className="text-sm" />
                {t("terminal.recentSessions")}
              </span>
            </DropdownMenuLabel>
            {recentConnections.length > 0
              ? recentConnections.map((connection) =>
                  renderConnectionMenuItem(
                    connection,
                    getRecentConnectionLabel(connection),
                  ),
                )
              : renderEmptyMenuItem(t("terminal.noRecentSessions"))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <TabRenameDialog
        open={!!renameTab}
        value={renameValue}
        onOpenChange={(open) => {
          if (!open) setRenameTab(null);
        }}
        onValueChange={setRenameValue}
        onSubmit={handleRenameSubmit}
      />

      <TabStartupCommandDialog
        open={!!commandDialog}
        value={commandValue}
        delayMs={commandDelayMs}
        onOpenChange={(open) => {
          if (!open) closeCommandDialog();
        }}
        onValueChange={setCommandValue}
        onDelayMsChange={setCommandDelayMs}
        onSubmit={handleCommandDialogSubmit}
      />

      <CloseAllSessionsDialog
        open={closeAllDialogOpen}
        closing={closingAllSessions}
        onOpenChange={(open) => {
          if (closingAllSessions && !open) return;
          setCloseAllDialogOpen(open);
        }}
        onConfirm={handleConfirmCloseAll}
      />
    </>
  );
}

export default memo(TabBar);

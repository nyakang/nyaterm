import {
  type DragEvent,
  memo,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdCellTower,
  MdClose,
  MdDns,
  MdErrorOutline,
  MdFolder,
  MdHistory,
  MdTerminal,
} from "react-icons/md";
import { RiExpandDiagonalLine } from "react-icons/ri";
import { getActiveGroupForSession, isSessionPausedInGroup } from "@/lib/syncInputGroups";
import { getActivePane, getTabDisplayName } from "@/lib/workspaceTabs";
import type { Group, PaneSplitDirection, SavedConnection, Tab } from "@/types/global";
import { useApp } from "../../context/AppContext";
import { CONNECTION_ICONS } from "../icons";
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
  onTabChange: (tabId: string) => void;
  onTabClose: (tab: Tab) => void | Promise<void>;
  onAddTab: () => void;
  onConnectConnection: (connection: SavedConnection) => void | Promise<void>;
  onDuplicateSession: (tab: Tab) => void | Promise<void>;
  onReconnectSession: (tab: Tab) => void | Promise<void>;
  onSplitSession: (tab: Tab, direction: PaneSplitDirection) => void | Promise<void>;
  onCloseSession: (tab: Tab) => void | Promise<void>;
  onCloseAll: () => void | Promise<void>;
  onCloseInactive: (keepTabId: string) => void | Promise<void>;
  onCloseRight: (tabId: string) => void | Promise<void>;
  onSessionInfo: (tab: Tab) => void | Promise<void>;
  onReorderTabs: (fromTabId: string, toIndex: number) => void;
}

interface ConnectionGroupNode {
  group: Group;
  children: ConnectionGroupNode[];
  connections: SavedConnection[];
  totalCount: number;
}

const TAB_WIDTH_FALLBACK = 220;
const TAB_OVERFLOW_TRIGGER_WIDTH = 36;

function areStringArraysEqual(left: string[], right: string[]) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function computeVisibleTabIds(
  tabs: Tab[],
  tabWidths: Map<string, number>,
  availableWidth: number,
  activeTabId: string | null,
) {
  if (tabs.length === 0) return [];
  if (availableWidth <= 0) {
    return [tabs.find((tab) => tab.id === activeTabId)?.id ?? tabs[0].id];
  }

  const widths = tabs.map((tab) => Math.ceil(tabWidths.get(tab.id) ?? TAB_WIDTH_FALLBACK));
  const totalWidth = widths.reduce((sum, width) => sum + width, 0);
  if (totalWidth <= availableWidth) return tabs.map((tab) => tab.id);

  const selectedIndexes: number[] = [];
  let usedWidth = 0;

  for (let index = tabs.length - 1; index >= 0; index -= 1) {
    const width = widths[index];
    if (usedWidth + width > availableWidth) break;
    selectedIndexes.push(index);
    usedWidth += width;
  }

  const activeIndex = activeTabId ? tabs.findIndex((tab) => tab.id === activeTabId) : -1;
  const activeWidth = activeIndex >= 0 ? widths[activeIndex] : 0;
  if (activeIndex >= 0 && !selectedIndexes.includes(activeIndex)) {
    while (selectedIndexes.length > 0 && usedWidth + activeWidth > availableWidth) {
      const leftmostSelectedIndex = Math.min(...selectedIndexes);
      selectedIndexes.splice(selectedIndexes.indexOf(leftmostSelectedIndex), 1);
      usedWidth -= widths[leftmostSelectedIndex];
    }

    if (activeWidth <= availableWidth || selectedIndexes.length === 0) {
      selectedIndexes.push(activeIndex);
      usedWidth += activeWidth;
    }
  }

  for (let index = tabs.length - 1; index >= 0; index -= 1) {
    if (selectedIndexes.includes(index)) continue;
    const width = widths[index];
    if (usedWidth + width <= availableWidth) {
      selectedIndexes.push(index);
      usedWidth += width;
    }
  }

  const selectedSet = new Set(selectedIndexes);
  return tabs.filter((_, index) => selectedSet.has(index)).map((tab) => tab.id);
}

function compareSortOrder(left: { sort_order?: number }, right: { sort_order?: number }) {
  return (left.sort_order ?? 0) - (right.sort_order ?? 0);
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
    if (!sessionId || pane?.connecting || pane?.connectError) return null;
    if (broadcastToAll) return null;
    return getActiveGroupForSession(sessionId, syncGroups);
  }, [sessionId, syncGroups, broadcastToAll, pane?.connecting, pane?.connectError]);

  const isMember = broadcastToAll || !!activeGroup;
  const isPaused =
    activeGroup && sessionId ? isSessionPausedInGroup(activeGroup, sessionId) : false;

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
  onTabChange,
  onTabClose,
  onAddTab,
  onConnectConnection,
  onDuplicateSession,
  onReconnectSession,
  onSplitSession,
  onCloseSession,
  onCloseAll,
  onCloseInactive,
  onCloseRight,
  onSessionInfo,
  onReorderTabs,
}: TabBarProps) {
  const { t } = useTranslation();
  const { appSettings, savedConnections, savedGroups, syncGroups, broadcastToAll } = useApp();
  const [draggedTabId, setDraggedTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [overflowMenuOpen, setOverflowMenuOpen] = useState(false);
  const [overflowTooltipOpen, setOverflowTooltipOpen] = useState(false);
  const [suppressOverflowTooltip, setSuppressOverflowTooltip] = useState(false);
  const tabStripRef = useRef<HTMLDivElement | null>(null);
  const tabMeasureRefs = useRef(new Map<string, HTMLDivElement>());
  const [visibleTabIds, setVisibleTabIds] = useState<string[]>(() => tabs.map((tab) => tab.id));

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

    const pruneEmpty = (node: ConnectionGroupNode): ConnectionGroupNode | null => {
      const children = node.children
        .map(pruneEmpty)
        .filter((child): child is ConnectionGroupNode => !!child);
      if (node.connections.length === 0 && children.length === 0) return null;
      return { ...node, children };
    };

    return {
      roots: roots.map(pruneEmpty).filter((node): node is ConnectionGroupNode => !!node),
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
    const byId = new Map(savedConnections.map((connection) => [connection.id, connection]));
    return (appSettings.ui.recent_connection_ids ?? [])
      .map((connectionId) => byId.get(connectionId))
      .filter((connection): connection is SavedConnection => !!connection)
      .slice(0, 10);
  }, [appSettings.ui.recent_connection_ids, savedConnections]);

  const updateVisibleTabs = useCallback(() => {
    const strip = tabStripRef.current;
    if (!strip) return;

    const tabWidths = new Map<string, number>();
    for (const tab of tabs) {
      const measuredWidth = tabMeasureRefs.current.get(tab.id)?.offsetWidth;
      if (measuredWidth) tabWidths.set(tab.id, measuredWidth);
    }

    const totalTabWidth = tabs.reduce(
      (sum, tab) => sum + Math.ceil(tabWidths.get(tab.id) ?? TAB_WIDTH_FALLBACK),
      0,
    );
    const hasOverflowTrigger = visibleTabIds.length < tabs.length;
    const availableWithoutOverflowTrigger =
      strip.clientWidth + (hasOverflowTrigger ? TAB_OVERFLOW_TRIGGER_WIDTH : 0);
    const availableWidth =
      totalTabWidth <= availableWithoutOverflowTrigger
        ? availableWithoutOverflowTrigger
        : Math.max(0, availableWithoutOverflowTrigger - TAB_OVERFLOW_TRIGGER_WIDTH);

    const nextVisibleTabIds = computeVisibleTabIds(tabs, tabWidths, availableWidth, activeTabId);

    setVisibleTabIds((current) =>
      areStringArraysEqual(current, nextVisibleTabIds) ? current : nextVisibleTabIds,
    );
  }, [activeTabId, tabs, visibleTabIds.length]);

  useLayoutEffect(() => {
    updateVisibleTabs();

    const strip = tabStripRef.current;
    if (!strip) return;

    const observer = new ResizeObserver(() => updateVisibleTabs());
    observer.observe(strip);

    return () => observer.disconnect();
  }, [updateVisibleTabs]);

  const visibleTabIdSet = useMemo(() => new Set(visibleTabIds), [visibleTabIds]);
  const visibleTabs = useMemo(
    () => tabs.filter((tab) => visibleTabIdSet.has(tab.id)),
    [tabs, visibleTabIdSet],
  );
  const overflowTabs = useMemo(
    () => tabs.filter((tab) => !visibleTabIdSet.has(tab.id)),
    [tabs, visibleTabIdSet],
  );

  const getInsertionIndex = useCallback((event: DragEvent<HTMLDivElement>, index: number) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientX < rect.left + rect.width / 2 ? index : index + 1;
  }, []);

  const resetDragState = useCallback(() => {
    setDraggedTabId(null);
    setDropIndex(null);
  }, []);

  const setMeasureTabRef = useCallback((tabId: string, element: HTMLDivElement | null) => {
    if (element) {
      tabMeasureRefs.current.set(tabId, element);
    } else {
      tabMeasureRefs.current.delete(tabId);
    }
  }, []);

  const handleDropAtIndex = useCallback(
    (insertionIndex: number) => {
      if (!draggedTabId) return;

      const fromIndex = tabs.findIndex((tab) => tab.id === draggedTabId);
      if (fromIndex === -1) {
        resetDragState();
        return;
      }

      const nextIndex = insertionIndex > fromIndex ? insertionIndex - 1 : insertionIndex;
      onReorderTabs(draggedTabId, nextIndex);
      requestAnimationFrame(() => {
        window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
      });
      resetDragState();
    },
    [draggedTabId, onReorderTabs, resetDragState, tabs],
  );

  const handleDragStart = (event: DragEvent<HTMLDivElement>, tabId: string) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", tabId);
    setDraggedTabId(tabId);
    setDropIndex(tabs.findIndex((tab) => tab.id === tabId));
  };

  const renderTabIcon = (tab: Tab) => {
    const pane = getActivePane(tab);

    if (pane?.connecting) {
      return (
        <svg
          aria-hidden="true"
          className="shrink-0 animate-spin"
          style={{ width: "0.875rem", height: "0.875rem", color: "var(--df-primary)" }}
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
      ? savedConnections.find((connection) => connection.id === pane.connectionId)
      : undefined;
    const iconDef = conn?.icon ? CONNECTION_ICONS[conn.icon] : null;
    if (iconDef) {
      const IconComp = iconDef.icon;
      return <IconComp className="text-sm shrink-0" style={{ color: iconDef.color }} />;
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
    const iconDef = connection.icon ? CONNECTION_ICONS[connection.icon] : null;
    if (iconDef) {
      const IconComp = iconDef.icon;
      return <IconComp className="text-sm shrink-0" style={{ color: iconDef.color }} />;
    }

    if (connection.type === "local_terminal") {
      return <MdTerminal className="text-sm shrink-0 text-emerald-500/70" />;
    }

    return <MdDns className="text-sm shrink-0 text-emerald-500/70" />;
  };

  const renderTabItem = (tab: Tab, index: number, measureOnly = false) => {
    const isActive = activeTabId === tab.id;
    const isFocused = focusedTabId === tab.id;
    const showUnreadIndicator = !isFocused && unreadTabIds?.has(tab.id);
    const displayName = getTabDisplayName(tab);
    const accentColor = tab.tabColor;

    const tabButton = (
      <div
        ref={measureOnly ? (element) => setMeasureTabRef(tab.id, element) : undefined}
        draggable={!measureOnly}
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
        onClick={measureOnly ? undefined : () => onTabChange(tab.id)}
        onContextMenu={measureOnly ? undefined : () => onTabChange(tab.id)}
        onDragStart={measureOnly ? undefined : (event) => handleDragStart(event, tab.id)}
        onDragEnd={
          measureOnly
            ? undefined
            : () => {
                resetDragState();
                requestAnimationFrame(() => {
                  window.dispatchEvent(new CustomEvent("nyaterm:refresh-terminals"));
                });
              }
        }
        onDragOver={
          measureOnly
            ? undefined
            : (event) => {
                if (!draggedTabId) return;
                event.preventDefault();
                setDropIndex(getInsertionIndex(event, index));
              }
        }
        onDrop={
          measureOnly
            ? undefined
            : (event) => {
                event.preventDefault();
                handleDropAtIndex(getInsertionIndex(event, index));
              }
        }
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

        {measureOnly ? (
          <span className="max-w-[160px] truncate whitespace-nowrap">{displayName}</span>
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="max-w-[160px] truncate whitespace-nowrap">{displayName}</span>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={6} showArrow className="max-w-xs truncate">
              {displayName}
            </TooltipContent>
          </Tooltip>
        )}

        <SyncIndicator tab={tab} syncGroups={syncGroups} broadcastToAll={broadcastToAll} />

        <div className="relative ml-0.5 flex h-[18px] w-[18px] shrink-0 items-center justify-center">
          {showUnreadIndicator ? (
            <span className="h-2 w-2 rounded-full bg-green-500 animate-breathing" />
          ) : (
            <div
              className={`absolute inset-0 flex items-center justify-center rounded transition-all duration-200 ${
                isActive
                  ? "text-[var(--df-text-muted)]"
                  : "text-[var(--df-text-dimmed)] opacity-0 group-hover:opacity-100"
              } hover:!bg-red-500/10 hover:!text-red-500 active:scale-90 active:!bg-red-500/20`}
              onClick={
                measureOnly
                  ? undefined
                  : (event) => {
                      event.stopPropagation();
                      void onTabClose(tab);
                    }
              }
            >
              <MdClose className="text-[12px]" />
            </div>
          )}
        </div>
      </div>
    );

    if (measureOnly) {
      return (
        <div key={tab.id} className="relative flex shrink-0">
          {tabButton}
        </div>
      );
    }

    return (
      <div key={tab.id} className="relative flex shrink-0">
        {draggedTabId && dropIndex === index && (
          <div
            className="pointer-events-none absolute inset-y-1 left-0 z-20 w-0.5 rounded-full"
            style={{ backgroundColor: "var(--df-primary)" }}
          />
        )}

        <TabContextMenu
          tab={tab}
          tabs={tabs}
          onDuplicateSession={onDuplicateSession}
          onReconnectSession={onReconnectSession}
          onSplitSession={onSplitSession}
          onCloseSession={onCloseSession}
          onCloseAll={onCloseAll}
          onCloseInactive={onCloseInactive}
          onCloseRight={onCloseRight}
          onSessionInfo={onSessionInfo}
          onActivateTab={onTabChange}
        >
          {tabButton}
        </TabContextMenu>
      </div>
    );
  };

  const renderOverflowTabItem = (tab: Tab) => {
    const isActive = activeTabId === tab.id;
    const displayName = getTabDisplayName(tab);

    return (
      <DropdownMenuItem
        key={tab.id}
        className="max-w-[320px] pr-1"
        onSelect={(event) => {
          event.preventDefault();
          onTabChange(tab.id);
          setOverflowMenuOpen(false);
        }}
      >
        {renderTabIcon(tab)}
        <span
          className={`min-w-0 flex-1 truncate ${isActive ? "font-semibold" : ""}`}
          style={{ color: isActive ? "var(--df-text)" : undefined }}
        >
          {displayName}
        </span>
        <button
          type="button"
          tabIndex={-1}
          aria-label={t("tabCtx.close")}
          className="ml-2 flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-red-500/10 hover:text-red-500"
          onPointerDown={(event) => {
            event.stopPropagation();
          }}
          onPointerUp={(event) => {
            event.stopPropagation();
          }}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void onTabClose(tab);
            setOverflowMenuOpen(false);
          }}
        >
          <MdClose className="text-[12px]" />
        </button>
      </DropdownMenuItem>
    );
  };

  const renderConnectionMenuItem = (connection: SavedConnection, label = connection.name) => (
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
        {node.children.length > 0 && node.connections.length > 0 && <DropdownMenuSeparator />}
        {node.connections.map((connection) => renderConnectionMenuItem(connection))}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );

  return (
    <div
      className="flex h-9 shrink-0 overflow-hidden"
      style={{
        backgroundColor: "var(--df-bg-panel)",
        boxShadow: "inset 0 -1px 0 var(--df-border)",
      }}
    >
      <div ref={tabStripRef} className="relative flex min-w-0 flex-1 overflow-hidden">
        <div
          className="pointer-events-none absolute -left-[10000px] top-0 flex h-9 opacity-0"
          aria-hidden="true"
        >
          {tabs.map((tab, index) => renderTabItem(tab, index, true))}
        </div>

        {visibleTabs.map((tab) =>
          renderTabItem(
            tab,
            tabs.findIndex((item) => item.id === tab.id),
          ),
        )}

        <div
          className="relative flex min-w-6 flex-1 shrink-0"
          onDragOver={(event) => {
            if (!draggedTabId) return;
            event.preventDefault();
            setDropIndex(tabs.length);
          }}
          onDrop={(event) => {
            event.preventDefault();
            handleDropAtIndex(tabs.length);
          }}
        >
          {draggedTabId && dropIndex === tabs.length && (
            <div
              className="pointer-events-none absolute inset-y-1 left-0 z-20 w-0.5 rounded-full"
              style={{ backgroundColor: "var(--df-primary)" }}
            />
          )}
        </div>
      </div>

      {overflowTabs.length > 0 && (
        <DropdownMenu
          open={overflowMenuOpen}
          onOpenChange={(open) => {
            setOverflowMenuOpen(open);
            if (open) {
              setOverflowTooltipOpen(false);
              setSuppressOverflowTooltip(true);
            } else {
              setSuppressOverflowTooltip(true);
            }
          }}
        >
          <Tooltip
            open={!overflowMenuOpen && !suppressOverflowTooltip && overflowTooltipOpen}
            onOpenChange={(open) =>
              setOverflowTooltipOpen(open && !overflowMenuOpen && !suppressOverflowTooltip)
            }
          >
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <button
                  className="flex h-full w-9 shrink-0 items-center justify-center border-l transition-colors df-hover"
                  style={{ color: "var(--df-text-muted)", borderColor: "var(--df-border)" }}
                  aria-label={t("terminal.showHiddenSessions")}
                  onPointerEnter={() => {
                    if (!overflowMenuOpen && !suppressOverflowTooltip) {
                      setOverflowTooltipOpen(true);
                    }
                  }}
                  onPointerLeave={() => {
                    setOverflowTooltipOpen(false);
                    setSuppressOverflowTooltip(false);
                  }}
                  onClick={() => {
                    setOverflowTooltipOpen(false);
                    setSuppressOverflowTooltip(true);
                  }}
                >
                  <RiExpandDiagonalLine className="text-base" />
                </button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={6} showArrow>
              {t("terminal.showHiddenSessions")}
            </TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="min-w-[240px] max-w-[360px]">
            <DropdownMenuLabel className="text-muted-foreground">
              {t("terminal.hiddenSessions")}
            </DropdownMenuLabel>
            {overflowTabs.map(renderOverflowTabItem)}
          </DropdownMenuContent>
        </DropdownMenu>
      )}

      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button
                className="flex h-full w-9 shrink-0 items-center justify-center border-l transition-colors df-hover"
                style={{ color: "var(--df-text-muted)", borderColor: "var(--df-border)" }}
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
        <DropdownMenuContent align="end" className="min-w-[260px] max-w-[360px]">
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
                {connectionTree.roots.length === 0 && connectionTree.ungrouped.length === 0 ? (
                  renderEmptyMenuItem(t("terminal.noSavedSessions"))
                ) : (
                  <>
                    {connectionTree.roots.map(renderGroupNode)}
                    {connectionTree.roots.length > 0 && connectionTree.ungrouped.length > 0 && (
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
            ? shellConnections.map((connection) => renderConnectionMenuItem(connection))
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
                renderConnectionMenuItem(connection, getRecentConnectionLabel(connection)),
              )
            : renderEmptyMenuItem(t("terminal.noRecentSessions"))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

export default memo(TabBar);

import { type DragEvent, memo, useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdAutoAwesome,
  MdCellTower,
  MdClose,
  MdDns,
  MdErrorOutline,
  MdTerminal,
} from "react-icons/md";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { openAIAssistant } from "@/lib/aiEvents";
import { getActiveGroupForSession, isSessionPausedInGroup } from "@/lib/syncInputGroups";
import { getActivePane, getTabDisplayName } from "@/lib/workspaceTabs";
import type { PaneSplitDirection, Tab } from "@/types/global";
import { useApp } from "../../context/AppContext";
import { CONNECTION_ICONS } from "../icons";
import TabContextMenu from "./TabContextMenu";

interface TabBarProps {
  tabs: Tab[];
  activeTabId: string | null;
  focusedTabId?: string | null;
  unreadTabIds?: Set<string>;
  onTabChange: (tabId: string) => void;
  onTabClose: (tab: Tab) => void | Promise<void>;
  onAddTab: () => void;
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
  const { savedConnections, syncGroups, broadcastToAll } = useApp();
  const [draggedTabId, setDraggedTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const getInsertionIndex = useCallback((event: DragEvent<HTMLDivElement>, index: number) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientX < rect.left + rect.width / 2 ? index : index + 1;
  }, []);

  const resetDragState = useCallback(() => {
    setDraggedTabId(null);
    setDropIndex(null);
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
        window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"));
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

  return (
    <div
      className="flex h-9 overflow-x-auto overflow-y-hidden terminal-scroll shrink-0"
      style={{
        backgroundColor: "var(--df-bg-panel)",
        boxShadow: "inset 0 -1px 0 var(--df-border)",
      }}
    >
      {tabs.map((tab, index) => {
        const isActive = activeTabId === tab.id;
        const isFocused = focusedTabId === tab.id;
        const showUnreadIndicator = !isFocused && unreadTabIds?.has(tab.id);
        const displayName = getTabDisplayName(tab);
        const accentColor = tab.tabColor;

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
            >
              <div
                draggable
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
                onClick={() => onTabChange(tab.id)}
                onDragStart={(event) => handleDragStart(event, tab.id)}
                onDragEnd={() => {
                  resetDragState();
                  requestAnimationFrame(() => {
                    window.dispatchEvent(new CustomEvent("dragonfly:refresh-terminals"));
                  });
                }}
                onDragOver={(event) => {
                  if (!draggedTabId) return;
                  event.preventDefault();
                  setDropIndex(getInsertionIndex(event, index));
                }}
                onDrop={(event) => {
                  event.preventDefault();
                  handleDropAtIndex(getInsertionIndex(event, index));
                }}
                title={displayName}
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

                <span className="max-w-[160px] truncate whitespace-nowrap">{displayName}</span>

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
            </TabContextMenu>
          </div>
        );
      })}

      <div
        className="relative flex shrink-0"
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
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              className="px-3 transition-colors df-hover"
              style={{ color: "var(--df-text-muted)" }}
              title={t("ai.title")}
            >
              <MdAutoAwesome className="mx-auto text-base" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onClick={() => openAIAssistant({ action: "explain_output" })}>
              {t("ai.explainRecent")}
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => openAIAssistant({ action: "generate_command" })}>
              {t("ai.generateCommand")}
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => openAIAssistant({ action: "analyze_error" })}>
              {t("ai.analyzeError")}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onClick={() =>
                openAIAssistant({
                  action: "generate_command",
                  userInput: "生成一组只读巡检命令计划",
                })
              }
            >
              {t("ai.inspectionPlan")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {draggedTabId && dropIndex === tabs.length && (
          <div
            className="pointer-events-none absolute inset-y-1 left-0 z-20 w-0.5 rounded-full"
            style={{ backgroundColor: "var(--df-primary)" }}
          />
        )}
        <button
          className="px-3 transition-colors df-hover"
          style={{ color: "var(--df-text-muted)" }}
          onClick={onAddTab}
          title={t("terminal.newConnection")}
        >
          <MdAdd className="mx-auto text-base" />
        </button>
      </div>
    </div>
  );
}

export default memo(TabBar);

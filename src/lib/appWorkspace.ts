import { supportsPanel } from "./backend/runtime";
import type { TerminalWindowNode } from "@/lib/tabWindows";
import { collectSessionPanes } from "@/lib/workspaceTabs";
import { parsePluginPanelId } from "@/lib/plugins";
import type {
  ActivityBarLayout,
  ActivityBarZone,
  SavedConnection,
  SessionInfo,
  SessionPane,
  Tab,
  UiConfig,
  WorkspaceSessionType,
} from "@/types/global";

export const ACTIVITY_LAYOUT_ZONES = [
  "left_top",
  "left_bottom",
  "right_top",
  "right_bottom",
] as const satisfies readonly ActivityBarZone[];

export const DEFAULT_ACTIVITY_BAR_LAYOUT: ActivityBarLayout = {
  left_top: ["fileExplorer", "notes", "network", "securityAuth"],
  left_bottom: ["syncBackupHistory", "plugins", "settings"],
  right_top: [
    "savedConnections",
    "aiAssistant",
    "activeSessions",
    "commandHistory",
    "resourceMonitor",
    "gpuMonitor",
    "ascendNpuMonitor",
    "processManager",
    "dockerManager",
  ],
  right_bottom: ["quickCmdBar", "serialSend", "recording", "lock"],
  show_labels: false,
  hidden_items: [],
};

export const ACTIVITY_BAR_ITEM_IDS = new Set<string>([
  ...DEFAULT_ACTIVITY_BAR_LAYOUT.left_top,
  ...DEFAULT_ACTIVITY_BAR_LAYOUT.left_bottom,
  ...DEFAULT_ACTIVITY_BAR_LAYOUT.right_top,
  ...DEFAULT_ACTIVITY_BAR_LAYOUT.right_bottom,
]);

export const ACTIVITY_BAR_PANEL_ITEM_IDS = new Set<string>([
  "plugins",
  "fileExplorer",
  "notes",
  "network",
  "securityAuth",
  "syncBackupHistory",
  "savedConnections",
  "aiAssistant",
  "activeSessions",
  "commandHistory",
  "resourceMonitor",
  "gpuMonitor",
  "ascendNpuMonitor",
  "processManager",
  "dockerManager",
  "recording",
]);

export const NON_PANEL_IDS = new Set([
  "settings",
  "lock",
  "quickCmdBar",
  "serialSend",
]);

/** Panels that never join the multi-open stack and are always shown on their own. */
export const EXCLUSIVE_PANEL_IDS = new Set(["aiAssistant"]);

export type PanelOpenMode = "docked" | "floating";

export interface FloatingPanelsState {
  left: string | null;
  right: string | null;
}

export function normalizePanelOpenMode(
  value: string | null | undefined,
): PanelOpenMode {
  return value === "floating" ? "floating" : "docked";
}

export function canUseFloatingPanel(id: string): boolean {
  return (
    (ACTIVITY_BAR_PANEL_ITEM_IDS.has(id) || Boolean(parsePluginPanelId(id))) &&
    !NON_PANEL_IDS.has(id)
  );
}

const MONITOR_PANEL_VISIBILITY: Record<string, (ui: UiConfig) => boolean> = {
  notes: (ui) => ui.show_notes_panel ?? true,
  resourceMonitor: (ui) => ui.show_remote_stats ?? true,
  gpuMonitor: (ui) => ui.show_gpu_monitor ?? false,
  ascendNpuMonitor: (ui) => ui.show_ascend_npu_monitor ?? false,
  processManager: (ui) => ui.show_process_manager ?? false,
  dockerManager: (ui) => ui.show_docker_manager ?? false,
};

export type TrayAction =
  | { type: "open_new_session"; targetWindowLabel?: string | null }
  | {
      type: "focus_session";
      sessionId: string;
      targetWindowLabel?: string | null;
    }
  | {
      type: "open_panel";
      panelId: "activeSessions" | "syncBackupHistory";
      targetWindowLabel?: string | null;
    }
  | { type: "open_settings"; targetWindowLabel?: string | null }
  | { type: "lock_screen"; targetWindowLabel?: string | null }
  | { type: "check_updates"; targetWindowLabel?: string | null }
  | { type: "request_quit"; targetWindowLabel?: string | null };

export function canCreateSessionFromPane(
  pane:
    | Pick<SessionPane, "paneKind" | "type" | "connectionId" | "temporaryConfig">
    | null
    | undefined,
): pane is Pick<SessionPane, "paneKind" | "type" | "connectionId" | "temporaryConfig"> {
  return (
    !!pane &&
    pane.paneKind !== "file" &&
    (pane.type === "Local" || !!pane.connectionId || hasMatchingTemporaryConfig(pane))
  );
}

type SessionActionPane = Pick<
  SessionPane,
  "paneKind" | "type" | "connectionId" | "temporaryConfig" | "sessionId"
>;

export function isSavedMoshSessionPane(
  pane: SessionActionPane | null | undefined,
  savedConnections: SavedConnection[],
) {
  if (
    !pane ||
    pane.paneKind !== "terminal" ||
    pane.type !== "SSH" ||
    !pane.connectionId
  ) {
    return false;
  }
  const connection = savedConnections.find((item) => item.id === pane.connectionId);
  return connection?.type === "ssh" && connection.ssh_transport === "mosh";
}

export function canUseStartupCommandForPane(
  pane: SessionActionPane | null | undefined,
  savedConnections: SavedConnection[],
) {
  return (
    !!pane &&
    pane.paneKind === "terminal" &&
    !isSavedMoshSessionPane(pane, savedConnections)
  );
}

export function canMultiplexSshPane(
  pane: SessionActionPane | null | undefined,
  sessionInfoById: Map<string, SessionInfo> | null | undefined,
) {
  if (!pane || pane.paneKind !== "terminal" || pane.type !== "SSH") return false;
  const runtimeMode = sessionInfoById?.get(pane.sessionId)?.ssh_runtime_mode;
  return runtimeMode === "standard" || runtimeMode === "terminal";
}

export function hasMatchingTemporaryConfig(
  pane: Pick<SessionPane, "type" | "temporaryConfig"> | null | undefined,
) {
  if (!pane?.temporaryConfig) return false;
  switch (pane.type) {
    case "SSH":
      return pane.temporaryConfig.protocol === "ssh";
    case "Telnet":
      return pane.temporaryConfig.protocol === "telnet";
    case "Serial":
      return pane.temporaryConfig.protocol === "serial";
    default:
      return false;
  }
}

export function assertMatchingTemporaryConfig<
  T extends Pick<SessionPane, "type" | "temporaryConfig">,
>(pane: T) {
  if (!pane.temporaryConfig || hasMatchingTemporaryConfig(pane)) return;
  throw new Error("Temporary session config protocol mismatch");
}

export function hasLiveSession<
  T extends Pick<SessionPane, "connecting" | "connectError">,
>(pane: T | null | undefined): pane is T {
  return !!pane && !pane.connecting && !pane.connectError;
}

export function isNonSerialSessionType(type: WorkspaceSessionType): boolean {
  return type === "SSH" || type === "Local" || type === "Telnet";
}

export function getItemSide(
  id: string,
  layout: ActivityBarLayout,
): "left" | "right" | null {
  if (layout.left_top.includes(id) || layout.left_bottom.includes(id))
    return "left";
  if (layout.right_top.includes(id) || layout.right_bottom.includes(id))
    return "right";
  return null;
}

export function isActivityItemAvailable(id: string, ui: UiConfig): boolean {
  return supportsPanel(id) && (MONITOR_PANEL_VISIBILITY[id]?.(ui) ?? true);
}

export function reduceFloatingPanelSelect(
  state: FloatingPanelsState,
  panelId: string,
  side: "left" | "right",
): FloatingPanelsState {
  return {
    ...state,
    [side]: state[side] === panelId ? null : panelId,
  };
}

export function moveFloatingPanelSide(
  state: FloatingPanelsState,
  panelId: string,
  targetSide: "left" | "right",
): FloatingPanelsState {
  const sourceSide = targetSide === "left" ? "right" : "left";
  if (state[targetSide] !== panelId && state[sourceSide] !== panelId) {
    return state;
  }
  return {
    ...state,
    [sourceSide]: state[sourceSide] === panelId ? null : state[sourceSide],
    [targetSide]: panelId,
  };
}

export function clearUnavailableFloatingPanels(
  state: FloatingPanelsState,
  ui: UiConfig,
): FloatingPanelsState {
  const left =
    state.left && isActivityItemAvailable(state.left, ui) ? state.left : null;
  const right =
    state.right && isActivityItemAvailable(state.right, ui) ? state.right : null;
  return left === state.left && right === state.right ? state : { left, right };
}

export function isActivityItemVisible(id: string, ui: UiConfig): boolean {
  return isActivityItemAvailable(id, ui);
}

export function isActivityItemHidden(id: string, ui: UiConfig): boolean {
  return (ui.activity_bar_layout.hidden_items ?? []).includes(id);
}

export function isActivityBarItemVisible(id: string, ui: UiConfig): boolean {
  return isActivityItemAvailable(id, ui) && !isActivityItemHidden(id, ui);
}

export function getVisibleActivityIds(ids: string[], ui: UiConfig): string[] {
  return ids.filter((id) => isActivityBarItemVisible(id, ui));
}

export function hasVisibleActivityBarItems({
  items,
  bottomItems,
}: {
  items: readonly unknown[];
  bottomItems?: readonly unknown[];
  hiddenItems?: readonly unknown[];
}): boolean {
  return items.length > 0 || (bottomItems?.length ?? 0) > 0;
}

export function cloneDefaultActivityBarLayout(): ActivityBarLayout {
  return {
    left_top: [...DEFAULT_ACTIVITY_BAR_LAYOUT.left_top],
    left_bottom: [...DEFAULT_ACTIVITY_BAR_LAYOUT.left_bottom],
    right_top: [...DEFAULT_ACTIVITY_BAR_LAYOUT.right_top],
    right_bottom: [...DEFAULT_ACTIVITY_BAR_LAYOUT.right_bottom],
    show_labels: DEFAULT_ACTIVITY_BAR_LAYOUT.show_labels,
    hidden_items: [],
  };
}

export function hideActivityBarItem(layout: ActivityBarLayout, itemId: string): ActivityBarLayout {
  if ((layout.hidden_items ?? []).includes(itemId)) return layout;
  return {
    ...layout,
    hidden_items: [...(layout.hidden_items ?? []), itemId],
  };
}

export function showActivityBarItem(layout: ActivityBarLayout, itemId: string): ActivityBarLayout {
  if (!(layout.hidden_items ?? []).includes(itemId)) return layout;
  return {
    ...layout,
    hidden_items: (layout.hidden_items ?? []).filter((id) => id !== itemId),
  };
}

export function toggleActivityBarItemVisibility(
  layout: ActivityBarLayout,
  itemId: string,
): ActivityBarLayout {
  return (layout.hidden_items ?? []).includes(itemId)
    ? showActivityBarItem(layout, itemId)
    : hideActivityBarItem(layout, itemId);
}

export function resetActivityBarLayout(): ActivityBarLayout {
  return cloneDefaultActivityBarLayout();
}

export function getActivityBarItemIdsForSide(
  layout: ActivityBarLayout,
  side: "left" | "right",
): string[] {
  return side === "left"
    ? [...layout.left_top, ...layout.left_bottom]
    : [...layout.right_top, ...layout.right_bottom];
}

export function getHiddenActivityItemsForSide(
  ui: UiConfig,
  side: "left" | "right",
  itemIds: Set<string> = ACTIVITY_BAR_ITEM_IDS,
): string[] {
  const hidden = new Set(ui.activity_bar_layout.hidden_items ?? []);
  return getActivityBarItemIdsForSide(ui.activity_bar_layout, side).filter(
    (id) => (itemIds.has(id) || Boolean(parsePluginPanelId(id))) && hidden.has(id) && isActivityItemAvailable(id, ui),
  );
}

export function mergeVisibleReorder(
  currentIds: string[],
  orderedVisibleIds: string[],
  uiConfig: UiConfig,
): string[] {
  const orderedVisibleSet = new Set(orderedVisibleIds);
  const nextVisibleIds = [...orderedVisibleIds];
  const reordered = currentIds.map((id) => {
    if (!orderedVisibleSet.has(id) || !isActivityBarItemVisible(id, uiConfig)) return id;
    return nextVisibleIds.shift() ?? id;
  });
  return [...reordered, ...nextVisibleIds.filter((id) => !reordered.includes(id))];
}

function getSidePanelOrder(
  layout: ActivityBarLayout,
  side: "left" | "right",
): string[] {
  return side === "left"
    ? [...layout.left_top, ...layout.left_bottom]
    : [...layout.right_top, ...layout.right_bottom];
}

/** Panels currently visible on one side, ordered by activity bar icon order. */
export function getSideOpenPanels(
  ui: UiConfig,
  side: "left" | "right",
  multiOpen: boolean,
): string[] {
  const active = side === "left" ? ui.active_left_panel : ui.active_right_panel;
  if (!multiOpen) {
    return active && isActivityItemVisible(active, ui) ? [active] : [];
  }
  const open = new Set(
    (side === "left" ? ui.left_open_panels : ui.right_open_panels) ?? [],
  );
  if (open.size === 0) return [];
  return getSidePanelOrder(ui.activity_bar_layout, side).filter(
    (id) =>
      open.has(id) &&
      isActivityItemVisible(id, ui) &&
      !NON_PANEL_IDS.has(id) &&
      !EXCLUSIVE_PANEL_IDS.has(id),
  );
}

/** Exclusive panel currently shown on its own over the stack (multi-open mode only). */
export function getSideOverlayPanel(
  ui: UiConfig,
  side: "left" | "right",
  multiOpen: boolean,
): string | null {
  if (!multiOpen) return null;
  const active = side === "left" ? ui.active_left_panel : ui.active_right_panel;
  return active &&
    isActivityItemVisible(active, ui) &&
    EXCLUSIVE_PANEL_IDS.has(active)
    ? active
    : null;
}

/** Toggle a panel in the multi-open stack of its side (activity bar click). */
export function buildMultiPanelToggleUpdate(
  prev: UiConfig,
  panelId: string,
  side: "left" | "right",
): Partial<UiConfig> {
  const openList =
    (side === "left" ? prev.left_open_panels : prev.right_open_panels) ?? [];
  const active =
    side === "left" ? prev.active_left_panel : prev.active_right_panel;

  if (EXCLUSIVE_PANEL_IDS.has(panelId)) {
    const nextActive =
      active === panelId
        ? (openList.find((id) => !EXCLUSIVE_PANEL_IDS.has(id)) ?? null)
        : panelId;
    return side === "left"
      ? { active_left_panel: nextActive }
      : { active_right_panel: nextActive };
  }

  const isOpen = openList.includes(panelId);

  // Clicking an already-open stacked panel while an exclusive panel is shown
  // dismisses the exclusive panel and reveals the stack instead of closing it.
  if (isOpen && active && EXCLUSIVE_PANEL_IDS.has(active)) {
    return side === "left"
      ? { active_left_panel: panelId }
      : { active_right_panel: panelId };
  }

  const nextOpen = isOpen
    ? openList.filter((id) => id !== panelId)
    : [...openList, panelId];
  const nextActive =
    nextOpen.length === 0
      ? null
      : isOpen
        ? active && nextOpen.includes(active)
          ? active
          : nextOpen[0]
        : panelId;
  return side === "left"
    ? { left_open_panels: nextOpen, active_left_panel: nextActive }
    : { right_open_panels: nextOpen, active_right_panel: nextActive };
}

/** Ensure a panel is visible, respecting single/multi-open mode. */
export function buildPanelOpenUpdate(
  prev: UiConfig,
  panelId: string,
  multiOpen: boolean,
  fallbackSide: "left" | "right" = "left",
): Partial<UiConfig> {
  const side = getItemSide(panelId, prev.activity_bar_layout) ?? fallbackSide;
  if (!multiOpen) {
    return side === "right"
      ? {
          active_right_panel: panelId,
          ...(prev.active_left_panel === panelId
            ? { active_left_panel: null }
            : {}),
        }
      : {
          active_left_panel: panelId,
          ...(prev.active_right_panel === panelId
            ? { active_right_panel: null }
            : {}),
        };
  }
  if (EXCLUSIVE_PANEL_IDS.has(panelId)) {
    return side === "left"
      ? { active_left_panel: panelId }
      : { active_right_panel: panelId };
  }
  const openList =
    (side === "left" ? prev.left_open_panels : prev.right_open_panels) ?? [];
  const nextOpen = openList.includes(panelId)
    ? openList
    : [...openList, panelId];
  return side === "left"
    ? { left_open_panels: nextOpen, active_left_panel: panelId }
    : { right_open_panels: nextOpen, active_right_panel: panelId };
}

export function collectActiveNonSerialSessionIds(
  layout: TerminalWindowNode | null,
  tabsById: Map<string, Tab>,
) {
  if (!layout) return [];

  const sessionIds = new Set<string>();

  const visit = (node: TerminalWindowNode) => {
    if (node.kind === "split") {
      visit(node.first);
      visit(node.second);
      return;
    }

    for (const tabId of node.tabIds) {
      const tab = tabsById.get(tabId);
      if (!tab) continue;

      for (const pane of collectSessionPanes(tab.root)) {
        if (
          pane.paneKind === "terminal" &&
          hasLiveSession(pane) &&
          isNonSerialSessionType(pane.type)
        ) {
          sessionIds.add(pane.sessionId);
        }
      }
    }
  };

  visit(layout);
  return [...sessionIds];
}

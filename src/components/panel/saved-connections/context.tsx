import type { TFunction } from "i18next";
import type {
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
} from "react";
import { createContext, useContext } from "react";
import type { NewSessionTarget } from "@/lib/windowManager";
import type { Group, SavedConnection } from "@/types/global";

// ── Component-local types ─────────────────────────────────────────────────

export interface GroupNode {
  group: Group;
  children: GroupNode[];
  connections: SavedConnection[];
  totalCount: number;
}

export type SortMode = "default" | "name-asc" | "name-desc";

export type DragPosition = "before" | "after" | "inside";

export interface DragTarget {
  id: string | null;
  type: "connection" | "group" | "background";
  position: DragPosition;
}

export const naturalCompare = (a: string, b: string) =>
  a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

export interface SavedConnectionsContextValue {
  // UI state
  isDragEnabled: boolean;
  isPointerDragEnabled: boolean;
  dragTarget: DragTarget | null;
  expandedGroups: Set<string>;
  selectedConnectionIds: Set<string>;
  keyboardActiveConnectionId: string | null;
  savedConnections: SavedConnection[];
  savedGroups: Group[];

  // List actions
  toggleGroup: (id: string) => void;
  handleConnect: (conn: SavedConnection) => void;
  handleConnectOnly: (conn: SavedConnection) => void;
  handleConnectSelected: () => void;
  handlePingHost: (conn: SavedConnection) => void;
  handleCopyConnection: (conn: SavedConnection) => void;
  requestMoveConnectionToGroup: (
    conn: SavedConnection,
    groupId: string | null,
  ) => void;
  requestMoveSelectedConnectionsToGroup: (groupId: string | null) => void;
  handleConnectionSelectionStart: (
    conn: SavedConnection,
    event: ReactMouseEvent,
  ) => void;
  handleConnectionContextMenu: (
    conn: SavedConnection,
    event: ReactMouseEvent,
  ) => void;
  registerConnectionElement: (
    id: string,
    element: HTMLDivElement | null,
  ) => void;
  onEditConnection: (
    conn: SavedConnection,
    autoConnect?: boolean,
    target?: NewSessionTarget,
  ) => void;

  // Dialog triggers
  onNewConnection: (parentGroupId?: string) => void;
  requestDeleteConnection: (conn: SavedConnection) => void;
  setRenamingConn: (conn: SavedConnection | null) => void;
  setRenameValue: (v: string) => void;
  setDeleteFolderTarget: (g: Group | null) => void;
  openNewFolderDialog: (parentId: string | null) => void;
  openRenameFolderDialog: (g: Group) => void;
  requestOpenGroupConnections: (node: GroupNode) => void;

  // Drag handlers
  handleDragStart: (
    e: React.DragEvent,
    type: "connection" | "group",
    id: string,
  ) => void;
  handleDragEnd: () => void;
  handleDragEnterItem: (
    e: React.DragEvent,
    id: string,
    type: "connection" | "group",
  ) => void;
  handleDragOverItem: (
    e: React.DragEvent,
    id: string,
    type: "connection" | "group",
  ) => void;
  handleDragLeaveItem: (
    e: React.DragEvent,
    id: string,
    type: "connection" | "group",
  ) => void;
  handleDropItem: (
    e: React.DragEvent,
    id: string,
    tgtType: "connection" | "group",
  ) => Promise<void>;
  handlePointerDragStart: (
    e: ReactPointerEvent,
    type: "connection" | "group",
    id: string,
  ) => void;
  handlePointerDragMove: (e: ReactPointerEvent) => void;
  handlePointerDragEnd: (e: ReactPointerEvent) => void;
  handlePointerDragCancel: (e: ReactPointerEvent) => void;

  t: TFunction;
}

export const SavedConnectionsContext =
  createContext<SavedConnectionsContextValue | null>(null);

export const useSavedConnectionsContext = () => {
  const ctx = useContext(SavedConnectionsContext);
  if (!ctx)
    throw new Error(
      "useSavedConnectionsContext must be used within SavedConnectionsContext.Provider",
    );
  return ctx;
};

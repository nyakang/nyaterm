import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdClose,
  MdCreateNewFolder,
  MdDeleteSweep,
  MdFileUpload,
  MdSearch,
  MdSort,
  MdSortByAlpha,
} from "react-icons/md";
import { toast } from "sonner";
import PanelHeader from "@/components/layout/PanelHeader";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import type { Group, SavedConnection } from "@/types/global";
import { useApp } from "../../context/AppContext";
import { invoke } from "../../lib/invoke";
import { logger } from "../../lib/logger";
import ClearAllDialog from "../dialog/connections/ClearAllDialog";
import DeleteConnectionDialog from "../dialog/connections/DeleteConnectionDialog";
import DeleteFolderDialog from "../dialog/connections/DeleteFolderDialog";
import FolderDialog from "../dialog/connections/FolderDialog";
import ImportDialog from "../dialog/connections/ImportDialog";
import RenameConnectionDialog from "../dialog/connections/RenameConnectionDialog";
import ConnectionItem from "./saved-connections/ConnectionItem";
import type { SavedConnectionsContextValue } from "./saved-connections/context";
import {
  type DragTarget,
  type GroupNode,
  naturalCompare,
  SavedConnectionsContext,
  type SortMode,
} from "./saved-connections/context";
import GroupNodeItem from "./saved-connections/GroupNodeItem";

interface SavedConnectionsProps {
  onNewConnection: (parentGroupId?: string) => void;
  onEditConnection: (connection: SavedConnection, autoConnect?: boolean) => void;
}

/** Grouped saved SSH connections panel. Delegates rendering to sub-components via context. */
export default function SavedConnections({
  onNewConnection,
  onEditConnection,
}: SavedConnectionsProps) {
  const {
    savedConnections,
    savedGroups,
    refreshConnections,
    addPendingTab,
    updateTabSession,
    closeTab,
    appSettings,
    updateUi,
  } = useApp();
  const { t } = useTranslation();

  // ── UI state ──────────────────────────────────────────────────────────────
  // Tracks in-flight connections to prevent duplicate invocations (not shown in UI)
  const connectingIdRef = useRef<string | null>(null);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const [filterText, setFilterText] = useState("");
  const sortMode = (appSettings.ui.saved_connections_sort_mode || "default") as SortMode;

  // ── Dialog state ──────────────────────────────────────────────────────────
  const [deleteTarget, setDeleteTarget] = useState<SavedConnection | null>(null);
  const [renamingConn, setRenamingConn] = useState<SavedConnection | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [deleteFolderTarget, setDeleteFolderTarget] = useState<Group | null>(null);
  const [showClearAllDialog, setShowClearAllDialog] = useState(false);
  const [showImportDialog, setShowImportDialog] = useState(false);
  const [folderDialogOpen, setFolderDialogOpen] = useState(false);
  const [folderDialogName, setFolderDialogName] = useState("");
  const [folderDialogParentId, setFolderDialogParentId] = useState<string | null>(null);
  const [editingGroup, setEditingGroup] = useState<Group | null>(null);

  // ── Drag state ────────────────────────────────────────────────────────────
  const [dragTarget, _setDragTarget] = useState<DragTarget | null>(null);
  const dragTargetRef = useRef<DragTarget | null>(null);
  const dragSourceRef = useRef<{ type: "connection" | "group"; id: string } | null>(null);
  const connectionsRef = useRef(savedConnections);
  const groupsRef = useRef(savedGroups);
  connectionsRef.current = savedConnections;
  groupsRef.current = savedGroups;

  const keyword = filterText.trim().toLowerCase();
  const isDragEnabled = sortMode === "default";

  // ── Derived tree ──────────────────────────────────────────────────────────
  const { rootNodes, ungrouped } = useMemo(() => {
    const filtered = keyword
      ? savedConnections.filter(
          (c) =>
            c.name.toLowerCase().includes(keyword) ||
            c.host.toLowerCase().includes(keyword) ||
            c.username.toLowerCase().includes(keyword),
        )
      : savedConnections;

    const sortConns = (list: SavedConnection[]) => {
      if (sortMode === "name-asc") return [...list].sort((a, b) => naturalCompare(a.name, b.name));
      if (sortMode === "name-desc") return [...list].sort((a, b) => naturalCompare(b.name, a.name));
      return [...list].sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0));
    };

    const sortGroups = (list: Group[]) => {
      if (sortMode === "name-asc") return [...list].sort((a, b) => naturalCompare(a.name, b.name));
      if (sortMode === "name-desc") return [...list].sort((a, b) => naturalCompare(b.name, a.name));
      return [...list].sort((a, b) => a.sort_order - b.sort_order);
    };

    const sorted = sortConns(filtered);
    const connByGroup: Record<string, SavedConnection[]> = {};
    const noGroup: SavedConnection[] = [];

    sorted.forEach((conn) => {
      if (conn.group_id) {
        if (!connByGroup[conn.group_id]) connByGroup[conn.group_id] = [];
        connByGroup[conn.group_id].push(conn);
      } else {
        noGroup.push(conn);
      }
    });

    const map: Record<string, GroupNode> = {};
    const sortedGroups = sortGroups(savedGroups);
    for (const g of sortedGroups) {
      map[g.id] = { group: g, children: [], connections: connByGroup[g.id] || [], totalCount: 0 };
    }

    const roots: GroupNode[] = [];
    for (const g of sortedGroups) {
      const node = map[g.id];
      if (g.parent_id && map[g.parent_id]) map[g.parent_id].children.push(node);
      else roots.push(node);
    }

    const computeTotal = (node: GroupNode): number => {
      node.totalCount =
        node.connections.length + node.children.reduce((s, c) => s + computeTotal(c), 0);
      return node.totalCount;
    };
    roots.forEach(computeTotal);

    const prune = (node: GroupNode): boolean => {
      node.children = node.children.filter(prune);
      return node.connections.length > 0 || node.children.length > 0;
    };

    return { rootNodes: keyword ? roots.filter(prune) : roots, ungrouped: noGroup };
  }, [savedConnections, savedGroups, keyword, sortMode]);

  // ── Actions ───────────────────────────────────────────────────────────────
  const toggleGroup = (groupId: string) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  };

  const handleConnect = async (conn: SavedConnection) => {
    if (connectingIdRef.current === conn.id) return;
    connectingIdRef.current = conn.id;
    const tabId = addPendingTab(conn.name, "SSH", conn.id);
    try {
      const sessionId = await invoke<string>("create_ssh_session", { connectionId: conn.id });
      updateTabSession(tabId, sessionId);
    } catch (e) {
      logger.error(`SSH connection failed for "${conn.name}"`, e);
      toast.error(t("savedConnections.connectionFailed", { error: e }));
      closeTab(tabId);
      onEditConnection(conn, true);
    } finally {
      connectingIdRef.current = null;
    }
  };

  const handleCopyConnection = async (conn: SavedConnection) => {
    try {
      await invoke("save_connection", {
        connection: { ...conn, id: "", name: `${conn.name} (copy)`, password: undefined },
      });
      refreshConnections();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleDeleteConfirm = async () => {
    if (!deleteTarget) return;
    try {
      await invoke("delete_connection", { id: deleteTarget.id });
      refreshConnections();
    } catch (e) {
      toast.error(t("savedConnections.deleteFailed", { error: e }));
    } finally {
      setDeleteTarget(null);
    }
  };

  const handleRenameConnection = async () => {
    if (!renamingConn || !renameValue.trim()) return;
    try {
      await invoke("save_connection", {
        connection: { ...renamingConn, name: renameValue.trim() },
      });
      refreshConnections();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setRenamingConn(null);
    }
  };

  // ── Folder actions ────────────────────────────────────────────────────────
  const openNewFolderDialog = (parentId: string | null) => {
    setEditingGroup(null);
    setFolderDialogName("");
    setFolderDialogParentId(parentId);
    setFolderDialogOpen(true);
  };

  const openRenameFolderDialog = (group: Group) => {
    setEditingGroup(group);
    setFolderDialogName(group.name);
    setFolderDialogParentId(group.parent_id || null);
    setFolderDialogOpen(true);
  };

  const handleFolderDialogSubmit = async () => {
    if (!folderDialogName.trim()) return;
    try {
      if (editingGroup) {
        await invoke("save_group", { group: { ...editingGroup, name: folderDialogName.trim() } });
      } else {
        await invoke("save_group", {
          group: {
            id: "",
            name: folderDialogName.trim(),
            parent_id: folderDialogParentId || null,
            sort_order: savedGroups.length,
          },
        });
      }
      refreshConnections();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setFolderDialogOpen(false);
    }
  };

  const handleDeleteFolder = async () => {
    if (!deleteFolderTarget) return;
    try {
      await invoke("delete_group", { id: deleteFolderTarget.id });
      refreshConnections();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setDeleteFolderTarget(null);
    }
  };

  const handleClearAll = async () => {
    try {
      await invoke("clear_all_connections");
      refreshConnections();
      toast.success(t("savedConnections.clearAllSuccess"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setShowClearAllDialog(false);
    }
  };

  // ── Drag & Drop ───────────────────────────────────────────────────────────
  const setDragTarget = (val: DragTarget | null) => {
    dragTargetRef.current = val;
    _setDragTarget(val);
  };

  const isDescendant = (groupId: string, ancestorId: string): boolean => {
    let cur: string | undefined = groupId;
    while (cur) {
      if (cur === ancestorId) return true;
      cur = groupsRef.current.find((g) => g.id === cur)?.parent_id;
    }
    return false;
  };

  const computeDropPosition = (
    e: React.DragEvent,
    itemType: "connection" | "group",
    srcType: "connection" | "group",
  ): DragTarget["position"] => {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const y = e.clientY - rect.top;
    if (itemType === "group") {
      if (srcType === "connection") return "inside";
      if (y < rect.height * 0.25) return "before";
      if (y > rect.height * 0.75) return "after";
      return "inside";
    }
    return y < rect.height * 0.5 ? "before" : "after";
  };

  const handleDragStart = (e: React.DragEvent, type: "connection" | "group", id: string) => {
    e.stopPropagation();
    e.dataTransfer.effectAllowed = "move";
    dragSourceRef.current = { type, id };
  };

  const handleDragEnd = () => {
    setDragTarget(null);
    dragSourceRef.current = null;
  };

  const handleDragOverItem = (e: React.DragEvent, id: string, type: "connection" | "group") => {
    e.preventDefault();
    e.stopPropagation();
    const source = dragSourceRef.current;
    if (!source) return;
    if (source.type === type && source.id === id) {
      e.dataTransfer.dropEffect = "none";
      return;
    }
    if (source.type === "group" && type === "group" && isDescendant(id, source.id)) {
      e.dataTransfer.dropEffect = "none";
      return;
    }
    if (source.type === "group" && type === "connection") {
      e.dataTransfer.dropEffect = "none";
      return;
    }
    e.dataTransfer.dropEffect = "move";
    const position = computeDropPosition(e, type, source.type);
    const prev = dragTargetRef.current;
    if (prev?.id === id && prev.type === type && prev.position === position) return;
    setDragTarget({ id, type, position });
  };

  const handleDragLeaveItem = (e: React.DragEvent, id: string, type: "connection" | "group") => {
    const related = e.relatedTarget as Node | null;
    if (related && (e.currentTarget as HTMLElement).contains(related)) return;
    const cur = dragTargetRef.current;
    if (cur?.id === id && cur.type === type) setDragTarget(null);
  };

  const handleDropItem = async (
    e: React.DragEvent,
    id: string,
    tgtType: "connection" | "group",
  ) => {
    e.preventDefault();
    e.stopPropagation();
    const source = dragSourceRef.current;
    const target = dragTargetRef.current;
    setDragTarget(null);
    dragSourceRef.current = null;
    if (!source || !target || target.id !== id || target.type !== tgtType) return;

    const connections = connectionsRef.current;
    const groups = groupsRef.current;
    const { id: srcId, type: srcType } = source;
    const { position } = target;

    try {
      if (position === "inside" && tgtType === "group") {
        if (srcType === "connection") {
          const conn = connections.find((c) => c.id === srcId);
          if (conn && conn.group_id !== id) {
            const groupConns = connections
              .filter((c) => c.group_id === id && c.id !== srcId)
              .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0));
            groupConns.push({ ...conn, group_id: id });
            await invoke("save_connection", { connection: { ...conn, group_id: id } });
            await invoke("reorder_items", {
              connections: groupConns.map((c, i) => ({ id: c.id, sort_order: i })),
              groups: [],
            });
            refreshConnections();
          }
        } else {
          const grp = groups.find((g) => g.id === srcId);
          if (grp && grp.parent_id !== id) {
            const groupChildren = groups
              .filter((g) => g.parent_id === id && g.id !== srcId)
              .sort((a, b) => a.sort_order - b.sort_order);
            groupChildren.push({ ...grp, parent_id: id });
            await invoke("save_group", { group: { ...grp, parent_id: id } });
            await invoke("reorder_items", {
              connections: [],
              groups: groupChildren.map((g, i) => ({ id: g.id, sort_order: i })),
            });
            refreshConnections();
          }
        }
        return;
      }

      const targetParentId: string | null =
        tgtType === "connection"
          ? (connections.find((c) => c.id === id)?.group_id ?? null)
          : (groups.find((g) => g.id === id)?.parent_id ?? null);

      const srcConn = srcType === "connection" ? connections.find((c) => c.id === srcId) : null;
      const srcGrp = srcType === "group" ? groups.find((g) => g.id === srcId) : null;

      if (srcConn && (srcConn.group_id ?? null) !== targetParentId)
        await invoke("save_connection", { connection: { ...srcConn, group_id: targetParentId } });
      if (srcGrp && (srcGrp.parent_id ?? null) !== targetParentId)
        await invoke("save_group", { group: { ...srcGrp, parent_id: targetParentId } });

      const connsUpdates: { id: string; sort_order: number }[] = [];
      const groupsUpdates: { id: string; sort_order: number }[] = [];

      if (srcType === "connection" && tgtType === "connection") {
        const siblings = connections
          .filter((c) => (c.group_id ?? null) === targetParentId)
          .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0));
        const list = siblings.filter((c) => c.id !== srcId);
        const tgtIdx = list.findIndex((c) => c.id === id);
        if (tgtIdx >= 0 && srcConn)
          list.splice(position === "before" ? tgtIdx : tgtIdx + 1, 0, srcConn);
        list.forEach((c, i) => {
          connsUpdates.push({ id: c.id, sort_order: i });
        });
      } else if (srcType === "group" && tgtType === "group") {
        const siblings = groups
          .filter((g) => (g.parent_id ?? null) === targetParentId)
          .sort((a, b) => a.sort_order - b.sort_order);
        const list = siblings.filter((g) => g.id !== srcId);
        const tgtIdx = list.findIndex((g) => g.id === id);
        if (tgtIdx >= 0 && srcGrp)
          list.splice(position === "before" ? tgtIdx : tgtIdx + 1, 0, srcGrp);
        list.forEach((g, i) => {
          groupsUpdates.push({ id: g.id, sort_order: i });
        });
      } else {
        if (srcType === "connection" && srcConn) {
          const siblings = connections
            .filter((c) => (c.group_id ?? null) === targetParentId && c.id !== srcId)
            .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0));
          siblings.push(srcConn);
          siblings.forEach((c, i) => {
            connsUpdates.push({ id: c.id, sort_order: i });
          });
        } else if (srcType === "group" && srcGrp) {
          const siblings = groups
            .filter((g) => (g.parent_id ?? null) === targetParentId && g.id !== srcId)
            .sort((a, b) => a.sort_order - b.sort_order);
          siblings.push(srcGrp);
          siblings.forEach((g, i) => {
            groupsUpdates.push({ id: g.id, sort_order: i });
          });
        }
      }

      if (connsUpdates.length > 0 || groupsUpdates.length > 0)
        await invoke("reorder_items", { connections: connsUpdates, groups: groupsUpdates });
      refreshConnections();
    } catch (err) {
      logger.error("Drag drop failed", err);
    }
  };

  const handleDragOverBg = (e: React.DragEvent) => {
    e.preventDefault();
    const source = dragSourceRef.current;
    if (!source) return;
    const isAtRoot =
      source.type === "connection"
        ? !connectionsRef.current.find((c) => c.id === source.id)?.group_id
        : !groupsRef.current.find((g) => g.id === source.id)?.parent_id;
    if (isAtRoot) {
      e.dataTransfer.dropEffect = "none";
      if (dragTargetRef.current !== null) setDragTarget(null);
      return;
    }
    e.dataTransfer.dropEffect = "move";
    if (dragTargetRef.current?.type !== "background")
      setDragTarget({ id: null, type: "background", position: "inside" });
  };

  const handleDropBg = async (e: React.DragEvent) => {
    e.preventDefault();
    const source = dragSourceRef.current;
    const target = dragTargetRef.current;
    setDragTarget(null);
    dragSourceRef.current = null;
    if (!source || target?.type !== "background") return;
    try {
      if (source.type === "connection") {
        const conn = connectionsRef.current.find((c) => c.id === source.id);
        if (conn?.group_id) {
          await invoke("save_connection", { connection: { ...conn, group_id: null } });
          const siblings = connectionsRef.current
            .filter((c) => !c.group_id && c.id !== source.id)
            .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0));
          siblings.push(conn);
          await invoke("reorder_items", {
            connections: siblings.map((c, i) => ({ id: c.id, sort_order: i })),
            groups: [],
          });
          refreshConnections();
        }
      } else {
        const grp = groupsRef.current.find((g) => g.id === source.id);
        if (grp?.parent_id) {
          await invoke("save_group", { group: { ...grp, parent_id: null } });
          const siblings = groupsRef.current
            .filter((g) => !g.parent_id && g.id !== source.id)
            .sort((a, b) => a.sort_order - b.sort_order);
          siblings.push(grp);
          await invoke("reorder_items", {
            connections: [],
            groups: siblings.map((g, i) => ({ id: g.id, sort_order: i })),
          });
          refreshConnections();
        }
      }
    } catch (err) {
      logger.error("Drop to root failed", err);
    }
  };

  // ── Sort button helpers ───────────────────────────────────────────────────
  const cycleSortMode = () => {
    const next =
      sortMode === "default" ? "name-asc" : sortMode === "name-asc" ? "name-desc" : "default";
    updateUi({ saved_connections_sort_mode: next });
  };
  const sortTitle =
    sortMode === "default"
      ? t("savedConnections.sortDefault")
      : sortMode === "name-asc"
        ? t("savedConnections.sortNameAsc")
        : t("savedConnections.sortNameDesc");
  const SortIcon = sortMode === "default" ? MdSort : MdSortByAlpha;
  const sortActive = sortMode !== "default";

  // ── Context value ─────────────────────────────────────────────────────────
  const ctxValue: SavedConnectionsContextValue = {
    isDragEnabled,
    dragTarget,
    expandedGroups,
    toggleGroup,
    handleConnect,
    handleCopyConnection,
    onEditConnection,
    onNewConnection,
    setDeleteTarget,
    setRenamingConn,
    setRenameValue,
    setDeleteFolderTarget,
    openNewFolderDialog,
    openRenameFolderDialog,
    handleDragStart,
    handleDragEnd,
    handleDragOverItem,
    handleDragLeaveItem,
    handleDropItem,
    t,
  };

  // ── Render ────────────────────────────────────────────────────────────────
  return (
    <SavedConnectionsContext.Provider value={ctxValue}>
      <div className="h-full flex flex-col overflow-hidden">
        <PanelHeader
          title={t("panel.savedConnections")}
          actions={
            <span className="text-[0.6875rem]" style={{ color: "var(--df-text-dimmed)" }}>
              {savedConnections.length}
            </span>
          }
        />

        {/* Filter + toolbar */}
        <div
          className="px-2 py-1.5 border-b shrink-0 flex items-center gap-1.5"
          style={{ borderColor: "var(--df-border)" }}
        >
          <div className="relative flex items-center flex-1 min-w-0">
            <MdSearch
              className="absolute left-2 text-sm pointer-events-none"
              style={{ color: "var(--df-text-dimmed)" }}
            />
            <input
              type="text"
              value={filterText}
              onChange={(e) => setFilterText(e.target.value)}
              placeholder={t("savedConnections.filter")}
              className="w-full pl-6 pr-6 py-1 text-xs rounded bg-transparent outline-none border"
              style={{
                borderColor: filterText ? "var(--df-primary)" : "var(--df-border)",
                color: "var(--df-text)",
              }}
            />
            {filterText && (
              <button
                className="absolute right-1.5 p-0.5 rounded transition-colors hover:opacity-70"
                style={{ color: "var(--df-text-dimmed)" }}
                onClick={() => setFilterText("")}
              >
                <MdClose className="text-xs" />
              </button>
            )}
          </div>
          <button
            className="shrink-0 p-1 rounded border transition-colors hover:opacity-80"
            style={{
              color: sortActive ? "var(--df-primary)" : "var(--df-text-muted)",
              borderColor: sortActive ? "var(--df-primary)" : "var(--df-border)",
              backgroundColor: sortActive
                ? "color-mix(in srgb, var(--df-primary) 10%, transparent)"
                : "var(--df-bg-hover)",
            }}
            title={sortTitle}
            onClick={cycleSortMode}
          >
            <SortIcon
              className="text-sm"
              style={{ transform: sortMode === "name-desc" ? "scaleY(-1)" : undefined }}
            />
          </button>
          <button
            className="shrink-0 p-1 rounded border transition-colors hover:opacity-80"
            style={{
              color: "var(--df-text-muted)",
              borderColor: "var(--df-border)",
              backgroundColor: "var(--df-bg-hover)",
            }}
            title={t("savedConnections.importSessions")}
            onClick={() => setShowImportDialog(true)}
          >
            <MdFileUpload className="text-sm" />
          </button>
          <button
            className="shrink-0 p-1 rounded border transition-colors hover:opacity-80"
            style={{
              color: "var(--df-text-muted)",
              borderColor: "var(--df-border)",
              backgroundColor: "var(--df-bg-hover)",
            }}
            title={t("savedConnections.newFolder")}
            onClick={() => openNewFolderDialog(null)}
          >
            <MdCreateNewFolder className="text-sm" />
          </button>
          <button
            className="shrink-0 p-1 rounded border transition-colors hover:opacity-80"
            style={{
              color: "var(--df-text-muted)",
              borderColor: "var(--df-border)",
              backgroundColor: "var(--df-bg-hover)",
            }}
            title={t("savedConnections.newConnection")}
            onClick={() => onNewConnection()}
          >
            <MdAdd className="text-sm" />
          </button>
          {savedConnections.length > 0 && (
            <button
              className="shrink-0 p-1 rounded border transition-colors hover:opacity-80"
              style={{
                color: "var(--df-text-muted)",
                borderColor: "var(--df-border)",
                backgroundColor: "var(--df-bg-hover)",
              }}
              title={t("savedConnections.clearAll")}
              onClick={() => setShowClearAllDialog(true)}
            >
              <MdDeleteSweep className="text-sm" />
            </button>
          )}
        </div>

        {/* List */}
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div
              className={`flex-1 overflow-y-auto p-1.5 text-xs space-y-0.5 terminal-scroll ${dragTarget?.type === "background" ? "ring-inset ring-2 ring-primary/20" : ""}`}
              onDragEnter={isDragEnabled ? (e) => e.preventDefault() : undefined}
              onDragOver={isDragEnabled ? handleDragOverBg : undefined}
              onDrop={isDragEnabled ? handleDropBg : undefined}
            >
              {savedConnections.length === 0 ? (
                <div
                  className="text-center py-4 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  {t("panel.noSavedConnections")}
                </div>
              ) : rootNodes.length === 0 && ungrouped.length === 0 ? (
                <div
                  className="text-center py-4 text-xs"
                  style={{ color: "var(--df-text-dimmed)" }}
                >
                  {t("savedConnections.noResults")}
                </div>
              ) : (
                <>
                  {rootNodes.map((node) => (
                    <GroupNodeItem key={node.group.id} node={node} depth={0} />
                  ))}
                  {ungrouped.length > 0 && rootNodes.length > 0 && (
                    <div
                      className="mt-1 pt-1 border-t"
                      style={{
                        borderColor: "color-mix(in srgb, var(--df-border) 50%, transparent)",
                      }}
                    />
                  )}
                  {ungrouped.map((conn) => (
                    <ConnectionItem key={conn.id} conn={conn} indented={false} />
                  ))}
                </>
              )}
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent className="min-w-[160px]">
            <ContextMenuItem onClick={() => onNewConnection()}>
              <MdAdd className="text-[0.875rem] text-muted-foreground mr-2" />
              {t("savedConnections.newConnection")}
            </ContextMenuItem>
            <ContextMenuItem onClick={() => openNewFolderDialog(null)}>
              <MdCreateNewFolder className="text-[0.875rem] text-muted-foreground mr-2" />
              {t("savedConnections.newFolder")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onClick={() => setShowImportDialog(true)}>
              <MdFileUpload className="text-[0.875rem] text-muted-foreground mr-2" />
              {t("savedConnections.importSessions")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>

        {/* Dialogs */}
        <DeleteConnectionDialog
          open={!!deleteTarget}
          connectionName={deleteTarget?.name}
          onConfirm={handleDeleteConfirm}
          onCancel={() => setDeleteTarget(null)}
        />
        <DeleteFolderDialog
          open={!!deleteFolderTarget}
          folderName={deleteFolderTarget?.name}
          onConfirm={handleDeleteFolder}
          onCancel={() => setDeleteFolderTarget(null)}
        />
        <FolderDialog
          open={folderDialogOpen}
          isEditing={!!editingGroup}
          name={folderDialogName}
          onNameChange={setFolderDialogName}
          onSubmit={handleFolderDialogSubmit}
          onCancel={() => setFolderDialogOpen(false)}
        />
        <RenameConnectionDialog
          open={!!renamingConn}
          value={renameValue}
          onValueChange={setRenameValue}
          onSubmit={handleRenameConnection}
          onCancel={() => setRenamingConn(null)}
        />
        <ClearAllDialog
          open={showClearAllDialog}
          onConfirm={handleClearAll}
          onCancel={() => setShowClearAllDialog(false)}
        />
        <ImportDialog open={showImportDialog} onClose={() => setShowImportDialog(false)} />
      </div>
    </SavedConnectionsContext.Provider>
  );
}

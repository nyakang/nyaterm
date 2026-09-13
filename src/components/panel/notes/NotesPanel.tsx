import { open } from "@tauri-apps/plugin-dialog";
import {
  type DragEvent,
  type KeyboardEvent,
  useDeferredValue,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdCreateNewFolder, MdDescription } from "react-icons/md";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useNotesTree } from "@/hooks/useNotesTree";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { openNoteEditor } from "@/lib/windowManager";
import type { NoteExportResult, NoteTreeNode } from "@/types/notes";
import NotesPanelHeader from "./NotesPanelHeader";
import NoteTree from "./NoteTree";
import {
  buildNoteTree,
  collectSiblingNames,
  filterNoteTree,
  findNoteNode,
  flattenNoteFolders,
  flattenVisibleNoteTree,
  isDescendantFolder,
  validateNoteInputName,
} from "./noteTreeUtils";

function countFolderContents(node: NoteTreeNode) {
  let folders = 0;
  let notes = 0;
  const visit = (item: NoteTreeNode) => {
    for (const child of item.children) {
      if (child.kind === "folder") folders += 1;
      else notes += 1;
      visit(child);
    }
  };
  visit(node);
  return { folders, notes };
}

export default function NotesPanel() {
  const { t } = useTranslation();
  const {
    folders,
    notes,
    loading,
    error,
    refresh,
    selectedNodeId,
    setSelectedNodeId,
    expandedFolderIds,
    setExpandedFolderIds,
    createFolder,
    createNote,
    renameNode,
    moveNode,
    deleteNode,
    runAction,
  } = useNotesTree();
  const [search, setSearch] = useState("");
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<NoteTreeNode | null>(null);
  const [dragOverNodeId, setDragOverNodeId] = useState<string | null>(null);
  const dragSourceRef = useRef<NoteTreeNode | null>(null);
  const deferredSearch = useDeferredValue(search);

  const tree = useMemo(() => buildNoteTree(folders, notes), [folders, notes]);
  const visibleTree = useMemo(
    () => filterNoteTree(tree, deferredSearch.trim()),
    [tree, deferredSearch],
  );
  const visibleRows = useMemo(
    () => flattenVisibleNoteTree(visibleTree, expandedFolderIds),
    [expandedFolderIds, visibleTree],
  );
  const selectedNode = useMemo(() => findNoteNode(tree, selectedNodeId), [selectedNodeId, tree]);
  const folderTargets = useMemo(() => flattenNoteFolders(tree), [tree]);

  const labels = {
    open: t("notes.open"),
    newNote: t("notes.newNote"),
    newFolder: t("notes.newFolder"),
    rename: t("notes.rename"),
    moveTo: t("notes.moveTo"),
    delete: t("notes.delete"),
    refresh: t("common.refresh"),
    root: t("notes.root"),
    search: t("notes.search"),
    expandAll: t("notes.expandAll"),
    collapseAll: t("notes.collapseAll"),
    more: t("common.more"),
    export: t("notes.export"),
  };

  const exportNotes = async () => {
    try {
      const destination = await open({ directory: true, multiple: false });
      if (!destination) return;
      const result = await invoke<NoteExportResult>("export_notes", { destination });
      toast.success(t("notes.exportSuccess", { count: result.noteCount }));
    } catch (err) {
      toast.error(getErrorMessage(err));
      logger.error({
        domain: "ui.error",
        event: "notes.export_failed",
        message: "Failed to export notes",
        error: err,
      });
    }
  };

  const creationParentId = () => {
    if (!selectedNode) return null;
    return selectedNode.kind === "folder" ? selectedNode.id : selectedNode.parentId;
  };

  const startCreateNote = (parentId = creationParentId()) => {
    void runAction(async () => {
      const note = await createNote(parentId);
      setEditingNodeId(note.id);
    });
  };

  const startCreateFolder = (parentId = creationParentId()) => {
    void runAction(async () => {
      const folder = await createFolder(parentId);
      setEditingNodeId(folder.id);
    });
  };

  const toggleFolder = (node: NoteTreeNode) => {
    if (node.kind !== "folder") return;
    const next = new Set(expandedFolderIds);
    if (next.has(node.id)) next.delete(node.id);
    else next.add(node.id);
    setExpandedFolderIds(next);
  };

  const openNode = (node: NoteTreeNode) => {
    if (node.kind !== "note") return;
    void openNoteEditor(node.id, node.name);
  };

  const submitRename = (node: NoteTreeNode, name: string) => {
    const validation = validateNoteInputName(
      name,
      collectSiblingNames(folders, notes, node.parentId, { id: node.id, kind: node.kind }),
    );
    if (validation) return;
    setEditingNodeId(null);
    void runAction(() => renameNode(node.kind, node.id, name));
  };

  const moveToParent = (node: NoteTreeNode, parentId: string | null) => {
    if (
      node.kind === "folder" &&
      parentId &&
      (parentId === node.id || isDescendantFolder(tree, node.id, parentId))
    ) {
      return;
    }
    void runAction(() => moveNode(node.kind, node.id, parentId, Date.now()));
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!selectedNode) return;
    if (event.key === "Enter") {
      event.preventDefault();
      if (selectedNode.kind === "folder") toggleFolder(selectedNode);
      else openNode(selectedNode);
    } else if (event.key === "F2") {
      event.preventDefault();
      setEditingNodeId(selectedNode.id);
    } else if (event.key === "Delete") {
      event.preventDefault();
      setDeleteTarget(selectedNode);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const index = visibleRows.findIndex(({ node }) => node.id === selectedNode.id);
      const nextIndex = event.key === "ArrowDown" ? index + 1 : index - 1;
      const next = visibleRows[Math.max(0, Math.min(visibleRows.length - 1, nextIndex))]?.node;
      if (next) setSelectedNodeId(next.id);
    }
  };

  const handleDragOverNode = (event: DragEvent<HTMLDivElement>, node: NoteTreeNode) => {
    const source = dragSourceRef.current;
    if (!source || source.id === node.id) return;
    if (
      source.kind === "folder" &&
      node.kind === "folder" &&
      isDescendantFolder(tree, source.id, node.id)
    ) {
      event.dataTransfer.dropEffect = "none";
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setDragOverNodeId((current) => (current === node.id ? current : node.id));
  };

  const handleDropNode = (event: DragEvent<HTMLDivElement>, node: NoteTreeNode) => {
    event.preventDefault();
    const source = dragSourceRef.current;
    setDragOverNodeId(null);
    dragSourceRef.current = null;
    if (!source || source.id === node.id) return;
    const parentId = node.kind === "folder" ? node.id : node.parentId;
    moveToParent(source, parentId);
  };

  const handleDragOverRoot = (event: DragEvent<HTMLDivElement>) => {
    if (!dragSourceRef.current) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
  };

  const handleDropRoot = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    const source = dragSourceRef.current;
    setDragOverNodeId(null);
    dragSourceRef.current = null;
    if (source) moveToParent(source, null);
  };

  const deleteDescription = deleteTarget
    ? deleteTarget.kind === "folder"
      ? t("notes.deleteFolderDescription", {
          name: deleteTarget.name,
          ...countFolderContents(deleteTarget),
        })
      : t("notes.deleteNoteDescription", { name: deleteTarget.name })
    : "";

  const isEmpty = folders.length === 0 && notes.length === 0;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <NotesPanelHeader
        title={t("notes.title")}
        search={search}
        onSearchChange={setSearch}
        onNewNote={() => startCreateNote()}
        onNewFolder={() => startCreateFolder()}
        onExpandAll={() => setExpandedFolderIds(new Set(folders.map((folder) => folder.id)))}
        onCollapseAll={() => setExpandedFolderIds(new Set())}
        onRefresh={() => void refresh()}
        onExport={() => void exportNotes()}
        labels={labels}
      />
      <div className="min-h-0 flex-1" role="tree" tabIndex={0} onKeyDown={handleKeyDown}>
        {loading ? (
          <div className="flex h-full items-center justify-center text-xs text-[var(--df-text-dimmed)]">
            {t("common.loading")}
          </div>
        ) : error ? (
          <div className="p-3 text-xs text-red-400">{error}</div>
        ) : isEmpty ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center">
            <MdDescription className="text-2xl text-[var(--df-text-dimmed)]" />
            <div className="text-sm font-medium text-[var(--df-text)]">{t("notes.emptyTitle")}</div>
            <div className="text-xs text-[var(--df-text-dimmed)]">
              {t("notes.emptyDescription")}
            </div>
            <div className="flex items-center gap-2 pt-1">
              <Button size="sm" className="h-7 gap-1.5" onClick={() => startCreateNote(null)}>
                <MdAdd />
                {t("notes.newNote")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="h-7 gap-1.5"
                onClick={() => startCreateFolder(null)}
              >
                <MdCreateNewFolder />
                {t("notes.newFolder")}
              </Button>
            </div>
          </div>
        ) : (
          <NoteTree
            rows={visibleRows}
            folderTargets={folderTargets}
            selectedNodeId={selectedNodeId}
            expandedFolderIds={expandedFolderIds}
            editingNodeId={editingNodeId}
            dragOverNodeId={dragOverNodeId}
            labels={labels}
            onSelect={(node) => setSelectedNodeId(node.id)}
            onToggle={toggleFolder}
            onOpen={openNode}
            onRenameStart={(node) => setEditingNodeId(node.id)}
            onRenameSubmit={submitRename}
            onRenameCancel={() => setEditingNodeId(null)}
            onCreateNote={startCreateNote}
            onCreateFolder={startCreateFolder}
            onMove={moveToParent}
            onDelete={setDeleteTarget}
            onRefresh={() => void refresh()}
            onDragStartNode={(node) => {
              dragSourceRef.current = node;
            }}
            onDragOverNode={handleDragOverNode}
            onDropNode={handleDropNode}
            onDragEnd={() => {
              dragSourceRef.current = null;
              setDragOverNodeId(null);
            }}
            onDragOverRoot={handleDragOverRoot}
            onDropRoot={handleDropRoot}
          />
        )}
      </div>
      <AlertDialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("notes.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{deleteDescription}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                const target = deleteTarget;
                setDeleteTarget(null);
                if (target) void runAction(() => deleteNode(target.kind, target.id));
              }}
            >
              {t("notes.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

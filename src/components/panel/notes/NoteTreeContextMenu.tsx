import { useRef } from "react";
import {
  MdAdd,
  MdCreateNewFolder,
  MdDelete,
  MdDriveFileMove,
  MdEdit,
  MdOpenInNew,
  MdRefresh,
} from "react-icons/md";
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import type { NoteTreeNode } from "@/types/notes";
import type { NoteTreeRow } from "./noteTreeUtils";

export interface NoteTreeMenuLabels {
  open: string;
  newNote: string;
  newFolder: string;
  rename: string;
  moveTo: string;
  delete: string;
  refresh: string;
  root: string;
  expandAll: string;
  collapseAll: string;
}

interface NoteTreeContextMenuProps {
  node: NoteTreeNode | null;
  folderTargets: NoteTreeRow[];
  labels: NoteTreeMenuLabels;
  onOpen: (node: NoteTreeNode) => void;
  onCreateNote: (parentId: string | null) => void;
  onCreateFolder: (parentId: string | null) => void;
  onRename: (node: NoteTreeNode) => void;
  onMove: (node: NoteTreeNode, parentId: string | null) => void;
  onDelete: (node: NoteTreeNode) => void;
  onRefresh: () => void;
}

function containsNode(node: NoteTreeNode, targetId: string): boolean {
  return node.children.some((child) => child.id === targetId || containsNode(child, targetId));
}

export default function NoteTreeContextMenu({
  node,
  folderTargets,
  labels,
  onOpen,
  onCreateNote,
  onCreateFolder,
  onRename,
  onMove,
  onDelete,
  onRefresh,
}: NoteTreeContextMenuProps) {
  const preventNextContextMenuAutoFocusRef = useRef(false);
  const parentId = node?.kind === "folder" ? node.id : (node?.parentId ?? null);
  const moveTargets = folderTargets.filter(
    (item) =>
      item.node.id !== node?.id && !(node?.kind === "folder" && containsNode(node, item.node.id)),
  );

  return (
    <ContextMenuContent
      className="min-w-40"
      onCloseAutoFocus={(event) => {
        if (!preventNextContextMenuAutoFocusRef.current) return;
        preventNextContextMenuAutoFocusRef.current = false;
        event.preventDefault();
      }}
    >
      {node?.kind === "note" ? (
        <ContextMenuItem onClick={() => onOpen(node)}>
          <MdOpenInNew />
          {labels.open}
        </ContextMenuItem>
      ) : null}
      {node ? (
        <>
          {node.kind === "folder" ? (
            <>
              <ContextMenuItem onClick={() => onCreateNote(node.id)}>
                <MdAdd />
                {labels.newNote}
              </ContextMenuItem>
              <ContextMenuItem onClick={() => onCreateFolder(node.id)}>
                <MdCreateNewFolder />
                {labels.newFolder}
              </ContextMenuItem>
            </>
          ) : null}
          <ContextMenuItem
            onClick={() => {
              preventNextContextMenuAutoFocusRef.current = true;
              onRename(node);
            }}
          >
            <MdEdit />
            {labels.rename}
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <MdDriveFileMove />
              {labels.moveTo}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent className="max-h-72 min-w-44 overflow-y-auto">
              <ContextMenuItem onClick={() => onMove(node, null)}>{labels.root}</ContextMenuItem>
              {moveTargets.map(({ node: folder, depth }) => (
                <ContextMenuItem key={folder.id} onClick={() => onMove(node, folder.id)}>
                  <span style={{ paddingLeft: depth * 10 }}>{folder.name}</span>
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuItem variant="destructive" onClick={() => onDelete(node)}>
            <MdDelete />
            {labels.delete}
          </ContextMenuItem>
        </>
      ) : (
        <>
          <ContextMenuItem onClick={() => onCreateNote(parentId)}>
            <MdAdd />
            {labels.newNote}
          </ContextMenuItem>
          <ContextMenuItem onClick={() => onCreateFolder(parentId)}>
            <MdCreateNewFolder />
            {labels.newFolder}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onClick={onRefresh}>
            <MdRefresh />
            {labels.refresh}
          </ContextMenuItem>
        </>
      )}
    </ContextMenuContent>
  );
}

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { ContextMenu, ContextMenuTrigger } from "@/components/ui/context-menu";
import type { NoteTreeNode } from "@/types/notes";
import NoteTreeContextMenu, {
  type NoteTreeMenuLabels,
} from "./NoteTreeContextMenu";
import NoteTreeItem from "./NoteTreeItem";

const labels: NoteTreeMenuLabels = {
  open: "Open",
  newNote: "New note",
  newFolder: "New folder",
  rename: "Rename",
  moveTo: "Move to",
  delete: "Delete",
  refresh: "Refresh",
  root: "Root",
  expandAll: "Expand all",
  collapseAll: "Collapse all",
};

const baseNode = {
  parentId: null,
  sortOrder: 0,
  updatedAtMs: 1,
  children: [],
};

describe("NoteTreeContextMenu rename focus", () => {
  it.each([
    [
      "note",
      { ...baseNode, id: "note-1", kind: "note", name: "Note 1", revision: 1 },
    ],
    [
      "folder",
      { ...baseNode, id: "folder-1", kind: "folder", name: "Folder 1" },
    ],
  ] as const)(
    "keeps the %s inline editor focused after choosing Rename",
    async (_kind, node) => {
      render(<RenameHarness node={node} />);

      screen.getByRole("treeitem").focus();
      fireEvent.contextMenu(screen.getByTestId("notes-tree-trigger"));

      const renameItem = await screen.findByText(labels.rename);
      fireEvent.click(renameItem);

      const input = await screen.findByDisplayValue(node.name);
      await waitFor(() => {
        expect(
          document.querySelector('[data-slot="context-menu-content"]'),
        ).toBeNull();
        expect(document.activeElement).toBe(input);
      });
    },
  );
});

function RenameHarness({ node }: { node: NoteTreeNode }) {
  const [editing, setEditing] = useState(false);

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div data-testid="notes-tree-trigger">
          <NoteTreeItem
            node={node}
            depth={0}
            selected
            expanded={false}
            editing={editing}
            dragOver={false}
            labels={labels}
            onSelect={vi.fn()}
            onToggle={vi.fn()}
            onOpen={vi.fn()}
            onRenameSubmit={vi.fn()}
            onRenameCancel={() => setEditing(false)}
            onDragStartNode={vi.fn()}
            onDragOverNode={vi.fn()}
            onDropNode={vi.fn()}
            onDragEnd={vi.fn()}
          />
        </div>
      </ContextMenuTrigger>
      <NoteTreeContextMenu
        node={node}
        folderTargets={[]}
        labels={labels}
        onOpen={vi.fn()}
        onCreateNote={vi.fn()}
        onCreateFolder={vi.fn()}
        onRename={() => setEditing(true)}
        onMove={vi.fn()}
        onDelete={vi.fn()}
        onRefresh={vi.fn()}
      />
    </ContextMenu>
  );
}

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SavedConnection } from "@/types/global";
import ConnectionItem from "./ConnectionItem";
import { SavedConnectionsContext, type SavedConnectionsContextValue } from "./context";

vi.mock("@/components/ui/context-menu", () => ({
  ContextMenu: ({ children }: { children: ReactNode }) => <>{children}</>,
  ContextMenuTrigger: ({ children }: { children: ReactNode }) => <>{children}</>,
  ContextMenuContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ContextMenuItem: ({ children, onClick }: { children: ReactNode; onClick?: () => void }) => (
    <button type="button" onClick={onClick}>
      {children}
    </button>
  ),
  ContextMenuSeparator: () => null,
}));

vi.mock("@/components/ui/tooltip", () => ({
  Tooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
  TooltipTrigger: ({ children }: { children: ReactNode }) => <>{children}</>,
  TooltipContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
}));

vi.mock("./MoveToGroupMenu", () => ({ MoveToGroupContextMenu: () => null }));

const connection: SavedConnection = {
  id: "ssh-1",
  name: "Production",
  type: "ssh",
  host: "prod.example.com",
  port: 22,
  username: "root",
  tags: ["production", "gpu", "database", "critical"],
};

const handleConnectOnlyMock = vi.fn();
const onEditConnectionMock = vi.fn();

function createContextValue(conn = connection): SavedConnectionsContextValue {
  return {
    isDragEnabled: false,
    isPointerDragEnabled: false,
    dragTarget: null,
    expandedGroups: new Set(),
    selectedConnectionIds: new Set([conn.id]),
    keyboardActiveConnectionId: null,
    savedConnections: [conn],
    savedGroups: [],
    toggleGroup: vi.fn(),
    handleConnect: vi.fn(),
    handleConnectOnly: handleConnectOnlyMock,
    handleOpenSftp: vi.fn(),
    handleConnectSelected: vi.fn(),
    handleCopyConnection: vi.fn(),
    requestMoveConnectionToGroup: vi.fn(),
    requestMoveSelectedConnectionsToGroup: vi.fn(),
    handleConnectionSelectionStart: vi.fn(),
    handleConnectionContextMenu: vi.fn(),
    registerConnectionElement: vi.fn(),
    onEditConnection: onEditConnectionMock,
    onNewConnection: vi.fn(),
    requestDeleteConnection: vi.fn(),
    setRenamingConn: vi.fn(),
    setRenameValue: vi.fn(),
    setDeleteFolderTarget: vi.fn(),
    openNewFolderDialog: vi.fn(),
    openRenameFolderDialog: vi.fn(),
    requestOpenGroupConnections: vi.fn(),
    handleDragStart: vi.fn(),
    handleDragEnd: vi.fn(),
    handleDragEnterItem: vi.fn(),
    handleDragOverItem: vi.fn(),
    handleDragLeaveItem: vi.fn(),
    handleDropItem: vi.fn(),
    handlePointerDragStart: vi.fn(),
    handlePointerDragMove: vi.fn(),
    handlePointerDragEnd: vi.fn(),
    handlePointerDragCancel: vi.fn(),
    t: ((key: string) => key) as SavedConnectionsContextValue["t"],
  };
}

function renderConnectionItem(conn = connection) {
  return render(
    <SavedConnectionsContext.Provider value={createContextValue(conn)}>
      <ConnectionItem conn={conn} indented={false} />
    </SavedConnectionsContext.Provider>,
  );
}

function getConnectionTrigger(container: HTMLElement) {
  const trigger = container.querySelector<HTMLElement>(`[data-saved-drop-id="${connection.id}"]`);
  if (!trigger) throw new Error("Connection trigger not found");
  return trigger;
}

describe("ConnectionItem", () => {
  beforeEach(() => {
    handleConnectOnlyMock.mockReset();
    onEditConnectionMock.mockReset();
  });

  it("connects exactly once when the focused connection receives Enter", async () => {
    const user = userEvent.setup();
    const { container } = renderConnectionItem();
    const trigger = getConnectionTrigger(container);

    trigger.focus();
    await user.keyboard("{Enter}");

    expect(handleConnectOnlyMock).toHaveBeenCalledTimes(1);
    expect(handleConnectOnlyMock).toHaveBeenCalledWith(connection);
  });

  it("keeps nested action buttons independent from the parent Enter handler", async () => {
    const user = userEvent.setup();
    const { container } = renderConnectionItem();
    const trigger = getConnectionTrigger(container);
    const connectButton = screen.getAllByRole("button", {
      name: "savedConnections.connect",
    })[0];
    const editButton = screen.getAllByRole("button", {
      name: "savedConnections.edit",
    })[0];

    connectButton.focus();
    await user.keyboard("{Enter}");
    expect(handleConnectOnlyMock).toHaveBeenCalledTimes(1);

    handleConnectOnlyMock.mockClear();
    editButton.focus();
    await user.keyboard("{Enter}");

    expect(handleConnectOnlyMock).not.toHaveBeenCalled();
    expect(onEditConnectionMock).toHaveBeenCalledTimes(1);
    expect(onEditConnectionMock).toHaveBeenCalledWith(connection);
    expect(document.activeElement).toBe(trigger);
  });

  it("preserves double-click connect and focuses the item before context-menu edit", () => {
    const { container } = renderConnectionItem();
    const trigger = getConnectionTrigger(container);
    const row = trigger.firstElementChild as HTMLElement;

    fireEvent.doubleClick(row);
    expect(handleConnectOnlyMock).toHaveBeenCalledTimes(1);

    const editButtons = screen.getAllByRole("button", {
      name: "savedConnections.edit",
    });
    fireEvent.click(editButtons[1]);

    expect(onEditConnectionMock).toHaveBeenCalledTimes(1);
    expect(onEditConnectionMock).toHaveBeenCalledWith(connection);
    expect(document.activeElement).toBe(trigger);
  });

  it("shows two compact tags, an overflow count, and all tags in details", () => {
    const { container } = renderConnectionItem();

    expect(container.querySelector('[data-connection-tag="production"]')).not.toBeNull();
    expect(container.querySelector('[data-connection-tag="gpu"]')).not.toBeNull();
    expect(container.querySelector('[data-connection-tag="database"]')).toBeNull();
    expect(container.querySelector("[data-connection-tag-overflow]")?.textContent).toBe("+2");
    expect(screen.queryByText("savedConnections.tags")).not.toBeNull();
    expect(screen.queryByText("production · gpu · database · critical")).not.toBeNull();
  });

  it("renders normally without tags", () => {
    const withoutTags = { ...connection, id: "ssh-2", tags: undefined };
    const { container } = renderConnectionItem(withoutTags);

    expect(container.querySelector("[data-connection-tag]")).toBeNull();
    expect(container.querySelector("[data-connection-tag-overflow]")).toBeNull();
  });
});

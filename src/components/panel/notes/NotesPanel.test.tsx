import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import NotesPanel from "./NotesPanel";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  invoke: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  log: vi.fn(),
  refresh: vi.fn(),
  setSelectedNodeId: vi.fn(),
  setExpandedFolderIds: vi.fn(),
  runAction: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/logger", () => ({ logger: { error: mocks.log } }));
vi.mock("sonner", () => ({ toast: { success: mocks.success, error: mocks.error } }));
vi.mock("@/lib/windowManager", () => ({ openNoteEditor: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { count: number }) => (options ? `${key}: ${options.count}` : key),
  }),
}));
vi.mock("@/hooks/useNotesTree", () => ({
  useNotesTree: () => ({
    folders: [],
    notes: [],
    loading: false,
    error: null,
    selectedNodeId: null,
    expandedFolderIds: new Set(),
    refresh: mocks.refresh,
    setSelectedNodeId: mocks.setSelectedNodeId,
    setExpandedFolderIds: mocks.setExpandedFolderIds,
    runAction: mocks.runAction,
  }),
}));

async function chooseExport() {
  const user = userEvent.setup();
  render(
    <TooltipProvider>
      <NotesPanel />
    </TooltipProvider>,
  );
  await user.click(screen.getByRole("button", { name: "common.more" }));
  await user.click(await screen.findByRole("menuitem", { name: "notes.export" }));
}

describe("Notes panel export menu", () => {
  beforeEach(() => vi.resetAllMocks());

  it("exports all notes through one command and reports the count without changing tree state", async () => {
    mocks.open.mockResolvedValue("/Documents");
    mocks.invoke.mockResolvedValue({
      outputPath: "/Documents/NyaTerm Notes",
      folderCount: 1,
      noteCount: 3,
    });
    await chooseExport();
    await waitFor(() => expect(mocks.success).toHaveBeenCalledWith("notes.exportSuccess: 3"));
    expect(mocks.open).toHaveBeenCalledWith({ directory: true, multiple: false });
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("export_notes", {
      destination: "/Documents",
    });
    expect(mocks.refresh).not.toHaveBeenCalled();
    expect(mocks.setSelectedNodeId).not.toHaveBeenCalled();
    expect(mocks.setExpandedFolderIds).not.toHaveBeenCalled();
    expect(mocks.runAction).not.toHaveBeenCalled();
  });

  it("does nothing when directory selection is cancelled", async () => {
    mocks.open.mockResolvedValue(null);
    await chooseExport();
    expect(mocks.open).toHaveBeenCalledOnce();
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.error).not.toHaveBeenCalled();
  });

  it.each(["dialog", "command"])("reports and logs %s failures", async (source) => {
    const error = new Error("Access denied");
    mocks.open.mockResolvedValue("/Documents");
    if (source === "dialog") mocks.open.mockRejectedValue(error);
    else mocks.invoke.mockRejectedValue(error);
    await chooseExport();
    await waitFor(() => expect(mocks.error).toHaveBeenCalledWith("Access denied"));
    expect(mocks.log).toHaveBeenCalledWith(
      expect.objectContaining({ event: "notes.export_failed", error }),
    );
    expect(mocks.success).not.toHaveBeenCalled();
    expect(mocks.runAction).not.toHaveBeenCalled();
  });
});

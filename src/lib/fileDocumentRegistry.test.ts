import { describe, expect, it, vi } from "vitest";
import {
  discardFileDocuments,
  getDirtyFileDocumentIds,
  getFileDocumentController,
  registerFileDocument,
  saveFileDocuments,
  updateFileDocumentState,
} from "./fileDocumentRegistry";

describe("fileDocumentRegistry", () => {
  it("exposes dirty document state and its close controller to the workspace", async () => {
    const save = vi.fn().mockResolvedValue("saved" as const);
    const discard = vi.fn();
    const unregister = registerFileDocument("pane-file", { save, discard });

    try {
      updateFileDocumentState("pane-file", { dirty: true, saving: false });

      expect(getDirtyFileDocumentIds()).toContain("pane-file");
      expect(await getFileDocumentController("pane-file")?.save()).toBe("saved");
      getFileDocumentController("pane-file")?.discard();
      expect(discard).toHaveBeenCalledTimes(1);
    } finally {
      unregister();
    }

    expect(getDirtyFileDocumentIds()).not.toContain("pane-file");
    expect(getFileDocumentController("pane-file")).toBeNull();
  });

  it("exposes the current editor snapshot for AI file references", () => {
    const snapshot = {
      content: "latest unsaved text",
      size: 19,
      mtime: 42,
      contentHash: "snapshot-hash",
    };
    const unregister = registerFileDocument("pane-file", {
      save: vi.fn().mockResolvedValue("saved" as const),
      discard: vi.fn(),
      getSnapshot: () => snapshot,
    });

    try {
      expect(getFileDocumentController("pane-file")?.getSnapshot?.()).toEqual(snapshot);
    } finally {
      unregister();
    }
  });

  it("stops a multi-document close when any save is blocked", async () => {
    const firstSave = vi.fn().mockResolvedValue("saved" as const);
    const blockedSave = vi.fn().mockResolvedValue("conflict" as const);
    const neverReachedSave = vi.fn().mockResolvedValue("saved" as const);
    const unregister = [
      registerFileDocument("first", { save: firstSave, discard: vi.fn() }),
      registerFileDocument("blocked", { save: blockedSave, discard: vi.fn() }),
      registerFileDocument("last", {
        save: neverReachedSave,
        discard: vi.fn(),
      }),
    ];

    try {
      expect(await saveFileDocuments(["first", "blocked", "last"])).toBe(false);
      expect(firstSave).toHaveBeenCalledWith(false);
      expect(blockedSave).toHaveBeenCalledWith(false);
      expect(neverReachedSave).not.toHaveBeenCalled();
    } finally {
      unregister.forEach((dispose) => {
        dispose();
      });
    }
  });

  it("discards only the documents included in the close request", () => {
    const firstDiscard = vi.fn();
    const secondDiscard = vi.fn();
    const unregister = [
      registerFileDocument("first", { save: vi.fn(), discard: firstDiscard }),
      registerFileDocument("second", { save: vi.fn(), discard: secondDiscard }),
    ];

    try {
      discardFileDocuments(["second"]);
      expect(firstDiscard).not.toHaveBeenCalled();
      expect(secondDiscard).toHaveBeenCalledTimes(1);
    } finally {
      unregister.forEach((dispose) => {
        dispose();
      });
    }
  });
});

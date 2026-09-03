import { act, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getFileDocumentController } from "@/lib/fileDocumentRegistry";
import { invoke } from "@/lib/invoke";
import type { FileDocumentPane } from "@/types/global";
import FileDocumentEditor from "./FileDocumentEditor";

vi.mock("@codemirror/state", () => ({
  EditorState: {
    create: ({ doc }: { doc: string }) => ({ doc: { length: doc.length } }),
  },
}));

vi.mock("@codemirror/view", () => ({
  EditorView: class {
    static updateListener = { of: (listener: unknown) => listener };
    state: { doc: { length: number } };

    constructor({ state }: { state: { doc: { length: number } } }) {
      this.state = state;
    }

    dispatch({ changes }: { changes: { insert: string } }) {
      this.state = { doc: { length: changes.insert.length } };
    }

    destroy() {}

    focus() {}
  },
}));

vi.mock("@/lib/codeMirrorFileView", () => ({
  codeMirrorFileViewExtensions: () => [],
}));

vi.mock("@/lib/invoke", () => ({ invoke: vi.fn() }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    appSettings: {
      transfer: {
        internal_editor_font_size: 13,
      },
    },
  }),
}));

function pane(): FileDocumentPane {
  return {
    id: "pane-file",
    kind: "leaf",
    paneKind: "file",
    sessionId: "session-1",
    name: "notes.md",
    type: "SSH",
    connectionId: "connection-1",
    file: {
      backend: "remote",
      path: "/tmp/notes.md",
      initial: {
        content: "hello",
        size: 5,
        mtime: 10,
        contentHash: "hash-open",
      },
    },
  };
}

describe("FileDocumentEditor save baseline", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("sends the opened content hash and updates the baseline after save", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        status: "saved",
        mtime: 11,
        size: 5,
        contentHash: "hash-saved",
      })
      .mockResolvedValueOnce({
        status: "saved",
        mtime: 12,
        size: 5,
        contentHash: "hash-saved-again",
      });

    render(<FileDocumentEditor pane={pane()} active />);

    await waitFor(() => expect(getFileDocumentController("pane-file")).not.toBeNull());

    await act(async () => {
      await getFileDocumentController("pane-file")?.save();
    });

    expect(invoke).toHaveBeenLastCalledWith("write_remote_file_text", {
      sessionId: "session-1",
      path: "/tmp/notes.md",
      content: "hello",
      expectedMtime: 10,
      expectedSize: 5,
      expectedMtimeNanos: undefined,
      expectedHash: "hash-open",
      force: false,
    });

    await act(async () => {
      await getFileDocumentController("pane-file")?.save();
    });

    expect(invoke).toHaveBeenLastCalledWith("write_remote_file_text", {
      sessionId: "session-1",
      path: "/tmp/notes.md",
      content: "hello",
      expectedMtime: 11,
      expectedSize: 5,
      expectedMtimeNanos: undefined,
      expectedHash: "hash-saved",
      force: false,
    });
  });
});

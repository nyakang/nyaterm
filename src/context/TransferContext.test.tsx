import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TransferProvider, useTransfer } from "./TransferContext";

interface TransferEventPayload {
  id: string;
  session_id: string;
  file_name: string;
  remote_path: string;
  local_path: string;
  direction: string;
  kind?: string;
  status: string;
  size: number;
  bytes_transferred: number;
  total_size: number;
  error_msg?: string;
  source?: "sftp" | "rdp" | "zmodem" | "serial_modem";
}

const mocks = vi.hoisted(() => ({
  listener: undefined as ((event: { payload: TransferEventPayload }) => void) | undefined,
  invoke: vi.fn().mockResolvedValue(undefined),
  translate: vi.fn((key: string, options?: { name?: string }) =>
    options?.name ? `${key}:${options.name}` : key,
  ),
  toastDismiss: vi.fn(),
  toastError: vi.fn(),
  toastMessage: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (_event: string, listener: (event: { payload: TransferEventPayload }) => void) => {
      mocks.listener = listener;
      return () => {
        mocks.listener = undefined;
      };
    },
  ),
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));

vi.mock("@/context/AppContext", () => ({
  useApp: () => ({
    appSettings: {
      transfer: {
        download_threads: 2,
        duplicate_strategy: "overwrite",
        upload_threads: 2,
      },
    },
  }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: mocks.translate,
  }),
}));

vi.mock("sonner", () => ({
  toast: {
    dismiss: mocks.toastDismiss,
    error: mocks.toastError,
    message: mocks.toastMessage,
    success: mocks.toastSuccess,
    warning: mocks.toastWarning,
  },
}));

const baseEvent: TransferEventPayload = {
  id: "transfer-1",
  session_id: "session-1",
  file_name: "folder",
  remote_path: "/remote/folder",
  local_path: "/local/folder",
  direction: "upload",
  kind: "directory",
  status: "completed",
  size: 10,
  bytes_transferred: 10,
  total_size: 10,
};

let transferContext: ReturnType<typeof useTransfer> | null = null;

function TransferProbe() {
  transferContext = useTransfer();
  return null;
}

async function emitTransferEvent(payload: TransferEventPayload) {
  await waitFor(() => expect(mocks.listener).toBeDefined());
  act(() => {
    mocks.listener?.({ payload });
  });
}

describe("TransferProvider transfer completion toasts", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.listener = undefined;
    transferContext = null;
    render(
      <TransferProvider>
        <TransferProbe />
      </TransferProvider>,
    );
  });

  afterEach(() => {
    cleanup();
  });

  it("warns when a directory upload completes with skipped files", async () => {
    await emitTransferEvent({
      ...baseEvent,
      error_msg: "Skipped 1 failed file(s); first failure: /remote/folder/locked.txt",
    });

    expect(mocks.toastWarning).toHaveBeenCalledWith("fileTransfer.uploadFolderCompleted", {
      description: "Skipped 1 failed file(s); first failure: /remote/folder/locked.txt",
    });
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });

  it("keeps the success toast for a fully successful directory upload", async () => {
    await emitTransferEvent(baseEvent);

    expect(mocks.toastSuccess).toHaveBeenCalledWith("fileTransfer.uploadFolderCompleted", {
      description: "/remote/folder",
    });
    expect(mocks.toastWarning).not.toHaveBeenCalled();
  });

  it("keeps the failure toast for an upload error", async () => {
    await emitTransferEvent({
      ...baseEvent,
      error_msg: "permission denied",
      status: "error",
    });

    expect(mocks.toastError).toHaveBeenCalledWith("fileTransfer.uploadFolderFailed:folder", {
      description: "permission denied",
    });
    expect(mocks.toastWarning).not.toHaveBeenCalled();
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });

  it.each([
    "file",
    "directory",
  ])("removes a skipped %s download without a completion toast or changing earlier history", async (kind) => {
    const downloadEvent = { ...baseEvent, direction: "download", kind };
    await emitTransferEvent({ ...downloadEvent, id: "earlier", status: "started" });
    await emitTransferEvent({ ...downloadEvent, id: "earlier" });
    const earlier = transferContext?.transfers.find((item) => item.id === "earlier");
    expect(earlier?.status).toBe("completed");
    mocks.toastSuccess.mockClear();

    let id = "";
    act(() => {
      [id] = transferContext!.enqueueDownloads([
        {
          sessionId: baseEvent.session_id,
          fileName: baseEvent.file_name,
          remotePath: baseEvent.remote_path,
          localPath: baseEvent.local_path,
          kind: kind as "file" | "directory",
        },
      ]);
    });
    await waitFor(() =>
      expect(transferContext?.transfers.find((item) => item.id === id)?.status).toBe(
        "transferring",
      ),
    );
    await emitTransferEvent({
      ...downloadEvent,
      id,
      status: "skipped",
      size: 0,
      bytes_transferred: 0,
      total_size: 0,
    });

    await waitFor(() => expect(transferContext?.transfers).toEqual([earlier]));
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
    expect(mocks.toastWarning).not.toHaveBeenCalled();
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  it("starts the next queued download after a skipped download", async () => {
    let ids: string[] = [];
    act(() => {
      ids = transferContext!.enqueueDownloads(
        ["first.log", "second.log"].map((fileName) => ({
          sessionId: "session-1",
          fileName,
          remotePath: `/remote/${fileName}`,
          localPath: `/local/${fileName}`,
          kind: "file" as const,
        })),
      );
    });
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1));
    expect(transferContext?.transfers.find((item) => item.id === ids[1])?.status).toBe("queued");
    await emitTransferEvent({
      ...baseEvent,
      id: ids[0],
      file_name: "first.log",
      direction: "download",
      kind: "file",
      status: "skipped",
      size: 0,
      bytes_transferred: 0,
      total_size: 0,
    });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("download_remote_file", {
        sessionId: "session-1",
        remotePath: "/remote/second.log",
        localPath: "/local/second.log",
        transferId: ids[1],
      }),
    );
    expect(transferContext?.transfers.some((item) => item.id === ids[0])).toBe(false);
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });

  it("ignores skipped downloads without a queued record", async () => {
    await emitTransferEvent({ ...baseEvent, direction: "download", status: "skipped" });
    expect(transferContext?.transfers).toEqual([]);
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });

  it("still reports a genuinely completed empty download", async () => {
    const event = {
      ...baseEvent,
      direction: "download",
      kind: "file",
      size: 0,
      bytes_transferred: 0,
      total_size: 0,
    };
    await emitTransferEvent({ ...event, status: "started" });
    await emitTransferEvent(event);
    expect(transferContext?.transfers.find((item) => item.id === event.id)?.status).toBe(
      "completed",
    );
    expect(mocks.toastSuccess).toHaveBeenCalledWith("fileTransfer.downloadCompleted", {
      description: event.local_path,
    });
  });

  it.each([
    "file",
    "directory",
  ] as const)("uses the final local path for a completed %s download without changing source information", async (kind) => {
    const event = {
      ...baseEvent,
      direction: "download",
      kind,
      file_name: "logs.2026",
      remote_path: "/remote/logs.2026",
      local_path: "C:\\Downloads\\logs.2026(1)",
    };
    await emitTransferEvent({ ...event, status: "started" });
    for (const status of ["progress", "paused", "resumed"]) {
      await emitTransferEvent({ ...event, status });
    }
    expect(transferContext?.transfers[0].localPath).toBe(event.local_path);
    const finalPath = "C:\\Downloads\\logs.2026(2)";
    await emitTransferEvent({ ...event, local_path: finalPath });
    expect(transferContext?.transfers[0]).toMatchObject({
      status: "completed",
      fileName: "logs.2026",
      remotePath: "/remote/logs.2026",
      localPath: finalPath,
    });
    expect(mocks.toastSuccess).toHaveBeenCalledWith(
      kind === "directory"
        ? "fileTransfer.downloadFolderCompleted"
        : "fileTransfer.downloadCompleted",
      { description: finalPath },
    );
  });

  it("keeps the resolved download path if a completed event has no local path", async () => {
    const event = { ...baseEvent, direction: "download", local_path: "/local/folder(1)" };
    await emitTransferEvent({ ...event, status: "started" });
    await emitTransferEvent({ ...event, local_path: "" });
    expect(transferContext?.transfers[0].localPath).toBe("/local/folder(1)");
  });

  it("retries a renamed download with the original remote file name and path", async () => {
    const event = {
      ...baseEvent,
      direction: "download",
      kind: "file",
      file_name: "app.log",
      remote_path: "/remote/app.log",
      local_path: "/local/app(1).log",
    };
    await emitTransferEvent({ ...event, status: "started" });
    await emitTransferEvent({ ...event, status: "error", error_msg: "connection lost" });
    const item = transferContext!.transfers[0];
    await act(async () => {
      await transferContext!.retryTransfer(item);
    });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("download_remote_file", {
        sessionId: event.session_id,
        remotePath: "/remote/app.log",
        localPath: "/local/app(1).log",
        transferId: event.id,
      }),
    );
    expect(transferContext?.transfers[0].fileName).toBe("app.log");
  });

  it("preserves the Serial modem source for backend-driven progress", async () => {
    await waitFor(() => expect(transferContext).not.toBeNull());

    act(() => {
      transferContext?.upsertExternalTransferProgress({
        id: "serial-modem-1",
        sessionId: "serial-1",
        fileName: "firmware.bin",
        direction: "upload",
        bytesTransferred: 128,
        totalSize: 1024,
        source: "serial_modem",
      });
    });

    await waitFor(() => {
      expect(transferContext?.transfers).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            id: "serial-modem-1",
            source: "serial_modem",
            sessionId: "serial-1",
          }),
        ]),
      );
    });
  });

  it("maps RDP backend events and reports files ready to paste", async () => {
    const event = {
      ...baseEvent,
      direction: "download",
      local_path: "C:/cache/rdp-files",
      source: "rdp",
    } satisfies TransferEventPayload;

    await emitTransferEvent({ ...event, status: "started" });
    await emitTransferEvent(event);

    await waitFor(() => {
      expect(transferContext?.transfers).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ id: "transfer-1", source: "rdp", status: "completed" }),
        ]),
      );
    });
    expect(mocks.toastSuccess).toHaveBeenCalledWith("fileTransfer.readyToPaste", {
      description: "C:/cache/rdp-files",
    });
  });
});

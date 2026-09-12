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
}

const mocks = vi.hoisted(() => ({
  listener: undefined as
    | ((event: { payload: TransferEventPayload }) => void)
    | undefined,
  invoke: vi.fn().mockResolvedValue(undefined),
  toastDismiss: vi.fn(),
  toastError: vi.fn(),
  toastMessage: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (
      _event: string,
      listener: (event: { payload: TransferEventPayload }) => void,
    ) => {
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
    t: (key: string, options?: { name?: string }) =>
      options?.name ? `${key}:${options.name}` : key,
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
      error_msg:
        "Skipped 1 failed file(s); first failure: /remote/folder/locked.txt",
    });

    expect(mocks.toastWarning).toHaveBeenCalledWith(
      "fileTransfer.uploadFolderCompleted",
      {
        description:
          "Skipped 1 failed file(s); first failure: /remote/folder/locked.txt",
      },
    );
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
  });

  it("keeps the success toast for a fully successful directory upload", async () => {
    await emitTransferEvent(baseEvent);

    expect(mocks.toastSuccess).toHaveBeenCalledWith(
      "fileTransfer.uploadFolderCompleted",
      {
        description: "/remote/folder",
      },
    );
    expect(mocks.toastWarning).not.toHaveBeenCalled();
  });

  it("keeps the failure toast for an upload error", async () => {
    await emitTransferEvent({
      ...baseEvent,
      error_msg: "permission denied",
      status: "error",
    });

    expect(mocks.toastError).toHaveBeenCalledWith(
      "fileTransfer.uploadFolderFailed:folder",
      {
        description: "permission denied",
      },
    );
    expect(mocks.toastWarning).not.toHaveBeenCalled();
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
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
});

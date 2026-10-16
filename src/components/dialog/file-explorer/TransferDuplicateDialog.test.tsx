import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  getActiveTransferDuplicatePrompt,
  removeBackendTransferDuplicatePrompt,
  setBackendTransferDuplicatePrompt,
  type TransferDuplicateRequest,
} from "@/lib/transferDuplicatePrompt";
import { TransferDuplicateDialog } from "./TransferDuplicateDialog";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/logger", () => ({ logger: { error: vi.fn() } }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function request(requestId = "first", local = true): TransferDuplicateRequest {
  return {
    requestId,
    sessionId: "ssh-1",
    remotePath: "C:/Downloads/app.log",
    fileName: "app.log",
    isDirectory: false,
    destinationKind: local ? "local" : "remote",
  };
}

beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  while (getActiveTransferDuplicatePrompt()) setBackendTransferDuplicatePrompt(null);
});

describe("TransferDuplicateDialog", () => {
  it("overrides the small-dialog two-column footer for three local conflict buttons", () => {
    setBackendTransferDuplicatePrompt(request());
    render(<TransferDuplicateDialog />);
    const footer = screen
      .getByRole("button", { name: "fileTransfer.duplicateRename" })
      .closest("[data-slot='alert-dialog-footer']");
    expect(
      footer?.classList.contains("group-data-[size=sm]/alert-dialog-content:grid-cols-3"),
    ).toBe(true);
    expect(
      footer?.classList.contains("group-data-[size=sm]/alert-dialog-content:grid-cols-2"),
    ).toBe(false);
  });

  it("shows local conflict text and sends keep-both to the backend", async () => {
    setBackendTransferDuplicatePrompt(request());
    render(<TransferDuplicateDialog />);
    expect(screen.getByText("fileTransfer.duplicateLocalDescription")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "fileTransfer.duplicateRename" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
        requestId: "first",
        action: "rename",
      }),
    );
    await waitFor(() => expect(getActiveTransferDuplicatePrompt()).toBeNull());
  });

  it.each([
    "skip",
    "overwrite",
  ])("preserves the explicit %s choice for a local conflict", async (action) => {
    setBackendTransferDuplicatePrompt(request());
    render(<TransferDuplicateDialog />);
    const key =
      action === "skip" ? "fileTransfer.duplicateSkip" : "fileTransfer.duplicateOverwrite";
    fireEvent.click(screen.getByRole("button", { name: key }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
        requestId: "first",
        action,
      }),
    );
    await waitFor(() => expect(getActiveTransferDuplicatePrompt()).toBeNull());
  });

  it("keeps remote prompts unchanged without a keep-both button", () => {
    setBackendTransferDuplicatePrompt(request("remote", false));
    render(<TransferDuplicateDialog />);
    expect(screen.getByText("fileTransfer.duplicateDescription")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "fileTransfer.duplicateRename" })).toBeNull();
  });

  it("does not override Enter on a focused overwrite button with rename", () => {
    setBackendTransferDuplicatePrompt(request());
    render(<TransferDuplicateDialog />);
    fireEvent.keyDown(screen.getByRole("button", { name: "fileTransfer.duplicateOverwrite" }), {
      key: "Enter",
    });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("starts the answer timer only for the currently displayed backend prompt", async () => {
    setBackendTransferDuplicatePrompt({ ...request(), requiresActivation: true });
    setBackendTransferDuplicatePrompt({ ...request("second"), requiresActivation: true });
    render(<TransferDuplicateDialog />);
    expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
      requestId: "first",
      action: "shown",
    });
    expect(mocks.invoke).not.toHaveBeenCalledWith("respond_transfer_duplicate", {
      requestId: "second",
      action: "shown",
    });
    act(() => removeBackendTransferDuplicatePrompt("first"));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
        requestId: "second",
        action: "shown",
      }),
    );
  });

  it.each([
    "resolved",
    "rejected",
  ])("does not dismiss the next prompt after a late %s response", async (outcome) => {
    let finish: (() => void) | undefined;
    mocks.invoke.mockReturnValueOnce(
      new Promise<void>((resolve, reject) => {
        finish = outcome === "resolved" ? resolve : () => reject(new Error("Prompt expired"));
      }),
    );
    setBackendTransferDuplicatePrompt(request());
    setBackendTransferDuplicatePrompt(request("second"));
    render(<TransferDuplicateDialog />);
    fireEvent.click(screen.getByRole("button", { name: "fileTransfer.duplicateRename" }));
    act(() => removeBackendTransferDuplicatePrompt("first"));
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("second");
    await act(async () => {
      finish?.();
    });
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("second");
    expect(mocks.invoke).not.toHaveBeenCalledWith("respond_transfer_duplicate", {
      requestId: "second",
      action: "skip",
    });
  });

  it("safely skips an activation failure without discarding the next request", async () => {
    mocks.invoke.mockRejectedValueOnce(new Error("Activation failed"));
    setBackendTransferDuplicatePrompt({ ...request(), requiresActivation: true });
    setBackendTransferDuplicatePrompt(request("second"));
    render(<TransferDuplicateDialog />);
    await waitFor(() => expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("second"));
    expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
      requestId: "first",
      action: "skip",
    });
  });

  it("displays the next queued conflict after answering the first", async () => {
    setBackendTransferDuplicatePrompt(request());
    render(<TransferDuplicateDialog />);
    act(() => setBackendTransferDuplicatePrompt(request("second")));
    fireEvent.click(screen.getByRole("button", { name: "fileTransfer.duplicateRename" }));
    await waitFor(() => expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("second"));
    fireEvent.click(screen.getByRole("button", { name: "fileTransfer.duplicateSkip" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("respond_transfer_duplicate", {
        requestId: "second",
        action: "skip",
      }),
    );
    await waitFor(() => expect(getActiveTransferDuplicatePrompt()).toBeNull());
  });
});

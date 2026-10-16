import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getActiveTransferDuplicatePrompt,
  removeBackendTransferDuplicatePrompt,
  resolveTransferDuplicatePrompt,
  setBackendTransferDuplicatePrompt,
  showTransferDuplicatePrompt,
  subscribeTransferDuplicatePrompt,
  type TransferDuplicateRequest,
} from "./transferDuplicatePrompt";

function request(requestId: string): TransferDuplicateRequest {
  return {
    requestId,
    sessionId: "ssh-1",
    remotePath: "C:/Downloads/app.log",
    fileName: "app.log",
    isDirectory: false,
    destinationKind: "local",
  };
}

afterEach(() => {
  while (getActiveTransferDuplicatePrompt()) {
    if (getActiveTransferDuplicatePrompt()?.respondViaBackend) {
      setBackendTransferDuplicatePrompt(null);
    } else {
      resolveTransferDuplicatePrompt("skip");
    }
  }
});

describe("transfer duplicate prompt queue", () => {
  it("queues simultaneous backend prompts instead of replacing the active one", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeTransferDuplicatePrompt(listener);
    setBackendTransferDuplicatePrompt(request("first"));
    setBackendTransferDuplicatePrompt(request("second"));
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("first");
    expect(listener).toHaveBeenCalledTimes(2);
    setBackendTransferDuplicatePrompt(null);
    expect(getActiveTransferDuplicatePrompt()).toEqual({
      ...request("second"),
      respondViaBackend: true,
      allowApplyToTask: false,
    });
    setBackendTransferDuplicatePrompt(null);
    expect(getActiveTransferDuplicatePrompt()).toBeNull();
    unsubscribe();
  });

  it("removes an expired queued request without touching the active request", () => {
    setBackendTransferDuplicatePrompt(request("first"));
    setBackendTransferDuplicatePrompt(request("expired"));
    setBackendTransferDuplicatePrompt(request("third"));
    removeBackendTransferDuplicatePrompt("expired");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("first");
    removeBackendTransferDuplicatePrompt("first");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("third");
    removeBackendTransferDuplicatePrompt("first");
    removeBackendTransferDuplicatePrompt("unknown");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("third");
  });

  it("advances after timeout or cancellation without losing a local resolver", async () => {
    setBackendTransferDuplicatePrompt(request("ended"));
    const local = showTransferDuplicatePrompt(request("local"));
    removeBackendTransferDuplicatePrompt("ended");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("local");
    removeBackendTransferDuplicatePrompt("local");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("local");
    resolveTransferDuplicatePrompt("skip");
    await expect(local).resolves.toBe("skip");
  });

  it("deduplicates backend request IDs", () => {
    setBackendTransferDuplicatePrompt(request("first"));
    setBackendTransferDuplicatePrompt(request("first"));
    setBackendTransferDuplicatePrompt(null);
    expect(getActiveTransferDuplicatePrompt()).toBeNull();
  });

  it("keeps local resolvers associated with their own queued prompts", async () => {
    const first = showTransferDuplicatePrompt(request("first"));
    const second = showTransferDuplicatePrompt(request("second"));
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("first");
    resolveTransferDuplicatePrompt("rename");
    await expect(first).resolves.toBe("rename");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("second");
    resolveTransferDuplicatePrompt("skip");
    await expect(second).resolves.toBe("skip");
    expect(getActiveTransferDuplicatePrompt()).toBeNull();
  });

  it("queues backend and local prompts without clearing the wrong request", async () => {
    const local = showTransferDuplicatePrompt(request("local"));
    setBackendTransferDuplicatePrompt(request("backend"));
    setBackendTransferDuplicatePrompt(null);
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("local");
    resolveTransferDuplicatePrompt("overwriteAllForTask");
    await expect(local).resolves.toBe("overwriteAllForTask");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("backend");
    resolveTransferDuplicatePrompt("skip");
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("backend");
    setBackendTransferDuplicatePrompt(null);
  });

  it("keeps a local prompt queued behind a backend prompt", async () => {
    setBackendTransferDuplicatePrompt(request("backend"));
    const local = showTransferDuplicatePrompt(request("local"));
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("backend");
    setBackendTransferDuplicatePrompt(null);
    expect(getActiveTransferDuplicatePrompt()?.requestId).toBe("local");
    resolveTransferDuplicatePrompt("skip");
    await expect(local).resolves.toBe("skip");
  });
});

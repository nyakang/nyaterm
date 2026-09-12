import { beforeEach, describe, expect, it, vi } from "vitest";
import { createSerialModemEventHandler } from "./serialModemTerminalEvents";

const toastError = vi.hoisted(() => vi.fn());

vi.mock("sonner", () => ({ toast: { error: toastError } }));

describe("createSerialModemEventHandler", () => {
  beforeEach(() => vi.clearAllMocks());

  it("publishes Serial modem progress through the external transfer sink", () => {
    const sink = {
      upsertProgress: vi.fn(),
      complete: vi.fn(),
      fail: vi.fn(),
    };
    const handler = createSerialModemEventHandler("serial-1", () => (key) => key, sink);

    handler.handle({
      type: "progress",
      protocol: "ymodem",
      fileName: "firmware.bin",
      fileIndex: 0,
      bytesTransferred: 128,
      totalSize: 1024,
    });

    expect(sink.upsertProgress).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: "serial-1",
        fileName: "firmware.bin",
        direction: "upload",
        bytesTransferred: 128,
        totalSize: 1024,
        source: "serial_modem",
      }),
    );

    handler.handle({ type: "complete", protocol: "ymodem", fileCount: 1 });
    expect(sink.complete).toHaveBeenCalledTimes(1);
  });

  it("marks active modem transfers failed and surfaces the backend reason", () => {
    const sink = {
      upsertProgress: vi.fn(),
      complete: vi.fn(),
      fail: vi.fn(),
    };
    const handler = createSerialModemEventHandler("serial-1", () => (key) => key, sink);
    handler.handle({
      type: "progress",
      protocol: "xmodem",
      fileName: "firmware.bin",
      fileIndex: 0,
      bytesTransferred: 0,
      totalSize: 1024,
    });
    handler.handle({
      type: "failed",
      protocol: "xmodem",
      reason: "Timed out waiting for modem receiver handshake",
      fileIndex: 0,
      fileName: "firmware.bin",
    });

    expect(sink.fail).toHaveBeenCalledWith(
      expect.stringContaining("serial-modem-serial-1"),
      "Timed out waiting for modem receiver handshake",
    );
    expect(toastError).toHaveBeenCalledWith("terminal.serialModemUploadFailed", {
      description: "Timed out waiting for modem receiver handshake",
    });
  });

  it("keeps an earlier YMODEM file completed when the next file fails", () => {
    const sink = {
      upsertProgress: vi.fn(),
      complete: vi.fn(),
      fail: vi.fn(),
    };
    const handler = createSerialModemEventHandler("serial-1", () => (key) => key, sink);

    handler.handle({
      type: "progress",
      protocol: "ymodem",
      fileName: "first.bin",
      fileIndex: 0,
      bytesTransferred: 3,
      totalSize: 3,
    });
    const firstId = sink.upsertProgress.mock.calls[0][0].id;
    handler.handle({
      type: "fileComplete",
      protocol: "ymodem",
      fileName: "first.bin",
      fileIndex: 0,
    });

    handler.handle({
      type: "progress",
      protocol: "ymodem",
      fileName: "second.bin",
      fileIndex: 1,
      bytesTransferred: 0,
      totalSize: 4,
    });
    const secondId = sink.upsertProgress.mock.calls[1][0].id;
    handler.handle({
      type: "failed",
      protocol: "ymodem",
      reason: "Receiver cancelled the modem transfer",
      fileName: "second.bin",
      fileIndex: 1,
    });

    expect(sink.complete).toHaveBeenCalledWith(firstId);
    expect(sink.fail).toHaveBeenCalledTimes(1);
    expect(sink.fail).toHaveBeenCalledWith(secondId, "Receiver cancelled the modem transfer");
    expect(sink.fail).not.toHaveBeenCalledWith(firstId, expect.anything());
  });

  it("records a panel failure when final YMODEM batch termination fails", () => {
    const sink = {
      upsertProgress: vi.fn(),
      complete: vi.fn(),
      fail: vi.fn(),
    };
    const handler = createSerialModemEventHandler("serial-1", () => (key) => key, sink);

    handler.handle({
      type: "progress",
      protocol: "ymodem",
      fileName: "last.bin",
      fileIndex: 0,
      bytesTransferred: 4,
      totalSize: 4,
    });
    const id = sink.upsertProgress.mock.calls[0][0].id;
    handler.handle({
      type: "fileComplete",
      protocol: "ymodem",
      fileName: "last.bin",
      fileIndex: 0,
    });
    handler.handle({
      type: "failed",
      protocol: "ymodem",
      reason: "Modem receiver did not acknowledge the transfer",
      fileIndex: 0,
      fileName: undefined,
    });

    expect(sink.complete).toHaveBeenCalledWith(id);
    expect(sink.fail).toHaveBeenCalledWith(
      id,
      "Modem receiver did not acknowledge the transfer",
    );
  });
});

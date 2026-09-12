import { beforeEach, describe, expect, it, vi } from "vitest";
import { handleTerminalFileDrop } from "./terminalFileDrop";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  sendSessionInput: vi.fn(),
  uploadFilesViaZmodem: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/sessionInput", () => ({ sendSessionInput: mocks.sendSessionInput }));
vi.mock("@/lib/terminalZmodemUpload", () => ({
  isZmodemUploadErrorHandled: () => false,
  uploadFilesViaZmodem: mocks.uploadFilesViaZmodem,
}));
vi.mock("sonner", () => ({
  toast: {
    error: mocks.toastError,
    message: vi.fn(),
  },
}));

const t = (key: string) => key;

describe("handleTerminalFileDrop", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.invoke.mockResolvedValue(undefined);
    mocks.uploadFilesViaZmodem.mockResolvedValue(undefined);
    mocks.sendSessionInput.mockResolvedValue(undefined);
  });

  it("routes Serial file drops to direct modem upload without injecting shell text", async () => {
    await handleTerminalFileDrop({
      sessionId: "serial-1",
      sessionType: "Serial",
      entries: [
        { path: "C:\\firmware\\one.bin", isDir: false },
        { path: "C:\\firmware\\two.bin", isDir: false },
      ],
      t,
    });

    expect(mocks.invoke).toHaveBeenCalledWith("serial_modem_upload", {
      sessionId: "serial-1",
      filePaths: ["C:\\firmware\\one.bin", "C:\\firmware\\two.bin"],
    });
    expect(mocks.uploadFilesViaZmodem).not.toHaveBeenCalled();
    expect(mocks.sendSessionInput).not.toHaveBeenCalled();
  });

  it("keeps SSH drops on the existing ZMODEM path", async () => {
    await handleTerminalFileDrop({
      sessionId: "ssh-1",
      sessionType: "SSH",
      entries: [{ path: "/tmp/fw.bin", isDir: false }],
      t,
      duplicateStrategy: "overwrite",
    });

    expect(mocks.uploadFilesViaZmodem).toHaveBeenCalledWith(
      "ssh-1",
      ["/tmp/fw.bin"],
      "overwrite",
    );
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("rejects Serial directories before starting a modem upload", async () => {
    await handleTerminalFileDrop({
      sessionId: "serial-1",
      sessionType: "Serial",
      entries: [{ path: "C:\\firmware", isDir: true }],
      t,
    });

    expect(mocks.toastError).toHaveBeenCalledWith("terminal.dropFoldersSerialModemOnly");
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});

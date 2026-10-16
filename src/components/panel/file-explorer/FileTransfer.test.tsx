import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TransferItem } from "@/context/TransferContext";
import FileTransfer from "./FileTransfer";

const mocks = vi.hoisted(() => ({
  transfers: [] as TransferItem[],
  nativeFiles: true,
  translate: (key: string, options?: { name?: string; path?: string }) => {
    const value = options?.name ?? options?.path;
    return value ? `${key}:${value}` : key;
  },
}));

vi.mock("@/context/TransferContext", () => ({
  useTransfer: () => ({
    transfers: mocks.transfers,
    clearCompleted: vi.fn(),
    removeTransfer: vi.fn(),
    pauseTransfer: vi.fn(),
    resumeTransfer: vi.fn(),
    cancelTransfer: vi.fn(),
    retryTransfer: vi.fn(),
  }),
}));
vi.mock("@/context/AppContext", () => ({
  useApp: () => ({ appSettings: { transfer: { download_path: "" } } }),
}));
vi.mock("@/lib/backend/runtime", () => ({
  runtime: "desktop",
  supports: (capability: string) => capability !== "nativeFiles" || mocks.nativeFiles,
}));
vi.mock("@/lib/logger", () => ({ logger: { error: vi.fn() } }));
vi.mock("@/lib/backend/platform/path", () => ({ downloadDir: async () => "" }));
vi.mock("@/lib/invoke", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: mocks.translate }) }));

const baseItem: TransferItem = {
  id: "download-1",
  sessionId: "session-1",
  fileName: "business-cem.log",
  remotePath: "/remote/business-cem.log",
  localPath: "C:\\Downloads\\business-cem(1).log",
  direction: "download",
  kind: "file",
  source: "sftp",
  status: "transferring",
  size: 0,
  bytesTransferred: 10,
  totalSize: 100,
  timestamp: 1000,
};

beforeEach(() => {
  mocks.transfers = [{ ...baseItem }];
  mocks.nativeFiles = true;
});
afterEach(cleanup);

describe("FileTransfer download names", () => {
  it("shows the original name and renamed target while downloading, then switches immediately on completion", () => {
    const view = render(<FileTransfer activeSessionId="session-1" />);
    const sourceName = screen.getByText("business-cem.log");
    expect(sourceName.classList.contains("text-xs")).toBe(true);
    expect(screen.getByText("fileTransfer.renamed")).toBeTruthy();
    expect(screen.getByText("fileTransfer.saveAs:business-cem(1).log")).toBeTruthy();
    expect(sourceName.closest("[title]")?.getAttribute("title")).toContain(
      `fileTransfer.saveAs:${baseItem.localPath}`,
    );

    mocks.transfers = [{ ...baseItem, status: "completed" }];
    view.rerender(<FileTransfer activeSessionId="session-1" />);
    expect(screen.getByText("business-cem(1).log").classList.contains("text-xs")).toBe(true);
    expect(screen.getByText("fileTransfer.originalName:business-cem.log")).toBeTruthy();
    expect(screen.getByText("fileTransfer.completed")).toBeTruthy();
    expect(screen.queryByText("fileTransfer.saveAs:business-cem(1).log")).toBeNull();
    expect(mocks.transfers[0].fileName).toBe("business-cem.log");
    expect(mocks.transfers[0].remotePath).toBe("/remote/business-cem.log");
  });

  it.each([
    ["file", "/Downloads/business-cem(2).log", "business-cem.log", "business-cem(2).log"],
    ["directory", "C:\\Downloads\\logs.2026(1)\\", "logs.2026", "logs.2026(1)"],
    ["directory", "/Downloads/logs.2026(2)/", "logs.2026", "logs.2026(2)"],
    ["file", "C:/Downloads/custom.log", "business-cem.log", "custom.log"],
  ] as const)("displays the actual %s target for %s", (kind, localPath, fileName, targetName) => {
    mocks.transfers = [{ ...baseItem, kind, localPath, fileName, status: "completed" }];
    render(<FileTransfer activeSessionId="session-1" />);
    expect(screen.getByText(targetName)).toBeTruthy();
    expect(screen.getByText("fileTransfer.renamed")).toBeTruthy();
    expect(screen.getByText(`fileTransfer.originalName:${fileName}`)).toBeTruthy();
  });

  it("does not infer a rename from a suffix already present on the remote file", () => {
    mocks.transfers = [
      {
        ...baseItem,
        fileName: "business-cem(1).log",
        status: "completed",
      },
    ];
    render(<FileTransfer activeSessionId="session-1" />);
    const name = screen.getByText("business-cem(1).log");
    expect(name).toBeTruthy();
    expect(screen.queryByText("fileTransfer.renamed")).toBeNull();
    expect(name.closest("[title]")?.getAttribute("title")).toContain(
      `fileTransfer.savePath:${baseItem.localPath}`,
    );
  });

  it.each([
    "queued",
    "paused",
  ] as const)("does not label an unresolved %s target as renamed", (status) => {
    mocks.transfers = [{ ...baseItem, status, queueState: "pending" }];
    render(<FileTransfer activeSessionId="session-1" />);
    expect(screen.getByText("business-cem.log")).toBeTruthy();
    expect(screen.queryByText("fileTransfer.renamed")).toBeNull();
  });

  it.each([
    ["upload", "sftp"],
    ["copy", "sftp"],
    ["download", "rdp"],
    ["download", "zmodem"],
    ["download", "serial_modem"],
  ] as const)("preserves %s/%s display behavior", (direction, source) => {
    mocks.transfers = [{ ...baseItem, direction, source, status: "completed" }];
    render(<FileTransfer activeSessionId="session-1" />);
    expect(screen.getByText("business-cem.log")).toBeTruthy();
    expect(screen.queryByText("fileTransfer.renamed")).toBeNull();
  });

  it("does not expose internal local paths in browser mode", () => {
    mocks.nativeFiles = false;
    mocks.transfers = [{ ...baseItem, status: "completed" }];
    render(<FileTransfer activeSessionId="session-1" />);
    const sourceName = screen.getByText("business-cem.log");
    expect(screen.queryByText("fileTransfer.renamed")).toBeNull();
    expect(sourceName.closest("[title]")?.getAttribute("title")).not.toContain(baseItem.localPath);
  });

  it("falls back to the source name when the target path is unavailable", () => {
    mocks.transfers = [{ ...baseItem, localPath: "", status: "completed" }];
    render(<FileTransfer activeSessionId="session-1" />);
    expect(screen.getByText("business-cem.log")).toBeTruthy();
    expect(screen.queryByText("fileTransfer.renamed")).toBeNull();
  });
});

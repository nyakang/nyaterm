import { createRef } from "react";
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SshRuntimeMode } from "@/types/global";
import { useTerminalExternalDrop } from "./useTerminalExternalDrop";

const mocks = vi.hoisted(() => ({
  useTerminalFileDrop: vi.fn(),
  handleTerminalFileDrop: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("@/hooks/useTerminalFileDrop", () => ({
  useTerminalFileDrop: mocks.useTerminalFileDrop,
}));

vi.mock("@/lib/terminalFileDrop", () => ({
  getTerminalDropOverlayCopy: () => ({ title: "drop", hint: "hint" }),
  handleTerminalFileDrop: mocks.handleTerminalFileDrop,
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/logger", () => ({
  logger: { warn: vi.fn(), error: vi.fn() },
}));
vi.mock("sonner", () => ({
  toast: { error: vi.fn() },
}));

type HookProps = {
  sessionId: string;
  sshRuntimeMode?: SshRuntimeMode | null;
};

const containerRef = createRef<HTMLDivElement>();
const t = (key: string) => key;

function latestDropOptions() {
  return mocks.useTerminalFileDrop.mock.lastCall?.[0] as {
    enabled: boolean;
    processDropPaths: (paths: string[]) => Promise<void>;
  };
}

describe("useTerminalExternalDrop SSH runtime capability", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.handleTerminalFileDrop.mockResolvedValue(undefined);
    mocks.invoke.mockResolvedValue([{ path: "/tmp/file.bin", isDir: false }]);
  });

  it("disables drop immediately when the same pane reconnects from SSH to Mosh", async () => {
    const initialProps: HookProps = {
      sessionId: "ssh-1",
      sshRuntimeMode: "standard",
    };
    const view = renderHook(
      ({ sessionId, sshRuntimeMode }: HookProps) =>
        useTerminalExternalDrop({
          sessionId,
          sessionType: "SSH",
          sshRuntimeMode,
          visible: true,
          containerRef,
          t,
          duplicateStrategy: "overwrite",
        }),
      { initialProps },
    );

    expect(latestDropOptions().enabled).toBe(true);

    view.rerender({ sessionId: "mosh-2", sshRuntimeMode: null });
    expect(latestDropOptions().enabled).toBe(false);

    await act(async () => {
      await latestDropOptions().processDropPaths(["/tmp/file.bin"]);
    });
    expect(mocks.handleTerminalFileDrop).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "mosh-2", sshTransport: "mosh" }),
    );
  });

  it("re-enables drop when the same pane reconnects from Mosh to ordinary SSH", async () => {
    const initialProps: HookProps = {
      sessionId: "mosh-1",
      sshRuntimeMode: null,
    };
    const view = renderHook(
      ({ sessionId, sshRuntimeMode }: HookProps) =>
        useTerminalExternalDrop({
          sessionId,
          sessionType: "SSH",
          sshRuntimeMode,
          visible: true,
          containerRef,
          t,
          duplicateStrategy: "overwrite",
        }),
      { initialProps },
    );

    expect(latestDropOptions().enabled).toBe(false);

    view.rerender({ sessionId: "ssh-2", sshRuntimeMode: "standard" });
    expect(latestDropOptions().enabled).toBe(true);

    await act(async () => {
      await latestDropOptions().processDropPaths(["/tmp/file.bin"]);
    });
    expect(mocks.handleTerminalFileDrop).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "ssh-2", sshTransport: "ssh" }),
    );
  });
});

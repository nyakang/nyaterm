import { renderHook, waitFor } from "@testing-library/react";
import type { Terminal } from "@xterm/xterm";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalFitScheduler } from "./terminalFitScheduler";
import { useTerminalRefreshEffects } from "./useTerminalRefreshEffects";

const windowMocks = vi.hoisted(() => ({
  scaleChanged: undefined as
    | ((event: { payload: { scaleFactor: number } }) => void)
    | undefined,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onResized: async () => vi.fn(),
    onMoved: async () => vi.fn(),
    onFocusChanged: async () => vi.fn(),
    onScaleChanged: async (
      callback: (event: { payload: { scaleFactor: number } }) => void,
    ) => {
      windowMocks.scaleChanged = callback;
      return vi.fn();
    },
  }),
}));

describe("useTerminalRefreshEffects", () => {
  beforeEach(() => {
    windowMocks.scaleChanged = undefined;
  });

  it("repaints an active visible terminal without texture invalidation", () => {
    const schedule = vi.fn();
    renderHook(() =>
      useTerminalRefreshEffects({
        terminalRef: { current: {} as Terminal },
        fitSchedulerRef: {
          current: { schedule } as unknown as TerminalFitScheduler,
        },
        active: true,
        visible: true,
        terminalReady: true,
        performanceMode: "normal",
        sessionId: "session-1",
        showGutter: false,
        showContentPadding: false,
      }),
    );

    const activeRefresh = schedule.mock.calls
      .map(([request]) => request)
      .find((request) => request.reason === "active");
    expect(activeRefresh).toEqual(
      expect.objectContaining({ force: true, refresh: true, focus: true }),
    );
    expect(activeRefresh).not.toHaveProperty("clearTextureAtlas");
  });

  it("still invalidates textures after a DPI scale change", async () => {
    const schedule = vi.fn();
    renderHook(() =>
      useTerminalRefreshEffects({
        terminalRef: { current: {} as Terminal },
        fitSchedulerRef: {
          current: { schedule } as unknown as TerminalFitScheduler,
        },
        active: true,
        visible: true,
        terminalReady: true,
        performanceMode: "normal",
        sessionId: "session-1",
        showGutter: false,
        showContentPadding: false,
      }),
    );
    await waitFor(() =>
      expect(windowMocks.scaleChanged).toBeTypeOf("function"),
    );
    windowMocks.scaleChanged?.({ payload: { scaleFactor: 2 } });

    expect(schedule).toHaveBeenCalledWith(
      expect.objectContaining({
        reason: "scale-factor",
        force: true,
        refresh: true,
        clearTextureAtlas: true,
      }),
    );
  });

  it("suppresses incidental refreshes while a snapshot restore is finalizing", async () => {
    const schedule = vi.fn();
    const snapshotRestoringRef = { current: true };
    renderHook(() =>
      useTerminalRefreshEffects({
        terminalRef: { current: {} as Terminal },
        fitSchedulerRef: {
          current: { schedule } as unknown as TerminalFitScheduler,
        },
        active: true,
        visible: true,
        terminalReady: true,
        performanceMode: "normal",
        sessionId: "session-1",
        showGutter: false,
        showContentPadding: false,
        snapshotRestoringRef,
      }),
    );
    await waitFor(() =>
      expect(windowMocks.scaleChanged).toBeTypeOf("function"),
    );
    schedule.mockClear();

    window.dispatchEvent(new Event("nyaterm:refresh-terminals"));
    windowMocks.scaleChanged?.({ payload: { scaleFactor: 2 } });

    expect(schedule).not.toHaveBeenCalled();
  });
});

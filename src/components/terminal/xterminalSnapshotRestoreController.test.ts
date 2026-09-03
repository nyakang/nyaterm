import type { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalReconnectSnapshot } from "@/lib/terminalReconnectHistory";
import { writeTextInFrames } from "./xterminalOutputQueue";
import { createXTerminalSnapshotRestoreController } from "./xterminalSnapshotRestoreController";

const snapshot = (content: string): TerminalReconnectSnapshot => ({
  content,
  lineTimestamps: [],
  captureStartLine: 0,
  captureEndLine: 0,
});

describe("createXTerminalSnapshotRestoreController", () => {
  let animationFrames: FrameRequestCallback[];

  beforeEach(() => {
    animationFrames = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      animationFrames.push(callback);
      return animationFrames.length;
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const createHarness = () => {
    let rendererVisible = true;
    const readyStates: boolean[] = [];
    const restoringRef = { current: false };
    const controller = createXTerminalSnapshotRestoreController({
      restoringRef,
      setRestoring: (restoring) => {
        rendererVisible = !restoring;
      },
      setTerminalReady: (ready) => readyStates.push(ready),
    });
    return {
      controller,
      readyStates,
      rendererVisible: () => rendererVisible,
    };
  };

  const runNextFrame = () => {
    const callback = animationFrames.shift();
    expect(callback).toBeTypeOf("function");
    callback?.(performance.now());
  };

  it("keeps the renderer hidden through every replay frame until final fit", async () => {
    const harness = createHarness();
    const terminal = {
      write: vi.fn((_data: string, callback: () => void) => callback()),
    } as unknown as Terminal;
    const replay = writeTextInFrames(terminal, "x".repeat(96 * 1024));

    expect(harness.controller.begin(snapshot("large snapshot"))).toBe(true);
    expect(harness.rendererVisible()).toBe(false);

    runNextFrame();
    expect(terminal.write).toHaveBeenCalledTimes(1);
    expect(harness.rendererVisible()).toBe(false);

    runNextFrame();
    expect(terminal.write).toHaveBeenCalledTimes(2);
    expect(harness.rendererVisible()).toBe(false);

    while (animationFrames.length > 0) runNextFrame();
    await replay;
    expect(harness.controller.getPhase()).toBe("replaying");
    expect(harness.rendererVisible()).toBe(false);

    expect(harness.controller.markReplayAndAttachComplete()).toBe(true);
    expect(harness.controller.getPhase()).toBe("awaiting-final-fit");
    expect(harness.rendererVisible()).toBe(false);
    expect(harness.readyStates[harness.readyStates.length - 1]).toBe(true);

    expect(harness.controller.completeAfterFinalFit()).toBe(true);
    expect(harness.rendererVisible()).toBe(true);
    expect(harness.controller.completeAfterFinalFit()).toBe(false);
  });

  it("does not hide a terminal when there is no snapshot content", () => {
    const harness = createHarness();

    expect(harness.controller.begin(null)).toBe(false);
    expect(harness.controller.begin(snapshot(""))).toBe(false);
    expect(harness.rendererVisible()).toBe(true);
    expect(harness.controller.getPhase()).toBe("idle");
    expect(harness.readyStates).toEqual([]);
  });

  it("clears a stale restore barrier when the replacement has no snapshot", () => {
    const harness = createHarness();

    harness.controller.begin(snapshot("old session"));
    expect(harness.rendererVisible()).toBe(false);

    expect(harness.controller.begin(null)).toBe(false);
    expect(harness.rendererVisible()).toBe(true);
    expect(harness.controller.getPhase()).toBe("idle");
  });
});

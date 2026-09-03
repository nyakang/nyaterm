import type { Terminal } from "@xterm/xterm";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalReconnectSnapshot } from "@/lib/terminalReconnectHistory";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import type { SessionType } from "@/types/global";
import type { TerminalOutputDrain } from "./terminalOutputDrain";
import { createXTerminalHibernationController } from "./xterminalHibernationController";
import type {
  HibernationLogEvent,
  HibernationPhase,
} from "./xterminalInternalTypes";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));

const settle = async () => {
  for (let i = 0; i < 20; i += 1) {
    await Promise.resolve();
  }
};

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

function createHarness(
  options: {
    visible?: boolean;
    sessionType?: SessionType;
    lastOutputActivityAt?: number;
    flushFrameGateAndDrain?: (reason: string) => Promise<boolean>;
    reconnectSnapshot?: TerminalReconnectSnapshot | null;
  } = {},
) {
  let now = 0;
  let nextTimerId = 1;
  const timers = new Map<number, { at: number; callback: () => void }>();
  const hibernateTimerRef = { current: null as number | null };
  const hibernationPhaseRef = {
    current: "idle" as HibernationPhase,
  };
  const hibernatedRef = { current: false };
  const outputDrain = {
    getPendingBytes: vi.fn(() => 0),
    getQueueBytes: vi.fn(() => 0),
    isWriteInFlight: vi.fn(() => false),
    setMode: vi.fn(),
  } as unknown as TerminalOutputDrain<{ beforeLine: number; ts: number }>;
  const outputDrainRef = { current: outputDrain };
  const logs: Array<{
    event: HibernationLogEvent;
    message: string;
    data?: Record<string, unknown>;
  }> = [];
  const setTerminalReady = vi.fn();
  const setHibernated = vi.fn((value: boolean) => {
    hibernatedRef.current = value;
  });
  const setTerminalGeneration = vi.fn();
  const updateOutputDrainMode = vi.fn();
  const repaintVisibleTerminal = vi.fn();
  const beginSnapshotRestore = vi.fn();
  const flushFrameGateAndDrain =
    options.flushFrameGateAndDrain ?? vi.fn(async () => true);

  const clearHibernateTimer = () => {
    if (hibernateTimerRef.current !== null) {
      timers.delete(hibernateTimerRef.current);
      hibernateTimerRef.current = null;
    }
  };

  const controller = createXTerminalHibernationController({
    sessionId: "session-1",
    terminal: {
      buffer: { active: { type: "normal" } },
    } as unknown as Terminal,
    outputDrain,
    visibleRef: { current: options.visible ?? false },
    sessionTypeRef: { current: options.sessionType ?? "SSH" },
    aiCapturingRef: { current: false },
    zmodemActiveRef: { current: false },
    syncPeerSessionIdsRef: { current: undefined },
    outputDrainRef,
    disconnectedRef: { current: false },
    reconnectingRef: { current: false },
    hibernateTimerRef,
    hibernationEpochRef: { current: 0 },
    hibernationPendingRef: { current: false },
    hibernationPhaseRef,
    detachedHibernateEpochRef: { current: null },
    hibernationSnapshotRef: {
      current: null as TerminalReconnectSnapshot | null,
    },
    hibernationCleanupRef: { current: false },
    hibernatedRef,
    lastOutputActivityAtRef: {
      current: options.lastOutputActivityAt ?? 0,
    },
    showSearchBar: false,
    activeMode: "buffer",
    isTerminalAlive: () => true,
    logHibernation: (event, message, data) => {
      logs.push({ event, message, data });
    },
    clearHibernateTimer,
    enterDisconnectedStateIfAttachSessionMissing: () => false,
    updateOutputDrainMode,
    flushFrameGateAndDrain,
    captureReconnectSnapshot: () => options.reconnectSnapshot ?? null,
    beginSnapshotRestore,
    setTerminalReady,
    setHibernated,
    setTerminalGeneration,
    maybeRecoverPerformanceMode: vi.fn(),
    refreshOutputPressureMode: vi.fn(),
    repaintVisibleTerminal,
    timers: {
      now: () => now,
      setTimeout: (callback, delay) => {
        const id = nextTimerId;
        nextTimerId += 1;
        timers.set(id, { at: now + delay, callback });
        return id;
      },
      clearTimeout: (id) => {
        timers.delete(id);
      },
    },
  });

  const advance = (ms: number) => {
    now += ms;
    const due = [...timers.entries()]
      .filter(([, timer]) => timer.at <= now)
      .sort((left, right) => left[1].at - right[1].at);
    for (const [id, timer] of due) {
      timers.delete(id);
      timer.callback();
    }
  };

  controller.applyVisibilityPolicy();

  return {
    advance,
    beginSnapshotRestore,
    controller,
    flushFrameGateAndDrain,
    hibernatedRef,
    hibernationPhaseRef,
    logs,
    repaintVisibleTerminal,
    setHibernated,
    setTerminalGeneration,
    setTerminalReady,
    timers,
    updateOutputDrainMode,
    getNow: () => now,
  };
}

describe("createXTerminalHibernationController", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.invoke.mockResolvedValue(undefined);
  });

  it("hibernates a hidden terminal after a full output-idle timeout", async () => {
    const { advance, hibernatedRef, hibernationPhaseRef, setHibernated } =
      createHarness();

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs);
    await settle();

    expect(mocks.invoke).toHaveBeenCalledWith("detach_session_renderer", {
      sessionId: "session-1",
    });
    expect(hibernationPhaseRef.current).toBe("hibernated");
    expect(hibernatedRef.current).toBe(true);
    expect(setHibernated).toHaveBeenCalledWith(true);
  });

  it("starts snapshot restore before disposing a hibernated renderer", async () => {
    const snapshot: TerminalReconnectSnapshot = {
      content: "preserved terminal history",
      lineTimestamps: [],
      captureStartLine: 0,
      captureEndLine: 0,
    };
    const { advance, beginSnapshotRestore, setHibernated } = createHarness({
      reconnectSnapshot: snapshot,
    });

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs);
    await settle();

    expect(beginSnapshotRestore).toHaveBeenCalledWith(snapshot);
    expect(beginSnapshotRestore.mock.invocationCallOrder[0]).toBeLessThan(
      setHibernated.mock.invocationCallOrder[0],
    );
  });

  it("does not hibernate while hidden output activity continues", async () => {
    const { advance, controller } = createHarness();

    for (let i = 0; i < 3; i += 1) {
      advance(60_000);
      controller.noteOutputActivity();
      await settle();
      expect(mocks.invoke).not.toHaveBeenCalledWith(
        "detach_session_renderer",
        expect.anything(),
      );
    }

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs - 1);
    await settle();

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "detach_session_renderer",
      expect.anything(),
    );
  });

  it("does not hibernate when output arrives at 119 seconds", async () => {
    const { advance, controller } = createHarness();

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs - 1_000);
    controller.noteOutputActivity();
    advance(1_000);
    await settle();

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "detach_session_renderer",
      expect.anything(),
    );
  });

  it("hibernates only after output stops for another full idle timeout", async () => {
    const { advance, controller, hibernationPhaseRef } = createHarness();

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs - 1_000);
    controller.noteOutputActivity();
    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs - 1);
    await settle();
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "detach_session_renderer",
      expect.anything(),
    );

    advance(1);
    await settle();

    expect(mocks.invoke).toHaveBeenCalledWith("detach_session_renderer", {
      sessionId: "session-1",
    });
    expect(hibernationPhaseRef.current).toBe("hibernated");
  });

  it("never hibernates a visible terminal from output idleness", async () => {
    const { advance, controller, repaintVisibleTerminal, timers } =
      createHarness({
        visible: true,
      });

    expect(timers.size).toBe(0);
    expect(repaintVisibleTerminal).toHaveBeenCalled();
    controller.noteOutputActivity();
    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs * 2);
    await settle();

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "detach_session_renderer",
      expect.anything(),
    );
  });

  it("cancels preparing hibernation when output activity arrives", async () => {
    const firstDrain = createDeferred<boolean>();
    const { advance, controller, hibernationPhaseRef, timers } = createHarness({
      flushFrameGateAndDrain: vi.fn(() => firstDrain.promise),
    });

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs);
    await settle();
    expect(hibernationPhaseRef.current).toBe("preparing");

    controller.noteOutputActivity();
    firstDrain.resolve(true);
    await settle();

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "detach_session_renderer",
      expect.anything(),
    );
    expect(hibernationPhaseRef.current).toBe("idle");
    expect(timers.size).toBe(1);
  });

  it("rolls back a detached renderer when output activity arrives", async () => {
    const afterDetachDrain = createDeferred<boolean>();
    const flushFrameGateAndDrain = vi
      .fn()
      .mockResolvedValueOnce(true)
      .mockReturnValueOnce(afterDetachDrain.promise);
    const { advance, controller, hibernatedRef, hibernationPhaseRef } =
      createHarness({
        flushFrameGateAndDrain,
      });

    advance(XTERM_PERFORMANCE_CONFIG.lifecycle.deepHibernateDelayMs);
    await settle();
    expect(hibernationPhaseRef.current).toBe("detached");

    controller.noteOutputActivity();
    await settle();

    expect(mocks.invoke).toHaveBeenCalledWith("attach_session", {
      sessionId: "session-1",
    });
    expect(hibernationPhaseRef.current).toBe("idle");

    afterDetachDrain.resolve(true);
    await settle();

    expect(hibernatedRef.current).toBe(false);
    expect(hibernationPhaseRef.current).toBe("idle");
  });
});

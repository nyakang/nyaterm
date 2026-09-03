import { describe, expect, it, vi } from "vitest";
import { XTERM_PERFORMANCE_CONFIG } from "@/lib/xtermPerformance";
import {
  TerminalOutputDrain,
  type TerminalOutputBackgroundDrainStats,
  type TerminalOutputForegroundFrameFallbackStats,
} from "./terminalOutputDrain";

const settle = async () => {
  for (let i = 0; i < 10; i += 1) {
    await Promise.resolve();
  }
};

function createHarness(
  options: {
    writeChunkBytes?: number;
    autoCompleteWrites?: boolean;
    shouldUseLowLatencyFlush?: () => boolean;
    getForegroundDelayMs?: () => number;
    writeDurationMs?: number;
  } = {},
) {
  let now = 0;
  let nextTimerId = 1;
  let nextFrameId = 1;
  const timers = new Map<number, { at: number; callback: () => void }>();
  const frames = new Map<number, FrameRequestCallback>();
  const canceledFrames: FrameRequestCallback[] = [];
  const pendingWriteCallbacks: Array<() => void> = [];
  const writes: string[] = [];
  const acks: number[] = [];
  const pressure: number[] = [];
  const backgroundDrains: TerminalOutputBackgroundDrainStats[] = [];
  const foregroundFallbacks: TerminalOutputForegroundFrameFallbackStats[] = [];

  const terminal = {
    write: vi.fn((data: string, callback?: () => void) => {
      writes.push(data);
      now += options.writeDurationMs ?? 0;
      if (!callback) return;
      if (options.autoCompleteWrites === false) {
        pendingWriteCallbacks.push(callback);
      } else {
        callback();
      }
    }),
  };

  const drain = new TerminalOutputDrain({
    sessionId: "session-1",
    getTerminal: () => terminal,
    getWriteChunkBytes: () => options.writeChunkBytes ?? 1024,
    getForegroundDelayMs: options.getForegroundDelayMs,
    shouldUseLowLatencyFlush: options.shouldUseLowLatencyFlush,
    onAck: (bytes) => acks.push(bytes),
    onPressureChange: (bytes) => pressure.push(bytes),
    onBackgroundDrain: (stats) => backgroundDrains.push(stats),
    onForegroundFrameFallback: (stats) => foregroundFallbacks.push(stats),
    timers: {
      requestAnimationFrame: (callback) => {
        const id = nextFrameId;
        nextFrameId += 1;
        frames.set(id, callback);
        return id;
      },
      cancelAnimationFrame: (id) => {
        const callback = frames.get(id);
        if (callback) {
          canceledFrames.push(callback);
        }
        frames.delete(id);
      },
      setTimeout: (callback, delay) => {
        const id = nextTimerId;
        nextTimerId += 1;
        timers.set(id, { at: now + delay, callback });
        return id;
      },
      clearTimeout: (id) => {
        timers.delete(id);
      },
      queueMicrotask: (callback) => callback(),
      now: () => now,
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

  const flushFrame = () => {
    const due = [...frames.values()];
    frames.clear();
    for (const callback of due) {
      callback(now);
    }
  };

  const flushCanceledFrames = () => {
    const due = [...canceledFrames];
    canceledFrames.length = 0;
    for (const callback of due) {
      callback(now);
    }
  };

  return {
    acks,
    advance,
    backgroundDrains,
    drain,
    flushCanceledFrames,
    flushFrame,
    foregroundFallbacks,
    pendingWriteCallbacks,
    pressure,
    terminal,
    timers,
    writes,
    getFrameCount: () => frames.size,
    getNow: () => now,
  };
}

describe("TerminalOutputDrain", () => {
  it("uses normal background cadence for small hidden queues", async () => {
    const { advance, backgroundDrains, drain, timers, writes } =
      createHarness();

    drain.setMode("background");
    drain.enqueue({ data: "small", bytes: 5 });

    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs,
    );
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs - 1);
    await settle();
    expect(writes).toEqual([]);

    advance(1);
    await settle();

    expect(writes).toEqual(["small"]);
    expect(backgroundDrains[0]).toMatchObject({
      backgroundCatchUp: false,
      drainChunkBytes: XTERM_PERFORMANCE_CONFIG.output.backgroundWriteChunkBytes,
      nextDelayMs: XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs,
    });
  });

  it("drains hidden output in original order", async () => {
    const { advance, drain, writes } = createHarness();

    drain.setMode("background");
    drain.enqueue({ data: "A", bytes: 1 });
    drain.enqueue({ data: "B", bytes: 1 });
    drain.enqueue({ data: "C", bytes: 1 });

    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs);
    await settle();

    expect(writes.join("")).toBe("ABC");
  });

  it("consumes hidden output periodically instead of waiting for reveal", async () => {
    const { advance, drain, writes } = createHarness({ writeChunkBytes: 1 });

    drain.setMode("background");
    drain.enqueue({ data: "A", bytes: 1 });
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs);
    await settle();
    drain.enqueue({ data: "B", bytes: 1 });
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs);
    await settle();

    expect(writes).toEqual(["A", "B"]);
  });

  it("enters background catch-up cadence when hidden backlog is strained", async () => {
    const { advance, backgroundDrains, drain, timers, writes } =
      createHarness();
    const bytes = XTERM_PERFORMANCE_CONFIG.output.strainedBacklogBytes;

    drain.setMode("background");
    drain.enqueue({ data: "x".repeat(bytes), bytes });

    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs,
    );

    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs);
    await settle();

    expect(writes).toHaveLength(1);
    expect(writes[0]).toHaveLength(
      XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpWriteChunkBytes,
    );
    expect(backgroundDrains[0]).toMatchObject({
      backgroundCatchUp: true,
      drainChunkBytes:
        XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpWriteChunkBytes,
      nextDelayMs: XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs,
    });
  });

  it("exits background catch-up after draining to the low watermark", async () => {
    const { advance, backgroundDrains, drain, timers } = createHarness();
    const bytes = XTERM_PERFORMANCE_CONFIG.output.strainedBacklogBytes;

    drain.setMode("background");
    drain.enqueue({ data: "x".repeat(bytes), bytes });
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs);
    await settle();

    expect(backgroundDrains[0]?.backgroundCatchUp).toBe(true);
    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs +
        XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs,
    );
  });

  it("uses foreground scheduling without background catch-up after reveal", async () => {
    const { drain, flushFrame, getFrameCount, timers, writes } = createHarness({
      writeChunkBytes: 1024,
    });
    const bytes = XTERM_PERFORMANCE_CONFIG.output.strainedBacklogBytes;

    drain.setMode("background");
    drain.enqueue({ data: "x".repeat(bytes), bytes });
    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.backgroundCatchUpIntervalMs,
    );

    drain.setMode("foreground");
    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs,
    );
    expect(getFrameCount()).toBe(1);

    flushFrame();
    await settle();

    expect(writes).toHaveLength(1);
    expect(writes[0]).toHaveLength(1024);
    expect(getFrameCount()).toBe(1);
  });

  it("does not background write while hibernating or hibernated", async () => {
    const { advance, drain, timers, writes } = createHarness();

    drain.setMode("hibernating");
    drain.enqueue({ data: "A", bytes: 1 });
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs * 10);
    await settle();
    expect(timers.size).toBe(0);
    expect(writes).toEqual([]);

    drain.setMode("hibernated");
    drain.enqueue({ data: "B", bytes: 1 });
    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs * 10);
    await settle();
    expect(timers.size).toBe(0);
    expect(writes).toEqual([]);
  });

  it("chunks large foreground output cooperatively", async () => {
    const { drain, flushFrame, writes } = createHarness({ writeChunkBytes: 4 });

    drain.setMode("foreground");
    drain.enqueue({ data: "abcdefghij", bytes: 10 });
    flushFrame();
    await settle();

    expect(writes).toEqual(["abcd"]);
    await settle();
    flushFrame();
    await settle();
    expect(writes).toEqual(["abcd", "efgh"]);
    await settle();
    flushFrame();
    await settle();
    expect(writes).toEqual(["abcd", "efgh", "ij"]);
  });

  it("uses the foreground frame fallback when animation frames are starved", async () => {
    const { acks, advance, drain, foregroundFallbacks, getFrameCount, writes } =
      createHarness({
        writeChunkBytes: 8,
      });

    drain.setMode("foreground");
    drain.enqueue({ data: "fallback", bytes: 8 });
    expect(getFrameCount()).toBe(1);

    advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs - 1);
    await settle();
    expect(writes).toEqual([]);

    advance(1);
    await settle();

    expect(writes).toEqual(["fallback"]);
    expect(acks).toEqual([8]);
    expect(getFrameCount()).toBe(0);
    expect(foregroundFallbacks).toHaveLength(1);
    expect(foregroundFallbacks[0]).toMatchObject({
      queueBytes: 8,
      pendingBytes: 8,
      fallbackDelayMs: XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs,
    });
  });

  it("cancels the foreground fallback when the animation frame wins", async () => {
    const { advance, drain, flushFrame, foregroundFallbacks, timers, writes } =
      createHarness();

    drain.setMode("foreground");
    drain.enqueue({ data: "frame", bytes: 5 });

    flushFrame();
    await settle();
    advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs);
    await settle();

    expect(writes).toEqual(["frame"]);
    expect(foregroundFallbacks).toEqual([]);
    expect(timers.size).toBe(0);
  });

  it("ignores stale animation frames after the foreground fallback wins", async () => {
    const { advance, drain, flushCanceledFrames, foregroundFallbacks, writes } =
      createHarness();

    drain.setMode("foreground");
    drain.enqueue({ data: "timer", bytes: 5 });

    advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs);
    await settle();
    flushCanceledFrames();
    await settle();

    expect(writes).toEqual(["timer"]);
    expect(foregroundFallbacks).toHaveLength(1);
  });

  it("cleans up foreground watchdogs when switching to background", async () => {
    const {
      advance,
      drain,
      flushCanceledFrames,
      foregroundFallbacks,
      getFrameCount,
      timers,
      writes,
    } = createHarness();

    drain.setMode("foreground");
    drain.enqueue({ data: "background", bytes: 10 });
    expect(getFrameCount()).toBe(1);

    drain.setMode("background");
    expect(getFrameCount()).toBe(0);
    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs,
    );

    flushCanceledFrames();
    advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs);
    await settle();
    expect(writes).toEqual([]);

    advance(
      XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs -
        XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs,
    );
    await settle();

    expect(writes).toEqual(["background"]);
    expect(foregroundFallbacks).toEqual([]);
  });

  it("does not write after disposing with pending foreground watchdogs", async () => {
    const { advance, drain, flushCanceledFrames, getFrameCount, timers, writes } =
      createHarness();

    drain.setMode("foreground");
    drain.enqueue({ data: "disposed", bytes: 8 });
    expect(getFrameCount()).toBe(1);

    drain.dispose();
    expect(getFrameCount()).toBe(0);
    expect(timers.size).toBe(0);

    flushCanceledFrames();
    advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs);
    await settle();

    expect(writes).toEqual([]);
  });

  it("keeps draining sustained foreground backlog through fallback timers", async () => {
    const { acks, advance, drain, foregroundFallbacks, writes } = createHarness({
      writeChunkBytes: 2,
    });

    drain.setMode("foreground");
    drain.enqueue({ data: "abcdef", bytes: 6 });

    for (let i = 0; i < 3; i += 1) {
      advance(XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs);
      await settle();
    }

    expect(writes).toEqual(["ab", "cd", "ef"]);
    expect(acks.reduce((total, bytes) => total + bytes, 0)).toBe(6);
    expect(foregroundFallbacks).toHaveLength(3);
    expect(drain.getQueueBytes()).toBe(0);
  });

  it("uses the microtask fast path for light foreground pressure", async () => {
    const { drain, getFrameCount, writes } = createHarness({
      shouldUseLowLatencyFlush: () => true,
      writeChunkBytes: 16,
    });

    drain.setMode("foreground");
    drain.enqueue({ data: "hello", bytes: 5 });
    await settle();

    expect(writes).toEqual(["hello"]);
    expect(getFrameCount()).toBe(0);
  });

  it("yields to the next frame when a foreground drain turn exhausts its budget", async () => {
    const { drain, flushFrame, getFrameCount, writes } = createHarness({
      shouldUseLowLatencyFlush: () => true,
      writeChunkBytes: 4,
      writeDurationMs: XTERM_PERFORMANCE_CONFIG.output.maxForegroundDrainTurnMs + 1,
    });

    drain.setMode("foreground");
    drain.enqueue({ data: "abcdefghijkl", bytes: 12 });
    await settle();

    expect(writes).toEqual(["abcd"]);
    expect(getFrameCount()).toBe(1);

    flushFrame();
    await settle();
    expect(writes).toEqual(["abcd", "efgh"]);
  });

  it("honors foreground delay only when the scheduler reports severe backlog", async () => {
    let severeBacklog = false;
    const { advance, drain, timers, writes } = createHarness({
      getForegroundDelayMs: () => (severeBacklog ? 50 : 0),
      writeChunkBytes: 8,
    });

    drain.setMode("foreground");
    severeBacklog = true;
    drain.enqueue({ data: "alt", bytes: 3 });
    expect(timers.size).toBe(1);
    expect(writes).toEqual([]);

    advance(49);
    await settle();
    expect(writes).toEqual([]);

    advance(1);
    await settle();
    expect(writes).toEqual(["alt"]);
  });

  it("acks only bytes completed by write callbacks", async () => {
    const { acks, drain, flushFrame, pendingWriteCallbacks } = createHarness({
      autoCompleteWrites: false,
      writeChunkBytes: 4,
    });

    drain.setMode("foreground");
    drain.enqueue({ data: "abcd", bytes: 4 });
    flushFrame();
    await settle();

    expect(acks).toEqual([]);
    pendingWriteCallbacks.shift()?.();
    await settle();
    expect(acks).toEqual([4]);
  });

  it("cancels background timers when switching to foreground without duplicate writes or acks", async () => {
    const { acks, advance, drain, flushFrame, pendingWriteCallbacks, timers, writes } =
      createHarness({
        autoCompleteWrites: false,
        writeChunkBytes: 4,
      });

    drain.setMode("background");
    drain.enqueue({ data: "abcd", bytes: 4 });
    expect(timers.size).toBe(1);

    drain.setMode("foreground");
    expect([...timers.values()][0]?.at).toBe(
      XTERM_PERFORMANCE_CONFIG.output.foregroundFrameFallbackMs,
    );
    flushFrame();
    await settle();

    expect(writes).toEqual(["abcd"]);
    expect(acks).toEqual([]);
    expect(timers.size).toBe(0);

    advance(XTERM_PERFORMANCE_CONFIG.output.backgroundDrainIntervalMs);
    await settle();
    expect(writes).toEqual(["abcd"]);
    expect(acks).toEqual([]);

    pendingWriteCallbacks.shift()?.();
    await settle();
    expect(acks).toEqual([4]);
  });

  it("waitForIdle drains all data without dropping queued bytes", async () => {
    const { drain, writes } = createHarness({ writeChunkBytes: 2 });

    drain.setMode("hibernating");
    drain.enqueue({ data: "\x1b[?25lxx\x1b[?25h", bytes: 14 });

    await expect(drain.waitForIdle(100)).resolves.toBe(true);
    expect(writes.join("")).toBe("\x1b[?25lxx\x1b[?25h");
  });

  it("waitForIdle has a hard timeout when xterm never calls back", async () => {
    const { advance, drain, pendingWriteCallbacks } = createHarness({
      autoCompleteWrites: false,
    });

    drain.setMode("hibernating");
    drain.enqueue({ data: "blocked", bytes: 7 });
    const idle = drain.waitForIdle(100);
    await settle();
    expect(pendingWriteCallbacks).toHaveLength(1);

    advance(100);
    await expect(idle).resolves.toBe(false);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  debug: vi.fn(),
  error: vi.fn(),
  warn: vi.fn(),
  nextRequestId: 0,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}));

vi.mock("./logger", () => ({
  logger: {
    createRequestId: () => {
      mocks.nextRequestId += 1;
      return `request-${mocks.nextRequestId}`;
    },
    debug: mocks.debug,
    error: mocks.error,
    warn: mocks.warn,
  },
}));

import { InvokeHealthTracker, invoke, invokeHealthTracker } from "./invoke";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, reject, resolve };
}

describe("Tauri invoke health tracking", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.debug.mockReset();
    mocks.error.mockReset();
    mocks.warn.mockReset();
    mocks.nextRequestId = 0;
    invokeHealthTracker.reset();
  });

  afterEach(() => {
    invokeHealthTracker.reset();
    vi.useRealTimers();
  });

  it("cleans up successful and failed invokes", async () => {
    mocks.invoke.mockResolvedValueOnce("ok").mockRejectedValueOnce(new Error("failed"));

    await expect(invoke("success_command")).resolves.toBe("ok");
    expect(invokeHealthTracker.snapshot().inflight_total).toBe(0);

    await expect(invoke("error_command")).rejects.toThrow("failed");
    expect(invokeHealthTracker.snapshot()).toEqual({
      inflight_total: 0,
      inflight_by_command: {},
      oldest_inflight_ms: 0,
    });
  });

  it("classifies concurrent commands and cleans up repeated failures", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const third = deferred<string>();
    mocks.invoke
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
      .mockReturnValueOnce(third.promise);

    const requests = [invoke("alpha"), invoke("alpha"), invoke("beta")];
    expect(invokeHealthTracker.snapshot()).toMatchObject({
      inflight_total: 3,
      inflight_by_command: { alpha: 2, beta: 1 },
    });

    first.reject(new Error("one"));
    second.reject(new Error("two"));
    third.reject(new Error("three"));
    await Promise.allSettled(requests);

    expect(invokeHealthTracker.snapshot().inflight_total).toBe(0);
  });

  it("calculates the oldest request age", () => {
    let now = 100;
    const tracker = new InvokeHealthTracker({
      now: () => now,
      setTimeout: () => 1 as unknown as ReturnType<typeof setTimeout>,
      clearTimeout: () => {},
    });

    tracker.start("one", "alpha");
    now = 250;
    tracker.start("two", "beta");
    now = 600;

    expect(tracker.snapshot()).toEqual({
      inflight_total: 2,
      inflight_by_command: { alpha: 1, beta: 1 },
      oldest_inflight_ms: 500,
    });
    tracker.finish("one");
    tracker.finish("one");
    expect(tracker.snapshot().oldest_inflight_ms).toBe(350);
  });

  it("reports a request when it crosses the oldest-age threshold", () => {
    let now = 0;
    let scheduled: { callback: () => void; delay: number } | null = null;
    const health = vi.fn();
    const tracker = new InvokeHealthTracker({
      now: () => now,
      highInflightCount: 32,
      staleAfterMs: 15,
      logIntervalMs: 60,
      onHealth: health,
      setTimeout: (callback, delay) => {
        scheduled = { callback, delay };
        return 1 as unknown as ReturnType<typeof setTimeout>;
      },
      clearTimeout: () => {
        scheduled = null;
      },
    });

    tracker.start("one", "slow_command");
    expect(scheduled).toMatchObject({ delay: 15 });
    now = 15;
    const callback = (scheduled as { callback: () => void } | null)?.callback;
    callback?.();

    expect(health).toHaveBeenCalledWith({
      inflight_total: 1,
      inflight_by_command: { slow_command: 1 },
      oldest_inflight_ms: 15,
    });
    tracker.finish("one");
    expect(scheduled).toBeNull();
  });

  it("rate limits unhealthy snapshots and releases its timer when idle", () => {
    let now = 0;
    let nextTimerId = 1;
    const timers = new Map<number, () => void>();
    const health = vi.fn();
    const tracker = new InvokeHealthTracker({
      now: () => now,
      highInflightCount: 2,
      staleAfterMs: 15,
      logIntervalMs: 60,
      onHealth: health,
      setTimeout: (callback) => {
        const id = nextTimerId;
        nextTimerId += 1;
        timers.set(id, callback);
        return id as unknown as ReturnType<typeof setTimeout>;
      },
      clearTimeout: (handle) => {
        timers.delete(handle as unknown as number);
      },
    });

    tracker.start("one", "alpha");
    expect(health).not.toHaveBeenCalled();
    expect(timers.size).toBe(1);

    tracker.start("two", "beta");
    expect(health).toHaveBeenCalledTimes(1);
    tracker.start("three", "beta");
    expect(health).toHaveBeenCalledTimes(1);

    now = 60;
    const callback = [...timers.values()][0];
    timers.clear();
    callback?.();
    expect(health).toHaveBeenCalledTimes(2);

    tracker.finish("one");
    tracker.finish("two");
    tracker.finish("three");
    expect(timers.size).toBe(0);
  });
});

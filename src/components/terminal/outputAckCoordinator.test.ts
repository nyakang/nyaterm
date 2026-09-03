import { describe, expect, it, vi } from "vitest";
import { OutputAckCoordinator } from "./outputAckCoordinator";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, reject, resolve };
}

async function settle() {
  for (let index = 0; index < 10; index += 1) {
    await Promise.resolve();
  }
}

function createHarness() {
  let now = 0;
  let activeInvokes = 0;
  let maxConcurrentInvokes = 0;
  let nextTimerId = 1;
  const calls: Array<{
    sessionId: string;
    bytes: number;
    deferred: ReturnType<typeof deferred>;
  }> = [];
  const timers = new Map<number, { callback: () => void; delay: number }>();
  const failures = vi.fn();
  const coordinator = new OutputAckCoordinator({
    invokeAck: (sessionId, bytes) => {
      const pending = deferred();
      activeInvokes += 1;
      maxConcurrentInvokes = Math.max(maxConcurrentInvokes, activeInvokes);
      calls.push({ sessionId, bytes, deferred: pending });
      return pending.promise.finally(() => {
        activeInvokes -= 1;
      });
    },
    onFailure: failures,
    timers: {
      now: () => now,
      setTimeout: (callback, delay) => {
        const id = nextTimerId;
        nextTimerId += 1;
        timers.set(id, { callback, delay });
        return id as unknown as ReturnType<typeof setTimeout>;
      },
      clearTimeout: (handle) => {
        timers.delete(handle as unknown as number);
      },
    },
  });

  return {
    calls,
    coordinator,
    failures,
    timers,
    getMaxConcurrentInvokes: () => maxConcurrentInvokes,
    advanceBy: (duration: number) => {
      now += duration;
    },
    advanceToNextTimer: () => {
      const next = [...timers.entries()][0];
      if (!next) return;
      const [id, timer] = next;
      timers.delete(id);
      now += timer.delay;
      timer.callback();
    },
  };
}

describe("OutputAckCoordinator", () => {
  it("sends a single ACK and releases disposed session state", async () => {
    const { calls, coordinator } = createHarness();
    const lease = coordinator.acquire("session-1", 1);

    lease.ack(128);
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([128]);
    calls[0].deferred.resolve();
    await settle();
    lease.dispose();

    expect(coordinator.snapshot("session-1")).toEqual({
      leases: 0,
      pendingBytes: 0,
      inFlight: 0,
      retryTimers: 0,
    });
  });

  it("coalesces pending bytes and never overlaps invokes", async () => {
    const { calls, coordinator, getMaxConcurrentInvokes } = createHarness();
    const lease = coordinator.acquire("session-1", 1);

    lease.ack(100);
    await settle();
    lease.ack(200);
    lease.ack(300);
    expect(calls.map(({ bytes }) => bytes)).toEqual([100]);

    calls[0].deferred.resolve();
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100, 500]);
    calls[1].deferred.resolve();
    await settle();

    expect(calls.reduce((total, call) => total + call.bytes, 0)).toBe(600);
    expect(getMaxConcurrentInvokes()).toBe(1);
    lease.dispose();
  });

  it("requeues failed bytes with backoff and merges later ACKs", async () => {
    const { advanceToNextTimer, calls, coordinator, failures, timers } = createHarness();
    const lease = coordinator.acquire("session-1", 1);

    lease.ack(100);
    await settle();
    calls[0].deferred.reject(new Error("ipc unavailable"));
    await settle();
    lease.ack(200);

    expect(timers.size).toBe(1);
    expect([...timers.values()][0]?.delay).toBe(250);
    expect(coordinator.snapshot("session-1").pendingBytes).toBe(300);
    expect(failures).toHaveBeenCalledTimes(1);

    advanceToNextTimer();
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100, 300]);
    calls[1].deferred.resolve();
    await settle();
    expect(coordinator.snapshot("session-1").pendingBytes).toBe(0);
    lease.dispose();
  });

  it("applies exponential backoff capped at five seconds", async () => {
    const { advanceToNextTimer, calls, coordinator, timers } = createHarness();
    const lease = coordinator.acquire("session-backoff", 1);
    const expectedDelays = [250, 500, 1_000, 2_000, 4_000, 5_000, 5_000];

    lease.ack(1);
    await settle();
    for (const [index, expectedDelay] of expectedDelays.entries()) {
      calls[index].deferred.reject(new Error(`failure-${index}`));
      await settle();
      expect([...timers.values()][0]?.delay).toBe(expectedDelay);
      advanceToNextTimer();
      await settle();
    }

    calls[expectedDelays.length].deferred.resolve();
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual(Array(expectedDelays.length + 1).fill(1));
    lease.dispose();
  });

  it("rate limits failure diagnostics per session", async () => {
    const { advanceBy, advanceToNextTimer, calls, coordinator, failures } = createHarness();
    const lease = coordinator.acquire("session-log-limit", 7);

    lease.ack(10);
    await settle();
    calls[0].deferred.reject(new Error("first"));
    await settle();
    advanceToNextTimer();
    await settle();
    calls[1].deferred.reject(new Error("suppressed"));
    await settle();
    expect(failures).toHaveBeenCalledTimes(1);

    advanceToNextTimer();
    await settle();
    advanceBy(30_000);
    calls[2].deferred.reject(new Error("reported"));
    await settle();

    expect(failures).toHaveBeenCalledTimes(2);
    expect(failures.mock.calls[1]?.[0]).toMatchObject({
      sessionId: "session-log-limit",
      generation: 7,
      bytes: 10,
      pendingBytes: 10,
      suppressedFailures: 1,
    });
    lease.dispose();
    await settle();
    calls[3].deferred.resolve();
    await settle();
  });

  it("turns synchronous invoke rejection into one scheduled retry", async () => {
    let attempts = 0;
    let timer: (() => void) | null = null;
    const coordinator = new OutputAckCoordinator({
      invokeAck: () => {
        attempts += 1;
        return Promise.reject(new Error("synchronous rejection"));
      },
      timers: {
        setTimeout: (callback) => {
          timer = callback;
          return 1 as unknown as ReturnType<typeof setTimeout>;
        },
        clearTimeout: () => {
          timer = null;
        },
      },
    });
    const lease = coordinator.acquire("session-sync-reject", 1);

    lease.ack(32);
    await settle();
    expect(attempts).toBe(1);
    expect(timer).not.toBeNull();

    const retry = timer as (() => void) | null;
    timer = null;
    retry?.();
    await settle();
    expect(attempts).toBe(2);
    expect(timer).not.toBeNull();

    lease.dispose();
    await settle();
    expect(attempts).toBe(3);
    expect(timer).toBeNull();
  });

  it("stops retrying after one final disposed attempt", async () => {
    const { calls, coordinator, timers } = createHarness();
    const lease = coordinator.acquire("session-1", 1);

    lease.ack(64);
    await settle();
    lease.dispose();
    calls[0].deferred.reject(new Error("first failure"));
    await settle();

    expect(calls.map(({ bytes }) => bytes)).toEqual([64, 64]);
    calls[1].deferred.reject(new Error("final failure"));
    await settle();

    expect(timers.size).toBe(0);
    expect(coordinator.snapshot("session-1")).toEqual({
      leases: 0,
      pendingBytes: 0,
      inFlight: 0,
      retryTimers: 0,
    });
  });

  it("serializes generations and prevents stale leases from adding bytes", async () => {
    const { calls, coordinator, getMaxConcurrentInvokes } = createHarness();
    const oldLease = coordinator.acquire("session-1", 1);
    oldLease.ack(100);
    await settle();

    const newLease = coordinator.acquire("session-1", 2);
    oldLease.ack(999);
    newLease.ack(200);
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100]);

    calls[0].deferred.resolve();
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100, 200]);
    calls[1].deferred.resolve();
    await settle();

    expect(getMaxConcurrentInvokes()).toBe(1);
    oldLease.dispose();
    newLease.dispose();
    expect(coordinator.snapshot("session-1").pendingBytes).toBe(0);
  });

  it("finishes an old generation compensation before sending the new generation", async () => {
    const { calls, coordinator, getMaxConcurrentInvokes } = createHarness();
    const oldLease = coordinator.acquire("session-1", 1);
    oldLease.ack(100);
    await settle();
    const newLease = coordinator.acquire("session-1", 2);
    newLease.ack(200);

    calls[0].deferred.reject(new Error("old generation failed"));
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100, 100]);
    calls[1].deferred.resolve();
    await settle();
    expect(calls.map(({ bytes }) => bytes)).toEqual([100, 100, 200]);
    calls[2].deferred.resolve();
    await settle();

    expect(getMaxConcurrentInvokes()).toBe(1);
    oldLease.dispose();
    newLease.dispose();
  });

  it("collapses a long-running ACK burst without losing bytes", async () => {
    const { calls, coordinator, getMaxConcurrentInvokes } = createHarness();
    const lease = coordinator.acquire("session-stress", 1);

    for (let index = 0; index < 10_000; index += 1) {
      lease.ack(1024);
    }
    await settle();
    expect(calls).toHaveLength(1);

    calls[0].deferred.resolve();
    await settle();
    expect(calls).toHaveLength(2);
    calls[1].deferred.resolve();
    await settle();

    expect(calls.reduce((total, call) => total + call.bytes, 0)).toBe(10_240_000);
    expect(calls.length).toBeLessThan(10_000);
    expect(getMaxConcurrentInvokes()).toBe(1);
    expect(coordinator.snapshot("session-stress").pendingBytes).toBe(0);
    lease.dispose();
    expect(coordinator.snapshot("session-stress").retryTimers).toBe(0);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createXTerminalAutoReconnectController } from "./xterminalAutoReconnectController";

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("createXTerminalAutoReconnectController", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function createHarness(attemptReconnect = vi.fn().mockResolvedValue(true)) {
    const reconnectingRef = { current: false };
    const controller = createXTerminalAutoReconnectController({
      reconnectingRef,
      attemptReconnect,
      getRetryDelayMs: () => 5_000,
    });
    return { controller, attemptReconnect, reconnectingRef };
  }

  it("attempts immediately when automatic reconnect starts", async () => {
    const { controller, attemptReconnect } = createHarness();

    await expect(controller.startAutoReconnect()).resolves.toBe(true);

    expect(attemptReconnect).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("retries after the configured delay when an automatic attempt fails", async () => {
    const attemptReconnect = vi.fn().mockResolvedValue(false);
    const { controller } = createHarness(attemptReconnect);

    await controller.startAutoReconnect();
    expect(attemptReconnect).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(4_999);
    expect(attemptReconnect).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(attemptReconnect).toHaveBeenCalledTimes(2);

    controller.cancel();
  });

  it("stops retrying after a successful attempt", async () => {
    const attemptReconnect = vi
      .fn()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    const { controller } = createHarness(attemptReconnect);

    await controller.startAutoReconnect();
    await vi.advanceTimersByTimeAsync(5_000);
    await vi.advanceTimersByTimeAsync(20_000);

    expect(attemptReconnect).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not start a second reconnect while one is in flight", async () => {
    const deferred = createDeferred<boolean>();
    const attemptReconnect = vi.fn(() => deferred.promise);
    const { controller, reconnectingRef } = createHarness(attemptReconnect);

    const firstAttempt = controller.startAutoReconnect();
    const duplicateAttempt = controller.attemptNow();

    expect(attemptReconnect).toHaveBeenCalledTimes(1);
    expect(reconnectingRef.current).toBe(true);
    await expect(duplicateAttempt).resolves.toBe(false);

    deferred.resolve(true);
    await expect(firstAttempt).resolves.toBe(true);
    expect(reconnectingRef.current).toBe(false);
  });

  it("cancels a pending automatic retry", async () => {
    const attemptReconnect = vi.fn().mockResolvedValue(false);
    const { controller } = createHarness(attemptReconnect);

    await controller.startAutoReconnect();
    controller.cancel();
    await vi.advanceTimersByTimeAsync(20_000);

    expect(attemptReconnect).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("allows a manual attempt to run immediately without enabling retries", async () => {
    const attemptReconnect = vi.fn().mockResolvedValue(false);
    const { controller } = createHarness(attemptReconnect);

    await expect(controller.attemptNow()).resolves.toBe(false);
    await vi.advanceTimersByTimeAsync(20_000);

    expect(attemptReconnect).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("runs a manual attempt immediately while an automatic retry is pending", async () => {
    const attemptReconnect = vi.fn().mockResolvedValue(false);
    const { controller } = createHarness(attemptReconnect);

    await controller.startAutoReconnect();
    expect(attemptReconnect).toHaveBeenCalledTimes(1);

    await controller.attemptNow();
    expect(attemptReconnect).toHaveBeenCalledTimes(2);

    controller.cancel();
  });
});

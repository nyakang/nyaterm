import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RemoteStats } from "@/types/global";
import { useRemoteStats } from "./useRemoteStats";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("useRemoteStats session ownership", () => {
  it("hides session A stats immediately while session B is pending", async () => {
    const sessionB = deferred<RemoteStats>();
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) =>
      args.sessionId === "session-a"
        ? Promise.resolve(remoteStats("Ubuntu", "aggregate"))
        : sessionB.promise,
    );

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteStats(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );

    await waitFor(() => expect(result.current.stats?.system.os).toBe("Ubuntu"));

    rerender({ sessionId: "session-b" });

    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.stats).toBeNull();
    expect(result.current.error).toBe(false);
  });

  it("keeps session B when session A resolves after it", async () => {
    const sessionA = deferred<RemoteStats>();
    const sessionB = deferred<RemoteStats>();
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) =>
      args.sessionId === "session-a" ? sessionA.promise : sessionB.promise,
    );

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteStats(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    rerender({ sessionId: "session-b" });

    await act(async () => sessionB.resolve(remoteStats("SwitchOS", "aggregate")));
    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.stats?.system.os).toBe("SwitchOS");

    await act(async () => sessionA.resolve(remoteStats("Ubuntu", "aggregate")));
    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.stats?.system.os).toBe("SwitchOS");
  });

  it("never exposes session A stats while session B requests fail", async () => {
    let sessionBCalls = 0;
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) => {
      if (args.sessionId === "session-a") {
        return Promise.resolve(remoteStats("Ubuntu", "aggregate"));
      }
      sessionBCalls += 1;
      return Promise.reject(new Error(`failure-${sessionBCalls}`));
    });

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteStats(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    await waitFor(() => expect(result.current.stats?.system.os).toBe("Ubuntu"));

    rerender({ sessionId: "session-b" });
    await waitFor(() => expect(sessionBCalls).toBe(1));
    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.stats).toBeNull();

    for (let expectedCalls = 2; expectedCalls <= 3; expectedCalls += 1) {
      act(() => result.current.refresh());
      await waitFor(() => expect(sessionBCalls).toBe(expectedCalls));
      expect(result.current.sessionId).toBe("session-b");
      expect(result.current.stats).toBeNull();
    }
  });

  it("preserves current-session stats until the existing failure threshold is reached", async () => {
    let calls = 0;
    mocks.invoke.mockImplementation(() => {
      calls += 1;
      return calls === 1
        ? Promise.resolve(remoteStats("Ubuntu", "aggregate"))
        : Promise.reject(new Error(`failure-${calls}`));
    });

    const { result } = renderHook(() => useRemoteStats("session-a", true, 60));
    await waitFor(() => expect(result.current.stats?.system.os).toBe("Ubuntu"));

    for (let failure = 1; failure <= 3; failure += 1) {
      act(() => result.current.refresh());
      await waitFor(() => expect(calls).toBe(failure + 1));
      await act(async () => Promise.resolve());
      expect(result.current.error).toBe(true);
      if (failure < 3) {
        expect(result.current.stats?.system.os).toBe("Ubuntu");
      } else {
        expect(result.current.stats).toBeNull();
      }
    }
  });

  it("rejects stale responses and warmup timers across a rapid A to B to A switch", async () => {
    vi.useFakeTimers();
    const firstA = deferred<RemoteStats>();
    const sessionB = deferred<RemoteStats>();
    const finalA = deferred<RemoteStats>();
    const requests = [firstA.promise, sessionB.promise, finalA.promise];
    mocks.invoke.mockImplementation(() => requests.shift());

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteStats(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    rerender({ sessionId: "session-b" });
    rerender({ sessionId: "session-a" });
    expect(mocks.invoke).toHaveBeenCalledTimes(3);

    await act(async () => finalA.resolve(remoteStats("Final A", "aggregate")));
    await act(async () => firstA.resolve(remoteStats("Stale A", "warming_up")));
    await act(async () => sessionB.resolve(remoteStats("Stale B", "aggregate")));

    expect(result.current.sessionId).toBe("session-a");
    expect(result.current.stats?.system.os).toBe("Final A");

    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
  });

  it("does not let an old manual refresh completion clear the new session state", async () => {
    const oldManualRefresh = deferred<RemoteStats>();
    let sessionACalls = 0;
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) => {
      if (args.sessionId === "session-a") {
        sessionACalls += 1;
        return sessionACalls === 1
          ? Promise.resolve(remoteStats("Initial A", "aggregate"))
          : oldManualRefresh.promise;
      }
      return Promise.resolve(remoteStats("Session B", "aggregate"));
    });

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteStats(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    await waitFor(() => expect(result.current.stats?.system.os).toBe("Initial A"));

    act(() => result.current.refresh());
    expect(result.current.isManualRefreshing).toBe(true);
    rerender({ sessionId: "session-b" });
    await waitFor(() => expect(result.current.stats?.system.os).toBe("Session B"));
    expect(result.current.isManualRefreshing).toBe(false);

    await act(async () => oldManualRefresh.resolve(remoteStats("Stale manual A", "aggregate")));
    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.stats?.system.os).toBe("Session B");
    expect(result.current.isManualRefreshing).toBe(false);
  });
});

function remoteStats(os: string, usageSource: RemoteStats["cpu"]["usage_source"]): RemoteStats {
  return {
    system: { hostname: os.toLowerCase(), uptime_sec: 100, os, arch: "x86_64" },
    load: { load1: 0.1, load5: 0.2, load15: 0.3 },
    cpu: {
      model: "CPU",
      cores: 8,
      usage: 10,
      per_core: [],
      sample_window_ms: 1000,
      usage_source: usageSource,
    },
    memory: { used: 1024, available: 1024, cached: 0 },
    networks: [],
    network_summary: { rx_bytes_per_sec: 0, tx_bytes_per_sec: 0 },
    disks: [],
  };
}

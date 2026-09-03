import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RemoteGpuOverview, RemoteNpuOverview } from "@/types/global";
import { useRemoteGpuOverview } from "./useRemoteGpuOverview";
import { useRemoteNpuOverview } from "./useRemoteNpuOverview";
import { useRemoteStats } from "./useRemoteStats";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

afterEach(() => {
  vi.clearAllMocks();
});

describe("remote accelerator overview session ownership", () => {
  it("does not expose or persist a stale GPU overview after switching sessions", async () => {
    const sessionA = deferred<RemoteGpuOverview>();
    const sessionB = deferred<RemoteGpuOverview>();
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) =>
      args.sessionId === "session-a" ? sessionA.promise : sessionB.promise,
    );

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteGpuOverview(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    rerender({ sessionId: "session-b" });

    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.overview).toBeNull();

    await act(async () => sessionB.resolve(gpuOverview("GPU B")));
    await act(async () => sessionA.resolve(gpuOverview("GPU A")));

    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.overview?.gpus[0]?.name).toBe("GPU B");
  });

  it("does not expose or persist a stale NPU overview after switching sessions", async () => {
    const sessionA = deferred<RemoteNpuOverview>();
    const sessionB = deferred<RemoteNpuOverview>();
    mocks.invoke.mockImplementation((_command: string, args: { sessionId: string }) =>
      args.sessionId === "session-a" ? sessionA.promise : sessionB.promise,
    );

    const { result, rerender } = renderHook(
      ({ sessionId }) => useRemoteNpuOverview(sessionId, true, 60),
      { initialProps: { sessionId: "session-a" } },
    );
    rerender({ sessionId: "session-b" });

    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.overview).toBeNull();

    await act(async () => sessionB.resolve(npuOverview("NPU B")));
    await act(async () => sessionA.resolve(npuOverview("NPU A")));

    expect(result.current.sessionId).toBe("session-b");
    expect(result.current.overview?.npus[0]?.name).toBe("NPU B");
  });

  it("does not invoke any monitoring command while monitoring is disabled", () => {
    const stats = renderHook(() => useRemoteStats("network-session", false, 60));
    const gpu = renderHook(() => useRemoteGpuOverview("network-session", false, 60));
    const npu = renderHook(() => useRemoteNpuOverview("network-session", false, 60));

    expect(stats.result.current.sessionId).toBeNull();
    expect(gpu.result.current.sessionId).toBeNull();
    expect(npu.result.current.sessionId).toBeNull();
    expect(mocks.invoke).not.toHaveBeenCalled();

    stats.unmount();
    gpu.unmount();
    npu.unmount();
  });
});

function gpuOverview(name: string): RemoteGpuOverview {
  return {
    available: true,
    driver_version: "550",
    cuda_version: "12.4",
    gpus: [
      {
        index: 0,
        uuid: name,
        name,
        memory_total_mb: 1024,
        memory_used_mb: 128,
        memory_free_mb: 896,
        pstate: "P0",
      },
    ],
    processes: [],
  };
}

function npuOverview(name: string): RemoteNpuOverview {
  return {
    available: true,
    driver_version: "24",
    cann_version: "8",
    npus: [
      {
        index: 0,
        chip_id: 0,
        device_key: name,
        name,
        health: "OK",
        bus_id: "0000:01:00.0",
        memory_total_mb: 1024,
        memory_used_mb: 128,
        memory_free_mb: 896,
        memory_kind: "HBM",
      },
    ],
    processes: [],
  };
}

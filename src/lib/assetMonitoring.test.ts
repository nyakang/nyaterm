import { describe, expect, it } from "vitest";
import {
  type AssetMonitoringCacheEntry,
  buildAssetPatchFromGpuOverview,
  buildAssetPatchFromNpuOverview,
  buildAssetPatchFromRemoteStats,
  mergeMonitoringAssetPatch,
  recordAssetMonitoringPatch,
} from "@/lib/assetMonitoring";
import type {
  AssetMetadata,
  RemoteGpuOverview,
  RemoteNpuOverview,
  RemoteStats,
} from "@/types/global";

describe("asset monitoring cache", () => {
  it("maps successful remote stats into an asset patch", () => {
    const patch = buildAssetPatchFromRemoteStats(remoteStats());

    expect(patch).toEqual({
      hostname: "node-01",
      os_name: "Ubuntu Linux",
      architecture: "x86_64",
      cpu_model: "AMD EPYC",
      cpu_cores: 32,
      memory_bytes: 64 * 1024 ** 3,
      disks: [
        {
          model: "/dev/nvme0n1",
          capacity_bytes: 1024 * 1024 ** 3,
          count: 1,
        },
      ],
    });
  });

  it("records patches in memory without requiring a save callback", () => {
    const cache = new Map<string, AssetMonitoringCacheEntry>();

    recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-1",
      targetSessionId: "session-1",
      connectionId: "conn-1",
      patch: { hostname: "node-01" },
    });
    recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-1",
      targetSessionId: "session-1",
      connectionId: "conn-1",
      patch: { cpu_cores: 16 },
    });
    recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-2",
      targetSessionId: "session-2",
      connectionId: "conn-2",
      patch: null,
    });

    expect(cache.get("session-1")).toEqual({
      sessionId: "session-1",
      connectionId: "conn-1",
      lastAssetPatch: {
        hostname: "node-01",
        cpu_cores: 16,
      },
    });
    expect(cache.has("session-2")).toBe(false);
  });

  it("rejects patches whose source session does not match the target session", () => {
    const cache = new Map<string, AssetMonitoringCacheEntry>();

    const recorded = recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-a",
      targetSessionId: "session-b",
      connectionId: "conn-b",
      patch: { os_name: "Ubuntu" },
    });

    expect(recorded).toBe(false);
    expect(cache.size).toBe(0);
  });

  it("does not merge a previous connection patch when a session key is rebound", () => {
    const cache = new Map<string, AssetMonitoringCacheEntry>();

    recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-1",
      targetSessionId: "session-1",
      connectionId: "conn-old",
      patch: { os_name: "Ubuntu", cpu_cores: 8 },
    });
    recordAssetMonitoringPatch(cache, {
      sourceSessionId: "session-1",
      targetSessionId: "session-1",
      connectionId: "conn-new",
      patch: { hostname: "switch-1" },
    });

    expect(cache.get("session-1")).toEqual({
      sessionId: "session-1",
      connectionId: "conn-new",
      lastAssetPatch: { hostname: "switch-1" },
    });
  });

  it("maps GPU and NPU overviews to their own accelerator types", () => {
    expect(buildAssetPatchFromGpuOverview(gpuOverview())?.accelerators).toEqual([
      {
        type: "gpu",
        vendor: "NVIDIA",
        model: "H100",
        memory_bytes: 80 * 1024 ** 3,
        count: 2,
      },
    ]);
    expect(buildAssetPatchFromNpuOverview(npuOverview())?.accelerators).toEqual([
      {
        type: "npu",
        vendor: "Huawei",
        model: "Ascend 910B",
        memory_bytes: 32 * 1024 ** 3,
        count: 1,
      },
    ]);
  });

  it("merges accelerator patches by type without wiping other monitored types", () => {
    const current: AssetMetadata = {
      accelerators: [
        { type: "gpu", vendor: "NVIDIA", model: "A100", count: 1 },
        { type: "npu", vendor: "Huawei", model: "Ascend 910B", count: 1 },
      ],
      tags: ["keep"],
      notes: "keep notes",
    };

    const next = mergeMonitoringAssetPatch(current, {
      accelerators: [{ type: "gpu", vendor: "NVIDIA", model: "H100", count: 4 }],
      tags: ["ignored"],
      notes: "ignored",
    });

    expect(next.accelerators).toEqual([
      { type: "npu", vendor: "Huawei", model: "Ascend 910B", count: 1 },
      { type: "gpu", vendor: "NVIDIA", model: "H100", count: 4 },
    ]);
    expect(next.tags).toEqual(["keep"]);
    expect(next.notes).toBe("keep notes");
  });

  it("does not cache unavailable accelerator snapshots", () => {
    expect(buildAssetPatchFromGpuOverview({ ...gpuOverview(), available: false })).toBeNull();
    expect(buildAssetPatchFromNpuOverview({ ...npuOverview(), npus: [] })).toBeNull();
  });
});

function remoteStats(): RemoteStats {
  return {
    system: {
      hostname: "node-01",
      uptime_sec: 100,
      os: "Ubuntu Linux",
      arch: "x86_64",
    },
    load: {
      load1: 0.1,
      load5: 0.2,
      load15: 0.3,
    },
    cpu: {
      model: "AMD EPYC",
      cores: 32,
      usage: 12,
      per_core: [],
      sample_window_ms: 1000,
      usage_source: "aggregate",
    },
    memory: {
      used: 32 * 1024 ** 3,
      available: 32 * 1024 ** 3,
      cached: 0,
    },
    networks: [],
    network_summary: {
      rx_bytes_per_sec: 0,
      tx_bytes_per_sec: 0,
    },
    disks: [
      {
        device: "/dev/nvme0n1",
        mount: "/",
        total: 1024 * 1024 ** 3,
        available: 512 * 1024 ** 3,
        use_percent: 50,
      },
    ],
  };
}

function gpuOverview(): RemoteGpuOverview {
  return {
    available: true,
    driver_version: "550",
    cuda_version: "12.4",
    gpus: [
      {
        index: 0,
        uuid: "gpu-0",
        name: "H100",
        memory_total_mb: 80 * 1024,
        memory_used_mb: 1024,
        memory_free_mb: 79 * 1024,
        pstate: "P0",
      },
      {
        index: 1,
        uuid: "gpu-1",
        name: "H100",
        memory_total_mb: 80 * 1024,
        memory_used_mb: 1024,
        memory_free_mb: 79 * 1024,
        pstate: "P0",
      },
    ],
    processes: [],
  };
}

function npuOverview(): RemoteNpuOverview {
  return {
    available: true,
    driver_version: "24",
    cann_version: "8",
    npus: [
      {
        index: 0,
        chip_id: 0,
        device_key: "0:0",
        name: "Ascend 910B",
        health: "OK",
        bus_id: "0000:01:00.0",
        memory_total_mb: 32 * 1024,
        memory_used_mb: 1024,
        memory_free_mb: 31 * 1024,
        memory_kind: "HBM",
      },
    ],
    processes: [],
  };
}

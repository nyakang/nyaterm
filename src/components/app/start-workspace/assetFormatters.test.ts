import { describe, expect, it } from "vitest";
import type { AssetMetadata, SavedConnection } from "@/types/global";
import {
  buildAssetSearchText,
  compareAssetAddress,
  formatAccelerators,
  formatBytes,
  formatDiskSummary,
  getDiskTotalBytes,
} from "./assetFormatters";

describe("assetFormatters", () => {
  it("formats bytes and keeps undefined distinct from confirmed empty arrays", () => {
    expect(formatBytes(16 * 1024 ** 3)).toBe("16 GB");
    expect(formatBytes(undefined)).toBe("-");
    expect(formatAccelerators(undefined)).toBe("-");
    expect(formatAccelerators([])).toBe("None");
  });

  it("summarizes disk capacity with device counts", () => {
    const disks = [
      { capacity_bytes: 512 * 1024 ** 3, count: 2 },
      { capacity_bytes: 1 * 1024 ** 4, count: 1 },
    ];

    expect(getDiskTotalBytes(disks)).toBe(2 * 1024 ** 4);
    expect(formatDiskSummary(disks)).toBe("2.0 TB");
    expect(formatDiskSummary([])).toBe("None");
    expect(formatDiskSummary(undefined)).toBe("-");
  });

  it("sorts IPv4 addresses numerically before non-IP addresses", () => {
    expect(["host-2", "10.0.0.11", "10.0.0.2"].sort(compareAssetAddress)).toEqual([
      "10.0.0.2",
      "10.0.0.11",
      "host-2",
    ]);
  });

  it("builds searchable text from CPU, accelerators, tags and group path", () => {
    const searchText = buildAssetSearchText(
      {
        ...connection({
          cpu_model: "EPYC 9654",
          accelerators: [
            { type: "gpu", vendor: "NVIDIA", model: "H100" },
            { type: "npu", vendor: "Huawei", model: "Ascend 910B" },
          ],
        }),
        tags: ["Inference", "Production"],
      },
      "Assets / Lab",
    );

    expect(searchText).toContain("epyc");
    expect(searchText).toContain("h100");
    expect(searchText).toContain("ascend");
    expect(searchText).toContain("inference production");
    expect(searchText).toContain("assets / lab");
  });

  it("handles connections without asset metadata", () => {
    expect(buildAssetSearchText(connection(undefined), "Assets")).toContain("gpu-node");
  });
});

function connection(asset: AssetMetadata | undefined): SavedConnection {
  return {
    id: "conn-1",
    name: "gpu-node",
    type: "ssh",
    host: "10.0.0.2",
    port: 22,
    username: "root",
    asset,
  };
}

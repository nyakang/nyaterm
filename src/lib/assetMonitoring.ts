import type {
  AssetAccelerator,
  AssetAcceleratorType,
  AssetDisk,
  AssetMetadata,
  RemoteGpuOverview,
  RemoteNpuOverview,
  RemoteStats,
} from "@/types/global";

export interface AssetMonitoringCacheEntry {
  sessionId: string;
  connectionId: string;
  lastAssetPatch: AssetMetadata;
}

export interface RecordAssetMonitoringPatchOptions {
  sourceSessionId: string;
  targetSessionId: string;
  connectionId: string;
  patch: AssetMetadata | null;
}

const UNKNOWN_VALUES = new Set(["", "-", "unknown", "n/a", "null"]);
const BYTES_PER_MB = 1024 ** 2;

function cleanText(value: string | null | undefined): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed) return undefined;
  return UNKNOWN_VALUES.has(trimmed.toLowerCase()) ? undefined : trimmed;
}

function optionalNonNegativeInteger(value: number | null | undefined): number | undefined {
  if (value == null || !Number.isFinite(value) || value < 0) return undefined;
  return Math.floor(value);
}

function hasMonitoringAssetPatchValue(patch: AssetMetadata): boolean {
  return (
    patch.hostname !== undefined ||
    patch.os_name !== undefined ||
    patch.architecture !== undefined ||
    patch.cpu_model !== undefined ||
    patch.cpu_cores !== undefined ||
    patch.memory_bytes !== undefined ||
    patch.disks !== undefined ||
    patch.accelerators !== undefined
  );
}

export function buildAssetPatchFromRemoteStats(stats: RemoteStats): AssetMetadata | null {
  const memoryBytes = optionalNonNegativeInteger(stats.memory.used + stats.memory.available);
  const disks: AssetDisk[] = stats.disks
    .map((disk) => ({
      model: cleanText(disk.device) ?? cleanText(disk.mount),
      capacity_bytes: optionalNonNegativeInteger(disk.total),
      count: 1,
    }))
    .filter((disk) => disk.model || disk.capacity_bytes !== undefined);

  const patch: AssetMetadata = {
    hostname: cleanText(stats.system.hostname),
    os_name: cleanText(stats.system.os),
    architecture: cleanText(stats.system.arch),
    cpu_model: cleanText(stats.cpu.model),
    cpu_cores: optionalNonNegativeInteger(stats.cpu.cores),
    memory_bytes: memoryBytes && memoryBytes > 0 ? memoryBytes : undefined,
    disks,
  };

  return hasMonitoringAssetPatchValue(patch) ? patch : null;
}

export function buildAssetPatchFromGpuOverview(overview: RemoteGpuOverview): AssetMetadata | null {
  if (!overview.available || overview.gpus.length === 0) return null;

  const accelerators = groupAccelerators(
    overview.gpus.map((gpu) => ({
      type: "gpu",
      vendor: "NVIDIA",
      model: cleanText(gpu.name),
      memory_bytes: optionalNonNegativeInteger(gpu.memory_total_mb * BYTES_PER_MB),
      count: 1,
    })),
  );

  return accelerators.length > 0 ? { accelerators } : null;
}

export function buildAssetPatchFromNpuOverview(overview: RemoteNpuOverview): AssetMetadata | null {
  if (!overview.available || overview.npus.length === 0) return null;

  const accelerators = groupAccelerators(
    overview.npus.map((npu) => ({
      type: "npu",
      vendor: "Huawei",
      model: cleanText(npu.name),
      memory_bytes: optionalNonNegativeInteger(npu.memory_total_mb * BYTES_PER_MB),
      count: 1,
    })),
  );

  return accelerators.length > 0 ? { accelerators } : null;
}

function groupAccelerators(accelerators: AssetAccelerator[]): AssetAccelerator[] {
  const grouped = new Map<string, AssetAccelerator>();

  for (const accelerator of accelerators) {
    const type = accelerator.type;
    const vendor = cleanText(accelerator.vendor);
    const model = cleanText(accelerator.model);
    const memoryBytes = optionalNonNegativeInteger(accelerator.memory_bytes);
    const key = `${type}\u0000${vendor ?? ""}\u0000${model ?? ""}\u0000${memoryBytes ?? ""}`;
    const existing = grouped.get(key);

    if (existing) {
      existing.count = (existing.count ?? 1) + (accelerator.count ?? 1);
    } else {
      grouped.set(key, {
        type,
        vendor,
        model,
        memory_bytes: memoryBytes,
        count: accelerator.count ?? 1,
      });
    }
  }

  return [...grouped.values()];
}

export function mergeMonitoringAssetPatch(
  current: AssetMetadata | undefined,
  patch: AssetMetadata,
): AssetMetadata {
  const next: AssetMetadata = { ...(current ?? {}) };
  assignIfPresent(next, patch, "hostname");
  assignIfPresent(next, patch, "os_name");
  assignIfPresent(next, patch, "architecture");
  assignIfPresent(next, patch, "cpu_model");
  assignIfPresent(next, patch, "cpu_cores");
  assignIfPresent(next, patch, "memory_bytes");
  assignIfPresent(next, patch, "disks");
  assignIfPresent(next, patch, "updated_at");

  if (patch.accelerators !== undefined) {
    next.accelerators = mergeAcceleratorTypes(next.accelerators, patch.accelerators);
  }

  return next;
}

function assignIfPresent<TKey extends keyof AssetMetadata>(
  target: AssetMetadata,
  source: AssetMetadata,
  key: TKey,
) {
  if (source[key] !== undefined) {
    target[key] = source[key];
  }
}

function mergeAcceleratorTypes(
  current: AssetAccelerator[] | undefined,
  patch: AssetAccelerator[],
): AssetAccelerator[] {
  if (patch.length === 0) return current ?? [];

  const patchTypes = new Set<AssetAcceleratorType>(patch.map((accelerator) => accelerator.type));
  return [...(current ?? []).filter((accelerator) => !patchTypes.has(accelerator.type)), ...patch];
}

export function recordAssetMonitoringPatch(
  cache: Map<string, AssetMonitoringCacheEntry>,
  { sourceSessionId, targetSessionId, connectionId, patch }: RecordAssetMonitoringPatchOptions,
): boolean {
  if (!patch || sourceSessionId !== targetSessionId) return false;

  const current = cache.get(targetSessionId);
  const canMerge = current?.sessionId === targetSessionId && current.connectionId === connectionId;
  cache.set(targetSessionId, {
    sessionId: targetSessionId,
    connectionId,
    lastAssetPatch: mergeMonitoringAssetPatch(canMerge ? current.lastAssetPatch : undefined, patch),
  });
  return true;
}
